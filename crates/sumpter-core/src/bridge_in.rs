//! 入站 OpenAI 兼容层(Codex 等 OpenAI 系客户端接入):
//! chat/completions 与 Responses 请求体 → Anthropic Messages body(请求侧),
//! Anthropic SSE → chat chunks / Responses SSE(响应侧,反向流桥)。
//!
//! 与出站桥(bridge.rs,Anthropic→OpenAI)方向相反;**必须**涵盖工具调用
//! (Codex 的 shell/apply_patch 全靠 function call),这是与出站桥「仅文本」
//! 的关键差异。转换后的 body 走既有路由/粘性/failover 管线,`stream` 恒置
//! true(客户端要非流式时由响应桥聚合出单 JSON)。

use serde_json::{Map, Value, json};

use crate::bridge::{
    BridgeTerminal, SseBlockBuffer, SseBridge, TranslationError, canonical_json, data_payload,
    validate_text_content,
};
use crate::model_name::{self, ReasoningEffort};

/// 入站客户端方言(由入站路径判定)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientDialect {
    /// `/v1/chat/completions`
    Chat,
    /// `/v1/responses`
    Responses,
}
/// 客户端是否要求流式(`"stream": true`;缺省按非流式处理,与 OpenAI 语义一致)。
pub fn client_wants_stream(body: &Value) -> bool {
    body.get("stream").and_then(Value::as_bool).unwrap_or(false)
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a str, TranslationError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| TranslationError::InvalidInput(format!("{field} is required")))
}

fn required_value<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<&'a Value, TranslationError> {
    object
        .get(field)
        .filter(|value| !value.is_null())
        .ok_or_else(|| TranslationError::InvalidInput(format!("{path}.{field} is required")))
}

fn validate_arguments(value: Option<&Value>, path: &str) -> Result<(), TranslationError> {
    let Some(value) = value else {
        return Err(TranslationError::InvalidInput(format!(
            "{path} is required"
        )));
    };
    match value {
        Value::Object(_) => Ok(()),
        Value::String(raw) => {
            let parsed = serde_json::from_str::<Value>(raw).map_err(|_| {
                TranslationError::InvalidInput(format!("{path} must be a JSON object"))
            })?;
            if parsed.is_object() {
                Ok(())
            } else {
                Err(TranslationError::InvalidInput(format!(
                    "{path} must be a JSON object"
                )))
            }
        }
        _ => Err(TranslationError::InvalidInput(format!(
            "{path} must be a JSON object"
        ))),
    }
}

fn validate_tool_definition(
    tool: &Value,
    path: &str,
    nested_function: bool,
) -> Result<(), TranslationError> {
    let object = tool
        .as_object()
        .ok_or_else(|| TranslationError::InvalidInput(format!("{path} must be an object")))?;
    let tool_type = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("function");
    if tool_type != "function" {
        return Err(TranslationError::UnsupportedTool(tool_type.to_string()));
    }
    let (function, function_path) = if nested_function {
        (
            object.get("function").ok_or_else(|| {
                TranslationError::InvalidInput(format!("{path}.function is required"))
            })?,
            format!("{path}.function"),
        )
    } else {
        (tool, path.to_string())
    };
    let function = function.as_object().ok_or_else(|| {
        TranslationError::InvalidInput(format!("{function_path} must be an object"))
    })?;
    required_string(function, "name")?;
    if let Some(description) = function.get("description")
        && !description.is_null()
        && !description.is_string()
    {
        return Err(TranslationError::InvalidInput(format!(
            "{function_path}.description must be a string"
        )));
    }
    if let Some(parameters) = function.get("parameters")
        && !parameters.is_null()
        && !parameters.is_object()
    {
        return Err(TranslationError::InvalidInput(format!(
            "{function_path}.parameters must be an object"
        )));
    }
    if let Some(strict) = function.get("strict").filter(|value| !value.is_null()) {
        match strict.as_bool() {
            Some(false) => {}
            Some(true) => {
                return Err(TranslationError::UnsupportedField(format!(
                    "{function_path}.strict"
                )));
            }
            None => {
                return Err(TranslationError::InvalidInput(format!(
                    "{function_path}.strict must be a boolean"
                )));
            }
        }
    }
    Ok(())
}

fn validate_tool_choice(
    choice: Option<&Value>,
    has_tools: bool,
    nested_function: bool,
) -> Result<(), TranslationError> {
    let Some(choice) = choice.filter(|value| !value.is_null()) else {
        return Ok(());
    };
    if !has_tools {
        return match choice.as_str() {
            Some("auto" | "none") => Ok(()),
            _ => Err(TranslationError::UnsupportedField("tool_choice".into())),
        };
    }
    match choice {
        Value::String(value) if matches!(value.as_str(), "auto" | "required" | "none") => {}
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) != Some("function") {
                return Err(TranslationError::UnsupportedField("tool_choice".into()));
            }
            let function = if nested_function {
                object.get("function").and_then(Value::as_object)
            } else {
                Some(object)
            };
            if function
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .is_none_or(str::is_empty)
            {
                return Err(TranslationError::InvalidInput(
                    "tool_choice function name is required".into(),
                ));
            }
        }
        _ => {
            return Err(TranslationError::InvalidInput(
                "tool_choice is not representable".into(),
            ));
        }
    }
    Ok(())
}

