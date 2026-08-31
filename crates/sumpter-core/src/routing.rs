//! 路由:请求指纹识别(RequestInspector)、复合会话粘性键(StickyKey/AffinityHasher)、
//! 路由规划(RoutePlanner)。对齐 Swift `Routing.swift`,行为注释以 Swift 版为准绳。

use serde_json::{Map, Value};

use crate::config::{
    AppConfig, ContextMode, Endpoint, EndpointProtocolMode, FeatureRule, ProviderProtocol,
    RequestKind, ThinkingMode,
};
use crate::model_name;

// ---------------------------------------------------------------------------
// 请求模型
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct RoutingMessage {
    pub role: String,
    pub content: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutingRequest {
    pub model: String,
    pub system: Option<Value>,
    pub messages: Vec<RoutingMessage>,
    pub tools: Vec<Map<String, Value>>,
    pub raw: Map<String, Value>,
}

impl RoutingRequest {
    /// 从请求 body 的 JSON 构建;非 object 返回 None(引擎回 400 invalid_request)。
    pub fn from_value(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let parsed = model_name::parse(object.get("model").and_then(Value::as_str).unwrap_or(""));
        let model = match parsed.effort {
            Some(effort) => format!("{}({})", parsed.base_name, effort.as_str()),
            None => parsed.base_name,
        };
        let system = object.get("system").cloned();
        let messages = object
            .get("messages")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let obj = item.as_object()?;
                        Some(RoutingMessage {
                            role: obj
                                .get("role")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                            content: obj.get("content").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let tools = object
            .get("tools")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_object().cloned())
                    .collect()
            })
            .unwrap_or_default();
        let mut raw = object.clone();
        raw.insert("model".into(), Value::String(model.clone()));
        Some(Self {
            model,
            system,
            messages,
            tools,
            raw,
        })
    }
}

/// 请求用途,仅用于日志/统计/排障,不参与路由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RequestPurpose {
    #[serde(rename = "standard")]
    Standard,
    #[serde(rename = "session_title")]
    SessionTitle,
    #[serde(rename = "websearch")]
    WebSearch,
    #[serde(rename = "webfetch")]
    WebFetch,
    #[serde(rename = "classifier")]
    Classifier,
    #[serde(rename = "compact")]
    Compact,
    #[serde(rename = "image_generation")]
    ImageGeneration,
    #[serde(rename = "image_edit")]
    ImageEdit,
    #[serde(rename = "alpha_search")]
    AlphaSearch,
    #[serde(rename = "token_count")]
    TokenCount,
}

// ---------------------------------------------------------------------------
// RequestInspector:Claude Code 独立子请求的严格指纹
// ---------------------------------------------------------------------------

pub mod inspector {
    use super::*;

    const CLAUDE_CODE_IDENTITY: &str = "You are Claude Code, Anthropic's official CLI for Claude.";
    const WEB_SEARCH_SYSTEM: &str = "You are an assistant for performing a web search tool use";
    const WEB_SEARCH_MESSAGE_PREFIX: &str = "Perform a web search for the query: ";
    const WEB_FETCH_PREFIX: &str = "Web page content:\n---\n";
    const WEB_FETCH_CONCISE_SUFFIX: &str = "Provide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed.";
    const WEB_FETCH_RESTRICTED_MARKER: &str =
        "Provide a concise response based only on the content above. In your response:";
    const WEB_FETCH_RESTRICTED_SUFFIX: &str = "- Never produce or reproduce exact song lyrics.";
    const CLASSIFIER_SYSTEM: &str = "You are a security monitor for autonomous AI coding agents.";
    const SESSION_TITLE_SYSTEM_PREFIX: &str = "Write the title in ";
    const SESSION_TITLE_SYSTEM_SUFFIX: &str =
        "Keep technical terms and code identifiers in their original form.";

    pub fn system_text(request: &RoutingRequest) -> String {
        text_from_system(request.system.as_ref())
    }

    pub fn first_user_text(request: &RoutingRequest) -> String {
        let Some(message) = request.messages.iter().find(|m| m.role == "user") else {
            return String::new();
        };
        if let Some(text) = message.content.as_str() {
            return text.to_string();
        }
        python_style_json_string(&message.content)
    }

