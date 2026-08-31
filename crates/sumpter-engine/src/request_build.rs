//! 出站请求构造:header 过滤/强制注入、anthropic body 改写(model/thinking/effort)、
//! 桥接 body 与路径。纯函数,对齐 Swift `makeOutboundRequest`(specs/spec-engine.md §3-4)。

use serde_json::{Value, json};
use sumpter_core::bridge;
use sumpter_core::config::{ContextMode, ProviderProtocol, ThinkingMode};
use sumpter_core::model_name::{self, ReasoningEffort};
use sumpter_core::routing::{PlannedEndpoint, RequestPurpose, RoutingRequest};

use crate::outbound::{OutboundRequest, join_paths};

/// Claude Code 指纹 UA:入站 UA 透传优先,仅在客户端未带 UA 时作回落
/// (无 UA 探针打 DashScope 会 405,回落保住指纹放行面)。
pub const CLAUDE_CODE_USER_AGENT: &str = "claude-cli/2.1.220 (external, cli)";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
const IDENTITY_ACCEPT_ENCODING: &str = "identity";

/// Authentication headers forced by the real data plane and reused by
/// daemon-originated Provider probes.
pub fn provider_auth_headers(api_key: &str) -> Vec<(&'static str, String)> {
    let key = api_key.trim();
    if key.is_empty() {
        Vec::new()
    } else {
        vec![
            ("authorization", format!("Bearer {key}")),
            ("x-api-key", key.into()),
        ]
    }
}

/// Common fingerprint/authentication headers for a daemon-originated
/// Provider probe.  The first attempt must match the data plane's forced
/// identity surface; compatibility retries can replace only the auth set.
pub fn provider_probe_headers(api_key: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("accept", "application/json".into()),
        ("accept-encoding", IDENTITY_ACCEPT_ENCODING.into()),
        ("connection", "close".into()),
        ("anthropic-version", ANTHROPIC_VERSION.into()),
        ("user-agent", CLAUDE_CODE_USER_AGENT.into()),
    ];
    headers.extend(provider_auth_headers(api_key));
    headers
}

const ANTHROPIC_BETA_BASE: &str =
    "claude-code-20250219,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12";
const ANTHROPIC_BETA_EFFORT: &str = "effort-2025-11-24";

/// 入站 header 黑名单(不透传;其余如 anthropic-version 原样透传)。
///
/// `x-sumpter-*` 是客户端声明项目归因用的入站专用 header(见 `ClientDeclaredMetadata`):
/// 代理读完就必须剥掉,否则项目名与本机工作区路径会跟着请求外泄给上游中转站。
const HEADER_BLOCKLIST: [&str; 20] = [
    "host",
    "x-sumpter-project",
    "x-sumpter-workspace",
    "x-sumpter-git-remote",
    "x-kekulv-project",
    "x-kekulv-workspace",
    "x-kekulv-git-remote",
    "authorization",
    "x-api-key",
    "content-length",
    "content-type",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "accept-encoding",
];

pub struct OutboundBuild {
    pub request: OutboundRequest,
    /// 本次请求解析出的 effort 后缀(记账/调试用)。
    pub effort: Option<ReasoningEffort>,
}

/// Native OpenAI/ChatGPT requests that bypass the Anthropic bridge while still
/// using the common pool, sticky-session, failover and event pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassthroughKind {
    Chat,
    Completions,
    Responses,
    ResponsesCompact,
    ImagesGenerations,
    ImagesEdits,
    AlphaSearch,
    ClaudeCountTokens,
}

impl PassthroughKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Chat => "/chat/completions",
            Self::Completions => "/completions",
            Self::Responses => "/responses",
            Self::ResponsesCompact => "/responses/compact",
            Self::ImagesGenerations => "/images/generations",
            Self::ImagesEdits => "/images/edits",
            Self::AlphaSearch => "/alpha/search",
            Self::ClaudeCountTokens => "/messages/count_tokens",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Completions => "completions",
            Self::Responses => "responses",
            Self::ResponsesCompact => "responses-compact",
            Self::ImagesGenerations => "images-generations",
            Self::ImagesEdits => "images-edits",
            Self::AlphaSearch => "alpha-search",
            Self::ClaudeCountTokens => "claude-count-tokens",
        }
    }
}