/// 检查 chat/completions → Anthropic 的首版可转换能力。
///
/// 只接受纯文本、function 工具及其对象参数；任何会被旧实现过滤或压平的
/// 内容块、工具类型、引用或历史 reasoning 都在请求进入转换器前拒绝。
pub fn check_chat_to_anthropic(body: &Value) -> Result<(), TranslationError> {
    let object = body
        .as_object()
        .ok_or_else(|| TranslationError::InvalidInput("body is not an object".into()))?;
    required_string(object, "model")?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| TranslationError::InvalidInput("messages is required".into()))?;
    for (index, message) in messages.iter().enumerate() {
        let message = message.as_object().ok_or_else(|| {
            TranslationError::InvalidInput(format!("messages[{index}] must be an object"))
        })?;
        let role = required_string(message, "role")?;
        match role {
            "system" | "developer" | "user" => {
                let content = required_value(message, "content", &format!("messages[{index}]"))?;
                validate_text_content(
                    Some(content),
                    &format!("messages[{index}].content"),
                    &["text", "input_text", "output_text"],
                    true,
                )?;
            }
            "assistant" => {
                validate_text_content(
                    message.get("content"),
                    &format!("messages[{index}].content"),
                    &["text", "input_text", "output_text"],
                    true,
                )?;
                if let Some(refusal) = message.get("refusal")
                    && !refusal.is_null()
                {
                    return Err(TranslationError::UnsupportedField(format!(
                        "messages[{index}].refusal"
                    )));
                }
                if let Some(calls) = message.get("tool_calls") {
                    let calls = calls.as_array().ok_or_else(|| {
                        TranslationError::InvalidInput(format!(
                            "messages[{index}].tool_calls must be an array"
                        ))
                    })?;
                    for (call_index, call) in calls.iter().enumerate() {
                        let call = call.as_object().ok_or_else(|| {
                            TranslationError::InvalidInput(format!(
                                "messages[{index}].tool_calls[{call_index}] must be an object"
                            ))
                        })?;
                        let call_type = call
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("function");
                        if call_type != "function" {
                            return Err(TranslationError::UnsupportedTool(call_type.into()));
                        }
                        required_string(call, "id")?;
                        let function = call.get("function").ok_or_else(|| {
                            TranslationError::InvalidInput(format!(
                                "messages[{index}].tool_calls[{call_index}].function is required"
                            ))
                        })?;
                        let function = function.as_object().ok_or_else(|| {
                            TranslationError::InvalidInput(format!(
                                "messages[{index}].tool_calls[{call_index}].function must be an object"
                            ))
                        })?;
                        required_string(function, "name")?;
                        validate_arguments(
                            function.get("arguments"),
                            &format!(
                                "messages[{index}].tool_calls[{call_index}].function.arguments"
                            ),
                        )?;
                    }
                }
            }
            "tool" => {
                required_string(message, "tool_call_id")?;
                let content = required_value(message, "content", &format!("messages[{index}]"))?;
                validate_text_content(
                    Some(content),
                    &format!("messages[{index}].content"),
                    &["text", "input_text", "output_text"],
                    true,
                )?;
            }
            other => {
                return Err(TranslationError::UnsupportedField(format!(
                    "messages[{index}].role={other}"
                )));
            }
        }
    }

    let tools = match object.get("tools") {
        None | Some(Value::Null) => &[][..],
        Some(value) => value
            .as_array()
            .ok_or_else(|| TranslationError::InvalidInput("tools must be an array".into()))?,
    };
    for (index, tool) in tools.iter().enumerate() {
        validate_tool_definition(tool, &format!("tools[{index}]"), true)?;
    }
    validate_tool_choice(object.get("tool_choice"), !tools.is_empty(), true)?;
    if let Some(effort) = object.get("reasoning_effort")
        && !effort.is_null()
        && effort
            .as_str()
            .and_then(|value| ReasoningEffort::parse(value.trim()))
            .is_none()
    {
        return Err(TranslationError::InvalidInput(
            "reasoning_effort is not supported".into(),
        ));
    }
    Ok(())
}

