//! Gemini Developer API(`generateContent` / `streamGenerateContent`)与会话协议的
//! 互转。
//!
//! 方向:
//! - 入站 Gemini 会话请求 → Anthropic Messages 归一化(路由用宽松转换;
//!   Translated 候选先过 [`check_gemini_to_anthropic`])。
//! - 出站 Anthropic 中间格式 → Gemini 请求体([`try_make_gemini_body`])。
//! - 上游 Gemini SSE → Anthropic SSE([`GeminiStreamBridge`])。
//! - Anthropic SSE → 客户端 Gemini SSE / 非流式 JSON([`GeminiClientBridge`])。
//!
//! 只有会话操作走这里;`countTokens` / `embedContent` 仍是原生透传 —— 它们没有会话
//! 语义,混进来会把「转换」变成对未知契约的猜测。
//!
//! **thoughtSignature** 是 Gemini 侧的原生字段,必须在多轮工具调用里原样回传。本模块
//! 不伪造签名:入站转换把签名留在原始 part 里交由调用方(engine 的回放状态)决定是否
//! 复用,出站转换只在拿得到真实签名时才写入。

use serde_json::{Map, Value, json};

use crate::bridge::{
    BridgeTerminal, SseBlockBuffer, SseBridge, SseEvent, TranslationError,
    content_block_stop_event, data_payload, error_event, input_json_delta_event,
    message_start_event, message_stop_events, text_block_start_event, text_delta_event,
};
use crate::model_name::ReasoningEffort;
use crate::routing::RoutingRequest;

/// Gemini 会话操作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiOperation {
    Generate,
    StreamGenerate,
    /// `countTokens` / `embedContent` 等辅助操作:不参与会话转换。
    Auxiliary,
}

/// 从路径判定 Gemini 操作。`model` 由路径给出(Google 的 REST 形状把模型放在
/// `/v1beta/models/{model}:{action}`),请求体里没有。
pub fn gemini_operation(path: &str) -> GeminiOperation {
    let action = path.rsplit(':').next().unwrap_or_default();
    match action {
        "generateContent" => GeminiOperation::Generate,
        "streamGenerateContent" => GeminiOperation::StreamGenerate,
        _ => GeminiOperation::Auxiliary,
    }
}

/// 模型名去掉 `models/` 前缀:`models/gemini-2.5-pro` → `gemini-2.5-pro`。
fn bare_model(model: &str) -> &str {
    model.strip_prefix("models/").unwrap_or(model)
}

// ---------------------------------------------------------------------------
// 请求:Gemini → Anthropic
// ---------------------------------------------------------------------------

/// Gemini `parts` → Anthropic 内容块。
///
/// `thought` 为 true 的 part 是模型的思考摘要,没有可回放给 Anthropic 的签名格式,
/// 转换时跳过(与 bridge.rs 对历史 thinking 块的处理一致:不回放,但不是拒绝面)。
fn parts_to_blocks(parts: &[Value], path: &str) -> Result<Vec<Value>, TranslationError> {
    let mut blocks = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let part_path = format!("{path}[{index}]");
        let object = part.as_object().ok_or_else(|| {
            TranslationError::InvalidInput(format!("{part_path} must be an object"))
        })?;
        if object.get("thought").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(text) = object.get("text").and_then(Value::as_str) {
            if !text.is_empty() {
                blocks.push(json!({"type": "text", "text": text}));
            }
            continue;
        }
        if let Some(inline) = object.get("inlineData").and_then(Value::as_object) {
            let mime = inline
                .get("mimeType")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    TranslationError::InvalidInput(format!("{part_path}.mimeType is required"))
                })?;
            let data = inline.get("data").and_then(Value::as_str).ok_or_else(|| {
                TranslationError::InvalidInput(format!("{part_path}.data is required"))
            })?;
            blocks.push(json!({
                "type": "image",
                "source": {"type": "base64", "media_type": mime, "data": data},
            }));
            continue;
        }
        if let Some(call) = object.get("functionCall").and_then(Value::as_object) {
            let name = call.get("name").and_then(Value::as_str).ok_or_else(|| {
                TranslationError::InvalidInput(format!("{part_path}.name is required"))
            })?;
            blocks.push(json!({
                "type": "tool_use",
                "id": call.get("id").and_then(Value::as_str).unwrap_or(name),
                "name": name,
                "input": call.get("args").cloned().unwrap_or_else(|| json!({})),
            }));
            continue;
        }
        if let Some(response) = object.get("functionResponse").and_then(Value::as_object) {
            let name = response
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    TranslationError::InvalidInput(format!("{part_path}.name is required"))
                })?;
            let content = response
                .get("response")
                .map(|value| {
                    // Gemini 的 response 是对象(常见 `{"output": ...}`);Anthropic 的
                    // tool_result content 收字符串或块数组。
                    value
                        .get("output")
                        .or_else(|| value.get("content"))
                        .cloned()
                        .unwrap_or_else(|| value.clone())
                })
                .unwrap_or(Value::Null);
            blocks.push(json!({
                "type": "tool_result",
                "tool_use_id": response.get("id").and_then(Value::as_str).unwrap_or(name),
                "content": content,
            }));
            continue;
        }
        // `fileData` 是 provider 侧的文件引用,目标协议没有等价物,且转换器不该替
        // 用户取回内容;其余未知 part 同理。
        let kind = if object.contains_key("fileData") {
            "fileData".to_string()
        } else {
            object
                .keys()
                .next()
                .cloned()
                .unwrap_or_else(|| "unknown".into())
        };
        return Err(TranslationError::UnsupportedContentBlock(kind));
    }
    Ok(blocks)
}