    /// 工具类型前缀判定:存在目标前缀工具、且**不存在** type 为空的客户端工具。
    pub fn has_tool_type(request: &RoutingRequest, prefix: &str) -> bool {
        if prefix.is_empty() {
            return false;
        }
        let mut has_target = false;
        let mut has_client_tool = false;
        for tool in &request.tools {
            let tool_type = tool.get("type").and_then(Value::as_str).unwrap_or("");
            if tool_type.starts_with(prefix) {
                has_target = true;
            } else if tool_type.is_empty() {
                has_client_tool = true;
            }
        }
        has_target && !has_client_tool
    }

    /// 任一消息前 800 字符(大小写不敏感)包含 needle。
    pub fn messages_contain(request: &RoutingRequest, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        let lowered = needle.to_lowercase();
        request.messages.iter().any(|message| {
            content_text(&message.content, Some(800))
                .to_lowercase()
                .contains(&lowered)
        })
    }

    /// 严格识别 Claude Code 内建三类独立请求;未命中或同时命中多个 → None
    /// (由普通模型路由接管,不依赖规则排列顺序)。
    pub fn detected_request_kind(request: &RoutingRequest) -> Option<RequestKind> {
        let matches: Vec<RequestKind> = [
            matches_web_search(request).then_some(RequestKind::WebSearch),
            matches_web_fetch(request).then_some(RequestKind::WebFetch),
            matches_classifier(request).then_some(RequestKind::Classifier),
        ]
        .into_iter()
        .flatten()
        .collect();
        if matches.len() == 1 {
            Some(matches[0])
        } else {
            None
        }
    }

    /// 用途标签(与路由解耦;标题生成不触发任何特征分流)。
    pub fn request_purpose(request: &RoutingRequest) -> RequestPurpose {
        let feature_kind = detected_request_kind(request);
        let is_session_title = matches_session_title(request);

        // 异常请求同时长得像标题生成和内建分流时不猜测,按普通请求展示。
        if is_session_title {
            return if feature_kind.is_none() {
                RequestPurpose::SessionTitle
            } else {
                RequestPurpose::Standard
            };
        }
        match feature_kind {
            Some(RequestKind::WebSearch) => RequestPurpose::WebSearch,
            Some(RequestKind::WebFetch) => RequestPurpose::WebFetch,
            Some(RequestKind::Classifier) => RequestPurpose::Classifier,
            None => RequestPurpose::Standard,
        }
    }

    /// 「形似 CC 内部辅助请求,却没命中任何已知指纹」:带专用 system、无 tools、
    /// 仅单条 user 消息,且用途判为 Standard。
    ///
    /// 只陈述形状,不猜用途。判据逐条都在排除已知的正常形状:
    /// - 无 tools + 单条 user:CC 主对话恒定带 tools 且多轮;
    /// - system 非空:排除裸 curl / 简易客户端的探测请求;
    /// - system 不含 CC 身份标识:主对话的 system 必含它,内部辅助请求各有专用 system
    ///   (`Write the title in …`、`You are a security monitor …` 等)。
    ///
    /// 剩下的就是「带专用 system 的单轮无工具请求,却谁也没匹配上」——几乎必然是
    /// CC 升级后指纹失配的内部请求。存在的意义就是把这种失配变成可见信号:否则
    /// `matches_session_title` 等会静默退化成 Standard,无人察觉。
    pub fn is_unmatched_no_tools(request: &RoutingRequest) -> bool {
        if !request.tools.is_empty()
            || request.messages.len() != 1
            || request.messages[0].role != "user"
        {
            return false;
        }
        let system = system_text(request);
        !system.trim().is_empty()
            && !system.contains(CLAUDE_CODE_IDENTITY)
            && request_purpose(request) == RequestPurpose::Standard
    }

