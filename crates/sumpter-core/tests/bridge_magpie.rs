use serde_json::{Value, json};
use sumpter_core::bridge::{
    BridgeTerminal, ResponsesStreamBridge, SseBridge, try_make_openai_chat_body,
    try_make_responses_body,
};
use sumpter_core::bridge_gemini::try_make_gemini_body;
use sumpter_core::routing::RoutingRequest;

fn compacted_request() -> Value {
    json!({
        "model": "synthetic", "max_tokens": 100,
        "messages": [{"role": "user", "content": "Compacted history: continue monitoring the peer."}],
        "tools": [
            {"name": "ToolSearch", "input_schema": {"type": "object"}},
            {"name": "Monitor", "defer_loading": true, "strict": false,
             "input_schema": {"type": "object", "properties": {"command": {"type": "string"}}}},
            {"name": "DeferredToolPlaceholder", "defer_loading": true,
             "input_schema": {"type": "object"}}
        ]
    })
}

#[test]
fn magpie_regression_compacted_history_keeps_declared_tools() {
    for forced in [false, true] {
        let mut value = compacted_request();
        if forced {
            value["tool_choice"] = json!({"type": "tool", "name": "Monitor"});
        }
        let request = RoutingRequest::from_value(&value).unwrap();
        let chat = try_make_openai_chat_body(&request, "m", None, false).unwrap();
        let responses = try_make_responses_body(&request, "m", None, false).unwrap();
        let gemini = try_make_gemini_body(&request, None, None).unwrap();
        for (tools, nested) in [
            (&chat["tools"], true),
            (&responses["tools"], false),
            (&gemini["tools"][0]["functionDeclarations"], false),
        ] {
            let names: Vec<&str> = tools
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| {
                    let tool = if nested { &tool["function"] } else { tool };
                    tool["name"].as_str().unwrap()
                })
                .collect();
            assert_eq!(names, ["ToolSearch", "Monitor"]);
        }
        assert_eq!(request.raw, value.as_object().unwrap().clone());
    }
}

#[test]
fn placeholder_and_removed_tools_cannot_be_forced_or_inferred_from_summary() {
    for name in ["DeferredToolPlaceholder", "Monitor"] {
        let mut value = compacted_request();
        value["tools"].as_array_mut().unwrap().remove(1);
        let request = RoutingRequest::from_value(&value).unwrap();
        let responses = try_make_responses_body(&request, "m", None, false).unwrap();
        assert_eq!(responses["tools"].as_array().unwrap().len(), 1);
        value["tool_choice"] = json!({"type": "tool", "name": name});
        let request = RoutingRequest::from_value(&value).unwrap();
        assert!(try_make_openai_chat_body(&request, "m", None, false).is_err());
        assert!(try_make_responses_body(&request, "m", None, false).is_err());
        assert!(try_make_gemini_body(&request, None, None).is_err());
    }
}

fn item(index: usize, arguments: &str) -> Value {
    json!({"type": "function_call", "id": format!("fc_{index}"),
        "call_id": format!("call_{index}"), "name": "Monitor", "arguments": arguments})
}

fn added(index: usize) -> Value {
    json!({"type": "response.output_item.added", "output_index": index, "item": item(index, "")})
}

fn delta(index: usize, fragment: &str) -> Value {
    json!({"type": "response.function_call_arguments.delta", "item_id": format!("fc_{index}"), "delta": fragment})
}

fn done(index: usize, arguments: &str) -> Value {
    json!({"type": "response.function_call_arguments.done", "item_id": format!("fc_{index}"), "arguments": arguments})
}

fn completed() -> Value {
    json!({"type": "response.completed", "response": {"status": "completed"}})
}

fn feed(bridge: &mut ResponsesStreamBridge, event: &Value) -> Vec<u8> {
    // 包括 UTF-8 字符中间的网络分片。
    format!("data: {event}\n\n")
        .bytes()
        .flat_map(|byte| bridge.feed(&[byte]))
        .collect()
}

