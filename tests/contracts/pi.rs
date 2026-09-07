//! Identical pi attribution contract for both platform adapters.
use super::*;

fn pi_config(protocol: EndpointProtocolMode) -> AppConfig {
    let mut config = two_endpoint_config();
    for endpoint in &mut config.endpoints {
        endpoint.protocol = protocol;
        endpoint.mappings = vec![ModelMapping {
            client_pattern: "pi-model".into(),
            context: ContextMode::Standard,
            failover_timeout_seconds: None,
            thinking: ThinkingMode::Passthrough,
            effort: None,
            upstream_model: String::new(),
            capabilities: Vec::new(),
        }];
    }
    config.listener.auth_token = "listener-secret".into();
    config
}

fn headers() -> Vec<(String, String)> {
    vec![
        ("user-agent".into(), "claude-cli/1".into()),
        ("x-sumpter-client".into(), "pi".into()),
        ("authorization".into(), "Bearer listener-secret".into()),
        ("content-type".into(), "application/json".into()),
        ("x-sumpter-workspace".into(), "/work/pi-project".into()),
        ("x-sumpter-project".into(), "pi-project".into()),
        ("x-sumpter-session-id".into(), "pi-session".into()),
        ("session_id".into(), "native-session".into()),
        ("originator".into(), "pi".into()),
    ]
}

fn assert_pi_events(engine: &Engine) {
    let events = engine.runtime_snapshot().recent_events;
    let events: Vec<_> = events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "client" | "upstream"))
        .collect();
    assert!(!events.is_empty());
    for event in events {
        assert_eq!(event.client_kind, Some(ClientKind::Pi));
        assert_eq!(event.session_id.as_deref(), Some("pi-session"));
        assert!(event.codex_metadata.is_none());
        assert_eq!(
            event
                .client_declared
                .as_ref()
                .and_then(|m| m.project.as_deref()),
            Some("pi-project")
        );
    }
}

#[tokio::test]
async fn pi_existing_protocol_entries_preserve_payload_and_attribution() {
    for (protocol, path, body, response) in [
        (
            EndpointProtocolMode::Anthropic,
            "/v1/messages",
            r#"{ "model":"pi-model","max_tokens":16,"messages":[{"role":"user","content":"hello"}] }"#,
            r#"{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#,
        ),
        (
            EndpointProtocolMode::OpenAI,
            "/v1/chat/completions",
            r#"{ "model":"pi-model","messages":[{"role":"user","content":"hello"}] }"#,
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":1}}"#,
        ),
        (
            EndpointProtocolMode::OpenAIResponses,
            "/v1/responses",
            r#"{ "model":"pi-model","input":"hello","originator":"Codex Desktop" }"#,
            r#"{"object":"response","status":"completed","output":[],"usage":{"input_tokens":2,"output_tokens":1}}"#,
        ),
        (
            EndpointProtocolMode::Gemini,
            "/v1beta/models/pi-model:generateContent",
            r#"{ "contents":[{"role":"user","parts":[{"text":"hello"}]}] }"#,
            r#"{"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":2,"candidatesTokenCount":1}}"#,
        ),
    ] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 200,
                headers: vec![("content-type".into(), "application/json".into())],
                chunks: vec![response.as_bytes().to_vec()],
            },
        );
        let engine = engine_with(pi_config(protocol), fake.clone());
        let result = engine
            .handle_request(loopback(), "POST", path, headers(), Body::from(body))
            .await;
        assert_eq!(result.status(), 200, "{path}");
        let received = axum::body::to_bytes(result.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(received.as_ref(), response.as_bytes());
        let calls = fake.requests();
        assert_eq!(calls.len(), 1);
        if protocol == EndpointProtocolMode::Anthropic {
            // The existing Messages pipeline serializes JSON after inspection.
            assert_eq!(
                serde_json::from_slice::<Value>(&calls[0].body).unwrap(),
                serde_json::from_str::<Value>(body).unwrap()
            );
        } else {
            assert_eq!(calls[0].body, body.as_bytes());
        }
        assert!(
            !calls[0]
                .headers
                .iter()
                .any(|(name, _)| name.to_ascii_lowercase().starts_with("x-sumpter-"))
        );
        assert_pi_events(&engine);
    }
}

#[tokio::test]
async fn pi_rejected_retry_stream_and_cancel_keep_attribution() {
    let fake = FakeTransport::new();
    let engine = engine_with(
        pi_config(EndpointProtocolMode::OpenAIResponses),
        fake.clone(),
    );
    let body = r#"{"model":"pi-model","input":"hello","stream":true}"#;
    let mut unauthorized = headers();
    unauthorized.retain(|(name, _)| name != "authorization");
    assert_eq!(
        engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/responses",
                unauthorized,
                Body::from(body)
            )
            .await
            .status(),
        401
    );
    assert!(fake.requests().is_empty());
    assert_pi_events(&engine);

    let stream = "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":2,\"output_tokens\":1}}}\n\n";
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![b"busy".to_vec()],
        },
    );
    fake.push(
        "b.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            chunks: stream.as_bytes().chunks(13).map(<[u8]>::to_vec).collect(),
        },
    );
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            headers(),
            Body::from(body),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        stream.as_bytes()
    );
    assert_eq!(fake.requests().len(), 2);
    assert_pi_events(&engine);

    let fake = FakeTransport::new();
    fake.push("a.example.com", Outcome::Gated { status: 200 });
    let engine = engine_with(pi_config(EndpointProtocolMode::OpenAIResponses), fake);
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            headers(),
            Body::from(body),
        )
        .await;
    drop(response);
    for _ in 0..100 {
        if engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.kind == "client" && e.outcome == Some(RuntimeEventOutcome::Cancelled))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.kind == "client" && e.outcome == Some(RuntimeEventOutcome::Cancelled))
    );
    assert_pi_events(&engine);
}
