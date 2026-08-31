//! 入站 OpenAI 兼容层测试:请求转换器(chat/Responses → Anthropic)与
//! 反向流桥(Anthropic SSE → chat chunks / Responses SSE),含分片投喂。

use serde_json::{Value, json};
use sumpter_core::bridge::SseBridge;
use sumpter_core::bridge_in::{
    ChatClientBridge, ResponsesClientBridge, chat_to_anthropic, check_chat_to_anthropic,
    check_responses_to_anthropic, client_wants_stream, responses_to_anthropic,
    try_chat_to_anthropic, try_responses_to_anthropic,
};

// ---------------------------------------------------------------------------
// 请求侧:chat → anthropic
// ---------------------------------------------------------------------------

#[test]
fn chat_basic_conversion() {
    let body = json!({
        "model": "claude-sonnet-5",
        "messages": [
            {"role": "system", "content": "be brief"},
            {"role": "user", "content": "hi"},
            {"role": "assistant", "content": "hello"},
            {"role": "user", "content": [{"type": "text", "text": "again"}]},
        ],
        "max_tokens": 128,
        "temperature": 0.5,
        "stop": ["</x>"],
        "stream": false,
    });
    let out = chat_to_anthropic(&body).unwrap();
    assert_eq!(out["model"], "claude-sonnet-5");
    assert_eq!(out["system"], "be brief");
    assert_eq!(out["max_tokens"], 128);
    assert_eq!(out["temperature"], 0.5);
    assert_eq!(out["stop_sequences"], json!(["</x>"]));
    // 内部恒流式,客户端非流式由响应桥聚合。
    assert_eq!(out["stream"], true);
    assert!(!client_wants_stream(&body));
    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"][0]["text"], "hi");
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[2]["content"][0]["text"], "again");
}
#[test]
fn chat_tools_and_tool_history_roundtrip() {
    let body = json!({
        "model": "claude-sonnet-5",
        "messages": [
            {"role": "user", "content": "run ls"},
            {"role": "assistant", "content": null, "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": {"name": "shell", "arguments": "{\"cmd\":[\"ls\"]}"},
            }]},
            {"role": "tool", "tool_call_id": "call_1", "content": "a.txt"},
        ],
        "tools": [{"type": "function", "function": {
            "name": "shell", "description": "run", "parameters": {"type": "object", "properties": {}},
        }}],
        "tool_choice": "auto",
    });
    let out = chat_to_anthropic(&body).unwrap();
    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["id"], "call_1");
    assert_eq!(messages[1]["content"][0]["input"]["cmd"], json!(["ls"]));
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "call_1");
    assert_eq!(messages[2]["content"][0]["content"], "a.txt");
    let tools = out["tools"].as_array().unwrap();
    assert_eq!(tools[0]["name"], "shell");
    assert!(tools[0]["input_schema"].is_object());
    assert_eq!(out["tool_choice"], json!({"type": "auto"}));
}

#[test]
fn chat_effort_and_defaults() {
    let body = json!({
        "model": "claude-opus-5",
        "messages": [{"role": "user", "content": "hi"}],
        "reasoning_effort": "high",
    });
    let out = chat_to_anthropic(&body).unwrap();
    assert_eq!(out["model"], "claude-opus-5(high)");
    assert_eq!(out["max_tokens"], 32000); // 缺省回落
    // 模型名自带后缀时客户端参数让位。
    let body2 = json!({
        "model": "claude-opus-5(low)",
        "messages": [{"role": "user", "content": "hi"}],
        "reasoning_effort": "high",
    });
    assert_eq!(
        chat_to_anthropic(&body2).unwrap()["model"],
        "claude-opus-5(low)"
    );
}

#[test]
fn chat_rejects_bad_bodies() {
    assert!(chat_to_anthropic(&json!([])).is_err());
    assert!(chat_to_anthropic(&json!({"messages": []})).is_err());
    assert!(chat_to_anthropic(&json!({"model": "m", "messages": []})).is_err());
}

