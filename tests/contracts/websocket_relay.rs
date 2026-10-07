//! Real-socket relay regressions run through both adapters, without real providers.
use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use sumpter_core::events::{RuntimeEvent, RuntimeEventOutcome, RuntimeFailureKind};
use sumpter_engine::engine::EngineNotice;
use tokio_tungstenite::tungstenite::{
    Message,
    client::IntoClientRequest,
    protocol::{CloseFrame, WebSocketConfig},
};

const FIRST: &str =
    r#"{ "type":"response.create", "model":"gpt-4o", "generate":false, "input":[] }"#;
const TERMINAL: &str = r#"{"type":"response.completed","response":{"status":"completed"}}"#;

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(address: std::net::SocketAddr, query: &str) -> Client {
    let mut request = format!("ws://{address}/v1/responses{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", "Codex Desktop/test".parse().unwrap());
    let (socket, response) =
        tokio_tungstenite::connect_async_with_config(request, Some(unlimited()), false)
            .await
            .unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
}

fn unlimited() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_frame_size(None)
        .max_message_size(None)
}

async fn completed(notices: &mut tokio::sync::broadcast::Receiver<EngineNotice>) -> RuntimeEvent {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let EngineNotice::Event(event) = notices.recv().await.unwrap()
                && event.kind == "client"
            {
                return event;
            }
        }
    })
    .await
    .expect("relay terminated")
}

#[tokio::test]
async fn responses_frames_and_close_remain_ordered_and_exact() {
    let frames = vec![
        FIRST,
        r#"{ "type":"response.create", "stream_id":"main", "previous_response_id":"resp-1", "input":[{"type":"tool_addition","tools":[]}], "opaque":{"keep":[1,null]} }"#,
        r#"{"type":"response.append","input":[{"type":"function_call_output","call_id":"call-1","output":"PRIVATE_OUTPUT"}]}"#,
        r#"{"type":"response.steer","previous_response_id":"resp-2","input":[{"role":"user","content":"PRIVATE_PROMPT"}]}"#,
        r#"{"type":"response.interrupt","response_id":"resp-2","mode":"discard_partial_items","future_flag":true}"#,
    ];
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
    config.migrate_model_groups();
    let expected = frames.clone();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        for expected in expected {
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text(expected.into())
            );
            socket.send(Message::Text(expected.into())).await.unwrap();
        }
        socket.send(Message::Text(TERMINAL.into())).await.unwrap();
        socket
            .send(Message::Close(Some(CloseFrame {
                code: 1000.into(),
                reason: "PRIVATE_CLOSE".into(),
            })))
            .await
            .unwrap();
        let _ = socket.next().await;
    });
    let engine = pi_test_engine(config);
    engine.set_diagnostic_capture(true, Some(64 * 1024));
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut socket = connect(address, "").await;
    for frame in &frames {
        socket.send(Message::Text((*frame).into())).await.unwrap();
    }
    for frame in &frames {
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text((*frame).into())
        );
    }
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text(TERMINAL.into())
    );
    assert!(
        matches!(socket.next().await.unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 1000)
    );
    let event = completed(&mut notices).await;
    assert_eq!(event.outcome, Some(RuntimeEventOutcome::Succeeded));
    let trace = event
        .stream_trace
        .as_ref()
        .unwrap()
        .websocket_trace
        .as_ref()
        .unwrap();
    assert_eq!(trace.last_event_type.as_deref(), Some("response.completed"));
    assert_eq!(trace.client_message_count, Some(frames.len() as u64));
    assert_eq!(trace.upstream_message_count, Some(frames.len() as u64 + 2));
    assert!(trace.transport_error_kind.is_none());
    let snapshot = engine.runtime_snapshot();
    assert_eq!(snapshot.client_requests, 1);
    assert_eq!(snapshot.upstream_attempts, 1);
    assert_eq!(snapshot.recent_events.len(), 2);
    let encoded = serde_json::to_string(&engine.diagnostic_capture_snapshot()).unwrap();
    assert!(!encoded.contains("PRIVATE") && !encoded.contains("listener-secret"));
    upstream.await.unwrap();
    pi_test_shutdown(handle).await;
}

