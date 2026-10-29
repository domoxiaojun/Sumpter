//! The same HTTP 200 stream retry contract runs through both adapters.
use super::*;

const COMPLETED: &str = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n";

#[tokio::test]
async fn final_http_retry_headers_are_bounded_and_obey_visibility() {
    for visible in [false, true] {
        for status in [200, 400, 429, 503] {
            let fake = FakeTransport::new();
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status,
                    headers: vec![
                        ("content-type".into(), "text/event-stream".into()),
                        ("retry-after".into(), "900".into()),
                    ],
                    chunks: vec![COMPLETED.as_bytes().to_vec()],
                },
            );
            let mut config = config(0, false);
            config.endpoints.truncate(1);
            config.retry.max_deferred_rounds = 1;
            config.retry.session_sticky_retries = 0;
            config.retry.pass_through_retry_delay = visible;
            let engine = engine_with(config, fake.clone());
            let response = engine
                .handle_request(
                    loopback(),
                    "POST",
                    "/v1/responses",
                    vec![],
                    codex_responses_body(),
                )
                .await;
            assert_eq!(response.status().as_u16(), status);
            assert_eq!(
                response
                    .headers()
                    .get("retry-after")
                    .map(|v| v.to_str().unwrap()),
                visible.then_some("300")
            );
            let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            assert_eq!(fake.requests().len(), 1);
        }
    }
}

fn failed(code: &str) -> String {
    format!(
        "data: {{\"type\":\"response.failed\",\"response\":{{\"error\":{{\"code\":\"{code}\"}},\"output\":[]}}}}\n\n"
    )
}

#[tokio::test(start_paused = true)]
async fn stream_event_retry_after_uses_the_largest_valid_http_or_event_hint() {
    for (event_hint, expected) in [("3", 3.0), ("NaN", 1.0), ("-2", 1.0)] {
        let fake = FakeTransport::new();
        let payload = format!(
            "data: {{\"type\":\"response.failed\",\"response\":{{\"output\":[],\"error\":{{\"code\":\"slow_down\",\"headers\":{{\"Retry-After\":\"{event_hint}\"}}}}}}}}\n\n"
        );
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![
                    ("content-type".into(), "text/event-stream".into()),
                    ("retry-after".into(), "1".into()),
                ],
                chunks: vec![payload.into_bytes()],
            },
        );
        fake.push("a.example.com", sse_ok(&[COMPLETED]));
        let engine = engine_with(config(1, false), fake.clone());
        let started = tokio::time::Instant::now();
        assert_eq!(invoke(&engine).await.1, COMPLETED.as_bytes());
        let elapsed = started.elapsed().as_secs_f64();
        assert!(
            (elapsed - expected).abs() < 0.01,
            "{event_hint}: elapsed {elapsed}"
        );
        assert_eq!(fake.requests().len(), 2);
    }
}

#[tokio::test(start_paused = true)]
async fn long_retry_after_and_request_budget_cover_http_and_sse() {
    for status in [200, 429, 500, 503] {
        for (hint, budget, expected_wait, attempts) in [
            ("30", 0.0, 30.0, 2),
            ("300", 0.0, 300.0, 2),
            ("900", 0.0, 300.0, 2),
            ("300", 5.0, 0.0, 1),
        ] {
            let fake = FakeTransport::new();
            let error = format!(
                "data: {{\"type\":\"response.failed\",\"response\":{{\"output\":[],\"error\":{{\"code\":\"slow_down\",\"headers\":{{\"Retry-After\":\"{hint}\"}}}}}}}}\n\n"
            );
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status,
                    headers: vec![
                        ("content-type".into(), "text/event-stream".into()),
                        ("retry-after".into(), hint.into()),
                    ],
                    chunks: vec![error.as_bytes().to_vec()],
                },
            );
            fake.push("a.example.com", sse_ok(&[COMPLETED]));
            let mut config = config(1, false);
            config.endpoints.truncate(1);
            config.retry.max_retry_duration_seconds = budget;
            config.retry.max_500_retries = 1;
            config.retry.max_deferred_rounds = 2;
            let engine = engine_with(config.normalized(), fake.clone());
            let started = tokio::time::Instant::now();
            let (actual_status, bytes) = call(
                &engine,
                loopback(),
                "/v1/responses",
                vec![],
                codex_responses_body(),
            )
            .await;
            assert_eq!(
                fake.requests().len(),
                attempts,
                "status {status}, hint {hint}, budget {budget}"
            );
            assert!((started.elapsed().as_secs_f64() - expected_wait).abs() < 0.1);
            if attempts == 2 {
                assert_eq!(&bytes[..], COMPLETED.as_bytes());
            } else {
                assert_eq!(actual_status, status);
            }
        }
    }
}

