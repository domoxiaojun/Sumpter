//! Public Realtime model forwarding shared by both platform adapters.
use super::{http_only_test_engine, pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use sumpter_core::config::AppConfig;
use sumpter_core::events::{RuntimeFailureKind, RuntimeFailurePhase};
use sumpter_engine::replay::{ReplayReply, ReplayTransport};
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
                "capabilities": ["live"],
                "mappings": []
            }],
            "modelGroups": [{
                "id": "default",
                "name": "default",
                "enabled": true,
                "priority": 0,
                "models": [],
                "bindings": [{
                    "endpointID": "cpa",
                    "enabled": true,
                    "priority": 0,
                    "models": []
                }]
            }]
        })
        .to_string(),
    )
    .unwrap()
    .normalized()
}

#[tokio::test]
async fn codex_client_owned_call_bootstrap_binds_the_follow_up_websocket() {
    tokio::time::timeout(Duration::from_secs(10), async {
        for (bootstrap, sideband) in [
            ("/wham/realtime/calls", "/v1/live/rtc_desktop"),
            ("/backend-api/wham/realtime/calls", "/v1/live/rtc_desktop"),
            ("/wham/realtime/calls", "/wham/realtime/calls/rtc_desktop"),
            ("/v1/live", "/v1/live/rtc_desktop"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_address = listener.local_addr().unwrap();
            let (sent, received) = tokio::sync::oneshot::channel();
            let upstream = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut sent = Some(sent);
                #[allow(clippy::result_large_err)]
                let mut socket = accept_hdr_async(stream, move |request: &Request, response: Response| {
                    sent.take().unwrap().send((request.uri().to_string(), request.headers().clone())).unwrap();
                    Ok(response)
                }).await.unwrap();
                let frame = socket.next().await.unwrap().unwrap();
                socket.send(frame).await.unwrap();
                let _ = socket.close(None).await;
            });
            let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply {
                status: 201,
                headers: vec![
                    ("content-type".into(), "application/sdp".into()),
                    ("location".into(), "/v1/realtime/calls/rtc_desktop".into()),
                ],
                chunks: vec![Ok(bytes::Bytes::from_static(b"v=0\r\nanswer\r\n"))],
            })]));
            let engine = http_only_test_engine(realtime_config(upstream_address), transport.clone());
            let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap()).await.unwrap();
            let body = r#"{ "sdp":"v=0\r\noffer\r\n", "session":{"model":"gpt-live-1.1-codex","delegation":{"type":"client"},"audio":{"output":{"voice":"marin"}},"initial_items":[],"voice_session_id":"synthetic-voice","future_field":{"preserve":true}} }"#;
            let query = "intent=quicksilver&architecture=avas&trace=keep%2Bvalue";
            let response = reqwest::Client::builder().no_proxy().build().unwrap()
                .post(format!("http://{address}{bootstrap}?{query}"))
                .header("authorization", "Bearer listener-secret")
                .header("content-type", "application/json")
                .header("accept", "application/sdp")
                .header("openai-alpha", "quicksilver=v2")
                .header("thread-id", "synthetic-thread")
                .body(body).send().await.unwrap();
            assert_eq!(response.status().as_u16(), 201);
            assert_eq!(response.headers()["location"], "/v1/realtime/calls/rtc_desktop");
            assert_eq!(response.bytes().await.unwrap().as_ref(), b"v=0\r\nanswer\r\n");
            let calls = transport.calls();
            assert_eq!(calls.len(), 1);
            let expected_path = if bootstrap == "/v1/live" { "/v1/live" } else { "/v1/realtime/calls" };
            assert_eq!(calls[0].path_and_query, format!("{expected_path}?{query}"));
            assert_eq!(calls[0].body.as_ref(), body.as_bytes());
            for (name, value) in [("authorization", "Bearer provider-secret"), ("openai-alpha", "quicksilver=v2"), ("thread-id", "synthetic-thread")] {
                assert!(calls[0].headers.iter().any(|(key, actual)| key.eq_ignore_ascii_case(name) && actual == value));
            }
            let mut request = format!("ws://{address}{sideband}").into_client_request().unwrap();
            request.headers_mut().insert("authorization", "Bearer listener-secret".parse().unwrap());
            let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
            let frame = Message::Binary(b"opaque-v3-control".as_slice().into());
            socket.send(frame.clone()).await.unwrap();
            assert_eq!(socket.next().await.unwrap().unwrap(), frame);
            let (target, headers) = received.await.unwrap();
            assert_eq!(target, if sideband.starts_with("/wham/") { "/v1/realtime/calls/rtc_desktop" } else { sideband });
            assert_eq!(headers["authorization"], "Bearer provider-secret");
            let _ = socket.close(None).await;
            upstream.await.unwrap();
            for _ in 0..100 {
                if engine.runtime_snapshot().recent_events.iter().any(|event| event.request_path.as_deref() == Some(sideband)) { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let events = engine.runtime_snapshot().recent_events;
            let control = events.iter().find(|event| event.kind == "client" && event.request_path.as_deref() == Some(sideband)).unwrap();
            assert_eq!(control.effective_model.as_deref(), Some("gpt-live-1.1-codex"));
            assert_eq!(control.endpoint_id.as_deref(), Some("cpa"));
            pi_test_shutdown(handle).await;
        }
    }).await.expect("client-owned call proxy contract completed");
}

