//! 跨协议工具契约：发现状态、参数约束与结果引用共用同一套规则。

use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use crate::bridge::TranslationError;
use crate::config::ProviderProtocol;
use crate::routing::{RoutingRequest, inspector};

pub(crate) fn reference_text(block: &Value) -> Option<String> {
    (block.get("type").and_then(Value::as_str) == Some("tool_reference"))
        .then(|| block.get("tool_name").and_then(Value::as_str))
        .flatten()
        .map(|name| format!("Tool reference: {name}"))
}

/// 只有成功的用户工具结果能加载延迟工具；模型自己的文字或失败调用不授权加载。
fn loaded_names(request: &RoutingRequest) -> HashSet<&str> {
    let mut names = HashSet::new();
    for message in &request.messages {
        if message.role != "user" {
            continue;
        }
        for result in message.content.as_array().into_iter().flatten() {
            if result.get("type").and_then(Value::as_str) != Some("tool_result")
                || result.get("is_error").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            for block in result
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if block.get("type").and_then(Value::as_str) == Some("tool_reference")
                    && let Some(name) = block.get("tool_name").and_then(Value::as_str)
                {
                    names.insert(name);
                }
            }
        }
    }
    names
}

pub(crate) fn active_tools(request: &RoutingRequest) -> Vec<&Map<String, Value>> {
    let loaded = loaded_names(request);
    request
        .tools
        .iter()
        .filter(|tool| {
            inspector::is_client_tool_type(tool.get("type").and_then(Value::as_str).unwrap_or(""))
                && (tool.get("defer_loading").and_then(Value::as_bool) != Some(true)
                    || tool
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| loaded.contains(name)))
        })
        .collect()
}