/// Gemini 请求 → Anthropic Messages body(宽松归一化,仅用于路由与出站构造)。
///
/// `model` 由路径给出;`systemInstruction` → system;`contents` → messages
/// (assistant→`model` 换算成 Anthropic 的 `assistant`);functionDeclarations →
/// tools;generationConfig → max_tokens / temperature / top_p / stop_sequences。
pub fn gemini_to_anthropic(body: &Value, model: &str) -> Result<Value, TranslationError> {
    let object = body
        .as_object()
        .ok_or_else(|| TranslationError::InvalidInput("body is not an object".into()))?;
    let mut messages: Vec<Value> = Vec::new();
    let contents = object
        .get("contents")
        .and_then(Value::as_array)
        .ok_or_else(|| TranslationError::InvalidInput("contents is required".into()))?;
    for (index, content) in contents.iter().enumerate() {
        let content = content.as_object().ok_or_else(|| {
            TranslationError::InvalidInput(format!("contents[{index}] must be an object"))
        })?;
        let role = match content.get("role").and_then(Value::as_str) {
            Some("model") => "assistant",
            // Gemini 允许省略 role,按 user 处理。
            _ => "user",
        };
        let parts = content
            .get("parts")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TranslationError::InvalidInput(format!("contents[{index}].parts is required"))
            })?;
        let blocks = parts_to_blocks(parts, &format!("contents[{index}].parts"))?;
        if blocks.is_empty() {
            continue;
        }
        messages.push(json!({"role": role, "content": blocks}));
    }
    if messages.is_empty() {
        return Err(TranslationError::InvalidInput(
            "no convertible contents".into(),
        ));
    }

    let mut out = Map::new();
    out.insert("model".into(), json!(bare_model(model)));
    out.insert("messages".into(), Value::Array(messages));
    out.insert("stream".into(), json!(true));
    out.insert("max_tokens".into(), json!(generation_max_tokens(object)));

    if let Some(system) = object.get("systemInstruction") {
        if let Some(text) = system_instruction_text(system)? {
            out.insert("system".into(), json!(text));
        }
    }
    let config = object.get("generationConfig").and_then(Value::as_object);
    if let Some(config) = config {
        for (key, target) in [("temperature", "temperature"), ("topP", "top_p")] {
            if let Some(value) = config.get(key).and_then(Value::as_f64) {
                out.insert(target.into(), json!(value));
            }
        }
        if let Some(stops) = config.get("stopSequences").and_then(Value::as_array) {
            let stops: Vec<Value> = stops
                .iter()
                .filter(|stop| stop.as_str().is_some_and(|text| !text.is_empty()))
                .cloned()
                .collect();
            if !stops.is_empty() {
                out.insert("stop_sequences".into(), Value::Array(stops));
            }
        }
        if let Some(schema) = gemini_output_schema(config)? {
            out.insert(
                "output_config".into(),
                json!({"format": {"type": "json_schema", "schema": schema}}),
            );
        }
    }
    let tools = gemini_tools_to_anthropic(object.get("tools"))?;
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
    }
    Ok(Value::Object(out))
}

fn generation_max_tokens(object: &Map<String, Value>) -> i64 {
    object
        .get("generationConfig")
        .and_then(|config| config.get("maxOutputTokens"))
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(32000)
}

fn system_instruction_text(system: &Value) -> Result<Option<String>, TranslationError> {
    if system.is_null() {
        return Ok(None);
    }
    let parts = system
        .get("parts")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TranslationError::InvalidInput("systemInstruction.parts is required".into())
        })?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok((!text.is_empty()).then_some(text))
}

/// Gemini 的结构化输出:`responseMimeType: application/json` 搭配
/// `responseJsonSchema`(新形状)或 `responseSchema`(OpenAPI 子集)。
fn gemini_output_schema(config: &Map<String, Value>) -> Result<Option<Value>, TranslationError> {
    let mime = config.get("responseMimeType").and_then(Value::as_str);
    let schema = config
        .get("responseJsonSchema")
        .or_else(|| config.get("responseSchema"))
        .filter(|value| !value.is_null());
    match (mime, schema) {
        (None, None) => Ok(None),
        (Some("application/json"), Some(schema)) => {
            if !schema.is_object() {
                return Err(TranslationError::InvalidInput(
                    "responseJsonSchema must be an object".into(),
                ));
            }
            Ok(Some(schema.clone()))
        }
        // 声明了 JSON 契约却给不出 schema,或给了 schema 却没声明 JSON —— 两种都
        // 说明客户端期望的解析契约无法被 Anthropic 侧表达,不能静默降级成自由文本。
        (Some("application/json"), None) => Err(TranslationError::UnsupportedField(
            "generationConfig.responseMimeType without a schema".into(),
        )),
        (Some(other), _) => Err(TranslationError::UnsupportedField(format!(
            "generationConfig.responseMimeType={other}"
        ))),
        (None, Some(_)) => Err(TranslationError::UnsupportedField(
            "generationConfig.responseSchema without responseMimeType".into(),
        )),
    }
}

fn gemini_tools_to_anthropic(tools: Option<&Value>) -> Result<Vec<Value>, TranslationError> {
    let Some(tools) = tools.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let tools = tools
        .as_array()
        .ok_or_else(|| TranslationError::InvalidInput("tools must be an array".into()))?;
    let mut out = Vec::new();
    for (index, tool) in tools.iter().enumerate() {
        let declarations = tool
            .get("functionDeclarations")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TranslationError::UnsupportedTool(format!(
                    "tools[{index}] without functionDeclarations"
                ))
            })?;
        for (decl_index, declaration) in declarations.iter().enumerate() {
            let path = format!("tools[{index}].functionDeclarations[{decl_index}]");
            let declaration = declaration.as_object().ok_or_else(|| {
                TranslationError::InvalidInput(format!("{path} must be an object"))
            })?;
            let name = declaration
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .ok_or_else(|| {
                    TranslationError::InvalidInput(format!("{path}.name is required"))
                })?;
            let mut entry = Map::new();
            entry.insert("name".into(), json!(name));
            if let Some(description) = declaration.get("description").and_then(Value::as_str) {
                entry.insert("description".into(), json!(description));
            }
            // `parameters` 是 OpenAPI 子集,`parametersJsonSchema` 是完整 JSON Schema;
            // Anthropic 的 input_schema 收完整 JSON Schema,所以后者优先。
            let schema = declaration
                .get("parametersJsonSchema")
                .or_else(|| declaration.get("parameters"))
                .filter(|value| !value.is_null());
            if let Some(schema) = schema {
                if !schema.is_object() {
                    return Err(TranslationError::InvalidInput(format!(
                        "{path}.parameters must be an object"
                    )));
                }
                entry.insert("input_schema".into(), normalize_object_schema(schema));
            }
            out.push(Value::Object(entry));
        }
    }
    Ok(out)
}

