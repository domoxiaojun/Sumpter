//! Failure boundaries exercised through both platform adapters.
use super::*;

fn config(protocol: EndpointProtocolMode) -> AppConfig {
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    config.endpoints[0].protocol = protocol;
    config.endpoints[0].mappings[0].client_pattern = "synthetic".into();
    config.endpoints[0].mappings[0].upstream_model = "synthetic".into();
    config.retry.max_deferred_rounds = 2;
    config.retry.max_stream_error_retries = 2;
    config.retry.max_500_retries = 2;
    config.normalized()
}

#[tokio::test]
async fn provider_side_tools_do_not_replay_http_or_truncated_stream_failures() {
    for kind in [
        "mcp",
        "computer_use_preview",
        "shell",
        "future_provider_tool",
    ] {
        for status in [200, 500, 503] {
            let fake = FakeTransport::new();
            fake.push("a.example.com", Outcome::Status {
                status,
                headers: vec![("content-type".into(), "text/event-stream".into())],
                chunks: vec![b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"mcp_call\",\"name\":\"synthetic_write\",\"output\":\"executed\"}}\n\n".to_vec()],
            });
            fake.push("a.example.com", sse_ok(&["data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n"]));
            let engine = engine_with(config(EndpointProtocolMode::OpenAIResponses), fake.clone());
            let request = json!({"model":"synthetic","stream":true,"input":"perform operation","tools":[{"type":kind,"server_url":"https://synthetic.invalid"}]}).to_string();
            let response = engine
                .handle_request(
                    loopback(),
                    "POST",
                    "/v1/responses",
                    vec![],
                    Body::from(request),
                )
                .await;
            let _ = axum::body::to_bytes(response.into_body(), usize::MAX).await;
            assert_eq!(fake.requests().len(), 1, "{kind} {status}");
        }
    }
}

#[tokio::test]
async fn a_truncated_gemini_turn_preserves_the_last_successful_signature() {
    fn tool(signature: &str, finished: bool) -> String {
        let mut candidate = json!({"content":{"role":"model","parts":[{"functionCall":{"name":"read_file","args":{"path":"a"}},"thoughtSignature":signature}]}});
        if finished {
            candidate["finishReason"] = json!("STOP");
        }
        format!("data: {}\n\n", json!({"candidates":[candidate]}))
    }
    let fake = FakeTransport::new();
    fake.push("a.example.com", sse_ok(&[&tool("valid-signature", true)]));
    fake.push(
        "a.example.com",
        sse_ok(&[&tool("incomplete-signature", false)]),
    );
    fake.push("a.example.com", sse_ok(&["data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"done\"}]},\"finishReason\":\"STOP\"}]}\n\n"]));
    let engine = engine_with(config(EndpointProtocolMode::Gemini), fake.clone());
    let headers = vec![("session_id".into(), "synthetic-session".into())];
    let first = r#"{"model":"synthetic","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"read a"}],"tools":[{"name":"read_file","input_schema":{"type":"object"}}]}"#;
    for _ in 0..2 {
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                headers.clone(),
                Bytes::from_static(first.as_bytes())
            )
            .await
            .0,
            200
        );
    }
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|e| e.kind == "client" && e.is_failed())
    );
    let next = r#"{"model":"synthetic","max_tokens":64,"stream":true,"messages":[{"role":"user","content":"read a"},{"role":"assistant","content":[{"type":"tool_use","id":"gemini_call_1","name":"read_file","input":{"path":"a"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"gemini_call_1","content":"contents"}]}],"tools":[{"name":"read_file","input_schema":{"type":"object"}}]}"#;
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            headers,
            Bytes::from_static(next.as_bytes())
        )
        .await
        .0,
        200
    );
    let calls = fake.requests();
    assert_eq!(calls.len(), 3);
    let sent: Value = serde_json::from_slice(&calls[2].body).unwrap();
    assert_eq!(
        sent["contents"][1]["parts"][0]["thoughtSignature"],
        "valid-signature"
    );
}