#[test]
fn chat_checker_rejects_lossy_content_and_tool_shapes() {
    let image = json!({
        "model": "m",
        "messages": [{"role": "user", "content": [
            {"type": "image_url", "image_url": {"url": "https://example.invalid/a.png"}}
        ]}],
    });
    let error = check_chat_to_anthropic(&image).unwrap_err().to_string();
    assert!(error.contains("unsupported content block `image_url`"));

    let citations = json!({
        "model": "m",
        "messages": [{"role": "user", "content": [{
            "type": "text", "text": "quoted",
            "citations": [{"url": "https://example.invalid"}]
        }]}],
    });
    assert!(
        check_chat_to_anthropic(&citations)
            .unwrap_err()
            .to_string()
            .contains("citations")
    );

    let malformed_arguments = json!({
        "model": "m",
        "messages": [
            {"role": "user", "content": "run"},
            {"role": "assistant", "tool_calls": [{
                "id": "call_1", "type": "function",
                "function": {"name": "shell", "arguments": "not-json"}
            }]}
        ]
    });
    assert!(
        try_chat_to_anthropic(&malformed_arguments)
            .unwrap_err()
            .to_string()
            .contains("must be a JSON object")
    );

    let custom_tool = json!({
        "model": "m",
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{"type": "custom", "name": "shell"}],
    });
    assert!(
        check_chat_to_anthropic(&custom_tool)
            .unwrap_err()
            .to_string()
            .contains("unsupported tool type `custom`")
    );
}

// ---------------------------------------------------------------------------
// 请求侧:responses → anthropic
// ---------------------------------------------------------------------------

#[test]
fn responses_conversion_with_tools() {
    let body = json!({
        "model": "claude-sonnet-5",
        "instructions": "you are codex",
        "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "ls"}]},
            {"type": "reasoning", "summary": []},
            {"type": "function_call", "call_id": "call_9", "name": "shell",
             "arguments": "{\"cmd\":[\"ls\"]}"},
            {"type": "function_call_output", "call_id": "call_9", "output": "a.txt"},
        ],
        "tools": [
            {"type": "function", "name": "shell", "description": "run",
             "parameters": {"type": "object"}},
            {"type": "web_search", "name": "must-not-be-normalized"},
        ],
        "tool_choice": "required",
        "max_output_tokens": 999,
        "reasoning": {"effort": "medium"},
        "stream": true,
    });
    let out = responses_to_anthropic(&body).unwrap();
    assert_eq!(out["model"], "claude-sonnet-5(medium)");
    assert_eq!(out["system"], "you are codex");
    assert_eq!(out["max_tokens"], 999);
    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3); // user / assistant(tool_use) / user(tool_result)
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["id"], "call_9");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    let tools = out["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1); // 宽松归一化仍忽略 Responses 内建工具
    assert_eq!(out["tool_choice"], json!({"type": "any"}));
    assert!(client_wants_stream(&body));
}

#[test]
fn responses_string_input_and_bare_message() {
    let out = responses_to_anthropic(&json!({"model": "m", "input": "hi"})).unwrap();
    assert_eq!(out["messages"][0]["content"][0]["text"], "hi");
    // 无 type 带 role 的裸消息(SDK 简写)。
    let out2 = responses_to_anthropic(&json!({
        "model": "m",
        "input": [{"role": "user", "content": "yo"}],
    }))
    .unwrap();
    assert_eq!(out2["messages"][0]["content"][0]["text"], "yo");
}

#[test]
fn responses_checker_rejects_reasoning_unknown_blocks_and_builtin_tools() {
    let reasoning = json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "reasoning", "summary": [
                {"type": "summary_text", "text": "private chain"}
            ]}
        ]
    });
    assert!(
        check_responses_to_anthropic(&reasoning)
            .unwrap_err()
            .to_string()
            .contains("reasoning")
    );

    let unknown = json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "computer_call", "id": "c1"}
        ]
    });
    assert!(
        check_responses_to_anthropic(&unknown)
            .unwrap_err()
            .to_string()
            .contains("computer_call")
    );

    let builtin = json!({
        "model": "m",
        "input": "search",
        "tools": [{"type": "web_search"}]
    });
    assert!(
        try_responses_to_anthropic(&builtin)
            .unwrap_err()
            .to_string()
            .contains("unsupported tool type `web_search`")
    );

    // 空 reasoning 占位没有正文/签名，不造成可观察信息丢失。
    let empty_reasoning = json!({
        "model": "m",
        "input": [
            {"type": "message", "role": "user", "content": "hi"},
            {"type": "reasoning", "summary": []}
        ]
    });
    assert!(check_responses_to_anthropic(&empty_reasoning).is_ok());
}

