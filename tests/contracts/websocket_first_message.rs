//! First-message lifecycle contract, exercised through both platform adapters.
use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use std::time::Duration;
use sumpter_core::events::{RuntimeEvent, RuntimeEventOutcome, RuntimeFailureKind};
use sumpter_engine::engine::EngineNotice;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest, protocol::CloseFrame};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(address: std::net::SocketAddr) -> Socket {
    let mut request = format!("ws://{address}/v1/responses?private=DO_NOT_CAPTURE")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", "Codex Desktop/test".parse().unwrap());
    request
        .headers_mut()
        .insert("x-openai-subagent", "guardian".parse().unwrap());
    let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
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
    .expect("connection completed")
}

#[tokio::test]
async fn first_message_close_eof_and_invalid_requests_have_distinct_outcomes() {
    for case in [
        "normal",
        "going_away",
        "no_code",
        "abnormal",
        "eof",
        "binary",
        "json",
        "model",
        "shutdown",
    ] {
        let engine = pi_test_engine(super::config());
        engine.set_diagnostic_capture(true, Some(64 * 1024));
        let mut notices = engine.subscribe();
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let mut client = connect(address).await;
        // A Pong proves the first-message reader has started. No upstream is contacted.
        client
            .send(Message::Ping(b"control-secret".as_slice().into()))
            .await
            .unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), client.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Message::Pong(b"control-secret".as_slice().into())
        );
        let normal = matches!(case, "normal" | "going_away" | "no_code");
        let rejected = matches!(case, "binary" | "json" | "model");
        let mut handle = Some(handle);
        match case {
            "normal" | "going_away" | "abnormal" | "no_code" => {
                let frame = match case {
                    "no_code" => None,
                    _ => Some(CloseFrame {
                        code: match case {
                            "normal" => 1000,
                            "going_away" => 1001,
                            _ => 1011,
                        }
                        .into(),
                        reason: "PRIVATE_CLOSE_REASON".into(),
                    }),
                };
                client.send(Message::Close(frame)).await.unwrap();
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(3), client.next())
                        .await
                        .unwrap(),
                    Some(Ok(Message::Close(_)))
                ));
            }
            "eof" => {
                drop(client);
            }
            "shutdown" => {
                pi_test_shutdown(handle.take().unwrap()).await;
                let _ = tokio::time::timeout(Duration::from_secs(3), client.next())
                    .await
                    .unwrap();
            }
            _ => {
                let message = match case {
                    "binary" => Message::Binary(b"PRIVATE_BODY".as_slice().into()),
                    "json" => Message::Text("PRIVATE_INVALID_JSON".into()),
                    _ => {
                        Message::Text(r#"{"type":"response.create","input":"PRIVATE_BODY"}"#.into())
                    }
                };
                client.send(message).await.unwrap();
                assert!(matches!(
                    tokio::time::timeout(Duration::from_secs(3), client.next())
                        .await
                        .unwrap(),
                    Some(Ok(Message::Text(_)))
                ));
            }
        }
        let event = completed(&mut notices).await;
        assert_eq!(event.status_code, 101, "{case}");
        assert_eq!(
            event.outcome,
            Some(if normal {
                RuntimeEventOutcome::Cancelled
            } else {
                RuntimeEventOutcome::Failed
            }),
            "{case}"
        );
        assert_eq!(
            event.failure_kind,
            Some(if normal {
                RuntimeFailureKind::ClientCancelled
            } else if rejected {
                RuntimeFailureKind::ClientRequestRejected
            } else {
                RuntimeFailureKind::StreamInterrupted
            }),
            "{case}"
        );
        assert!(
            event.upstream_host.is_none()
                && event.upstream_model.is_none()
                && event.upstream_status_code.is_none()
        );
        assert!(event.client_model.is_none() && event.effective_model.is_none());
        let trace = event
            .stream_trace
            .as_ref()
            .unwrap()
            .websocket_trace
            .as_ref()
            .unwrap();
        assert_eq!(trace.client_handshake_status, Some(101));
        assert_eq!(trace.stage.as_deref(), Some("awaiting_first_message"));
        assert_eq!(trace.handshake_status, None);
        assert_eq!(trace.attempt_count, Some(0));
        assert!(trace.first_message_wait_ms.unwrap() <= event.duration_ms);
        if case == "shutdown" {
            assert_eq!(trace.relay_error.as_deref(), Some("server_shutdown"));
        }
        let runtime = engine.runtime_snapshot();
        assert_eq!(runtime.client_requests, 1);
        assert_eq!(runtime.client_successes, 0);
        assert_eq!(runtime.client_failures, i64::from(!normal));
        assert_eq!(runtime.upstream_attempts, 0);
        assert_eq!(runtime.recent_events.len(), 1);
        let capture = engine.diagnostic_capture_snapshot();
        assert_eq!(capture.records.len(), 1);
        let record = &capture.records[0];
        assert_eq!(Some(&record.request_id), event.request_id.as_ref());
        assert_eq!(record.websocket_trace.as_ref(), Some(trace));
        assert_eq!(record.status_code, Some(101));
        assert!(record.inbound_headers.is_empty() && record.inbound_body.is_empty());
        assert!(record.attempts.is_empty() && record.client_chunks.is_empty());
        let encoded = serde_json::to_string(record).unwrap();
        for secret in [
            "PRIVATE",
            "DO_NOT_CAPTURE",
            "listener-secret",
            "control-secret",
        ] {
            assert!(!encoded.contains(secret));
        }
        if let Some(handle) = handle {
            pi_test_shutdown(handle).await;
        }
    }
}

