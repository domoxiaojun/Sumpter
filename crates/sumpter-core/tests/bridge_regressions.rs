//! 全仓审查发现的桥接语义回归：严格检查不能放行随后丢弃的内容。

use serde_json::{Value, json};
use sumpter_core::bridge::{
    BridgeTerminal, SseBridge, check_anthropic_translation, try_make_openai_chat_body,
    try_make_responses_body,
};
use sumpter_core::bridge_gemini::{
    GeminiStreamBridge, check_anthropic_to_gemini, try_make_gemini_body,
};
use sumpter_core::bridge_in::{
    chat_to_anthropic, check_chat_to_anthropic, check_responses_to_anthropic,
    responses_to_anthropic, try_chat_to_anthropic, try_responses_to_anthropic,
};
use sumpter_core::config::ProviderProtocol;
use sumpter_core::routing::RoutingRequest;

fn request(value: &Value) -> RoutingRequest {
    RoutingRequest::from_value(value).unwrap()
}

fn data_jsons(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8_lossy(bytes)
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|payload| serde_json::from_str(payload).ok())
        .collect()
}

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {"answer": {"type": "string", "enum": ["yes", "no"]}},
        "required": ["answer"],
        "additionalProperties": false,
    })
}

#[test]
fn structured_output_round_trips_name_schema_strict_and_description() {
    for strict in [true, false] {
        let chat_format = json!({
            "type": "json_schema",
            "json_schema": {"name": "decision", "strict": strict, "description": "pick one", "schema": schema()},
        });
        let input = json!({
            "model": "synthetic", "messages": [{"role": "user", "content": "choose"}],
            "response_format": chat_format,
        });
        let intermediate = try_chat_to_anthropic(&input).unwrap();
        assert_eq!(intermediate["output_config"]["format"]["schema"], schema());
        let responses =
            try_make_responses_body(&request(&intermediate), "synthetic", None, false).unwrap();
        assert_eq!(
            responses["text"]["format"],
            json!({
                "type": "json_schema", "name": "decision", "strict": strict,
                "description": "pick one", "schema": schema(),
            })
        );
        let back = try_responses_to_anthropic(&responses).unwrap();
        let chat = try_make_openai_chat_body(&request(&back), "synthetic", None, false).unwrap();
        assert_eq!(chat["response_format"], chat_format);
        for target in [ProviderProtocol::Anthropic, ProviderProtocol::Gemini] {
            assert!(check_anthropic_translation(&request(&back), target, false).is_err());
        }
    }
}

#[test]
fn schema_without_openai_metadata_remains_convertible() {
    let req = request(&json!({
        "model": "synthetic", "messages": [{"role": "user", "content": "choose"}],
        "output_config": {"format": {"type": "json_schema", "schema": schema()}},
    }));
    assert!(check_anthropic_translation(&req, ProviderProtocol::Anthropic, false).is_ok());
    assert_eq!(
        try_make_gemini_body(&req, None, None).unwrap()["generationConfig"]["responseJsonSchema"],
        schema()
    );
    let chat = try_make_openai_chat_body(&req, "synthetic", None, false).unwrap();
    assert!(
        chat["response_format"]["json_schema"]
            .get("strict")
            .is_none()
    );
}

#[test]
fn unsupported_output_formats_fail_only_strict_translation_checks() {
    for format in [
        json!({"type": "json_object"}),
        json!({"type": "json_schema", "json_schema": {"name": "x", "strict": "yes", "schema": schema()}}),
    ] {
        let body = json!({"model": "synthetic", "messages": [{"role": "user", "content": "hi"}], "response_format": format});
        assert!(chat_to_anthropic(&body).is_ok());
        assert!(check_chat_to_anthropic(&body).is_err());
    }
    let body =
        json!({"model": "synthetic", "input": "hi", "text": {"format": {"type": "json_object"}}});
    assert!(responses_to_anthropic(&body).is_ok());
    assert!(check_responses_to_anthropic(&body).is_err());
}

#[test]
fn responses_context_references_are_rejected_only_for_translation() {
    for (field, value) in [
        ("previous_response_id", json!("resp_synthetic")),
        ("conversation", json!({"id": "conv_synthetic"})),
    ] {
        let mut body = json!({"model": "synthetic", "input": "continue"});
        body[field] = value;
        let original = body.clone();
        assert!(responses_to_anthropic(&body).is_ok());
        let error = check_responses_to_anthropic(&body).unwrap_err();
        assert!(error.to_string().contains(field));
        assert_eq!(body, original);
        body[field] = Value::Null;
        assert!(check_responses_to_anthropic(&body).is_ok());
    }
}

fn tool_result(content: Value) -> RoutingRequest {
    request(&json!({
        "model": "synthetic", "max_tokens": 100,
        "messages": [
            {"role": "assistant", "content": [{"type": "tool_use", "id": "call_1", "name": "screenshot", "input": {}}]},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call_1", "content": content}]},
        ],
    }))
}

