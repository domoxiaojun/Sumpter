//! Same native Gemini contract runs through both platform adapters.
use super::*;
use axum::body::to_bytes;

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

/// Vertex 形状的路径(`.../publishers/google/models/<id>:generateContent`)不带
/// `/v1beta/models/` 前缀,不是 Gemini Developer API 的会话入口:它必须继续走 Raw
/// 透传,不能被会话分支抢走、再因取不到模型名而 400。
#[tokio::test]
async fn vertex_shaped_paths_are_not_hijacked_by_the_session_branch() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![b"{\"candidates\":[]}".to_vec()],
        },
    );
    let engine = engine_with(config(), fake.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        "/v1/projects/p/locations/l/publishers/google/models/gemini-test:generateContent?model=gemini-test",
        headers(),
        Bytes::from_static(b"{\"contents\":[]}"),
    )
    .await;
    assert_eq!(
        status,
        200,
        "Vertex 形状的路径应继续 Raw 透传: {}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn gemini_native_auxiliary_methods_and_json_array_stream() {
    for (operation, response) in [
        (
            "batchEmbedContents",
            r#"{"embeddings":[{"values":[0.1]}],"usageMetadata":{"promptTokenCount":12}}"#,
        ),
        ("batchGenerateContent", r#"{"name":"batches/demo"}"#),
        ("asyncBatchEmbedContent", r#"{"name":"batches/demo"}"#),
        (
            "predict",
            r#"{"predictions":[{"bytesBase64Encoded":"AA=="}]}"#,
        ),
        (
            "predictLongRunning",
            r#"{"name":"operations/demo","done":false}"#,
        ),
        (
            "streamGenerateContent",
            r#"[{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]},{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"toolUsePromptTokenCount":3,"candidatesTokenCount":4,"thoughtsTokenCount":2}}]"#,
        ),
    ] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: response.as_bytes().chunks(13).map(<[u8]>::to_vec).collect(),
            },
        );
        let engine = engine_with(config(), fake.clone());
        let body = br#"{ "requests":[{"model":"models/gemini-test","content":{"parts":[{"text":"literal gemini-test"}]}}] }"#;
        let (status, received) = call(
            &engine,
            loopback(),
            &format!("/v1alpha/models/gemini-test:{operation}"),
            headers(),
            Bytes::from_static(body),
        )
        .await;
        assert_eq!(status, 200, "{operation}");
        assert_eq!(received, response.as_bytes());
        let sent = fake.requests();
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0].path,
            format!("/v1beta/models/gemini-upstream:{operation}")
        );
        let request: Value = serde_json::from_slice(&sent[0].body).unwrap();
        assert_eq!(request["requests"][0]["model"], "models/gemini-upstream");
        assert_eq!(
            request["requests"][0]["content"]["parts"][0]["text"],
            "literal gemini-test"
        );
        let runtime = engine.runtime_snapshot();
        let event = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .unwrap();
        assert!(event.is_succeeded(), "{operation}: {event:?}");
        if operation == "streamGenerateContent" {
            let usage = event.stream_trace.as_ref().unwrap().usage.as_ref().unwrap();
            assert_eq!(usage.input_tokens, Some(13));
            assert_eq!(usage.output_tokens, Some(6));
        }
    }
}