    /// 分流规则匹配:enabled、各条件 AND、且至少存在一个条件。
    pub fn feature_rule_matches(rule: &FeatureRule, request: &RoutingRequest) -> bool {
        if !rule.enabled {
            return false;
        }
        let mut condition_count = 0;
        if let Some(kind) = rule.match_.request_kind {
            condition_count += 1;
            if detected_request_kind(request) != Some(kind) {
                return false;
            }
        }
        if let Some(prefix) = rule
            .match_
            .tool_type_prefix
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            condition_count += 1;
            if !has_tool_type(request, prefix) {
                return false;
            }
        }
        if let Some(needle) = rule
            .match_
            .system_contains
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            condition_count += 1;
            if !system_text(request)
                .to_lowercase()
                .contains(&needle.to_lowercase())
            {
                return false;
            }
        }
        if let Some(needle) = rule
            .match_
            .messages_contain
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            condition_count += 1;
            if !messages_contain(request, needle) {
                return false;
            }
        }
        if let Some(expected) = rule
            .match_
            .model_equals
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            condition_count += 1;
            if model_name::clean(&request.model) != model_name::clean(expected) {
                return false;
            }
        }
        condition_count > 0
    }

    fn matches_web_search(request: &RoutingRequest) -> bool {
        if request.messages.len() != 1 || request.messages[0].role != "user" {
            return false;
        }
        let Some(message) = strict_text(&request.messages[0].content) else {
            return false;
        };
        normalized_newlines(&message)
            .trim()
            .starts_with(WEB_SEARCH_MESSAGE_PREFIX)
            && system_text(request).contains(WEB_SEARCH_SYSTEM)
            && request.tools.len() == 1
            && request.tools[0].get("name").and_then(Value::as_str) == Some("web_search")
            && request.tools[0]
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|t| t.starts_with("web_search"))
            && forced_tool_choice(request, "web_search")
    }

    fn matches_web_fetch(request: &RoutingRequest) -> bool {
        if !request.tools.is_empty()
            || request.messages.len() != 1
            || request.messages[0].role != "user"
            || !system_text(request).contains(CLAUDE_CODE_IDENTITY)
        {
            return false;
        }
        let Some(raw_text) = strict_text(&request.messages[0].content) else {
            return false;
        };
        let normalized = normalized_newlines(&raw_text);
        let text = normalized.trim();
        let Some(content_and_tail) = text.strip_prefix(WEB_FETCH_PREFIX) else {
            return false;
        };
        if !content_and_tail.contains("\n---\n") {
            return false;
        }
        text.ends_with(WEB_FETCH_CONCISE_SUFFIX)
            || (text.contains(WEB_FETCH_RESTRICTED_MARKER)
                && text.ends_with(WEB_FETCH_RESTRICTED_SUFFIX))
    }

    fn matches_classifier(request: &RoutingRequest) -> bool {
        if !system_text(request).contains(CLASSIFIER_SYSTEM) {
            return false;
        }
        let Some(last) = request.messages.last() else {
            return false;
        };
        if last.role != "user" {
            return false;
        }
        let Some(raw_text) = strict_text(&last.content) else {
            return false;
        };
        let normalized = normalized_newlines(&raw_text);
        let text = normalized.trim();
        if !text.starts_with("<transcript>\n") || !text.contains("\n</transcript>") {
            return false;
        }

        if request.tools.is_empty() {
            // 当前 XML fast/thinking 两阶段:stage 1 可带 stop_sequences,stage 2 不带。
            return match request.raw.get("stop_sequences") {
                None => true,
                Some(Value::Array(values)) => values
                    .iter()
                    .any(|v| v.as_str() == Some("</block>") || v.as_str() == Some("</severity>")),
                Some(_) => false,
            };
        }
        // 兼容旧单阶段分类器:只暴露 classify_result 并强制调用。
        request.tools.len() == 1
            && request.tools[0].get("name").and_then(Value::as_str) == Some("classify_result")
            && forced_tool_choice(request, "classify_result")
    }

    /// Claude Code 2.1.x 自动会话标题请求:专用 system、单条 `<session>` 消息、无工具。
    /// 不看模型名,避免把用户主动选择的模型主请求误标成内部辅助请求。
    fn matches_session_title(request: &RoutingRequest) -> bool {
        if !request.tools.is_empty()
            || request.messages.len() != 1
            || request.messages[0].role != "user"
        {
            return false;
        }
        let Some(raw_text) = strict_text(&request.messages[0].content) else {
            return false;
        };
        let system = normalized_newlines(&system_text(request));
        if !system.contains(SESSION_TITLE_SYSTEM_PREFIX)
            || !system.contains(SESSION_TITLE_SYSTEM_SUFFIX)
        {
            return false;
        }
        let normalized = normalized_newlines(&raw_text);
        let text = normalized.trim();
        text.starts_with("<session>") && text.ends_with("</session>")
    }

    fn forced_tool_choice(request: &RoutingRequest, name: &str) -> bool {
        let Some(Value::Object(choice)) = request.raw.get("tool_choice") else {
            return false;
        };
        choice.get("type").and_then(Value::as_str) == Some("tool")
            && choice.get("name").and_then(Value::as_str) == Some(name)
    }

    /// 严格请求指纹只接受纯字符串或纯 text block;tool_result 及嵌套 content
    /// 只供自定义 messagesContain 使用,不能参与内建识别。
    fn strict_text(value: &Value) -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Array(blocks) => {
                if blocks.is_empty() {
                    return None;
                }
                let mut parts = Vec::with_capacity(blocks.len());
                for block in blocks {
                    let object = block.as_object()?;
                    let block_type = object.get("type").and_then(Value::as_str).unwrap_or("text");
                    if block_type != "text" {
                        return None;
                    }
                    parts.push(object.get("text")?.as_str()?.to_string());
                }
                Some(parts.join("\n"))
            }
            _ => None,
        }
    }

    pub(super) fn normalized_newlines(value: &str) -> String {
        value.replace("\r\n", "\n").replace('\r', "\n")
    }

    fn text_from_system(value: Option<&Value>) -> String {
        match value {
            None => String::new(),
            Some(Value::String(text)) => text.clone(),
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter_map(|block| block.as_object()?.get("text")?.as_str().map(str::to_string))
                .collect::<Vec<_>>()
                .join("\n"),
            Some(_) => String::new(),
        }
    }

    /// 消息内容渲染为可搜索文本:text 直取,嵌套 content 递归,块间空格连接;
    /// `prefix_limit` 按字符截断(Swift 为 grapheme,极端 emoji 情形可忽略差异)。
    pub(super) fn content_text(value: &Value, prefix_limit: Option<usize>) -> String {
        let rendered = match value {
            Value::String(raw) => raw.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .map(|block| {
                    if let Some(object) = block.as_object() {
                        if let Some(direct) = object.get("text").and_then(Value::as_str) {
                            return direct.to_string();
                        }
                        if let Some(nested) = object.get("content") {
                            return content_text(nested, None);
                        }
                    }
                    String::new()
                })
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        match prefix_limit {
            Some(limit) => rendered.chars().take(limit).collect(),
            None => rendered,
        }
    }
}