#[test]
fn translator_checker_handles_tool_choice_without_tools_and_include() {
    for tool_choice in ["auto", "none"] {
        let chat = json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hi"}],
            "tool_choice": tool_choice,
        });
        assert!(check_chat_to_anthropic(&chat).is_ok());

        let responses = json!({
            "model": "m",
            "input": "hi",
            "tool_choice": tool_choice,
            "include": [],
        });
        assert!(check_responses_to_anthropic(&responses).is_ok());
    }

    let required_without_tools = json!({
        "model": "m",
        "input": "hi",
        "tool_choice": "required",
    });
    assert!(
        check_responses_to_anthropic(&required_without_tools)
            .unwrap_err()
            .to_string()
            .contains("tool_choice")
    );

    let include = json!({
        "model": "m",
        "input": "hi",
        "include": ["reasoning.encrypted_content"],
    });
    assert!(
        check_responses_to_anthropic(&include)
            .unwrap_err()
            .to_string()
            .contains("include")
    );

    let missing_output = json!({
        "model": "m",
        "input": [{"type": "function_call_output", "call_id": "call_1"}],
    });
    assert!(
        check_responses_to_anthropic(&missing_output)
            .unwrap_err()
            .to_string()
            .contains("output is required")
    );

    let missing_tool_content = json!({
        "model": "m",
        "messages": [{"role": "tool", "tool_call_id": "call_1"}],
    });
    assert!(
        check_chat_to_anthropic(&missing_tool_content)
            .unwrap_err()
            .to_string()
            .contains("content is required")
    );

    let strict_tool = json!({
        "model": "m",
        "input": "hi",
        "tools": [{
            "type": "function",
            "name": "shell",
            "parameters": {"type": "object"},
            "strict": true
        }],
    });
    assert!(
        check_responses_to_anthropic(&strict_tool)
            .unwrap_err()
            .to_string()
            .contains("strict")
    );

    let non_strict_tool = json!({
        "model": "m",
        "input": "hi",
        "tools": [{
            "type": "function",
            "name": "shell",
            "parameters": {"type": "object"},
            "strict": false
        }],
    });
    assert!(check_responses_to_anthropic(&non_strict_tool).is_ok());

    let invalid_choice = json!({
        "model": "m",
        "input": "hi",
        "tools": [{"type": "function", "name": "shell"}],
        "tool_choice": {"type": "auto"},
    });
    assert!(
        check_responses_to_anthropic(&invalid_choice)
            .unwrap_err()
            .to_string()
            .contains("tool_choice")
    );
}

// ---------------------------------------------------------------------------
// 响应侧公共 fixture:一段带 text + thinking + tool_use 的 Anthropic SSE
// ---------------------------------------------------------------------------

fn anthropic_sse() -> String {
    [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","model":"claude-up","usage":{"input_tokens":10}}}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"mull"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"I'll run ls."}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_7","name":"shell","input":{}}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"[\"ls\"]}"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":2}"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":42}}"#,
        r#"event: message_stop
data: {"type":"message_stop"}"#,
    ]
    .map(|block| format!("{block}\n\n"))
    .join("")
}

/// 按奇数步长分片投喂,验证跨 chunk 拼装。
fn feed_in_pieces(bridge: &mut dyn SseBridge, raw: &str, step: usize) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    for chunk in bytes.chunks(step) {
        out.extend(bridge.feed(chunk));
    }
    out.extend(bridge.finish());
    String::from_utf8(out).unwrap()
}