/// 工具入参为空对象时补上 `{"type":"object"}`:Anthropic 要求 input_schema 是对象
/// schema,缺 type 会被上游拒绝。
fn normalize_object_schema(schema: &Value) -> Value {
    if schema.as_object().is_some_and(|object| object.is_empty()) {
        return json!({"type": "object"});
    }
    schema.clone()
}

/// Gemini 会话请求 → Anthropic 的可转换能力检查。与 [`gemini_to_anthropic`] 共用
/// 同一份映射,所以「检查放行、转换丢字段」不会发生。
pub fn check_gemini_to_anthropic(body: &Value, model: &str) -> Result<(), TranslationError> {
    gemini_to_anthropic(body, model).map(|_| ())?;
    let object = body.as_object().expect("validated as object");
    // 生成配置里没有等价位置的键(安全设置、缓存、思考预算以外的采样项)显式拒绝,
    // 不静默丢弃 —— 安全过滤被悄悄取消是最不该发生的一类静默降级。
    if let Some(config) = object.get("generationConfig").and_then(Value::as_object) {
        for key in config.keys() {
            if !matches!(
                key.as_str(),
                "maxOutputTokens"
                    | "temperature"
                    | "topP"
                    | "stopSequences"
                    | "responseMimeType"
                    | "responseJsonSchema"
                    | "responseSchema"
            ) {
                return Err(TranslationError::UnsupportedField(format!(
                    "generationConfig.{key}"
                )));
            }
        }
    }
    for key in object.keys() {
        if !matches!(
            key.as_str(),
            "contents" | "systemInstruction" | "tools" | "generationConfig"
        ) {
            return Err(TranslationError::UnsupportedField(key.to_string()));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 请求:Anthropic → Gemini
// ---------------------------------------------------------------------------

/// 上一轮真实收到的 Gemini assistant parts(含 `thoughtSignature`)。
///
/// 签名只能**原样复用**:伪造的签名会被上游拒绝或静默改变行为,所以拿不到就不写这个
/// 字段,而不是生成占位值。复用前必须确认这条历史轮次就是产出这些 parts 的那一轮 ——
/// 把签名配到别的调用上比没有签名更糟。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GeminiReplay {
    pub parts: Vec<Value>,
}

impl GeminiReplay {
    /// 这条 assistant 消息是否就是产出 [`Self::parts`] 的那一轮。
    ///
    /// 判据是 functionCall 的**名称与入参**按顺序一一对应:客户端回传的正是上一轮
    /// 我们发出的内容,所以入参必须逐字相同。数量或内容对不上就不复用。
    pub fn matches_tool_use(&self, content: &Value) -> bool {
        let Some(blocks) = content.as_array() else {
            return false;
        };
        let calls: Vec<(&str, &Value)> = self
            .parts
            .iter()
            .filter_map(|part| part.get("functionCall").and_then(Value::as_object))
            .filter_map(|call| {
                let name = call.get("name").and_then(Value::as_str)?;
                Some((name, call.get("args").unwrap_or(&Value::Null)))
            })
            .collect();
        let uses: Vec<(&str, &Value)> = blocks
            .iter()
            .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            .filter_map(|block| {
                let name = block.get("name").and_then(Value::as_str)?;
                Some((name, block.get("input").unwrap_or(&Value::Null)))
            })
            .collect();
        !uses.is_empty() && calls == uses
    }
}

/// Anthropic 中间格式 → Gemini 请求体。
///
/// `replay` 给出本次会话此前真实收到过的 Gemini assistant parts(含
/// `thoughtSignature`)。签名只能原样复用,不能生成;拿不到就省略该字段,而不是
/// 伪造一个 —— 伪造的签名会让上游校验失败或静默改变行为。
/// 目标模型名走 URL(`/v1beta/models/{model}:generateContent`),所以请求体里不带
/// model —— 与 OpenAI/Anthropic 的 body 形状不同,由 `request_build` 单独拼路径。
pub fn try_make_gemini_body(
    request: &RoutingRequest,
    effort: Option<ReasoningEffort>,
    replay: Option<&GeminiReplay>,
) -> Result<Value, TranslationError> {
    // 只有**最后**一轮带工具调用的 assistant 消息才需要回放签名:那正是上一轮
    // 上游刚签发、且客户端即将回传结果的那一轮。
    let replay_turn = replay.and_then(|_| {
        request
            .messages
            .iter()
            .rposition(|message| message.role == "assistant" && has_tool_use(&message.content))
    });
    let mut contents: Vec<Value> = Vec::new();
    let mut tool_names: ToolNames = ToolNames::new();
    for (index, message) in request.messages.iter().enumerate() {
        let path = format!("messages[{index}].content");
        let role = match message.role.as_str() {
            "assistant" => "model",
            "user" => "user",
            other => {
                return Err(TranslationError::UnsupportedField(format!(
                    "messages[{index}].role={other}"
                )));
            }
        };
        // 命中回放:整轮用真实 parts(含签名)替换重建结果。
        let mut parts = anthropic_blocks_to_parts(&message.content, &path, &tool_names)?;
        // 回放只搬运签名本身:正文与顺序仍走正常重建 —— 整轮替换会把上游分片
        // 累积的文本一起覆盖掉,而匹配又要求逐字相同,等于两边都不成立。
        if let Some(replay) = replay.filter(|replay| {
            Some(index) == replay_turn && replay.matches_tool_use(&message.content)
        }) {
            apply_replay_signatures(&mut parts, replay);
        }
        if parts.is_empty() {
            continue;
        }
        // 先登记本轮的 id→name,后面 user 消息里的 tool_result 才能取到函数名。
        for block in message.content.as_array().into_iter().flatten() {
            if block.get("type").and_then(Value::as_str) == Some("tool_use")
                && let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                )
            {
                tool_names.insert(id.to_string(), name.to_string());
            }
        }
        contents.push(json!({"role": role, "parts": parts}));
    }
    if contents.is_empty() {
        return Err(TranslationError::InvalidInput(
            "no convertible messages".into(),
        ));
    }

    let mut body = Map::new();
    body.insert("contents".into(), Value::Array(contents));
    if let Some(system) = request.system.as_ref() {
        let text = system_text(system)?;
        if !text.is_empty() {
            body.insert(
                "systemInstruction".into(),
                json!({"parts": [{"text": text}]}),
            );
        }
    }
    let tools = anthropic_tools_to_gemini(request)?;
    if !tools.is_empty() {
        body.insert("tools".into(), Value::Array(tools));
    }
    let mut config = Map::new();
    if let Some(max_tokens) = request.raw.get("max_tokens").and_then(Value::as_i64) {
        config.insert("maxOutputTokens".into(), json!(max_tokens));
    }
    if let Some(temperature) = request.raw.get("temperature").and_then(Value::as_f64) {
        config.insert("temperature".into(), json!(temperature));
    }
    if let Some(top_p) = request.raw.get("top_p").and_then(Value::as_f64) {
        config.insert("topP".into(), json!(top_p));
    }
    if let Some(stops) = request.raw.get("stop_sequences").and_then(Value::as_array) {
        let stops: Vec<Value> = stops
            .iter()
            .filter(|stop| stop.as_str().is_some_and(|text| !text.is_empty()))
            .cloned()
            .collect();
        if !stops.is_empty() {
            config.insert("stopSequences".into(), Value::Array(stops));
        }
    }
    if let Some(schema) = crate::bridge::output_config_schema(request) {
        config.insert("responseMimeType".into(), json!("application/json"));
        config.insert("responseJsonSchema".into(), schema);
    }
    if let Some(effort) = effort {
        // Gemini 的思考预算以 token 计;这里只做档位到预算的映射,不假设具体模型
        // 支持哪些档位 —— 不支持时上游会回明确错误,比代理侧猜测更可诊断。
        config.insert(
            "thinkingConfig".into(),
            json!({"thinkingBudget": thinking_budget(effort), "includeThoughts": true}),
        );
    }
    if !config.is_empty() {
        body.insert("generationConfig".into(), Value::Object(config));
    }
    Ok(Value::Object(body))
}

fn thinking_budget(effort: ReasoningEffort) -> i64 {
    match effort {
        ReasoningEffort::None => 0,
        ReasoningEffort::Minimal | ReasoningEffort::Low => 2048,
        ReasoningEffort::Auto | ReasoningEffort::Medium => 8192,
        ReasoningEffort::High | ReasoningEffort::Max => 16384,
        ReasoningEffort::Xhigh | ReasoningEffort::Ultra => 24576,
    }
}

fn system_text(system: &Value) -> Result<String, TranslationError> {
    match system {
        Value::Null => Ok(String::new()),
        Value::String(text) => Ok(text.clone()),
        Value::Array(parts) => {
            let mut out = Vec::new();
            for (index, part) in parts.iter().enumerate() {
                let text = part.get("text").and_then(Value::as_str).ok_or_else(|| {
                    TranslationError::UnsupportedContentBlock(format!(
                        "system[{index}] is not a text block"
                    ))
                })?;
                out.push(text.to_string());
            }
            Ok(out.join("\n\n"))
        }
        _ => Err(TranslationError::InvalidInput(
            "system must be a string or text-block array".into(),
        )),
    }
}

/// 历史工具调用 id → 函数名。
///
/// Gemini 的 `functionResponse.name` 必须是被调用**函数的名字**(要与
/// `functionDeclarations` 对得上),而不是 Anthropic 那边的 `tool_use_id` —— 转换面
/// 合成的 id 形如 `gemini_call_1`,拿它当名字上游会把结果挂到不存在的函数上。
type ToolNames = std::collections::HashMap<String, String>;

fn anthropic_blocks_to_parts(
    content: &Value,
    path: &str,
    tool_names: &ToolNames,
) -> Result<Vec<Value>, TranslationError> {
    match content {
        Value::Null => Ok(Vec::new()),
        Value::String(text) => Ok(text_part(text).into_iter().collect()),
        Value::Array(blocks) => {
            let mut parts = Vec::new();
            for (index, block) in blocks.iter().enumerate() {
                let block_path = format!("{path}[{index}]");
                let object = block.as_object().ok_or_else(|| {
                    TranslationError::InvalidInput(format!("{block_path} must be an object"))
                })?;
                match object.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text" => {
                        let text = object.get("text").and_then(Value::as_str).ok_or_else(|| {
                            TranslationError::InvalidInput(format!(
                                "{block_path}.text must be a string"
                            ))
                        })?;
                        parts.extend(text_part(text));
                    }
                    "image" => {
                        let source =
                            object
                                .get("source")
                                .and_then(Value::as_object)
                                .ok_or_else(|| {
                                    TranslationError::InvalidInput(format!(
                                        "{block_path}.source is required"
                                    ))
                                })?;
                        // Gemini 只接受内联字节;URL 源需要代理先取回内容,而转换器
                        // 不该替用户发起网络请求 —— 明确拒绝。
                        let source_type = source.get("type").and_then(Value::as_str).unwrap_or("");
                        if source_type != "base64" {
                            return Err(TranslationError::UnsupportedField(format!(
                                "{block_path}.source.type={source_type}"
                            )));
                        }
                        let mime = source
                            .get("media_type")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                TranslationError::InvalidInput(format!(
                                    "{block_path}.source.media_type is required"
                                ))
                            })?;
                        let data = source.get("data").and_then(Value::as_str).ok_or_else(|| {
                            TranslationError::InvalidInput(format!(
                                "{block_path}.source.data is required"
                            ))
                        })?;
                        parts.push(json!({"inlineData": {"mimeType": mime, "data": data}}));
                    }
                    "tool_use" => {
                        let name = object.get("name").and_then(Value::as_str).ok_or_else(|| {
                            TranslationError::InvalidInput(format!("{block_path}.name is required"))
                        })?;
                        let mut call = Map::new();
                        call.insert("name".into(), json!(name));
                        call.insert(
                            "args".into(),
                            object.get("input").cloned().unwrap_or_else(|| json!({})),
                        );
                        if let Some(id) = object.get("id").and_then(Value::as_str) {
                            call.insert("id".into(), json!(id));
                        }
                        parts.push(json!({"functionCall": Value::Object(call)}));
                    }
                    "tool_result" => {
                        let tool_use_id = object
                            .get("tool_use_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| {
                                TranslationError::InvalidInput(format!(
                                    "{block_path}.tool_use_id is required"
                                ))
                            })?;
                        // 名字取自前面那条 assistant 里对应的 tool_use;查不到就退回 id
                        // (单轮、无历史的调用仍能走通),但绝不凭 id 编造函数名。
                        let name = tool_names
                            .get(tool_use_id)
                            .map(String::as_str)
                            .unwrap_or(tool_use_id);
                        let output = tool_result_text(object.get("content"))?;
                        let mut response = Map::new();
                        response.insert("name".into(), json!(name));
                        response.insert("response".into(), json!({"output": output}));
                        response.insert("id".into(), json!(tool_use_id));
                        parts.push(json!({"functionResponse": Value::Object(response)}));
                    }
                    // 历史推理摘要无法回签:与 bridge.rs 对 thinking 块的同一口径,
                    // 不回放历史推理(对话仍成立)而不是拒绝整条请求。
                    "thinking" | "redacted_thinking" => {}
                    other => {
                        return Err(TranslationError::UnsupportedContentBlock(other.to_string()));
                    }
                }
            }
            Ok(parts)
        }
        _ => Err(TranslationError::InvalidInput(format!(
            "{path} must be a string or content-block array"
        ))),
    }
}