#[tokio::test]
async fn relay_close_eof_reset_and_cancel_have_distinct_results() {
    for case in ["normal", "abnormal", "eof", "reset", "client_close"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = super::config();
        config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
        config.migrate_model_groups();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text(FIRST.into())
            );
            // Ensure the consumer sees an actual response before injecting the fault.
            socket.send(Message::Text(TERMINAL.into())).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text("ack".into())
            );
            match case {
                "normal" | "abnormal" => {
                    socket
                        .send(Message::Close(Some(CloseFrame {
                            code: if case == "normal" { 1000 } else { 1011 }.into(),
                            reason: "PRIVATE_CLOSE".into(),
                        })))
                        .await
                        .unwrap();
                    let _ = socket.next().await;
                }
                "reset" => {
                    socket.get_ref().set_zero_linger().unwrap();
                }
                "client_close" => {
                    assert!(matches!(
                        socket.next().await.unwrap().unwrap(),
                        Message::Close(_)
                    ));
                    let _ = socket.flush().await;
                }
                _ => {} // Drop without sending a Close frame.
            }
        });
        let engine = pi_test_engine(config);
        engine.set_diagnostic_capture(true, Some(64 * 1024));
        let mut notices = engine.subscribe();
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut socket = connect(address, "").await;
        socket.send(Message::Text(FIRST.into())).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text(TERMINAL.into())
        );
        socket.send(Message::Text("ack".into())).await.unwrap();
        if case == "client_close" {
            socket.send(Message::Close(None)).await.unwrap();
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), socket.next())
            .await
            .unwrap();
        let event = completed(&mut notices).await;
        assert_eq!(event.status_code, 101);
        let trace = event
            .stream_trace
            .as_ref()
            .unwrap()
            .websocket_trace
            .as_ref()
            .unwrap();
        assert_eq!(trace.last_event_type.as_deref(), Some("response.completed"));
        assert_eq!(trace.attempt_count, Some(1));
        let (outcome, error) = match case {
            "normal" => (RuntimeEventOutcome::Succeeded, None),
            "client_close" => (RuntimeEventOutcome::Cancelled, None),
            "abnormal" => (RuntimeEventOutcome::Failed, Some("abnormal_close")),
            "reset" => (RuntimeEventOutcome::Failed, Some("connection_reset")),
            _ => (RuntimeEventOutcome::Failed, Some("eof_without_close")),
        };
        assert_eq!(event.outcome, Some(outcome), "{case}: {event:?}");
        assert_eq!(trace.transport_error_kind.as_deref(), error, "{case}");
        assert_eq!(
            trace.closed_by.as_deref(),
            Some(if case == "client_close" {
                "client"
            } else {
                "upstream"
            })
        );
        if case == "client_close" {
            assert_eq!(
                event.failure_kind,
                Some(RuntimeFailureKind::ClientCancelled)
            );
        }
        assert_eq!(engine.runtime_snapshot().recent_events.len(), 2);
        let capture = engine.diagnostic_capture_snapshot();
        assert_eq!(capture.records.len(), 1);
        assert_eq!(capture.records[0].websocket_trace.as_ref(), Some(trace));
        let encoded = serde_json::to_string(&capture).unwrap();
        assert!(!encoded.contains("PRIVATE_CLOSE") && !encoded.contains("listener-secret"));
        upstream.await.unwrap();
        pi_test_shutdown(handle).await;
    }
}

// Test the source's heartbeat while its destination deliberately does not read.
// A 32 MiB frame is larger than loopback TCP buffers. The sink remains blocked
// until AFTER Pong, so the old read-then-send relay cannot pass this test.
#[tokio::test]
async fn slow_destinations_do_not_starve_source_pongs_or_initial_frames() {
    tokio::time::timeout(Duration::from_secs(30), async {
        for from_client in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let mut config = super::config();
            config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
            config.migrate_model_groups();
            let engine = pi_test_engine(config);
            let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap()).await.unwrap();
            let (release, wait) = tokio::sync::oneshot::channel();
            let (release_other, wait_other) = tokio::sync::oneshot::channel();
            let large = if from_client {
                Message::Text(serde_json::json!({"type":"response.create","model":"gpt-4o","input":"x".repeat(32 * 1024 * 1024)}).to_string().into())
            } else { Message::Binary(vec![0x5a; 32 * 1024 * 1024].into()) };
            let upstream_large = large.clone();
            let upstream = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async_with_config(stream, Some(unlimited())).await.unwrap();
                if from_client {
                    wait.await.unwrap();
                    drain_large_and_ping(&mut socket, upstream_large).await;
                } else {
                    assert_eq!(socket.next().await.unwrap().unwrap(), Message::Text(FIRST.into()));
                    socket.send(upstream_large).await.unwrap();
                    socket.send(Message::Ping(b"heartbeat".as_slice().into())).await.unwrap();
                    assert_eq!(tokio::time::timeout(Duration::from_secs(3), socket.next()).await.expect("upstream Pong before client starts reading").unwrap().unwrap(), Message::Pong(b"heartbeat".as_slice().into()));
                    release_other.send(()).unwrap();
                }
                loop {
                    if let Some(Ok(Message::Close(_))) = socket.next().await { let _ = socket.flush().await; break; }
                }
            });
            let mut client = connect(address, "").await;
            if from_client {
                client.send(large).await.unwrap();
                client.send(Message::Ping(b"heartbeat".as_slice().into())).await.unwrap();
                assert_eq!(tokio::time::timeout(Duration::from_secs(3), client.next()).await.expect("client Pong while initial frame is blocked").unwrap().unwrap(), Message::Pong(b"heartbeat".as_slice().into()));
                release.send(()).unwrap();
                // Wait for the upstream to drain data+Ping and confirm delivery.
                loop { if let Some(Ok(Message::Text(text))) = client.next().await { assert_eq!(text, "drained"); break; } }
            } else {
                client.send(Message::Text(FIRST.into())).await.unwrap();
                wait_other.await.unwrap();
                drain_large_and_ping(&mut client, large).await;
            }
            client.send(Message::Close(None)).await.unwrap();
            let _ = client.next().await;
            upstream.await.unwrap();
            pi_test_shutdown(handle).await;
        }
    }).await.expect("slow-peer relay finishes");
}

