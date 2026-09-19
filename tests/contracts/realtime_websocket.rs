//! Public Realtime model forwarding shared by both platform adapters.
use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use sumpter_core::config::AppConfig;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn realtime_config(upstream_address: std::net::SocketAddr) -> AppConfig {
    AppConfig::from_json(
        &serde_json::json!({
            "schemaVersion": 6,
            "listener": {"host": "127.0.0.1", "port": 0, "authToken": "listener-secret"},
            "retry": {"maxDeferredRounds": 0, "sessionStickyRetries": 0},
            "endpoints": [{
                "id": "cpa",
                "name": "CPA",
                "baseURL": format!("http://{upstream_address}"),
                "apiKey": "provider-secret",
                "protocol": "openai",
                "enabled": true,
                "mappings": [
                    {
                        "clientPattern": "gpt-live-1-codex",
                        "upstreamModel": "gpt-live-1-codex",
                        "capabilities": ["live"]
                    },
                    {
                        "clientPattern": "gpt-realtime-2.1",
                        "upstreamModel": "gpt-realtime-2.1",
                        "capabilities": ["live"]
                    }
                ]
            }]
        })
        .to_string(),
    )
    .unwrap()
    .normalized()
}

#[tokio::test]
async fn public_realtime_21_model_is_preserved_to_the_upstream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let (target_tx, target_rx) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
            let _ = target_tx.send(request.uri().to_string());
            Ok(response)
        })
        .await
        .unwrap();
        socket
            .send(Message::Text(r#"{"type":"session.created"}"#.into()))
            .await
            .unwrap();
        let _ = socket.close(None).await;
    });

    let engine = pi_test_engine(realtime_config(upstream_address));
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut request = format!("ws://{address}/v1/realtime?model=gpt-realtime-2.1")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    assert_eq!(
        target_rx.await.unwrap(),
        "/v1/realtime?model=gpt-realtime-2.1"
    );
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text(r#"{"type":"session.created"}"#.into())
    );

    let _ = socket.close(None).await;
    upstream_task.await.unwrap();
    pi_test_shutdown(handle).await;
}