#[tokio::test]
async fn responses_bridge_output_limits_keep_usage_and_never_count_as_success() {
    for stream in [false, true] {
        let fake = FakeTransport::new();
        let input = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"synthetic\",\"usage\":{\"input_tokens\":7}}}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"partial\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":3}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
        fake.push("a.example.com", sse_ok(&[input]));
        let engine = engine_with(two_endpoint_config(), fake.clone());
        let (status, bytes) = call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            Bytes::from(
                serde_json::to_vec(
                    &json!({"model":"claude-opus-5", "input":"hi", "stream":stream}),
                )
                .unwrap(),
            ),
        )
        .await;
        assert_eq!(status, 200);
        let rendered = String::from_utf8(bytes.to_vec()).unwrap();
        let response: Value = if stream {
            let terminal: Value = rendered
                .split("\n\n")
                .filter_map(|block| block.lines().find_map(|line| line.strip_prefix("data: ")))
                .filter_map(|data| serde_json::from_str::<Value>(data).ok())
                .find(|event| event["type"] == "response.incomplete")
                .expect("incomplete event");
            assert!(!rendered.contains("response.completed"));
            terminal["response"].clone()
        } else {
            serde_json::from_str(&rendered).unwrap()
        };
        assert_eq!(
            response["incomplete_details"]["reason"],
            "max_output_tokens"
        );
        assert_eq!(response["usage"]["input_tokens"], 7);
        assert_eq!(response["usage"]["output_tokens"], 3);
        let runtime = runtime_of(&engine).await;
        assert_eq!(runtime.client_successes, 0);
        assert_eq!(runtime.client_failures, 1);
        assert!(runtime.recent_events.iter().any(
            |event| event.failure_kind == Some(RuntimeFailureKind::UpstreamResponseIncomplete)
        ));
        assert_eq!(fake.requests().len(), 1);
    }
}

fn config(retries: i64, failover: bool) -> AppConfig {
    let mut config = two_endpoint_config();
    for endpoint in &mut config.endpoints {
        endpoint.protocol = EndpointProtocolMode::Auto;
    }
    config.retry.max_stream_error_retries = retries;
    config.retry.failover_on_stream_error = failover;
    // These settings must not turn stream errors into unbounded retries.
    config.retry.max_deferred_rounds = 0;
    config.retry.session_sticky_retries = 3;
    config.normalized()
}

async fn invoke(engine: &Engine) -> (u16, Vec<u8>) {
    let (status, body) = tokio::time::timeout(
        Duration::from_secs(10),
        call(
            engine,
            loopback(),
            "/v1/responses",
            vec![],
            codex_responses_body(),
        ),
    )
    .await
    .unwrap();
    (status, body.to_vec())
}

