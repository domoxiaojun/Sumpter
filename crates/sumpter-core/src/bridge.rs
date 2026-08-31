//! OpenAI(chat/completions)与 OpenAI Responses(/v1/responses)→ Anthropic SSE 桥接。
//! 对齐 Swift `OpenAIBridge.swift` / `OpenAIResponsesBridge.swift`。
//!
//! 仅桥接文本对话(system/instructions、多轮文本、max_tokens、usage、stop_reason);
//! 工具调用不透传 —— 引擎对带 tools 的请求会跳过非 anthropic 协议入口。
//! 【Rust 修正】chat 桥的 stream_options 键用规范的 `include_usage`
//! (Swift 版漏了 CodingKeys 写成 `includeUsage`,上游忽略之;修正后 usage 真正生效)。

use serde_json::{Map, Value, json};

use crate::config::ProviderProtocol;
use crate::model_name::{self, ReasoningEffort};
use crate::routing::{RoutingRequest, inspector};

/// Translator 能力检查失败。
///
/// 路由器应在选择桥接候选前调用对应的 checker；转换器不能把无法表达的
/// reasoning、引用、工具或内容块静默压平/丢弃。错误文本不包含请求正文。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TranslationError {
    #[error("unsupported translation field `{0}`")]
    UnsupportedField(String),
    #[error("unsupported content block `{0}`")]
    UnsupportedContentBlock(String),
    #[error("unsupported tool type `{0}`")]
    UnsupportedTool(String),
    #[error("invalid translation input `{0}`")]
    InvalidInput(String),
}

/// 校验一个会被压平成文本的内容值。
///
/// 允许字符串、以及指定的文本块类型；数组中的任何未知块、非对象元素、
/// 非字符串 `text`、非空 citations/annotations 都会显式拒绝。
pub(crate) fn validate_text_content(
    content: Option<&Value>,
    path: &str,
    allowed_types: &[&str],
    allow_empty: bool,
) -> Result<(), TranslationError> {
    let Some(content) = content else {
        return Ok(());
    };
    match content {
        Value::Null => Ok(()),
        Value::String(_) => Ok(()),
        Value::Array(parts) => {
            for (index, part) in parts.iter().enumerate() {
                let Some(object) = part.as_object() else {
                    return Err(TranslationError::InvalidInput(format!(
                        "{path}[{index}] must be an object"
                    )));
                };
                let part_type = object.get("type").and_then(Value::as_str).unwrap_or("text");
                if !allowed_types.contains(&part_type) {
                    return Err(TranslationError::UnsupportedContentBlock(
                        part_type.to_string(),
                    ));
                }
                if let Some(annotation) = object.get("annotations")
                    && !annotation.is_null()
                    && annotation.as_array().is_none_or(|items| !items.is_empty())
                {
                    return Err(TranslationError::UnsupportedField(format!(
                        "{path}[{index}].annotations"
                    )));
                }
                if let Some(citations) = object.get("citations")
                    && !citations.is_null()
                    && citations.as_array().is_none_or(|items| !items.is_empty())
                {
                    return Err(TranslationError::UnsupportedField(format!(
                        "{path}[{index}].citations"
                    )));
                }
                if !object.get("text").is_some_and(Value::is_string) {
                    return Err(TranslationError::InvalidInput(format!(
                        "{path}[{index}].text must be a string"
                    )));
                }
            }
            if !allow_empty && parts.is_empty() {
                return Err(TranslationError::InvalidInput(format!(
                    "{path} must not be empty"
                )));
            }
            Ok(())
        }
        _ => Err(TranslationError::InvalidInput(format!(
            "{path} must be a string or text-block array"
        ))),
    }
}

fn validate_anthropic_content(content: Option<&Value>, path: &str) -> Result<(), TranslationError> {
    let Some(content) = content else {
        return Ok(());
    };
    match content {
        Value::Null | Value::String(_) => Ok(()),
        Value::Array(blocks) => {
            for (index, block) in blocks.iter().enumerate() {
                let object = block.as_object().ok_or_else(|| {
                    TranslationError::InvalidInput(format!("{path}[{index}] must be an object"))
                })?;
                let block_type = object.get("type").and_then(Value::as_str).unwrap_or("text");
                match block_type {
                    "text" => {
                        if !object.get("text").is_some_and(Value::is_string) {
                            return Err(TranslationError::InvalidInput(format!(
                                "{path}[{index}].text must be a string"
                            )));
                        }
                        if object
                            .get("citations")
                            .is_some_and(|value| !value.is_null() && value != &Value::Array(vec![]))
                        {
                            return Err(TranslationError::UnsupportedField(format!(
                                "{path}[{index}].citations"
                            )));
                        }
                    }
                    // 当前 OpenAI 出站 body 构造器只会把 tool_result 压成普通文本，
                    // 无法保留 call id、错误状态与工具结果语义，因此桥接阶段拒绝。
                    "tool_result" => {
                        return Err(TranslationError::UnsupportedContentBlock(
                            "tool_result".into(),
                        ));
                    }
                    other => {
                        return Err(TranslationError::UnsupportedContentBlock(other.to_string()));
                    }
                }
            }
            Ok(())
        }
        _ => Err(TranslationError::InvalidInput(format!(
            "{path} must be a string or content-block array"
        ))),
    }
}

fn validate_anthropic_request(
    request: &RoutingRequest,
    target: ProviderProtocol,
    websearch: bool,
) -> Result<(), TranslationError> {
    if let Some(system) = request.system.as_ref() {
        validate_text_content(Some(system), "system", &["text"], true)?;
    }
    for (index, message) in request.messages.iter().enumerate() {
        if message.role != "user" && message.role != "assistant" {
            return Err(TranslationError::UnsupportedField(format!(
                "messages[{index}].role={}",
                message.role
            )));
        }
        validate_anthropic_content(
            Some(&message.content),
            &format!("messages[{index}].content"),
        )?;
    }
    if !request.tools.is_empty() {
        if !websearch {
            return Err(TranslationError::UnsupportedField("tools".into()));
        }
        for tool in &request.tools {
            let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
            if !tool_type.starts_with("web_search") {
                return Err(TranslationError::UnsupportedTool(if tool_type.is_empty() {
                    "function".into()
                } else {
                    tool_type.into()
                }));
            }
        }
    }
    if request
        .raw
        .get("thinking")
        .is_some_and(|value| !value.is_null())
    {
        return Err(TranslationError::UnsupportedField("thinking".into()));
    }
    if request
        .raw
        .get("output_config")
        .is_some_and(|value| !value.is_null())
    {
        return Err(TranslationError::UnsupportedField("output_config".into()));
    }
    if target == ProviderProtocol::OpenAIResponses
        && request
            .raw
            .get("stop_sequences")
            .is_some_and(|value| value.as_array().is_some_and(|items| !items.is_empty()))
    {
        return Err(TranslationError::UnsupportedField("stop_sequences".into()));
    }
    Ok(())
}