pub(crate) fn validate_tools(
    request: &RoutingRequest,
    target: ProviderProtocol,
) -> Result<(), TranslationError> {
    let mut names = HashSet::new();
    for (index, tool) in request.tools.iter().enumerate() {
        if !inspector::is_client_tool_type(tool.get("type").and_then(Value::as_str).unwrap_or("")) {
            continue;
        }
        let path = format!("tools[{index}]");
        let name = tool
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                TranslationError::InvalidInput(format!("{path}.name must not be empty"))
            })?;
        if !names.insert(name) {
            return Err(TranslationError::InvalidInput(format!(
                "{path}.name must be unique"
            )));
        }
        for field in ["strict", "defer_loading"] {
            if tool.get(field).is_some_and(|value| !value.is_boolean()) {
                return Err(TranslationError::InvalidInput(format!(
                    "{path}.{field} must be a boolean"
                )));
            }
        }
        if tool
            .get("description")
            .is_some_and(|value| !value.is_string())
        {
            return Err(TranslationError::InvalidInput(format!(
                "{path}.description must be a string"
            )));
        }
        if let Some(schema) = tool.get("input_schema") {
            if !schema.is_object()
                || schema
                    .get("type")
                    .is_some_and(|kind| kind.as_str() != Some("object"))
            {
                return Err(TranslationError::InvalidInput(format!(
                    "{path}.input_schema must describe an object"
                )));
            }
        }
        if target == ProviderProtocol::Gemini
            && tool.get("strict").and_then(Value::as_bool) == Some(true)
        {
            return Err(TranslationError::UnsupportedField(format!("{path}.strict")));
        }
        if let Some(callers) = tool.get("allowed_callers") {
            if callers
                .as_array()
                .is_none_or(|items| items.len() != 1 || items[0].as_str() != Some("direct"))
            {
                return Err(TranslationError::UnsupportedField(format!(
                    "{path}.allowed_callers"
                )));
            }
        }
    }
    for (message_index, message) in request.messages.iter().enumerate() {
        for (result_index, result) in message.content.as_array().into_iter().flatten().enumerate() {
            if result.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            for (index, block) in result
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .enumerate()
            {
                if block.get("type").and_then(Value::as_str) != Some("tool_reference") {
                    continue;
                }
                if message.role != "user"
                    || block
                        .get("tool_name")
                        .and_then(Value::as_str)
                        .is_none_or(|name| !names.contains(name))
                {
                    return Err(TranslationError::InvalidInput(format!(
                        "messages[{message_index}].content[{result_index}].content[{index}].tool_name must reference a declared client tool in a user tool result"
                    )));
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn validate_choice(request: &RoutingRequest) -> Result<(), TranslationError> {
    let Some(choice) = request.raw.get("tool_choice") else {
        return Ok(());
    };
    let choice = choice
        .as_object()
        .ok_or_else(|| TranslationError::InvalidInput("tool_choice must be an object".into()))?;
    for key in choice.keys() {
        if !matches!(key.as_str(), "type" | "name" | "disable_parallel_tool_use") {
            return Err(TranslationError::UnsupportedField(format!(
                "tool_choice.{key}"
            )));
        }
    }
    let kind = choice
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(kind, "auto" | "any" | "none" | "tool") {
        return Err(TranslationError::UnsupportedField(
            "tool_choice.type".into(),
        ));
    }
    if choice
        .get("disable_parallel_tool_use")
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(TranslationError::InvalidInput(
            "tool_choice.disable_parallel_tool_use must be a boolean".into(),
        ));
    }
    if kind == "tool" {
        let name = choice
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| TranslationError::InvalidInput("tool_choice.name is required".into()))?;
        let active = active_tools(request);
        if !active
            .iter()
            .any(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            && !request.tools.iter().any(|tool| {
                !inspector::is_client_tool_type(
                    tool.get("type").and_then(Value::as_str).unwrap_or(""),
                ) && tool.get("name").and_then(Value::as_str) == Some(name)
            })
        {
            return Err(TranslationError::InvalidInput(
                "tool_choice.name must name an available tool".into(),
            ));
        }
    } else if choice.contains_key("name") {
        return Err(TranslationError::UnsupportedField(
            "tool_choice.name".into(),
        ));
    }
    if kind == "any"
        && active_tools(request).is_empty()
        && request.tools.iter().all(|tool| {
            inspector::is_client_tool_type(tool.get("type").and_then(Value::as_str).unwrap_or(""))
        })
    {
        return Err(TranslationError::InvalidInput(
            "tool_choice requires an available tool".into(),
        ));
    }
    Ok(())
}

pub(crate) fn parallel_tool_calls(request: &RoutingRequest) -> Option<bool> {
    request
        .raw
        .get("tool_choice")?
        .get("disable_parallel_tool_use")?
        .as_bool()
        .map(|disabled| !disabled)
}

/// 不修补或伪造上游参数；只有完整 JSON 对象才能作为可执行工具调用结束。
pub(crate) fn parse_arguments(arguments: &str) -> Result<Value, &'static str> {
    serde_json::from_str::<Value>(arguments)
        .ok()
        .filter(Value::is_object)
        .ok_or("upstream tool arguments must be a complete JSON object")
}

#[derive(Default)]
struct AnthropicCall {
    arguments: String,
    initial_complete: bool,
    closed: bool,
}

/// 三种客户端反向桥共用：校验关联及完整性，并把 start.input 统一成参数增量。
#[derive(Default)]
pub(crate) struct AnthropicToolStream {
    indexes: HashSet<u64>,
    call_ids: HashSet<String>,
    calls: HashMap<u64, AnthropicCall>,
}

impl AnthropicToolStream {
    pub(crate) fn normalize(&mut self, mut event: Value) -> Result<Vec<Value>, &'static str> {
        let mut prefix = Vec::new();
        match event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "content_block_start" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .filter(|index| *index <= i64::MAX as u64)
                    .ok_or("invalid Anthropic content block index")?;
                if !self.indexes.insert(index) {
                    return Err("duplicate Anthropic content block index");
                }
                let block = &event["content_block"];
                if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                    return Ok(vec![event]);
                }
                let id = block
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                    .ok_or("upstream tool call is missing its id")?;
                if !self.call_ids.insert(id.to_string()) {
                    return Err("duplicate Anthropic tool call id");
                }
                if block
                    .get("name")
                    .and_then(Value::as_str)
                    .is_none_or(|name| name.is_empty())
                {
                    return Err("upstream tool call is missing its name");
                }
                let input = block
                    .get("input")
                    .and_then(Value::as_object)
                    .ok_or("upstream tool input must be a JSON object")?;
                if self.calls.len() >= 4096 {
                    return Err("Anthropic tool call count exceeds limit");
                }
                let arguments = if input.is_empty() {
                    String::new()
                } else {
                    Value::Object(input.clone()).to_string()
                };
                let initial_complete = !arguments.is_empty();
                self.calls.insert(
                    index,
                    AnthropicCall {
                        arguments: arguments.clone(),
                        initial_complete,
                        closed: false,
                    },
                );
                if initial_complete {
                    event["content_block"]["input"] = serde_json::json!({});
                    return Ok(vec![event, Self::delta(index, &arguments)]);
                }
            }
            "content_block_delta"
                if event.pointer("/delta/type").and_then(Value::as_str)
                    == Some("input_json_delta") =>
            {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .filter(|index| *index <= i64::MAX as u64)
                    .ok_or("invalid Anthropic tool argument index")?;
                let call = self
                    .calls
                    .get_mut(&index)
                    .ok_or("Anthropic tool arguments have no matching call")?;
                let fragment = event
                    .pointer("/delta/partial_json")
                    .and_then(Value::as_str)
                    .ok_or("Anthropic tool argument delta must be a string")?;
                if call.closed || call.initial_complete {
                    return Err("Anthropic tool arguments changed after completion");
                }
                call.arguments.push_str(fragment);
            }
            "content_block_stop" => {
                let index = event
                    .get("index")
                    .and_then(Value::as_u64)
                    .filter(|index| *index <= i64::MAX as u64)
                    .ok_or("invalid Anthropic content block index")?;
                if let Some(call) = self.calls.get_mut(&index) {
                    if call.closed {
                        return Err("duplicate Anthropic tool block stop");
                    }
                    if call.arguments.is_empty() {
                        // 已验证 start.input 是 {}；无增量时它就是合法的无参数调用。
                        call.arguments = "{}".into();
                        prefix.push(Self::delta(index, "{}"));
                    }
                    parse_arguments(&call.arguments)?;
                    call.closed = true;
                }
            }
            "message_stop" if self.calls.values().any(|call| !call.closed) => {
                return Err("upstream message ended with an unclosed tool call");
            }
            _ => {}
        }
        prefix.push(event);
        Ok(prefix)
    }

    fn delta(index: u64, arguments: &str) -> Value {
        serde_json::json!({"type": "content_block_delta", "index": index,
            "delta": {"type": "input_json_delta", "partial_json": arguments}})
    }
}
