//! Realtime covers both directions, fragmented messages and upstream dial paths.
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

fn security_config(port: u16, resolve_ip: &str) -> AppConfig {
    let host = if resolve_ip.is_empty() {
        "127.0.0.1"
    } else {
        "ws-security.invalid"
    };
    AppConfig::from_json(&serde_json::json!({
        "schemaVersion":7,
        "listener":{"host":"127.0.0.1","port":0,"authToken":"size-test-token"},
        "retry":{"maxDeferredRounds":0,"sessionStickyRetries":0},
        "requestPolicy":{"websocket":{"maxFrameMib":1,"maxMessageMib":1,"maxConnections":2}},
        "endpoints":[{"id":"size","name":"size","baseURL":format!("http://{host}:{port}"),"resolveIP":resolve_ip,"apiKey":"synthetic-key","protocol":"openai-responses","enabled":true,"capabilities":["live"],
        "mappings":[{"clientPattern":"size-model","upstreamModel":"size-model"}]}]
    }).to_string()).unwrap().normalized()
}

async fn send_oversized<S>(
    socket: &mut WebSocketStream<S>,
    fragmented: bool,
    limit: usize,
    from_server: bool,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    if fragmented {
        let _ = socket
            .send(Message::Frame(Frame::message(
                vec![b'x'; limit / 2],
                OpCode::Data(Data::Binary),
                false,
            )))
            .await;
        let _ = socket
            .send(Message::Frame(Frame::message(
                vec![b'x'; limit - limit / 2 + 1],
                OpCode::Data(Data::Continue),
                true,
            )))
            .await;
    } else {
        // A declared oversized frame must be rejected before allocating or
        // waiting for its payload. Send only its valid header, including the
        // client mask when applicable, then read the server's 1009 response.
        use tokio::io::AsyncWriteExt;
        let mut header = vec![0x82, if from_server { 127 } else { 255 }];
        header.extend_from_slice(&(limit as u64 + 1).to_be_bytes());
        if !from_server {
            header.extend_from_slice(&[0; 4]);
        }
        socket.get_mut().write_all(&header).await.unwrap();
    }
}

async fn security_limit_case(
    resolve_ip: &str,
    from_upstream: bool,
    fragmented: bool,
    limit: usize,
) {
    tokio::time::timeout(Duration::from_secs(120), async {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(unlimited())).await.unwrap();
            let exact = socket.next().await.unwrap().unwrap();
            assert!(matches!(&exact, Message::Binary(bytes) if bytes.len() == limit));
            socket.send(exact).await.unwrap();
            if from_upstream { send_oversized(&mut socket, fragmented, limit, true).await; }
            let close = socket.next().await.unwrap().unwrap();
            assert!(matches!(close, Message::Close(Some(frame)) if u16::from(frame.code) == 1009));
        });
        let mut config = security_config(port, resolve_ip);
        config.request_policy.websocket.max_frame_mib = (limit / (1024 * 1024)) as u32;
        config.request_policy.websocket.max_message_mib = config.request_policy.websocket.max_frame_mib;
        let engine = pi_test_engine(config);
        let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap()).await.unwrap();
        let mut request = format!("ws://{address}/v1/realtime?model=size-model").into_client_request().unwrap();
        request.headers_mut().insert("authorization", "Bearer size-test-token".parse().unwrap());
        let (mut socket, _) = tokio_tungstenite::connect_async_with_config(request, Some(unlimited()), false).await.unwrap();
        socket.send(Message::Binary(vec![b'x'; limit].into())).await.unwrap();
        assert!(matches!(socket.next().await.unwrap().unwrap(), Message::Binary(bytes) if bytes.len() == limit));
        if !from_upstream { send_oversized(&mut socket, fragmented, limit, false).await; }
        let close = socket.next().await.unwrap().unwrap();
        assert!(matches!(close, Message::Close(Some(frame)) if u16::from(frame.code) == 1009));
        upstream.await.unwrap();
        pi_test_shutdown(handle).await;
    }).await.expect("bounded oversize teardown");
}

#[tokio::test]
async fn security_websocket_closes_both_directions_and_fragment_overflow_with_1009() {
    for resolve_ip in ["", "127.0.0.1"] {
        for from_upstream in [false, true] {
            for fragmented in [false, true] {
                security_limit_case(resolve_ip, from_upstream, fragmented, 1024 * 1024).await;
            }
        }
    }
}

#[tokio::test]
#[ignore = "large 256 MiB boundary: run alone in authorized CI with --test-threads=1"]
async fn security_websocket_256_mib_boundary() {
    for from_upstream in [false, true] {
        for fragmented in [false, true] {
            security_limit_case("", from_upstream, fragmented, 256 * 1024 * 1024).await;
        }
    }
}

#[tokio::test]
async fn security_websocket_quota_precedes_dial_and_lowering_preserves_live_leases() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = accepted.clone();
    let upstream = tokio::spawn(async move {
        let mut sockets = Vec::new();
        for _ in 0..3 {
            let (stream, _) = listener.accept().await.unwrap();
            sockets.push(tokio_tungstenite::accept_async(stream).await.unwrap());
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        sockets
    });
    let mut config = security_config(port, "");
    let engine = pi_test_engine(config.clone());
    let headers = vec![("authorization".into(), "Bearer size-test-token".into())];
    let path = "/v1/realtime?model=size-model";
    let first = engine
        .prepare_realtime_websocket(None, path, &headers)
        .await
        .unwrap();
    let second = engine
        .prepare_realtime_websocket(None, path, &headers)
        .await
        .unwrap();
    config.request_policy.websocket.max_connections = 1;
    config.request_policy.websocket.max_frame_mib = 2;
    config.request_policy.websocket.max_message_mib = 2;
    engine.replace_config(config);
    let rejection = engine
        .prepare_realtime_websocket(None, path, &headers)
        .await
        .err()
        .unwrap();
    let response = sumpter_engine::engine::websocket_prepare_error_response(&rejection);
    assert_eq!(response.status(), 429);
    assert_eq!(response.headers()["retry-after"], "1");
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(first.max_message_bytes(), 1024 * 1024);
    drop(first);
    assert!(
        engine
            .prepare_realtime_websocket(None, path, &headers)
            .await
            .is_err()
    );
    drop(second);
    let next = engine
        .prepare_realtime_websocket(None, path, &headers)
        .await
        .unwrap();
    assert_eq!(next.max_message_bytes(), 2 * 1024 * 1024);
    drop(next);
    let _sockets = upstream.await.unwrap();
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
async fn request_size_websocket_preserves_large_messages_within_security_limits() {
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
                    "resolveIP":resolve_ip, "apiKey":"upstream-test-token", "protocol":"openai-responses", "enabled":true, "capabilities":["live"],
                    "mappings":[{"clientPattern":"size-model", "upstreamModel":"size-model"}]}]
            }).to_string()).unwrap().normalized();
            let engine = pi_test_engine(config);
            let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap()).await.unwrap();
            let mut request = format!("ws://{address}/v1/realtime?model=size-model").into_client_request().unwrap();
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