#[tokio::test]
async fn first_message_controls_do_not_consume_or_duplicate_the_business_frame() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut upstream = tokio_tungstenite::accept_async(stream).await.unwrap();
        let first = upstream.next().await.unwrap().unwrap();
        upstream.send(first.clone()).await.unwrap();
        assert!(
            matches!(upstream.next().await.unwrap().unwrap(), Message::Close(_)),
            "business frame forwarded twice"
        );
        let _ = upstream.flush().await;
        first
    });
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{address}");
    config.migrate_model_groups();
    let engine = pi_test_engine(config);
    engine.set_diagnostic_capture(true, Some(64 * 1024));
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut client = connect(address).await;
    client
        .send(Message::Pong(b"unsolicited".as_slice().into()))
        .await
        .unwrap();
    client
        .send(Message::Ping(b"ping".as_slice().into()))
        .await
        .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        Message::Pong(b"ping".as_slice().into())
    );
    assert_eq!(engine.runtime_snapshot().upstream_attempts, 0);
    let capture_id = engine.diagnostic_capture_snapshot().records[0]
        .request_id
        .clone();
    let first = Message::Text(r#"{ "type":"response.create", "model":"gpt-4o", "generate":false, "input":"PRIVATE_PROMPT" }"#.into());
    client.send(first.clone()).await.unwrap();
    assert_eq!(client.next().await.unwrap().unwrap(), first);
    client.send(Message::Close(None)).await.unwrap();
    let event = completed(&mut notices).await;
    assert_eq!(task.await.unwrap(), first);
    assert_eq!(event.request_id.as_deref(), Some(capture_id.as_str()));
    let trace = event
        .stream_trace
        .as_ref()
        .unwrap()
        .websocket_trace
        .as_ref()
        .unwrap();
    assert_eq!(trace.stage.as_deref(), Some("relay"));
    assert!(event.duration_ms >= trace.first_message_wait_ms.unwrap());
    assert_eq!(engine.diagnostic_capture_snapshot().records.len(), 1);
    assert_eq!(
        engine.diagnostic_capture_snapshot().records[0]
            .websocket_trace
            .as_ref(),
        Some(trace)
    );
    assert!(
        !serde_json::to_string(&engine.diagnostic_capture_snapshot())
            .unwrap()
            .contains("PRIVATE_PROMPT")
    );
    pi_test_shutdown(handle).await;
}