/// 检查 Responses → Anthropic 的首版可转换能力。
pub fn check_responses_to_anthropic(body: &Value) -> Result<(), TranslationError> {
    let object = body
        .as_object()
        .ok_or_else(|| TranslationError::InvalidInput("body is not an object".into()))?;
    required_string(object, "model")?;
    if let Some(instructions) = object.get("instructions")
        && !instructions.is_null()
        && !instructions.is_string()
    {
        return Err(TranslationError::InvalidInput(
            "instructions must be a string".into(),
        ));
    }
    match object.get("input") {
        Some(Value::String(_)) => {}
        Some(Value::Array(items)) => {
            for (index, item) in items.iter().enumerate() {
                let item = item.as_object().ok_or_else(|| {
                    TranslationError::InvalidInput(format!("input[{index}] must be an object"))
                })?;
                let item_type = item.get("type").and_then(Value::as_str).unwrap_or_else(|| {
                    if item.get("role").is_some() {
                        "message"
                    } else {
                        ""
                    }
                });
                match item_type {
                    "message" => {
                        let role = required_string(item, "role")?;
                        match role {
                            "system" | "developer" | "user" | "assistant" => {
                                let content =
                                    required_value(item, "content", &format!("input[{index}]"))?;
                                validate_text_content(
                                    Some(content),
                                    &format!("input[{index}].content"),
                                    &["text", "input_text", "output_text"],
                                    true,
                                )?;
                            }
                            other => {
                                return Err(TranslationError::UnsupportedField(format!(
                                    "input[{index}].role={other}"
                                )));
                            }
                        }
                    }
                    "function_call" => {
                        required_string(item, "name")?;
                        required_string(item, "call_id")
                            .or_else(|_| required_string(item, "id"))?;
                        validate_arguments(
                            item.get("arguments"),
                            &format!("input[{index}].arguments"),
                        )?;
                    }
                    "function_call_output" => {
                        required_string(item, "call_id")?;
                        let output = required_value(item, "output", &format!("input[{index}]"))?;
                        validate_text_content(
                            Some(output),
                            &format!("input[{index}].output"),
                            &["text", "input_text", "output_text"],
                            true,
                        )?;
                    }
                    "reasoning" => {
                        // 空 summary 且没有 encrypted_content 时没有可观察推理内容，
                        // 可以安全忽略；一旦有正文/签名，Anthropic 中间格式无法
                        // 保真表达，必须拒绝而不是静默丢弃。
                        let summary_empty = item.get("summary").is_none_or(|summary| {
                            summary.as_array().is_some_and(|items| items.is_empty())
                        });
                        let encrypted_empty =
                            item.get("encrypted_content").is_none_or(Value::is_null);
                        if !summary_empty || !encrypted_empty {
                            return Err(TranslationError::UnsupportedField(format!(
                                "input[{index}].reasoning"
                            )));
                        }
                    }
                    other => {
                        return Err(TranslationError::UnsupportedContentBlock(
                            if other.is_empty() {
                                "missing input item type".into()
                            } else {
                                other.into()
                            },
                        ));
                    }
                }
            }
        }
        Some(Value::Null) | None => {
            return Err(TranslationError::InvalidInput("input is required".into()));
        }
        Some(_) => {
            return Err(TranslationError::InvalidInput(
                "input must be a string or array".into(),
            ));
        }
    }
    let tools = match object.get("tools") {
        None | Some(Value::Null) => &[][..],
        Some(value) => value
            .as_array()
            .ok_or_else(|| TranslationError::InvalidInput("tools must be an array".into()))?,
    };
    for (index, tool) in tools.iter().enumerate() {
        // Responses 内建工具的 schema 与 Anthropic 工具定义不同；其余专用
        // API 由独立 Native Adapter 处理，不能在普通消息桥中猜测转换。
        validate_tool_definition(tool, &format!("tools[{index}]"), false)?;
    }
    validate_tool_choice(object.get("tool_choice"), !tools.is_empty(), false)?;
    if let Some(reasoning) = object.get("reasoning")
        && !reasoning.is_null()
    {
        let reasoning = reasoning
            .as_object()
            .ok_or_else(|| TranslationError::InvalidInput("reasoning must be an object".into()))?;
        for key in reasoning.keys() {
            if key != "effort" {
                return Err(TranslationError::UnsupportedField(format!(
                    "reasoning.{key}"
                )));
            }
        }
        if let Some(effort) = reasoning.get("effort")
            && !effort.is_null()
            && effort
                .as_str()
                .and_then(|value| ReasoningEffort::parse(value.trim()))
                .is_none()
        {
            return Err(TranslationError::InvalidInput(
                "reasoning.effort is not supported".into(),
            ));
        }
    }
    if let Some(include) = object.get("include").filter(|value| !value.is_null()) {
        let items = include
            .as_array()
            .ok_or_else(|| TranslationError::InvalidInput("include must be an array".into()))?;
        if !items.is_empty() {
            return Err(TranslationError::UnsupportedField("include".into()));
        }
    }
    Ok(())
}

/// 能力检查通过后执行 Chat → Anthropic 转换。
pub fn try_chat_to_anthropic(body: &Value) -> Result<Value, TranslationError> {
    check_chat_to_anthropic(body)?;
    chat_to_anthropic(body).map_err(TranslationError::InvalidInput)
}

/// 能力检查通过后执行 Responses → Anthropic 转换。
pub fn try_responses_to_anthropic(body: &Value) -> Result<Value, TranslationError> {
    check_responses_to_anthropic(body)?;
    responses_to_anthropic(body).map_err(TranslationError::InvalidInput)
}

// ---------------------------------------------------------------------------
// 请求侧:OpenAI → Anthropic
// ---------------------------------------------------------------------------

/// chat/completions 请求体 → Anthropic Messages body。
/// system/developer → system;assistant.tool_calls → tool_use;role:tool → tool_result;
/// 连续同 role 合并(tool 结果并进 user);`reasoning_effort` 合法档位注入模型名
/// `(effort)` 后缀交给既有管线。
///
/// 这是路由归一化所需的宽松转换，不负责能力决策。真正选择 Translated 候选前
/// 必须先调用 [`check_chat_to_anthropic`]；Native 路径只使用本结果做模型/分流，
/// 上游仍发送原始 body。
pub fn chat_to_anthropic(body: &Value) -> Result<Value, String> {
    let object = body.as_object().ok_or("body is not an object")?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or("missing model")?;

    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let inbound = object
        .get("messages")
        .and_then(Value::as_array)
        .ok_or("missing messages")?;
    for message in inbound {
        let role = message.get("role").and_then(Value::as_str).unwrap_or("");
        match role {
            "system" | "developer" => {
                let text = text_of_content(message.get("content"));
                if !text.is_empty() {
                    system_parts.push(text);
                }
            }
            "user" => {
                let text = text_of_content(message.get("content"));
                if !text.is_empty() {
                    push_block(&mut messages, "user", json!({"type": "text", "text": text}));
                }
            }
            "assistant" => {
                let text = text_of_content(message.get("content"));
                if !text.is_empty() {
                    push_block(
                        &mut messages,
                        "assistant",
                        json!({"type": "text", "text": text}),
                    );
                }
                if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
                    for (offset, call) in calls.iter().enumerate() {
                        if call
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("function")
                            != "function"
                        {
                            continue;
                        }
                        let function = call.get("function").cloned().unwrap_or(Value::Null);
                        let name = function
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if name.is_empty() {
                            continue;
                        }
                        let id = call
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|v| !v.is_empty())
                            .map(str::to_string)
                            .unwrap_or_else(|| format!("toolu_in_{}", offset));
                        push_block(
                            &mut messages,
                            "assistant",
                            json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": parse_tool_arguments(function.get("arguments")),
                            }),
                        );
                    }
                }
            }
            "tool" => {
                let tool_use_id = message
                    .get("tool_call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                push_block(
                    &mut messages,
                    "user",
                    json!({
                        "type": "tool_result",
                        "tool_use_id": tool_use_id,
                        "content": text_of_content(message.get("content")),
                    }),
                );
            }
            _ => {}
        }
    }
    if messages.is_empty() {
        return Err("no convertible messages".into());
    }

    let effort = object.get("reasoning_effort").and_then(Value::as_str);
    let mut out = Map::new();
    out.insert("model".into(), json!(apply_effort_suffix(model, effort)));
    if !system_parts.is_empty() {
        out.insert("system".into(), json!(system_parts.join("\n\n")));
    }
    out.insert("messages".into(), Value::Array(messages));
    out.insert(
        "max_tokens".into(),
        json!(max_tokens_of(
            object,
            &["max_completion_tokens", "max_tokens"]
        )),
    );
    out.insert("stream".into(), json!(true));
    copy_number(object, &mut out, "temperature");
    copy_number(object, &mut out, "top_p");
    if let Some(stops) = stop_sequences_of(object.get("stop")) {
        out.insert("stop_sequences".into(), stops);
    }
    let tools = chat_tools(object.get("tools"));
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
        if let Some(choice) = map_tool_choice(object.get("tool_choice")) {
            out.insert("tool_choice".into(), choice);
        }
    }
    Ok(Value::Object(out))
}

