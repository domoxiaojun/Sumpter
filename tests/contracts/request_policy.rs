//! Versioned synthetic client fixtures and request-governance contracts, on both adapters.
use super::*;

fn config() -> AppConfig {
    let mut c = two_endpoint_config();
    c.endpoints.truncate(1);
    c.endpoints[0].protocol = EndpointProtocolMode::Auto;
    c.retry.session_sticky_retries = 0;
    c
}
fn reply(status: u16, body: &[u8]) -> Outcome {
    Outcome::Status {
        status,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![body.to_vec()],
    }
}

#[tokio::test]
async fn versioned_native_client_fixtures_preserve_tool_and_signature_payloads() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../fixtures/client_contracts_v1.json")).unwrap();
    assert_eq!(fixtures["schemaVersion"], 1);
    for case in fixtures["requests"].as_array().unwrap() {
        let transport = FakeTransport::new();
        let output = serde_json::to_vec(&case["response"]).unwrap();
        transport.push("a.example.com", reply(200, &output));
        let engine = engine_with(config(), transport.clone());
        let body = serde_json::to_vec(&case["body"]).unwrap();
        let headers = case["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h[0].as_str().unwrap().into(), h[1].as_str().unwrap().into()))
            .collect();
        let (status, received) = call(
            &engine,
            loopback(),
            case["path"].as_str().unwrap(),
            headers,
            Bytes::from(body.clone()),
        )
        .await;
        assert_eq!(
            status,
            200,
            "{}: {}",
            case["client"],
            String::from_utf8_lossy(&received)
        );
        assert_eq!(received.as_ref(), output);
        assert_eq!(transport.requests().len(), 1);
        assert_eq!(
            transport.requests()[0].body,
            body,
            "native client payload changed: {}",
            case["client"]
        );
    }
}

#[test]
fn versioned_codex_prewarm_and_controls_remain_distinct() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../fixtures/client_contracts_v1.json")).unwrap();
    let mut observer = sumpter_core::responses_observation::ResponsesRequestObserver::default();
    for item in fixtures["websocket"].as_array().unwrap() {
        if item["direction"] == "client" {
            observer.client(&item["frame"], None);
        } else {
            observer.upstream(&item["frame"]);
        }
    }
    let summary = observer.summary();
    assert_eq!(summary.counts.get("prewarm"), Some(&1));
    assert_eq!(summary.recent.len(), 2);
    assert_eq!(summary.controls.len(), 3);
    assert_eq!(
        summary.recent[1].terminal.as_deref(),
        Some("response.interrupted")
    );
}

#[tokio::test(start_paused = true)]
async fn hard_quota_429_stays_inside_proxy_retry_loop() {
    for status in [429, 503] {
        let transport = FakeTransport::new();
        for _ in 0..3 {
            transport.push(
                "a.example.com",
                reply(status, br#"{"error":{"code":"insufficient_quota"}}"#),
            );
        }
        transport.push(
            "a.example.com",
            sse_ok(&["event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"]),
        );
        let mut c = config();
        c.retry.max_deferred_rounds = 0;
        let engine = engine_with(c, transport.clone());
        let request_engine = engine.clone();
        let request = tokio::spawn(async move {
            request_engine
                .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while transport.requests().len() < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("must keep retrying quota-shaped HTTP errors");
        assert!(
            !request.is_finished(),
            "intermediate errors must not commit client headers"
        );
        let snapshot = engine.runtime_snapshot();
        let client = snapshot
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap();
        assert_eq!(client.status_code, 0);
        assert_eq!(client.ttfb_ms, None);
        let response = tokio::time::timeout(Duration::from_secs(10), request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), 200);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("message_stop"));
        assert_eq!(transport.requests().len(), 4);
    }
}

#[tokio::test]
async fn total_attempt_budget_stops_before_another_sticky_or_deferred_attempt() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", reply(503, b"busy"));
    let mut c = config();
    c.retry.max_deferred_rounds = 0;
    c.retry.session_sticky_retries = 3;
    c.request_policy.max_total_attempts = 1;
    let engine = engine_with(c, transport.clone());
    let (status, _) = tokio::time::timeout(
        Duration::from_secs(3),
        call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body(),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, 503);
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn concurrency_rejection_is_local_and_dropping_stream_releases_slot() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", Outcome::Gated { status: 200 });
    transport.push("a.example.com", reply(200, br#"{"ok":true}"#));
    let mut c = config();
    c.request_policy.global.max_concurrent = 1;
    c.request_policy.global.max_queued = 0;
    let engine = engine_with(c, transport.clone());
    let held = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![],
            codex_responses_body(),
        )
        .await;
    let sender = transport.gate_sender();
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["scope"],
        "local"
    );
    assert_eq!(transport.requests().len(), 1);
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.failure_kind == Some(RuntimeFailureKind::LocalAdmissionRejected))
    );
    drop(held);
    drop(sender);
    tokio::task::yield_now().await;
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(transport.requests().len(), 2);
}

