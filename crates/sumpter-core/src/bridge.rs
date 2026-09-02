//! OpenAI(chat/completions)与 OpenAI Responses(/v1/responses)→ Anthropic SSE 桥接。
//! 对齐 Swift `OpenAIBridge.swift` / `OpenAIResponsesBridge.swift`。
//!
//! 桥接范围:system/instructions、多轮文本、图片、**工具调用**(客户端 function
//! 工具的定义、调用与结果双向映射)、max_tokens、usage、stop_reason。
//! 服务端工具(`web_search_*` 等)不映射成 function,只在 websearch 透传模式下由
//! 目标协议的内建搜索工具承接。
//! 【Rust 修正】chat 桥的 stream_options 键用规范的 `include_usage`
//! (Swift 版漏了 CodingKeys 写成 `includeUsage`,上游忽略之;修正后 usage 真正生效)。

use serde_json::{Map, Value, json};

use crate::config::ProviderProtocol;
use crate::model_name::{self, ReasoningEffort};
use crate::routing::{RoutingMessage, RoutingRequest, inspector};

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

/// 校验会进入出站桥的 Anthropic 内容块。
///
/// 判据是「映射会不会丢掉客户端依赖的语义」,不是「形状是否完全对应」:
/// text / image / tool_use / tool_result 都有映射,放行;thinking 只是不回放,放行;
/// 其余块(`document` 等)承载的是模型必须看到的输入,丢了会让回答基于残缺上下文,
/// 所以显式拒绝而不是静默压平。
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
                    // 只支持能转成 data URL 或直链的图片源。
                    "image" => {
                        let source_type = object
                            .get("source")
                            .and_then(Value::as_object)
                            .and_then(|source| source.get("type"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if source_type != "base64" && source_type != "url" {
                            return Err(TranslationError::UnsupportedContentBlock(format!(
                                "image/{source_type}"
                            )));
                        }
                    }
                    "tool_use" => {
                        if !object.get("id").is_some_and(Value::is_string)
                            || !object.get("name").is_some_and(Value::is_string)
                        {
                            return Err(TranslationError::InvalidInput(format!(
                                "{path}[{index}] tool_use requires string id and name"
                            )));
                        }
                    }
                    "tool_result" => {
                        if !object.get("tool_use_id").is_some_and(Value::is_string) {
                            return Err(TranslationError::InvalidInput(format!(
                                "{path}[{index}].tool_use_id must be a string"
                            )));
                        }
                    }
                    // 历史推理不回放:两个目标协议都没有可回放它的输入字段,模型只是
                    // 少了自己上一轮的思考过程,对话本身仍然成立。
                    "thinking" | "redacted_thinking" => {}
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
    // 客户端工具已有 function 映射,不再整体拒绝。服务端工具由上游执行,只有
    // websearch 透传模式能用目标协议的内建搜索工具承接;其余情况丢弃会让模型
    // 以为自己有检索能力,却永远拿不到结果。
    for tool in &request.tools {
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
        if inspector::is_client_tool_type(tool_type) {
            if !tool.get("name").is_some_and(Value::is_string) {
                return Err(TranslationError::InvalidInput("tools[].name".into()));
            }
            continue;
        }
        if !(websearch && tool_type.starts_with("web_search")) {
            return Err(TranslationError::UnsupportedTool(tool_type.to_string()));
        }
    }
    // `thinking` 不再拒绝:见 validate_anthropic_content 对 thinking 块的说明。
    // 这类「能用但有损」的降级属于 Translated 路由的可见性问题,不是拒绝面。
    //
    // `output_config` 相反,它是结构化输出契约(json_schema)。丢掉它上游会回自由
    // 文本,客户端按 schema 解析必然失败,而且这种失败在客户端侧无法诊断。
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
            push_chat_message(&mut messages, message);
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
    // 客户端工具声明必须透传:Claude Code 的主对话恒带工具,丢掉它们会让上游
    // 只能回纯文本,客户端表现为「模型不听话」且没有任何错误可查。服务端工具
    // (`web_search_*` 等)不是 function,由下面的 websearch 分支单独注入。
    let tools = chat_tools_from_anthropic(request);
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
        if let Some(choice) = chat_tool_choice(request) {
            body.insert("tool_choice".into(), choice);
        }
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
        push_responses_input(&mut input, message);
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
    // 客户端工具与内建搜索工具共存:两者都要出现在同一个 `tools` 数组里。
    let mut tools = responses_tools_from_anthropic(request);
    if websearch {
        // 强制 tool_choice 各家支持不一,搜索工具本身不带 choice(prompt 就是搜索
        // 指令,模型会调)。
        tools.push(json!({"type": "web_search"}));
    }
    if !tools.is_empty() {
        let has_client_tools = tools
            .iter()
            .any(|tool| tool.get("type").and_then(Value::as_str) == Some("function"));
        body.insert("tools".into(), Value::Array(tools));
        if has_client_tools && let Some(choice) = responses_tool_choice(request) {
            body.insert("tool_choice".into(), choice);
        }
    }
    Value::Object(body)
}

// ---------------------------------------------------------------------------
// 工具与多模态映射(Anthropic → OpenAI)
// ---------------------------------------------------------------------------

/// Anthropic `input` 对象 → OpenAI `arguments` 字符串。
/// 两侧的形状差异只有这一处:Anthropic 用对象,OpenAI 用 JSON 文本。
fn tool_arguments_json(input: Option<&Value>) -> String {
    input
        .map(|value| serde_json::to_string(value).unwrap_or_else(|_| "{}".into()))
        .unwrap_or_else(|| "{}".into())
}

/// 只保留客户端自定义工具。服务端工具由上游执行,其 schema 与 function 不同,
/// 映射过去会被上游拒绝或当成同名 function 误调。
fn client_tools(request: &RoutingRequest) -> impl Iterator<Item = &Map<String, Value>> {
    request.tools.iter().filter(|tool| {
        inspector::is_client_tool_type(tool.get("type").and_then(Value::as_str).unwrap_or(""))
    })
}

/// Anthropic 工具定义 → OpenAI chat `{type:"function", function:{...}}`。
fn chat_tools_from_anthropic(request: &RoutingRequest) -> Vec<Value> {
    client_tools(request)
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let mut function = Map::new();
            function.insert("name".into(), json!(name));
            if let Some(description) = tool.get("description").and_then(Value::as_str) {
                function.insert("description".into(), json!(description));
            }
            function.insert("parameters".into(), tool_parameters(tool));
            Some(json!({"type": "function", "function": Value::Object(function)}))
        })
        .collect()
}

