//! Both platform routers must accept complete bodies above the former 64 MiB cap.
use super::*;

const LARGE_BODY_BYTES: usize = 64 * 1024 * 1024 + 1;

#[tokio::test]
async fn request_size_large_http_bodies_reach_upstream_intact() {
    // Keep the large fixtures sequential so the full suite has bounded test memory use.
    for (path, prefix, suffix, content_type, chunked, text_pointer) in [
        (
            "/v1/messages",
            r#"{"model":"size-model","max_tokens":16,"messages":[{"role":"user","content":""#,
            r#""}]}"#,
            "application/json",
            false,
            Some("/messages/0/content"),
        ),
        (
            "/v1/responses",
            r#"{"model":"size-model","input":""#,
            r#""}"#,
            "application/json",
            true,
            Some("/input"),
        ),
        (
            "/v1/chat/completions",
            r#"{"model":"size-model","messages":[{"role":"user","content":""#,
            r#""}]}"#,
            "application/json",
            false,
            Some("/messages/0/content"),
        ),
        (
            "/opaque-upload?model=size-model",
            "",
            "",
            "application/octet-stream",
            false,
            None,
        ),
        (
            "/v1/images/edits",
            "--size-boundary\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nsize-model\r\n\
             --size-boundary\r\nContent-Disposition: form-data; name=\"image\"; filename=\"test.png\"\r\n\
             Content-Type: image/png\r\n\r\n",
            "\r\n--size-boundary--\r\n",
            "multipart/form-data; boundary=size-boundary",
            true,
            None,
        ),
    ] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: vec![br#"{"ok":true}"#.to_vec()],
            },
        );
        let mut config = native_passthrough_config("size-model");
        config.listener.auth_token = "size-test-token".into();
        if path == "/v1/images/edits" {
            config.endpoints[0]
                .mappings
                .last_mut()
                .unwrap()
                .capabilities = vec![sumpter_core::capability::ModelCapability::Image];
        }
        let engine = engine_with(config, fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = server::router(engine).layer(axum::middleware::from_fn(
            move |request: axum::http::Request<Body>, next: axum::middleware::Next| async move {
                assert_eq!(request.headers().contains_key("content-length"), !chunked);
                assert_eq!(request.headers().contains_key("transfer-encoding"), chunked);
                next.run(request).await
            },
        ));
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });

        let mut payload = prefix.as_bytes().to_vec();
        payload.resize(payload.len() + LARGE_BODY_BYTES, b'x');
        payload.extend_from_slice(suffix.as_bytes());
        let payload = Bytes::from(payload);
        let body = if chunked {
            let bytes = payload.clone();
            let chunks = (0..bytes.len()).step_by(1024 * 1024).map(move |start| {
                Ok::<_, std::io::Error>(bytes.slice(start..(start + 1024 * 1024).min(bytes.len())))
            });
            reqwest::Body::wrap_stream(futures_util::stream::iter(chunks))
        } else {
            reqwest::Body::from(payload.clone())
        };
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap()
            .post(format!("http://{address}{path}"))
            .bearer_auth("size-test-token")
            .header("content-type", content_type)
            .body(body)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let reply = response.text().await.unwrap();
        assert_eq!(status.as_u16(), 200, "{path}: {reply}");
        {
            let requests = fake.requests.lock().unwrap();
            assert_eq!(requests.len(), 1, "{path}");
            let sent = &requests[0];
            if let Some(pointer) = text_pointer {
                let json: Value = serde_json::from_slice(&sent.body).unwrap();
                let text = json.pointer(pointer).unwrap().as_str().unwrap();
                assert_eq!(text.len(), LARGE_BODY_BYTES, "{path}");
                assert!(text.bytes().all(|byte| byte == b'x'), "{path}");
            } else {
                assert!(
                    sent.body.as_slice() == payload.as_ref(),
                    "{path}: body changed"
                );
                assert!(sent.headers.iter().any(|(name, value)| {
                    name.eq_ignore_ascii_case("content-type") && value == content_type
                }));
            }
        }
        task.abort();
        let _ = task.await;
    }
}

#[tokio::test]
async fn request_size_auth_precedes_reading_and_read_errors_are_not_size_errors() {
    let fake = FakeTransport::new();
    let mut config = native_passthrough_config("size-model");
    config.listener.auth_token = "size-test-token".into();
    let engine = engine_with(config, fake.clone());
    for path in [
        "/v1/messages",
        "/v1/responses",
        "/v1/chat/completions",
        "/opaque-upload?model=size-model",
    ] {
        let polled = Arc::new(AtomicBool::new(false));
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                path,
                vec![("content-encoding".into(), "gzip".into())],
                body_with_poll_flag(polled.clone()),
            )
            .await;
        assert_eq!(response.status().as_u16(), 401, "{path}");
        assert!(!polled.load(Ordering::SeqCst), "{path}");
    }
    let broken = Body::from_stream(futures_util::stream::once(async {
        Err::<Bytes, _>(std::io::Error::other("synthetic body read failure"))
    }));
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![
                ("authorization".into(), "Bearer size-test-token".into()),
                ("content-encoding".into(), "gzip".into()),
            ],
            broken,
        )
        .await;
    assert_eq!(response.status().as_u16(), 400);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(error["error"], "request_body_read_failed");
    assert!(fake.requests.lock().unwrap().is_empty());
}
