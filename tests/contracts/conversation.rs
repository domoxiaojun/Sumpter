//! 会话协议转换矩阵:四种客户端方言 × 四种上游协议的**原生**与**转换**路径。
//!
//! 一个用例覆盖一个方向,断言真实出站(host/path/auth)与客户端拿到的方言,
//! 而不是只看 HTTP 200 —— 「200 但内容形状不是客户端协议」正是这里要防的回归。
//!
//! 复杂字段语义(图片顺序、工具关联、用量换算、终态细则)留在 core 测试里,
//! 这里只保证接线正确,避免矩阵乘以所有字段。
use super::*;

/// 一个会话协议方向:客户端方言 + 上游协议。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wire {
    Anthropic,
    Chat,
    Responses,
    Gemini,
}

impl Wire {
    fn mode(self) -> EndpointProtocolMode {
        match self {
            Self::Anthropic => EndpointProtocolMode::Anthropic,
            Self::Chat => EndpointProtocolMode::OpenAI,
            Self::Responses => EndpointProtocolMode::OpenAIResponses,
            Self::Gemini => EndpointProtocolMode::Gemini,
        }
    }

    /// 该方言的入站路径(带模型)。
    fn inbound_path(self, model: &str) -> String {
        match self {
            Self::Anthropic => "/v1/messages".into(),
            Self::Chat => "/v1/chat/completions".into(),
            Self::Responses => "/v1/responses".into(),
            Self::Gemini => format!("/v1beta/models/{model}:streamGenerateContent?alt=sse"),
        }
    }

    /// 该方言的请求体。
    fn body(self) -> Value {
        match self {
            Self::Anthropic => json!({
                "model": "claude-opus-5",
                "system": "instruction-top",
                "max_tokens": 64,
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "system", "content": "instruction-system"},
                    {"role": "developer", "content": [{"type":"text", "text":"instruction-developer"}]}
                ],
                "stream": true,
            }),
            Self::Chat => json!({
                "model": "claude-opus-5",
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "system", "content": "instruction-system"},
                    {"role": "developer", "content": "instruction-developer"}
                ],
                "stream": true,
            }),
            Self::Responses => json!({
                "model": "claude-opus-5",
                "instructions": "instruction-top",
                "input": [
                    {"role":"user", "content":"hi"},
                    {"role":"system", "content":"instruction-system"},
                    {"role":"developer", "content":"instruction-developer"}
                ],
                "stream": true,
            }),
            // Gemini 的模型在 URL 上,请求体里没有。
            Self::Gemini => json!({
                "systemInstruction": {"parts":[{"text":"instruction-system"}, {"text":"instruction-developer"}]},
                "contents": [{"role": "user", "parts": [{"text": "hi"}]}],
            }),
        }
    }

    /// 上游返回该协议的**成功流**。
    fn upstream_stream(self) -> String {
        match self {
            Self::Anthropic => concat!(
                "event: message_start\n",
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"up\",\"usage\":{\"input_tokens\":3}}}\n\n",
                "event: content_block_start\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                "event: content_block_delta\n",
                "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n",
                "event: content_block_stop\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "event: message_delta\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}\n\n",
                "event: message_stop\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            )
            .to_string(),
            Self::Chat => concat!(
                "data: {\"model\":\"up\",\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\n",
                "data: [DONE]\n\n",
            )
            .to_string(),
            Self::Responses => concat!(
                "data: {\"type\":\"response.created\",\"response\":{\"model\":\"up\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n",
            )
            .to_string(),
            Self::Gemini => concat!(
                "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"ok\"}]},\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":3,\"candidatesTokenCount\":1,\"totalTokenCount\":4}}\n\n",
            )
            .to_string(),
        }
    }

    /// 客户端期望看到的回包形状里必须出现的事件/字段。
    fn client_marker(self) -> &'static str {
        match self {
            Self::Anthropic => "event: message_stop",
            Self::Chat => "[DONE]",
            Self::Responses => "response.completed",
            Self::Gemini => "\"candidates\"",
        }
    }
}

/// 单入口、固定协议的配置:只保留被转换的那一个方向。
fn single_wire_config(wire: Wire) -> AppConfig {
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    let endpoint = &mut config.endpoints[0];
    endpoint.protocol = wire.mode();
    endpoint.mappings = vec![ModelMapping {
        client_pattern: "claude-*".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Adaptive,
        effort: None,
        upstream_model: String::new(),
        capabilities: Vec::new(),
    }];
    // Gemini 的目标模型必须是模型 ID。
    if wire == Wire::Gemini {
        endpoint.mappings[0].client_pattern = "claude-opus-5".into();
        endpoint.mappings[0].upstream_model = "gemini-upstream".into();
    }
    config.normalized()
}

