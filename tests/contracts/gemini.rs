//! Same native Gemini contract runs through both platform adapters.
use super::*;

fn config() -> AppConfig {
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    let ep = &mut config.endpoints[0];
    ep.base_url = "https://a.example.com/v1beta".into();
    ep.protocol = EndpointProtocolMode::Gemini;
    ep.api_key = "provider-secret".into();
    ep.mappings[0].client_pattern = "gemini-*".into();
    ep.mappings[0].upstream_model = "models/gemini-upstream".into();
    config.listener.auth_token = "listener-secret".into();
    config
}

fn headers() -> Vec<(String, String)> {
    vec![
        (
            "user-agent".into(),
            "GeminiCLI-acp/0.1/gemini-test (darwin; arm64; cli)".into(),
        ),
        ("x-goog-api-key".into(), "listener-secret".into()),
        ("content-type".into(), "application/json".into()),
        ("x-sumpter-project".into(), "gemini-project".into()),
        ("x-sumpter-session-id".into(), "gemini-session".into()),
    ]
}

#[tokio::test]
async fn gemini_native_stream_preserves_bytes_and_accounts_late_usage() {
    let fake = FakeTransport::new();
    let engine = engine_with(config(), fake.clone());
    let response = concat!(
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"demo\"}},\"thoughtSignature\":\"opaque\"}]},\"finishReason\":\"STOP\"}]}\n\n",
        "data: {\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":8,\"thoughtsTokenCount\":3,\"cachedContentTokenCount\":5}}\n\n"
    );
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: response.as_bytes().chunks(17).map(<[u8]>::to_vec).collect(),
        },
    );
    let body = br#"{ "contents":[{"role":"user","parts":[{"text":"hi"},{"inlineData":{"mimeType":"image/png","data":"AA=="}}]}],"tools":[{"functionDeclarations":[{"name":"read_file","parameters":{"type":"OBJECT"}}]}],"generationConfig":{"thinkingConfig":{"includeThoughts":true}},"cachedContent":"cachedContents/demo" }"#;
    let (status, received) = call(
        &engine,
        loopback(),
        "/v1beta/models/gemini-test:streamGenerateContent?alt=sse&trace=a%2Fb",
        headers(),
        Bytes::from_static(body),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(received, response.as_bytes());
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let sent = &requests[0];
    assert_eq!(
        sent.path,
        "/v1beta/models/gemini-upstream:streamGenerateContent?alt=sse&trace=a%2Fb"
    );
    assert_eq!(sent.body, body);
    assert!(
        sent.headers
            .iter()
            .any(|(n, v)| n == "x-goog-api-key" && v == "provider-secret")
    );
    assert!(
        !sent
            .headers
            .iter()
            .any(|(n, v)| n.starts_with("x-sumpter-") || v == "listener-secret")
    );
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert!(event.is_succeeded(), "{event:?}");
    assert_eq!(event.client_kind, Some(ClientKind::GeminiCli));
    assert_eq!(event.session_id.as_deref(), Some("gemini-session"));
    assert_eq!(
        event.client_declared.as_ref().unwrap().project.as_deref(),
        Some("gemini-project")
    );
    assert_eq!(event.source_format, Some(ProviderProtocol::Gemini));
    let serialized = serde_json::to_value(event).unwrap();
    // Find the summary in the stable stream trace rather than treating HTTP 200 as success.
    let trace = serialized.get("streamTrace").unwrap();
    assert!(!trace.to_string().contains("promptTokenCount"));
    assert_eq!(event.tool_calls.as_ref().unwrap(), &["read_file"]);
    let usage = event.stream_trace.as_ref().unwrap().usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(20));
    assert_eq!(usage.output_tokens, Some(11));
    assert_eq!(usage.reasoning_tokens, Some(3));
    assert_eq!(usage.cache_read_input_tokens, Some(5));
}

#[tokio::test]
async fn gemini_native_unary_and_auxiliary_operations() {
    for (operation, response) in [
        (
            "generateContent",
            r#"{"candidates":[{"finishReason":"STOP","content":{"parts":[{"text":"ok"}]}}]}"#,
        ),
        ("countTokens", r#"{"totalTokens":19}"#),
        ("embedContent", r#"{"embedding":{"values":[0.1,0.2]}}"#),
    ] {
        let fake = FakeTransport::new();
        let engine = engine_with(config(), fake.clone());
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: vec![response.as_bytes().to_vec()],
            },
        );
        let (status, received) = call(
            &engine,
            loopback(),
            &format!("/v1beta/models/gemini-test:{operation}"),
            headers(),
            Bytes::from_static(b"{\"contents\":[]}"),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(received, response.as_bytes());
        let runtime = engine.runtime_snapshot();
        let event = runtime
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap();
        assert!(event.is_succeeded(), "{operation}: {event:?}");
    }
}

#[tokio::test]
async fn gemini_native_rejects_invalid_auth_method_query_and_mapping() {
    for (method, path, auth, status) in [
        (
            "POST",
            "/v1beta/models/gemini-test:generateContent",
            false,
            401,
        ),
        (
            "GET",
            "/v1beta/models/gemini-test:generateContent",
            true,
            405,
        ),
        (
            "POST",
            "/v1beta/models/gemini-test:streamGenerateContent",
            true,
            400,
        ),
        (
            "POST",
            "/v1beta/models/gemini-test:generateContent?%6Bey=secret",
            true,
            400,
        ),
    ] {
        let fake = FakeTransport::new();
        let engine = engine_with(config(), fake.clone());
        let mut hs = headers();
        if !auth {
            hs.retain(|(n, _)| n != "x-goog-api-key");
        }
        let response = engine
            .handle_request(loopback(), method, path, hs, Bytes::from_static(b"{}"))
            .await;
        assert_eq!(response.status().as_u16(), status, "{path}");
        assert!(fake.requests().is_empty());
    }
    for (protocol, mapping) in [
        (EndpointProtocolMode::Gemini, "../../escape"),
        (EndpointProtocolMode::Anthropic, "gemini-upstream"),
    ] {
        let fake = FakeTransport::new();
        let mut config = config();
        config.endpoints[0].protocol = protocol;
        config.endpoints[0].mappings[0].upstream_model = mapping.into();
        let engine = engine_with(config, fake.clone());
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1beta/models/gemini-test:generateContent",
                headers(),
                Bytes::from_static(b"{}")
            )
            .await
            .0,
            400
        );
        assert!(fake.requests().is_empty());
    }
}

#[tokio::test]
async fn gemini_native_truncated_or_blocked_body_is_not_success() {
    for response in [
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
        "data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n",
        "data: {\"candidates\":[{\"finishReason\":\"MAX_TOKENS\"}]}\n\n",
    ] {
        let fake = FakeTransport::new();
        let engine = engine_with(config(), fake.clone());
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                chunks: vec![response.as_bytes().to_vec()],
            },
        );
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
                headers(),
                Bytes::from_static(b"{}")
            )
            .await
            .1,
            response.as_bytes()
        );
        let runtime = engine.runtime_snapshot();
        assert!(
            !runtime
                .recent_events
                .iter()
                .find(|e| e.kind == "client")
                .unwrap()
                .is_succeeded()
        );
    }
}