/// Anthropic 工具定义 → Responses 的扁平 function 形状(无 `function` 包装)。
fn responses_tools_from_anthropic(request: &RoutingRequest) -> Vec<Value> {
    client_tools(request)
        .filter_map(|tool| {
            let name = tool.get("name").and_then(Value::as_str)?;
            let mut item = Map::new();
            item.insert("type".into(), json!("function"));
            item.insert("name".into(), json!(name));
            if let Some(description) = tool.get("description").and_then(Value::as_str) {
                item.insert("description".into(), json!(description));
            }
            item.insert("parameters".into(), tool_parameters(tool));
            Some(Value::Object(item))
        })
        .collect()
}

/// `input_schema` 缺失或不是对象时补一个空 object schema:两个目标协议都要求
/// `parameters` 存在且是 JSON Schema 对象。
fn tool_parameters(tool: &Map<String, Value>) -> Value {
    tool.get("input_schema")
        .filter(|schema| schema.is_object())
        .cloned()
        .unwrap_or_else(|| json!({"type": "object", "properties": {}}))
}

/// Anthropic `tool_choice` → OpenAI chat `tool_choice`。
/// `any` 对应 OpenAI 的 `required`(两者都是「必须调一个工具」)。
fn chat_tool_choice(request: &RoutingRequest) -> Option<Value> {
    let choice = request.raw.get("tool_choice")?.as_object()?;
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => {
            let name = choice.get("name").and_then(Value::as_str)?;
            Some(json!({"type": "function", "function": {"name": name}}))
        }
        _ => None,
    }
}

/// Responses 的具名选择是扁平的 `{type:"function", name}`;其余档位同 chat。
fn responses_tool_choice(request: &RoutingRequest) -> Option<Value> {
    let choice = request.raw.get("tool_choice")?.as_object()?;
    match choice.get("type").and_then(Value::as_str)? {
        "auto" => Some(json!("auto")),
        "any" => Some(json!("required")),
        "none" => Some(json!("none")),
        "tool" => {
            let name = choice.get("name").and_then(Value::as_str)?;
            Some(json!({"type": "function", "name": name}))
        }
        _ => None,
    }
}

/// Anthropic image 块 → OpenAI chat `image_url` 部件;base64 源转 data URL。
fn chat_image_part(block: &Map<String, Value>) -> Option<Value> {
    Some(json!({"type": "image_url", "image_url": {"url": image_url(block)?}}))
}

fn image_url(block: &Map<String, Value>) -> Option<String> {
    let source = block.get("source")?.as_object()?;
    match source.get("type").and_then(Value::as_str)? {
        "base64" => {
            let media_type = source.get("media_type").and_then(Value::as_str)?;
            let data = source.get("data").and_then(Value::as_str)?;
            Some(format!("data:{media_type};base64,{data}"))
        }
        "url" => Some(source.get("url").and_then(Value::as_str)?.to_string()),
        _ => None,
    }
}

/// 一条 Anthropic 消息 → 一段 OpenAI chat 消息。
///
/// 两侧的消息边界不同:assistant 的工具调用与文本同属一条 OpenAI 消息
/// (`tool_calls` + `content`),而每个工具结果必须是独立的 `role:"tool"` 消息。
/// Anthropic 把 `tool_result` 放在 user 消息里,所以这里先按出现顺序展开全部
/// tool_result,再把剩余文本/图片作为一条 user 消息 —— 结果就是 tool 消息紧随
/// 触发它的 assistant 消息,满足 OpenAI 对顺序的要求。
fn push_chat_message(messages: &mut Vec<Value>, message: &RoutingMessage) {
    if let Some(text) = message.content.as_str() {
        if !text.is_empty() {
            messages.push(json!({"role": message.role, "content": text}));
        }
        return;
    }
    let Some(blocks) = message.content.as_array() else {
        return;
    };
    let mut text_parts: Vec<String> = Vec::new();
    let mut images: Vec<Value> = Vec::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    for block in blocks {
        let Some(object) = block.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str).unwrap_or("text") {
            "text" => {
                if let Some(text) = object.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text_parts.push(text.to_string());
                }
            }
            "image" => {
                if let Some(part) = chat_image_part(object) {
                    images.push(part);
                }
            }
            "tool_use" => {
                if let Some(id) = object.get("id").and_then(Value::as_str)
                    && let Some(name) = object.get("name").and_then(Value::as_str)
                {
                    tool_calls.push(json!({
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": name,
                            "arguments": tool_arguments_json(object.get("input")),
                        },
                    }));
                }
            }
            "tool_result" => {
                // `is_error` 在 OpenAI chat 上没有对应字段;错误文本本身已在
                // content 里,不额外加标记以免改写模型看到的工具输出。
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": object
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "content": object.get("content").map(flatten_text).unwrap_or_default(),
                }));
            }
            // thinking / redacted_thinking:OpenAI chat 没有可回放的推理输入字段,
            // 历史推理内容不重放(Responses 侧同样如此,它只接受自己签发的 item)。
            _ => {}
        }
    }
    let content = if images.is_empty() {
        let text = text_parts.join("\n");
        (!text.is_empty()).then(|| json!(text))
    } else {
        let mut parts: Vec<Value> = Vec::new();
        let text = text_parts.join("\n");
        if !text.is_empty() {
            parts.push(json!({"type": "text", "text": text}));
        }
        parts.extend(images);
        Some(Value::Array(parts))
    };
    if content.is_none() && tool_calls.is_empty() {
        return;
    }
    let mut out = Map::new();
    out.insert("role".into(), json!(message.role));
    // 只带 tool_calls 的 assistant 消息 content 为 null,这是 OpenAI 的规范形状。
    out.insert("content".into(), content.unwrap_or(Value::Null));
    if !tool_calls.is_empty() {
        out.insert("tool_calls".into(), Value::Array(tool_calls));
    }
    messages.push(Value::Object(out));
}