fn stream_values(output: &[u8]) -> Vec<Value> {
    std::str::from_utf8(output)
        .unwrap()
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn streamed_arguments(output: &[u8], index: usize) -> String {
    stream_values(output)
        .iter()
        .filter(|event| event["index"] == index)
        .filter_map(|event| event.pointer("/delta/partial_json").and_then(Value::as_str))
        .collect()
}

fn assert_input(output: &[u8], stream: bool, index: usize, expected: &Value) {
    if stream {
        assert_eq!(
            serde_json::from_str::<Value>(&streamed_arguments(output, index)).unwrap(),
            *expected
        );
        assert!(
            stream_values(output)
                .iter()
                .any(|event| event["type"] == "message_stop")
        );
    } else {
        let body: Value = serde_json::from_slice(output).unwrap();
        assert_eq!(body["content"][index]["input"], *expected);
        assert_eq!(body["stop_reason"], "tool_use");
    }
}

#[test]
fn magpie_regression_closed_arguments_accept_richer_final_snapshots() {
    let full = r#"{"to":"peer","command":"inspect"}"#;
    for stream in [false, true] {
        for fragments in [vec!["{}"], vec![r#"{"to":"peer""#, "}"]] {
            for source in ["arguments_done", "item_done", "completed_output"] {
                let mut bridge =
                    ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
                let mut output = feed(&mut bridge, &added(0));
                for fragment in &fragments {
                    output.extend(feed(&mut bridge, &delta(0, fragment)));
                }
                if stream {
                    let partial = streamed_arguments(&output, 0);
                    assert!(serde_json::from_str::<Value>(&partial).is_err());
                    if fragments.len() > 1 {
                        assert_eq!(partial, fragments[0], "稳定前缀仍应实时发送");
                    }
                }
                let initial = fragments.concat();
                output.extend(feed(
                    &mut bridge,
                    &done(
                        0,
                        if source == "arguments_done" {
                            full
                        } else {
                            &initial
                        },
                    ),
                ));
                let mut terminal = completed();
                if source == "item_done" {
                    output.extend(feed(&mut bridge, &json!({
                        "type": "response.output_item.done", "output_index": 0, "item": item(0, full)
                    })));
                } else if source == "completed_output" {
                    terminal["response"]["output"] = json!([item(0, full)]);
                }
                output.extend(feed(&mut bridge, &terminal));
                output.extend(bridge.finish());
                assert_eq!(bridge.terminal(), BridgeTerminal::Completed, "{source}");
                assert_input(&output, stream, 0, &serde_json::from_str(full).unwrap());
            }
        }
    }
}

#[test]
fn parallel_calls_and_json_escapes_keep_independent_closing_tails() {
    let arguments = [
        json!({"command": "引号\"反斜线\\和括号}[]", "nested": [{"ok": true}, [1, 2]]}),
        json!({"command": "peer two", "nested": {"empty": {}}}),
    ];
    let encoded: Vec<String> = arguments.iter().map(Value::to_string).collect();
    for stream in [false, true] {
        let mut bridge =
            ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
        let mut output = feed(&mut bridge, &added(0));
        output.extend(feed(&mut bridge, &added(1)));
        let fragments: Vec<Vec<char>> = encoded.iter().map(|text| text.chars().collect()).collect();
        for position in 0..fragments.iter().map(Vec::len).max().unwrap() {
            for (index, chars) in fragments.iter().enumerate() {
                if let Some(character) = chars.get(position) {
                    output.extend(feed(&mut bridge, &delta(index, &character.to_string())));
                }
            }
        }
        // 无工具结束快照时，由合法 response.completed 提交闭合尾部。
        output.extend(feed(&mut bridge, &delta(0, " \n")));
        if stream {
            for index in 0..2 {
                assert!(
                    serde_json::from_str::<Value>(&streamed_arguments(&output, index)).is_err()
                );
            }
        }
        output.extend(feed(&mut bridge, &completed()));
        assert_eq!(bridge.terminal(), BridgeTerminal::Completed);
        for (index, expected) in arguments.iter().enumerate() {
            assert_input(&output, stream, index, expected);
        }
    }
}

#[test]
fn final_snapshots_cannot_rewrite_fields_or_committed_prefixes() {
    for stream in [false, true] {
        for (initial, final_args, commit) in [
            (r#"{"to":"peer"}"#, r#"{"to":"other"}"#, false),
            (r#"{"to":"peer""#, r#"{"to":"other"}"#, false),
            (r#"{"to":"peer"}"#, r#"{"to":"peer","command":"new"}"#, true),
        ] {
            let mut bridge =
                ResponsesStreamBridge::new_with_stream("m".into(), "m".into(), false, stream);
            let mut output = feed(&mut bridge, &added(0));
            output.extend(feed(&mut bridge, &delta(0, initial)));
            if commit {
                output.extend(feed(&mut bridge, &json!({
                    "type": "response.output_item.done", "output_index": 0, "item": item(0, initial)
                })));
            }
            output.extend(feed(&mut bridge, &done(0, final_args)));
            output.extend(feed(&mut bridge, &completed()));
            assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
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
}

#[test]
fn closed_tail_requires_response_completion_and_rejects_extra_json() {
    for extra in [false, true] {
        let mut bridge = ResponsesStreamBridge::new("m".into(), "m".into(), false);
        let mut output = feed(&mut bridge, &added(0));
        output.extend(feed(&mut bridge, &delta(0, "{}")));
        if extra {
            output.extend(feed(&mut bridge, &delta(0, "{}")));
            output.extend(feed(&mut bridge, &completed()));
        }
        output.extend(bridge.finish());
        assert!(matches!(bridge.terminal(), BridgeTerminal::Failed(_)));
        assert_eq!(streamed_arguments(&output, 0), "");
        assert!(
            stream_values(&output)
                .iter()
                .all(|event| event["type"] != "message_stop")
        );
    }
}