#[derive(Clone, Copy)]
pub struct PassthroughRequest<'a> {
    pub kind: PassthroughKind,
    pub body: &'a [u8],
    pub content_type: Option<&'a str>,
    pub stream: bool,
}
/// 是否为本次请求启用上游服务端检索。
///
/// - WebSearch 分流在三种目标协议下都自动启用；
/// - Grok 家族的 WebFetch 仅在 Responses 目标协议下由路由规则放行；
/// - 普通主对话绝不因模型名前缀自动联网。
pub fn server_retrieval_enabled(
    endpoint: &PlannedEndpoint,
    request: &RoutingRequest,
    purpose: RequestPurpose,
) -> bool {
    let routed_model = model_name::clean(&endpoint.routed_model).to_ascii_lowercase();
    let is_grok = routed_model.starts_with("grok-");
    match purpose {
        RequestPurpose::WebSearch => true,
        RequestPurpose::WebFetch => is_grok && request_contains_url(request),
        _ => false,
    }
}

fn request_contains_url(request: &RoutingRequest) -> bool {
    request.messages.iter().any(|message| {
        let text = message.content.to_string();
        text.contains("https://") || text.contains("http://")
    })
}

/// 构造出站请求。`inbound_headers` 为原始入站 header(任意大小写),
/// `inbound_path_and_query` 为入站路径+query。
/// `purpose` + 路由后的逻辑模型共同决定服务端检索适配。WebSearch 的能力由
/// 严格用途指纹和最终 TargetFormat 决定，不再依赖 Provider 额外能力开关。
#[allow(clippy::too_many_arguments)]
pub fn build_outbound(
    endpoint: &PlannedEndpoint,
    request: &RoutingRequest,
    inbound_headers: &[(String, String)],
    inbound_method: &str,
    inbound_path_and_query: &str,
    api_key: &str,
    pinned_ip: Option<String>,
    purpose: RequestPurpose,
    // Native Adapter:Some((方言, 原始请求体))时原样打上游对应端点,仅改写 model。
    // 实际协议已由 RoutePlanner 从入口四态模式解析完成。
    passthrough: Option<PassthroughRequest<'_>>,
) -> OutboundBuild {
    let effort = endpoint
        .effort_override
        .or_else(|| model_name::reasoning_effort(&request.model));
    let server_retrieval = server_retrieval_enabled(endpoint, request, purpose);
    let protocol = endpoint.protocol;

    // 1. 透传非黑名单 header(顺带防双份:同名只出现一次,后到覆盖)。
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in inbound_headers {
        let lower = name.to_lowercase();
        if HEADER_BLOCKLIST.contains(&lower.as_str()) {
            continue;
        }
        set_header(&mut headers, &lower, value);
    }

    // 2. 强制写入(先删同名再写,严防双份 —— DashScope 对重复 MIME 直接 500)。
    //    空 key = 无鉴权上游:不发任何鉴权头(入站的 authorization/x-api-key 已被黑名单剥除)。
    for (name, value) in provider_auth_headers(api_key) {
        set_header(&mut headers, name, &value);
    }
    set_header(&mut headers, "accept-encoding", IDENTITY_ACCEPT_ENCODING);
    set_header(&mut headers, "content-type", "application/json");
    // UA 透传入站(claude-cli / codex 等各用其真);缺失才回填 CC 指纹。
    if !headers.iter().any(|(n, _)| n == "user-agent") {
        set_header(&mut headers, "user-agent", CLAUDE_CODE_USER_AGENT);
    }

    let base_path = base_path_of(&endpoint.base_url);

    // 透传:出站即客户端方言,零转换(工具/reasoning 原生保真)。
    if let Some(passthrough) = passthrough {
        set_header(
            &mut headers,
            "accept",
            if passthrough.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        );
        set_header(
            &mut headers,
            "content-type",
            passthrough.content_type.unwrap_or("application/json"),
        );
        if passthrough.kind == PassthroughKind::AlphaSearch {
            set_header(&mut headers, "originator", "codex_cli_rs");
        }
        let passthrough_body = if passthrough.kind == PassthroughKind::AlphaSearch {
            sanitize_alpha_search_body(passthrough.body)
        } else {
            passthrough.body.to_vec()
        };
        return OutboundBuild {
            request: OutboundRequest {
                method: inbound_method.to_string(),
                base_url: endpoint.base_url.clone(),
                path_and_query: openai_suffix_path(&base_path, passthrough.kind.suffix()),
                headers,
                // JSON bodies can safely follow feature-rule model rewrites. Multipart
                // uploads stay byte-for-byte intact so boundaries and binary images are
                // never reconstructed by the proxy.
                body: rewrite_passthrough_model(
                    &passthrough_body,
                    &endpoint.upstream_model,
                    passthrough.content_type,
                ),
                pinned_ip,
                keep_alive: endpoint.keep_alive,
            },
            effort,
        };
    }

    let (path, body) = match protocol {
        ProviderProtocol::Anthropic => {
            // 客户端自己声明的 beta:重建头时以它为基底(见 beta_header)。
            let client_beta = headers
                .iter()
                .find(|(name, _)| name == "anthropic-beta")
                .map(|(_, value)| value.clone());
            set_header(
                &mut headers,
                "anthropic-beta",
                &beta_header(client_beta.as_deref(), endpoint.context, effort),
            );
            (
                join_paths(&base_path, inbound_path_and_query),
                rewrite_anthropic_body(
                    request,
                    &endpoint.routed_model,
                    &endpoint.upstream_model,
                    endpoint.thinking,
                    effort,
                    purpose,
                    server_retrieval,
                ),
            )
        }
        ProviderProtocol::OpenAI => {
            set_header(&mut headers, "accept", "text/event-stream");
            (
                openai_suffix_path(&base_path, "/chat/completions"),
                serde_json::to_vec(&bridge::make_openai_chat_body(
                    request,
                    &endpoint.upstream_model,
                    effort,
                    server_retrieval,
                ))
                .unwrap_or_default(),
            )
        }
        ProviderProtocol::OpenAIResponses => {
            set_header(&mut headers, "accept", "text/event-stream");
            (
                openai_suffix_path(&base_path, "/responses"),
                serde_json::to_vec(&bridge::make_responses_body(
                    request,
                    &endpoint.upstream_model,
                    effort,
                    server_retrieval,
                ))
                .unwrap_or_default(),
            )
        }
    };

    OutboundBuild {
        request: OutboundRequest {
            method: inbound_method.to_string(),
            base_url: endpoint.base_url.clone(),
            path_and_query: path,
            headers,
            body,
            pinned_ip,
            keep_alive: endpoint.keep_alive,
        },
        effort,
    }
}