#[tokio::test]
async fn gemini_native_resources_preserve_method_body_headers_and_upload_handoff() {
    for (method, path, body, response) in [
        ("GET", "/v1beta/models", "", "{\"models\":[]}"),
        ("GET", "/v1beta/files/demo", "", "{\"name\":\"files/demo\"}"),
        ("DELETE", "/v1beta/files/demo", "", "{}"),
        (
            "PATCH",
            "/v1beta/cachedContents/demo?updateMask=ttl",
            "{ \"ttl\":\"300s\" }",
            "{\"name\":\"cachedContents/demo\"}",
        ),
        (
            "GET",
            "/v1beta/batches/demo",
            "",
            "{\"name\":\"batches/demo\"}",
        ),
        (
            "POST",
            "/upload/v1beta/files?upload_id=opaque%2Fid",
            "raw bytes",
            "{}",
        ),
        (
            "GET",
            "/download/v1beta/files/demo:download?alt=media",
            "",
            "file bytes",
        ),
    ] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![
                    ("content-type".into(), "application/octet-stream".into()),
                    (
                        "x-goog-upload-url".into(),
                        "https://a.example.com/upload/v1beta/files?upload_id=opaque%2Fid".into(),
                    ),
                ],
                chunks: vec![response.as_bytes().to_vec()],
            },
        );
        let mut config = config();
        config.endpoints[0].capabilities = vec![sumpter_core::capability::ModelCapability::Files];
        let engine = engine_with(config, fake.clone());
        let mut incoming = headers();
        incoming.retain(|(name, _)| name != "content-type");
        incoming.extend([
            ("host".into(), "proxy.example.invalid".into()),
            ("content-type".into(), "application/octet-stream".into()),
            ("x-goog-upload-command".into(), "upload, finalize".into()),
            ("x-goog-upload-offset".into(), "0".into()),
        ]);
        let result = engine
            .handle_request(
                loopback(),
                method,
                path,
                incoming,
                Bytes::copy_from_slice(body.as_bytes()),
            )
            .await;
        assert_eq!(result.status().as_u16(), 200, "{path}");
        assert_eq!(
            result.headers()["x-goog-upload-url"],
            "http://proxy.example.invalid/upload/v1beta/files?upload_id=opaque%2Fid"
        );
        assert_eq!(
            to_bytes(result.into_body(), usize::MAX).await.unwrap(),
            response.as_bytes()
        );
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, method);
        assert_eq!(requests[0].path, path);
        assert_eq!(requests[0].body, body.as_bytes());
        assert!(requests[0].headers.iter().any(|(name, value)| name == "x-goog-upload-command" && value == "upload, finalize"));
        assert!(
            requests[0]
                .headers
                .iter()
                .any(|(name, value)| name == "x-goog-api-key" && value == "provider-secret")
        );
    }
}

#[tokio::test]
async fn gemini_project_resources_require_an_unambiguous_native_endpoint_and_files_opt_in() {
    for selector in [None, Some("a")] {
        let fake = FakeTransport::new();
        let mut config = config();
        let mut second = config.endpoints[0].clone();
        second.id = "another-project".into();
        config.endpoints.push(second);
        let first_id = config.endpoints[0].id.clone();
        let engine = engine_with(config, fake.clone());
        let mut incoming = headers();
        if selector.is_some() {
            incoming.push(("x-sumpter-endpoint-id".into(), first_id));
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status: 200,
                    headers: vec![],
                    chunks: vec![b"{}".to_vec()],
                },
            );
        }
        let response = engine
            .handle_request(
                loopback(),
                "GET",
                "/v1beta/cachedContents/demo",
                incoming,
                Bytes::new(),
            )
            .await;
        assert_eq!(
            response.status().as_u16(),
            if selector.is_some() { 200 } else { 400 }
        );
        assert_eq!(fake.requests().len(), usize::from(selector.is_some()));
    }
    let fake = FakeTransport::new();
    let engine = engine_with(config(), fake.clone());
    assert_eq!(
        engine
            .handle_request(
                loopback(),
                "DELETE",
                "/v1beta/files/demo",
                headers(),
                Bytes::new()
            )
            .await
            .status()
            .as_u16(),
        400
    );
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn gemini_interactions_stream_keeps_native_state_and_accounts_usage() {
    let fake = FakeTransport::new();
    let response = concat!(
        "event: interaction.created\ndata: {\"event_type\":\"interaction.created\",\"interaction\":{\"object\":\"interaction\",\"status\":\"in_progress\"}}\n\n",
        "event: interaction.completed\ndata: {\"event_type\":\"interaction.completed\",\"interaction\":{\"object\":\"interaction\",\"status\":\"completed\",\"usage\":{\"total_input_tokens\":20,\"total_tool_use_tokens\":3,\"total_output_tokens\":7,\"total_thought_tokens\":2,\"total_cached_tokens\":5}}}\n\n"
    );
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: response.as_bytes().chunks(11).map(<[u8]>::to_vec).collect(),
        },
    );
    let engine = engine_with(config(), fake.clone());
    let body = br#"{"model":"gemini-test","input":"hi","stream":true,"store":false,"previous_interaction_id":"native-state","generation_config":{"thinking_level":"high"}}"#;
    let (status, received) = call(
        &engine,
        loopback(),
        "/v1beta/interactions",
        headers(),
        Bytes::from_static(body),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(received, response.as_bytes());
    let sent: Value = serde_json::from_slice(&fake.requests()[0].body).unwrap();
    assert_eq!(sent["model"], "gemini-upstream");
    assert_eq!(sent["store"], false);
    assert_eq!(sent["previous_interaction_id"], "native-state");
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert!(event.is_succeeded(), "{event:?}");
    let usage = event.stream_trace.as_ref().unwrap().usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(23));
    assert_eq!(usage.output_tokens, Some(9));
    assert_eq!(usage.reasoning_tokens, Some(2));
    assert_eq!(usage.cache_read_input_tokens, Some(5));
}