/// Responses 请求体 → Anthropic Messages body。
/// instructions → system;input string/items(message、function_call、
/// function_call_output)→ messages;reasoning 项丢弃(无法回签 thinking);
/// 扁平 function 工具 → anthropic tools;`reasoning.effort` → 模型名后缀。
///
/// 这是路由归一化所需的宽松转换；Translated 候选必须先通过
/// [`check_responses_to_anthropic`]，Native 路径不得被此 checker 拦截。
pub fn responses_to_anthropic(body: &Value) -> Result<Value, String> {
    let object = body.as_object().ok_or("body is not an object")?;
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .ok_or("missing model")?;

    let mut system_parts: Vec<String> = Vec::new();
    if let Some(instructions) = object.get("instructions").and_then(Value::as_str)
        && !instructions.trim().is_empty()
    {
        system_parts.push(instructions.to_string());
    }

    let mut messages: Vec<Value> = Vec::new();
    match object.get("input") {
        Some(Value::String(text)) => {
            if !text.is_empty() {
                push_block(&mut messages, "user", json!({"type": "text", "text": text}));
            }
        }
        Some(Value::Array(items)) => {
            for item in items {
                let item_type = item
                    .get("type")
                    .and_then(Value::as_str)
                    // 无 type 但带 role 的裸消息(OpenAI SDK 简写)按 message 处理。
                    .unwrap_or_else(|| {
                        if item.get("role").is_some() {
                            "message"
                        } else {
                            ""
                        }
                    });
                match item_type {
                    "message" => {
                        let role = item.get("role").and_then(Value::as_str).unwrap_or("");
                        let text = text_of_content(item.get("content"));
                        if text.is_empty() {
                            continue;
                        }
                        match role {
                            "system" | "developer" => system_parts.push(text),
                            "user" | "assistant" => push_block(
                                &mut messages,
                                role,
                                json!({"type": "text", "text": text}),
                            ),
                            _ => {}
                        }
                    }
                    "function_call" => {
                        let name = item.get("name").and_then(Value::as_str).unwrap_or_default();
                        if name.is_empty() {
                            continue;
                        }
                        let id = item
                            .get("call_id")
                            .or_else(|| item.get("id"))
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_block(
                            &mut messages,
                            "assistant",
                            json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": parse_tool_arguments(item.get("arguments")),
                            }),
                        );
                    }
                    "function_call_output" => {
                        let tool_use_id = item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        push_block(
                            &mut messages,
                            "user",
                            json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": text_of_content(item.get("output")),
                            }),
                        );
                    }
                    // reasoning 项没有可回签的 thinking 签名,丢弃。
                    _ => {}
                }
            }
        }
        _ => {}
    }
    if messages.is_empty() {
        return Err("no convertible input".into());
    }

    let effort = object
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(Value::as_str);
    let mut out = Map::new();
    out.insert("model".into(), json!(apply_effort_suffix(model, effort)));
    if !system_parts.is_empty() {
        out.insert("system".into(), json!(system_parts.join("\n\n")));
    }
    out.insert("messages".into(), Value::Array(messages));
    out.insert(
        "max_tokens".into(),
        json!(max_tokens_of(object, &["max_output_tokens"])),
    );
    out.insert("stream".into(), json!(true));
    copy_number(object, &mut out, "temperature");
    copy_number(object, &mut out, "top_p");
    let tools = responses_tools(object.get("tools"));
    if !tools.is_empty() {
        out.insert("tools".into(), Value::Array(tools));
        if let Some(choice) = map_tool_choice(object.get("tool_choice")) {
            out.insert("tool_choice".into(), choice);
        }
    }
    Ok(Value::Object(out))
}

