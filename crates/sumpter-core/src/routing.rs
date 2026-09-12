//! 路由:请求指纹识别(RequestInspector)、复合会话粘性键(StickyKey/AffinityHasher)、
//! 路由规划(RoutePlanner)。对齐 Swift `Routing.swift`,行为注释以 Swift 版为准绳。

use serde_json::{Map, Value};

use crate::config::{
    AppConfig, ContextMode, Endpoint, EndpointProtocolMode, FeatureRule, ModelMapping,
    ProviderProtocol, RequestKind, ThinkingMode,
};
use crate::model_groups::ModelGroupSchedulingStrategy;
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

/// 请求用途。
///
/// **不参与路由**(入口选择只看 mappings 与 featureRules),但确实参与出站 body
/// 改写:`request_build::server_retrieval_enabled` 用它判断是否启用上游服务端检索,
/// WebFetch 命中时会替换 `tools`。改这里的判定会改变数据面行为,不只是改统计口径。
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

    /// 工具类型前缀判定:存在目标前缀工具、且**不存在**客户端自定义工具。
    ///
    /// 客户端自定义工具在 Anthropic Messages 上有两种等价 wire 形状:省略 `type`
    /// (Claude Code 当前形状)与显式 `type: "custom"`(SDK 也接受,见
    /// `tests/bridge_in.rs` 的 Codex 侧样本)。只认省略形状会让带 `"custom"` 的
    /// 主对话请求在同时挂了一个 `mcp__*` 工具时命中 toolTypePrefix 规则,被分流
    /// 到内建子请求专用的入口。
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
            } else if is_client_tool_type(tool_type) {
                has_client_tool = true;
            }
        }
        has_target && !has_client_tool
    }

    /// 客户端自定义工具的 type 形状:省略(空)或显式 `"custom"`。
    /// 出站桥用它区分「要映射成 OpenAI function 的客户端工具」与「由上游执行的
    /// 服务端工具」。
    pub fn is_client_tool_type(tool_type: &str) -> bool {
        tool_type.is_empty() || tool_type == "custom"
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
    /// - 无 tools + 单条 user:CC 主对话恒定带 tools 且多轮,进不到这里;
    /// - system 非空:排除裸 curl / 简易客户端的探测请求;
    /// - 用途为 Standard:已识别的辅助请求(标题/搜索/抓取/分类)自己有标签。
    ///
    /// 剩下的就是「带专用 system 的单轮无工具请求,却谁也没匹配上」——几乎必然是
    /// CC 升级后指纹失配的内部请求。存在的意义就是把这种失配变成可见信号:否则
    /// `matches_session_title` 等会静默退化成 Standard,无人察觉。
    ///
    /// 身份标识的处理:判「除 CC 身份之外还有没有专用指令」,而不是「出现身份标识
    /// 就排除」。CC 2.1.220+ 的辅助请求 system 是「billing 头 + CC 身份 + 专用指令」
    /// 三段,按出现即排除会把最需要告警的一类请求恰好屏蔽掉;而只带身份(或身份加
    /// 环境上下文)的请求本就不是辅助请求,剥掉身份后没有指令剩下,自然不命中。
    pub fn is_unmatched_no_tools(request: &RoutingRequest) -> bool {
        if !request.tools.is_empty()
            || request.messages.len() != 1
            || request.messages[0].role != "user"
        {
            return false;
        }
        let instructions = system_text(request).replace(CLAUDE_CODE_IDENTITY, "");
        !instructions.trim().is_empty() && request_purpose(request) == RequestPurpose::Standard
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

    /// Claude Code 自动会话标题请求:单条 `<session>` 消息、无工具,外加一条身份判据。
    /// 不看模型名,避免把用户主动选择的模型主请求误标成内部辅助请求。
    ///
    /// 身份判据有两条,取或:早期版本只能靠 system 措辞;2.1.220+ 改用结构化输出,
    /// `output_config` 的 json_schema 比措辞稳定得多 —— 措辞每次改版都可能动,而
    /// schema 是客户端的解析契约,不会随便变。只留措辞判据会让新版静默退化成
    /// Standard。
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
        let normalized = normalized_newlines(&raw_text);
        let text = normalized.trim();
        if !text.starts_with("<session>") || !text.ends_with("</session>") {
            return false;
        }
        let system = normalized_newlines(&system_text(request));
        (system.contains(SESSION_TITLE_SYSTEM_PREFIX)
            && system.contains(SESSION_TITLE_SYSTEM_SUFFIX))
            || matches_title_output_schema(request)
    }

    /// `output_config.format` 是 json_schema 且只约束一个字符串 `title`。
    fn matches_title_output_schema(request: &RoutingRequest) -> bool {
        let Some(format) = request
            .raw
            .get("output_config")
            .and_then(|config| config.pointer("/format"))
        else {
            return false;
        };
        if format.get("type").and_then(Value::as_str) != Some("json_schema") {
            return false;
        }
        format
            .pointer("/schema/properties/title")
            .is_some_and(|title| title.get("type").and_then(Value::as_str) == Some("string"))
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
    pub model_group_id: Option<String>,
    pub model_group_name: Option<String>,
    pub model_group_rank: usize,
    pub scheduling_strategy: ModelGroupSchedulingStrategy,
    pub base_url: String,
    /// 配置中声明的四态入口模式。
    pub configured_protocol: EndpointProtocolMode,
    /// 入站路径确定的真实 SourceFormat。
    pub source_format: ProviderProtocol,
    /// 本次请求已解析出的真实 TargetFormat；绝不包含 Auto。
    pub protocol: ProviderProtocol,
    pub user_agent: crate::config::UserAgentSettings,
    pub route_mode: RouteMode,
    /// 路由规则选中的逻辑模型名；用于按模型家族做协议兼容，不能从上游别名反推。
    pub routed_model: String,
    pub upstream_model: String,
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
    #[error("no Provider accepts {capability} capability")]
    NoProviderForCapability { capability: String },
}

pub struct RoutePlanner;

/// Synthetic model used for native resource requests that do not carry a
/// model field (Files, Videos, Models and their dynamic sub-paths).  It is
/// never sent to an upstream Provider; it only gives the shared sticky and
/// runtime-event pipeline a stable routing identity.
pub const RESOURCE_ROUTING_MODEL: &str = "__sumpter_resource__";

impl RoutePlanner {
    /// 无既有会话归属时的确定性入口顺序：按分组最低 priority 排序，
    /// 同级按首次出现位置；组内保持配置顺序。
    pub fn order_endpoints(endpoints: &[PlannedEndpoint]) -> Vec<PlannedEndpoint> {
        let mut groups: Vec<(String, usize, i64, usize)> = Vec::new();
        for (index, endpoint) in endpoints.iter().enumerate() {
            let group = endpoint.scheduling_group().to_string();
            if let Some(existing) = groups.iter_mut().find(|item| item.0 == group) {
                existing.2 = existing.2.min(endpoint.priority);
            } else {
                groups.push((group, endpoint.model_group_rank, endpoint.priority, index));
            }
        }
        groups.sort_by(|a, b| {
            a.1.cmp(&b.1)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(&b.3))
        });
        groups
            .iter()
            .flat_map(|(group, _, _, _)| {
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

    /// Plan a data-plane request that must be relayed byte-for-byte.
    ///
    /// Raw requests still use the configured model mappings (and feature-rule
    /// endpoint/model selection), but an endpoint's declared protocol is not a
    /// compatibility gate. The upstream owns the protocol contract; Sumpter
    /// must not reject a request merely because its path looks unlike the
    /// endpoint's configured protocol.
    pub fn plan_for_passthrough(
        request: &RoutingRequest,
        config: &AppConfig,
        source_format: ProviderProtocol,
    ) -> Result<RoutePlan, RoutePlanError> {
        Self::plan_for_source_mode(request, config, source_format, true, None)
    }

    /// First configured mapping that can serve `capability`.
    pub fn default_model_for_capability(
        config: &AppConfig,
        capability: crate::capability::ModelCapability,
    ) -> Option<String> {
        for scoped in config.routing_endpoints() {
            let endpoint = &scoped.endpoint;
            if !endpoint.enabled {
                continue;
            }
            for mapping in &endpoint.mappings {
                // A family wildcard such as `grok-imagine-*` does not by
                // itself identify image vs video.  Prefer a concrete model
                // from this endpoint's catalog when one is available; this
                // prevents the no-model Videos fallback from fabricating the
                // ambiguous stem `grok-imagine` and then missing its mapping.
                if !mapping.capabilities.is_empty() {
                    if mapping.capabilities.contains(&capability) {
                        return Some(default_model_for_mapping(endpoint, mapping));
                    }
                    continue;
                }
                let concrete = endpoint
                    .catalog
                    .as_ref()
                    .into_iter()
                    .flat_map(|catalog| catalog.models.iter())
                    .map(|model| crate::model_name::clean(model))
                    .filter(|model| {
                        !model.is_empty()
                            && !model.contains('*')
                            && crate::model_name::pattern_matches(&mapping.client_pattern, model)
                    })
                    .find(|model| {
                        crate::capability::capabilities_for_model(
                            &mapping.capabilities,
                            &mapping.client_pattern,
                            model,
                        )
                        .contains(&capability)
                    });
                if let Some(model) = concrete {
                    return Some(model);
                }
                if crate::capability::inferred_capabilities(&mapping.client_pattern)
                    .contains(&capability)
                {
                    return Some(crate::capability::canonical_model_from_pattern(
                        &mapping.client_pattern,
                    ));
                }
            }
        }
        None
    }

    /// Passthrough planning that also requires the matched mapping to serve
    /// `capability`. A text wildcard therefore cannot steal image/video/live
    /// traffic merely because it matches the client model string.
    pub fn plan_for_capability(
        request: &RoutingRequest,
        config: &AppConfig,
        source_format: ProviderProtocol,
        capability: crate::capability::ModelCapability,
    ) -> Result<RoutePlan, RoutePlanError> {
        Self::plan_for_source_mode(request, config, source_format, true, Some(capability))
    }

    /// Plan a native resource request that has no model field.  CPA routes
    /// resource APIs by the provider/credential surface rather than by a text
    /// model mapping.  Sumpter keeps its explicit mapping requirement for
    /// conversational requests, while resource intents may use any enabled
    /// non-Anthropic endpoint and remain byte-for-byte native.
    pub fn plan_for_resource(
        config: &AppConfig,
        source_format: ProviderProtocol,
    ) -> Result<RoutePlan, RoutePlanError> {
        let endpoints = Self::planned_endpoints(
            config,
            RESOURCE_ROUTING_MODEL,
            source_format,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .into_iter()
        .filter(|endpoint| endpoint.protocol != ProviderProtocol::Anthropic)
        .collect::<Vec<_>>();
        if endpoints.is_empty() {
            return Err(RoutePlanError::NoCompatibleProvider {
                source_format: source_format.token().into(),
            });
        }
        Ok(RoutePlan {
            client_model: RESOURCE_ROUTING_MODEL.into(),
            effective_model: RESOURCE_ROUTING_MODEL.into(),
            feature_rule_id: None,
            endpoints,
        })
    }

    /// Plan a resource request against providers that explicitly advertise a
    /// resource capability. Files do not carry a model and no model name can
    /// reliably imply upload/storage support, so `files` is fail-closed: an
    /// endpoint becomes eligible only when one of its mappings declares
    /// `capabilities: ["files"]`. This avoids silently sending a file or
    /// deletion request to the first text/media credential surface.
    pub fn plan_for_resource_capability(
        config: &AppConfig,
        source_format: ProviderProtocol,
        capability: crate::capability::ModelCapability,
    ) -> Result<RoutePlan, RoutePlanError> {
        let mut endpoints = Self::planned_endpoints(
            config,
            RESOURCE_ROUTING_MODEL,
            source_format,
            None,
            None,
            None,
            true,
            true,
            None,
        )
        .into_iter()
        .filter(|endpoint| endpoint.protocol != ProviderProtocol::Anthropic)
        .collect::<Vec<_>>();

        endpoints.retain(|planned| {
            config
                .endpoint(&planned.endpoint_id)
                .is_some_and(|endpoint| {
                    endpoint.enabled
                        && endpoint
                            .mappings
                            .iter()
                            .any(|mapping| mapping.capabilities.contains(&capability))
                })
        });
        if endpoints.is_empty() {
            return Err(RoutePlanError::NoProviderForCapability {
                capability: capability.as_str().into(),
            });
        }
        Ok(RoutePlan {
            client_model: RESOURCE_ROUTING_MODEL.into(),
            effective_model: RESOURCE_ROUTING_MODEL.into(),
            feature_rule_id: None,
            endpoints,
        })
    }

    /// 入口路径已确定 SourceFormat 的路由规划；不读取 UA，也不从请求体猜测协议。
    pub fn plan_for_source(
        request: &RoutingRequest,
        config: &AppConfig,
        source_format: ProviderProtocol,
    ) -> Result<RoutePlan, RoutePlanError> {
        Self::plan_for_source_mode(request, config, source_format, false, None)
    }

    fn plan_for_source_mode(
        request: &RoutingRequest,
        config: &AppConfig,
        source_format: ProviderProtocol,
        passthrough: bool,
        capability: Option<crate::capability::ModelCapability>,
    ) -> Result<RoutePlan, RoutePlanError> {
        let base_model = model_name::clean(&request.model);
        let feature_rule = config
            .feature_rules
            .iter()
            .find(|rule| inspector::feature_rule_matches(rule, request));

        if let Some(rule) = feature_rule
            && let Some(result) = Self::feature_plan(
                config,
                rule,
                &base_model,
                source_format,
                passthrough,
                capability,
            )
        {
            return result;
        }
        // Capability routes may use CPA's public Realtime aliases even when
        // the configuration intentionally declares only the private
        // `gpt-live-1-codex` mapping.  Check the capability-aware mapping
        // surface instead of the text-model union in that case; a broad text
        // wildcard still cannot satisfy image/video/live requests.
        let model_is_mapped = match capability {
            Some(wanted) => config.routing_endpoints().iter().any(|scoped| {
                let endpoint = &scoped.endpoint;
                endpoint.enabled
                    && endpoint
                        .mapping_for_capability(&base_model, wanted)
                        .is_some()
            }),
            None => config.matches_model(&base_model),
        };
        if !model_is_mapped {
            return Err(match capability {
                Some(capability) => RoutePlanError::NoProviderForCapability {
                    capability: capability.as_str().into(),
                },
                None => RoutePlanError::NoProviderForModel(base_model.clone()),
            });
        }
        let endpoints = Self::planned_endpoints(
            config,
            &base_model,
            source_format,
            None,
            None,
            None,
            passthrough,
            false,
            capability,
        );

        if endpoints.is_empty() {
            return Err(match capability {
                Some(capability) => RoutePlanError::NoProviderForCapability {
                    capability: capability.as_str().into(),
                },
                None => RoutePlanError::NoCompatibleProvider {
                    source_format: source_format.token().into(),
                },
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
        passthrough: bool,
        capability: Option<crate::capability::ModelCapability>,
    ) -> Option<Result<RoutePlan, RoutePlanError>> {
        let target = &rule.target;
        let effective_model = model_name::clean(&target.model);
        let grok_requires_responses =
            matches!(
                rule.match_.request_kind,
                Some(RequestKind::WebSearch | RequestKind::WebFetch)
            ) && effective_model.to_ascii_lowercase().starts_with("grok-");
        if !passthrough
            && grok_requires_responses
            && target
                .protocol_override
                .is_some_and(|protocol| protocol != ProviderProtocol::OpenAIResponses)
        {
            return Some(Err(RoutePlanError::NoCompatibleProvider {
                source_format: source_format.token().into(),
            }));
        }
        let protocol_override = if passthrough {
            None
        } else if grok_requires_responses {
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
            config,
            &effective_model,
            source_format,
            protocol_override,
            target.endpoint_id.as_deref(),
            target.effort,
            passthrough,
            false,
            capability,
        );
        // 规则钉住的入口被停用/删除时降级为候选序列 failover,而不是让整条规则失效。
        if endpoints.is_empty() && target.endpoint_id.is_some() && !pinned_endpoint_available {
            endpoints = Self::planned_endpoints(
                config,
                &effective_model,
                source_format,
                protocol_override,
                None,
                target.effort,
                passthrough,
                false,
                capability,
            );
        }
        if endpoints.is_empty() {
            return Some(Err(match capability {
                Some(capability) => RoutePlanError::NoProviderForCapability {
                    capability: capability.as_str().into(),
                },
                None => RoutePlanError::NoCompatibleProvider {
                    source_format: source_format.token().into(),
                },
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
    #[allow(clippy::too_many_arguments)]
    fn planned_endpoints(
        config: &AppConfig,
        effective_model: &str,
        source_format: ProviderProtocol,
        protocol_override: Option<ProviderProtocol>,
        pinned_endpoint_id: Option<&str>,
        effort_override: Option<model_name::ReasoningEffort>,
        passthrough: bool,
        allow_unmapped: bool,
        capability: Option<crate::capability::ModelCapability>,
    ) -> Vec<PlannedEndpoint> {
        let scoped = if pinned_endpoint_id.is_some() {
            // Explicit feature rules/resource affinity retain their original semantics.
            config
                .endpoints
                .iter()
                .cloned()
                .map(crate::model_groups::RoutingEndpoint::legacy)
                .collect()
        } else {
            config.routing_endpoints()
        };
        let candidates: Vec<PlannedEndpoint> = scoped
            .iter()
            .filter(|e| e.endpoint.enabled)
            .filter(|e| pinned_endpoint_id.is_none_or(|id| e.endpoint.id == id))
            .filter_map(|scoped| {
                let endpoint = &scoped.endpoint;
                // 每个入口都通过显式 mappings 声明承接范围。
                let mapping = match capability {
                    Some(wanted) => endpoint.mapping_for_capability(effective_model, wanted),
                    None => endpoint.mapping_for(effective_model),
                }
                .cloned();
                // A feature rule may pin an endpoint for ordinary passthrough
                // even when its model is intentionally unmapped (legacy
                // behavior).  Capability routes are different: allowing the
                // pin to bypass the mapping check would reintroduce the exact
                // text-provider-stealing bug this planner is meant to stop.
                if mapping.is_none()
                    && (capability.is_some() || (pinned_endpoint_id.is_none() && !allow_unmapped))
                {
                    return None;
                }
                let failover_timeout = mapping.as_ref().and_then(|m| m.failover_timeout_seconds);
                let configured_protocol = endpoint.protocol;
                let protocol = if passthrough {
                    configured_protocol
                        .fixed_protocol()
                        .unwrap_or(source_format)
                } else {
                    configured_protocol.resolve(source_format, protocol_override)?
                };
                let mut upstream_model = mapping
                    .as_ref()
                    .map(|m| m.upstream_model_for(effective_model))
                    .unwrap_or_else(|| effective_model.to_string());
                // A legacy/compact config often leaves `upstreamModel` empty
                // on the private Live mapping.  When that mapping is being
                // used as the alias target for a public Realtime model, the
                // OAuth upstream still expects its canonical Codex model.
                // Keep `routed_model` (and therefore event attribution) as
                // the caller's logical `gpt-realtime` value.
                if capability == Some(crate::capability::ModelCapability::Live)
                    && upstream_model == effective_model
                    && crate::capability::is_realtime_model_name(effective_model)
                    && mapping.as_ref().is_some_and(|mapping| {
                        model_name::clean(&mapping.client_pattern) == "gpt-live-1-codex"
                    })
                {
                    upstream_model = "gpt-live-1-codex".into();
                }
                Some(PlannedEndpoint {
                    endpoint_id: endpoint.id.clone(),
                    endpoint_name: endpoint.name.clone(),
                    model_group_id: scoped.group_id.clone(),
                    model_group_name: scoped.group_name.clone(),
                    model_group_rank: scoped.group_rank,
                    scheduling_strategy: scoped.scheduling_strategy,
                    base_url: endpoint.base_url.clone(),
                    configured_protocol,
                    source_format,
                    protocol,
                    user_agent: endpoint.user_agent.clone(),
                    route_mode: if passthrough || source_format == protocol {
                        RouteMode::Native
                    } else {
                        RouteMode::Translated
                    },
                    routed_model: effective_model.to_string(),
                    upstream_model,
                    priority: scoped
                        .model_priorities
                        .get(effective_model)
                        .copied()
                        .unwrap_or(endpoint.priority),
                    sticky_group: match &scoped.group_id {
                        // Preserve legacy affinity keys for the migrated default group.
                        Some(id) if id != "default" => Some(format!(
                            "model-group:{}:{}:{}",
                            id.len(),
                            id,
                            endpoint.scheduling_group()
                        )),
                        _ => endpoint.sticky_group.clone(),
                    },
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
                    effort_override: effort_override
                        .or_else(|| mapping.as_ref().and_then(|m| m.effort)),
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
        let ordered = Self::order_endpoints(&selected);
        let mut unique: Vec<PlannedEndpoint> = Vec::new();
        for candidate in ordered {
            let duplicate = candidate.model_group_id.is_some()
                && unique.iter().any(|prior| {
                    let mut comparable = candidate.clone();
                    comparable.model_group_id = prior.model_group_id.clone();
                    comparable.model_group_name = prior.model_group_name.clone();
                    comparable.model_group_rank = prior.model_group_rank;
                    comparable.scheduling_strategy = prior.scheduling_strategy;
                    comparable.priority = prior.priority;
                    comparable.sticky_group = prior.sticky_group.clone();
                    &comparable == prior
                });
            if !duplicate {
                unique.push(candidate);
            }
        }
        unique
    }
}

fn default_model_for_mapping(endpoint: &Endpoint, mapping: &ModelMapping) -> String {
    endpoint
        .catalog
        .as_ref()
        .into_iter()
        .flat_map(|catalog| catalog.models.iter())
        .map(|model| crate::model_name::clean(model))
        .find(|model| {
            !model.is_empty()
                && !model.contains('*')
                && crate::model_name::pattern_matches(&mapping.client_pattern, model)
        })
        .unwrap_or_else(|| crate::capability::canonical_model_from_pattern(&mapping.client_pattern))
}
