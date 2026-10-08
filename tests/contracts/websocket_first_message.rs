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
    connect_with_metadata(address, Some("guardian"), None).await
}

async fn connect_with_metadata(
    address: std::net::SocketAddr,
    role: Option<&str>,
    canonical: Option<&str>,
) -> Socket {
    connect_path(
        address,
        "/v1/responses?private=DO_NOT_CAPTURE",
        role,
        canonical,
    )
    .await
}

async fn connect_path(
    address: std::net::SocketAddr,
    path: &str,
    role: Option<&str>,
    canonical: Option<&str>,
) -> Socket {
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("authorization", "Bearer listener-secret".parse().unwrap());
    request
        .headers_mut()
        .insert("user-agent", "Codex Desktop/test".parse().unwrap());
    if let Some(role) = role {
        request
            .headers_mut()
            .insert("x-openai-subagent", role.parse().unwrap());
    }
    if let Some(canonical) = canonical {
        request
            .headers_mut()
            .insert("x-codex-turn-metadata", canonical.parse().unwrap());
    }
    let (socket, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 101);
    socket
}

async fn live_connections(
    engine: &super::Engine,
    waiting: u64,
    guardian: u64,
    connecting: u64,
    relaying: u64,
) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let value = engine.runtime_summary_value()["responsesWebSocketConnections"].clone();
            if value["awaitingFirstMessage"] == waiting
                && value["guardianAwaitingFirstMessage"] == guardian
                && value["connectingUpstream"] == connecting
                && value["relaying"] == relaying
            {
                assert_eq!(value["total"], waiting + connecting + relaying);
                if waiting == 0 {
                    assert!(value["oldestFirstMessageWaitMS"].is_null());
                }
                return value;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("live connection stages converge")
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
        "eof_plain",
        "eof_subagent",
        "eof_conflicting",
        "eof_malformed",
        "reset",
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
        let (role, canonical) = match case {
            "eof_plain" => (None, None),
            "eof_subagent" => (Some("collab_spawn"), None),
            "eof_conflicting" => (
                Some("guardian"),
                Some(r#"{"subagent_kind":"thread_spawn"}"#),
            ),
            "eof_malformed" => (Some("guardian"), Some("INVALID_METADATA")),
            _ => (Some("guardian"), None),
        };
        let mut client = connect_with_metadata(address, role, canonical).await;
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
        let guardian = !matches!(
            case,
            "eof_plain" | "eof_subagent" | "eof_conflicting" | "eof_malformed"
        );
        live_connections(&engine, 1, u64::from(guardian), 0, 0).await;
        let unused = normal || case == "eof";
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
            "eof" | "eof_plain" | "eof_subagent" | "eof_conflicting" | "eof_malformed" => {
                drop(client);
            }
            "reset" => {
                let tokio_tungstenite::MaybeTlsStream::Plain(stream) = client.get_ref() else {
                    unreachable!()
                };
                stream.set_zero_linger().unwrap();
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
        live_connections(&engine, 0, 0, 0, 0).await;
        assert_eq!(event.status_code, 101, "{case}");
        assert_eq!(
            event.outcome,
            Some(if unused {
                RuntimeEventOutcome::Cancelled
            } else {
                RuntimeEventOutcome::Failed
            }),
            "{case}"
        );
        assert_eq!(
            event.failure_kind,
            Some(if unused {
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
        assert_eq!(trace.abnormal_close, Some(!normal));
        if case.starts_with("eof") {
            assert_eq!(
                trace.relay_error.as_deref(),
                Some("client_eof_without_close")
            );
            assert_eq!(
                trace.transport_error_kind.as_deref(),
                Some("eof_without_close")
            );
        }
        if unused {
            assert_eq!(
                event.message.as_deref(),
                Some("websocket_unused_guardian_connection_closed")
            );
        } else {
            assert_ne!(
                event.message.as_deref(),
                Some("websocket_unused_guardian_connection_closed")
            );
        }
        assert!(trace.first_message_wait_ms.unwrap() <= event.duration_ms);
        if case == "shutdown" {
            assert_eq!(trace.relay_error.as_deref(), Some("server_shutdown"));
        }
        let runtime = engine.runtime_snapshot();
        assert_eq!(runtime.client_requests, 1);
        assert_eq!(runtime.client_successes, 0);
        assert_eq!(runtime.client_failures, i64::from(!unused));
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
async fn guardian_pool_recycles_seven_unused_sockets_without_hiding_active_failures() {
    for case in ["normal", "completed_eof", "pending_eof"] {
        tokio::time::timeout(
            Duration::from_secs(20),
            assert_guardian_pool_lifecycle(case),
        )
        .await
        .expect("Guardian pool lifecycle completed");
    }
}

async fn assert_guardian_pool_lifecycle(case: &'static str) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
    config.migrate_model_groups();
    let first = r#"{ "type":"response.create", "model":"gpt-4o", "generate":false, "input":[], "opaque":"PRIVATE_PROMPT" }"#;
    let response = if case == "pending_eof" {
        r#"{"type":"response.created","response":{"status":"in_progress"}}"#
    } else {
        r#"{"type":"response.completed","response":{"status":"completed"}}"#
    };
    let (release, wait) = tokio::sync::oneshot::channel();
    let upstream = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text(first.into())
        );
        socket.send(Message::Text(response.into())).await.unwrap();
        wait.await.unwrap();
        if case == "normal" {
            socket.send(Message::Close(None)).await.unwrap();
            let _ = socket.next().await;
        }
        // Other cases drop TCP without a Close, after business began.
    });
    let engine = pi_test_engine(config);
    engine.set_diagnostic_capture(true, Some(128 * 1024));
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut sockets = Vec::new();
    for _ in 0..8 {
        let mut socket = connect(address).await;
        socket
            .send(Message::Ping(b"control-secret".as_slice().into()))
            .await
            .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Pong(b"control-secret".as_slice().into())
        );
        sockets.push(socket);
    }
    assert_eq!(engine.runtime_snapshot().upstream_attempts, 0);
    assert_eq!(engine.runtime_snapshot().client_requests, 0);
    assert_eq!(engine.diagnostic_capture_snapshot().records.len(), 8);
    let before = live_connections(&engine, 8, 8, 0, 0).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    let after = live_connections(&engine, 8, 8, 0, 0).await;
    assert!(
        after["oldestFirstMessageWaitMS"].as_i64().unwrap()
            > before["oldestFirstMessageWaitMS"].as_i64().unwrap()
    );
    let mut active = sockets.pop().unwrap();
    drop(sockets);
    let mut ids = std::collections::HashSet::new();
    for _ in 0..7 {
        let event = completed(&mut notices).await;
        assert!(ids.insert(event.id));
        assert_eq!(event.outcome, Some(RuntimeEventOutcome::Cancelled));
        assert_eq!(
            event.message.as_deref(),
            Some("websocket_unused_guardian_connection_closed")
        );
        let trace = event.stream_trace.unwrap().websocket_trace.unwrap();
        assert_eq!(trace.stage.as_deref(), Some("awaiting_first_message"));
        assert_eq!(trace.attempt_count, Some(0));
        assert_eq!(trace.client_message_count, Some(0));
        assert_eq!(trace.upstream_message_count, Some(0));
        assert_eq!(trace.bytes_sent, Some(0));
        assert_eq!(trace.bytes_received, Some(0));
        assert_eq!(trace.abnormal_close, Some(true));
        assert_eq!(
            trace.transport_error_kind.as_deref(),
            Some("eof_without_close")
        );
    }
    assert_eq!(engine.runtime_snapshot().client_failures, 0);
    assert_eq!(engine.runtime_snapshot().client_successes, 0);
    assert!(engine.last_error().is_none());
    live_connections(&engine, 1, 1, 0, 0).await;
    // The remaining prewarmed socket is still usable, byte-for-byte.
    active.send(Message::Text(first.into())).await.unwrap();
    assert_eq!(
        active.next().await.unwrap().unwrap(),
        Message::Text(response.into())
    );
    live_connections(&engine, 0, 0, 0, 1).await;
    release.send(()).unwrap();
    let _ = active.next().await;
    let event = completed(&mut notices).await;
    live_connections(&engine, 0, 0, 0, 0).await;
    assert!(ids.insert(event.id));
    let normal = case == "normal";
    assert_eq!(
        event.outcome,
        Some(if normal {
            RuntimeEventOutcome::Succeeded
        } else {
            RuntimeEventOutcome::Failed
        })
    );
    assert_eq!(
        event.failure_kind,
        if normal {
            None
        } else {
            Some(RuntimeFailureKind::StreamInterrupted)
        }
    );
    let trace = event.stream_trace.unwrap().websocket_trace.unwrap();
    assert_eq!(trace.stage.as_deref(), Some("relay"));
    assert_eq!(trace.attempt_count, Some(1));
    assert_eq!(trace.client_message_count, Some(1));
    let runtime = engine.runtime_snapshot();
    assert_eq!(runtime.client_requests, 8);
    assert_eq!(runtime.client_successes, i64::from(normal));
    assert_eq!(runtime.client_failures, i64::from(!normal));
    assert_eq!(runtime.upstream_attempts, 1);
    assert_eq!(runtime.recent_events.len(), 9);
    let capture = engine.diagnostic_capture_snapshot();
    assert_eq!(capture.records.len(), 8);
    let encoded = serde_json::to_string(&capture).unwrap();
    for secret in [
        "PRIVATE_PROMPT",
        "control-secret",
        "listener-secret",
        "DO_NOT_CAPTURE",
    ] {
        assert!(!encoded.contains(secret));
    }
    upstream.await.unwrap();
    pi_test_shutdown(handle).await;
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

#[tokio::test]
async fn live_connections_cover_upstream_setup_query_models_and_prepare_failure() {
    for query_model in [false, true] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut config = super::config();
        config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
        config.migrate_model_groups();
        let (handshake_tx, handshake_rx) = tokio::sync::oneshot::channel();
        let (close_tx, close_rx) = tokio::sync::oneshot::channel();
        let upstream = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handshake_rx.await.unwrap();
            let mut peer = tokio_tungstenite::accept_async(stream).await.unwrap();
            close_rx.await.unwrap();
            peer.send(Message::Close(None)).await.unwrap();
        });
        let engine = pi_test_engine(config);
        let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let path = if query_model {
            "/v1/responses?model=gpt-4o"
        } else {
            "/v1/responses"
        };
        let mut client = connect_path(address, path, Some("guardian"), None).await;
        if !query_model {
            live_connections(&engine, 1, 1, 0, 0).await;
            client
                .send(Message::Text(
                    r#"{"type":"response.create","model":"gpt-4o","input":[]}"#.into(),
                ))
                .await
                .unwrap();
        }
        live_connections(&engine, 0, 0, 1, 0).await;
        handshake_tx.send(()).unwrap();
        live_connections(&engine, 0, 0, 0, 1).await;
        close_tx.send(()).unwrap();
        let _ = client.next().await;
        live_connections(&engine, 0, 0, 0, 0).await;
        upstream.await.unwrap();
        pi_test_shutdown(handle).await;
    }

    // An upstream that rejects Upgrade must not leave a connecting gauge behind.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{}", listener.local_addr().unwrap());
    config.migrate_model_groups();
    let upstream = tokio::spawn(async move {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        loop {
            line.clear();
            assert!(reader.read_line(&mut line).await.unwrap() > 0);
            if line == "\r\n" {
                break;
            }
        }
        drop(reader);
        stream
            .write_all(
                b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
    });
    let engine = pi_test_engine(config);
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut client = connect_path(
        address,
        "/v1/responses?model=gpt-4o",
        Some("guardian"),
        None,
    )
    .await;
    let _ = tokio::time::timeout(Duration::from_secs(3), client.next())
        .await
        .unwrap();
    assert_eq!(
        completed(&mut notices).await.outcome,
        Some(RuntimeEventOutcome::Failed)
    );
    live_connections(&engine, 0, 0, 0, 0).await;
    upstream.await.unwrap();
    pi_test_shutdown(handle).await;
}
