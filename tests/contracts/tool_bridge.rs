//! 两个 adapter 共用的工具发现与参数保真合同。
use super::*;

fn discovery_body(discovered: bool) -> Value {
    let mut value = json!({
        "model": "claude-opus-5", "stream": false, "max_tokens": 100,
        "messages": [{"role": "user", "content": "inspect a synthetic peer"}],
        "tools": [
            {"name": "ToolSearch", "input_schema": {"type": "object", "properties": {"query": {"type": "string"}}}},
            {"name": "SendMessage", "strict": false, "defer_loading": true,
             "input_schema": {"type": "object", "properties": {"to": {"type": "string"}, "message": {"type": "string"}}, "required": ["to"]}}
        ],
        "tool_choice": {"type": "auto", "disable_parallel_tool_use": true}
    });
    if !discovered {
        value["tools"].as_array_mut().unwrap().remove(1);
    }
    value["tools"].as_array_mut().unwrap().push(json!({
        "name": "DeferredToolPlaceholder", "defer_loading": true,
        "input_schema": {"type": "object", "properties": {}}
    }));
    if discovered {
        let messages = value["messages"].as_array_mut().unwrap();
        messages.push(json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "search_1", "name": "ToolSearch", "input": {"query": "SendMessage"}}
        ]}));
        messages.push(json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "search_1", "content": [{"type": "tool_reference", "tool_name": "SendMessage"}]}
        ]}));
    }
    value
}

#[tokio::test]
async fn tool_discovery_and_final_arguments_survive_responses_bridge() {
    for discovered in [false, true] {
        let fake = FakeTransport::new();
        let frames = if discovered {
            vec![
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_0\",\"call_id\":\"call_0\",\"name\":\"SendMessage\",\"arguments\":\"\"}}\n\n",
                "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_0\",\"arguments\":\"{\\\"to\\\":\\\"synthetic_peer\\\"}\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ]
        } else {
            vec![
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            ]
        };
        fake.push("a.example.com", sse_ok(&frames));
        let mut config = two_endpoint_config();
        config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
        config.endpoints[1].enabled = false;
        let engine = engine_with(config.normalized(), fake.clone());
        let value = discovery_body(discovered);
        let (status, output) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![("content-type".into(), "application/json".into())],
            Bytes::from(serde_json::to_vec(&value).unwrap()),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&output));
        let requests = fake.requests();
        assert_eq!(requests.len(), 1);
        let outbound: Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert_eq!(outbound["parallel_tool_calls"], false);
        assert_eq!(outbound["tools"][0]["strict"], false);
        assert_eq!(
            outbound["tools"].as_array().unwrap().len(),
            if discovered { 2 } else { 1 }
        );
        if discovered {
            assert_eq!(outbound["tools"][1]["name"], "SendMessage");
            assert_eq!(
                outbound["tools"][1]["parameters"],
                value["tools"][1]["input_schema"]
            );
            let response: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(
                response["content"][0]["input"],
                json!({"to": "synthetic_peer"})
            );
            assert_eq!(response["stop_reason"], "tool_use");
        }
    }
}

#[tokio::test]
async fn native_anthropic_keeps_deferred_tools_and_references_intact() {
    let fake = FakeTransport::new();
    let frames = anthropic_tool_sse();
    let borrowed: Vec<&str> = frames.iter().map(String::as_str).collect();
    fake.push("a.example.com", sse_ok(&borrowed));
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Anthropic;
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    let mut value = discovery_body(true);
    value["stream"] = json!(true);
    value["tools"][1]["strict"] = json!(true);
    let (status, output) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(serde_json::to_vec(&value).unwrap()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(output, frames.concat().as_bytes());
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let outbound: Value = serde_json::from_slice(&requests[0].body).unwrap();
    for field in ["tools", "messages", "tool_choice"] {
        assert_eq!(outbound[field], value[field]);
    }
}

#[tokio::test]
async fn compacted_history_keeps_current_tools_and_excludes_placeholder() {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        sse_ok(&[
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        ]),
    );
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    let mut value = discovery_body(true);
    value["messages"].as_array_mut().unwrap().truncate(1);
    value["messages"][0]["content"] = json!("Compacted history: continue monitoring.");
    value["tool_choice"] = json!({"type": "tool", "name": "SendMessage"});
    let (status, output) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(serde_json::to_vec(&value).unwrap()),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&output));
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    let outbound: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(outbound["tools"].as_array().unwrap().len(), 2);
    assert_eq!(outbound["tools"][1]["name"], "SendMessage");
    assert_eq!(outbound["tool_choice"]["name"], "SendMessage");
}