async fn run_direction(client: Wire, upstream: Wire) {
    let fake = FakeTransport::new();
    fake.push(
        "a.example.com",
        sse_ok(&[upstream.upstream_stream().as_str()]),
    );
    let engine = engine_with(single_wire_config(upstream), fake.clone());
    let (status, body) = call(
        &engine,
        loopback(),
        &client.inbound_path("claude-opus-5"),
        vec![("content-type".into(), "application/json".into())],
        Bytes::from(serde_json::to_vec(&client.body()).unwrap()),
    )
    .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, 200, "{client:?} -> {upstream:?} 应成功: {rendered}");
    assert!(
        rendered.contains(client.client_marker()),
        "{client:?} -> {upstream:?} 回包不是客户端方言: {rendered}"
    );

    let recorded = fake.requests();
    assert_eq!(recorded.len(), 1, "{client:?} -> {upstream:?}");
    let sent = &recorded[0];
    assert_eq!(sent.host, "a.example.com");
    let outbound: Value = serde_json::from_slice(&sent.body).unwrap();
    let instructions = match upstream {
        Wire::Anthropic if client == Wire::Anthropic => outbound.to_string(),
        Wire::Anthropic => outbound["system"].to_string(),
        Wire::Chat => outbound["messages"].to_string(),
        Wire::Responses => format!("{} {}", outbound["instructions"], outbound["input"]),
        Wire::Gemini => outbound["systemInstruction"].to_string(),
    };
    for marker in ["instruction-system", "instruction-developer"] {
        assert!(
            instructions.contains(marker),
            "{client:?} -> {upstream:?} lost {marker}: {outbound}"
        );
    }
    if client == upstream {
        let expected = client.body();
        for key in [
            "model",
            "system",
            "messages",
            "instructions",
            "input",
            "contents",
            "systemInstruction",
        ] {
            if expected.get(key).is_some() {
                assert_eq!(
                    outbound.get(key),
                    expected.get(key),
                    "native path changed {key}"
                );
            }
        }
    }
    let expected_path = match upstream {
        Wire::Anthropic => "/v1/messages",
        Wire::Chat => "/v1/chat/completions",
        Wire::Responses => "/v1/responses",
        Wire::Gemini => "/v1beta/models/gemini-upstream:streamGenerateContent?alt=sse",
    };
    assert_eq!(
        sent.path, expected_path,
        "{client:?} -> {upstream:?} 出站路径错误"
    );
    if upstream == Wire::Gemini {
        assert!(
            sent.headers
                .iter()
                .any(|(name, _)| name == "x-goog-api-key"),
            "Gemini 目标必须发 Gemini 凭据"
        );
    }
}

const WIRES: [Wire; 4] = [Wire::Anthropic, Wire::Chat, Wire::Responses, Wire::Gemini];

/// 4 条原生路径:同协议进同协议出,走原生面(字节级保真由各协议的原生契约测试覆盖)。
#[tokio::test]
async fn native_directions_stay_on_the_native_surface() {
    for wire in WIRES {
        run_direction(wire, wire).await;
    }
}

/// 12 条转换路径:异协议入口必须经转换面,不能原样透传。
#[tokio::test]
async fn translated_directions_use_the_conversion_surface() {
    for client in WIRES {
        for upstream in WIRES {
            if client == upstream {
                continue;
            }
            run_direction(client, upstream).await;
        }
    }
}