/// content 压平成纯文本:string 原样;数组取 text/input_text/output_text 项拼接。
fn text_of_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("text");
                match part_type {
                    "text" | "input_text" | "output_text" => {
                        part.get("text").and_then(Value::as_str).map(str::to_string)
                    }
                    _ => None,
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// 追加内容块;与上一条同 role 时合并(Anthropic 对连续同 role 消息不保证接受,
/// tool 结果按协议必须落在 user 消息里)。
fn push_block(messages: &mut Vec<Value>, role: &str, block: Value) {
    if let Some(last) = messages.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(content) = last.get_mut("content").and_then(Value::as_array_mut)
    {
        content.push(block);
        return;
    }
    messages.push(json!({"role": role, "content": [block]}));
}

/// 工具入参:JSON 字符串解析成对象;解析失败或非对象一律空对象
/// (Anthropic input 必须是 object)。
fn parse_tool_arguments(arguments: Option<&Value>) -> Value {
    match arguments {
        Some(Value::String(raw)) => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({})),
        Some(value @ Value::Object(_)) => value.clone(),
        _ => json!({}),
    }
}

/// 客户端 effort 参数注入模型名 `(effort)` 后缀(既有管线按后缀分发);
/// 模型名自带合法后缀时客户端参数让位。
fn apply_effort_suffix(model: &str, effort: Option<&str>) -> String {
    let Some(effort) = effort.map(str::trim).and_then(ReasoningEffort::parse) else {
        return model.to_string();
    };
    if model_name::reasoning_effort(model).is_some() {
        return model.to_string();
    }
    format!("{model}({})", effort.as_str())
}

/// Anthropic 必填 max_tokens:按候选键取首个正整数,全缺用 32000
/// (代理不知上游具体上限,取普遍安全值;上游按需自行钳制)。
fn max_tokens_of(object: &Map<String, Value>, keys: &[&str]) -> i64 {
    keys.iter()
        .filter_map(|key| object.get(*key).and_then(Value::as_i64))
        .find(|v| *v > 0)
        .unwrap_or(32000)
}

fn copy_number(from: &Map<String, Value>, to: &mut Map<String, Value>, key: &str) {
    if let Some(value) = from.get(key).and_then(Value::as_f64) {
        to.insert(key.into(), json!(value));
    }
}

fn stop_sequences_of(stop: Option<&Value>) -> Option<Value> {
    let stops: Vec<Value> = match stop {
        Some(Value::String(s)) if !s.is_empty() => vec![json!(s)],
        Some(Value::Array(items)) => items
            .iter()
            .filter(|s| s.as_str().is_some_and(|t| !t.is_empty()))
            .cloned()
            .collect(),
        _ => Vec::new(),
    };
    if stops.is_empty() {
        None
    } else {
        Some(Value::Array(stops))
    }
}

/// chat 嵌套工具声明 → anthropic(仅 function 型)。
fn chat_tools(tools: Option<&Value>) -> Vec<Value> {
    let Some(items) = tools.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|t| t.get("type").and_then(Value::as_str).unwrap_or("function") == "function")
        .filter_map(|t| anthropic_tool(t.get("function").unwrap_or(t)))
        .collect()
}

/// Responses 扁平工具声明 → Anthropic(仅 function 型;内建工具保留给
/// 规划阶段识别，但不进入宽松的 Anthropic 归一化结果)。
fn responses_tools(tools: Option<&Value>) -> Vec<Value> {
    let Some(items) = tools.and_then(Value::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|tool| {
            tool.get("type")
                .and_then(Value::as_str)
                .unwrap_or("function")
                == "function"
        })
        .filter_map(anthropic_tool)
        .collect()
}

fn anthropic_tool(function: &Value) -> Option<Value> {
    let name = function.get("name").and_then(Value::as_str)?;
    if name.is_empty() {
        return None;
    }
    let mut tool = Map::new();
    tool.insert("name".into(), json!(name));
    if let Some(description) = function.get("description").and_then(Value::as_str) {
        tool.insert("description".into(), json!(description));
    }
    let schema = function
        .get("parameters")
        .filter(|p| p.is_object())
        .cloned()
        .unwrap_or_else(|| json!({"type": "object"}));
    tool.insert("input_schema".into(), schema);
    Some(Value::Object(tool))
}