/// 检查 Anthropic → OpenAI Chat 的首版安全转换范围。
pub fn check_anthropic_to_openai_chat(
    request: &RoutingRequest,
    websearch: bool,
) -> Result<(), TranslationError> {
    validate_anthropic_request(request, ProviderProtocol::OpenAI, websearch)
}

/// 检查 Anthropic → OpenAI Responses 的首版安全转换范围。
pub fn check_anthropic_to_openai_responses(
    request: &RoutingRequest,
    websearch: bool,
) -> Result<(), TranslationError> {
    validate_anthropic_request(request, ProviderProtocol::OpenAIResponses, websearch)
}

/// 按真实 TargetFormat 选择对应的请求能力检查器。
pub fn check_anthropic_translation(
    request: &RoutingRequest,
    target: ProviderProtocol,
    websearch: bool,
) -> Result<(), TranslationError> {
    match target {
        ProviderProtocol::Anthropic => Ok(()),
        ProviderProtocol::OpenAI => check_anthropic_to_openai_chat(request, websearch),
        ProviderProtocol::OpenAIResponses => {
            check_anthropic_to_openai_responses(request, websearch)
        }
    }
}

/// 带能力检查的 OpenAI Chat body 构造器；桥接调用方优先使用此接口。
pub fn try_make_openai_chat_body(
    request: &RoutingRequest,
    upstream_model: &str,
    reasoning_effort: Option<ReasoningEffort>,
    websearch: bool,
) -> Result<Value, TranslationError> {
    check_anthropic_to_openai_chat(request, websearch)?;
    Ok(make_openai_chat_body(
        request,
        upstream_model,
        reasoning_effort,
        websearch,
    ))
}

/// 带能力检查的 OpenAI Responses body 构造器；桥接调用方优先使用此接口。
pub fn try_make_responses_body(
    request: &RoutingRequest,
    upstream_model: &str,
    reasoning_effort: Option<ReasoningEffort>,
    websearch: bool,
) -> Result<Value, TranslationError> {
    check_anthropic_to_openai_responses(request, websearch)?;
    Ok(make_responses_body(
        request,
        upstream_model,
        reasoning_effort,
        websearch,
    ))
}

/// SSE 事件:`event: {event}\ndata: {canonical JSON}\n\n`(键按字母序,对齐 Swift sortedKeys)。
#[derive(Debug, Clone, PartialEq)]
pub struct SseEvent {
    pub event: String,
    pub data: Value,
}

impl SseEvent {
    pub fn to_bytes(&self) -> Vec<u8> {
        format!(
            "event: {}\ndata: {}\n\n",
            self.event,
            canonical_json(&self.data)
        )
        .into_bytes()
    }
}

/// 键按字母序递归序列化(Swift `canonicalJSONString` 的 sortedKeys 语义)。
pub fn canonical_json(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let body: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        canonical_json(&map[k])
                    )
                })
                .collect();
            format!("{{{}}}", body.join(","))
        }
        Value::Array(items) => {
            let body: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", body.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// 引擎侧统一的上游 SSE 桥接口:按 chunk 喂入上游 SSE,吐出 Anthropic SSE 字节。
/// 单请求任务内独占使用(&mut),无需并发原语。
/// Terminal state reported by a protocol bridge to the relay. `Pending` is
/// deliberately distinct from successful EOF: a translated stream must have
/// an actual protocol terminal before it can be counted as successful.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeTerminal {
    Pending,
    Completed,
    Failed(String),
}

pub trait SseBridge: Send {
    fn feed(&mut self, data: &[u8]) -> Vec<u8>;
    fn finish(&mut self) -> Vec<u8>;
    fn terminal(&self) -> BridgeTerminal {
        BridgeTerminal::Pending
    }
}

pub fn new_message_id() -> String {
    // Swift: "msg_oai_" + UUID 去横杠。这里用 32 位随机 hex,形状等价。
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!("msg_oai_{hex}")
}

// ---------------------------------------------------------------------------
// 请求侧
// ---------------------------------------------------------------------------

/// Anthropic 请求 → chat/completions 请求体。
/// system 文本 → 首条 system 消息;user/assistant 文本压平;总是流式。
/// 后缀 effort 优先于显式入参?——语义与 Swift 一致:入参(入口注入)优先,
/// 其次模型名后缀解析。
/// `websearch`:严格 WebSearch 用途且最终 TargetFormat 为 OpenAI Chat → 注入
/// `web_search_options:{}`(搜索由上游执行,桥只翻译)。
pub fn make_openai_chat_body(
    request: &RoutingRequest,
    upstream_model: &str,
    reasoning_effort: Option<ReasoningEffort>,
    websearch: bool,
) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let system = inspector::system_text(request);
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system}));
    }
    for message in &request.messages {
        if message.role == "user" || message.role == "assistant" {
            messages.push(json!({
                "role": message.role,
                "content": flatten_text(&message.content),
            }));
        }
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(upstream_model));
    body.insert("messages".into(), Value::Array(messages));
    body.insert("stream".into(), json!(true));
    body.insert("stream_options".into(), json!({"include_usage": true}));
    if let Some(max_tokens) = request.raw.get("max_tokens").and_then(Value::as_i64) {
        body.insert("max_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.raw.get("temperature").and_then(Value::as_f64) {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.raw.get("top_p").and_then(Value::as_f64) {
        body.insert("top_p".into(), json!(top_p));
    }
    // stop_sequences → stop(OpenAI 限最多 4 条,超出取前 4)。分类器 stage-1 靠
    // `</block>`/`</severity>` 截停,不透传会生成到 max_tokens 才停(白烧时间和 tokens)。
    // Responses API 无对应参数(spec §9 记为该协议限制)。
    if let Some(stops) = request.raw.get("stop_sequences").and_then(Value::as_array) {
        let stops: Vec<Value> = stops
            .iter()
            .filter(|s| s.as_str().is_some_and(|t| !t.is_empty()))
            .take(4)
            .cloned()
            .collect();
        if !stops.is_empty() {
            body.insert("stop".into(), Value::Array(stops));
        }
    }
    let effort = reasoning_effort.or_else(|| model_name::reasoning_effort(&request.model));
    if let Some(effort) = effort {
        body.insert("reasoning_effort".into(), json!(effort.as_str()));
    }
    if websearch {
        body.insert("web_search_options".into(), json!({}));
    }
    Value::Object(body)
}