#[tokio::test]
async fn codex_discovers_and_calls_configured_gemini_models() {
    for upstream in [Wire::Chat, Wire::Responses, Wire::Gemini] {
        let mut config = single_wire_config(upstream);
        config.model_groups = None;
        config.endpoints[0].mappings[0].client_pattern = "gemini-dynamic-review".into();
        config.endpoints[0].mappings[0].upstream_model = "gemini-dynamic-review".into();
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            sse_ok(&[upstream.upstream_stream().as_str()]),
        );
        let engine = engine_with(config.normalized(), fake.clone());
        let (status, models) = call_get(
            &engine,
            loopback(),
            "/v1/models?client_version=0.149.1",
            vec![],
        )
        .await;
        assert_eq!(status, 200);
        let catalog: Value = serde_json::from_slice(&models).unwrap();
        let entry = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["slug"] == "gemini-dynamic-review")
            .expect("Gemini 应在 Codex 目录可见");
        assert_eq!(entry["visibility"], "list");
        let (status, body) = call(
            &engine,
            loopback(),
            "/v1/responses",
            vec![],
            Bytes::from_static(br#"{"model":"gemini-dynamic-review","input":"hi","stream":true}"#),
        )
        .await;
        assert_eq!(
            status,
            200,
            "{upstream:?}: {}",
            String::from_utf8_lossy(&body)
        );
        assert!(String::from_utf8_lossy(&body).contains("response.completed"));
        assert_eq!(fake.requests().len(), 1);
        let expected = match upstream {
            Wire::Chat => "/v1/chat/completions",
            Wire::Responses => "/v1/responses",
            Wire::Gemini => "/v1beta/models/gemini-dynamic-review:streamGenerateContent?alt=sse",
            Wire::Anthropic => unreachable!(),
        };
        assert_eq!(fake.requests()[0].path, expected);
    }
}

/// Gemini 的辅助操作没有会话语义,不进转换面:即便入口是别的协议也不得改写成
/// 会话请求,而是留在原生路径上(或按能力边界拒绝)。
#[tokio::test]
async fn gemini_auxiliary_operations_never_enter_the_conversion_surface() {
    for path in [
        "/v1beta/models/gemini-test:countTokens",
        "/v1beta/models/text-embedding-004:embedContent",
    ] {
        let fake = FakeTransport::new();
        let engine = engine_with(single_wire_config(Wire::Anthropic), fake.clone());
        let (status, _) = call(
            &engine,
            loopback(),
            path,
            vec![("content-type".into(), "application/json".into())],
            Bytes::from_static(b"{\"contents\":[]}"),
        )
        .await;
        // 没有 Gemini 原生入口 → 拒绝;绝不能把 countTokens 改写成会话请求发出去。
        assert_eq!(status, 400, "{path}");
        assert!(fake.requests().is_empty(), "{path} 不得触达上游");
    }
}

#[tokio::test]
async fn unsupported_instruction_blocks_are_rejected_before_dispatch() {
    for target in [Wire::Chat, Wire::Responses, Wire::Gemini] {
        let fake = FakeTransport::new();
        let engine = engine_with(single_wire_config(target), fake.clone());
        let mut body = Wire::Anthropic.body();
        body["messages"][1]["content"] = json!([{"type":"image", "text":"not text"}]);
        let (status, response) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![],
            Bytes::from(serde_json::to_vec(&body).unwrap()),
        )
        .await;
        assert_eq!(status, 400, "{target:?}");
        assert!(String::from_utf8_lossy(&response).contains("image"));
        assert!(fake.requests().is_empty());
    }
}

#[tokio::test]
async fn instruction_messages_survive_failover_between_translated_protocols() {
    for first in [Wire::Chat, Wire::Responses, Wire::Gemini] {
        for fallback in [Wire::Chat, Wire::Responses, Wire::Gemini] {
            if first == fallback {
                continue;
            }
            let mut config = single_wire_config(first);
            let mut backup = single_wire_config(fallback).endpoints.remove(0);
            backup.id = "b".into();
            backup.base_url = "https://b.example.com".into();
            backup.priority = 10;
            config.endpoints.push(backup);
            config.retry.session_sticky_retries = 0;
            let fake = FakeTransport::new();
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status: 503,
                    headers: vec![],
                    chunks: vec![],
                },
            );
            fake.push(
                "b.example.com",
                sse_ok(&[fallback.upstream_stream().as_str()]),
            );
            let engine = engine_with(config, fake.clone());
            let (status, body) = call(
                &engine,
                loopback(),
                "/v1/messages",
                vec![],
                Bytes::from(serde_json::to_vec(&Wire::Anthropic.body()).unwrap()),
            )
            .await;
            assert_eq!(status, 200, "{first:?} -> {fallback:?}");
            assert!(String::from_utf8_lossy(&body).contains(Wire::Anthropic.client_marker()));
            let sent = fake.requests();
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[1].host, "b.example.com");
            for request in sent {
                let body = String::from_utf8_lossy(&request.body);
                for marker in [
                    "instruction-top",
                    "instruction-system",
                    "instruction-developer",
                ] {
                    assert!(
                        body.contains(marker),
                        "{first:?} -> {fallback:?} lost {marker}"
                    );
                }
            }
        }
    }
}