/// Codex Alpha Search 的 prompt cache 字段属于 Responses 提交层，CPA 在转发
/// 独立搜索端点前会移除；保留其它未知字段，避免代理追着客户端版本枚举参数。
fn sanitize_alpha_search_body(raw: &[u8]) -> Vec<u8> {
    let Ok(mut value) = serde_json::from_slice::<Value>(raw) else {
        return raw.to_vec();
    };
    let Some(object) = value.as_object_mut() else {
        return raw.to_vec();
    };
    let removed = object.remove("prompt_cache_key").is_some()
        | object.remove("prompt_cache_retention").is_some();
    if !removed {
        return raw.to_vec();
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| raw.to_vec())
}

/// 透传体的唯一改写:`model` 换成入口的 upstreamModel(其余字段——tools、
/// reasoning、include、metadata 等——一字不动)。解析失败则原样发出,
/// 让上游自己拒绝,不在代理侧擅自造 body。
fn rewrite_passthrough_model(
    raw: &[u8],
    upstream_model: &str,
    content_type: Option<&str>,
) -> Vec<u8> {
    if upstream_model.is_empty() {
        return raw.to_vec();
    }
    if content_type.is_some_and(|value| {
        !value
            .trim()
            .to_ascii_lowercase()
            .starts_with("application/json")
    }) {
        return raw.to_vec();
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(raw) else {
        return raw.to_vec();
    };
    match value.as_object_mut() {
        Some(object) => {
            object.insert("model".into(), json!(upstream_model));
            serde_json::to_vec(&value).unwrap_or_else(|_| raw.to_vec())
        }
        None => raw.to_vec(),
    }
}

fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    headers.push((name.to_string(), value.to_string()));
}

fn base_path_of(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .map(|u| u.path().to_string())
        .unwrap_or_else(|_| String::new())
}

/// openai 系路径:base 以 `/v1` 结尾则不再重复补 v1。
fn openai_suffix_path(base_path: &str, suffix: &str) -> String {
    let trimmed = base_path.trim_end_matches('/');
    if trimmed.ends_with("/v1") || trimmed == "v1" {
        format!("{trimmed}{suffix}")
    } else {
        format!("{trimmed}/v1{suffix}")
    }
}

