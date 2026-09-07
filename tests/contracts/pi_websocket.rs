//! Loopback wire test shared by both platform servers.
use super::server;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use sumpter_core::{config::AppConfig, events::ClientKind};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

#[tokio::test]
async fn pi_websocket_preserves_frames_and_attributes_both_sides() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_address = listener.local_addr().unwrap();
        let (sent, received) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut sent = Some(sent);
            // Tungstenite requires the full HTTP error response in this callback signature.
            #[allow(clippy::result_large_err)]
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, move |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                sent.take().unwrap().send(request.headers().clone()).unwrap();
                Ok(response)
            }).await.unwrap();
            let frame = socket.next().await.unwrap().unwrap();
            socket.send(frame.clone()).await.unwrap();
            socket.send(Message::Binary(b"opaque-binary".as_slice().into())).await.unwrap();
            let _ = socket.close(None).await;
            frame
        });
        let config = AppConfig::from_json(&serde_json::json!({
            "schemaVersion": 6,
            "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
            "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0},
            "endpoints":[{"id":"pi-upstream", "name":"pi-upstream", "baseURL":format!("http://{upstream_address}"),
                "apiKey":"upstream-secret", "protocol":"openai-responses", "enabled":true,
                "mappings":[{"clientPattern":"pi-model", "upstreamModel":"pi-model"}]}]
        }).to_string()).unwrap().normalized();
        let engine = super::pi_test_engine(config);
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap()).await.unwrap();
        let mut request = format!("ws://{address}/v1/responses").into_client_request().unwrap();
        for (name, value) in [
            ("authorization", "Bearer listener-secret"), ("user-agent", "pi (test)"),
            ("x-sumpter-client", "pi"), ("x-sumpter-project", "pi-project"),
            ("x-sumpter-workspace", "/work/pi-project"), ("x-sumpter-session-id", "pi-session"),
            ("x-sumpter-attribution-encoding", "uri-v1"), ("session-id", "native-session"),
        ] { request.headers_mut().insert(name, value.parse().unwrap()); }
        let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
        let frame = Message::Text(r#"{ "type":"response.create","model":"pi-model","input":"hello","originator":"Codex Desktop","session_id":"frame-session" }"#.into());
        socket.send(frame.clone()).await.unwrap();
        assert_eq!(socket.next().await.unwrap().unwrap(), frame);
        assert_eq!(socket.next().await.unwrap().unwrap(), Message::Binary(b"opaque-binary".as_slice().into()));
        let _ = socket.close(None).await;
        let headers = received.await.unwrap();
        assert!(!headers.keys().any(|name| name.as_str().starts_with("x-sumpter-")));
        assert_eq!(headers["authorization"], "Bearer upstream-secret");
        assert_eq!(headers["session-id"], "native-session");
        assert_eq!(task.await.unwrap(), frame);
        for _ in 0..100 {
            if engine.runtime_snapshot().recent_events.iter().filter(|e| matches!(e.kind.as_str(), "client" | "upstream")).count() >= 2 { break; }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let events = engine.runtime_snapshot().recent_events;
        assert!(events.iter().any(|e| e.kind == "client"));
        assert!(events.iter().any(|e| e.kind == "upstream"));
        for event in events.iter().filter(|e| matches!(e.kind.as_str(), "client" | "upstream")) {
            assert_eq!(event.client_kind, Some(ClientKind::Pi));
            assert_eq!(event.session_id.as_deref(), Some("pi-session"));
            assert!(event.codex_metadata.is_none());
            assert_eq!(event.client_declared.as_ref().and_then(|m| m.project.as_deref()), Some("pi-project"));
        }
        super::pi_test_shutdown(handle).await;
    }).await.expect("loopback pi websocket completed");
}
