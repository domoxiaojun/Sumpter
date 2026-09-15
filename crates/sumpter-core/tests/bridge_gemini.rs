//! Gemini 会话桥测试:请求双向转换、能力边界、流式状态机。

use serde_json::{Value, json};
use sumpter_core::bridge::SseBridge;
use sumpter_core::bridge_gemini::{
    GeminiClientBridge, GeminiOperation, GeminiReplay, GeminiStreamBridge,
    check_anthropic_to_gemini, check_gemini_to_anthropic, gemini_operation, gemini_to_anthropic,
    try_make_gemini_body,
};
use sumpter_core::model_name::ReasoningEffort;
use sumpter_core::routing::RoutingRequest;

fn request(value: Value) -> RoutingRequest {
    RoutingRequest::from_value(&value).unwrap()
}

fn data_jsons(text: &str) -> Vec<Value> {
    text.split("\n\n")
        .filter_map(|block| {
            block
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .and_then(|payload| serde_json::from_str(payload).ok())
        })
        .collect()
}

fn sse_events(text: &str) -> Vec<(String, Value)> {
    text.split("\n\n")
        .filter(|block| !block.trim().is_empty())
        .filter_map(|block| {
            let mut event = String::new();
            let mut data = Value::Null;
            for line in block.lines() {
                if let Some(rest) = line.strip_prefix("event: ") {
                    event = rest.to_string();
                } else if let Some(rest) = line.strip_prefix("data: ") {
                    data = serde_json::from_str(rest).unwrap();
                }
            }
            (!event.is_empty()).then_some((event, data))
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 操作判定
// ---------------------------------------------------------------------------

#[test]
fn session_operations_are_distinguished_from_auxiliary_ones() {
    assert_eq!(
        gemini_operation("/v1beta/models/gemini-2.5-pro:generateContent"),
        GeminiOperation::Generate
    );
    assert_eq!(
        gemini_operation("/v1beta/models/gemini-2.5-pro:streamGenerateContent"),
        GeminiOperation::StreamGenerate
    );
    // 辅助操作不能进会话转换。
    for path in [
        "/v1beta/models/gemini-2.5-pro:countTokens",
        "/v1beta/models/text-embedding-004:embedContent",
        "/v1beta/models/gemini-2.5-pro:predict",
    ] {
        assert_eq!(gemini_operation(path), GeminiOperation::Auxiliary, "{path}");
    }
}

// ---------------------------------------------------------------------------
// 请求:Gemini → Anthropic
// ---------------------------------------------------------------------------

#[test]
fn gemini_request_maps_text_system_tools_and_images() {
    let body = json!({
        "systemInstruction": {"parts": [{"text": "be brief"}]},
        "contents": [
            {"role": "user", "parts": [
                {"text": "look"},
                {"inlineData": {"mimeType": "image/png", "data": "AAAA"}},
            ]},
            {"role": "model", "parts": [
                {"functionCall": {"name": "read_file", "args": {"path": "a"}}}
            ]},
            {"role": "user", "parts": [
                {"functionResponse": {"name": "read_file", "response": {"output": "内容"}}}
            ]},
        ],
        "tools": [{"functionDeclarations": [{
            "name": "read_file",
            "description": "read",
            "parameters": {"type": "object"},
        }]}],
        "generationConfig": {"maxOutputTokens": 256, "temperature": 0.4, "stopSequences": ["END"]},
    });
    let out = gemini_to_anthropic(&body, "models/gemini-2.5-pro").unwrap();
    assert_eq!(out["model"], "gemini-2.5-pro");
    assert_eq!(out["system"], "be brief");
    assert_eq!(out["max_tokens"], 256);
    assert_eq!(out["temperature"], 0.4);
    assert_eq!(out["stop_sequences"], json!(["END"]));
    assert_eq!(out["tools"][0]["name"], "read_file");
    assert_eq!(out["tools"][0]["input_schema"], json!({"type": "object"}));

    let messages = out["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(
        messages[0]["content"][0],
        json!({"type": "text", "text": "look"})
    );
    assert_eq!(
        messages[0]["content"][1],
        json!({"type": "image",
               "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}})
    );
    // model → assistant,functionCall → tool_use。
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"][0]["type"], "tool_use");
    assert_eq!(messages[1]["content"][0]["name"], "read_file");
    assert_eq!(messages[1]["content"][0]["input"], json!({"path": "a"}));
    // functionResponse → tool_result(user 消息),response.output 取出来当正文。
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["content"], "内容");

    assert!(check_gemini_to_anthropic(&body, "gemini-2.5-pro").is_ok());
}

#[test]
fn gemini_thinking_parts_are_not_replayed_as_text() {
    let body = json!({
        "contents": [{"role": "model", "parts": [
            {"text": "mull", "thought": true},
            {"text": "answer"},
        ]}],
    });
    let out = gemini_to_anthropic(&body, "gemini-2.5-pro").unwrap();
    // 思考摘要没有可回放的块类型,不回放;正文保留。
    assert_eq!(out["messages"][0]["content"].as_array().unwrap().len(), 1);
    assert_eq!(out["messages"][0]["content"][0]["text"], "answer");
}

#[test]
fn gemini_checker_rejects_unrepresentable_fields() {
    // 安全设置被悄悄取消是最不该发生的静默降级。
    let safety = json!({
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
        "safetySettings": [{"category": "HARM_CATEGORY_HATE_SPEECH", "threshold": "BLOCK_NONE"}],
    });
    assert!(
        check_gemini_to_anthropic(&safety, "gemini-2.5-pro")
            .unwrap_err()
            .to_string()
            .contains("safetySettings")
    );

    let cached = json!({
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
        "cachedContent": "cachedContents/demo",
    });
    assert!(
        check_gemini_to_anthropic(&cached, "gemini-2.5-pro")
            .unwrap_err()
            .to_string()
            .contains("cachedContent")
    );

    let remote_file = json!({
        "contents": [{"role": "user", "parts": [
            {"fileData": {"mimeType": "image/png", "fileUri": "https://example.invalid/a"}}
        ]}],
    });
    assert!(
        check_gemini_to_anthropic(&remote_file, "gemini-2.5-pro")
            .unwrap_err()
            .to_string()
            .contains("fileData")
    );

    // 声明了 JSON 契约却没有 schema:客户端会按 schema 解析,不能降级成自由文本。
    let json_without_schema = json!({
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
        "generationConfig": {"responseMimeType": "application/json"},
    });
    assert!(
        check_gemini_to_anthropic(&json_without_schema, "gemini-2.5-pro")
            .unwrap_err()
            .to_string()
            .contains("responseMimeType")
    );
}

#[test]
fn gemini_json_schema_maps_to_output_config() {
    let body = json!({
        "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
        "generationConfig": {
            "responseMimeType": "application/json",
            "responseJsonSchema": {"type": "object", "properties": {"a": {"type": "string"}}},
        },
    });
    let out = gemini_to_anthropic(&body, "gemini-2.5-pro").unwrap();
    assert_eq!(out["output_config"]["format"]["type"], "json_schema");
    assert_eq!(
        out["output_config"]["format"]["schema"]["properties"]["a"],
        json!({"type": "string"})
    );
}

// ---------------------------------------------------------------------------
// 请求:Anthropic → Gemini
// ---------------------------------------------------------------------------

#[test]
fn anthropic_request_maps_to_gemini_body() {
    let req = request(json!({
        "model": "gemini-2.5-pro",
        "system": [{"type": "text", "text": "sys"}],
        "max_tokens": 512,
        "messages": [
            {"role": "user", "content": [
                {"type": "text", "text": "look"},
                {"type": "image",
                 "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}},
            ]},
            {"role": "assistant", "content": [
                {"type": "tool_use", "id": "call_1", "name": "read_file", "input": {"path": "a"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "call_1", "content": "内容"}
            ]},
        ],
        "tools": [{"name": "read_file", "description": "read",
                   "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}}],
        "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}},
    }));
    let body = try_make_gemini_body(&req, Some(ReasoningEffort::High), None).unwrap();
    assert_eq!(body["systemInstruction"]["parts"][0]["text"], "sys");
    let contents = body["contents"].as_array().unwrap();
    assert_eq!(contents[0]["role"], "user");
    assert_eq!(
        contents[0]["parts"][1]["inlineData"]["mimeType"],
        "image/png"
    );
    assert_eq!(contents[1]["role"], "model");
    assert_eq!(contents[1]["parts"][0]["functionCall"]["name"], "read_file");
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"]["response"]["output"],
        "内容"
    );
    // 完整 JSON Schema 走 parametersJsonSchema,简单 schema 不会丢关键字。
    let schema = &body["tools"][0]["functionDeclarations"][0]["parametersJsonSchema"];
    assert_eq!(schema["properties"]["path"]["type"], "string");
    let config = &body["generationConfig"];
    assert_eq!(config["maxOutputTokens"], 512);
    assert_eq!(config["responseMimeType"], "application/json");
    assert_eq!(config["responseJsonSchema"], json!({"type": "object"}));
    assert_eq!(config["thinkingConfig"]["includeThoughts"], true);
    assert!(config["thinkingConfig"]["thinkingBudget"].as_i64().unwrap() > 0);

    assert!(check_anthropic_to_gemini(&req).is_ok());
}

#[test]
fn anthropic_to_gemini_rejects_url_images() {
    // Gemini 只收内联字节;URL 源要代理先取回内容,转换器不替用户发网络请求。
    let req = request(json!({
        "model": "m",
        "messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "url", "url": "https://example.invalid/a.png"}}
        ]}],
    }));
    assert!(
        check_anthropic_to_gemini(&req)
            .unwrap_err()
            .to_string()
            .contains("source.type=url")
    );
}

// ---------------------------------------------------------------------------
// 响应:Gemini SSE → Anthropic SSE
// ---------------------------------------------------------------------------

#[test]
fn gemini_stream_bridge_emits_anthropic_events() {
    let mut bridge = GeminiStreamBridge::new("msg_g".into(), "gemini-2.5-pro".into(), true);
    // Gemini 的文本可能是累计快照:重复的前缀不能重复发出。
    let out = String::from_utf8(bridge.feed(
        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"He\"}]}}]}\n\n",
    ))
    .unwrap();
    let events = sse_events(&out);
    assert_eq!(events[0].0, "message_start");
    assert_eq!(events[1].0, "content_block_start");
    assert_eq!(events[2].1["delta"]["text"], "He");

    let out = String::from_utf8(bridge.feed(
        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Hello\"}]}}]}\n\n",
    ))
    .unwrap();
    let events = sse_events(&out);
    assert_eq!(events[0].1["delta"]["text"], "llo");

    let out = String::from_utf8(bridge.feed(
        b"data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"functionCall\":{\"name\":\"read_file\",\"args\":{\"path\":\"a\"}}}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":20,\"cachedContentTokenCount\":5,\"candidatesTokenCount\":8,\"thoughtsTokenCount\":3}}\n\n",
    ))
    .unwrap();
    let events = sse_events(&out);
    // 文本块先关,再开工具块。
    assert_eq!(events[0].0, "content_block_stop");
    assert_eq!(events[1].1["content_block"]["type"], "tool_use");
    assert_eq!(events[1].1["content_block"]["name"], "read_file");
    assert_eq!(events[2].1["delta"]["type"], "input_json_delta");

    let out = String::from_utf8(bridge.finish()).unwrap();
    let events = sse_events(&out);
    assert_eq!(events[0].0, "content_block_stop");
    assert_eq!(events[1].0, "message_delta");
    assert_eq!(events[1].1["delta"]["stop_reason"], "end_turn");
    // prompt 含缓存命中 → Anthropic 的 input 是新鲜输入(20-5)。
    assert_eq!(events[1].1["usage"]["input_tokens"], 15);
    assert_eq!(events[1].1["usage"]["output_tokens"], 8);
    assert_eq!(events[2].0, "message_stop");

    let (input, output, reasoning) = bridge.usage();
    assert_eq!((input, output, reasoning), (15, 8, 3));
    // 真实的 assistant parts 留给 engine 做工具回放。
    assert_eq!(bridge.replay_parts().len(), 3);
}

#[test]
fn gemini_stream_bridge_fails_on_block_and_error_envelopes() {
    let mut blocked = GeminiStreamBridge::new("m".into(), "g".into(), true);
    let out = String::from_utf8(
        blocked.feed(b"data: {\"promptFeedback\":{\"blockReason\":\"SAFETY\"}}\n\n"),
    )
    .unwrap();
    assert!(out.contains("event: error"));
    assert!(matches!(
        blocked.terminal(),
        sumpter_core::bridge::BridgeTerminal::Failed(_)
    ));

    let mut errored = GeminiStreamBridge::new("m".into(), "g".into(), true);
    let out = String::from_utf8(
        errored.feed(b"data: {\"error\":{\"code\":400,\"message\":\"bad request\"}}\n\n"),
    )
    .unwrap();
    assert!(out.contains("bad request"));
    assert!(matches!(
        errored.terminal(),
        sumpter_core::bridge::BridgeTerminal::Failed(_)
    ));

    // 畸形 JSON 不能让流「正常结束」。
    let mut malformed = GeminiStreamBridge::new("m".into(), "g".into(), true);
    malformed.feed(b"data: {not json}\n\n");
    assert!(matches!(
        malformed.terminal(),
        sumpter_core::bridge::BridgeTerminal::Failed(_)
    ));
}

// ---------------------------------------------------------------------------
// 响应:Anthropic SSE → 客户端 Gemini
// ---------------------------------------------------------------------------

fn anthropic_sse() -> String {
    [
        r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","model":"gemini-2.5-pro","usage":{"input_tokens":0}}}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"He"}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"llo"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":0}"#,
        r#"event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"read_file","input":{}}}"#,
        r#"event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"path\":\"a\"}"}}"#,
        r#"event: content_block_stop
data: {"type":"content_block_stop","index":1}"#,
        r#"event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":12,"output_tokens":7}}"#,
        r#"event: message_stop
data: {"type":"message_stop"}"#,
    ]
    .map(|block| format!("{block}\n\n"))
    .join("")
}

#[test]
fn gemini_client_bridge_streams_text_and_tools() {
    let mut bridge = GeminiClientBridge::new("gemini-2.5-pro".into(), true);
    let out = String::from_utf8(bridge.feed(anthropic_sse().as_bytes())).unwrap();
    let frames = data_jsons(&out);
    let texts: Vec<&str> = frames
        .iter()
        .filter_map(|frame| frame.pointer("/candidates/0/content/parts/0/text"))
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(texts, vec!["He", "llo"]);
    let last = frames.last().unwrap();
    assert_eq!(last["candidates"][0]["finishReason"], "STOP");
    assert_eq!(
        last["candidates"][0]["content"]["parts"][0]["functionCall"]["name"],
        "read_file"
    );
    assert_eq!(
        last["candidates"][0]["content"]["parts"][0]["functionCall"]["args"],
        json!({"path": "a"})
    );
    assert_eq!(last["usageMetadata"]["promptTokenCount"], 12);
    assert_eq!(last["usageMetadata"]["candidatesTokenCount"], 7);
    assert!(matches!(
        bridge.terminal(),
        sumpter_core::bridge::BridgeTerminal::Completed
    ));
}

#[test]
fn gemini_client_bridge_aggregates_non_stream() {
    let mut bridge = GeminiClientBridge::new("gemini-2.5-pro".into(), false);
    // 非流式在 message_stop 时一次性渲染,所以 JSON 从 feed 返回。
    let out = String::from_utf8(bridge.feed(anthropic_sse().as_bytes())).unwrap();
    let body: Value = serde_json::from_str(&out).unwrap();
    let parts = body["candidates"][0]["content"]["parts"]
        .as_array()
        .unwrap();
    assert_eq!(parts[0]["text"], "Hello");
    assert_eq!(parts[1]["functionCall"]["name"], "read_file");
    assert_eq!(body["usageMetadata"]["promptTokenCount"], 12);
}

#[test]
fn gemini_client_bridge_reports_missing_terminal_as_failure() {
    // 上游没给 message_stop:不能把截断的流当成功。
    let truncated = anthropic_sse()
        .split("event: message_stop")
        .next()
        .unwrap()
        .to_string();
    let mut bridge = GeminiClientBridge::new("m".into(), true);
    bridge.feed(truncated.as_bytes());
    bridge.finish();
    assert!(matches!(
        bridge.terminal(),
        sumpter_core::bridge::BridgeTerminal::Failed(_)
    ));
}

#[test]
fn anthropic_to_gemini_reuses_real_signatures_only_for_the_matching_turn() {
    let replay = GeminiReplay {
        parts: json!([
            {"text": "先看一下"},
            {"functionCall": {"name": "read_file", "args": {"path": "a"}},
             "thoughtSignature": "sig-abc"},
        ])
        .as_array()
        .unwrap()
        .clone(),
    };
    let turn = |input: Value| {
        request(json!({
            "model": "gemini-2.5-pro",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "gemini_call_1", "name": "read_file",
                     "input": input}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "gemini_call_1", "content": "内容"}
                ]},
            ],
        }))
    };

    // 入参与上一轮逐字相同 → 整轮换成真实 parts,签名原样回传。
    let body = try_make_gemini_body(&turn(json!({"path": "a"})), None, Some(&replay)).unwrap();
    let parts = body["contents"][0]["parts"].as_array().unwrap();
    assert_eq!(parts[0]["text"], "先看一下");
    assert_eq!(parts[1]["thoughtSignature"], "sig-abc");

    // 入参不同 → 不是同一轮。宁可没有签名,也不能把签名配到别的调用上。
    let drifted = try_make_gemini_body(&turn(json!({"path": "OTHER"})), None, Some(&replay));
    let parts = drifted.unwrap()["contents"][0]["parts"].clone();
    assert!(parts[0].get("thoughtSignature").is_none());
    assert!(parts[0].get("text").is_none());

    // 没有回放状态时同样只发重建结果(不伪造签名)。
    let plain = try_make_gemini_body(&turn(json!({"path": "a"})), None, None).unwrap();
    assert!(
        plain["contents"][0]["parts"][0]
            .get("thoughtSignature")
            .is_none()
    );
}