/// 把回放里的真实签名贴到重建出的 functionCall 上。
///
/// 位置由工具调用在各自序列中的出现顺序决定:调用方已经用
/// [`GeminiReplay::matches_tool_use`] 确认过名称与入参逐字对应。签名只搬不造 ——
/// 回放里没有对应项时就不写这个字段。
fn apply_replay_signatures(parts: &mut [Value], replay: &GeminiReplay) {
    let signatures: Vec<&Value> = replay
        .parts
        .iter()
        .filter_map(|part| part.get("thoughtSignature"))
        .collect();
    let mut next = 0;
    for part in parts.iter_mut() {
        if part.get("functionCall").is_none() {
            continue;
        }
        if let Some(signature) = signatures.get(next) {
            part["thoughtSignature"] = (*signature).clone();
        }
        next += 1;
    }
}

fn has_tool_use(content: &Value) -> bool {
    content.as_array().is_some_and(|blocks| {
        blocks
            .iter()
            .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
    })
}

fn text_part(text: &str) -> Option<Value> {
    (!text.is_empty()).then(|| json!({"text": text}))
}

/// tool_result 内容 → 文本。Gemini 的 functionResponse.response 是 JSON 对象;
/// 非文本块(图片等)没有等价位置,压平会丢信息,所以显式拒绝。
fn tool_result_text(content: Option<&Value>) -> Result<Value, TranslationError> {
    let Some(content) = content.filter(|value| !value.is_null()) else {
        return Ok(json!(""));
    };
    match content {
        Value::String(text) => Ok(json!(text)),
        Value::Array(blocks) => {
            let mut out = String::new();
            for (index, block) in blocks.iter().enumerate() {
                let text = block.get("text").and_then(Value::as_str).ok_or_else(|| {
                    TranslationError::UnsupportedContentBlock(format!(
                        "tool_result.content[{index}] is not a text block"
                    ))
                })?;
                out.push_str(text);
            }
            Ok(json!(out))
        }
        other => Ok(other.clone()),
    }
}