/// `anthropic-beta` 组装:以客户端原值为基底,再补齐代理需要的 base token。
/// `oneMillion` 强制补 `context-1m`;`standard` 保留客户端原值;
/// `strip` 只移除 `context-1m-*`,让上游自行决定上下文能力。
fn beta_header(
    client_beta: Option<&str>,
    context: ContextMode,
    effort: Option<ReasoningEffort>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for token in client_beta.unwrap_or_default().split(',') {
        if context != ContextMode::Strip || !is_context_1m_beta(token) {
            push_beta(&mut parts, token);
        }
    }
    for token in ANTHROPIC_BETA_BASE.split(',') {
        push_beta(&mut parts, token);
    }
    if context == ContextMode::OneMillion {
        push_beta(&mut parts, sumpter_core::config::ANTHROPIC_CONTEXT_1M_BETA);
    }
    if effort.is_some() {
        push_beta(&mut parts, ANTHROPIC_BETA_EFFORT);
    }
    parts.join(",")
}

fn is_context_1m_beta(token: &str) -> bool {
    let token = token.trim().to_ascii_lowercase();
    token == "context-1m" || token.starts_with("context-1m-")
}

/// trim 后追加一个 beta token;空值跳过,已存在(忽略大小写)不重复 —— 重复 beta 会被
/// 部分上游判为非法请求。
fn push_beta(parts: &mut Vec<String>, token: &str) {
    let token = token.trim();
    if token.is_empty() || parts.iter().any(|part| part.eq_ignore_ascii_case(token)) {
        return;
    }
    parts.push(token.to_string());
}

/// anthropic 直连的 body 改写:
/// - `model` 换成上游模型;
/// - 客户端模型带合法 `(effort)` 后缀时优先注入:
///   none = 删 thinking + output_config.effort;auto = thinking adaptive;
///   其余档位 = thinking adaptive + output_config.effort;
/// - 否则按入口 thinking:disabled 删 / adaptive 写 `{"type":"adaptive"}` / passthrough 不动。
fn rewrite_anthropic_body(
    request: &RoutingRequest,
    routed_model: &str,
    upstream_model: &str,
    thinking: ThinkingMode,
    effort: Option<ReasoningEffort>,
    purpose: RequestPurpose,
    server_retrieval: bool,
) -> Vec<u8> {
    let mut body = request.raw.clone();
    body.insert("model".into(), json!(upstream_model));

    // Claude Code 强制指定 WebSearch 工具时使用 `{type:"tool",name:"web_search"}`；
    // Grok 的 Anthropic 兼容层接受 `{type:"any"}`，并由服务端继续执行 web/x 搜索。
    // 只按规则选中的逻辑模型判断，避免 ccc 等入口把上游模型改成不透明别名后失配。
    if server_retrieval {
        if purpose == RequestPurpose::WebFetch {
            body.insert(
                "tools".into(),
                json!([{
                    "name": "web_search",
                    "type": "web_search_20250305",
                    "max_uses": 8
                }]),
            );
        }
        // ccc/Grok 的 Anthropic 兼容层不接受 Claude Code 的命名工具选择
        // `{type:"tool",name:"web_search"}`；`any` 仍强制执行一次服务端检索。
        if model_name::clean(routed_model)
            .to_ascii_lowercase()
            .starts_with("grok-")
        {
            body.insert("tool_choice".into(), json!({"type": "any"}));
        }
    }

    match effort {
        Some(ReasoningEffort::None) => {
            body.remove("thinking");
            remove_output_config_effort(&mut body);
        }
        Some(ReasoningEffort::Auto) => {
            body.insert("thinking".into(), json!({"type": "adaptive"}));
            remove_output_config_effort(&mut body);
        }
        Some(level) => {
            body.insert("thinking".into(), json!({"type": "adaptive"}));
            let config = body
                .entry("output_config".to_string())
                .or_insert_with(|| Value::Object(Default::default()));
            if let Some(map) = config.as_object_mut() {
                map.insert("effort".into(), json!(level.as_str()));
            }
        }
        None => match thinking {
            ThinkingMode::Disabled => {
                body.remove("thinking");
            }
            ThinkingMode::Adaptive => {
                body.insert("thinking".into(), json!({"type": "adaptive"}));
            }
            ThinkingMode::Passthrough => {}
        },
    }
    serde_json::to_vec(&Value::Object(body)).unwrap_or_default()
}