#[tokio::test]
async fn closed_responses_delta_is_completed_by_final_arguments() {
    for stream in [false, true] {
        let fake = FakeTransport::new();
        let args = json!({"to": "synthetic_peer", "message": "继续监控"});
        let frames: Vec<String> = [
            json!({"type": "response.output_item.added", "output_index": 0, "item": {
                "type": "function_call", "id": "fc_0", "call_id": "call_0", "name": "SendMessage", "arguments": ""
            }}),
            json!({"type": "response.function_call_arguments.delta", "item_id": "fc_0", "delta": "{}"}),
            json!({"type": "response.function_call_arguments.done", "item_id": "fc_0", "arguments": args.to_string()}),
            json!({"type": "response.completed", "response": {"status": "completed"}}),
        ]
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect();
        let borrowed: Vec<&str> = frames.iter().map(String::as_str).collect();
        fake.push("a.example.com", sse_ok(&borrowed));
        let mut config = two_endpoint_config();
        config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
        config.endpoints[1].enabled = false;
        let engine = engine_with(config.normalized(), fake);
        let mut value = discovery_body(true);
        value["stream"] = json!(stream);
        let (status, output) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![("content-type".into(), "application/json".into())],
            Bytes::from(serde_json::to_vec(&value).unwrap()),
        )
        .await;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&output));
        if stream {
            let events: Vec<Value> = std::str::from_utf8(&output)
                .unwrap()
                .lines()
                .filter_map(|line| line.strip_prefix("data: "))
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let actual: String = events
                .iter()
                .filter_map(|event| event.pointer("/delta/partial_json").and_then(Value::as_str))
                .collect();
            assert_eq!(serde_json::from_str::<Value>(&actual).unwrap(), args);
            assert!(events.iter().any(|event| event["type"] == "message_stop"));
            assert!(!events.iter().any(|event| event["type"] == "error"));
        } else {
            let body: Value = serde_json::from_slice(&output).unwrap();
            assert_eq!(body["content"][0]["input"], args);
            assert_eq!(body["stop_reason"], "tool_use");
        }
    }
}

#[tokio::test]
async fn invalid_discovery_reference_is_rejected_before_upstream() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAIResponses;
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    let mut value = discovery_body(true);
    value["messages"][2]["content"][0]["content"][0]["tool_name"] = json!("unknown");
    let (status, output) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(serde_json::to_vec(&value).unwrap()),
    )
    .await;
    assert_eq!(status, 400);
    assert!(String::from_utf8_lossy(&output).contains("tool_name"));
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn reverse_bridge_preserves_initial_tool_input() {
    let fake = FakeTransport::new();
    let frames: Vec<String> = [
        json!({"type": "message_start", "message": {"model": "claude-opus-5"}}),
        json!({"type": "content_block_start", "index": 0, "content_block": {
            "type": "tool_use", "id": "call_0", "name": "SendMessage", "input": {"to": "synthetic_peer"}
        }}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}}),
        json!({"type": "message_stop"}),
    ]
    .iter()
    .map(|event| format!("data: {event}\n\n"))
    .collect();
    let borrowed: Vec<&str> = frames.iter().map(String::as_str).collect();
    fake.push("a.example.com", sse_ok(&borrowed));
    let mut config = two_endpoint_config();
    config.endpoints[1].enabled = false;
    let engine = engine_with(config.normalized(), fake.clone());
    let (status, output) = call(
        &engine,
        loopback(),
        "/v1/chat/completions",
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(
            serde_json::to_vec(&json!({
                "model": "claude-opus-5", "stream": false,
                "messages": [{"role": "user", "content": "inspect a synthetic peer"}]
            }))
            .unwrap(),
        ),
    )
    .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&output));
    let response: Value = serde_json::from_slice(&output).unwrap();
    let args = response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
        .as_str()
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(args).unwrap(),
        json!({"to": "synthetic_peer"})
    );
}
