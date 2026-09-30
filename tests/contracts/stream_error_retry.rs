//! The same HTTP 200 stream retry contract runs through both adapters.
use super::*;

const COMPLETED: &str = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n";

fn failed(code: &str) -> String {
    format!(
        "data: {{\"type\":\"response.failed\",\"response\":{{\"error\":{{\"code\":\"{code}\"}},\"output\":[]}}}}\n\n"
    )
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
    ]);
    payloads.push("data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n".into());
    payloads.push("data: {\"type\":\"response.failed\",\"response\":{\"output\":[{\"type\":\"function_call\"}],\"error\":{\"code\":\"server_error\"}}}\n\n".into());
    for payload in payloads {
        let fake = FakeTransport::new();
        fake.push("a.example.com", sse_ok(&[&payload]));
        let engine = engine_with(config(2, true), fake.clone());
        assert_eq!(invoke(&engine).await.1, payload.as_bytes());
        assert_eq!(fake.requests().len(), 1);
    }
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