async fn drain_large_and_ping<S>(
    socket: &mut tokio_tungstenite::WebSocketStream<S>,
    expected: Message,
) where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut data_seen = false;
    let mut ping_seen = false;
    while !data_seen || !ping_seen {
        let message = socket.next().await.unwrap().unwrap();
        match message {
            Message::Ping(_) => {
                ping_seen = true;
                socket.flush().await.unwrap();
            }
            Message::Pong(_) => {}
            _ => {
                assert!(!data_seen, "no duplicated payload");
                assert_eq!(message, expected);
                data_seen = true;
            }
        }
    }
    socket.send(Message::Text("drained".into())).await.unwrap();
}

#[tokio::test]
async fn idle_limit_starts_after_business_and_counts_activity_in_both_directions() {
    for query in ["", "?model=gpt-4o"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = super::config();
        config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
        config.retry.stream_idle_timeout_seconds = Some(0.2);
        config.migrate_model_groups();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Text(FIRST.into())
            );
            socket.send(Message::Text(TERMINAL.into())).await.unwrap();
            while let Some(Ok(message)) = socket.next().await {
                match message {
                    Message::Text(text) => {
                        socket.send(Message::Text(text)).await.unwrap();
                    }
                    Message::Ping(_) => {
                        socket.flush().await.unwrap();
                    }
                    _ => {}
                }
            }
        });
        let engine = pi_test_engine(config);
        let mut notices = engine.subscribe();
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut client = connect(address, query).await;
        // Prewarmed connection may wait longer than configured relay timeout.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), client.next())
                .await
                .is_err()
        );
        client.send(Message::Text(FIRST.into())).await.unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            Message::Text(TERMINAL.into())
        );
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_millis(60)).await;
            client
                .send(Message::Text("keep-active".into()))
                .await
                .unwrap();
            assert_eq!(
                client.next().await.unwrap().unwrap(),
                Message::Text("keep-active".into())
            );
        }
        let event = completed(&mut notices).await;
        assert_eq!(event.outcome, Some(RuntimeEventOutcome::Failed));
        assert_eq!(
            event.failure_kind,
            Some(RuntimeFailureKind::StreamIdleTimeout)
        );
        assert_eq!(event.timeout_ms, Some(200));
        let trace = event.stream_trace.unwrap().websocket_trace.unwrap();
        assert_eq!(trace.idle_timeout_ms, Some(200));
        assert_eq!(trace.transport_error_kind.as_deref(), Some("idle_timeout"));
        assert_eq!(trace.stage.as_deref(), Some("relay"));
        assert_eq!(trace.handshake_status, Some(101));
        upstream.await.unwrap();
        pi_test_shutdown(handle).await;
    }
}

#[tokio::test]
async fn terminal_frame_before_eof_is_delivered_without_synthetic_success() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
    config.migrate_model_groups();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        socket.next().await.unwrap().unwrap();
        socket.send(Message::Text(TERMINAL.into())).await.unwrap();
        // Immediate EOF after the terminal frame, without a Close handshake.
    });
    let engine = pi_test_engine(config);
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut client = connect(address, "").await;
    client.send(Message::Text(FIRST.into())).await.unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        Message::Text(TERMINAL.into())
    );
    let event = completed(&mut notices).await;
    assert_eq!(event.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(
        event
            .stream_trace
            .unwrap()
            .websocket_trace
            .unwrap()
            .transport_error_kind
            .as_deref(),
        Some("eof_without_close")
    );
    upstream.await.unwrap();
    pi_test_shutdown(handle).await;
}