/// Anthropic 请求 → Responses 请求体。system → instructions;
/// user 文本 → input_text,assistant 历史 → output_text;总是流式。
/// `websearch`:注入 Responses 内建 `web_search` 工具(上游执行搜索)。
pub fn make_responses_body(
    request: &RoutingRequest,
    upstream_model: &str,
    reasoning_effort: Option<ReasoningEffort>,
    websearch: bool,
) -> Value {
    let mut input: Vec<Value> = Vec::new();
    for message in &request.messages {
        if message.role != "user" && message.role != "assistant" {
            continue;
        }
        let text = flatten_text(&message.content);
        if text.is_empty() {
            continue;
        }
        let content_type = if message.role == "assistant" {
            "output_text"
        } else {
            "input_text"
        };
        input.push(json!({
            "role": message.role,
            "content": [{"type": content_type, "text": text}],
        }));
    }
    let mut body = Map::new();
    body.insert("model".into(), json!(upstream_model));
    body.insert("input".into(), Value::Array(input));
    body.insert("stream".into(), json!(true));
    let system = inspector::system_text(request);
    if !system.is_empty() {
        body.insert("instructions".into(), json!(system));
    }
    if let Some(max_tokens) = request.raw.get("max_tokens").and_then(Value::as_i64) {
        body.insert("max_output_tokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.raw.get("temperature").and_then(Value::as_f64) {
        body.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.raw.get("top_p").and_then(Value::as_f64) {
        body.insert("top_p".into(), json!(top_p));
    }
    let effort = reasoning_effort.or_else(|| model_name::reasoning_effort(&request.model));
    if let Some(effort) = effort {
        body.insert("reasoning".into(), json!({"effort": effort.as_str()}));
    }
    if websearch {
        // 强制 tool_choice 各家支持不一,先不注入(prompt 本身就是搜索指令,模型会调)。
        body.insert("tools".into(), json!([{"type": "web_search"}]));
    }
    Value::Object(body)
}

/// 文本压平:text 块直取、tool_result 递归,非空块以 \n 连接。
fn flatten_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .map(|block| {
                let Some(object) = block.as_object() else {
                    return String::new();
                };
                match object.get("type").and_then(Value::as_str) {
                    Some("text") => object
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    Some("tool_result") => {
                        object.get("content").map(flatten_text).unwrap_or_default()
                    }
                    _ => String::new(),
                }
            })
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// 响应侧共用件
// ---------------------------------------------------------------------------

fn message_start_event(message_id: &str, model: &str) -> SseEvent {
    SseEvent {
        event: "message_start".into(),
        data: json!({
            "type": "message_start",
            "message": {
                "id": message_id,
                "type": "message",
                "role": "assistant",
                "model": model,
                "content": [],
                "stop_reason": null,
                "stop_sequence": null,
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        }),
    }
}

fn text_block_start_event(index: usize) -> SseEvent {
    SseEvent {
        event: "content_block_start".into(),
        data: json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "text", "text": ""},
        }),
    }
}

fn start_events(message_id: &str, model: &str) -> Vec<SseEvent> {
    vec![
        message_start_event(message_id, model),
        text_block_start_event(0),
    ]
}

fn stop_events(stop_reason: &str, output_tokens: i64, text_index: usize) -> Vec<SseEvent> {
    vec![
        SseEvent {
            event: "content_block_stop".into(),
            data: json!({"type": "content_block_stop", "index": text_index}),
        },
        SseEvent {
            event: "message_delta".into(),
            data: json!({
                "type": "message_delta",
                "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                "usage": {"output_tokens": output_tokens},
            }),
        },
        SseEvent {
            event: "message_stop".into(),
            data: json!({"type": "message_stop"}),
        },
    ]
}

/// Anthropic error event used when a translated upstream response has an
/// explicit failure or ends before its protocol terminal.  Do not synthesize
/// `message_stop` in these cases: doing so makes an HTTP 200 failure look like
/// a successful model response to both the client and the runtime tracker.
fn error_event(detail: &str) -> SseEvent {
    SseEvent {
        event: "error".into(),
        data: json!({
            "type": "error",
            "error": {"type": "api_error", "message": detail},
        }),
    }
}

fn error_json(detail: &str) -> Value {
    json!({
        "type": "error",
        "error": {"type": "api_error", "message": detail},
    })
}

const MAX_SSE_BUFFER_BYTES: usize = 8 * 1024 * 1024;

fn text_delta_event(text: &str, index: usize) -> SseEvent {
    SseEvent {
        event: "content_block_delta".into(),
        data: json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "text_delta", "text": text},
        }),
    }
}

fn map_openai_stop_reason(finish_reason: &str) -> &'static str {
    match finish_reason {
        "stop" => "end_turn",
        "length" => "max_tokens",
        "tool_calls" => "tool_use",
        "content_filter" => "end_turn",
        _ => "end_turn",
    }
}

/// SSE 事件块缓冲:按 `\n\n` 切块,抽取 `data:` 行(多行以 \n 连接)。
pub(crate) struct SseBlockBuffer {
    buffer: String,
}

