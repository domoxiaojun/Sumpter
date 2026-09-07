//! 客户端可见 SSE 的协议终止跟踪。
//!
//! HTTP body EOF 不是流式协议唯一的完成边界：客户端在收到 Responses
//! `response.completed`、Chat `[DONE]` 或 Anthropic `message_stop` 后可以立即释放
//! body。这里按完整 SSE frame 解析终止事件，供 relay 在客户端正常收尾时提前完成
//! 记账；观察器不改写、缓存或重排实际转发给客户端的字节。

use serde_json::Value;

use crate::events::ResponseUsage;

/// 单个未闭合 SSE frame 的最大观察缓冲。超限只放弃当前 frame 的观察，不影响转发。
const MAX_PENDING_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseDialect {
    Anthropic,
    OpenAiChat,
    OpenAiImages,
    OpenAiResponses,
    Gemini,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseTerminal {
    Completed,
    Incomplete { detail: String },
    Failed { detail: String },
}

pub struct SseTerminalTracker {
    dialect: SseDialect,
    pending: Vec<u8>,
    terminal_seen: bool,
    tool_calls: Vec<String>,
    usage: ResponseUsage,
    stop_reason: Option<String>,
    json_pending: Vec<u8>,
    json_observed: bool,
    observation_truncated: bool,
    gemini_terminal: Option<SseTerminal>,
}

impl SseTerminalTracker {
    pub fn new(dialect: SseDialect) -> Self {
        Self {
            dialect,
            pending: Vec::new(),
            terminal_seen: false,
            tool_calls: Vec::new(),
            usage: ResponseUsage::default(),
            stop_reason: None,
            json_pending: Vec::new(),
            json_observed: false,
            observation_truncated: false,
            gemini_terminal: None,
        }
    }

    /// 本轮流中已经观察到的实际工具调用名称；只来自响应事件，不来自 tools 声明。
    pub fn tool_calls(&self) -> &[String] {
        &self.tool_calls
    }

    pub fn usage(&self) -> Option<ResponseUsage> {
        (!self.usage_is_empty()).then(|| self.usage.clone())
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    /// Gemini may send usage-only frames after finishReason. Observe until
    /// transport EOF so those frames reach the client and the accounting.
    pub fn finish(&mut self) -> Option<SseTerminal> {
        if self.dialect != SseDialect::Gemini {
            return None;
        }
        if !self.pending.is_empty() {
            let _ = self.push(b"\n\n");
        }
        self.gemini_terminal.take()
    }

    /// Whether the bounded non-stream observer had to drop body bytes. This is
    /// intentionally kept as an observer-quality signal rather than inferred
    /// as a successful zero-usage response.
    pub fn observation_truncated(&self) -> bool {
        self.observation_truncated
    }

    /// Observe a non-stream JSON response. The body is bounded and parsed only
    /// for the protocol's documented usage/terminal summary fields.
    pub fn observe_json(&mut self, data: &[u8]) -> Option<SseTerminal> {
        if self.terminal_seen || self.json_observed {
            return None;
        }
        if self.dialect == SseDialect::OpenAiImages {
            return None;
        }
        if self.json_pending.len() >= MAX_PENDING_BYTES {
            self.observation_truncated = true;
            return None;
        }
        let remaining = MAX_PENDING_BYTES - self.json_pending.len();
        let keep = data.len().min(remaining);
        self.json_pending.extend_from_slice(&data[..keep]);
        if keep < data.len() {
            self.observation_truncated = true;
        }

        // Avoid reparsing the complete prefix for every network chunk. A JSON
        // body can only be complete once its last non-whitespace byte closes an
        // object/array; malformed bodies remain observable as truncated/unknown
        // instead of consuming quadratic CPU.
        let last = self
            .json_pending
            .iter()
            .rev()
            .copied()
            .find(|byte| !byte.is_ascii_whitespace())?;
        if !matches!(last, b'}' | b']') {
            return None;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&self.json_pending) else {
            return None;
        };
        self.json_observed = true;
        observe_tool_call_value(self.dialect, &value, &mut self.tool_calls);
        observe_response_summary_value(
            self.dialect,
            &value,
            &mut self.usage,
            &mut self.stop_reason,
        );
        let terminal = classify_json(self.dialect, &value);
        if self.dialect == SseDialect::Gemini {
            self.gemini_terminal = terminal;
            return None;
        }
        if terminal.is_some() {
            self.terminal_seen = true;
            self.pending.clear();
        }
        terminal
    }

    fn usage_is_empty(&self) -> bool {
        self.usage.input_tokens.is_none()
            && self.usage.output_tokens.is_none()
            && self.usage.cache_read_input_tokens.is_none()
            && self.usage.cache_creation_input_tokens.is_none()
            && self.usage.reasoning_tokens.is_none()
    }

    /// 喂入客户端实际可见的原始字节。只在完整 SSE frame 中解析 `data:` JSON，
    /// 因此正文里出现 `response.completed` 字样不会误触发。
    pub fn push(&mut self, data: &[u8]) -> Option<SseTerminal> {
        if self.terminal_seen {
            return None;
        }
        self.pending.extend_from_slice(data);

        while let Some((frame_end, boundary_len)) = event_boundary(&self.pending) {
            let frame = String::from_utf8_lossy(&self.pending[..frame_end])
                .replace("\r\n", "\n")
                .replace('\r', "\n");
            self.pending.drain(..frame_end + boundary_len);
            let Some(payload) = data_payload(&frame) else {
                continue;
            };
            observe_tool_call(self.dialect, &payload, &mut self.tool_calls);
            observe_response_summary(
                self.dialect,
                &payload,
                &mut self.usage,
                &mut self.stop_reason,
            );
            if let Some(terminal) = classify(self.dialect, &payload)
                .or_else(|| classify_event_name(self.dialect, frame_event_name(&frame)))
            {
                if self.dialect == SseDialect::Gemini {
                    if !matches!(
                        self.gemini_terminal,
                        Some(SseTerminal::Failed { .. } | SseTerminal::Incomplete { .. })
                    ) {
                        self.gemini_terminal = Some(terminal);
                    }
                    continue;
                }
                self.terminal_seen = true;
                self.pending.clear();
                return Some(terminal);
            }
        }

        // 恶意或异常上游可能永远不结束一个巨大 frame。观察器必须有界；清空只会
        // 跳过该异常 frame，后续独立终止 frame 仍可被识别。
        if self.pending.len() > MAX_PENDING_BYTES {
            self.pending.clear();
        }
        None
    }
}

fn observe_response_summary(
    dialect: SseDialect,
    payload: &str,
    usage: &mut ResponseUsage,
    stop_reason: &mut Option<String>,
) {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    observe_response_summary_value(dialect, &value, usage, stop_reason);
}

fn observe_response_summary_value(
    dialect: SseDialect,
    value: &Value,
    usage: &mut ResponseUsage,
    stop_reason: &mut Option<String>,
) {
    let usage_value = match dialect {
        SseDialect::Anthropic => value
            .get("usage")
            .or_else(|| value.pointer("/message/usage")),
        SseDialect::OpenAiResponses => value
            .get("usage")
            .or_else(|| value.pointer("/response/usage")),
        SseDialect::Gemini => value.get("usageMetadata"),
        SseDialect::OpenAiChat => value.get("usage"),
        SseDialect::OpenAiImages => None,
    };
    if let Some(value) = usage_value {
        merge_max(
            &mut usage.input_tokens,
            value,
            &["input_tokens", "prompt_tokens"],
        );
        merge_max(
            &mut usage.output_tokens,
            value,
            &["output_tokens", "completion_tokens"],
        );
        merge_max(
            &mut usage.cache_read_input_tokens,
            value,
            &[
                "cache_read_input_tokens",
                "cache_read_tokens",
                "cached_tokens",
            ],
        );
        merge_nested_max(
            &mut usage.cache_read_input_tokens,
            value,
            &[
                "/input_tokens_details/cached_tokens",
                "/input_tokens_details/cache_read_tokens",
                "/prompt_tokens_details/cached_tokens",
                "/prompt_tokens_details/cache_read_tokens",
            ],
        );
        if dialect == SseDialect::Gemini {
            merge_max(&mut usage.input_tokens, value, &["promptTokenCount"]);
            let output = value.get("candidatesTokenCount").and_then(Value::as_u64);
            let thoughts = value.get("thoughtsTokenCount").and_then(Value::as_u64);
            if output.is_some() || thoughts.is_some() {
                merge_token_count(
                    &mut usage.output_tokens,
                    output.unwrap_or(0).saturating_add(thoughts.unwrap_or(0)),
                );
            }
            merge_max(&mut usage.reasoning_tokens, value, &["thoughtsTokenCount"]);
            merge_max(
                &mut usage.cache_read_input_tokens,
                value,
                &["cachedContentTokenCount"],
            );
        }
        merge_max(
            &mut usage.cache_creation_input_tokens,
            value,
            &[
                "cache_creation_input_tokens",
                "cache_write_tokens",
                "cache_creation_tokens",
                "cached_creation_input_tokens",
                "cached_creation_tokens",
            ],
        );
        merge_nested_max(
            &mut usage.cache_creation_input_tokens,
            value,
            &[
                "/input_tokens_details/cache_write_tokens",
                "/input_tokens_details/cache_creation_tokens",
                "/prompt_tokens_details/cache_write_tokens",
                "/prompt_tokens_details/cache_creation_tokens",
                "/prompt_tokens_details/cached_creation_tokens",
            ],
        );
        merge_nested_sum_max(
            &mut usage.cache_creation_input_tokens,
            value,
            "/cache_creation",
            &["ephemeral_5m_input_tokens", "ephemeral_1h_input_tokens"],
        );
        merge_nested_max(
            &mut usage.reasoning_tokens,
            value,
            &[
                "/output_tokens_details/reasoning_tokens",
                "/output_tokens_details/thinking_tokens",
                "/completion_tokens_details/reasoning_tokens",
                "/completion_tokens_details/thinking_tokens",
                "/thinking_tokens",
            ],
        );
    }
    if let Some(next) = value.get("thinking_tokens").and_then(Value::as_u64) {
        merge_token_count(&mut usage.reasoning_tokens, next);
    }
    let candidate = match dialect {
        SseDialect::Anthropic => value
            .pointer("/delta/stop_reason")
            .or_else(|| value.get("stop_reason"))
            .and_then(Value::as_str),
        SseDialect::OpenAiChat => value
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str),
        SseDialect::OpenAiResponses => value
            .pointer("/response/incomplete_details/reason")
            .or_else(|| value.pointer("/incomplete_details/reason"))
            .and_then(Value::as_str),
        SseDialect::Gemini => value
            .pointer("/candidates/0/finishReason")
            .and_then(Value::as_str),
        SseDialect::OpenAiImages => None,
    };
    if let Some(candidate) = candidate
        .map(str::trim)
        .filter(|candidate| !candidate.is_empty() && candidate.chars().count() <= 80)
    {
        *stop_reason = Some(candidate.to_string());
    }
}

fn classify_json(dialect: SseDialect, value: &Value) -> Option<SseTerminal> {
    match dialect {
        SseDialect::Anthropic => {
            if value.get("error").is_some()
                || value.get("type").and_then(Value::as_str) == Some("error")
            {
                return Some(SseTerminal::Failed {
                    detail: failed_detail(value, "anthropic response error"),
                });
            }
            // `type: message` alone is not a completion marker for a partial
            // unary body; require an explicit terminal stop_reason.
            value
                .get("stop_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty())
                .map(|_| SseTerminal::Completed)
        }
        SseDialect::OpenAiChat => {
            if value.get("error").is_some() {
                return Some(SseTerminal::Failed {
                    detail: failed_detail(value, "chat response error"),
                });
            }
            let choice = value.pointer("/choices/0")?;
            let finish = choice.get("finish_reason").and_then(Value::as_str);
            finish.map(|reason| {
                if matches!(reason, "length" | "content_filter" | "error") {
                    SseTerminal::Incomplete {
                        detail: format!("chat response incomplete ({reason})"),
                    }
                } else {
                    SseTerminal::Completed
                }
            })
        }
        SseDialect::OpenAiResponses => {
            let status = value.get("status").and_then(Value::as_str);
            let status =
                status.or_else(|| value.pointer("/response/status").and_then(Value::as_str));
            match status {
                Some("failed") => Some(SseTerminal::Failed {
                    detail: failed_detail(value, "response.failed"),
                }),
                Some("incomplete") => Some(SseTerminal::Incomplete {
                    detail: incomplete_detail(value),
                }),
                Some("completed") => Some(SseTerminal::Completed),
                _ if value.get("error").is_some() => Some(SseTerminal::Failed {
                    detail: failed_detail(value, "responses response error"),
                }),
                _ => None,
            }
        }
        SseDialect::Gemini => classify_gemini(value),
        SseDialect::OpenAiImages => None,
    }
}

fn merge_max(slot: &mut Option<u64>, value: &Value, keys: &[&str]) {
    for key in keys {
        if let Some(next) = value.get(*key).and_then(Value::as_u64) {
            merge_token_count(slot, next);
        }
    }
}

fn merge_nested_max(slot: &mut Option<u64>, value: &Value, pointers: &[&str]) {
    for pointer in pointers {
        if let Some(next) = value.pointer(pointer).and_then(Value::as_u64) {
            merge_token_count(slot, next);
        }
    }
}

fn merge_nested_sum_max(slot: &mut Option<u64>, value: &Value, pointer: &str, keys: &[&str]) {
    let Some(object) = value.pointer(pointer).and_then(Value::as_object) else {
        return;
    };
    let total = keys.iter().fold(0_u64, |total, key| {
        total.saturating_add(object.get(*key).and_then(Value::as_u64).unwrap_or(0))
    });
    if total > 0 || keys.iter().any(|key| object.contains_key(*key)) {
        merge_token_count(slot, total);
    }
}

fn merge_token_count(slot: &mut Option<u64>, next: u64) {
    let next = next.min(i64::MAX as u64);
    *slot = Some(slot.map_or(next, |current| current.max(next)));
}

const MAX_TOOL_CALLS: usize = 32;
const MAX_TOOL_NAME_CHARS: usize = 160;

fn observe_tool_call(dialect: SseDialect, payload: &str, calls: &mut Vec<String>) {
    let Ok(value) = serde_json::from_str::<Value>(payload) else {
        return;
    };
    observe_tool_call_value(dialect, &value, calls);
}

fn observe_tool_call_value(dialect: SseDialect, value: &Value, calls: &mut Vec<String>) {
    let candidates = match dialect {
        SseDialect::OpenAiResponses => responses_tool_names(value),
        SseDialect::OpenAiChat => chat_tool_names(value),
        SseDialect::OpenAiImages => Vec::new(),
        SseDialect::Anthropic => anthropic_tool_names(value),
        SseDialect::Gemini => value
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .flat_map(|candidate| {
                candidate
                    .pointer("/content/parts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter_map(|part| {
                part.pointer("/functionCall/name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
    };
    for name in candidates {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > MAX_TOOL_NAME_CHARS
            || calls.iter().any(|existing| existing == name)
        {
            continue;
        }
        if calls.len() >= MAX_TOOL_CALLS {
            break;
        }
        calls.push(name.to_string());
    }
}

fn responses_tool_names(value: &Value) -> Vec<String> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let items: Vec<&Value> = match event_type {
        "response.output_item.added" | "response.output_item.done" => {
            value.get("item").into_iter().collect()
        }
        // Some Codex-compatible upstreams only populate the terminal response.output
        // array. Observability must not depend on optional intermediate item events.
        "response.completed" => value
            .pointer("/response/output")
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default(),
        _ => value
            .get("output")
            .or_else(|| value.pointer("/response/output"))
            .and_then(Value::as_array)
            .map(|items| items.iter().collect())
            .unwrap_or_default(),
    };
    items
        .into_iter()
        .flat_map(response_item_tool_names)
        .collect()
}

fn response_item_tool_names(item: &Value) -> Vec<String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or_default();
    match item_type {
        "function_call" | "custom_tool_call" => item
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .into_iter()
            .collect(),
        "mcp_call" | "mcp_tool_call" => {
            let name = item.get("name").and_then(Value::as_str);
            let server = item
                .get("server_label")
                .or_else(|| item.get("serverLabel"))
                .and_then(Value::as_str);
            match (server, name) {
                (Some(server), Some(name)) => vec![format!("mcp:{server}/{name}")],
                (None, Some(name)) => vec![format!("mcp:{name}")],
                _ => Vec::new(),
            }
        }
        "local_shell_call" => vec!["shell_command".into()],
        "shell_call" => vec!["shell".into()],
        "web_search_call" => vec!["web_search".into()],
        "file_search_call" => vec!["file_search".into()],
        "code_interpreter_call" => vec!["code_interpreter".into()],
        "tool_search_call" => vec!["tool_search".into()],
        "image_generation_call" => vec!["image_generation".into()],
        "computer_call" => vec!["computer".into()],
        "apply_patch_call" => vec!["apply_patch".into()],
        "skills_call" => vec!["skills".into()],
        // Future built-in Responses tools follow the *_call item convention. Keep
        // the label bounded by the common observer limits instead of silently losing it.
        other if other.ends_with("_call") => item
            .get("name")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| other.strip_suffix("_call").map(str::to_string))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

fn chat_tool_names(value: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for delta in value
        .get("choices")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|choice| choice.get("delta"))
    {
        names.extend(
            delta
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|call| {
                    call.pointer("/function/name")
                        .or_else(|| call.get("name"))
                        .and_then(Value::as_str)
                        .map(str::to_string)
                }),
        );
        if let Some(name) = delta.pointer("/function_call/name").and_then(Value::as_str) {
            names.push(name.to_string());
        }
    }
    for call in value
        .pointer("/choices/0/message/tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = call
            .pointer("/function/name")
            .or_else(|| call.get("name"))
            .and_then(Value::as_str)
        {
            names.push(name.to_string());
        }
    }
    if let Some(name) = value
        .pointer("/choices/0/message/function_call/name")
        .and_then(Value::as_str)
    {
        names.push(name.to_string());
    }
    names
}

fn anthropic_tool_names(value: &Value) -> Vec<String> {
    let mut names = Vec::new();
    if value.get("type").and_then(Value::as_str) == Some("content_block_start") {
        let block = value.get("content_block").unwrap_or(value);
        if matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use" | "mcp_tool_use")
        ) && let Some(name) = block.get("name").and_then(Value::as_str)
        {
            names.push(name.to_string());
        }
    }
    for block in value
        .get("content")
        .or_else(|| value.pointer("/message/content"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if matches!(
            block.get("type").and_then(Value::as_str),
            Some("tool_use" | "server_tool_use" | "mcp_tool_use")
        ) && let Some(name) = block.get("name").and_then(Value::as_str)
        {
            names.push(name.to_string());
        }
    }
    names
}

/// 找到两个连续 SSE 行结束符，兼容 LF、CRLF 与 CR。
fn event_boundary(data: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    while index < data.len() {
        let first = line_ending_len(data, index);
        if first == 0 {
            index += 1;
            continue;
        }
        let second = line_ending_len(data, index + first);
        if second > 0 {
            return Some((index, first + second));
        }
        index += first;
    }
    None
}

fn line_ending_len(data: &[u8], index: usize) -> usize {
    match data.get(index) {
        Some(b'\n') => 1,
        Some(b'\r') if data.get(index + 1) == Some(&b'\n') => 2,
        Some(b'\r') => 1,
        _ => 0,
    }
}

fn data_payload(frame: &str) -> Option<String> {
    let parts: Vec<&str> = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect();
    (!parts.is_empty()).then(|| parts.join("\n"))
}

fn frame_event_name(frame: &str) -> Option<&str> {
    frame
        .lines()
        .find_map(|line| line.strip_prefix("event:").map(str::trim))
}

fn classify(dialect: SseDialect, payload: &str) -> Option<SseTerminal> {
    if dialect == SseDialect::OpenAiChat && payload.trim() == "[DONE]" {
        return Some(SseTerminal::Completed);
    }

    let value: Value = serde_json::from_str(payload).ok()?;
    let event_type = value.get("type").and_then(Value::as_str);
    match dialect {
        SseDialect::OpenAiResponses => match event_type {
            // Some Responses-compatible upstreams encode a failed or incomplete
            // terminal response as `response.completed` with the real status
            // nested under `response.status`. Treat those statuses as terminal
            // failures instead of counting the HTTP 200 stream as successful.
            Some("response.completed") => Some(classify_responses_completed(&value)),
            Some("response.incomplete") => Some(SseTerminal::Incomplete {
                detail: incomplete_detail(&value),
            }),
            Some("response.failed") | Some("error") => Some(SseTerminal::Failed {
                detail: failed_detail(&value, "response.failed"),
            }),
            _ => None,
        },
        SseDialect::OpenAiChat => value.get("error").map(|_| SseTerminal::Failed {
            detail: failed_detail(&value, "chat stream error"),
        }),
        SseDialect::OpenAiImages => match event_type {
            Some("image_generation.completed" | "image_edit.completed") => {
                Some(SseTerminal::Completed)
            }
            Some("image_generation.failed" | "image_edit.failed" | "error") => {
                Some(SseTerminal::Failed {
                    detail: failed_detail(&value, "image stream error"),
                })
            }
            _ => None,
        },
        SseDialect::Anthropic => match event_type {
            Some("message_stop") => Some(SseTerminal::Completed),
            Some("error") => Some(SseTerminal::Failed {
                detail: failed_detail(&value, "anthropic stream error"),
            }),
            _ => None,
        },
        SseDialect::Gemini => classify_gemini(&value),
    }
}

fn classify_gemini(value: &Value) -> Option<SseTerminal> {
    if value.get("error").is_some() {
        return Some(SseTerminal::Failed {
            detail: "gemini response error".into(),
        });
    }
    if value
        .pointer("/promptFeedback/blockReason")
        .and_then(Value::as_str)
        .is_some_and(|reason| !matches!(reason, "" | "BLOCK_REASON_UNSPECIFIED"))
    {
        return Some(SseTerminal::Incomplete {
            detail: "gemini prompt blocked".into(),
        });
    }
    // CountTokens and EmbedContent are complete unary response contracts.
    if value.get("totalTokens").is_some_and(Value::is_u64)
        || value
            .pointer("/embedding/values")
            .is_some_and(Value::is_array)
    {
        return Some(SseTerminal::Completed);
    }
    let candidates = value.get("candidates")?.as_array()?;
    if candidates.is_empty() {
        return None;
    }
    for candidate in candidates {
        let reason = candidate.get("finishReason")?.as_str()?;
        if matches!(reason, "" | "FINISH_REASON_UNSPECIFIED") {
            return None;
        }
        if reason != "STOP" {
            return Some(SseTerminal::Incomplete {
                detail: format!(
                    "gemini response incomplete ({})",
                    safe_token(reason).unwrap_or_else(|| "unknown".into())
                ),
            });
        }
    }
    Some(SseTerminal::Completed)
}

fn classify_responses_completed(value: &Value) -> SseTerminal {
    let status = value.get("status").and_then(Value::as_str);
    let status = status.or_else(|| value.pointer("/response/status").and_then(Value::as_str));
    match status {
        Some("failed") => SseTerminal::Failed {
            detail: failed_detail(value, "response.failed"),
        },
        Some("incomplete") => SseTerminal::Incomplete {
            detail: incomplete_detail(value),
        },
        _ => SseTerminal::Completed,
    }
}

fn classify_event_name(dialect: SseDialect, event_name: Option<&str>) -> Option<SseTerminal> {
    match (dialect, event_name) {
        (SseDialect::Anthropic, Some("message_stop")) => Some(SseTerminal::Completed),
        (SseDialect::OpenAiImages, Some("image_generation.completed" | "image_edit.completed")) => {
            Some(SseTerminal::Completed)
        }
        (
            SseDialect::OpenAiImages,
            Some("image_generation.failed" | "image_edit.failed" | "error"),
        ) => Some(SseTerminal::Failed {
            detail: "upstream emitted image stream error".into(),
        }),
        _ => None,
    }
}

fn incomplete_detail(value: &Value) -> String {
    let reason = value
        .pointer("/response/incomplete_details/reason")
        .or_else(|| value.pointer("/response/incompleteDetails/reason"))
        .and_then(Value::as_str)
        .and_then(safe_token);
    reason.map_or_else(
        || "upstream emitted response.incomplete".into(),
        |reason| format!("upstream emitted response.incomplete ({reason})"),
    )
}

fn failed_detail(value: &Value, fallback: &str) -> String {
    let error = value
        .pointer("/response/error")
        .or_else(|| value.get("error"));
    let code = error
        .and_then(|error| error.get("code").or_else(|| error.get("type")))
        .and_then(Value::as_str)
        .and_then(safe_token);
    code.map_or_else(
        || format!("upstream emitted {fallback}"),
        |code| format!("upstream emitted {fallback} ({code})"),
    )
}

/// 只保留短错误码/原因，不把任意上游文本、URL 或凭据写入 stats。
fn safe_token(raw: &str) -> Option<String> {
    let token = raw.trim();
    (!token.is_empty()
        && token.chars().count() <= 80
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.')))
    .then(|| token.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_completed_survives_arbitrary_chunks_and_crlf() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            tracker.push(b"event: response.completed\r\ndata: {\"type\":\"response.comp"),
            None
        );
        assert_eq!(
            tracker.push(b"leted\",\"response\":{\"status\":\"completed\"}}\r"),
            None
        );
        assert_eq!(tracker.push(b"\n\r\n"), Some(SseTerminal::Completed));
        assert_eq!(
            tracker.push(b"data: {\"type\":\"response.failed\"}\n\n"),
            None
        );
    }

    #[test]
    fn responses_text_cannot_spoof_terminal_event() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            tracker.push(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"response.completed\"}\n\n"
            ),
            None
        );
    }

    #[test]
    fn responses_records_actual_tool_items_only() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(tracker.push(b"data: {\"type\":\"response.created\",\"tools\":[{\"type\":\"function\",\"name\":\"not_called\"}]}\n\n"), None);
        assert!(tracker.tool_calls().is_empty());
        assert_eq!(tracker.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"name\":\"shell_command\"}}\n\n"), None);
        assert_eq!(tracker.push(b"data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"local_shell_call\"}}\n\n"), None);
        assert_eq!(tracker.tool_calls(), ["shell_command"]);
    }

    #[test]
    fn responses_labels_mcp_and_web_tools() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        tracker.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"mcp_tool_call\",\"server_label\":\"files\",\"name\":\"read\"}}\n\n");
        tracker.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"web_search_call\"}}\n\n");
        assert_eq!(tracker.tool_calls(), ["mcp:files/read", "web_search"]);
    }

    #[test]
    fn responses_recognizes_builtin_and_terminal_only_tool_items() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        tracker.push(b"data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"file_search_call\"}}\n\n");
        tracker.push(b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"mcp_call\",\"server_label\":\"repo\",\"name\":\"read\"}}\n\n");
        assert_eq!(
            tracker.push(b"data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"type\":\"apply_patch_call\"},{\"type\":\"code_interpreter_call\"}]}}\n\n"),
            Some(SseTerminal::Completed)
        );
        assert_eq!(
            tracker.tool_calls(),
            [
                "file_search",
                "mcp:repo/read",
                "apply_patch",
                "code_interpreter"
            ]
        );
    }

    #[test]
    fn image_api_stream_uses_its_own_terminal_events() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiImages);
        assert_eq!(
            tracker.push(b"event: image_edit.partial_image\ndata: {\"type\":\"image_edit.partial_image\"}\n\n"),
            None
        );
        assert_eq!(
            tracker.push(
                b"event: image_edit.completed\ndata: {\"type\":\"image_edit.completed\"}\n\n"
            ),
            Some(SseTerminal::Completed)
        );
    }

    #[test]
    fn responses_failure_and_incomplete_keep_bounded_details() {
        let mut failed = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            failed.push(
                b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"secret text\"}}}\n\n"
            ),
            Some(SseTerminal::Failed {
                detail: "upstream emitted response.failed (server_error)".into(),
            })
        );

        let mut incomplete = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            incomplete.push(
                b"data: {\"type\":\"response.incomplete\",\"response\":{\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n"
            ),
            Some(SseTerminal::Incomplete {
                detail: "upstream emitted response.incomplete (max_output_tokens)".into(),
            })
        );
    }

    #[test]
    fn responses_completed_honors_nested_failure_status() {
        let mut failed = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            failed.push(
                br#"data: {"type":"response.completed","response":{"status":"failed","error":{"code":"server_error"}}}

"#,
            ),
            Some(SseTerminal::Failed {
                detail: "upstream emitted response.failed (server_error)".into(),
            })
        );

        let mut incomplete = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            incomplete.push(
                br#"data: {"type":"response.completed","response":{"status":"incomplete","incomplete_details":{"reason":"max_output_tokens"}}}

"#,
            ),
            Some(SseTerminal::Incomplete {
                detail: "upstream emitted response.incomplete (max_output_tokens)".into(),
            })
        );

        let mut null_top_level = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            null_top_level.push(
                br#"data: {"type":"response.completed","status":null,"response":{"status":"failed","error":{"code":"server_error"}}}

"#,
            ),
            Some(SseTerminal::Failed {
                detail: "upstream emitted response.failed (server_error)".into(),
            })
        );
    }

    #[test]
    fn chat_and_anthropic_use_their_protocol_terminators() {
        let mut chat = SseTerminalTracker::new(SseDialect::OpenAiChat);
        assert_eq!(chat.push(b"data: [DONE]\n\n"), Some(SseTerminal::Completed));

        let mut anthropic = SseTerminalTracker::new(SseDialect::Anthropic);
        assert_eq!(
            anthropic.push(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"),
            Some(SseTerminal::Completed)
        );
        let mut envelope_only = SseTerminalTracker::new(SseDialect::Anthropic);
        assert_eq!(
            envelope_only.push(b"event: message_stop\ndata: {}\n\n"),
            Some(SseTerminal::Completed)
        );
    }

    #[test]
    fn records_anthropic_usage_and_stop_reason_without_body_text() {
        let mut tracker = SseTerminalTracker::new(SseDialect::Anthropic);
        tracker.push(b"data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":4}}}\n\n");
        assert_eq!(tracker.push(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"tool_use\"},\"usage\":{\"output_tokens\":7}}\n\n"), None);
        let usage = tracker.usage().expect("usage");
        assert_eq!(usage.input_tokens, Some(12));
        assert_eq!(usage.output_tokens, Some(7));
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(tracker.stop_reason(), Some("tool_use"));
    }

    #[test]
    fn records_non_stream_json_usage_for_all_conversation_dialects() {
        let mut anthropic = SseTerminalTracker::new(SseDialect::Anthropic);
        anthropic.observe_json(
            br#"{"usage":{"input_tokens":11,"output_tokens":5},"stop_reason":"end_turn"}"#,
        );
        assert_eq!(anthropic.usage().unwrap().input_tokens, Some(11));
        assert_eq!(anthropic.stop_reason(), Some("end_turn"));

        let mut chat = SseTerminalTracker::new(SseDialect::OpenAiChat);
        chat.observe_json(br#"{"usage":{"prompt_tokens":13,"completion_tokens":6},"choices":[{"finish_reason":"stop"}]}"#);
        assert_eq!(chat.usage().unwrap().output_tokens, Some(6));
        assert_eq!(chat.stop_reason(), Some("stop"));

        let mut responses = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        responses.observe_json(br#"{"usage":{"input_tokens":17,"output_tokens":8},"incomplete_details":{"reason":"max_output_tokens"}}"#);
        assert_eq!(responses.usage().unwrap().input_tokens, Some(17));
        assert_eq!(responses.stop_reason(), Some("max_output_tokens"));
    }

    #[test]
    fn records_cache_usage_aliases_and_explicit_zero() {
        let mut anthropic = SseTerminalTracker::new(SseDialect::Anthropic);
        anthropic.observe_json(
            br#"{"usage":{"input_tokens":11,"output_tokens":5,"cache_read_input_tokens":4,"cache_creation_input_tokens":2}}"#,
        );
        let usage = anthropic.usage().expect("anthropic usage");
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(usage.cache_creation_input_tokens, Some(2));

        let mut nested_anthropic = SseTerminalTracker::new(SseDialect::Anthropic);
        nested_anthropic
            .observe_json(br#"{"usage":{"cache_creation":{"ephemeral_5m_input_tokens":13503}}}"#);
        assert_eq!(
            nested_anthropic
                .usage()
                .expect("nested anthropic usage")
                .cache_creation_input_tokens,
            Some(13503)
        );

        let mut chat = SseTerminalTracker::new(SseDialect::OpenAiChat);
        chat.observe_json(
            br#"{"usage":{"prompt_tokens":13,"completion_tokens":6,"prompt_tokens_details":{"cached_tokens":4,"cached_creation_tokens":2}}}"#,
        );
        let usage = chat.usage().expect("chat usage");
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(usage.cache_creation_input_tokens, Some(2));

        let mut responses = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        responses.observe_json(
            br#"{"response":{"usage":{"input_tokens":17,"output_tokens":8,"input_tokens_details":{"cached_tokens":5,"cache_write_tokens":3}}}}"#,
        );
        let usage = responses.usage().expect("responses usage");
        assert_eq!(usage.cache_read_input_tokens, Some(5));
        assert_eq!(usage.cache_creation_input_tokens, Some(3));

        let mut explicit_zero = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        explicit_zero.observe_json(
            br#"{"usage":{"input_tokens":1,"input_tokens_details":{"cache_write_tokens":0}}}"#,
        );
        assert_eq!(
            explicit_zero
                .usage()
                .expect("explicit zero usage")
                .cache_creation_input_tokens,
            Some(0)
        );
    }

    #[test]
    fn records_openai_cache_usage_aliases_in_streams() {
        let mut responses = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            responses.push(
                br#"data: {"type":"response.completed","response":{"usage":{"input_tokens":17,"output_tokens":8,"input_tokens_details":{"cached_tokens":5,"cache_write_tokens":3}}}}

"#,
            ),
            Some(SseTerminal::Completed)
        );
        let usage = responses.usage().expect("responses stream usage");
        assert_eq!(usage.cache_read_input_tokens, Some(5));
        assert_eq!(usage.cache_creation_input_tokens, Some(3));

        let mut chat = SseTerminalTracker::new(SseDialect::OpenAiChat);
        assert_eq!(
            chat.push(
                br#"data: {"choices":[{"delta":{},"finish_reason":null}],"usage":{"prompt_tokens":13,"completion_tokens":6,"prompt_tokens_details":{"cached_tokens":4,"cached_creation_tokens":2}}}

"#,
            ),
            None
        );
        chat.push(b"data: [DONE]\n\n");
        let usage = chat.usage().expect("chat stream usage");
        assert_eq!(usage.cache_read_input_tokens, Some(4));
        assert_eq!(usage.cache_creation_input_tokens, Some(2));
    }

    #[test]
    fn records_non_stream_tool_calls_status_and_thinking_aliases() {
        let mut responses = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        assert_eq!(
            responses.observe_json(
                br#"{"status":"failed","error":{"code":"server_error"},"output":[{"type":"function_call","name":"exec"}],"usage":{"input_tokens":3,"output_tokens":2,"output_tokens_details":{"thinking_tokens":1}}}"#,
            ),
            Some(SseTerminal::Failed {
                detail: "upstream emitted response.failed (server_error)".into(),
            })
        );
        assert_eq!(responses.tool_calls(), ["exec"]);
        assert_eq!(responses.usage().unwrap().reasoning_tokens, Some(1));

        let mut chat = SseTerminalTracker::new(SseDialect::OpenAiChat);
        assert_eq!(
            chat.observe_json(
                br#"{"choices":[{"message":{"tool_calls":[{"function":{"name":"shell"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":2}}"#,
            ),
            Some(SseTerminal::Completed)
        );
        assert_eq!(chat.tool_calls(), ["shell"]);

        let mut anthropic = SseTerminalTracker::new(SseDialect::Anthropic);
        assert_eq!(
            anthropic.observe_json(
                br#"{"type":"message","stop_reason":"tool_use","content":[{"type":"tool_use","name":"read"}],"usage":{"input_tokens":1,"output_tokens":1,"thinking_tokens":2}}"#,
            ),
            Some(SseTerminal::Completed)
        );
        assert_eq!(anthropic.tool_calls(), ["read"]);
        assert_eq!(anthropic.usage().unwrap().reasoning_tokens, Some(2));
    }

    #[test]
    fn anthropic_non_stream_message_requires_stop_reason() {
        let mut tracker = SseTerminalTracker::new(SseDialect::Anthropic);
        assert_eq!(
            tracker.observe_json(br#"{"type":"message","content":[]}"#),
            None
        );
    }

    #[test]
    fn marks_non_stream_observation_when_body_exceeds_bound() {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiChat);
        tracker.observe_json(&vec![b' '; MAX_PENDING_BYTES + 1]);
        assert!(tracker.observation_truncated());
        assert!(tracker.usage().is_none());
    }
}