/// Python 风格 JSON 序列化(对齐 Swift `JSONValue.pythonStyleJSONString`):
/// 对象键序 = 偏好键(type/text/content/role/id/name/input/tool_use_id 中存在者)+ 其余字母序;
/// `, ` 分隔;整值数字无小数点;字符串按 JSON 引号(不转义斜杠)。
pub fn python_style_json_string(value: &Value) -> String {
    match value {
        Value::String(s) => quoted(s),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.0e15 {
                    return format!("{}", f as i64);
                }
                return format!("{f}");
            }
            n.to_string()
        }
        Value::Bool(b) => if *b { "true" } else { "false" }.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(values) => {
            let inner: Vec<String> = values.iter().map(python_style_json_string).collect();
            format!("[{}]", inner.join(", "))
        }
        Value::Object(object) => {
            const PREFERRED: [&str; 8] = [
                "type",
                "text",
                "content",
                "role",
                "id",
                "name",
                "input",
                "tool_use_id",
            ];
            let preferred_keys: Vec<&str> = PREFERRED
                .iter()
                .copied()
                .filter(|k| object.contains_key(*k))
                .collect();
            let mut rest: Vec<&str> = object
                .keys()
                .map(String::as_str)
                .filter(|k| !preferred_keys.contains(k))
                .collect();
            rest.sort_unstable();
            let body: Vec<String> = preferred_keys
                .into_iter()
                .chain(rest)
                .map(|key| {
                    let rendered = object
                        .get(key)
                        .map(python_style_json_string)
                        .unwrap_or_else(|| "null".to_string());
                    format!("{}: {}", quoted(key), rendered)
                })
                .collect();
            format!("{{{}}}", body.join(", "))
        }
    }
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