impl SseBlockBuffer {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
        }
    }

    pub(crate) fn push(&mut self, data: &[u8]) -> Vec<String> {
        let normalized = String::from_utf8_lossy(data).replace("\r\n", "\n");
        // A peer that never emits a blank line must not grow this buffer
        // without bound. Dropping the incomplete frame is preferable to an
        // OOM; subsequent complete frames remain observable.
        if normalized.len() > MAX_SSE_BUFFER_BYTES {
            self.buffer.clear();
            return Vec::new();
        }
        if self.buffer.len().saturating_add(normalized.len()) > MAX_SSE_BUFFER_BYTES {
            self.buffer.clear();
        }
        self.buffer.push_str(&normalized);
        let mut blocks = Vec::new();
        while let Some(pos) = self.buffer.find("\n\n") {
            let block: String = self.buffer[..pos].to_string();
            self.buffer.drain(..pos + 2);
            blocks.push(block);
        }
        blocks
    }

    pub(crate) fn drain_rest(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        Some(std::mem::take(&mut self.buffer))
    }
}

pub(crate) fn data_payload(block: &str) -> Option<String> {
    let payload: String = block
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            trimmed
                .strip_prefix("data:")
                .map(|rest| rest.trim().to_string())
        })
        .collect::<Vec<_>>()
        .join("\n");
    if payload.is_empty() {
        None
    } else {
        Some(payload)
    }
}

// ---------------------------------------------------------------------------
// chat/completions 流式桥
// ---------------------------------------------------------------------------

pub struct OpenAiStreamBridge {
    message_id: String,
    buffer: SseBlockBuffer,
    stream: bool,
    started: bool,
    finished: bool,
    terminal: BridgeTerminal,
    json_emitted: bool,
    model: String,
    text: String,
    input_tokens: i64,
    output_tokens: i64,
    stop_reason: &'static str,
    /// 请求侧声明过 stop_sequences:上游 finish_reason=stop 回映射为
    /// `stop_reason: "stop_sequence"`(chat API 不区分自然结束与 stop 命中,
    /// 按分类器 stage-1 的设计意图取 stop 命中;哪条序列命中不可知,置 null)。
    declared_stop_sequences: bool,
    /// websearch 透传模式:收集 url_citation 注解,流尾以引用清单追加到文本
    /// (chat 的搜索在服务端隐式发生,无独立结果事件可映射成结果块)。
    websearch: bool,
    /// (url, title) 去重收集。
    citations: Vec<(String, String)>,
}

impl OpenAiStreamBridge {
    /// `upstream_model`:message_start 的初始模型名(上游 chunk 带 model 时覆盖)。
    pub fn new(
        message_id: String,
        upstream_model: String,
        declared_stop_sequences: bool,
        websearch: bool,
    ) -> Self {
        Self {
            message_id,
            buffer: SseBlockBuffer::new(),
            stream: true,
            started: false,
            finished: false,
            terminal: BridgeTerminal::Pending,
            json_emitted: false,
            model: if upstream_model.is_empty() {
                "unknown".into()
            } else {
                upstream_model
            },
            text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: "end_turn",
            declared_stop_sequences,
            websearch,
            citations: Vec::new(),
        }
    }

    pub fn new_with_stream(
        message_id: String,
        upstream_model: String,
        declared_stop_sequences: bool,
        websearch: bool,
        stream: bool,
    ) -> Self {
        let mut bridge = Self::new(
            message_id,
            upstream_model,
            declared_stop_sequences,
            websearch,
        );
        bridge.stream = stream;
        bridge
    }

    fn render_non_stream(&mut self) -> Vec<u8> {
        if self.stream || self.json_emitted {
            return Vec::new();
        }
        self.json_emitted = true;
        match &self.terminal {
            BridgeTerminal::Failed(detail) => canonical_json(&error_json(detail)).into_bytes(),
            BridgeTerminal::Completed => canonical_json(&json!({
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": if self.text.is_empty() { json!([]) } else { json!([{"type":"text","text":self.text}]) },
                "stop_reason": self.stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
            })).into_bytes(),
            BridgeTerminal::Pending => Vec::new(),
        }
    }

    fn collect_citations(&mut self, choice: Option<&Value>) {
        let Some(annotations) = choice
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("annotations"))
            .and_then(Value::as_array)
        else {
            return;
        };
        for annotation in annotations {
            let cite = annotation.get("url_citation").unwrap_or(annotation);
            let Some(url) = cite.get("url").and_then(Value::as_str) else {
                continue;
            };
            if self.citations.iter().any(|(u, _)| u == url) {
                continue;
            }
            let title = cite
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.citations.push((url.to_string(), title.to_string()));
        }
    }

    fn complete(&mut self) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.terminal = BridgeTerminal::Completed;
        let mut citation_events = Vec::new();
        if self.websearch && !self.citations.is_empty() {
            let mut lines = String::from("\n\n引用:\n");
            for (url, title) in &self.citations {
                if title.is_empty() {
                    lines.push_str(&format!("- {url}\n"));
                } else {
                    lines.push_str(&format!("- {title} — {url}\n"));
                }
            }
            self.text.push_str(&lines);
            if self.stream {
                citation_events.push(text_delta_event(&lines, 0));
            }
        }
        if self.started {
            citation_events.extend(stop_events(self.stop_reason, self.output_tokens, 0));
            citation_events
        } else {
            let mut events = start_events(&self.message_id, &self.model);
            events.extend(citation_events);
            events
                .into_iter()
                .chain(stop_events(self.stop_reason, self.output_tokens, 0))
                .collect()
        }
    }

    fn fail(&mut self, detail: impl Into<String>) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        let detail = detail.into();
        self.finished = true;
        self.terminal = BridgeTerminal::Failed(detail.clone());
        let mut events = Vec::new();
        if !self.started {
            events.extend(start_events(&self.message_id, &self.model));
            self.started = true;
        }
        events.push(error_event(&detail));
        events
    }

    fn handle_block(&mut self, block: &str) -> Vec<SseEvent> {
        let Some(payload) = data_payload(block) else {
            return Vec::new();
        };
        if payload == "[DONE]" {
            return self.complete();
        }
        if let Ok(error) = serde_json::from_str::<Value>(&payload)
            && error.get("error").is_some()
        {
            return self.fail("upstream emitted chat stream error");
        }
        let Ok(chunk) = serde_json::from_str::<Value>(&payload) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        if !self.started {
            if let Some(model) = chunk.get("model").and_then(Value::as_str) {
                self.model = model.to_string();
            }
            events.extend(start_events(&self.message_id, &self.model));
            self.started = true;
        }
        let choice = chunk
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|c| c.first());
        if self.websearch {
            self.collect_citations(choice);
        }
        if let Some(text) = choice
            .and_then(|c| c.get("delta"))
            .and_then(|d| d.get("content"))
            .and_then(Value::as_str)
            && !text.is_empty()
        {
            self.output_tokens += 1;
            self.text.push_str(text);
            events.push(text_delta_event(text, 0));
        }
        if let Some(finish) = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(Value::as_str)
        {
            self.stop_reason = if finish == "stop" && self.declared_stop_sequences {
                "stop_sequence"
            } else {
                map_openai_stop_reason(finish)
            };
        }
        if let Some(tokens) = chunk
            .get("usage")
            .and_then(|u| u.get("completion_tokens"))
            .and_then(Value::as_i64)
        {
            self.output_tokens = tokens;
        }
        if let Some(tokens) = chunk
            .get("usage")
            .and_then(|u| u.get("prompt_tokens"))
            .and_then(Value::as_i64)
        {
            self.input_tokens = tokens;
        }
        events
    }
}