fn data_jsons(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter_map(|block| {
            block
                .lines()
                .find_map(|l| l.strip_prefix("data: "))
                .filter(|p| *p != "[DONE]")
                .and_then(|p| serde_json::from_str(p).ok())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// chat 反向桥
// ---------------------------------------------------------------------------

#[test]
fn chat_bridge_streams_text_and_tool_calls() {
    let mut bridge = ChatClientBridge::new("m1".into(), "fallback".into(), true);
    let out = feed_in_pieces(&mut bridge, &anthropic_sse(), 7);
    assert!(out.ends_with("data: [DONE]\n\n"));
    let chunks = data_jsons(&out);
    // message_start 模型覆盖 fallback。
    assert!(chunks.iter().all(|c| c["model"] == "claude-up"));
    let text: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["content"].as_str())
        .collect();
    assert_eq!(text, "I'll run ls.");
    let args: String = chunks
        .iter()
        .filter_map(|c| c["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"].as_str())
        .collect();
    assert_eq!(args, r#"{"cmd":["ls"]}"#);
    let last = chunks.last().unwrap();
    assert_eq!(last["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(last["usage"]["prompt_tokens"], 10);
    assert_eq!(last["usage"]["completion_tokens"], 42);
    // thinking 不外漏。
    assert!(!out.contains("mull"));
}

#[test]
fn chat_bridge_aggregates_non_stream() {
    let mut bridge = ChatClientBridge::new("m1".into(), "fallback".into(), false);
    let out = feed_in_pieces(&mut bridge, &anthropic_sse(), 11);
    let completion: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(completion["object"], "chat.completion");
    let message = &completion["choices"][0]["message"];
    assert_eq!(message["content"], "I'll run ls.");
    assert_eq!(message["tool_calls"][0]["id"], "toolu_7");
    assert_eq!(
        message["tool_calls"][0]["function"]["arguments"],
        r#"{"cmd":["ls"]}"#
    );
    assert_eq!(completion["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(completion["usage"]["total_tokens"], 52);
}

// ---------------------------------------------------------------------------
// Responses 反向桥
// ---------------------------------------------------------------------------

#[test]
fn responses_bridge_streams_items_and_completed() {
    let mut bridge = ResponsesClientBridge::new("m1".into(), "fallback".into(), true);
    let out = feed_in_pieces(&mut bridge, &anthropic_sse(), 9);
    assert!(out.contains("event: response.created\n"));
    assert!(out.contains("event: response.reasoning_summary_text.delta\n"));
    assert!(out.contains("event: response.output_text.delta\n"));
    assert!(out.contains("event: response.function_call_arguments.delta\n"));
    assert!(out.contains("event: response.completed\n"));
    let events = data_jsons(&out);
    let completed = events
        .iter()
        .find(|e| e["type"] == "response.completed")
        .unwrap();
    let output = completed["response"]["output"].as_array().unwrap();
    assert_eq!(output.len(), 3);
    assert_eq!(output[0]["type"], "reasoning");
    assert_eq!(output[0]["summary"][0]["text"], "mull");
    assert_eq!(output[1]["type"], "message");
    assert_eq!(output[1]["content"][0]["text"], "I'll run ls.");
    assert_eq!(output[2]["type"], "function_call");
    assert_eq!(output[2]["call_id"], "toolu_7");
    assert_eq!(output[2]["name"], "shell");
    assert_eq!(output[2]["arguments"], r#"{"cmd":["ls"]}"#);
    assert_eq!(completed["response"]["usage"]["input_tokens"], 10);
    assert_eq!(completed["response"]["usage"]["output_tokens"], 42);
    // function_call 项在流中间就完整给出(output_item.done),Codex 靠它拿参数。
    let item_done = events
        .iter()
        .filter(|e| e["type"] == "response.output_item.done")
        .find(|e| e["item"]["type"] == "function_call")
        .unwrap();
    assert_eq!(item_done["item"]["arguments"], r#"{"cmd":["ls"]}"#);
}

#[test]
fn responses_bridge_aggregates_non_stream() {
    let mut bridge = ResponsesClientBridge::new("m1".into(), "fallback".into(), false);
    let out = feed_in_pieces(&mut bridge, &anthropic_sse(), 13);
    let response: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(response["object"], "response");
    assert_eq!(response["status"], "completed");
    assert_eq!(response["model"], "claude-up");
    assert_eq!(response["output"].as_array().unwrap().len(), 3);
}

#[test]
fn bridges_report_error_when_upstream_truncated() {
    // 上游中途断流(无 message_stop)必须显式报错,不能伪造成功尾。
    let partial = anthropic_sse();
    let cut = partial.split("event: message_delta").next().unwrap();
    let mut chat = ChatClientBridge::new("m1".into(), "m".into(), true);
    let out = String::from_utf8({
        let mut v = chat.feed(cut.as_bytes());
        v.extend(chat.finish());
        v
    })
    .unwrap();
    assert!(out.contains("api_error"));
    let mut responses = ResponsesClientBridge::new("m1".into(), "m".into(), true);
    let out2 = String::from_utf8({
        let mut v = responses.feed(cut.as_bytes());
        v.extend(responses.finish());
        v
    })
    .unwrap();
    assert!(out2.contains("event: error\n"));
}