fn anthropic_tools_to_gemini(request: &RoutingRequest) -> Result<Vec<Value>, TranslationError> {
    if request.tools.is_empty() {
        return Ok(Vec::new());
    }
    let mut declarations = Vec::new();
    for (index, tool) in request.tools.iter().enumerate() {
        let path = format!("tools[{index}]");
        // 服务端工具(web_search 等)由上游执行,不是 function。把它们伪装成
        // functionDeclaration 会让模型以为自己有检索能力却永远拿不到结果 —— 与
        // bridge.rs 对同一字段的口径一致:没有等价位置就拒绝。
        let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
        if !crate::routing::inspector::is_client_tool_type(tool_type) {
            return Err(TranslationError::UnsupportedTool(tool_type.to_string()));
        }
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .ok_or_else(|| TranslationError::InvalidInput(format!("{path}.name is required")))?;
        let mut declaration = Map::new();
        declaration.insert("name".into(), json!(name));
        if let Some(description) = tool.get("description").and_then(Value::as_str) {
            declaration.insert("description".into(), json!(description));
        }
        if let Some(schema) = tool
            .get("input_schema")
            .filter(|value| !value.is_null())
            .map(normalize_object_schema)
        {
            // 完整 JSON Schema 走 parametersJsonSchema;`parameters` 只接受 OpenAPI
            // 子集,塞进去会被上游拒绝或静默裁掉关键字。
            declaration.insert("parametersJsonSchema".into(), schema);
        }
        declarations.push(Value::Object(declaration));
    }
    Ok(vec![json!({"functionDeclarations": declarations})])
}

/// Anthropic → Gemini 的可转换能力检查。
pub fn check_anthropic_to_gemini(request: &RoutingRequest) -> Result<(), TranslationError> {
    try_make_gemini_body(request, None, None).map(|_| ())
}

// ---------------------------------------------------------------------------
// 响应:Gemini 上游 SSE → Anthropic SSE
// ---------------------------------------------------------------------------

/// 上游 Gemini SSE → Anthropic SSE。
///
/// Gemini 的流是「累计快照 + 增量混用」:文本可能是完整前缀而不是增量片段。这里按
/// 快照语义处理 —— 记录已发出的前缀,只发出新增部分,重复快照不会重复输出。
pub struct GeminiStreamBridge {
    message_id: String,
    model: String,
    stream: bool,
    buffer: SseBlockBuffer,
    started: bool,
    finished: bool,
    terminal: BridgeTerminal,
    json_emitted: bool,
    text: String,
    input_tokens: i64,
    output_tokens: i64,
    reasoning_tokens: i64,
    stop_reason: &'static str,
    /// 是否见过上游的 finishReason。没见过的 EOF 是截断,不是完成。
    saw_finish: bool,
    /// 当前开着的文本块索引。
    text_block_index: Option<usize>,
    next_block_index: usize,
    /// 已开出的工具块(按声明顺序)。
    tool_blocks: Vec<ToolBlock>,
    /// 本轮上游真实返回的 `functionCall` parts(含 `thoughtSignature`),供 engine 回放。
    ///
    /// 只收 functionCall:签名只挂在它们上面,而它们不像文本那样走累计快照,
    /// 逐块累积既完整又不会重复。
    replay_parts: Vec<Value>,
}

