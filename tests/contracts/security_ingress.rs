//! Shared by Linux and macOS. Small injected limits exercise the same byte paths.
use super::*;

fn limited_config() -> AppConfig {
    let mut config = native_passthrough_config("size-model");
    config.listener.auth_token = "size-test-token".into();
    config.request_policy.ingress.max_body_mib = 1;
    config.request_policy.ingress.max_decoded_body_mib = 1;
    config.request_policy.ingress.max_buffered_mib = 2;
    config.request_policy.ingress.max_preprocessing = 1;
    config.request_policy.ingress.body_idle_timeout_seconds = 1;
    config.request_policy.ingress.body_read_timeout_seconds = 3;
    config
}
fn headers() -> Vec<(String, String)> {
    vec![
        ("authorization".into(), "Bearer size-test-token".into()),
        ("content-type".into(), "application/octet-stream".into()),
    ]
}

#[tokio::test]
#[ignore = "large 256 MiB boundary: run alone in authorized CI with --test-threads=1"]
async fn security_ingress_256_mib_boundary() {
    let fake = FakeTransport::new();
    let mut config = limited_config();
    config.request_policy.ingress = Default::default();
    let engine = engine_with(config, fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![],
            chunks: vec![],
        },
    );
    for (size, expected) in [(256 * 1024 * 1024, 200), (256 * 1024 * 1024 + 1, 413)] {
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                "/opaque-upload?model=size-model",
                headers(),
                Body::from(vec![b'x'; size]),
            )
            .await;
        assert_eq!(response.status().as_u16(), expected);
        drop(response);
        fake.requests.lock().unwrap().clear();
    }
}

#[tokio::test]
async fn security_ingress_exact_limit_and_reserved_admin_path() {
    let fake = FakeTransport::new();
    let engine = engine_with(limited_config(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![],
            chunks: vec![b"ok".to_vec()],
        },
    );
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/opaque-upload?model=size-model",
            headers(),
            Body::from(vec![b'x'; 1024 * 1024]),
        )
        .await;
    assert_eq!(response.status(), 200);
    drop(response);
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/opaque-upload?model=size-model",
            headers(),
            Body::from(vec![b'x'; 1024 * 1024 + 1]),
        )
        .await;
    assert_eq!(response.status(), 413);
    for path in [
        "/admin",
        "/admin/api/config",
        "/%61dmin/api",
        "/admin%2fapi",
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                path,
                headers(),
                body_with_poll_flag(polled.clone()),
            )
            .await;
        assert_eq!(response.status(), 404);
        assert!(!polled.load(Ordering::SeqCst));
    }
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn security_ingress_slow_body_and_cancel_release_preprocessing() {
    let fake = FakeTransport::new();
    let engine = engine_with(limited_config(), fake.clone());
    let pending =
        || Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>());
    let task_engine = engine.clone();
    let task = tokio::spawn(async move {
        task_engine
            .handle_request(loopback(), "POST", "/v1/responses", headers(), pending())
            .await
    });
    tokio::task::yield_now().await;
    let polled = Arc::new(AtomicBool::new(false));
    let rejected = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            headers(),
            body_with_poll_flag(polled.clone()),
        )
        .await;
    assert_eq!(rejected.status(), 429);
    assert_eq!(rejected.headers()["retry-after"], "1");
    assert!(!polled.load(Ordering::SeqCst));
    task.abort();
    let _ = task.await;
    let timeout = engine
        .handle_request(loopback(), "POST", "/v1/responses", headers(), pending())
        .await;
    assert_eq!(timeout.status(), 408);
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn security_ingress_budget_survives_queue_and_cancel() {
    let fake = FakeTransport::new();
    let mut config = limited_config();
    config.request_policy.global.max_concurrent = 1;
    config.request_policy.global.max_queued = 1;
    let engine = engine_with(config, fake.clone());
    fake.push("a.example.com", Outcome::Hang);
    let mut tasks = Vec::new();
    for _ in 0..2 {
        let engine = engine.clone();
        tasks.push(tokio::spawn(async move {
            engine
                .handle_request(
                    loopback(),
                    "POST",
                    "/opaque-upload?model=size-model",
                    headers(),
                    Body::from(vec![b'x'; 600 * 1024]),
                )
                .await
        }));
        // Each ready body is fully handled in its first poll, then waits at
        // the transport/admission boundary. No wall-clock delay is needed.
        tokio::task::yield_now().await;
    }
    assert!(tasks.iter().all(|task| !task.is_finished()));
    assert_eq!(fake.requests().len(), 1);
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/opaque-upload?model=size-model",
            headers(),
            Body::from(vec![b'x'; 600 * 1024]),
        )
        .await;
    assert_eq!(response.status(), 429);
    for task in tasks {
        task.abort();
        let _ = task.await;
    }
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![],
            chunks: vec![],
        },
    );
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/opaque-upload?model=size-model",
            headers(),
            Body::from(vec![b'x'; 600 * 1024]),
        )
        .await;
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn security_ingress_all_cookie_headers_are_filtered_for_raw_forwarding() {
    let fake = FakeTransport::new();
    let engine = engine_with(limited_config(), fake.clone());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![],
            chunks: vec![],
        },
    );
    let mut headers = headers();
    headers.extend([
        (
            "Cookie".into(),
            "a=one; sumpter_admin_session=synthetic-admin".into(),
        ),
        ("cookie".into(), "b=two".into()),
        ("Cookie".into(), "malformed".into()),
    ]);
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/opaque-upload?model=size-model",
            headers,
            Body::from("hello"),
        )
        .await;
    assert_eq!(response.status(), 200);
    let sent = fake.requests();
    let values: Vec<_> = sent[0]
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(values, ["a=one", "b=two"]);
}