// ---------------------------------------------------------------------------
// 复合会话粘性键与摘要
// ---------------------------------------------------------------------------

pub mod sticky {
    use super::*;
    use md5::{Digest, Md5};
    use sha2::Sha256;

    const STICKY_V3_DOMAIN: &[u8] = b"sumpter-sticky-v3";
    const STABLE_SESSION_TAG: u8 = 0x01;
    const CONTENT_FINGERPRINT_TAG: u8 = 0x02;
    const OPTIONAL_NONE_TAG: u8 = 0x00;
    const OPTIONAL_SOME_TAG: u8 = 0x01;

    #[derive(Clone, PartialEq, Eq)]
    pub enum SessionIdentity {
        StableSession(String),
        ContentFingerprint(String),
    }

    impl SessionIdentity {
        fn into_parts(self) -> (SessionSource, String) {
            match self {
                Self::StableSession(value) => (SessionSource::StableSession, value),
                Self::ContentFingerprint(value) => (SessionSource::ContentFingerprint, value),
            }
        }

        pub fn persistent(&self) -> bool {
            matches!(self, Self::StableSession(_))
        }
    }

    impl std::fmt::Debug for SessionIdentity {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            let variant = match self {
                Self::StableSession(_) => "StableSession",
                Self::ContentFingerprint(_) => "ContentFingerprint",
            };
            formatter.debug_tuple(variant).field(&"<redacted>").finish()
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SessionSource {
        StableSession,
        ContentFingerprint,
    }

    impl SessionSource {
        fn tag(self) -> u8 {
            match self {
                Self::StableSession => STABLE_SESSION_TAG,
                Self::ContentFingerprint => CONTENT_FINGERPRINT_TAG,
            }
        }

        fn persistent(self) -> bool {
            matches!(self, Self::StableSession)
        }
    }

    #[derive(Clone, PartialEq, Eq)]
    pub struct StickyKey {
        /// 单次摘要构造所需的会话身份；稳定 ID 原文不得写日志、事件或持久化文件。
        pub session_id: String,
        pub effective_model: String,
        pub feature_rule_id: Option<String>,
        source: SessionSource,
    }

    impl StickyKey {
        pub fn new(
            identity: SessionIdentity,
            effective_model: impl Into<String>,
            feature_rule_id: Option<String>,
        ) -> Self {
            let (source, session_id) = identity.into_parts();
            Self {
                session_id,
                effective_model: effective_model.into(),
                feature_rule_id,
                source,
            }
        }

        /// v3 规范编码（不再包含 Provider 池维度）:
        /// domain || source-tag || len+session || len+model || option-tag [+ len+rule].
        /// 长度统一按 UTF-8 字节数写入 u64 大端，避免字段边界及 Unicode 长度歧义。
        pub fn affinity_id(&self) -> String {
            let mut hasher = Sha256::new();
            hasher.update(STICKY_V3_DOMAIN);
            hasher.update([self.source.tag()]);
            update_length_prefixed(&mut hasher, &self.session_id);
            update_length_prefixed(&mut hasher, &self.effective_model);
            match self.feature_rule_id.as_deref() {
                None => hasher.update([OPTIONAL_NONE_TAG]),
                Some(rule_id) => {
                    hasher.update([OPTIONAL_SOME_TAG]);
                    update_length_prefixed(&mut hasher, rule_id);
                }
            }
            hasher
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect()
        }

        pub fn persistent(&self) -> bool {
            self.source.persistent()
        }

        pub fn session_key(self) -> SessionKey {
            SessionKey {
                value: self.affinity_id(),
                persistent: self.persistent(),
            }
        }
    }

    impl std::fmt::Debug for StickyKey {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter
                .debug_struct("StickyKey")
                .field("session_id", &"<redacted>")
                .field("effective_model", &self.effective_model)
                .field("feature_rule_id", &self.feature_rule_id)
                .field("source", &self.source)
                .finish()
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct SessionKey {
        /// 只保存摘要，绝不把 Claude 会话 ID / metadata.user_id 原文带入调度状态。
        pub value: String,
        /// 真实会话标识可跨上下文压缩和进程重启持久化；旧内容指纹只维持兼容。
        pub persistent: bool,
    }