impl SseBridge for OpenAiStreamBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        for block in self.buffer.push(data) {
            for event in self.handle_block(&block) {
                if self.stream {
                    output.extend(event.to_bytes());
                }
            }
            output.extend(self.render_non_stream());
        }
        output
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        if let Some(rest) = self.buffer.drain_rest() {
            for event in self.handle_block(&rest) {
                if self.stream {
                    output.extend(event.to_bytes());
                }
            }
            output.extend(self.render_non_stream());
        }
        // EOF without `[DONE]` is a truncated upstream response. Emit an
        // explicit error event instead of manufacturing a successful tail.
        for event in self.fail("upstream response stream ended before [DONE]") {
            if self.stream {
                output.extend(event.to_bytes());
            }
        }
        output.extend(self.render_non_stream());
        output
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }
}

// ---------------------------------------------------------------------------
// Responses 流式桥
// ---------------------------------------------------------------------------

/// 事件识别以 data JSON 里的 `type` 字段为准(比依赖 `event:` 行更稳)。
pub struct ResponsesStreamBridge {
    message_id: String,
    buffer: SseBlockBuffer,
    stream: bool,
    message_started: bool,
    finished: bool,
    terminal: BridgeTerminal,
    json_emitted: bool,
    model: String,
    text: String,
    input_tokens: i64,
    output_tokens: i64,
    stop_reason: &'static str,
    /// websearch 透传模式:web_search_call 结果项合成 Anthropic 的
    /// `server_tool_use` + `web_search_tool_result` 块(content 允许空,结果以
    /// text+引用为主 —— 对齐 ccc 实测形状);文本块索引随之后移。
    websearch: bool,
    next_block_index: usize,
    text_block_index: Option<usize>,
}

impl ResponsesStreamBridge {
    /// `upstream_model`:message_start 的初始模型名(Responses 流早期事件常不带
    /// model,老实现恒显示 "unknown";response.created/completed 带 model 时仍覆盖)。
    pub fn new(message_id: String, upstream_model: String, websearch: bool) -> Self {
        Self {
            message_id,
            buffer: SseBlockBuffer::new(),
            stream: true,
            message_started: false,
            finished: false,
            terminal: BridgeTerminal::Pending,
            json_emitted: false,
            model: if upstream_model.is_empty() {
                "unknown".into()
            } else {
                upstream_model
            },
            text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: "end_turn",
            websearch,
            next_block_index: 0,
            text_block_index: None,
        }
    }

    pub fn new_with_stream(
        message_id: String,
        upstream_model: String,
        websearch: bool,
        stream: bool,
    ) -> Self {
        let mut bridge = Self::new(message_id, upstream_model, websearch);
        bridge.stream = stream;
        bridge
    }

    fn render_non_stream(&mut self) -> Vec<u8> {
        if self.stream || self.json_emitted {
            return Vec::new();
        }
        self.json_emitted = true;
        match &self.terminal {
            BridgeTerminal::Failed(detail) => canonical_json(&error_json(detail)).into_bytes(),
            BridgeTerminal::Completed => canonical_json(&json!({
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": if self.text.is_empty() { json!([]) } else { json!([{"type":"text","text":self.text}]) },
                "stop_reason": self.stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
            })).into_bytes(),
            BridgeTerminal::Pending => Vec::new(),
        }
    }

