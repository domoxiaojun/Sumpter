use serde_json::{Value, json};
use sumpter_core::bridge::{
    BridgeTerminal, OpenAiStreamBridge, ResponsesStreamBridge, SseBridge,
    try_make_openai_chat_body, try_make_responses_body,
};
use sumpter_core::bridge_gemini::{GeminiClientBridge, GeminiStreamBridge, try_make_gemini_body};
use sumpter_core::bridge_in::{
    ChatClientBridge, ResponsesClientBridge, try_chat_to_anthropic, try_responses_to_anthropic,
};
use sumpter_core::routing::RoutingRequest;

fn request(discovered: Option<bool>) -> RoutingRequest {
    let mut value = json!({
        "model": "synthetic", "max_tokens": 100, "stream": true,
        "messages": [{"role": "user", "content": "inspect the peer"}],
        "tools": [
            {"name": "ToolSearch", "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}},
            {"name": "Monitor", "defer_loading": true, "strict": false,
             "input_schema": {"type": "object", "properties": {"command": {"type": "string"}, "ws": {"type": "object"}}}}
        ]
    });
    if let Some(success) = discovered {
        let messages = value["messages"].as_array_mut().unwrap();
        messages.push(json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "search_1", "name": "ToolSearch", "input": {"query": "Monitor"}}
        ]}));
        messages.push(json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "search_1", "is_error": !success, "content": [
                {"type": "text", "text": "before"},
                {"type": "tool_reference", "tool_name": "Monitor"},
                {"type": "text", "text": "after"}
            ]}
        ]}));
    }
    RoutingRequest::from_value(&value).unwrap()
}

#[test]
fn declared_tools_survive_missing_or_failed_discovery_history_across_targets() {
    for discovered in [None, Some(false), Some(true)] {
        let req = request(discovered);
        let original = req.raw.clone();
        let chat = try_make_openai_chat_body(&req, "synthetic", None, false).unwrap();
        let responses = try_make_responses_body(&req, "synthetic", None, false).unwrap();
        let gemini = try_make_gemini_body(&req, None, None).unwrap();
        let expected = 2;
        assert_eq!(chat["tools"].as_array().unwrap().len(), expected);
        assert_eq!(responses["tools"].as_array().unwrap().len(), expected);
        assert_eq!(
            gemini["tools"][0]["functionDeclarations"]
                .as_array()
                .unwrap()
                .len(),
            expected
        );
        if discovered.is_some() {
            for body in [&chat, &responses, &gemini] {
                let encoded = body.to_string();
                assert!(encoded.contains("Tool reference: Monitor"));
                assert!(encoded.find("before").unwrap() < encoded.find("Tool reference:").unwrap());
                assert!(encoded.find("Tool reference:").unwrap() < encoded.find("after").unwrap());
            }
        }
        assert_eq!(req.raw, original, "native input must remain unchanged");
    }
}

#[test]
fn unknown_references_and_undeclared_forced_tools_are_rejected() {
    let mut value = Value::Object(request(Some(true)).raw);
    value["messages"][2]["content"][0]["content"][1]["tool_name"] = json!("unknown");
    let unknown = RoutingRequest::from_value(&value).unwrap();
    assert!(try_make_openai_chat_body(&unknown, "m", None, false).is_err());
    assert!(try_make_responses_body(&unknown, "m", None, false).is_err());
    assert!(try_make_gemini_body(&unknown, None, None).is_err());

    let mut value = Value::Object(request(None).raw);
    value["tool_choice"] = json!({"type": "tool", "name": "Missing"});
    let unloaded = RoutingRequest::from_value(&value).unwrap();
    assert!(try_make_responses_body(&unloaded, "m", None, false).is_err());
    assert!(try_make_gemini_body(&unloaded, None, None).is_err());
}