#[tokio::test]
async fn stream_errors_retry_then_recover_with_true_attempt_outcomes() {
    for code in [
        "rate_limit_exceeded",
        "server_error",
        "internal_server_error",
        "overloaded_error",
        "service_unavailable",
        "slow_down",
        "server_is_overloaded",
    ] {
        let fake = FakeTransport::new();
        let error = failed(code);
        // Lifecycle prelude, split frames, CRLF and a per-attempt request ID.
        let prelude = "data: {\"type\":\"response.created\",\"response\":{\"output\":[]}}\r\n\r\n";
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![
                    ("content-type".into(), "text/event-stream".into()),
                    ("x-request-id".into(), "failed-attempt".into()),
                ],
                chunks: [prelude.as_bytes(), error.as_bytes()]
                    .concat()
                    .chunks(7)
                    .map(<[u8]>::to_vec)
                    .collect(),
            },
        );
        fake.push("a.example.com", sse_ok(&[COMPLETED]));
        let engine = engine_with(config(1, false), fake.clone());
        let (status, bytes) = invoke(&engine).await;
        assert_eq!(status, 200);
        assert_eq!(bytes, COMPLETED.as_bytes());
        assert_eq!(
            fake.requests()
                .iter()
                .map(|r| r.host.as_str())
                .collect::<Vec<_>>(),
            ["a.example.com", "a.example.com"]
        );
        let runtime = runtime_of(&engine).await;
        let failures = runtime
            .recent_events
            .iter()
            .filter(|event| {
                event.kind == "upstream" && event.outcome == Some(RuntimeEventOutcome::Failed)
            })
            .collect::<Vec<_>>();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].status_code, 200);
        assert_eq!(failures[0].upstream_status_code, Some(200));
        assert_eq!(
            failures[0].upstream_request_id.as_deref(),
            Some("failed-attempt")
        );
        assert_eq!(
            failures[0].failure_kind,
            Some(RuntimeFailureKind::UpstreamResponseFailed)
        );
        assert!(failures[0].failure_detail.as_ref().unwrap().contains(code));
        assert_eq!(runtime.client_successes, 1);
        assert_eq!(runtime.client_failures, 0);
    }
}

#[tokio::test]
async fn stream_ends_before_first_output_retries_before_committing_200_sse() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: vec![],
        },
    );
    fake.push("a.example.com", sse_ok(&[COMPLETED]));
    let engine = engine_with(config(1, false), fake.clone());
    let (status, bytes) = invoke(&engine).await;
    assert_eq!(status, 200);
    assert_eq!(bytes, COMPLETED.as_bytes());
    assert_eq!(fake.requests().len(), 2);
    let runtime = runtime_of(&engine).await;
    assert_eq!(runtime.client_successes, 1);
    assert_eq!(runtime.client_failures, 0);
    assert_eq!(runtime.upstream_failures, 1);
    assert_eq!(runtime.upstream_successes, 1);
}

#[tokio::test]
async fn stream_prelude_exhaustion_returns_proxy_error_instead_of_broken_sse() {
    let fake = FakeTransport::new();
    for _ in 0..2 {
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                chunks: vec![],
            },
        );
    }
    let engine = engine_with(config(1, false), fake.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/responses",
        vec![],
        codex_responses_body(),
    )
    .await;
    assert_eq!(status, 502);
    let json: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "upstream_unavailable");
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn stream_errors_exhaust_finite_budget_and_preserve_last_failure() {
    for (retries, failover, expected) in [(0, false, 1), (1, false, 2), (0, true, 2), (1, true, 4)]
    {
        let fake = FakeTransport::new();
        let first = failed("rate_limit_exceeded");
        let last = failed("server_error");
        for _ in 0..=retries {
            fake.push("a.example.com", sse_ok(&[&first]));
            fake.push("b.example.com", sse_ok(&[&last]));
        }
        let engine = engine_with(config(retries, failover), fake.clone());
        let (status, bytes) = tokio::time::timeout(Duration::from_secs(5), invoke(&engine))
            .await
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            bytes,
            if failover {
                last.as_bytes()
            } else {
                first.as_bytes()
            }
        );
        assert_eq!(fake.requests().len(), expected);
        let runtime = runtime_of(&engine).await;
        assert_eq!(runtime.client_failures, 1);
        assert_eq!(runtime.client_successes, 0);
        assert_eq!(
            runtime
                .recent_events
                .iter()
                .filter(|e| e.kind == "upstream" && e.outcome == Some(RuntimeEventOutcome::Failed))
                .count(),
            expected
        );
    }
}