#[tokio::test]
async fn admission_uses_free_same_group_candidate_without_crossing_groups() {
    for same_group in [true, false] {
        let transport = FakeTransport::new();
        transport.push("a.example.com", Outcome::Gated { status: 200 });
        transport.push("b.example.com", reply(200, b"{}"));
        let mut c = two_endpoint_config();
        set_all_endpoint_protocols(&mut c, EndpointProtocolMode::Auto);
        if !same_group {
            c.endpoints[1].sticky_group = Some("backup".into());
        }
        let mut policy = sumpter_core::request_policy::EndpointPolicy::default();
        policy.limits.max_concurrent = 1;
        policy.limits.max_queued = 0;
        c.request_policy.endpoints.insert("a".into(), policy);
        let engine = engine_with(c, transport.clone());
        let held = engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/responses",
                vec![],
                codex_responses_body(),
            )
            .await;
        let (status, _) = call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body(),
        )
        .await;
        assert_eq!(status, if same_group { 200 } else { 429 });
        assert_eq!(transport.requests().len(), if same_group { 2 } else { 1 });
        if same_group {
            assert_eq!(transport.requests()[1].host, "b.example.com");
        }
        drop(held);
    }
}

#[tokio::test]
async fn cancelled_queued_request_never_sends_or_cools_the_endpoint() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", Outcome::Gated { status: 200 });
    transport.push("a.example.com", sse_ok(&["data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"]));
    let mut c = config();
    c.request_policy.global.max_concurrent = 1;
    c.request_policy.global.max_queued = 1;
    let engine = engine_with(c, transport.clone());
    let held = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![],
            codex_responses_body(),
        )
        .await;
    let queued_engine = engine.clone();
    let queued = tokio::spawn(async move {
        call(
            &queued_engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body(),
        )
        .await
    });
    tokio::task::yield_now().await;
    queued.abort();
    let _ = queued.await;
    assert_eq!(transport.requests().len(), 1);
    drop(held);
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body()
        )
        .await
        .0,
        200
    );
    assert_eq!(transport.requests().len(), 2);
    assert!(
        !engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.kind == "upstream" && e.is_failed())
    );
}

#[tokio::test]
async fn chunk_fallback_commits_once_and_trace_finishes_with_protocol_failure() {
    let transport = FakeTransport::new();
    let mut chunks = vec![b": keepalive\n\n".to_vec(); 16];
    chunks.push(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\"}}}\n\n".to_vec());
    transport.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: chunks.clone(),
        },
    );
    let mut c = config();
    c.retry.max_stream_error_retries = 3;
    c.request_policy.stream_buffer_chunks = 16;
    let engine = engine_with(c, transport.clone());
    let (_, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(body.as_ref(), chunks.concat());
    assert_eq!(transport.requests().len(), 1);
    let snapshot = engine.runtime_snapshot();
    let client = snapshot
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    let trace = client.request_trace.as_ref().unwrap();
    assert!(trace.steps.iter().any(|s| s.detail == "chunk_budget"));
    assert_eq!(trace.steps.last().unwrap().detail, "failed");
}

#[tokio::test]
async fn stream_attempt_budget_prevents_replay_and_keeps_original_failure() {
    let transport = FakeTransport::new();
    let failed = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\"}}}\n\n";
    transport.push("a.example.com", sse_ok(&[failed]));
    let mut c = config();
    c.retry.max_stream_error_retries = 3;
    c.request_policy.max_total_attempts = 1;
    let engine = engine_with(c, transport.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(&body[..], failed.as_bytes());
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test]
async fn pinned_codex_grammar_fixture_rejects_lossy_bridge() {
    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/client_contracts_v1.json")).unwrap();
    let transport = FakeTransport::new();
    let mut c = config();
    c.endpoints[0].protocol = EndpointProtocolMode::Anthropic;
    let engine = engine_with(c, transport.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        Bytes::from(serde_json::to_vec(&fixture["requests"][0]["body"]).unwrap()),
    )
    .await;
    assert_eq!(status, 400, "{}", String::from_utf8_lossy(&body));
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn explicit_compatibility_denial_never_sends_or_changes_the_request() {
    let transport = FakeTransport::new();
    let mut c = config();
    let mut policy = sumpter_core::request_policy::EndpointPolicy::default();
    policy.models.insert(
        "*".into(),
        sumpter_core::request_policy::CompatibilityProfile {
            developer_role: Some(false),
            ..Default::default()
        },
    );
    c.request_policy.endpoints.insert("a".into(), policy);
    let engine = engine_with(c, transport.clone());
    let body = Bytes::from_static(br#"{"model":"claude-opus-5","input":[{"role":"developer","content":"hello"}],"stream":false}"#);
    let (status, _) = call(&engine, loopback(), "/v1/responses", vec![], body).await;
    assert_eq!(status, 400);
    assert!(transport.requests().is_empty());
}

#[tokio::test]
async fn prelude_mode_commits_output_without_replaying_a_later_error() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", Outcome::Status { status: 200, headers: vec![("content-type".into(), "text/event-stream".into())], chunks: vec![
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"visible\"}\n\n".to_vec(),
        b"data: {\"type\":\"response.failed\",\"response\":{\"status\":\"failed\",\"error\":{\"code\":\"server_error\"}}}\n\n".to_vec(),
    ] });
    let mut c = config();
    c.retry.max_stream_error_retries = 3;
    c.request_policy.stream_protection = sumpter_core::request_policy::StreamProtection::Prelude;
    let engine = engine_with(c, transport.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&body).contains("visible"));
    assert_eq!(transport.requests().len(), 1);
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.kind == "client" && e.is_failed())
    );
}