/// tool_choice 映射:auto→auto、required→any、none→none、
/// 指名(chat 嵌套 / responses 扁平)→ {type:tool,name}。
fn map_tool_choice(choice: Option<&Value>) -> Option<Value> {
    match choice? {
        Value::String(s) => match s.as_str() {
            "auto" => Some(json!({"type": "auto"})),
            "required" => Some(json!({"type": "any"})),
            "none" => Some(json!({"type": "none"})),
            _ => None,
        },
        object @ Value::Object(_) => {
            let name = object
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| object.get("name"))
                .and_then(Value::as_str)?;
            Some(json!({"type": "tool", "name": name}))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 响应侧:Anthropic SSE → 客户端方言
// ---------------------------------------------------------------------------

/// 累积中的输出项(anthropic content_block index → 客户端输出项)。
enum OutItem {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        id: String,
        name: String,
        args: String,
        call_index: usize,
    },
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// anthropic stop_reason → OpenAI finish_reason。
fn finish_reason_of(stop_reason: &str) -> &'static str {
    match stop_reason {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        _ => "stop",
    }
}

/// `data: {json}\n\n`(chat 无 event 行)。
fn chat_frame(value: &Value) -> Vec<u8> {
    format!("data: {}\n\n", canonical_json(value)).into_bytes()
}

/// `event: {type}\ndata: {json}\n\n`(Responses 带 event 行,type 同名)。
fn responses_frame(value: &Value) -> Vec<u8> {
    let event = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("message");
    format!("event: {event}\ndata: {}\n\n", canonical_json(value)).into_bytes()
}

// ---------------------------------------------------------------------------
// chat/completions 反向桥
// ---------------------------------------------------------------------------

/// Anthropic SSE → chat.completion.chunk 序列(stream)或 chat.completion 单
/// JSON(非流式聚合)。thinking 块丢弃(chat 无对应字段);tool_use 映射为
/// delta.tool_calls 增量。
pub struct ChatClientBridge {
    stream: bool,
    id: String,
    created: i64,
    model: String,
    buffer: SseBlockBuffer,
    role_sent: bool,
    items: Vec<(i64, OutItem)>,
    tool_count: usize,
    input_tokens: i64,
    output_tokens: i64,
    stop_reason: String,
    finished: bool,
    terminal: BridgeTerminal,
}

impl ChatClientBridge {
    pub fn new(message_id: String, model: String, stream: bool) -> Self {
        Self {
            stream,
            id: format!("chatcmpl_{message_id}"),
            created: unix_now(),
            model,
            buffer: SseBlockBuffer::new(),
            role_sent: false,
            items: Vec::new(),
            tool_count: 0,
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: "end_turn".into(),
            finished: false,
            terminal: BridgeTerminal::Pending,
        }
    }

    fn chunk(&self, delta: Value, finish_reason: Value) -> Value {
        json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish_reason}],
        })
    }

    fn item_mut(&mut self, index: i64) -> Option<&mut OutItem> {
        self.items
            .iter_mut()
            .find(|(i, _)| *i == index)
            .map(|(_, item)| item)
    }

    fn handle_block(&mut self, block: &str) -> Vec<u8> {
        let Some(payload) = data_payload(block) else {
            return Vec::new();
        };
        let Ok(event) = serde_json::from_str::<Value>(&payload) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "error" => {
                out.extend(self.emit_failure("upstream emitted Anthropic error"));
            }
            "message_start" => {
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                if let Some(model) = message.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                if let Some(tokens) = message
                    .get("usage")
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.input_tokens = tokens;
                }
                if self.stream && !self.role_sent {
                    self.role_sent = true;
                    out.extend(chat_frame(
                        &self.chunk(json!({"role": "assistant", "content": ""}), Value::Null),
                    ));
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                match block.get("type").and_then(Value::as_str).unwrap_or("") {
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        let call_index = self.tool_count;
                        self.tool_count += 1;
                        if self.stream {
                            out.extend(chat_frame(&self.chunk(
                                json!({"tool_calls": [{
                                    "index": call_index,
                                    "id": id,
                                    "type": "function",
                                    "function": {"name": name, "arguments": ""},
                                }]}),
                                Value::Null,
                            )));
                        }
                        self.items.push((
                            index,
                            OutItem::Tool {
                                id,
                                name,
                                args: String::new(),
                                call_index,
                            },
                        ));
                    }
                    "thinking" => {
                        self.items.push((
                            index,
                            OutItem::Thinking {
                                text: String::new(),
                            },
                        ));
                    }
                    _ => {
                        let initial = block
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if self.stream && !initial.is_empty() {
                            out.extend(chat_frame(
                                &self.chunk(json!({"content": initial}), Value::Null),
                            ));
                        }
                        self.items.push((index, OutItem::Text { text: initial }));
                    }
                }
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let piece = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if piece.is_empty() {
                            return out;
                        }
                        if let Some(OutItem::Text { text }) = self.item_mut(index) {
                            text.push_str(&piece);
                        }
                        if self.stream {
                            out.extend(chat_frame(
                                &self.chunk(json!({"content": piece}), Value::Null),
                            ));
                        }
                    }
                    "thinking_delta" => {
                        let piece = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if let Some(OutItem::Thinking { text }) = self.item_mut(index) {
                            text.push_str(&piece);
                        }
                    }
                    "input_json_delta" => {
                        let piece = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        if piece.is_empty() {
                            return out;
                        }
                        let mut call = None;
                        if let Some(OutItem::Tool {
                            args, call_index, ..
                        }) = self.item_mut(index)
                        {
                            args.push_str(&piece);
                            call = Some(*call_index);
                        }
                        if let (true, Some(call_index)) = (self.stream, call) {
                            out.extend(chat_frame(&self.chunk(
                                json!({"tool_calls": [{
                                    "index": call_index,
                                    "function": {"arguments": piece},
                                }]}),
                                Value::Null,
                            )));
                        }
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(stop) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = stop.to_string();
                }
                if let Some(tokens) = event
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.output_tokens = tokens;
                }
            }
            "message_stop" => {
                self.terminal = BridgeTerminal::Completed;
                out.extend(self.emit_tail());
            }
            _ => {}
        }
        out
    }

    fn usage_json(&self) -> Value {
        json!({
            "prompt_tokens": self.input_tokens,
            "completion_tokens": self.output_tokens,
            "total_tokens": self.input_tokens + self.output_tokens,
        })
    }

    /// 收尾:stream 出 finish chunk(带 usage)+ [DONE];非流式出完整 chat.completion。
    fn emit_tail(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let finish_reason = finish_reason_of(&self.stop_reason);
        if self.stream {
            let mut final_chunk = self.chunk(json!({}), json!(finish_reason));
            final_chunk["usage"] = self.usage_json();
            let mut out = chat_frame(&final_chunk);
            out.extend(b"data: [DONE]\n\n");
            return out;
        }
        let mut text = String::new();
        let mut tool_calls: Vec<Value> = Vec::new();
        for (_, item) in &self.items {
            match item {
                OutItem::Text { text: piece } => text.push_str(piece),
                OutItem::Thinking { .. } => {}
                OutItem::Tool {
                    id,
                    name,
                    args,
                    call_index,
                } => tool_calls.push(json!({
                    "index": call_index,
                    "id": id,
                    "type": "function",
                    "function": {"name": name, "arguments": args},
                })),
            }
        }
        let mut message = Map::new();
        message.insert("role".into(), json!("assistant"));
        message.insert(
            "content".into(),
            if text.is_empty() {
                Value::Null
            } else {
                json!(text)
            },
        );
        if !tool_calls.is_empty() {
            message.insert("tool_calls".into(), Value::Array(tool_calls));
        }
        let completion = json!({
            "id": self.id,
            "object": "chat.completion",
            "created": self.created,
            "model": self.model,
            "choices": [{
                "index": 0,
                "message": Value::Object(message),
                "finish_reason": finish_reason,
            }],
            "usage": self.usage_json(),
        });
        canonical_json(&completion).into_bytes()
    }

    fn emit_failure(&mut self, detail: &str) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.terminal = BridgeTerminal::Failed(detail.to_string());
        if self.stream {
            chat_frame(&json!({
                "error": {"type": "api_error", "message": detail}
            }))
        } else {
            canonical_json(&json!({
                "error": {"type": "api_error", "message": detail}
            }))
            .into_bytes()
        }
    }
}