#[test]
fn tool_images_survive_responses_and_are_rejected_by_text_only_targets() {
    for source in [
        json!({"type": "base64", "media_type": "image/png", "data": "AAAA"}),
        json!({"type": "url", "url": "https://example.invalid/synthetic.png"}),
    ] {
        let req = tool_result(json!([
            {"type": "text", "text": "before"},
            {"type": "image", "source": source},
            {"type": "text", "text": "after"},
        ]));
        let responses = try_make_responses_body(&req, "synthetic", None, false).unwrap();
        let output = &responses["input"][1]["output"];
        assert_eq!(output[0], json!({"type": "input_text", "text": "before"}));
        assert_eq!(output[1]["type"], "input_image");
        assert!(
            output[1]["image_url"].as_str().unwrap().contains("AAAA")
                || output[1]["image_url"]
                    .as_str()
                    .unwrap()
                    .contains("synthetic.png")
        );
        assert_eq!(output[2], json!({"type": "input_text", "text": "after"}));
        assert!(try_make_openai_chat_body(&req, "synthetic", None, false).is_err());
        assert!(try_make_gemini_body(&req, None, None).is_err());
        let back = try_responses_to_anthropic(&responses).unwrap();
        assert_eq!(
            back["messages"][1]["content"][0]["content"][1]["type"],
            "image"
        );
    }
}

#[test]
fn nested_tool_content_and_malformed_images_never_disappear_silently() {
    for content in [
        json!([{"type": "document", "source": {"type": "text", "data": "important"}}]),
        json!([{"type": "tool_result", "content": [{"type": "image", "source": {"type": "url", "url": "https://example.invalid/image"}}]}]),
        json!([{"type": "image", "source": {"type": "base64", "media_type": "image/png"}}]),
        json!([{"type": "text", "text": "quoted", "citations": [{"source": "important"}]}]),
    ] {
        let req = tool_result(content);
        assert!(try_make_openai_chat_body(&req, "synthetic", None, false).is_err());
        assert!(try_make_responses_body(&req, "synthetic", None, false).is_err());
        assert!(try_make_gemini_body(&req, None, None).is_err());
    }
}

#[test]
fn repeated_gemini_deltas_survive_every_byte_boundary() {
    let input = ["哈", "哈", "哈哈!"]
        .into_iter()
        .map(|text| {
            format!(
                "data: {}\n\n",
                json!({"candidates": [{"content": {"parts": [{"text": text}]}}]})
            )
        })
        .collect::<String>()
        + "data: {\"candidates\":[{\"finishReason\":\"STOP\"}]}\n\n";
    for stream in [true, false] {
        let mut bridge =
            GeminiStreamBridge::new("msg_synthetic".into(), "synthetic".into(), stream);
        let mut bytes = Vec::new();
        for byte in input.as_bytes() {
            bytes.extend(bridge.feed(&[*byte]));
        }
        bytes.extend(bridge.finish());
        assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
        if stream {
            let text: String = data_jsons(&bytes)
                .iter()
                .filter_map(|value| value.pointer("/delta/text").and_then(Value::as_str))
                .collect();
            assert_eq!(text, "哈哈哈哈!");
        } else {
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["content"][0]["text"], "哈哈哈哈!");
        }
    }
}

#[test]
fn gemini_tool_stop_reason_matches_in_stream_and_json() {
    for (finish_reason, expected) in [
        ("STOP", "tool_use"),
        ("MAX_TOKENS", "max_tokens"),
        ("SAFETY", "refusal"),
    ] {
        let input = format!(
            "data: {}\n\n",
            json!({"candidates": [{"content": {"parts": [{"functionCall": {"name": "read_file", "args": {"path": "synthetic"}}}]}, "finishReason": finish_reason}]})
        );
        for stream in [true, false] {
            let mut bridge =
                GeminiStreamBridge::new("msg_synthetic".into(), "synthetic".into(), stream);
            let mut bytes = bridge.feed(input.as_bytes());
            bytes.extend(bridge.finish());
            if stream {
                let events = data_jsons(&bytes);
                assert_eq!(
                    events
                        .iter()
                        .find_map(|event| event.pointer("/delta/stop_reason"))
                        .unwrap(),
                    expected
                );
            } else {
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(body["content"][0]["type"], "tool_use");
                assert_eq!(body["stop_reason"], expected);
            }
        }
    }
}

#[test]
fn gemini_tool_choice_is_mapped_and_invalid_combinations_rejected() {
    for (choice, expected) in [
        (json!({"type": "none"}), json!({"mode": "NONE"})),
        (json!({"type": "auto"}), json!({"mode": "AUTO"})),
        (json!({"type": "any"}), json!({"mode": "ANY"})),
        (
            json!({"type": "tool", "name": "read_file"}),
            json!({"mode": "ANY", "allowedFunctionNames": ["read_file"]}),
        ),
    ] {
        let req = request(
            &json!({"model": "synthetic", "messages": [{"role": "user", "content": "hi"}], "tools": [{"name": "read_file", "input_schema": {"type": "object"}}], "tool_choice": choice}),
        );
        assert!(check_anthropic_to_gemini(&req).is_ok());
        assert_eq!(
            try_make_gemini_body(&req, None, None).unwrap()["toolConfig"]["functionCallingConfig"],
            expected
        );
    }
    for choice in [
        json!({"type": "auto", "disable_parallel_tool_use": true}),
        json!({"type": "tool", "name": "not_declared"}),
        json!({"type": "auto", "name": "read_file"}),
        json!({"type": "unknown"}),
    ] {
        let req = request(
            &json!({"model": "synthetic", "messages": [{"role": "user", "content": "hi"}], "tools": [{"name": "read_file"}], "tool_choice": choice}),
        );
        assert!(check_anthropic_to_gemini(&req).is_err());
    }
}