fn remove_output_config_effort(body: &mut serde_json::Map<String, Value>) {
    let mut drop_config = false;
    if let Some(Value::Object(config)) = body.get_mut("output_config") {
        config.remove("effort");
        drop_config = config.is_empty();
    }
    if drop_config {
        body.remove("output_config");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sumpter_core::config::EndpointProtocolMode;
    use sumpter_core::routing::{RouteMode, RoutePlanner};

    fn endpoint(
        protocol: ProviderProtocol,
        context: ContextMode,
        thinking: ThinkingMode,
    ) -> PlannedEndpoint {
        PlannedEndpoint {
            endpoint_id: "e".into(),
            endpoint_name: "e".into(),
            base_url: "https://up.example.com".into(),
            configured_protocol: match protocol {
                ProviderProtocol::Anthropic => EndpointProtocolMode::Anthropic,
                ProviderProtocol::OpenAI => EndpointProtocolMode::OpenAI,
                ProviderProtocol::OpenAIResponses => EndpointProtocolMode::OpenAIResponses,
            },
            source_format: ProviderProtocol::Anthropic,
            protocol,
            route_mode: if protocol == ProviderProtocol::Anthropic {
                RouteMode::Native
            } else {
                RouteMode::Translated
            },
            routed_model: "m".into(),
            upstream_model: "up-model".into(),
            pinned_ips: vec![],
            pinned_ip_exclusive: false,
            priority: 0,
            sticky_group: None,
            thinking,
            context,
            effort_override: None,
            failover_timeout_seconds: None,
            keep_alive: false,
        }
    }

    fn request(model: &str) -> RoutingRequest {
        RoutingRequest::from_value(&json!({
            "model": model,
            "system": "s",
            "messages": [{"role": "user", "content": "hi"}],
            "thinking": {"type": "enabled", "budget_tokens": 1000},
        }))
        .unwrap()
    }

    fn websearch_request() -> RoutingRequest {
        RoutingRequest::from_value(&json!({
            "model": "claude-opus-5",
            "system": "You are an assistant for performing a web search tool use",
            "messages": [{"role": "user", "content": "Perform a web search for the query: x post"}],
            "tools": [{"name": "web_search", "type": "web_search_20250305"}],
            "tool_choice": {"type": "tool", "name": "web_search"},
        }))
        .unwrap()
    }

    fn webfetch_request() -> RoutingRequest {
        RoutingRequest::from_value(&json!({
            "model": "claude-opus-5",
            "system": "You are Claude Code, Anthropic's official CLI for Claude.",
            "messages": [{
                "role": "user",
                "content": "Web page content:\n---\nSource URL: https://x.com/example/status/1\nThe fetch was restricted.\n---\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."
            }],
        }))
        .unwrap()
    }

    fn header<'a>(build: &'a OutboundBuild, name: &str) -> Vec<&'a str> {
        build
            .request
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .collect()
    }

    #[test]
    fn provider_probe_headers_share_the_data_plane_fingerprint() {
        let headers = provider_probe_headers("sk-test");
        assert!(headers.contains(&("authorization", "Bearer sk-test".into())));
        assert!(headers.contains(&("x-api-key", "sk-test".into())));
        assert!(headers.contains(&("user-agent", CLAUDE_CODE_USER_AGENT.into())));
        assert!(headers.contains(&("anthropic-version", ANTHROPIC_VERSION.into())));
        assert!(headers.contains(&("accept-encoding", "identity".into())));
        let anonymous = provider_probe_headers(" ");
        assert!(
            !anonymous
                .iter()
                .any(|(name, _)| matches!(*name, "authorization" | "x-api-key"))
        );
    }

    #[test]
    fn client_declared_project_headers_never_reach_the_upstream() {
        // 这三个 header 是入站专用的项目归因通道(ClientDeclaredMetadata)。它们带着项目名和
        // 本机工作区路径,一旦跟着转发出去就等于把这些信息泄给上游中转站 —— 出站黑名单是
        // 唯一的拦截点,所以在这里钉死。
        let inbound = vec![
            ("X-Sumpter-Project".to_string(), "automode-proxy".to_string()),
            (
                "x-sumpter-workspace".to_string(),
                "/Users/kkl/.claude/automode-proxy".to_string(),
            ),
            (
                "X-Sumpter-Git-Remote".to_string(),
                "https://github.com/domoxiaojun/sumpter.git".to_string(),
            ),
            (
                "X-Kekulv-Project".to_string(),
                "legacy-should-not-leak".to_string(),
            ),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
        ];
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Passthrough,
            ),
            &request("claude-opus-5"),
            &inbound,
            "POST",
            "/v1/messages",
            "sk-key",
            None,
            RequestPurpose::Standard,
            None,
        );
        // 直接钉死黑名单常量:从名单里摘掉任一项都会立刻红,等价于负向验证。
        for name in [
            "x-sumpter-project",
            "x-sumpter-workspace",
            "x-sumpter-git-remote",
            "x-kekulv-project",
            "x-kekulv-workspace",
            "x-kekulv-git-remote",
        ] {
            assert!(
                HEADER_BLOCKLIST.contains(&name),
                "{name} 必须留在出站黑名单里"
            );
            assert!(header(&build, name).is_empty());
        }
        // 出站里不应残留任何 sumpter 私有前缀,也不应有本机路径片段。
        for (name, value) in &build.request.headers {
            assert!(
                !name.starts_with("x-sumpter-"),
                "出站残留私有 header: {name}"
            );
            assert!(
                !value.contains("/Users/"),
                "出站残留本机路径: {name}={value}"
            );
        }
        // 无关 header 仍照常透传。
        assert_eq!(header(&build, "anthropic-version"), vec!["2023-06-01"]);
    }

    #[test]
    fn forces_fingerprint_headers_exactly_once() {
        let inbound = vec![
            (
                "User-Agent".to_string(),
                "some-other-client/1.0".to_string(),
            ),
            ("Content-Type".to_string(), "application/json".to_string()),
            ("content-type".to_string(), "application/json".to_string()),
            ("anthropic-version".to_string(), "2023-06-01".to_string()),
            ("Accept-Encoding".to_string(), "gzip".to_string()),
        ];
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Passthrough,
            ),
            &request("claude-opus-5"),
            &inbound,
            "POST",
            "/v1/messages",
            "sk-key",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(header(&build, "user-agent"), vec!["some-other-client/1.0"]); // 透传
        assert_eq!(header(&build, "content-type"), vec!["application/json"]); // 恰一份
        assert_eq!(header(&build, "authorization"), vec!["Bearer sk-key"]);
        assert_eq!(header(&build, "x-api-key"), vec!["sk-key"]); // 双发
        assert_eq!(header(&build, "accept-encoding"), vec!["identity"]);
        assert_eq!(header(&build, "anthropic-version"), vec!["2023-06-01"]); // 透传
        assert!(header(&build, "host").is_empty());
    }

    #[test]
    fn beta_header_composition() {
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::OneMillion,
                ThinkingMode::Adaptive,
            ),
            &request("claude-opus-5(high)"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(
            header(&build, "anthropic-beta"),
            vec![
                "claude-code-20250219,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12,context-1m-2025-08-07,effort-2025-11-24"
            ]
        );
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(
            header(&build, "anthropic-beta"),
            vec!["claude-code-20250219,interleaved-thinking-2025-05-14,redact-thinking-2026-02-12"]
        );
    }

    #[test]
    fn standard_context_passes_client_beta_through_without_stripping_1m() {
        // standard + 客户端自带 context-1m → 保留(旧行为是整头重建剥掉它)。
        let client = vec![(
            "anthropic-beta".to_string(),
            "claude-code-20250219,context-1m-2025-08-07,fine-grained-tool-streaming-2025-05-14"
                .to_string(),
        )];
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5"),
            &client,
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let sent = header(&build, "anthropic-beta");
        assert_eq!(sent.len(), 1, "只发一份 anthropic-beta");
        let tokens: Vec<&str> = sent[0].split(',').collect();
        assert!(
            tokens.contains(&"context-1m-2025-08-07"),
            "客户端的 1M 不被剥掉"
        );
        assert!(
            tokens.contains(&"fine-grained-tool-streaming-2025-05-14"),
            "客户端其它 beta 一并透传"
        );
        for base in ANTHROPIC_BETA_BASE.split(',') {
            assert_eq!(
                tokens.iter().filter(|t| **t == base).count(),
                1,
                "{base} 恰一份"
            );
        }
        assert!(
            !tokens.contains(&ANTHROPIC_BETA_EFFORT),
            "无 effort 后缀不加档位"
        );
    }

    #[test]
    fn one_million_context_forces_1m_even_when_client_omits_it() {
        let client = vec![(
            "anthropic-beta".to_string(),
            "claude-code-20250219".to_string(),
        )];
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::OneMillion,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5"),
            &client,
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let tokens: Vec<&str> = header(&build, "anthropic-beta")[0].split(',').collect();
        assert!(tokens.contains(&"context-1m-2025-08-07"));
        assert_eq!(
            tokens
                .iter()
                .filter(|t| **t == "context-1m-2025-08-07")
                .count(),
            1,
            "强制添加时恰一份"
        );
    }

    #[test]
    fn strip_context_removes_only_client_1m_betas() {
        let client = vec![(
            "anthropic-beta".to_string(),
            "claude-code-20250219,context-1m-2025-08-07,CONTEXT-1M-EXPERIMENTAL,fine-grained-tool-streaming-2025-05-14"
                .to_string(),
        )];
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Strip,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5"),
            &client,
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let tokens: Vec<&str> = header(&build, "anthropic-beta")[0].split(',').collect();
        assert!(
            tokens
                .iter()
                .all(|token| !token.to_ascii_lowercase().starts_with("context-1m")),
            "客户端声明的所有 1M beta 都被剥离"
        );
        assert!(tokens.contains(&"fine-grained-tool-streaming-2025-05-14"));
        assert!(tokens.contains(&"claude-code-20250219"));
        for base in ANTHROPIC_BETA_BASE.split(',') {
            assert_eq!(tokens.iter().filter(|token| **token == base).count(), 1);
        }
    }

    #[test]
    fn anthropic_body_rewrites_model_and_thinking() {
        // passthrough:thinking 原样保留。
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Passthrough,
            ),
            &request("claude-opus-5"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["model"], "up-model");
        assert_eq!(body["thinking"]["type"], "enabled");

        // disabled:删 thinking。
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert!(body.get("thinking").is_none());

        // adaptive:改写为 adaptive。
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Adaptive,
            ),
            &request("claude-opus-5"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
    }

    #[test]
    fn grok_websearch_rewrites_named_tool_choice_only_for_that_family_and_purpose() {
        let mut ep = endpoint(
            ProviderProtocol::Anthropic,
            ContextMode::Standard,
            ThinkingMode::Passthrough,
        );
        ep.routed_model = "grok-4.5".into();
        ep.upstream_model = "opaque-upstream-alias".into();
        let request = websearch_request();
        let build = build_outbound(
            &ep,
            &request,
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::WebSearch,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["model"], "opaque-upstream-alias");
        assert_eq!(body["tool_choice"], json!({"type": "any"}));

        ep.routed_model = "claude-opus-5".into();
        let build = build_outbound(
            &ep,
            &request,
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::WebSearch,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(
            body["tool_choice"],
            json!({"type": "tool", "name": "web_search"})
        );

        ep.routed_model = "grok-4.5".into();
        let build = build_outbound(
            &ep,
            &request,
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(
            body["tool_choice"],
            json!({"type": "tool", "name": "web_search"})
        );
    }

    #[test]
    fn grok_webfetch_and_websearch_enable_server_retrieval_across_all_protocols() {
        for protocol in [
            ProviderProtocol::Anthropic,
            ProviderProtocol::OpenAI,
            ProviderProtocol::OpenAIResponses,
        ] {
            let mut ep = endpoint(protocol, ContextMode::Standard, ThinkingMode::Passthrough);
            ep.routed_model = "GROK-4.5[1m]".into();
            ep.upstream_model = "opaque-upstream-alias".into();

            for (request, purpose) in [
                (websearch_request(), RequestPurpose::WebSearch),
                (webfetch_request(), RequestPurpose::WebFetch),
            ] {
                assert!(server_retrieval_enabled(&ep, &request, purpose));
                let build = build_outbound(
                    &ep,
                    &request,
                    &[],
                    "POST",
                    "/v1/messages",
                    "k",
                    None,
                    purpose,
                    None,
                );
                let body: Value = serde_json::from_slice(&build.request.body).unwrap();
                match protocol {
                    ProviderProtocol::Anthropic => {
                        assert_eq!(body["tool_choice"], json!({"type": "any"}));
                        assert_eq!(body["tools"][0]["name"], "web_search");
                    }
                    ProviderProtocol::OpenAI => {
                        assert_eq!(build.request.path_and_query, "/v1/chat/completions");
                        assert_eq!(body["web_search_options"], json!({}));
                    }
                    ProviderProtocol::OpenAIResponses => {
                        assert_eq!(build.request.path_and_query, "/v1/responses");
                        assert_eq!(body["tools"], json!([{"type": "web_search"}]));
                    }
                }
            }
        }

        let mut non_grok = endpoint(
            ProviderProtocol::Anthropic,
            ContextMode::Standard,
            ThinkingMode::Passthrough,
        );
        non_grok.routed_model = "claude-opus-5".into();
        assert!(server_retrieval_enabled(
            &non_grok,
            &websearch_request(),
            RequestPurpose::WebSearch
        ));
        assert!(!server_retrieval_enabled(
            &non_grok,
            &webfetch_request(),
            RequestPurpose::WebFetch
        ));
        let build = build_outbound(
            &non_grok,
            &webfetch_request(),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::WebFetch,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());

        non_grok.routed_model = "grok-4.5".into();
        assert!(!server_retrieval_enabled(
            &non_grok,
            &request("grok-4.5"),
            RequestPurpose::Standard
        ));

        let no_url = RoutingRequest::from_value(&json!({
            "model": "claude-opus-5",
            "system": "You are Claude Code, Anthropic's official CLI for Claude.",
            "messages": [{"role": "user", "content": "Web page content:\n---\nfull body only\n---\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."}],
        }))
        .unwrap();
        assert!(!server_retrieval_enabled(
            &non_grok,
            &no_url,
            RequestPurpose::WebFetch
        ));
    }

    #[test]
    fn effort_suffix_overrides_endpoint_thinking() {
        // (high):adaptive + output_config.effort。
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Disabled,
            ),
            &request("claude-opus-5(high)"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert_eq!(body["output_config"]["effort"], "high");

        // (none):删 thinking 与 output_config.effort。
        let mut raw = json!({
            "model": "m(none)",
            "messages": [{"role": "user", "content": "x"}],
            "thinking": {"type": "adaptive"},
            "output_config": {"effort": "high"},
        });
        let req = RoutingRequest::from_value(&raw).unwrap();
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Adaptive,
            ),
            &req,
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert!(body.get("thinking").is_none());
        assert!(body.get("output_config").is_none());

        // (auto):thinking adaptive、不写 effort。
        raw["model"] = json!("m(auto)");
        let req = RoutingRequest::from_value(&raw).unwrap();
        let build = build_outbound(
            &endpoint(
                ProviderProtocol::Anthropic,
                ContextMode::Standard,
                ThinkingMode::Disabled,
            ),
            &req,
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["thinking"], json!({"type": "adaptive"}));
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn feature_route_effort_override_wins_client_suffix_for_all_protocols() {
        let request = request("claude-opus-5(low)");

        for protocol in [
            ProviderProtocol::Anthropic,
            ProviderProtocol::OpenAI,
            ProviderProtocol::OpenAIResponses,
        ] {
            let mut target = endpoint(protocol, ContextMode::Standard, ThinkingMode::Disabled);
            target.effort_override = Some(ReasoningEffort::High);
            let build = build_outbound(
                &target,
                &request,
                &[],
                "POST",
                "/v1/messages",
                "k",
                None,
                RequestPurpose::Classifier,
                None,
            );
            assert_eq!(build.effort, Some(ReasoningEffort::High));
            let body: Value = serde_json::from_slice(&build.request.body).unwrap();
            match protocol {
                ProviderProtocol::Anthropic => {
                    assert_eq!(body["output_config"]["effort"], "high");
                }
                ProviderProtocol::OpenAI => {
                    assert_eq!(body["reasoning_effort"], "high");
                }
                ProviderProtocol::OpenAIResponses => {
                    assert_eq!(body["reasoning"]["effort"], "high");
                }
            }
        }
    }

    #[test]
    fn openai_paths_and_bodies() {
        let mut ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("m"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(build.request.path_and_query, "/v1/chat/completions");
        assert_eq!(header(&build, "accept"), vec!["text/event-stream"]);
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["model"], "up-model");
        assert!(body.get("messages").is_some());

        // base 已以 /v1 结尾则不重复补。
        ep.base_url = "https://up.example.com/v1".into();
        let build = build_outbound(
            &ep,
            &request("m"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(build.request.path_and_query, "/v1/chat/completions");

        ep.protocol = ProviderProtocol::OpenAIResponses;
        ep.base_url = "https://up.example.com".into();
        let build = build_outbound(
            &ep,
            &request("m"),
            &[],
            "POST",
            "/v1/messages",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(build.request.path_and_query, "/v1/responses");
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert!(body.get("input").is_some());
        assert!(body.get("messages").is_none());
    }

    #[test]
    fn anthropic_path_joins_base_path() {
        let mut ep = endpoint(
            ProviderProtocol::Anthropic,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        ep.base_url = "https://coding.dashscope.aliyuncs.com/apps/anthropic".into();
        let build = build_outbound(
            &ep,
            &request("m"),
            &[],
            "POST",
            "/v1/messages?beta=true",
            "k",
            None,
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(
            build.request.path_and_query,
            "/apps/anthropic/v1/messages?beta=true"
        );
    }

    // 保证 RoutePlanner 类型在本 crate 可见(联动编译检查)。
    #[allow(dead_code)]
    fn _planner_visible(_: RoutePlanner) {}
}