impl SseBridge for ChatClientBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for block in self.buffer.push(data) {
            out.extend(self.handle_block(&block));
        }
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(rest) = self.buffer.drain_rest() {
            out.extend(self.handle_block(&rest));
        }
        if matches!(self.terminal, BridgeTerminal::Pending) {
            out.extend(
                self.emit_failure("upstream response stream ended before Anthropic message_stop"),
            );
        } else {
            out.extend(self.emit_tail());
        }
        out
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }
}

// ---------------------------------------------------------------------------
// Responses 反向桥
// ---------------------------------------------------------------------------

/// Anthropic SSE → Responses SSE(stream)或 response 单 JSON(非流式聚合)。
/// text → message/output_text 事件族;tool_use → function_call 项 +
/// arguments 增量;thinking → reasoning 项 + reasoning_summary_text 增量
/// (Codex 以此显示推理摘要)。
pub struct ResponsesClientBridge {
    stream: bool,
    id: String,
    created: i64,
    model: String,
    buffer: SseBlockBuffer,
    created_sent: bool,
    items: Vec<(i64, String, OutItem)>,
    input_tokens: i64,
    output_tokens: i64,
    stop_reason: String,
    finished: bool,
    terminal: BridgeTerminal,
}

impl ResponsesClientBridge {
    pub fn new(message_id: String, model: String, stream: bool) -> Self {
        Self {
            stream,
            id: format!("resp_{message_id}"),
            created: unix_now(),
            model,
            buffer: SseBlockBuffer::new(),
            created_sent: false,
            items: Vec::new(),
            input_tokens: 0,
            output_tokens: 0,
            stop_reason: "end_turn".into(),
            finished: false,
            terminal: BridgeTerminal::Pending,
        }
    }

    fn response_json(&self, status: &str, output: Vec<Value>, with_usage: bool) -> Value {
        let mut response = json!({
            "id": self.id,
            "object": "response",
            "created_at": self.created,
            "status": status,
            "model": self.model,
            "output": output,
        });
        if with_usage {
            response["usage"] = json!({
                "input_tokens": self.input_tokens,
                "output_tokens": self.output_tokens,
                "total_tokens": self.input_tokens + self.output_tokens,
            });
        }
        response
    }

    fn output_index_of(&self, index: i64) -> usize {
        self.items
            .iter()
            .position(|(i, _, _)| *i == index)
            .unwrap_or(self.items.len())
    }

    fn item_json(item_id: &str, item: &OutItem, completed: bool) -> Value {
        let status = if completed {
            "completed"
        } else {
            "in_progress"
        };
        match item {
            OutItem::Text { text } => json!({
                "type": "message",
                "id": item_id,
                "role": "assistant",
                "status": status,
                "content": if completed {
                    json!([{"type": "output_text", "text": text, "annotations": []}])
                } else {
                    json!([])
                },
            }),
            OutItem::Thinking { text } => json!({
                "type": "reasoning",
                "id": item_id,
                "summary": if completed {
                    json!([{"type": "summary_text", "text": text}])
                } else {
                    json!([])
                },
            }),
            OutItem::Tool { id, name, args, .. } => json!({
                "type": "function_call",
                "id": item_id,
                "call_id": id,
                "name": name,
                "arguments": args,
                "status": status,
            }),
        }
    }