    fn md5_hex(value: &str) -> String {
        let mut hasher = Md5::new();
        hasher.update(value.as_bytes());
        hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// 临时内容指纹 = md5hex(system 前 4000 字符 + "|" + 首条 user 文本前 2000 字符)。
    pub fn session_key(request: &RoutingRequest) -> String {
        let system: String = inspector::system_text(request)
            .chars()
            .take(4_000)
            .collect();
        let first_user: String = inspector::first_user_text(request)
            .chars()
            .take(2_000)
            .collect();
        md5_hex(&format!("{system}|{first_user}"))
    }

    pub fn resolve_session_identity(
        request: &RoutingRequest,
        claude_session_id: Option<&str>,
    ) -> SessionIdentity {
        claude_session_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| SessionIdentity::StableSession(value.to_string()))
            .unwrap_or_else(|| SessionIdentity::ContentFingerprint(session_key(request)))
    }

    /// 命名兼容:解析结果是会话身份，不是最终复合粘性摘要。
    pub fn resolved_session_identity(
        request: &RoutingRequest,
        claude_session_id: Option<&str>,
    ) -> SessionIdentity {
        resolve_session_identity(request, claude_session_id)
    }

    pub fn resolved_sticky_key(
        identity: SessionIdentity,
        effective_model: &str,
        feature_rule_id: Option<&str>,
    ) -> SessionKey {
        StickyKey::new(
            identity,
            effective_model,
            feature_rule_id.map(str::to_string),
        )
        .session_key()
    }

    /// 旧调用方兼容入口；新调度必须在 RoutePlan 之后调用 `resolved_sticky_key`。
    pub fn resolved_session_key(
        request: &RoutingRequest,
        claude_session_id: Option<&str>,
    ) -> SessionKey {
        let identity = resolve_session_identity(request, claude_session_id);
        resolved_sticky_key(identity, "", None)
    }