#[test]
fn strict_and_parallel_flags_survive_both_openai_directions() {
    for strict in [None, Some(false), Some(true)] {
        for disabled in [false, true] {
            let mut value = Value::Object(request(Some(true)).raw);
            if let Some(strict) = strict {
                value["tools"][0]["strict"] = json!(strict);
            }
            value["tool_choice"] = json!({"type": "auto", "disable_parallel_tool_use": disabled});
            let req = RoutingRequest::from_value(&value).unwrap();
            let chat = try_make_openai_chat_body(&req, "m", None, false).unwrap();
            let responses = try_make_responses_body(&req, "m", None, false).unwrap();
            assert_eq!(
                chat["tools"][0]["function"]["strict"],
                strict.unwrap_or(false)
            );
            assert_eq!(responses["tools"][0]["strict"], strict.unwrap_or(false));
            assert_eq!(chat["parallel_tool_calls"], !disabled);
            assert_eq!(responses["parallel_tool_calls"], !disabled);
            assert_eq!(
                chat["tools"][1]["function"]["parameters"],
                value["tools"][1]["input_schema"]
            );
            for back in [
                try_chat_to_anthropic(&chat).unwrap(),
                try_responses_to_anthropic(&responses).unwrap(),
            ] {
                assert_eq!(back["tools"][0]["strict"], strict.unwrap_or(false));
                if disabled {
                    assert_eq!(back["tool_choice"]["disable_parallel_tool_use"], true);
                }
            }
            if strict == Some(true) {
                assert!(try_make_gemini_body(&req, None, None).is_err());
            }
        }
    }
}

#[test]
fn malformed_tool_contracts_fail_before_mapping() {
    for (field, malformed) in [
        ("strict", json!("false")),
        ("defer_loading", json!(1)),
        ("input_schema", json!({"type": "array"})),
        ("input_schema", json!("object")),
        ("allowed_callers", json!(["code_execution_20260120"])),
    ] {
        let mut value = Value::Object(request(None).raw);
        value["tools"][0][field] = malformed;
        let req = RoutingRequest::from_value(&value).unwrap();
        assert!(try_make_openai_chat_body(&req, "m", None, false).is_err());
        assert!(try_make_responses_body(&req, "m", None, false).is_err());
        assert!(try_make_gemini_body(&req, None, None).is_err());
    }
    let mut value = Value::Object(request(Some(true)).raw);
    value["messages"][1]["content"][0]["input"] = json!("{}");
    let req = RoutingRequest::from_value(&value).unwrap();
    assert!(try_make_openai_chat_body(&req, "m", None, false).is_err());
    assert!(try_make_responses_body(&req, "m", None, false).is_err());
    assert!(try_make_gemini_body(&req, None, None).is_err());
}

fn added(index: u64, arguments: &str) -> Value {
    json!({"type": "response.output_item.added", "output_index": index, "item": {
        "type": "function_call", "id": format!("fc_{index}"),
        "call_id": format!("call_{index}"), "name": "SendMessage", "arguments": arguments
    }})
}

fn event_bytes(event: &Value) -> Vec<u8> {
    format!("data: {event}\n\n").into_bytes()
}