    fn handle_block(&mut self, block: &str) -> Vec<u8> {
        let Some(payload) = data_payload(block) else {
            return Vec::new();
        };
        let Ok(event) = serde_json::from_str::<Value>(&payload) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "error" => {
                out.extend(self.emit_failure("upstream emitted Anthropic error"));
            }
            "message_start" => {
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                if let Some(model) = message.get("model").and_then(Value::as_str) {
                    self.model = model.to_string();
                }
                if let Some(tokens) = message
                    .get("usage")
                    .and_then(|u| u.get("input_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.input_tokens = tokens;
                }
                if self.stream && !self.created_sent {
                    self.created_sent = true;
                    out.extend(responses_frame(&json!({
                        "type": "response.created",
                        "response": self.response_json("in_progress", Vec::new(), false),
                    })));
                }
            }
            "content_block_start" => {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                let block = event.get("content_block").cloned().unwrap_or(Value::Null);
                let output_index = self.items.len();
                let (item_id, item) = match block.get("type").and_then(Value::as_str).unwrap_or("")
                {
                    "tool_use" => {
                        let id = block
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        (
                            format!("fc_{}_{output_index}", self.id),
                            OutItem::Tool {
                                id,
                                name: block
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_string(),
                                args: String::new(),
                                call_index: output_index,
                            },
                        )
                    }
                    "thinking" => (
                        format!("rs_{}_{output_index}", self.id),
                        OutItem::Thinking {
                            text: String::new(),
                        },
                    ),
                    _ => (
                        format!("msg_{}_{output_index}", self.id),
                        OutItem::Text {
                            text: block
                                .get("text")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        },
                    ),
                };
                if self.stream {
                    out.extend(responses_frame(&json!({
                        "type": "response.output_item.added",
                        "output_index": output_index,
                        "item": Self::item_json(&item_id, &item, false),
                    })));
                    if let OutItem::Text { .. } = item {
                        out.extend(responses_frame(&json!({
                            "type": "response.content_part.added",
                            "item_id": item_id,
                            "output_index": output_index,
                            "content_index": 0,
                            "part": {"type": "output_text", "text": "", "annotations": []},
                        })));
                    }
                }
                self.items.push((index, item_id, item));
            }
            "content_block_delta" => {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                let delta = event.get("delta").cloned().unwrap_or(Value::Null);
                let output_index = self.output_index_of(index);
                let Some((_, item_id, item)) = self.items.iter_mut().find(|(i, _, _)| *i == index)
                else {
                    return out;
                };
                let item_id = item_id.clone();
                match delta.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        let piece = delta
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if piece.is_empty() {
                            return out;
                        }
                        if let OutItem::Text { text } = item {
                            text.push_str(piece);
                        }
                        if self.stream {
                            out.extend(responses_frame(&json!({
                                "type": "response.output_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "delta": piece,
                            })));
                        }
                    }
                    "thinking_delta" => {
                        let piece = delta
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if piece.is_empty() {
                            return out;
                        }
                        if let OutItem::Thinking { text } = item {
                            text.push_str(piece);
                        }
                        if self.stream {
                            out.extend(responses_frame(&json!({
                                "type": "response.reasoning_summary_text.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "summary_index": 0,
                                "delta": piece,
                            })));
                        }
                    }
                    "input_json_delta" => {
                        let piece = delta
                            .get("partial_json")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if piece.is_empty() {
                            return out;
                        }
                        if let OutItem::Tool { args, .. } = item {
                            args.push_str(piece);
                        }
                        if self.stream {
                            out.extend(responses_frame(&json!({
                                "type": "response.function_call_arguments.delta",
                                "item_id": item_id,
                                "output_index": output_index,
                                "delta": piece,
                            })));
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = event.get("index").and_then(Value::as_i64).unwrap_or(0);
                let output_index = self.output_index_of(index);
                let Some((_, item_id, item)) = self.items.iter().find(|(i, _, _)| *i == index)
                else {
                    return out;
                };
                if self.stream {
                    match item {
                        OutItem::Text { text } => {
                            out.extend(responses_frame(&json!({
                                "type": "response.output_text.done",
                                "item_id": item_id,
                                "output_index": output_index,
                                "content_index": 0,
                                "text": text,
                            })));
                        }
                        OutItem::Tool { args, .. } => {
                            out.extend(responses_frame(&json!({
                                "type": "response.function_call_arguments.done",
                                "item_id": item_id,
                                "output_index": output_index,
                                "arguments": args,
                            })));
                        }
                        OutItem::Thinking { .. } => {}
                    }
                    out.extend(responses_frame(&json!({
                        "type": "response.output_item.done",
                        "output_index": output_index,
                        "item": Self::item_json(item_id, item, true),
                    })));
                }
            }
            "message_delta" => {
                if let Some(stop) = event
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = stop.to_string();
                }
                if let Some(tokens) = event
                    .get("usage")
                    .and_then(|u| u.get("output_tokens"))
                    .and_then(Value::as_i64)
                {
                    self.output_tokens = tokens;
                }
            }
            "message_stop" => {
                self.terminal = BridgeTerminal::Completed;
                out.extend(self.emit_tail());
            }
            _ => {}
        }
        out
    }

    /// 收尾:stream 出 response.completed;非流式出完整 response JSON。
    fn emit_tail(&mut self) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        // max_tokens 截断按 Responses 语义标 incomplete,其余 completed。
        let status = if self.stop_reason == "max_tokens" {
            "incomplete"
        } else {
            "completed"
        };
        let output: Vec<Value> = self
            .items
            .iter()
            .map(|(_, item_id, item)| Self::item_json(item_id, item, true))
            .collect();
        let response = self.response_json(status, output, true);
        if self.stream {
            responses_frame(&json!({
                "type": "response.completed",
                "response": response,
            }))
        } else {
            canonical_json(&response).into_bytes()
        }
    }

    fn emit_failure(&mut self, detail: &str) -> Vec<u8> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        self.terminal = BridgeTerminal::Failed(detail.to_string());
        if self.stream {
            responses_frame(&json!({
                "type": "error",
                "error": {"type": "api_error", "message": detail}
            }))
        } else {
            canonical_json(&json!({
                "type": "error",
                "error": {"type": "api_error", "message": detail}
            }))
            .into_bytes()
        }
    }
}

impl SseBridge for ResponsesClientBridge {
    fn feed(&mut self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for block in self.buffer.push(data) {
            out.extend(self.handle_block(&block));
        }
        out
    }

    fn finish(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(rest) = self.buffer.drain_rest() {
            out.extend(self.handle_block(&rest));
        }
        if matches!(self.terminal, BridgeTerminal::Pending) {
            out.extend(
                self.emit_failure("upstream response stream ended before Anthropic message_stop"),
            );
        } else {
            out.extend(self.emit_tail());
        }
        out
    }

    fn terminal(&self) -> BridgeTerminal {
        self.terminal.clone()
    }
}