struct ToolBlock {
    index: usize,
    id: String,
    name: String,
    args: String,
}

impl GeminiStreamBridge {
    pub fn new(message_id: String, model: String, stream: bool) -> Self {
        Self {
            message_id,
            model,
            stream,
            buffer: SseBlockBuffer::new(),
            started: false,
            finished: false,
            terminal: BridgeTerminal::Pending,
            json_emitted: false,
            text: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            stop_reason: "end_turn",
            saw_finish: false,
            text_block_index: None,
            next_block_index: 0,
            tool_blocks: Vec::new(),
            replay_parts: Vec::new(),
        }
    }

    /// 本轮 Gemini assistant parts 的真实副本(含 `thoughtSignature`)。engine 用它
    /// 作为下轮工具回放的依据;没有真实签名时这里就是上游给的原样内容。
    pub fn replay_parts(&self) -> &[Value] {
        &self.replay_parts
    }

    /// 逐块累积本轮的 functionCall 部分。同一调用若被上游重复下发(相邻且内容相同)
    /// 只保留一份,避免匹配时多出并不存在的调用。
    fn collect_replay_calls(&mut self, parts: &[Value]) {
        for part in parts {
            if part.get("functionCall").is_none() {
                continue;
            }
            if self.replay_parts.last() == Some(part) {
                continue;
            }
            self.replay_parts.push(part.clone());
        }
    }

    fn ensure_started(&mut self, events: &mut Vec<SseEvent>) {
        if !self.started {
            events.push(message_start_event(&self.message_id, &self.model));
            self.started = true;
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
        self.text_block_index = None;
        if self.stream {
            events.push(content_block_stop_event(index));
        }
    }

    /// 累计快照语义:只发出相对已有的新增后缀。
    fn push_text(&mut self, snapshot: &str, events: &mut Vec<SseEvent>) {
        let delta = if snapshot.starts_with(&self.text) {
            snapshot[self.text.len()..].to_string()
        } else {
            // 上游回的是纯增量(或与已有文本无法拼接)时按增量处理。
            snapshot.to_string()
        };
        if delta.is_empty() {
            return;
        }
        self.text.push_str(&delta);
        let index = self.open_text_block(events);
        events.push(text_delta_event(&delta, index));
    }

    fn open_tool_block(&mut self, name: &str, args: &Value, events: &mut Vec<SseEvent>) -> usize {
        self.close_text_block(events);
        let index = self.next_block_index;
        self.next_block_index += 1;
        // Gemini 的 functionCall 常常没有 id;按声明顺序合成一个稳定 id,让客户端的
        // 工具结果能配回来。
        let id = format!("gemini_call_{index}");
        let arguments = serde_json::to_string(args).unwrap_or_else(|_| "{}".into());
        if self.stream {
            events.push(SseEvent {
                event: "content_block_start".into(),
                data: json!({
                    "type": "content_block_start",
                    "index": index,
                    "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}},
                }),
            });
            events.push(input_json_delta_event(&arguments, index));
        }
        self.tool_blocks.push(ToolBlock {
            index,
            id: id.clone(),
            name: name.to_string(),
            args: arguments,
        });
        index
    }

    fn close_all_blocks(&mut self, events: &mut Vec<SseEvent>) {
        self.close_text_block(events);
        if !self.stream {
            return;
        }
        let mut indexes: Vec<usize> = self.tool_blocks.iter().map(|block| block.index).collect();
        indexes.sort_unstable();
        for index in indexes {
            events.push(content_block_stop_event(index));
        }
    }

    fn complete(&mut self) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        // 上游没给 finishReason 就结束 = 截断。把片段当完整回答回给客户端比报错更糟:
        // 客户端会拿半截结果继续跑,而且没有任何可诊断的迹象。
        if !self.saw_finish {
            return self.fail("upstream stream ended without a finish reason");
        }
        self.finished = true;
        self.terminal = BridgeTerminal::Completed;
        let mut events = Vec::new();
        self.ensure_started(&mut events);
        if self.text_block_index.is_none() && self.tool_blocks.is_empty() {
            // 没有块时补一个空文本块:客户端总要收到至少一个 content_block。
            self.open_text_block(&mut events);
        }
        // 出过工具块就必须报 tool_use:回 end_turn 会让客户端以为轮次结束,不去执行
        // 工具(与 bridge.rs 对同一情形的判定一致)。
        let stop_reason = if self.tool_blocks.is_empty() {
            self.stop_reason
        } else {
            "tool_use"
        };
        self.close_all_blocks(&mut events);
        events.extend(message_stop_events(
            stop_reason,
            self.input_tokens,
            self.output_tokens,
        ));
        events
    }

    /// `(input, output, reasoning)` 用量。Gemini 把思考 token 单列,Anthropic 的
    /// usage 里没有对应字段,所以单独暴露给 engine 记账。
    pub fn usage(&self) -> (i64, i64, i64) {
        (self.input_tokens, self.output_tokens, self.reasoning_tokens)
    }