#[tokio::test]
async fn unknown_external_live_call_is_a_local_rejection_without_upstream_status() {
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = http_only_test_engine(
        realtime_config("127.0.0.1:9".parse().unwrap()),
        transport.clone(),
    );
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut request = format!("ws://{address}/v1/live/rtc_external")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .unwrap()
    .unwrap_err();
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        panic!("expected local HTTP rejection")
    };
    assert_eq!(response.status().as_u16(), 410);
    let events = engine.runtime_snapshot().recent_events;
    assert!(!events.iter().any(|event| event.kind == "upstream"));
    let event = events.iter().find(|event| event.kind == "client").unwrap();
    assert_eq!(
        event.failure_kind,
        Some(RuntimeFailureKind::ClientRequestRejected)
    );
    assert_eq!(
        event.failure_phase,
        Some(RuntimeFailurePhase::BeforeResponse)
    );
    assert_eq!(event.upstream_status_code, None);
    let trace = event
        .stream_trace
        .as_ref()
        .unwrap()
        .websocket_trace
        .as_ref()
        .unwrap();
    assert_eq!(trace.handshake_status, None);
    assert_eq!(trace.attempt_count, Some(0));
    assert_eq!(trace.stage.as_deref(), Some("local_rejection"));
    assert!(transport.calls().is_empty());
    pi_test_shutdown(handle).await;
}

#[tokio::test]
async fn realtime_upstream_http_rejection_keeps_its_upstream_status() {
    tokio::time::timeout(Duration::from_secs(5), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let engine = pi_test_engine(realtime_config(listener.local_addr().unwrap()));
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)]
            let result = accept_hdr_async(stream, |_: &Request, _: Response| {
                Err(tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(401)
                    .body(Some("synthetic denial".into()))
                    .unwrap())
            })
            .await;
            assert!(result.is_err());
        });
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut request = format!("ws://{address}/v1/realtime?model=gpt-realtime-2.1")
            .into_client_request()
            .unwrap();
        request
            .headers_mut()
            .insert("authorization", "Bearer listener-secret".parse().unwrap());
        let error = tokio_tungstenite::connect_async(request).await.unwrap_err();
        let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
            panic!("expected upstream HTTP rejection")
        };
        assert_eq!(response.status().as_u16(), 401);
        upstream.await.unwrap();
        let events = engine.runtime_snapshot().recent_events;
        assert!(events.iter().any(|event| event.kind == "upstream"));
        let event = events.iter().find(|event| event.kind == "client").unwrap();
        assert_eq!(
            event.failure_kind,
            Some(RuntimeFailureKind::UpstreamHttpStatus)
        );
        assert_eq!(event.upstream_status_code, Some(401));
        pi_test_shutdown(handle).await;
    })
    .await
    .expect("upstream rejection contract completed");
}

#[tokio::test]
async fn public_realtime_21_model_is_preserved_to_the_upstream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let (target_tx, target_rx) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        // Tungstenite fixes the callback error type to an unboxed HTTP response.
        #[allow(clippy::result_large_err)]
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

    let mut config = realtime_config(upstream_address);
    config.listener.auth_token.clear();
    config.listener.auth_tokens = vec![sumpter_core::config::InboundTokenConfig {
        id: "work".into(),
        name: "工作电脑".into(),
        enabled: true,
        token: "synthetic-work-token".into(),
    }];
    let engine = pi_test_engine(config);
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut request = format!("ws://{address}/v1/realtime?model=gpt-realtime-2.1")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "authorization",
        "Bearer synthetic-work-token".parse().unwrap(),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert!(
        engine
            .runtime_summary_value()
            .get("responsesWebSocketConnections")
            .is_none()
    );

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
    let events = engine.runtime_snapshot().recent_events;
    assert!(events.iter().any(|event| event.kind == "client"));
    for event in events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "client" | "upstream"))
    {
        assert_eq!(event.inbound_token.as_ref().unwrap().id, "work");
    }
}
