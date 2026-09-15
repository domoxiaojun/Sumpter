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

/// 转换面上的两轮工具调用:第二轮必须把第一轮真实收到的 `thoughtSignature`
/// 原样带回上游。客户端只看得到 Anthropic 方言,签名只有代理见过 —— 不回放就等于
/// 丢掉它,带思考的 Gemini 模型会拒绝这条多轮请求。
#[tokio::test]
async fn translated_gemini_replays_thought_signature_across_turns() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: vec![
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[",
                    "{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a\"}},",
                    "\"thoughtSignature\":\"sig-xyz\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                )
                .as_bytes()
                .to_vec(),
            ],
        },
    );
    // Claude 客户端 → 固定 Gemini 入口:走转换面。
    let mut config = config();
    config.endpoints[0].mappings[0].client_pattern = "claude-opus-5".into();
    let engine = engine_with(config, fake.clone());

    let session = vec![("session_id".to_string(), "gemini-replay-1".to_string())];
    let mut first_headers = headers();
    first_headers.extend(session.clone());
    let (status, first_body) = call(
        &engine,
        loopback(),
        "/v1/messages",
        first_headers,
        Bytes::from_static(
            br#"{"model":"claude-opus-5","max_tokens":64,"stream":true,
                 "messages":[{"role":"user","content":"read a"}],
                 "tools":[{"name":"read_file","input_schema":{"type":"object"}}]}"#,
        ),
    )
    .await;
    assert_eq!(
        status,
        200,
        "第一轮应成功: {} / requests={}",
        String::from_utf8_lossy(&first_body),
        fake.requests().len()
    );

    // 第二轮:同一个会话,回传工具结果。
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: vec![
                concat!(
                    "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[",
                    "{\"text\":\"done\"}]},\"finishReason\":\"STOP\"}]}\n\n"
                )
                .as_bytes()
                .to_vec(),
            ],
        },
    );
    let mut second_headers = headers();
    second_headers.extend(session);
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        second_headers,
        Bytes::from_static(
            br#"{"model":"claude-opus-5","max_tokens":64,"stream":true,
                 "messages":[
                   {"role":"user","content":"read a"},
                   {"role":"assistant","content":[{"type":"tool_use","id":"gemini_call_1",
                     "name":"read_file","input":{"path":"a"}}]},
                   {"role":"user","content":[{"type":"tool_result",
                     "tool_use_id":"gemini_call_1","content":"neirong"}]}],
                 "tools":[{"name":"read_file","input_schema":{"type":"object"}}]}"#,
        ),
    )
    .await;
    assert_eq!(status, 200, "第二轮应成功");

    let recorded = fake.requests();
    assert_eq!(recorded.len(), 2);
    let sent: Value = serde_json::from_slice(&recorded[1].body).unwrap();
    let parts = sent["contents"][1]["parts"].as_array().unwrap();
    assert_eq!(
        parts[0]["thoughtSignature"], "sig-xyz",
        "第二轮必须带回第一轮的签名: {sent}"
    );
    // 签名之外，历史轮次仍按真实 parts 回放。
    assert_eq!(parts[0]["functionCall"]["name"], "read_file");
}