    fn fail(&mut self, detail: impl Into<String>) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        let detail = detail.into();
        self.finished = true;
        self.terminal = BridgeTerminal::Failed(detail.clone());
        let mut events = Vec::new();
        self.ensure_started(&mut events);
        // 已经发出去的块必须收尾,否则客户端会停在未关闭的块上。
        self.close_all_blocks(&mut events);
        events.push(error_event(&detail));
        events
    }

    fn handle_chunk(&mut self, payload: &str) -> Vec<SseEvent> {
        if self.finished {
            return Vec::new();
        }
        let Ok(chunk) = serde_json::from_str::<Value>(payload) else {
            // 畸形 JSON 不能假装成功:上游的流契约已经破了。
            return self.fail("upstream emitted malformed JSON");
        };
        if let Some(feedback) = chunk.get("promptFeedback")
            && let Some(reason) = feedback.get("blockReason").and_then(Value::as_str)
        {
            return self.fail(format!("upstream blocked the prompt: {reason}"));
        }
        if let Some(error) = chunk.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("upstream returned an error");
            return self.fail(format!("upstream error: {message}"));
        }
        let mut events = Vec::new();
        if chunk.get("candidates").is_some() || chunk.get("usageMetadata").is_some() {
            self.ensure_started(&mut events);
        }
        if let Some(model) = chunk.get("modelVersion").and_then(Value::as_str) {
            self.model = model.to_string();
        }
        if let Some(usage) = chunk.get("usageMetadata") {
            self.capture_usage(usage);
        }
        let candidate = chunk
            .get("candidates")
            .and_then(Value::as_array)
            .and_then(|items| items.first());
        if let Some(candidate) = candidate {
            if let Some(content) = candidate.get("content") {
                if let Some(parts) = content.get("parts").and_then(Value::as_array) {
                    for part in parts {
                        self.handle_part(part, &mut events);
                    }
                    // 只累积 functionCall 部分:签名只挂在它们上面,而它们每个只出现
                    // 一次(累计快照语义只作用于文本)。整体替换会丢掉前面分片里的
                    // 调用,逐块追加文本又会让 "HeHe" 这类重复进回放。
                    self.collect_replay_calls(parts);
                }
            }
            if let Some(reason) = candidate.get("finishReason").and_then(Value::as_str) {
                self.stop_reason = map_finish_reason(reason);
                self.saw_finish = true;
                // finishReason 之后还可能有 usage-only 帧,完成留到流结束时再定。
            }
        }
        events
    }

    fn handle_part(&mut self, part: &Value, events: &mut Vec<SseEvent>) {
        let Some(object) = part.as_object() else {
            return;
        };
        if object.get("thought").and_then(Value::as_bool) == Some(true) {
            // 思考摘要不回放给 Anthropic 客户端(没有对应块类型),但计数保留。
            if let Some(text) = object.get("text").and_then(Value::as_str) {
                self.output_tokens += text.len() as i64;
            }
            return;
        }
        if let Some(text) = object.get("text").and_then(Value::as_str) {
            self.push_text(text, events);
            return;
        }
        if let Some(call) = object.get("functionCall").and_then(Value::as_object) {
            let name = call
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                return;
            }
            let args = call.get("args").cloned().unwrap_or_else(|| json!({}));
            self.open_tool_block(&name, &args, events);
        }
    }

    fn capture_usage(&mut self, usage: &Value) {
        let prompt = usage.get("promptTokenCount").and_then(Value::as_i64);
        let cached = usage
            .get("cachedContentTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let total = usage.get("totalTokenCount").and_then(Value::as_i64);
        let thoughts = usage
            .get("thoughtsTokenCount")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        if let Some(prompt) = prompt {
            // Anthropic 的 input_tokens 是「新鲜输入」,Gemini 的 promptTokenCount
            // 含缓存命中,所以这里减去缓存部分。
            self.input_tokens = (prompt - cached).max(0);
        }
        // 输出优先用 candidatesTokenCount(它不含思考 token);缺失时才用
        // total-prompt 兜底。
        if let Some(candidates) = usage.get("candidatesTokenCount").and_then(Value::as_i64) {
            self.output_tokens = candidates.max(0);
        } else if let (Some(total), Some(prompt)) = (total, prompt) {
            self.output_tokens = (total - prompt).max(0);
        }
        self.reasoning_tokens = thoughts;
    }

    fn render_non_stream(&mut self) -> Vec<u8> {
        if self.stream || self.json_emitted {
            return Vec::new();
        }
        let rendered = match &self.terminal {
            BridgeTerminal::Failed(detail) => crate::bridge::canonical_json(&json!({
                "type": "error",
                "error": {"type": "api_error", "message": detail},
            }))
            .into_bytes(),
            BridgeTerminal::Completed => {
                let mut content: Vec<Value> = Vec::new();
                if !self.text.is_empty() {
                    content.push(json!({"type": "text", "text": self.text}));
                }
                for block in &self.tool_blocks {
                    content.push(json!({
                        "type": "tool_use",
                        "id": block.id,
                        "name": block.name,
                        "input": serde_json::from_str::<Value>(&block.args)
                            .unwrap_or_else(|_| json!({})),
                    }));
                }
                crate::bridge::canonical_json(&json!({
                    "id": self.message_id,
                    "type": "message",
                    "role": "assistant",
                    "model": self.model,
                    "content": content,
                    "stop_reason": self.stop_reason,
                    "stop_sequence": null,
                    "usage": {"input_tokens": self.input_tokens, "output_tokens": self.output_tokens},
                }))
                .into_bytes()
            }
            BridgeTerminal::Pending => return Vec::new(),
        };
        self.json_emitted = true;
        rendered
    }
}

fn map_finish_reason(reason: &str) -> &'static str {
    match reason {
        "STOP" => "end_turn",
        "MAX_TOKENS" => "max_tokens",
        // 工具调用在 Gemini 里靠 parts 体现,finishReason 不是 tool_use;
        // 其余(SAFETY、RECITATION、OTHER 等)按 Anthropic 的 refusal 语义收口。
        "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" => "refusal",
        _ => "end_turn",
    }
}

impl SseBridge for GeminiStreamBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        for block in self.buffer.push(data) {
            let Some(payload) = data_payload(&block) else {
                continue;
            };
            if self.stream {
                for event in self.handle_chunk(&payload) {
                    output.extend(event.to_bytes());
                }
            } else {
                // 非流式客户端:继续吸收上游事件,收尾时一次性渲染。
                self.handle_chunk(&payload);
            }
            output.extend(self.render_non_stream());
        }
        output
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.finished && self.json_emitted {
            return Vec::new();
        }
        let mut output = Vec::new();
        if self.stream {
            for event in self.complete() {
                output.extend(event.to_bytes());
            }
        } else {
            if !self.finished {
                self.complete();
            }
            output.extend(self.render_non_stream());
        }
        output
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }

    fn replay_parts(&self) -> Option<Vec<Value>> {
        (!self.replay_parts.is_empty()).then(|| self.replay_parts.clone())
    }
}

// ---------------------------------------------------------------------------
// 响应:Anthropic SSE → 客户端 Gemini
// ---------------------------------------------------------------------------

/// Anthropic SSE → 客户端 Gemini SSE / 非流式 JSON。
pub struct GeminiClientBridge {
    buffer: SseBlockBuffer,
    stream: bool,
    finished: bool,
    terminal: BridgeTerminal,
    json_emitted: bool,
    model: String,
    input_tokens: i64,
    output_tokens: i64,
    /// 已发出的 parts;工具块按 index 累积,收尾时统一落成 functionCall。
    parts: Vec<Value>,
    /// anthropic content_block index → 累积中的工具调用。
    tools: Vec<(usize, ToolBlock)>,
    stop_reason: &'static str,
}