/// 一条 Anthropic 消息 → 一段 Responses input item。
///
/// Responses 的工具项是**顶层 item**(不带 role),所以工具调用与结果都从消息里
/// 提出来单独入列,文本/图片仍作为带 role 的消息项。
fn push_responses_input(input: &mut Vec<Value>, message: &RoutingMessage) {
    let content_type = if message.role == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    if let Some(text) = message.content.as_str() {
        if !text.is_empty() {
            input.push(json!({
                "role": message.role,
                "content": [{"type": content_type, "text": text}],
            }));
        }
        return;
    }
    let Some(blocks) = message.content.as_array() else {
        return;
    };
    let mut parts: Vec<Value> = Vec::new();
    let mut text_parts: Vec<String> = Vec::new();
    for block in blocks {
        let Some(object) = block.as_object() else {
            continue;
        };
        match object.get("type").and_then(Value::as_str).unwrap_or("text") {
            "text" => {
                if let Some(text) = object.get("text").and_then(Value::as_str)
                    && !text.is_empty()
                {
                    text_parts.push(text.to_string());
                }
            }
            "image" => {
                // Responses 的图片只在输入侧有意义;assistant 历史里的图片没有
                // 对应的 output 部件,跳过。
                if message.role == "user"
                    && let Some(url) = image_url(object)
                {
                    parts.push(json!({"type": "input_image", "image_url": url}));
                }
            }
            "tool_use" => {
                if let Some(id) = object.get("id").and_then(Value::as_str)
                    && let Some(name) = object.get("name").and_then(Value::as_str)
                {
                    flush_responses_text(
                        input,
                        &message.role,
                        content_type,
                        &mut text_parts,
                        &mut parts,
                    );
                    input.push(json!({
                        "type": "function_call",
                        "call_id": id,
                        "name": name,
                        "arguments": tool_arguments_json(object.get("input")),
                    }));
                }
            }
            "tool_result" => {
                flush_responses_text(
                    input,
                    &message.role,
                    content_type,
                    &mut text_parts,
                    &mut parts,
                );
                input.push(json!({
                    "type": "function_call_output",
                    "call_id": object
                        .get("tool_use_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default(),
                    "output": object.get("content").map(flatten_text).unwrap_or_default(),
                }));
            }
            _ => {}
        }
    }
    flush_responses_text(
        input,
        &message.role,
        content_type,
        &mut text_parts,
        &mut parts,
    );
}

/// 把已累积的文本/图片作为一条带 role 的消息项入列。
///
/// 工具项必须保持与文本的相对顺序:一次调用的 `function_call` 要排在解释它的
/// 文本之后,所以每遇到工具项就先 flush,而不是在消息末尾统一 flush。
fn flush_responses_text(
    input: &mut Vec<Value>,
    role: &str,
    content_type: &str,
    text_parts: &mut Vec<String>,
    parts: &mut Vec<Value>,
) {
    let text = std::mem::take(text_parts).join("\n");
    let mut content: Vec<Value> = Vec::new();
    if !text.is_empty() {
        content.push(json!({"type": content_type, "text": text}));
    }
    content.append(parts);
    if content.is_empty() {
        return;
    }
    input.push(json!({"role": role, "content": Value::Array(content)}));
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

fn content_block_stop_event(index: usize) -> SseEvent {
    SseEvent {
        event: "content_block_stop".into(),
        data: json!({"type": "content_block_stop", "index": index}),
    }
}

/// 工具参数增量。Anthropic 用 `input_json_delta` 传 JSON 文本片段,与 OpenAI 的
/// `function.arguments` 增量是一一对应的(两侧都不保证片段本身是合法 JSON)。
fn input_json_delta_event(partial_json: &str, index: usize) -> SseEvent {
    SseEvent {
        event: "content_block_delta".into(),
        data: json!({
            "type": "content_block_delta",
            "index": index,
            "delta": {"type": "input_json_delta", "partial_json": partial_json},
        }),
    }
}

/// 消息收尾,不含任何 `content_block_stop`:块的关闭由调用方按开启顺序自己负责
/// (有工具块时不止一个块要关)。
fn message_stop_events(stop_reason: &str, output_tokens: i64) -> Vec<SseEvent> {
    vec![
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

/// 一次 chat 工具调用的累积状态。
///
/// OpenAI 把 `id`/`name` 放在首个增量里,后续增量只带 `function.arguments` 片段,
/// 靠 `index` 关联;而 Anthropic 的 `content_block_start` 必须一次给出 name。所以
/// 这里攒到 name 齐了才开块,`emitted` 记住已发出的参数前缀,开块后一次补齐。
#[derive(Default)]
struct ChatToolCallState {
    id: String,
    name: String,
    arguments: String,
    emitted: usize,
    block_index: Option<usize>,
}

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
    /// 上游 `delta.tool_calls` 按 `index` 累积的调用状态。
    tool_calls: Vec<ChatToolCallState>,
    /// 下一个工具块的 Anthropic 块索引:文本块恒占 0,工具块从 1 起。
    next_tool_block_index: usize,
    /// 文本块是否已关闭。开第一个工具块前必须先关它 —— Anthropic 的内容块不
    /// 交错,客户端按开闭配对解析。
    text_block_closed: bool,
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
            tool_calls: Vec::new(),
            next_tool_block_index: 1,
            text_block_closed: false,
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
        // terminal 未定型时不渲染,**也不能**把 json_emitted 置位:feed() 会在每个
        // SSE block 之后调用这里,提前置位会让真正完成时直接返回空,非流式客户端
        // 拿到 0 字节响应。
        let rendered = match &self.terminal {
            BridgeTerminal::Failed(detail) => canonical_json(&error_json(detail)).into_bytes(),
            BridgeTerminal::Completed => canonical_json(&json!({
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": self.non_stream_content(),
                "stop_reason": self.stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
            }))
            .into_bytes(),
            BridgeTerminal::Pending => return Vec::new(),
        };
        self.json_emitted = true;
        rendered
    }

    /// 非流式响应体的 content 数组:文本块(若有)在前,工具调用块按上游 index 顺序
    /// 在后 —— 与流式下的块顺序一致。
    fn non_stream_content(&self) -> Value {
        let mut content: Vec<Value> = Vec::new();
        if !self.text.is_empty() {
            content.push(json!({"type": "text", "text": self.text}));
        }
        for state in &self.tool_calls {
            if state.name.is_empty() {
                continue;
            }
            content.push(json!({
                "type": "tool_use",
                "id": state.id,
                "name": state.name,
                // arguments 是上游拼出来的 JSON 文本;截断或畸形时给空对象,
                // 保留调用本身而不是整块丢掉(客户端至少知道模型想调什么)。
                "input": serde_json::from_str::<Value>(&state.arguments)
                    .unwrap_or_else(|_| json!({})),
            }));
        }
        Value::Array(content)
    }

    /// 累积上游 `delta.tool_calls`,并在 name 齐备后开块、补发参数增量。
    fn handle_tool_call_deltas(&mut self, choice: Option<&Value>, events: &mut Vec<SseEvent>) {
        let Some(deltas) = choice
            .and_then(|choice| choice.get("delta"))
            .and_then(|delta| delta.get("tool_calls"))
            .and_then(Value::as_array)
            .cloned()
        else {
            return;
        };
        for delta in &deltas {
            // 同一次调用的多个片段靠 `index` 关联;缺省按第 0 个处理。
            let slot = delta.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            while self.tool_calls.len() <= slot {
                self.tool_calls.push(ChatToolCallState::default());
            }
            if let Some(id) = delta.get("id").and_then(Value::as_str)
                && !id.is_empty()
            {
                self.tool_calls[slot].id = id.to_string();
            }
            if let Some(name) = delta.pointer("/function/name").and_then(Value::as_str)
                && !name.is_empty()
            {
                self.tool_calls[slot].name = name.to_string();
            }
            if let Some(fragment) = delta
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .filter(|fragment| !fragment.is_empty())
            {
                self.tool_calls[slot].arguments.push_str(fragment);
            }
            self.open_tool_block(slot, events);
            self.flush_tool_arguments(slot, events);
        }
    }

    /// name 齐备时开工具块。开块前先关文本块,保证内容块不交错。
    fn open_tool_block(&mut self, slot: usize, events: &mut Vec<SseEvent>) {
        if self.tool_calls[slot].block_index.is_some() || self.tool_calls[slot].name.is_empty() {
            return;
        }
        let index = self.next_tool_block_index;
        self.next_tool_block_index += 1;
        self.tool_calls[slot].block_index = Some(index);
        // 上游偶尔省略 id;合成一个稳定占位,否则客户端无法把结果配回调用。
        if self.tool_calls[slot].id.is_empty() {
            self.tool_calls[slot].id = format!("toolu_bridge_{slot}");
        }
        if !self.stream {
            return;
        }
        self.close_text_block(events);
        events.push(SseEvent {
            event: "content_block_start".into(),
            data: json!({
                "type": "content_block_start",
                "index": index,
                "content_block": {
                    "type": "tool_use",
                    "id": self.tool_calls[slot].id,
                    "name": self.tool_calls[slot].name,
                    "input": {},
                },
            }),
        });
    }

    /// 把尚未发出的参数尾巴作为一次 `input_json_delta` 发出。
    fn flush_tool_arguments(&mut self, slot: usize, events: &mut Vec<SseEvent>) {
        if !self.stream {
            return;
        }
        let Some(index) = self.tool_calls[slot].block_index else {
            return;
        };
        let state = &mut self.tool_calls[slot];
        if state.emitted >= state.arguments.len() {
            return;
        }
        let pending = state.arguments[state.emitted..].to_string();
        state.emitted = state.arguments.len();
        events.push(input_json_delta_event(&pending, index));
    }

    fn close_text_block(&mut self, events: &mut Vec<SseEvent>) {
        if self.text_block_closed {
            return;
        }
        self.text_block_closed = true;
        if self.stream {
            events.push(content_block_stop_event(0));
        }
    }

    /// 按开启顺序关闭所有工具块。
    fn close_tool_blocks(&self, events: &mut Vec<SseEvent>) {
        if !self.stream {
            return;
        }
        let mut indexes: Vec<usize> = self
            .tool_calls
            .iter()
            .filter_map(|state| state.block_index)
            .collect();
        indexes.sort_unstable();
        for index in indexes {
            events.push(content_block_stop_event(index));
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
        let mut events = Vec::new();
        if !self.started {
            events.extend(start_events(&self.message_id, &self.model));
            self.started = true;
        }
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
            // 文本块可能已因工具块提前关闭,那时不能再往 index 0 追加增量;
            // 引用仍留在 self.text 里,非流式响应看得到。
            if self.stream && !self.text_block_closed {
                events.push(text_delta_event(&lines, 0));
            }
        }
        self.close_text_block(&mut events);
        self.close_tool_blocks(&mut events);
        events.extend(message_stop_events(self.stop_reason, self.output_tokens));
        events
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
            // 工具块一开,文本块就关了;之后再来的文本仍计入非流式响应体,但不能
            // 作为 index 0 的增量重新发出(客户端已经收到过该块的 stop)。
            if !self.text_block_closed {
                events.push(text_delta_event(text, 0));
            }
        }
        self.handle_tool_call_deltas(choice, &mut events);
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

/// 一次 Responses function_call 的累积状态。
///
/// Responses 用 `item_id` 关联 `output_item.added` 与后续的
/// `function_call_arguments.delta`,而回给客户端的工具调用 id 是 `call_id`
/// (客户端要用它把 tool_result 配回来),两者不能混用。
#[derive(Default)]
struct ResponsesToolCallState {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    emitted: usize,
    block_index: Option<usize>,
}

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
    /// 文本块是否已发过 content_block_stop。开工具块前必须先关它 —— Anthropic 的
    /// 内容块不交错。
    text_block_closed: bool,
    /// 按 `item_id` 累积的 function_call 状态。
    tool_calls: Vec<ResponsesToolCallState>,
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
            text_block_closed: false,
            tool_calls: Vec::new(),
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
        // 同 chat 桥:terminal 未定型时不置位 json_emitted,否则完成时会渲染成空。
        let rendered = match &self.terminal {
            BridgeTerminal::Failed(detail) => canonical_json(&error_json(detail)).into_bytes(),
            BridgeTerminal::Completed => canonical_json(&json!({
                "id": self.message_id,
                "type": "message",
                "role": "assistant",
                "model": self.model,
                "content": self.non_stream_content(),
                "stop_reason": self.stop_reason,
                "stop_sequence": null,
                "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
            }))
            .into_bytes(),
            BridgeTerminal::Pending => return Vec::new(),
        };
        self.json_emitted = true;
        rendered
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

    fn close_text_block(&mut self, events: &mut Vec<SseEvent>) {
        let Some(index) = self.text_block_index else {
            return;
        };
        if self.text_block_closed {
            return;
        }
        self.text_block_closed = true;
        if self.stream {
            events.push(content_block_stop_event(index));
        }
    }

    /// `response.output_item.added` / `.done`(function_call)→ 开 tool_use 块。
    /// 同一个 `item_id` 只开一次,所以 added 与 done 都能安全调用(非流式聚合的
    /// 上游只发 done)。
    fn open_function_call_block(&mut self, item: &Value, events: &mut Vec<SseEvent>) {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if name.is_empty() {
            return;
        }
        let item_id = item
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !item_id.is_empty() && self.tool_calls.iter().any(|state| state.item_id == item_id) {
            return;
        }
        // 客户端用 call_id 把 tool_result 配回调用;上游省略时退到 item_id。
        let call_id = item
            .get("call_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .filter(|id| !id.is_empty())
            .or_else(|| (!item_id.is_empty()).then(|| item_id.clone()))
            .unwrap_or_else(|| format!("toolu_bridge_{}", self.tool_calls.len()));
        self.close_text_block(events);
        let index = self.next_block_index;
        self.next_block_index += 1;
        self.tool_calls.push(ResponsesToolCallState {
            item_id,
            call_id: call_id.clone(),
            name: name.clone(),
            // done 事件已带完整参数;added 通常是空串,靠后续 delta 补。
            arguments: item
                .get("arguments")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            emitted: 0,
            block_index: Some(index),
        });
        if self.stream {
            events.push(SseEvent {
                event: "content_block_start".into(),
                data: json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {
                        "type": "tool_use",
                        "id": call_id,
                        "name": name,
                        "input": {},
                    },
                }),
            });
        }
        let slot = self.tool_calls.len() - 1;
        self.flush_tool_arguments(slot, events);
    }

    fn push_function_call_arguments(&mut self, object: &Value, events: &mut Vec<SseEvent>) {
        let Some(fragment) = object
            .get("delta")
            .and_then(Value::as_str)
            .filter(|fragment| !fragment.is_empty())
        else {
            return;
        };
        let item_id = object
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // 上游省略 item_id 时落到最后一个已开的调用:Responses 的工具项是顺序发出的。
        let Some(slot) = self
            .tool_calls
            .iter()
            .rposition(|state| item_id.is_empty() || state.item_id == item_id)
        else {
            return;
        };
        self.tool_calls[slot].arguments.push_str(fragment);
        self.flush_tool_arguments(slot, events);
    }

    /// 把尚未发出的参数尾巴作为一次 `input_json_delta` 发出。
    fn flush_tool_arguments(&mut self, slot: usize, events: &mut Vec<SseEvent>) {
        if !self.stream {
            return;
        }
        let Some(index) = self.tool_calls[slot].block_index else {
            return;
        };
        let state = &mut self.tool_calls[slot];
        if state.emitted >= state.arguments.len() {
            return;
        }
        let pending = state.arguments[state.emitted..].to_string();
        state.emitted = state.arguments.len();
        events.push(input_json_delta_event(&pending, index));
    }

    /// 按开启顺序关闭所有工具块。
    fn close_tool_blocks(&self, events: &mut Vec<SseEvent>) {
        if !self.stream {
            return;
        }
        let mut indexes: Vec<usize> = self
            .tool_calls
            .iter()
            .filter_map(|state| state.block_index)
            .collect();
        indexes.sort_unstable();
        for index in indexes {
            events.push(content_block_stop_event(index));
        }
    }

    /// 非流式 content 数组:文本块(若有)在前,工具块按发出顺序在后。
    fn non_stream_content(&self) -> Value {
        let mut content: Vec<Value> = Vec::new();
        if !self.text.is_empty() {
            content.push(json!({"type": "text", "text": self.text}));
        }
        for state in &self.tool_calls {
            content.push(json!({
                "type": "tool_use",
                "id": state.call_id,
                "name": state.name,
                // 参数被截断时给空对象,保住调用本身。
                "input": serde_json::from_str::<Value>(&state.arguments)
                    .unwrap_or_else(|_| json!({})),
            }));
        }
        Value::Array(content)
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
            "response.output_item.added" | "response.output_item.done" => {
                let item = object.get("item").cloned().unwrap_or(Value::Null);
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if item_type == "function_call" {
                    self.ensure_message_started(&mut events);
                    self.open_function_call_block(&item, &mut events);
                } else if self.websearch
                    && item_type == "web_search_call"
                    && object.get("type").and_then(Value::as_str)
                        == Some("response.output_item.done")
                {
                    // 搜索块只在 done 时合成:added 阶段还没有 query 和结果。
                    self.ensure_message_started(&mut events);
                    self.synthesize_search_blocks(&item, &mut events);
                }
            }
            "response.function_call_arguments.delta" => {
                self.ensure_message_started(&mut events);
                self.push_function_call_arguments(&object, &mut events);
            }
            "response.output_text.delta" => {
                let Some(delta) = object.get("delta").and_then(Value::as_str) else {
                    return Vec::new();
                };
                if delta.is_empty() {
                    return Vec::new();
                }
                self.ensure_message_started(&mut events);
                self.output_tokens += 1;
                self.text.push_str(delta);
                // 文本块可能已因工具块关闭,那时不再发增量(客户端收过该块的 stop);
                // 文本仍进 self.text,非流式响应看得到。
                if !self.text_block_closed {
                    let index = self.open_text_block(&mut events);
                    events.push(text_delta_event(delta, index));
                }
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
                } else if !self.tool_calls.is_empty() {
                    // 模型请求调用工具时 Anthropic 的终止原因是 tool_use;回 end_turn
                    // 会让客户端以为轮次结束,不去执行工具。
                    "tool_use"
                } else {
                    "end_turn"
                };
                self.ensure_message_started(&mut events);
                // 有工具块时不强开一个空文本块;一个块都没有时才补(客户端总要收到
                // 至少一个 content_block)。
                if self.tool_calls.is_empty() || self.text_block_index.is_some() {
                    self.open_text_block(&mut events);
                }
                if !self.finished {
                    self.close_text_block(&mut events);
                    self.close_tool_blocks(&mut events);
                    events.extend(message_stop_events(self.stop_reason, self.output_tokens));
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
    fn chat_request_maps_client_tools_and_tool_turns() {
        let req = request(json!({
            "model": "m",
            "system": "sys",
            "messages": [
                {"role": "user", "content": "读一下 Cargo.toml"},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "好"},
                    {"type": "tool_use", "id": "toolu_1", "name": "Read",
                     "input": {"path": "Cargo.toml"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "toolu_1",
                     "content": [{"type": "text", "text": "[package]"}]},
                    {"type": "text", "text": "继续"}
                ]}
            ],
            "tools": [
                {"name": "Read", "description": "读文件",
                 "input_schema": {"type": "object", "properties": {"path": {"type": "string"}}}},
                {"name": "Custom", "type": "custom", "input_schema": {"type": "object"}},
                {"name": "web_search", "type": "web_search_20250305"}
            ],
            "tool_choice": {"type": "auto"},
        }));
        let body = make_openai_chat_body(&req, "up", None, false);

        // 客户端工具(含显式 `type:"custom"`)映射成 function;服务端 web_search
        // 由上游执行,不能当 function 声明。
        let tools = body["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "Read");
        assert_eq!(tools[0]["function"]["description"], "读文件");
        assert_eq!(
            tools[0]["function"]["parameters"]["properties"]["path"]["type"],
            "string"
        );
        assert_eq!(tools[1]["function"]["name"], "Custom");
        assert_eq!(body["tool_choice"], json!("auto"));

        let messages = body["messages"].as_array().unwrap();
        // system + user + assistant(text + tool_calls) + tool + user(剩余文本)
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[2]["role"], "assistant");
        assert_eq!(messages[2]["content"], "好");
        let calls = messages[2]["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["id"], "toolu_1");
        assert_eq!(calls[0]["type"], "function");
        assert_eq!(calls[0]["function"]["name"], "Read");
        // OpenAI 的 arguments 是 JSON 文本,不是对象。
        assert_eq!(
            calls[0]["function"]["arguments"],
            json!(r#"{"path":"Cargo.toml"}"#)
        );
        // tool_result 成为独立的 tool 消息,且排在触发它的 assistant 之后。
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "toolu_1");
        assert_eq!(messages[3]["content"], "[package]");
        // 同一条 Anthropic 消息里剩下的文本另起一条 user 消息。
        assert_eq!(messages[4]["role"], "user");
        assert_eq!(messages[4]["content"], "继续");
    }

    #[test]
    fn assistant_tool_call_only_message_keeps_null_content() {
        let req = request(json!({
            "model": "m",
            "messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {}}
            ]}],
            "tools": [{"name": "Read", "input_schema": {"type": "object"}}],
        }));
        let body = make_openai_chat_body(&req, "up", None, false);
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["content"], Value::Null);
        assert_eq!(
            messages[0]["tool_calls"][0]["function"]["arguments"],
            json!("{}")
        );
    }

    #[test]
    fn tool_choice_maps_to_each_protocol_shape() {
        let with_choice = |choice: Value| {
            request(json!({
                "model": "m",
                "messages": [{"role": "user", "content": "x"}],
                "tools": [{"name": "Read", "input_schema": {"type": "object"}}],
                "tool_choice": choice,
            }))
        };
        assert_eq!(
            make_openai_chat_body(&with_choice(json!({"type": "any"})), "up", None, false)["tool_choice"],
            json!("required")
        );
        assert_eq!(
            make_openai_chat_body(&with_choice(json!({"type": "none"})), "up", None, false)["tool_choice"],
            json!("none")
        );
        assert_eq!(
            make_openai_chat_body(
                &with_choice(json!({"type": "tool", "name": "Read"})),
                "up",
                None,
                false
            )["tool_choice"],
            json!({"type": "function", "function": {"name": "Read"}})
        );
        // Responses 的具名选择是扁平形状,没有 function 包装。
        assert_eq!(
            make_responses_body(
                &with_choice(json!({"type": "tool", "name": "Read"})),
                "up",
                None,
                false
            )["tool_choice"],
            json!({"type": "function", "name": "Read"})
        );
    }

    #[test]
    fn chat_request_maps_base64_image_to_data_url() {
        let req = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "这是什么"},
                {"type": "image",
                 "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}
            ]}],
        }));
        let body = make_openai_chat_body(&req, "up", None, false);
        let parts = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(parts[0], json!({"type": "text", "text": "这是什么"}));
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");
    }

    #[test]
    fn responses_request_maps_tool_turns_as_top_level_items() {
        let req = request(json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "text", "text": "先读文件"},
                    {"type": "tool_use", "id": "call_1", "name": "Read", "input": {"path": "a"}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "内容"}
                ]}
            ],
            "tools": [{"name": "Read", "input_schema": {"type": "object"}}],
        }));
        let body = make_responses_body(&req, "up", None, false);
        // Responses 工具定义是扁平的。
        assert_eq!(
            body["tools"],
            json!([{"type": "function", "name": "Read", "parameters": {"type": "object"}}])
        );
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 3);
        // 解释文本先入列,再是 function_call —— 顺序反了模型会看错因果。
        assert_eq!(input[0]["content"][0]["type"], "output_text");
        assert_eq!(input[0]["content"][0]["text"], "先读文件");
        assert_eq!(
            input[1],
            json!({"type": "function_call", "call_id": "call_1", "name": "Read",
                   "arguments": r#"{"path":"a"}"#})
        );
        assert_eq!(
            input[2],
            json!({"type": "function_call_output", "call_id": "call_1", "output": "内容"})
        );
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

        // 工具轮次现在有完整映射,不再是拒绝面。
        let tool_turn = request(json!({
            "model": "m",
            "messages": [
                {"role": "assistant", "content": [
                    {"type": "tool_use", "id": "call_1", "name": "Read", "input": {}}
                ]},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "call_1", "content": "result"}
                ]}
            ],
            "tools": [{"name": "Read", "input_schema": {"type": "object"}}],
        }));
        assert!(check_anthropic_to_openai_chat(&tool_turn, false).is_ok());
        assert!(check_anthropic_to_openai_responses(&tool_turn, false).is_ok());

        // thinking 只是不回放历史推理,对话仍成立 → 放行。
        let thinking = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
            "thinking": {"type": "enabled", "budget_tokens": 1024},
        }));
        assert!(check_anthropic_to_openai_responses(&thinking, false).is_ok());

        // output_config 是结构化输出契约,丢了客户端解析必然失败 → 仍然拒绝。
        let structured = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
            "output_config": {"format": {"type": "json_schema", "schema": {"type": "object"}}},
        }));
        assert!(
            check_anthropic_to_openai_chat(&structured, false)
                .unwrap_err()
                .to_string()
                .contains("output_config")
        );

        // 服务端工具在非 websearch 模式下无处承接 → 拒绝,不静默丢掉。
        let server_tool = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": "hello"}],
            "tools": [{"name": "web_search", "type": "web_search_20250305"}],
        }));
        assert!(
            check_anthropic_to_openai_chat(&server_tool, false)
                .unwrap_err()
                .to_string()
                .contains("web_search_20250305")
        );
        assert!(check_anthropic_to_openai_chat(&server_tool, true).is_ok());

        // 模型必须看到的输入块(document)丢了会让回答基于残缺上下文 → 拒绝。
        let document = request(json!({
            "model": "m",
            "messages": [{"role": "user", "content": [
                {"type": "document", "source": {"type": "base64", "data": "AAAA"}}
            ]}],
        }));
        assert!(
            check_anthropic_to_openai_chat(&document, false)
                .unwrap_err()
                .to_string()
                .contains("document")
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
    fn chat_bridge_maps_tool_calls_to_tool_use_blocks() {
        let mut bridge = OpenAiStreamBridge::new("msg_tc".into(), "up".into(), false, false);
        let mut chunk =
            |body: &str| parse_events(&bridge.feed(format!("data: {body}\n\n").as_bytes()));

        let opening = chunk(r#"{"choices":[{"delta":{"content":"我来读"}}]}"#);
        assert_eq!(opening[0].0, "message_start");
        assert_eq!(opening[1].0, "content_block_start");
        assert_eq!(opening[2].1["delta"]["text"], "我来读");

        // 首个工具增量带 id/name:先关文本块,再开 index 1 的 tool_use 块。
        let start = chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Read","arguments":""}}]}}]}"#,
        );
        assert_eq!(start[0].0, "content_block_stop");
        assert_eq!(start[0].1["index"], json!(0));
        assert_eq!(start[1].0, "content_block_start");
        assert_eq!(start[1].1["index"], json!(1));
        assert_eq!(start[1].1["content_block"]["type"], "tool_use");
        assert_eq!(start[1].1["content_block"]["id"], "call_a");
        assert_eq!(start[1].1["content_block"]["name"], "Read");

        // 后续增量只带 arguments 片段,逐片转成 input_json_delta。
        let first = chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\""}}]}}]}"#,
        );
        assert_eq!(first[0].0, "content_block_delta");
        assert_eq!(first[0].1["index"], json!(1));
        assert_eq!(first[0].1["delta"]["type"], "input_json_delta");
        assert_eq!(first[0].1["delta"]["partial_json"], "{\"path\"");
        let second = chunk(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"a\"}"}}]}}]}"#,
        );
        assert_eq!(second[0].1["delta"]["partial_json"], ":\"a\"}");

        assert!(chunk(r#"{"choices":[{"finish_reason":"tool_calls"}]}"#).is_empty());

        let done = parse_events(&bridge.feed(b"data: [DONE]\n\n"));
        // 文本块已经关过,收尾只关工具块。
        assert_eq!(done[0].0, "content_block_stop");
        assert_eq!(done[0].1["index"], json!(1));
        assert_eq!(done[1].1["delta"]["stop_reason"], "tool_use");
        assert_eq!(done[2].0, "message_stop");
    }

    #[test]
    fn chat_bridge_non_stream_emits_tool_use_content() {
        let mut bridge =
            OpenAiStreamBridge::new_with_stream("msg_ns".into(), "up".into(), false, false, false);
        let mut out = Vec::new();
        for body in [
            r#"{"choices":[{"delta":{"content":"读文件"}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"Read","arguments":"{\"path\":\"a\"}"}}]}}]}"#,
            r#"{"choices":[{"finish_reason":"tool_calls"}]}"#,
        ] {
            out.extend(bridge.feed(format!("data: {body}\n\n").as_bytes()));
        }
        out.extend(bridge.feed(b"data: [DONE]\n\n"));
        let body: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            body["content"][0],
            json!({"type": "text", "text": "读文件"})
        );
        assert_eq!(body["content"][1]["type"], "tool_use");
        assert_eq!(body["content"][1]["id"], "call_a");
        assert_eq!(body["content"][1]["name"], "Read");
        assert_eq!(body["content"][1]["input"], json!({"path": "a"}));
        assert_eq!(body["stop_reason"], "tool_use");
    }

    #[test]
    fn chat_bridge_handles_parallel_calls_and_truncated_arguments() {
        let mut bridge =
            OpenAiStreamBridge::new_with_stream("msg_p".into(), "up".into(), false, false, false);
        let mut out = Vec::new();
        for body in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c0","function":{"name":"Read","arguments":"{\"path\":\"a\"}"}},{"index":1,"id":"c1","function":{"name":"Grep","arguments":"{\"q\":"}}]}}]}"#,
            r#"{"choices":[{"finish_reason":"tool_calls"}]}"#,
        ] {
            out.extend(bridge.feed(format!("data: {body}\n\n").as_bytes()));
        }
        out.extend(bridge.feed(b"data: [DONE]\n\n"));
        let body: Value = serde_json::from_slice(&out).unwrap();
        let content = body["content"].as_array().unwrap();
        // 无文本时 content 只有两个工具块,按上游 index 顺序。
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["name"], "Read");
        assert_eq!(content[0]["input"], json!({"path": "a"}));
        // 参数被截断的调用仍保留,input 退化成空对象而不是丢掉整个调用。
        assert_eq!(content[1]["name"], "Grep");
        assert_eq!(content[1]["input"], json!({}));
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
    fn responses_bridge_maps_function_calls_to_tool_use_blocks() {
        let mut bridge = ResponsesStreamBridge::new("msg_rf".into(), "up".into(), false);
        let mut chunk =
            |body: &str| parse_events(&bridge.feed(format!("data: {body}\n\n").as_bytes()));

        let created = chunk(r#"{"type":"response.created","response":{"model":"codex-9"}}"#);
        assert_eq!(created[0].0, "message_start");
        assert_eq!(created[1].0, "content_block_start"); // 文本块 index 0
        let text = chunk(r#"{"type":"response.output_text.delta","delta":"先读"}"#);
        assert_eq!(text[0].1["delta"]["text"], "先读");

        // function_call 项:先关文本块,再开 tool_use 块,块 id 用 call_id。
        let added = chunk(
            r#"{"type":"response.output_item.added","item":{"type":"function_call","id":"fc_1","call_id":"call_x","name":"Read","arguments":""}}"#,
        );
        assert_eq!(added[0].0, "content_block_stop");
        assert_eq!(added[0].1["index"], json!(0));
        assert_eq!(added[1].0, "content_block_start");
        assert_eq!(added[1].1["index"], json!(1));
        assert_eq!(added[1].1["content_block"]["type"], "tool_use");
        assert_eq!(added[1].1["content_block"]["id"], "call_x");
        assert_eq!(added[1].1["content_block"]["name"], "Read");

        let args = chunk(
            r#"{"type":"response.function_call_arguments.delta","item_id":"fc_1","delta":"{\"path\":\"a\"}"}"#,
        );
        assert_eq!(args[0].1["delta"]["type"], "input_json_delta");
        assert_eq!(args[0].1["delta"]["partial_json"], "{\"path\":\"a\"}");

        // done 重复带同一个 item_id:不能再开一个块。
        assert!(
            chunk(
                r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc_1","call_id":"call_x","name":"Read","arguments":"{\"path\":\"a\"}"}}"#
            )
            .is_empty()
        );

        let done = chunk(
            r#"{"type":"response.completed","response":{"status":"completed","usage":{"output_tokens":7}}}"#,
        );
        // 文本块已关,收尾只关工具块;终止原因必须是 tool_use。
        assert_eq!(done[0].0, "content_block_stop");
        assert_eq!(done[0].1["index"], json!(1));
        assert_eq!(done[1].1["delta"]["stop_reason"], "tool_use");
        assert_eq!(done[1].1["usage"]["output_tokens"], json!(7));
        assert_eq!(done[2].0, "message_stop");
    }

    #[test]
    fn responses_bridge_non_stream_emits_tool_use_content() {
        let mut bridge =
            ResponsesStreamBridge::new_with_stream("msg_rn".into(), "up".into(), false, false);
        let mut out = Vec::new();
        for body in [
            r#"{"type":"response.created","response":{"model":"c"}}"#,
            r#"{"type":"response.output_text.delta","delta":"读"}"#,
            r#"{"type":"response.output_item.done","item":{"type":"function_call","id":"fc","call_id":"cx","name":"Read","arguments":"{\"path\":\"a\"}"}}"#,
            r#"{"type":"response.completed","response":{"status":"completed"}}"#,
        ] {
            out.extend(bridge.feed(format!("data: {body}\n\n").as_bytes()));
        }
        let body: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(body["content"][0], json!({"type": "text", "text": "读"}));
        assert_eq!(body["content"][1]["type"], "tool_use");
        assert_eq!(body["content"][1]["id"], "cx");
        assert_eq!(body["content"][1]["name"], "Read");
        assert_eq!(body["content"][1]["input"], json!({"path": "a"}));
        assert_eq!(body["stop_reason"], "tool_use");
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