    fn fail(&mut self, detail: impl Into<String>) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        let detail = detail.into();
        self.finished = true;
        self.terminal = BridgeTerminal::Failed(detail.clone());
        let mut events = Vec::new();
        self.ensure_message_started(&mut events);
        events.push(error_event(&detail));
        events
    }

    fn ensure_message_started(&mut self, events: &mut Vec<SseEvent>) {
        if !self.message_started {
            events.push(message_start_event(&self.message_id, &self.model));
            self.message_started = true;
        }
    }

    fn open_text_block(&mut self, events: &mut Vec<SseEvent>) -> usize {
        if let Some(index) = self.text_block_index {
            return index;
        }
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.text_block_index = Some(index);
        events.push(text_block_start_event(index));
        index
    }

    /// websearch:`response.output_item.done`(web_search_call)→ 合成两个已完成块。
    /// 只在文本块尚未打开时合成(Responses 语义里搜索先于回答文本)。
    fn synthesize_search_blocks(&mut self, item: &Value, events: &mut Vec<SseEvent>) {
        if self.text_block_index.is_some() {
            return;
        }
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("srvtoolu_bridge")
            .to_string();
        let input = item
            .get("action")
            .and_then(|a| a.get("query"))
            .and_then(Value::as_str)
            .map(|q| json!({"query": q}))
            .unwrap_or_else(|| json!({}));
        let tool_index = self.next_block_index;
        self.next_block_index += 1;
        events.push(SseEvent {
            event: "content_block_start".into(),
            data: json!({
                "type": "content_block_start",
                "index": tool_index,
                "content_block": {
                    "type": "server_tool_use",
                    "id": id,
                    "name": "web_search",
                    "input": input,
                },
            }),
        });
        events.push(SseEvent {
            event: "content_block_stop".into(),
            data: json!({"type": "content_block_stop", "index": tool_index}),
        });
        let result_index = self.next_block_index;
        self.next_block_index += 1;
        events.push(SseEvent {
            event: "content_block_start".into(),
            data: json!({
                "type": "content_block_start",
                "index": result_index,
                "content_block": {
                    "type": "web_search_tool_result",
                    "tool_use_id": id,
                    "content": [],
                },
            }),
        });
        events.push(SseEvent {
            event: "content_block_stop".into(),
            data: json!({"type": "content_block_stop", "index": result_index}),
        });
    }

    fn handle_block(&mut self, block: &str) -> Vec<SseEvent> {
        let Some(payload) = data_payload(block) else {
            return Vec::new();
        };
        if payload == "[DONE]" {
            return Vec::new();
        }
        let Ok(object) = serde_json::from_str::<Value>(&payload) else {
            return Vec::new();
        };
        let mut events = Vec::new();
        match object.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.created" | "response.in_progress" => {
                if let Some(model) = object
                    .get("response")
                    .and_then(|r| r.get("model"))
                    .and_then(Value::as_str)
                {
                    self.model = model.to_string();
                }
                self.ensure_message_started(&mut events);
                if !self.websearch {
                    // 非搜索模式维持老行为面:message_start 后立即开文本块(index 0)。
                    self.open_text_block(&mut events);
                }
            }
            "response.output_item.done" => {
                if self.websearch
                    && object
                        .get("item")
                        .and_then(|i| i.get("type"))
                        .and_then(Value::as_str)
                        == Some("web_search_call")
                {
                    self.ensure_message_started(&mut events);
                    let item = object.get("item").cloned().unwrap_or(Value::Null);
                    self.synthesize_search_blocks(&item, &mut events);
                }
            }
            "response.output_text.delta" => {
                let Some(delta) = object.get("delta").and_then(Value::as_str) else {
                    return Vec::new();
                };
                if delta.is_empty() {
                    return Vec::new();
                }
                self.ensure_message_started(&mut events);
                let index = self.open_text_block(&mut events);
                self.output_tokens += 1;
                self.text.push_str(delta);
                events.push(text_delta_event(delta, index));
            }
            "response.completed" => {
                let response = object.get("response");
                let status = response
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                if status == "incomplete" || status == "failed" {
                    let detail = if status == "incomplete" {
                        response
                            .and_then(|r| r.pointer("/incomplete_details/reason"))
                            .and_then(Value::as_str)
                            .map(|reason| format!("upstream response incomplete ({reason})"))
                            .unwrap_or_else(|| "upstream response incomplete".into())
                    } else {
                        "upstream response failed".into()
                    };
                    return self.fail(detail);
                }
                if let Some(model) = response
                    .and_then(|r| r.get("model"))
                    .and_then(Value::as_str)
                {
                    self.model = model.to_string();
                }
                if let Some(tokens) = response
                    .and_then(|r| r.get("usage"))
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.output_tokens = tokens;
                }
                if let Some(tokens) = response
                    .and_then(|r| r.get("usage"))
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.input_tokens = tokens;
                }
                let status = response
                    .and_then(|r| r.get("status"))
                    .and_then(Value::as_str)
                    .unwrap_or("completed");
                let incomplete_reason = response
                    .and_then(|r| r.get("incomplete_details"))
                    .and_then(|d| d.get("reason"))
                    .and_then(Value::as_str);
                self.stop_reason = if status == "incomplete" {
                    if incomplete_reason == Some("max_output_tokens") {
                        "max_tokens"
                    } else {
                        "end_turn"
                    }
                } else {
                    "end_turn"
                };
                self.ensure_message_started(&mut events);
                let index = self.open_text_block(&mut events);
                if !self.finished {
                    events.extend(stop_events(self.stop_reason, self.output_tokens, index));
                    self.finished = true;
                    self.terminal = BridgeTerminal::Completed;
                }
            }
            "response.incomplete" => {
                let reason = object
                    .pointer("/response/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream response incomplete");
                events
                    .extend(self.fail(format!("upstream emitted response.incomplete ({reason})")));
            }
            "response.failed" | "error" => {
                events.extend(self.fail("upstream emitted response.failed"));
            }
            _ => {}
        }
        events
    }
}