fn stream_values(bytes: &[u8]) -> Vec<Value> {
    std::str::from_utf8(bytes)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn responses_final_snapshots_complete_arguments_without_duplication() {
    let args = r#"{"to":"peer","message":"你好"}"#;
    for stream in [false, true] {
        for source in ["arguments_done", "item_done", "response_completed"] {
            for prefix in ["", r#"{"to":"peer", "#] {
                // 第二个前缀保留有效 JSON 的键间空格，最终快照也使用相同前缀。
                let args = if prefix.is_empty() {
                    args.to_string()
                } else {
                    format!("{prefix}\"message\":\"你好\"}}")
                };
                let mut events = vec![added(0, "")];
                if !prefix.is_empty() {
                    events.push(json!({"type": "response.function_call_arguments.delta", "item_id": "fc_0", "delta": prefix}));
                }
                let mut item = added(0, &args)["item"].clone();
                item["status"] = json!("completed");
                if source == "arguments_done" {
                    events.push(json!({"type": "response.function_call_arguments.done", "item_id": "fc_0", "arguments": args}));
                }
                if source != "response_completed" {
                    events.push(json!({"type": "response.output_item.done", "output_index": 0, "item": item}));
                }
                events.push(json!({"type": "response.completed", "response": {"status": "completed", "output": [item]}}));
                let mut bridge =
                    ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
                let mut output = Vec::new();
                for event in &events {
                    for byte in event_bytes(event) {
                        output.extend(bridge.feed(&[byte]));
                    }
                }
                output.extend(bridge.finish());
                assert_eq!(bridge.terminal(), BridgeTerminal::Completed, "{source}");
                let input = if stream {
                    let events = stream_values(&output);
                    let arguments: String = events
                        .iter()
                        .filter_map(|event| {
                            event.pointer("/delta/partial_json").and_then(Value::as_str)
                        })
                        .collect();
                    assert_eq!(
                        events
                            .iter()
                            .filter(|event| event.pointer("/content_block/type")
                                == Some(&json!("tool_use")))
                            .count(),
                        1
                    );
                    serde_json::from_str::<Value>(&arguments).unwrap()
                } else {
                    serde_json::from_slice::<Value>(&output).unwrap()["content"][0]["input"].clone()
                };
                assert_eq!(input, json!({"to": "peer", "message": "你好"}));
            }
        }
    }
}

#[test]
fn responses_parallel_deltas_use_output_index_and_reject_ambiguous_identity() {
    for ambiguous in [false, true] {
        let mut bridge = ResponsesStreamBridge::new("m".into(), "m".into(), false);
        let mut output = Vec::new();
        output.extend(bridge.feed(&event_bytes(&added(0, ""))));
        output.extend(bridge.feed(&event_bytes(&added(1, ""))));
        for index in [0, 1] {
            let mut delta = json!({"type": "response.function_call_arguments.delta", "delta": format!("{{\"to\":\"peer_{index}\"}}")});
            if !ambiguous {
                delta["output_index"] = json!(index);
            }
            output.extend(bridge.feed(&event_bytes(&delta)));
        }
        output.extend(bridge.feed(&event_bytes(
            &json!({"type": "response.completed", "response": {"status": "completed"}}),
        )));
        if ambiguous {
            assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
        } else {
            assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
            let events = stream_values(&output);
            for index in [0, 1] {
                let arguments: String = events
                    .iter()
                    .filter(|event| event["index"] == index)
                    .filter_map(|event| {
                        event.pointer("/delta/partial_json").and_then(Value::as_str)
                    })
                    .collect();
                assert_eq!(
                    serde_json::from_str::<Value>(&arguments).unwrap(),
                    json!({"to": format!("peer_{index}")})
                );
            }
        }
    }
}

#[test]
fn invalid_arguments_never_become_empty_successful_calls() {
    for stream in [false, true] {
        for args in ["", "null", "[]", "123", r#"{"to":"#] {
            let mut responses =
                ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
            let mut chat =
                OpenAiStreamBridge::new_with_stream("m".into(), "m".into(), false, false, stream);
            let mut response_output = responses.feed(&event_bytes(&added(0, args)));
            response_output.extend(responses.feed(&event_bytes(
                &json!({"type": "response.completed", "response": {"status": "completed"}}),
            )));
            let mut chat_output = chat.feed(&event_bytes(&json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "c", "function": {"name": "SendMessage", "arguments": args}}]}, "finish_reason": "tool_calls"}]})));
            chat_output.extend(chat.feed(b"data: [DONE]\n\n"));
            for (terminal, output) in [
                (responses.terminal(), response_output),
                (chat.terminal(), chat_output),
            ] {
                assert!(matches!(terminal, BridgeTerminal::Failed(_)), "{args}");
                if stream {
                    assert!(
                        !stream_values(&output)
                            .iter()
                            .any(|event| event["type"] == "message_stop")
                    );
                } else {
                    assert_eq!(
                        serde_json::from_slice::<Value>(&output).unwrap()["type"],
                        "error"
                    );
                }
            }
        }
        let mut gemini = GeminiStreamBridge::new("m".into(), "m".into(), stream);
        gemini.feed(&event_bytes(&json!({"candidates": [{"content": {"parts": [{"functionCall": {"name": "SendMessage", "args": []}}]}, "finishReason": "STOP"}]})));
        gemini.finish();
        assert!(matches!(gemini.terminal(), BridgeTerminal::Failed(_)));
    }
}