    fn update_length_prefixed(hasher: &mut Sha256, value: &str) {
        let bytes = value.as_bytes();
        let length = u64::try_from(bytes.len()).expect("sticky key field exceeds u64 length");
        hasher.update(length.to_be_bytes());
        hasher.update(bytes);
    }
}

// ---------------------------------------------------------------------------
// 路由规划
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RouteMode {
    Native,
    Translated,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlannedEndpoint {
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub base_url: String,
    /// 配置中声明的四态入口模式。
    pub configured_protocol: EndpointProtocolMode,
    /// 入站路径确定的真实 SourceFormat。
    pub source_format: ProviderProtocol,
    /// 本次请求已解析出的真实 TargetFormat；绝不包含 Auto。
    pub protocol: ProviderProtocol,
    pub route_mode: RouteMode,
    /// 路由规则选中的逻辑模型名；用于按模型家族做协议兼容，不能从上游别名反推。
    pub routed_model: String,
    pub upstream_model: String,
    pub pinned_ips: Vec<String>,
    pub pinned_ip_exclusive: bool,
    /// Provider 优先级：数值越小越优先；同组保持配置顺序。
    pub priority: i64,
    /// 同组入口共享会话粘性；None = 使用自身 id 作为独立组。
    pub sticky_group: Option<String>,
    pub thinking: ThinkingMode,
    pub context: ContextMode,
    /// 分流规则指定的 effort；Some 时优先于客户端请求。
    pub effort_override: Option<model_name::ReasoningEffort>,
    pub failover_timeout_seconds: Option<f64>,
    /// 【实验】出站连接复用(Endpoint.keepAlive 透传)。
    pub keep_alive: bool,
}

impl PlannedEndpoint {
    /// 会话调度的分组键:没设粘性分组的入口用自己的 id 作为独立组。
    pub fn scheduling_group(&self) -> &str {
        self.sticky_group.as_deref().unwrap_or(&self.endpoint_id)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RoutePlan {
    pub client_model: String,
    pub effective_model: String,
    pub feature_rule_id: Option<String>,
    pub endpoints: Vec<PlannedEndpoint>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RoutePlanError {
    #[error("no Provider accepts model {0}")]
    NoProviderForModel(String),
    #[error("Provider not found: {0}")]
    ProviderNotFound(String),
    #[error("no enabled Provider")]
    NoEnabledProvider,
    #[error("no compatible Provider for {source_format}")]
    NoCompatibleProvider { source_format: String },
}

pub struct RoutePlanner;

impl RoutePlanner {
    /// 无既有会话归属时的确定性入口顺序：按分组最低 priority 排序，
    /// 同级按首次出现位置；组内保持配置顺序。
    pub fn order_endpoints(endpoints: &[PlannedEndpoint]) -> Vec<PlannedEndpoint> {
        let mut groups: Vec<(String, i64, usize)> = Vec::new();
        for (index, endpoint) in endpoints.iter().enumerate() {
            let group = endpoint.scheduling_group().to_string();
            if let Some(existing) = groups.iter_mut().find(|item| item.0 == group) {
                existing.1 = existing.1.min(endpoint.priority);
            } else {
                groups.push((group, endpoint.priority, index));
            }
        }
        groups.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.2.cmp(&b.2)));
        groups
            .iter()
            .flat_map(|(group, _, _)| {
                endpoints
                    .iter()
                    .filter(move |endpoint| endpoint.scheduling_group() == group)
                    .cloned()
            })
            .collect()
    }

    /// 路由规划(配置须先 `normalized()`)。
    ///
    /// 分流规则:第一条 enabled 且命中的生效;固定 Provider 失效时降级候选序列;
    /// 普通路由在扁平 Provider 候选序列中按模型声明筛选入口。
    pub fn plan(request: &RoutingRequest, config: &AppConfig) -> Result<RoutePlan, RoutePlanError> {
        Self::plan_for_source(request, config, ProviderProtocol::Anthropic)
    }

    /// 入口路径已确定 SourceFormat 的路由规划；不读取 UA，也不从请求体猜测协议。
    pub fn plan_for_source(
        request: &RoutingRequest,
        config: &AppConfig,
        source_format: ProviderProtocol,
    ) -> Result<RoutePlan, RoutePlanError> {
        let base_model = model_name::clean(&request.model);
        let feature_rule = config
            .feature_rules
            .iter()
            .find(|rule| inspector::feature_rule_matches(rule, request));

        if let Some(rule) = feature_rule
            && let Some(result) = Self::feature_plan(config, rule, &base_model, source_format)
        {
            return result;
        }
        if !config.matches_model(&base_model) {
            return Err(RoutePlanError::NoProviderForModel(base_model.clone()));
        }
        let endpoints = Self::planned_endpoints(
            &config.endpoints,
            &base_model,
            source_format,
            None,
            None,
            None,
        );

        if endpoints.is_empty() {
            return Err(RoutePlanError::NoCompatibleProvider {
                source_format: source_format.token().into(),
            });
        }

        Ok(RoutePlan {
            client_model: base_model.clone(),
            effective_model: base_model,
            feature_rule_id: None,
            endpoints,
        })
    }

    /// 分流规则的目标解析。
    fn feature_plan(
        config: &AppConfig,
        rule: &FeatureRule,
        base_model: &str,
        source_format: ProviderProtocol,
    ) -> Option<Result<RoutePlan, RoutePlanError>> {
        let target = &rule.target;
        let effective_model = model_name::clean(&target.model);
        let grok_requires_responses =
            matches!(
                rule.match_.request_kind,
                Some(RequestKind::WebSearch | RequestKind::WebFetch)
            ) && effective_model.to_ascii_lowercase().starts_with("grok-");
        if grok_requires_responses
            && target
                .protocol_override
                .is_some_and(|protocol| protocol != ProviderProtocol::OpenAIResponses)
        {
            return Some(Err(RoutePlanError::NoCompatibleProvider {
                source_format: source_format.token().into(),
            }));
        }
        let protocol_override = if grok_requires_responses {
            Some(ProviderProtocol::OpenAIResponses)
        } else {
            target.protocol_override
        };
        let pinned_endpoint_available = target.endpoint_id.as_deref().is_some_and(|endpoint_id| {
            config
                .endpoint(endpoint_id)
                .is_some_and(|endpoint| endpoint.enabled)
        });
        let mut endpoints = Self::planned_endpoints(
            &config.endpoints,
            &effective_model,
            source_format,
            protocol_override,
            target.endpoint_id.as_deref(),
            target.effort,
        );
        // 规则钉住的入口被停用/删除时降级为候选序列 failover,而不是让整条规则失效。
        if endpoints.is_empty() && target.endpoint_id.is_some() && !pinned_endpoint_available {
            endpoints = Self::planned_endpoints(
                &config.endpoints,
                &effective_model,
                source_format,
                protocol_override,
                None,
                target.effort,
            );
        }
        if endpoints.is_empty() {
            return Some(Err(RoutePlanError::NoCompatibleProvider {
                source_format: source_format.token().into(),
            }));
        }
        Some(Ok(RoutePlan {
            client_model: base_model.to_string(),
            effective_model,
            feature_rule_id: Some(rule.id.clone()),
            endpoints,
        }))
    }

    /// `protocol_override` 非空(分流规则指定协议)时覆盖各入口自身协议;
    /// `pinned_endpoint_id` 非空(规则钉住入口)时只保留该入口并**跳过模型映射筛选**。
    fn planned_endpoints(
        endpoints: &[Endpoint],
        effective_model: &str,
        source_format: ProviderProtocol,
        protocol_override: Option<ProviderProtocol>,
        pinned_endpoint_id: Option<&str>,
        effort_override: Option<model_name::ReasoningEffort>,
    ) -> Vec<PlannedEndpoint> {
        let candidates: Vec<PlannedEndpoint> = endpoints
            .iter()
            .filter(|e| e.enabled)
            .filter(|e| pinned_endpoint_id.is_none_or(|id| e.id == id))
            .filter_map(|endpoint| {
                // 每个入口都通过显式 mappings 声明承接范围。
                let mapping = endpoint.mapping_for(effective_model).cloned();
                if pinned_endpoint_id.is_none() && mapping.is_none() {
                    return None;
                }
                let failover_timeout = mapping.as_ref().and_then(|m| m.failover_timeout_seconds);
                let configured_protocol = endpoint.protocol;
                let protocol = configured_protocol.resolve(source_format, protocol_override)?;
                let upstream_model = mapping
                    .as_ref()
                    .map(|m| m.upstream_model_for(effective_model))
                    .unwrap_or_else(|| effective_model.to_string());
                Some(PlannedEndpoint {
                    endpoint_id: endpoint.id.clone(),
                    endpoint_name: endpoint.name.clone(),
                    base_url: endpoint.base_url.clone(),
                    configured_protocol,
                    source_format,
                    protocol,
                    route_mode: if source_format == protocol {
                        RouteMode::Native
                    } else {
                        RouteMode::Translated
                    },
                    routed_model: effective_model.to_string(),
                    upstream_model,
                    pinned_ips: endpoint.pinned_ips.clone(),
                    pinned_ip_exclusive: endpoint.pinned_ip_exclusive,
                    priority: endpoint.priority,
                    sticky_group: endpoint.sticky_group.clone(),
                    thinking: mapping
                        .as_ref()
                        .map(|m| m.thinking)
                        .unwrap_or(ThinkingMode::Adaptive),
                    // 规则钉住入口且它没有匹配映射时的兜底:standard = 透传客户端自己的
                    // beta,不替它强开 1M(钉住的分类器/本地小模型尤其不该被强开)。
                    context: mapping
                        .as_ref()
                        .map(|m| m.context)
                        .unwrap_or(ContextMode::Standard),
                    effort_override,
                    failover_timeout_seconds: failover_timeout,
                    keep_alive: endpoint.keep_alive,
                })
            })
            .collect();

        let native: Vec<PlannedEndpoint> = candidates
            .iter()
            .filter(|endpoint| endpoint.route_mode == RouteMode::Native)
            .cloned()
            .collect();
        let selected = if native.is_empty() {
            candidates
        } else {
            native
        };
        Self::order_endpoints(&selected)
    }
}