impl GeminiClientBridge {
    pub fn new(model: String, stream: bool) -> Self {
        Self {
            buffer: SseBlockBuffer::new(),
            stream,
            finished: false,
            terminal: BridgeTerminal::Pending,
            json_emitted: false,
            model,
            input_tokens: 0,
            output_tokens: 0,
            parts: Vec::new(),
            tools: Vec::new(),
            stop_reason: "STOP",
        }
    }

    fn event_json(&self, parts: Vec<Value>, finish_reason: Option<&str>) -> Value {
        let mut candidate = Map::new();
        candidate.insert("content".into(), json!({"role": "model", "parts": parts}));
        candidate.insert("index".into(), json!(0));
        if let Some(reason) = finish_reason {
            candidate.insert("finishReason".into(), json!(reason));
        }
        let mut out = Map::new();
        out.insert("candidates".into(), json!([Value::Object(candidate)]));
        if finish_reason.is_some() {
            out.insert(
                "usageMetadata".into(),
                json!({
                    "promptTokenCount": self.input_tokens,
                    "candidatesTokenCount": self.output_tokens,
                    "totalTokenCount": self.input_tokens + self.output_tokens,
                }),
            );
        }
        Value::Object(out)
    }

    fn frame(&self, value: &Value) -> Vec<u8> {
        format!("data: {}\n\n", crate::bridge::canonical_json(value)).into_bytes()
    }

    /// 流式收尾:把累积的 functionCall 一次性发出(与 Gemini 官方流的行为一致 ——
    /// 工具参数不在流中分片)。
    fn emit_tail(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.terminal = BridgeTerminal::Completed;
        let mut parts = std::mem::take(&mut self.parts);
        for (_, tool) in std::mem::take(&mut self.tools) {
            let mut call = Map::new();
            call.insert("name".into(), json!(tool.name));
            call.insert(
                "args".into(),
                serde_json::from_str::<Value>(&tool.args).unwrap_or_else(|_| json!({})),
            );
            parts.push(json!({"functionCall": Value::Object(call)}));
        }
        if parts.is_empty() {
            // Gemini 要求 candidate.content.parts 非空。
            parts.push(json!({"text": ""}));
        }
        let value = self.event_json(parts, Some(self.stop_reason));
        if self.stream {
            self.frame(&value)
        } else {
            self.json_emitted = true;
            crate::bridge::canonical_json(&value).into_bytes()
        }
    }

    fn handle_event(&mut self, event: &Value) -> Vec<u8> {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "error" => {
                let detail = event
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("upstream error");
                self.finished = true;
                self.terminal = BridgeTerminal::Failed(detail.to_string());
                let body = json!({
                    "error": {"code": 502, "message": detail, "status": "UNAVAILABLE"},
                });
                // 非流式客户端收到的是 application/json,SSE 帧在那边不是合法 JSON。
                if self.stream {
                    self.frame(&body)
                } else {
                    self.json_emitted = true;
                    crate::bridge::canonical_json(&body).into_bytes()
                }
            }
            "message_start" => {
                if let Some(model) = event.pointer("/message/model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                Vec::new()
            }
            "content_block_start" => {
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                // 文本块按 Anthropic 的形状开,正文由 delta 处理 —— 流式逐帧发出,
                // 非流式累积到 parts(由 append_text 建 part),这里不需要动作。
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    self.tools.push((
                        index,
                        ToolBlock {
                            index,
                            id: block
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            name: block
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                            args: String::new(),
                        },
                    ));
                }
                Vec::new()
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let Some(text) = delta.get("text").and_then(Value::as_str) else {
                            return Vec::new();
                        };
                        self.output_tokens += 1;
                        // 文本是增量语义,直接作为 Gemini 的 text part 发出(客户端按
                        // 增量拼接,与官方流一致)。
                        let value = self.event_json(vec![json!({"text": text})], None);
                        return if self.stream {
                            self.frame(&value)
                        } else {
                            self.append_text(text);
                            Vec::new()
                        };
                    }
                    "input_json_delta" => {
                        if let Some(fragment) = delta.get("partial_json").and_then(Value::as_str)
                            && let Some((_, tool)) =
                                self.tools.iter_mut().find(|(slot, _)| *slot == index)
                        {
                            tool.args.push_str(fragment);
                        }
                    }
                    _ => {}
                }
                Vec::new()
            }
            "message_delta" => {
                if let Some(reason) = event.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    self.stop_reason = match reason {
                        "max_tokens" => "MAX_TOKENS",
                        "refusal" => "SAFETY",
                        // Gemini 没有 tool_use 终止原因:工具调用由 parts 体现。
                        _ => "STOP",
                    };
                }
                if let Some(tokens) = event
                    .pointer("/usage/output_tokens")
                    .and_then(Value::as_i64)
                {
                    self.output_tokens = tokens;
                }
                if let Some(tokens) = event.pointer("/usage/input_tokens").and_then(Value::as_i64) {
                    self.input_tokens = tokens;
                }
                Vec::new()
            }
            "message_stop" => self.emit_tail(),
            _ => Vec::new(),
        }
    }

    fn append_text(&mut self, text: &str) {
        match self.parts.last_mut() {
            Some(part) if part.get("text").is_some() => {
                let merged = format!(
                    "{}{}",
                    part.get("text").and_then(Value::as_str).unwrap_or_default(),
                    text
                );
                *part = json!({"text": merged});
            }
            _ => self.parts.push(json!({"text": text})),
        }
    }
}

impl SseBridge for GeminiClientBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        let mut output = Vec::new();
        for block in self.buffer.push(data) {
            let Some(payload) = data_payload(&block) else {
                continue;
            };
            let Ok(event) = serde_json::from_str::<Value>(&payload) else {
                continue;
            };
            output.extend(self.handle_event(&event));
        }
        output
    }

    fn finish(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        // 上游没给 message_stop 也算截断:不能把不完整的流当成功尾。
        self.terminal = BridgeTerminal::Failed("upstream stream ended without a terminal".into());
        if self.stream {
            self.finished = true;
            return Vec::new();
        }
        self.finished = true;
        self.json_emitted = true;
        Vec::new()
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }
}