#[tokio::test]
async fn stream_errors_failover_setting_is_independent_of_http_500() {
    let fake = FakeTransport::new();
    let mut config = config(0, true);
    config.retry.failover_on_500 = false;
    fake.push("a.example.com", sse_ok(&[&failed("server_error")]));
    fake.push("b.example.com", sse_ok(&[COMPLETED]));
    let engine = engine_with(config, fake.clone());
    assert_eq!(invoke(&engine).await.1, COMPLETED.as_bytes());
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn stream_errors_background_requests_are_not_replayed() {
    let fake = FakeTransport::new();
    let error = failed("server_error");
    fake.push("a.example.com", sse_ok(&[&error]));
    let engine = engine_with(config(1, true), fake.clone());
    let mut body: Value = serde_json::from_slice(&codex_responses_body()).unwrap();
    body["background"] = json!(true);
    let (status, bytes) = tokio::time::timeout(
        Duration::from_secs(5),
        call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        ),
    )
    .await
    .unwrap();
    assert_eq!(status, 200);
    assert_eq!(bytes, error.as_bytes());
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn stream_errors_never_replay_output_or_non_transient_errors() {
    let error = failed("server_error");
    let outputs = [
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"thinking\"}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\"}}\n\n",
        "data: {\"type\":\"unknown.event\"}\n\n",
        "data: invalid-json\n\n",
    ];
    let mut payloads = outputs
        .iter()
        .map(|prefix| format!("{prefix}{error}"))
        .collect::<Vec<_>>();
    payloads.extend([
        failed("invalid_api_key"),
        failed("invalid_request_error"),
        failed("unknown_error"),
        failed("credit_balance_exhausted"),
        failed("organization_spend_limit_exceeded"),
        failed("organization_usage_limit_exceeded"),
        failed("project_spend_limit_exceeded"),
    ]);
    payloads.push("data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n".into());
    payloads.push("data: {\"type\":\"response.failed\",\"response\":{\"output\":[{\"type\":\"function_call\"}],\"error\":{\"code\":\"server_error\"}}}\n\n".into());
    for payload in payloads {
        let fake = FakeTransport::new();
        fake.push("a.example.com", sse_ok(&[&payload]));
        if payload.contains("\"code\":\"server_error\"")
            || payload.contains("\"code\":\"rate_limit_exceeded\"")
        {
            // Retry-safe buffering now retries transient failures even when
            // the same SSE chunk already contained output. The client sees
            // only the recovered complete attempt.
            fake.push("a.example.com", sse_ok(&[COMPLETED]));
        }
        let engine = engine_with(config(2, true), fake.clone());
        if payload.contains("\"code\":\"server_error\"") {
            assert_eq!(invoke(&engine).await.1, COMPLETED.as_bytes());
            assert_eq!(fake.requests().len(), 2);
        } else {
            assert_eq!(invoke(&engine).await.1, payload.as_bytes());
            assert_eq!(fake.requests().len(), 1);
        }
    }
}

#[tokio::test]
async fn transient_failure_after_partial_output_is_hidden_until_retry_completes() {
    let fake = FakeTransport::new();
    let partial_then_failed = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\ndata: {\"type\":\"response.failed\",\"response\":{\"output\":[],\"error\":{\"code\":\"server_error\"}}}\n\n".to_string();
    fake.push("a.example.com", sse_ok(&[&partial_then_failed]));
    fake.push("a.example.com", sse_ok(&[COMPLETED]));
    let engine = engine_with(config(1, false), fake.clone());
    let (status, bytes) = invoke(&engine).await;
    assert_eq!(status, 200);
    assert_eq!(bytes, COMPLETED.as_bytes());
    assert!(!String::from_utf8_lossy(&bytes).contains("partial"));
    assert_eq!(fake.requests().len(), 2);
}

#[tokio::test]
async fn stream_errors_budget_does_not_reset_in_deferred_rounds() {
    let fake = FakeTransport::new();
    let mut config = config(1, true);
    config.retry.max_deferred_rounds = 2;
    config.retry.session_sticky_retries = 0;
    for _ in 0..2 {
        fake.push("a.example.com", sse_ok(&[&failed("server_error")]));
    }
    fake.push(
        "b.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    fake.push("b.example.com", sse_ok(&[COMPLETED]));
    let engine = engine_with(config, fake.clone());
    assert_eq!(invoke(&engine).await.1, COMPLETED.as_bytes());
    assert_eq!(
        fake.requests()
            .iter()
            .map(|r| r.host.as_str())
            .collect::<Vec<_>>(),
        [
            "a.example.com",
            "a.example.com",
            "b.example.com",
            "b.example.com"
        ]
    );
}