#[tokio::test]
async fn rpm_wait_timeout_does_not_issue_an_extra_upstream_request() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", reply(200, b"{}"));
    let mut c = config();
    c.request_policy.global.requests_per_minute = 1;
    c.request_policy.global.queue_timeout_seconds = 1;
    let engine = engine_with(c, transport.clone());
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body()
        )
        .await
        .0,
        200
    );
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 429);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["error"],
        "queue_timeout"
    );
    assert_eq!(transport.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn response_wait_timeout_bounds_backoff_headers_queue_and_buffer() {
    for stage in ["headers", "backoff", "buffer", "queue"] {
        let transport = FakeTransport::new();
        let mut c = config();
        c.retry.max_deferred_rounds = 0;
        c.retry.response_timeout_seconds = Some(200.0);
        c.request_policy.response_wait_timeout_seconds = 1;
        if stage == "queue" {
            c.request_policy.global.max_concurrent = 1;
            transport.push("a.example.com", Outcome::Gated { status: 200 });
        }
        match stage {
            "headers" => transport.push("a.example.com", Outcome::Hang),
            "backoff" => transport.push(
                "a.example.com",
                Outcome::Status {
                    status: 429,
                    headers: vec![("retry-after".into(), "300".into())],
                    chunks: vec![],
                },
            ),
            "buffer" => {
                c.retry.max_stream_error_retries = 1;
                transport.push("a.example.com", Outcome::GatedSse);
            }
            _ => {}
        }
        let engine = engine_with(c, transport.clone());
        let held = if stage == "queue" {
            Some(
                engine
                    .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
                    .await,
            )
        } else {
            None
        };
        let started = tokio::time::Instant::now();
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/responses",
                vec![],
                codex_responses_body(),
            )
            .await;
        assert_eq!(response.status(), 504, "{stage}");
        assert!(started.elapsed() <= Duration::from_millis(1050), "{stage}");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["failureKind"], "response_wait_timeout", "{stage}");
        assert_eq!(json["timeoutMS"], 1000);
        assert!(json.get("upstreamStatusCode").is_none());
        let attempts = transport.requests().len();
        tokio::time::sleep(Duration::from_secs(5)).await;
        assert_eq!(
            transport.requests().len(),
            attempts,
            "deadline must drop forward work"
        );
        let snapshot = engine.runtime_snapshot();
        let client = snapshot
            .recent_events
            .iter()
            .find(|e| e.failure_kind == Some(RuntimeFailureKind::ResponseWaitTimeout))
            .unwrap();
        assert_eq!(client.outcome, Some(RuntimeEventOutcome::Failed));
        assert_eq!(client.ttfb_ms, None);
        drop(held);
    }
}

#[tokio::test(start_paused = true)]
async fn response_wait_timeout_ends_at_commit_and_does_not_cut_long_output() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", Outcome::Gated { status: 200 });
    let mut c = config();
    c.request_policy.response_wait_timeout_seconds = 1;
    let engine = engine_with(c, transport.clone());
    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert_eq!(response.status(), 200);
    tokio::time::sleep(Duration::from_secs(3)).await;
    let tx = transport.gate_sender();
    tx.send(Ok(Bytes::from_static(
        b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    )))
    .unwrap();
    drop(tx);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("message_stop"));
}

#[tokio::test(start_paused = true)]
async fn upstream_200_second_timeout_retries_without_ending_client_request() {
    let transport = FakeTransport::new();
    transport.push("a.example.com", Outcome::Hang);
    transport.push(
        "b.example.com",
        sse_ok(&["event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"]),
    );
    let mut c = two_endpoint_config();
    c.retry.max_deferred_rounds = 0;
    c.retry.response_timeout_seconds = Some(200.0);
    let engine = engine_with(c, transport.clone());
    let started = tokio::time::Instant::now();
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(transport.requests().len(), 2);
    assert!(started.elapsed() >= Duration::from_secs(200));
    assert!(engine.runtime_snapshot().recent_events.iter().any(
        |e| e.kind == "upstream" && e.failure_kind == Some(RuntimeFailureKind::ResponseTimeout)
    ));
}
