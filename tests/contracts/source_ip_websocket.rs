//! Real TCP peer collection for both platform WebSocket dispatchers, and the
//! trusted-proxy substitution shared with the HTTP ingress.
use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use sumpter_core::config::AppConfig;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn websocket_config(upstream_address: std::net::SocketAddr, trusted: &[&str]) -> AppConfig {
    let mut listener = serde_json::json!({
        "host": "127.0.0.1", "port": 0, "authToken": "listener-secret"
    });
    if !trusted.is_empty() {
        listener["trustedProxyCIDRs"] = serde_json::json!(trusted);
    }
    AppConfig::from_json(
        &serde_json::json!({
            "schemaVersion": 6,
            "listener": listener,
            "retry": {"maxDeferredRounds": 0, "sessionStickyRetries": 0},
            "endpoints": [{
                "id": "source-ip", "name": "source-ip", "baseURL": format!("http://{upstream_address}"),
                "apiKey": "upstream-secret", "protocol": "openai-responses", "enabled": true,
                "mappings": [
                    {"clientPattern": "gpt-4o", "upstreamModel": "gpt-4o"},
                    {"clientPattern": "gpt-realtime", "upstreamModel": "gpt-realtime", "capabilities": ["live"]}
                ]
            }]
        })
        .to_string(),
    )
    .unwrap()
    .normalized()
}

async fn relay_and_reject(path: &str, trusted: &[&str], expected_ip: &str) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        let frame = socket.next().await.unwrap().unwrap();
        socket.send(frame).await.unwrap();
        let _ = socket.close(None).await;
    });
    let engine = pi_test_engine(websocket_config(upstream_address, trusted));
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    request
        .headers_mut()
        .insert("x-forwarded-for", "198.51.100.99".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    socket.send(Message::Text("hello".into())).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text("hello".into())
    );
    let _ = socket.close(None).await;
    task.await.unwrap();
    for _ in 0..100 {
        if engine.runtime_snapshot().recent_events.len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let events = engine.runtime_snapshot().recent_events;
    assert_eq!(events.len(), 2);
    for event in events {
        assert_eq!(event.source_ip.as_deref(), Some(expected_ip), "{path}");
    }
    // Early rejection (missing token) with the same forwarded header: auth still
    // fails and the rejection is attributed with the same rule.
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-forwarded-for", "198.51.100.99".parse().unwrap());
    let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(matches!(
        error,
        tokio_tungstenite::tungstenite::Error::Http(_)
    ));
    let events = engine.runtime_snapshot().recent_events;
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].status_code, 401);
    assert_eq!(events[0].source_ip.as_deref(), Some(expected_ip));
    pi_test_shutdown(handle).await;
}

#[tokio::test]
async fn source_ip_websocket_relay_and_early_rejections() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for path in [
            "/v1/responses?model=gpt-4o",
            "/v1/realtime?model=gpt-realtime",
        ] {
            relay_and_reject(path, &[], "127.0.0.1").await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn source_ip_websocket_uses_forwarded_client_from_trusted_proxy() {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        for path in [
            "/v1/responses?model=gpt-4o",
            "/v1/realtime?model=gpt-realtime",
        ] {
            relay_and_reject(path, &["127.0.0.1"], "198.51.100.99").await;
        }
    })
    .await
    .unwrap();
}
