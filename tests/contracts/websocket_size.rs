//! Exercise both directions, first-message routing, and both upstream dial paths.
use super::{pi_test_engine, pi_test_shutdown, server};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use sumpter_core::config::AppConfig;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::{
    Message,
    client::IntoClientRequest,
    protocol::{
        WebSocketConfig,
        frame::{
            Frame,
            coding::{Data, OpCode},
        },
    },
};

fn unlimited() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_frame_size(None)
        .max_message_size(None)
}

async fn send_fragmented<S>(socket: &mut WebSocketStream<S>, bytes: Bytes)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    for start in (0..bytes.len()).step_by(8 * 1024 * 1024) {
        let end = (start + 8 * 1024 * 1024).min(bytes.len());
        let opcode = if start == 0 {
            Data::Binary
        } else {
            Data::Continue
        };
        socket
            .send(Message::Frame(Frame::message(
                bytes.slice(start..end),
                OpCode::Data(opcode),
                end == bytes.len(),
            )))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn request_size_websocket_frames_and_fragmented_messages_are_unlimited() {
    tokio::time::timeout(Duration::from_secs(120), async {
        for resolve_ip in ["", "127.0.0.1"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            let mut binary = vec![0x5a; 64 * 1024 * 1024 + 1];
            *binary.last_mut().unwrap() = 0xff;
            let binary = Bytes::from(binary);
            let expected_binary = binary.clone();
            let upstream = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(unlimited()))
                    .await.unwrap();
                let first = socket.next().await.unwrap().unwrap();
                assert!(matches!(&first, Message::Text(text) if text.len() > 16 * 1024 * 1024));
                socket.send(first).await.unwrap();
                let received = socket.next().await.unwrap().unwrap();
                assert!(matches!(received, Message::Binary(bytes) if bytes == expected_binary));
                send_fragmented(&mut socket, expected_binary).await;
                // Wait for the client to consume the complete reply before closing.
                let _ = socket.next().await;
                let _ = socket.close(None).await;
            });
            let host = if resolve_ip.is_empty() { "127.0.0.1" } else { "size-upstream.invalid" };
            let config = AppConfig::from_json(&serde_json::json!({
                "schemaVersion": 6,
                "listener": {"host":"127.0.0.1", "port":0, "authToken":"size-test-token"},
                "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0},
                "endpoints":[{"id":"size", "name":"size", "baseURL":format!("http://{host}:{port}"),
                    "resolveIP":resolve_ip, "apiKey":"upstream-test-token", "protocol":"openai-responses", "enabled":true,
                    "mappings":[{"clientPattern":"size-model", "upstreamModel":"size-model"}]}]
            }).to_string()).unwrap().normalized();
            let engine = pi_test_engine(config);
            let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap()).await.unwrap();
            let mut request = format!("ws://{address}/v1/responses").into_client_request().unwrap();
            request.headers_mut().insert("authorization", "Bearer size-test-token".parse().unwrap());
            let (mut socket, _) = tokio_tungstenite::connect_async_with_config(request, Some(unlimited()), false)
                .await.unwrap();
            let mut text = String::from(r#"{"type":"response.create","model":"size-model","input":""#);
            text.push_str(&"x".repeat(16 * 1024 * 1024 + 1));
            text.push_str(r#""}"#);
            let first = Message::Text(text.into());
            socket.send(first.clone()).await.unwrap();
            assert!(socket.next().await.unwrap().unwrap() == first);
            send_fragmented(&mut socket, binary.clone()).await;
            assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Binary(bytes) if bytes == binary));
            let _ = socket.close(None).await;
            upstream.await.unwrap();
            pi_test_shutdown(handle).await;
        }
    }).await.expect("large WebSocket messages must finish relaying");
}