#[test]
fn conflicting_final_arguments_and_later_frames_cannot_reverse_failure() {
    let mut bridge = ResponsesStreamBridge::new("m".into(), "m".into(), false);
    let mut input = event_bytes(&added(0, r#"{"to":"first"}"#));
    input.extend(event_bytes(&json!({"type": "response.function_call_arguments.done", "item_id": "fc_0", "arguments": r#"{"to":"second"}"#})));
    input.extend(event_bytes(&added(1, "{}")));
    input.extend(event_bytes(
        &json!({"type": "response.completed", "response": {"status": "completed"}}),
    ));
    let output = bridge.feed(&input);
    assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
    let events = stream_values(&output);
    assert!(!events.iter().any(|event| event["type"] == "message_stop"));
    assert!(
        !events
            .iter()
            .any(|event| event.pointer("/content_block/id") == Some(&json!("call_1")))
    );
}

#[test]
fn responses_completed_only_and_equivalent_final_snapshots_are_supported() {
    for stream in [false, true] {
        for initial in [false, true] {
            let mut bridge =
                ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
            let mut output = Vec::new();
            if initial {
                output.extend(bridge.feed(&event_bytes(&added(0, r#"{ "to" : "peer" }"#))));
            }
            output.extend(bridge.feed(&event_bytes(
                &json!({"type": "response.completed", "response": {
                    "status": "completed", "output": [added(0, r#"{"to":"peer"}"#)["item"].clone()]
                }}),
            )));
            assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
            if stream {
                let events = stream_values(&output);
                assert_eq!(events[0]["type"], "message_start");
                let args: String = events
                    .iter()
                    .filter_map(|event| {
                        event.pointer("/delta/partial_json").and_then(Value::as_str)
                    })
                    .collect();
                assert_eq!(
                    serde_json::from_str::<Value>(&args).unwrap(),
                    json!({"to": "peer"})
                );
            } else {
                assert_eq!(
                    serde_json::from_slice::<Value>(&output).unwrap()["content"][0]["input"],
                    json!({"to": "peer"})
                );
            }
        }
    }
}

#[test]
fn responses_rejects_conflicting_identity_fields() {
    for item_done in [false, true] {
        let mut bridge = ResponsesStreamBridge::new("m".into(), "m".into(), false);
        bridge.feed(&event_bytes(&added(0, "")));
        bridge.feed(&event_bytes(&added(1, "")));
        let bad = if item_done {
            json!({"type": "response.output_item.done", "output_index": 1, "item": added(0, "{}")["item"]})
        } else {
            json!({"type": "response.function_call_arguments.delta", "item_id": "fc_0", "output_index": 1, "delta": "{}"})
        };
        bridge.feed(&event_bytes(&bad));
        assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
    }
}

#[test]
fn chat_keeps_parallel_arguments_separate_and_rejects_invalid_indexes() {
    let mut bridge =
        OpenAiStreamBridge::new_with_stream("m".into(), "m".into(), false, false, false);
    for index in [1, 0] {
        bridge.feed(&event_bytes(&json!({"choices": [{"delta": {"tool_calls": [{
            "index": index, "id": format!("c{index}"), "function": {"name": "SendMessage", "arguments": format!("{{\"to\":\"peer_{index}\"}}")}
        }]}}]})));
    }
    let output = bridge.feed(b"data: [DONE]\n\n");
    let body: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
    assert_eq!(body["content"][0]["input"]["to"], "peer_0");
    assert_eq!(body["content"][1]["input"]["to"], "peer_1");

    for index in [json!(-1), json!(4096), json!("0"), Value::Null] {
        let mut bridge = OpenAiStreamBridge::new("m".into(), "m".into(), false, false);
        let mut input = event_bytes(
            &json!({"choices": [{"delta": {"tool_calls": [{"index": index, "function": {"name": "SendMessage", "arguments": "{}"}}]}}]}),
        );
        input.extend_from_slice(b"data: [DONE]\n\n");
        bridge.feed(&input);
        assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
    }
    let mut bridge = OpenAiStreamBridge::new("m".into(), "m".into(), false, false);
    bridge.feed(&event_bytes(
        &json!({"choices": [{"delta": {"tool_calls": [{}]}}]}),
    ));
    bridge.feed(b"data: [DONE]\n\n");
    assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
}

fn client_bridges(stream: bool) -> Vec<Box<dyn SseBridge>> {
    vec![
        Box::new(ChatClientBridge::new("m".into(), "m".into(), stream)),
        Box::new(ResponsesClientBridge::new("m".into(), "m".into(), stream)),
        Box::new(GeminiClientBridge::new("m".into(), stream)),
    ]
}

fn tool_start(input: Value) -> Value {
    json!({"type": "content_block_start", "index": 0, "content_block": {
        "type": "tool_use", "id": "call_0", "name": "SendMessage", "input": input
    }})
}

#[test]
fn reverse_bridges_preserve_initial_input_and_empty_calls() {
    for stream in [false, true] {
        for args in [json!({}), json!({"to": "peer", "message": "你好"})] {
            let mut input = event_bytes(&tool_start(args.clone()));
            input.extend(event_bytes(
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            input.extend(event_bytes(
                &json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
            ));
            input.extend(event_bytes(&json!({"type": "message_stop"})));
            for (kind, mut bridge) in client_bridges(stream).into_iter().enumerate() {
                let mut output = Vec::new();
                for byte in &input {
                    output.extend(bridge.feed(&[*byte]));
                }
                output.extend(bridge.finish());
                assert_eq!(bridge.terminal(), BridgeTerminal::Completed, "{kind}");
                let actual = if stream {
                    let text = std::str::from_utf8(&output)
                        .unwrap()
                        .replace("data: [DONE]\n\n", "");
                    let events = stream_values(text.as_bytes());
                    match kind {
                        0 => {
                            let args: String = events.iter().filter_map(|event| event.pointer("/choices/0/delta/tool_calls/0/function/arguments").and_then(Value::as_str)).collect();
                            serde_json::from_str(&args).unwrap()
                        }
                        1 => serde_json::from_str(events.iter().find(|event| event["type"] == "response.function_call_arguments.done").unwrap()["arguments"].as_str().unwrap()).unwrap(),
                        _ => events.last().unwrap()["candidates"][0]["content"]["parts"][0]["functionCall"]["args"].clone(),
                    }
                } else {
                    let value: Value = serde_json::from_slice(&output).unwrap();
                    match kind {
                        0 => serde_json::from_str(value["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"].as_str().unwrap()).unwrap(),
                        1 => serde_json::from_str(value["output"][0]["arguments"].as_str().unwrap()).unwrap(),
                        _ => value["candidates"][0]["content"]["parts"][0]["functionCall"]["args"].clone(),
                    }
                };
                assert_eq!(actual, args, "{kind}");
            }
        }
    }
}

#[test]
fn reverse_bridges_reject_invalid_parameters_and_keep_failure_terminal() {
    for stream in [false, true] {
        for bad in [
            tool_start(json!([])),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"to\":"}}),
            json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": []}}),
        ] {
            let mut input = if bad["type"] == "content_block_start" {
                Vec::new()
            } else {
                event_bytes(&tool_start(json!({})))
            };
            input.extend(event_bytes(&bad));
            input.extend(event_bytes(
                &json!({"type": "content_block_stop", "index": 0}),
            ));
            input.extend(event_bytes(&json!({"type": "message_stop"})));
            for mut bridge in client_bridges(stream) {
                let output = bridge.feed(&input);
                assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
                assert!(String::from_utf8_lossy(&output).contains("error"));
            }
        }
        for mut bridge in client_bridges(stream) {
            let output = bridge.feed(b"data: {broken\n\ndata: {\"type\":\"message_stop\"}\n\n");
            assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
            assert!(String::from_utf8_lossy(&output).contains("error"));
        }
    }
}