impl SseBridge for ResponsesStreamBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        for block in self.buffer.push(data) {
            for event in self.handle_block(&block) {
                if self.stream {
                    output.extend(event.to_bytes());
                }
            }
            output.extend(self.render_non_stream());
        }
        output
    }

    /// 上游没发 completed(连接中断等)时兜底补齐结束事件,客户端不至于挂流。
    fn finish(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        if let Some(rest) = self.buffer.drain_rest() {
            for event in self.handle_block(&rest) {
                if self.stream {
                    output.extend(event.to_bytes());
                }
            }
            output.extend(self.render_non_stream());
        }
        let events = self.fail("upstream response stream ended before response.completed");
        for event in events {
            if self.stream {
                output.extend(event.to_bytes());
            }
        }
        output.extend(self.render_non_stream());
        output
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(value: Value) -> RoutingRequest {
        RoutingRequest::from_value(&value).unwrap()
    }

    fn parse_events(bytes: &[u8]) -> Vec<(String, Value)> {
        String::from_utf8_lossy(bytes)
            .split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .map(|block| {
                let mut event = String::new();
                let mut data = Value::Null;
                for line in block.lines() {
                    if let Some(rest) = line.strip_prefix("event: ") {
                        event = rest.to_string();
                    } else if let Some(rest) = line.strip_prefix("data: ") {
                        data = serde_json::from_str(rest).unwrap();
                    }
                }
                (event, data)
            })
            .collect()
    }

    #[test]
    fn chat_request_body_shape() {
        let req = request(json!({
            "model": "gpt-5.4(high)",
            "system": [{"type": "text", "text": "sys"}],
            "messages": [
                {"role": "user", "content": "hello"},
                {"role": "assistant", "content": [{"type": "text", "text": "hi"}]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t", "content": [{"type": "text", "text": "result"}]}
                ]}
            ],
            "max_tokens": 1024,
            "temperature": 0.5,
            "stop_sequences": ["</block>", "</severity>", "a", "b", "c", ""],
        }));
        let body = make_openai_chat_body(&req, "gpt-upstream", None, false);
        assert_eq!(body["model"], "gpt-upstream");
        assert_eq!(body["stream"], json!(true));
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
        assert_eq!(body["max_tokens"], json!(1024));
        assert_eq!(body["reasoning_effort"], json!("high")); // 模型名后缀解析
        // stop_sequences → stop:剔空串、限前 4 条(分类器 stage-1 截停的关键)。
        assert_eq!(body["stop"], json!(["</block>", "</severity>", "a", "b"]));
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 4); // system + 3
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[3]["content"], "result"); // tool_result 压平

        // 未声明 stop_sequences 时不产 stop 键。
        let plain = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "x"}],
        }));
        let body = make_openai_chat_body(&plain, "up", None, false);
        assert!(body.get("stop").is_none());
    }

    #[test]
    fn responses_request_body_shape() {
        let req = request(json!({
            "model": "codex-x",
            "system": "sys",
            "messages": [
                {"role": "user", "content": "q1"},
                {"role": "assistant", "content": "a1"},
            ],
            "max_tokens": 64,
        }));
        let body = make_responses_body(&req, "codex-up", Some(ReasoningEffort::Max), false);
        assert_eq!(body["model"], "codex-up");
        assert_eq!(body["instructions"], "sys");
        assert_eq!(body["max_output_tokens"], json!(64));
        assert_eq!(body["reasoning"], json!({"effort": "max"}));
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert!(body.get("messages").is_none());
    }

    #[test]
    fn translation_checker_rejects_lossy_anthropic_shapes() {
        let plain = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
        }));
        assert!(check_anthropic_to_openai_chat(&plain, false).is_ok());
        assert!(check_anthropic_to_openai_responses(&plain, false).is_ok());

        let tool_result = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": [{
                "type": "tool_result",
                "tool_use_id": "call_1",
                "content": "result"
            }]}],
        }));
        assert!(
            check_anthropic_to_openai_chat(&tool_result, false)
                .unwrap_err()
                .to_string()
                .contains("tool_result")
        );

        let thinking = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
            "thinking": {"type": "enabled", "budget_tokens": 1024},
        }));
        assert!(
            check_anthropic_to_openai_responses(&thinking, false)
                .unwrap_err()
                .to_string()
                .contains("thinking")
        );

        let stop_sequences = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
            "stop_sequences": ["STOP"],
        }));
        assert!(check_anthropic_to_openai_chat(&stop_sequences, false).is_ok());
        assert!(
            check_anthropic_to_openai_responses(&stop_sequences, false)
                .unwrap_err()
                .to_string()
                .contains("stop_sequences")
        );
    }

    #[test]
    fn openai_stream_bridged_incrementally_across_split_chunks() {
        let mut bridge =
            OpenAiStreamBridge::new("msg_oai_TEST".into(), String::new(), false, false);
        // 一个事件被拆成两次 feed(跨 chunk 拼装)。
        let part1 = b"data: {\"model\":\"m1\",\"choices\":[{\"delta\":{\"content\":\"He";
        let part2 = b"llo\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}\n\n";
        let out1 = bridge.feed(part1);
        assert!(out1.is_empty()); // 未见 \n\n,不输出
        let events = parse_events(&bridge.feed(part2));
        // message_start + content_block_start + 两个 delta
        assert_eq!(events[0].0, "message_start");
        assert_eq!(events[0].1["message"]["model"], "m1");
        assert_eq!(events[1].0, "content_block_start");
        assert_eq!(events[2].1["delta"]["text"], "Hello");
        assert_eq!(events[3].1["delta"]["text"], " world");

        let tail = bridge.feed(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}],\"usage\":{\"completion_tokens\":42}}\n\ndata: [DONE]\n\n",
        );
        let done = parse_events(&tail);
        assert_eq!(done[0].0, "content_block_stop");
        assert_eq!(done[1].1["delta"]["stop_reason"], "max_tokens");
        assert_eq!(done[1].1["usage"]["output_tokens"], json!(42));
        assert_eq!(done[2].0, "message_stop");
        assert!(bridge.finish().is_empty());
    }

    #[test]
    fn openai_stream_empty_body_is_reported_as_error() {
        let mut bridge = OpenAiStreamBridge::new("msg_oai_E".into(), String::new(), false, false);
        let events = parse_events(&bridge.finish());
        let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(names, vec!["message_start", "content_block_start", "error"]);
        assert_eq!(events[0].1["message"]["model"], "unknown");
    }

    #[test]
    fn responses_stream_full_cycle() {
        let mut bridge = ResponsesStreamBridge::new("msg_oai_R".into(), "seed-model".into(), false);
        let mut all = Vec::new();
        all.extend(bridge.feed(
            b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"codex-9\"}}\n\n",
        ));
        all.extend(
            bridge.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n"),
        );
        all.extend(bridge.feed(
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":7}}}\n\n",
        ));
        all.extend(bridge.finish());
        let events = parse_events(&all);
        let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop"
            ]
        );
        assert_eq!(events[0].1["message"]["model"], "codex-9");
        assert_eq!(events[4].1["usage"]["output_tokens"], json!(7));
        // completed 之后 finish 不重复补尾。
    }

    #[test]
    fn responses_incomplete_maps_max_tokens_and_dropped_stream_gets_fallback_tail() {
        let mut bridge = ResponsesStreamBridge::new("m".into(), "seed-model".into(), false);
        bridge.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n");
        let events = parse_events(
            &bridge.feed(b"data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n"),
        );
        assert!(events.iter().any(|(e, _)| e == "error"));

        // 上游中断没发 completed:finish 兜底补齐。
        let mut dropped = ResponsesStreamBridge::new("m2".into(), String::new(), false);
        dropped.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n");
        let tail = parse_events(&dropped.finish());
        let names: Vec<&str> = tail.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(names, vec!["error"]);
    }

    #[test]
    fn stop_mapping_and_seed_model() {
        // 声明过 stop_sequences:finish_reason=stop 回映射 stop_sequence;
        // 上游 chunk 不带 model 时 message_start 用构造种子而非 "unknown"。
        let mut bridge = OpenAiStreamBridge::new("m".into(), "qwen-up".into(), true, false);
        let first = parse_events(&bridge.feed(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"<block>false</block>\"},\"finish_reason\":null}]}\n\n",
        ));
        assert_eq!(first[0].1["message"]["model"], "qwen-up");
        let tail = parse_events(&bridge.feed(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ));
        let delta = tail.iter().find(|(e, _)| e == "message_delta").unwrap();
        assert_eq!(delta.1["delta"]["stop_reason"], "stop_sequence");

        // 未声明 stop:finish_reason=stop 维持 end_turn。
        let mut plain = OpenAiStreamBridge::new("m2".into(), String::new(), false, false);
        let tail = parse_events(&plain.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"));
        let delta = tail.iter().find(|(e, _)| e == "message_delta").unwrap();
        assert_eq!(delta.1["delta"]["stop_reason"], "end_turn");

        // responses 桥:流内始终不带 model → 种子模型生效(修 "unknown" 展示)。
        let mut responses = ResponsesStreamBridge::new("m3".into(), "qwen-up".into(), false);
        let out = parse_events(
            &responses.feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"x\"}\n\n"),
        );
        assert_eq!(out[0].1["message"]["model"], "qwen-up");
    }

    #[test]
    fn websearch_request_injection() {
        let req = request(json!({
            "model": "gpt-5.6-luna",
            "messages": [{"role": "user", "content": "Perform a web search for the query: x"}],
            "tools": [{"name": "web_search", "type": "web_search_20250305"}],
            "tool_choice": {"type": "tool", "name": "web_search"},
        }));
        // chat:注入 web_search_options(Anthropic 工具声明不透传——chat 桥本就不带 tools)。
        let body = make_openai_chat_body(&req, "up", None, true);
        assert_eq!(body["web_search_options"], json!({}));
        assert!(body.get("tools").is_none());
        // responses:注入内建 web_search 工具。
        let body = make_responses_body(&req, "up", None, true);
        assert_eq!(body["tools"], json!([{"type": "web_search"}]));
        // 非搜索模式两者都不注入。
        assert!(
            make_openai_chat_body(&req, "up", None, false)
                .get("web_search_options")
                .is_none()
        );
        assert!(
            make_responses_body(&req, "up", None, false)
                .get("tools")
                .is_none()
        );
    }

    #[test]
    fn chat_bridge_websearch_appends_citation_annotations() {
        let mut bridge = OpenAiStreamBridge::new("m".into(), "up".into(), false, true);
        let mut all = Vec::new();
        all.extend(bridge.feed(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Tokio 1.49 released.\",\"annotations\":[{\"type\":\"url_citation\",\"url_citation\":{\"url\":\"https://github.com/tokio-rs/tokio\",\"title\":\"tokio releases\"}}]}}]}\n\n",
        ));
        all.extend(bridge.feed(
            b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ));
        all.extend(bridge.finish());
        let text: String = parse_events(&all)
            .iter()
            .filter(|(e, _)| e == "content_block_delta")
            .filter_map(|(_, d)| d["delta"]["text"].as_str().map(str::to_string))
            .collect();
        assert!(text.contains("Tokio 1.49 released."));
        assert!(text.contains("引用:"));
        assert!(text.contains("tokio releases — https://github.com/tokio-rs/tokio"));
    }

    #[test]
    fn responses_bridge_websearch_synthesizes_tool_blocks() {
        let mut bridge = ResponsesStreamBridge::new("m".into(), "gpt-5.6-luna".into(), true);
        let mut all = Vec::new();
        all.extend(bridge.feed(
            b"data: {\"type\":\"response.created\",\"response\":{\"model\":\"gpt-5.6-luna\"}}\n\n",
        ));
        all.extend(bridge.feed(
            b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"web_search_call\",\"id\":\"ws_1\",\"status\":\"completed\",\"action\":{\"type\":\"search\",\"query\":\"tokio 1.49\"}}}\n\n",
        ));
        all.extend(
            bridge
                .feed(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Answer.\"}\n\n"),
        );
        all.extend(bridge.feed(
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":2}}}\n\n",
        ));
        all.extend(bridge.finish());
        let events = parse_events(&all);
        let names: Vec<&str> = events.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start", // 0: server_tool_use
                "content_block_stop",
                "content_block_start", // 1: web_search_tool_result(content 空,对齐 ccc 形状)
                "content_block_stop",
                "content_block_start", // 2: text
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        assert_eq!(events[1].1["content_block"]["type"], "server_tool_use");
        assert_eq!(
            events[1].1["content_block"]["input"],
            json!({"query": "tokio 1.49"})
        );
        assert_eq!(
            events[3].1["content_block"]["type"],
            "web_search_tool_result"
        );
        assert_eq!(events[3].1["content_block"]["tool_use_id"], "ws_1");
        assert_eq!(events[3].1["content_block"]["content"], json!([]));
        assert_eq!(events[5].1["index"], json!(2)); // 文本块顺延到 2
        assert_eq!(events[6].1["delta"]["text"], "Answer.");
        assert_eq!(events[7].1["index"], json!(2));
    }

    #[test]
    fn sse_bytes_canonical_key_order() {
        let event = SseEvent {
            event: "content_block_delta".into(),
            data: json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "a/b"}}),
        };
        let text = String::from_utf8(event.to_bytes()).unwrap();
        // 键按字母序;斜杠不转义。
        assert_eq!(
            text,
            "event: content_block_delta\ndata: {\"delta\":{\"text\":\"a/b\",\"type\":\"text_delta\"},\"index\":0,\"type\":\"content_block_delta\"}\n\n"
        );
    }
}
