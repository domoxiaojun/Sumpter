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
/// Codex-compatible identity required by several Responses upstream gateways.
pub const CODEX_USER_AGENT: &str = "codex_cli_rs/0.5.0";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Codex Desktop's private Live/Quicksilver bootstrap model.  It is distinct
/// from the ordinary text model selected by the same client session.
pub const DEFAULT_CODEX_LIVE_MODEL: &str = "gpt-live-1-codex";
/// Standard OpenAI Realtime's default stateful voice model.  Keep it separate
/// from Codex Live so a missing query/session model cannot inherit a text
/// mapping or the private quicksilver model.
pub const DEFAULT_REALTIME_MODEL: &str = "gpt-realtime";
const IDENTITY_ACCEPT_ENCODING: &str = "identity";

/// Authentication headers forced by the real data plane and reused by
/// daemon-originated Provider probes.
pub fn provider_auth_headers(api_key: &str) -> Vec<(&'static str, String)> {
    let key = api_key.trim();
    if key.is_empty() {
        Vec::new()
    } else if key.starts_with("ek_") {
        // Realtime client secrets are bearer credentials, not provider API
        // keys.  Sending the same short-lived token in both Authorization and
        // x-api-key can make upstream gateways treat the request as having
        // conflicting credentials.
        vec![("authorization", format!("Bearer {key}"))]
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
const HEADER_BLOCKLIST: [&str; 18] = [
    "host",
    "x-sumpter-project",
    "x-sumpter-workspace",
    "x-sumpter-git-remote",
    "x-sumpter-user",
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
    /// Arbitrary protocol/resource request.  The engine must not classify or
    /// reshape its payload; this marker only keeps runtime accounting useful.
    Raw,
    Chat,
    Completions,
    Responses,
    ResponsesCompact,
    ImagesGenerations,
    ImagesEdits,
    AlphaSearch,
    ClaudeCountTokens,
    /// OpenAI resource APIs (Files/Videos) and Realtime HTTP bootstrap. These
    /// keep the full inbound path because resource IDs are part of the URL.
    Files,
    Videos,
    Realtime,
    /// OpenAI model discovery. The response is owned by the upstream Provider;
    /// Sumpter only routes and relays it.
    Models,
}

/// CPA's Codex Live handler authenticates the downstream request with the
/// configured API key and then selects a Codex OAuth account itself. Keep the
/// Live hop identical to a direct CPA call: a single Bearer credential is
/// sufficient and avoids presenting the same key through two auth schemes to
/// gateways that treat `x-api-key` as a separate credential.
fn auth_headers_for_kind(
    api_key: &str,
    kind: Option<PassthroughKind>,
) -> Vec<(&'static str, String)> {
    if kind == Some(PassthroughKind::Realtime) {
        let key = api_key.trim();
        if key.is_empty() {
            Vec::new()
        } else {
            vec![("authorization", format!("Bearer {key}"))]
        }
    } else {
        provider_auth_headers(api_key)
    }
}

impl PassthroughKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Raw => "/",
            Self::Chat => "/chat/completions",
            Self::Completions => "/completions",
            Self::Responses => "/responses",
            Self::ResponsesCompact => "/responses/compact",
            Self::ImagesGenerations => "/images/generations",
            Self::ImagesEdits => "/images/edits",
            Self::AlphaSearch => "/alpha/search",
            Self::ClaudeCountTokens => "/messages/count_tokens",
            Self::Files => "/files",
            Self::Videos => "/videos",
            Self::Realtime => "/realtime",
            Self::Models => "/models",
        }
    }

    pub fn token(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Chat => "chat",
            Self::Completions => "completions",
            Self::Responses => "responses",
            Self::ResponsesCompact => "responses-compact",
            Self::ImagesGenerations => "images-generations",
            Self::ImagesEdits => "images-edits",
            Self::AlphaSearch => "alpha-search",
            Self::ClaudeCountTokens => "claude-count-tokens",
            Self::Files => "files",
            Self::Videos => "videos",
            Self::Realtime => "realtime",
            Self::Models => "models",
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
    purpose: RequestPurpose,
    // Data-plane passthrough: keep the client's original bytes and path. The
    // active Raw mode only changes an existing model field when its mapping
    // names a different upstream model.
    passthrough: Option<PassthroughRequest<'_>>,
) -> OutboundBuild {
    let raw_passthrough = passthrough
        .as_ref()
        .is_some_and(|request| request.kind == PassthroughKind::Raw);
    let effort = endpoint
        .effort_override
        .or_else(|| model_name::reasoning_effort(&request.model));
    let server_retrieval = server_retrieval_enabled(endpoint, request, purpose);
    let protocol = endpoint.protocol;

    // 1. 透传非黑名单 header。旧桥接路径会在这里去重；Raw 路径随后从
    // 原始 header 对重建，保留可转发字段的顺序与重复值。
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
    for (name, value) in
        auth_headers_for_kind(api_key, passthrough.as_ref().map(|request| request.kind))
    {
        set_header(&mut headers, name, &value);
    }
    set_header(&mut headers, "accept-encoding", IDENTITY_ACCEPT_ENCODING);
    set_header(&mut headers, "content-type", "application/json");
    // Legacy bridged requests get a Claude-compatible fallback UA. Raw
    // passthrough rebuilds its header list below without this synthetic value,
    // so arbitrary vendor requests stay wire-faithful.
    if !headers.iter().any(|(n, _)| n == "user-agent") {
        let fallback_ua = if protocol == ProviderProtocol::OpenAIResponses {
            CODEX_USER_AGENT
        } else {
            CLAUDE_CODE_USER_AGENT
        };
        set_header(&mut headers, "user-agent", fallback_ua);
    }

    if raw_passthrough {
        // Rebuild the raw header list from the original pairs so duplicate
        // negotiation/vendor headers and their order survive the proxy. The
        // two media-negotiation headers are intentionally included here even
        // though the legacy blocklist uses them for its synthetic defaults;
        // all other blocked transport/auth/private headers stay removed.
        headers.clear();
        for (name, value) in inbound_headers {
            let lower = name.to_ascii_lowercase();
            if HEADER_BLOCKLIST.contains(&lower.as_str())
                && !matches!(lower.as_str(), "content-type" | "accept-encoding")
            {
                continue;
            }
            headers.push((lower, value.clone()));
        }
        for (name, value) in
            auth_headers_for_kind(api_key, passthrough.as_ref().map(|request| request.kind))
        {
            headers.push((name.to_string(), value));
        }
    }

    let base_path = base_path_of(&endpoint.base_url);

    // 透传:出站即客户端方言,零转换(工具/reasoning 原生保真)。
    if let Some(passthrough) = passthrough {
        let inbound_accept = headers
            .iter()
            .rev()
            .find(|(name, _)| name.eq_ignore_ascii_case("accept"))
            .map(|(_, value)| value.clone());
        let is_resource_content = matches!(
            passthrough.kind,
            PassthroughKind::Files | PassthroughKind::Videos
        ) && inbound_path_and_query
            .split_once('?')
            .map_or(inbound_path_and_query, |(path, _)| path)
            .to_ascii_lowercase()
            .ends_with("/content");
        if raw_passthrough {
            // A raw relay keeps negotiation headers exactly as supplied by
            // the caller.  Synthetic JSON/SSE defaults are protocol logic,
            // not proxy behavior.
        } else {
            set_header(
                &mut headers,
                "accept",
                if is_resource_content {
                    // Content downloads are binary (or provider-selected media).
                    // Do not advertise JSON merely because the request is a
                    // non-streaming passthrough; callers may provide a narrower
                    // Accept value and otherwise */* is the interoperable default.
                    inbound_accept.as_deref().unwrap_or("*/*")
                } else if passthrough.kind == PassthroughKind::Realtime
                    || passthrough.kind == PassthroughKind::Models
                {
                    // Realtime HTTP bootstrap may return SDP rather than JSON;
                    // preserve the caller's preference and otherwise negotiate
                    // either media or JSON with the provider.
                    inbound_accept.as_deref().unwrap_or("*/*")
                } else if passthrough.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            );
        }
        if !raw_passthrough && (!passthrough.body.is_empty() || passthrough.content_type.is_some())
        {
            set_header(
                &mut headers,
                "content-type",
                passthrough.content_type.unwrap_or("application/json"),
            );
        } else if !raw_passthrough {
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
        }
        if passthrough.kind == PassthroughKind::AlphaSearch && !raw_passthrough {
            set_header(&mut headers, "originator", "codex_cli_rs");
        }
        let passthrough_body =
            if passthrough.kind == PassthroughKind::AlphaSearch && !raw_passthrough {
                sanitize_alpha_search_body(passthrough.body)
            } else {
                passthrough.body.to_vec()
            };
        let path_and_query = if passthrough.kind == PassthroughKind::Realtime {
            rewrite_realtime_model_query(
                &openai_resource_path(&base_path, inbound_path_and_query),
                &request.model,
                &endpoint.upstream_model,
            )
        } else {
            openai_resource_path(&base_path, inbound_path_and_query)
        };
        return OutboundBuild {
            request: OutboundRequest {
                method: inbound_method.to_string(),
                base_url: endpoint.base_url.clone(),
                path_and_query: if matches!(
                    passthrough.kind,
                    PassthroughKind::Raw
                        | PassthroughKind::Files
                        | PassthroughKind::Videos
                        | PassthroughKind::Realtime
                        | PassthroughKind::Models
                ) {
                    path_and_query
                } else {
                    openai_suffix_path(&base_path, passthrough.kind.suffix())
                },
                headers,
                // JSON bodies can safely follow feature-rule model rewrites. Multipart
                // uploads stay byte-for-byte intact so boundaries and binary images are
                // never reconstructed by the proxy.
                body: rewrite_passthrough_model(
                    &passthrough_body,
                    &endpoint.upstream_model,
                    passthrough.content_type,
                    passthrough.kind,
                ),
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

/// 透传体只做必要的模型改写(其它字段——tools、reasoning、include、metadata
/// 等——一字不动)。Files 没有模型字段；Realtime client-secrets 把模型放在
/// `session.model`，两者都保持原有 JSON 结构。解析失败则原样发出，让上游自己
/// 拒绝，不在代理侧擅自造 body。
fn rewrite_passthrough_model(
    raw: &[u8],
    upstream_model: &str,
    content_type: Option<&str>,
    kind: PassthroughKind,
) -> Vec<u8> {
    if upstream_model.is_empty() {
        return raw.to_vec();
    }
    // Without an explicit JSON media type the payload is opaque.  In
    // particular, a vendor may send JSON-looking bytes as a signed or
    // content-negotiated document; reparsing and reserializing it would break
    // the promised raw relay.  Callers that want model mapping must declare
    // application/json (parameters such as charset are accepted).
    let Some(content_type) = content_type else {
        return raw.to_vec();
    };
    if !content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return raw.to_vec();
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(raw) else {
        return raw.to_vec();
    };
    let Some(object) = value.as_object_mut() else {
        return raw.to_vec();
    };
    match kind {
        // Files requests have no model field; routing uses a configured
        // fallback but must not mutate the multipart/JSON resource payload.
        PassthroughKind::Raw => {
            if let Some(model) = object.get("model").and_then(Value::as_str) {
                if model != upstream_model {
                    object.insert("model".into(), json!(upstream_model));
                } else {
                    return raw.to_vec();
                }
            } else if let Some(session) = object.get_mut("session").and_then(Value::as_object_mut)
                && let Some(model) = session.get("model").and_then(Value::as_str)
            {
                if model != upstream_model {
                    session.insert("model".into(), json!(upstream_model));
                } else {
                    return raw.to_vec();
                }
            } else {
                return raw.to_vec();
            }
        }
        PassthroughKind::Files | PassthroughKind::Videos | PassthroughKind::Models => {
            return raw.to_vec();
        }
        PassthroughKind::Realtime => {
            let mut changed = false;
            if object.get("model").is_some_and(Value::is_string) {
                changed = object
                    .get("model")
                    .and_then(Value::as_str)
                    .is_none_or(|model| model != upstream_model);
                if changed {
                    object.insert("model".into(), json!(upstream_model));
                }
            } else if let Some(session) = object.get_mut("session").and_then(Value::as_object_mut)
                && session.get("model").is_some_and(Value::is_string)
            {
                changed = session
                    .get("model")
                    .and_then(Value::as_str)
                    .is_none_or(|model| model != upstream_model);
                if changed {
                    session.insert("model".into(), json!(upstream_model));
                }
            }
            if !changed {
                return raw.to_vec();
            }
        }
        _ => {
            if object
                .get("model")
                .and_then(Value::as_str)
                .is_some_and(|model| model == upstream_model)
            {
                return raw.to_vec();
            }
            object.insert("model".into(), json!(upstream_model));
        }
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| raw.to_vec())
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

/// Preserve the inbound resource path and query when joining it to the
/// configured Provider base path. The only normalization is removing a
/// duplicated base suffix such as base `/v1` + inbound `/v1/files`; aliases
/// like `/files` or `/openai/v1/files` are never rewritten to another path.
pub fn openai_resource_path(base_path: &str, inbound_path_and_query: &str) -> String {
    let (raw_path, query) = inbound_path_and_query
        .split_once('?')
        .map_or((inbound_path_and_query, None), |(path, query)| {
            (path, Some(query))
        });
    let path = if raw_path.starts_with('/') {
        raw_path.to_string()
    } else {
        format!("/{raw_path}")
    };
    let trimmed_base = base_path.trim_end_matches('/');
    let output = if trimmed_base.ends_with("/v1") && (path == "/v1" || path.starts_with("/v1/")) {
        format!("{trimmed_base}{}", &path[3..])
    } else {
        format!("{trimmed_base}{path}")
    };
    match query {
        Some(query) if !query.is_empty() => format!("{output}?{query}"),
        _ => output,
    }
}

/// Rewrite an explicit Realtime `model` query parameter when a configured
/// mapping uses a different upstream model. Query order and all unrelated
/// parameters stay byte-for-byte unchanged; a missing model is not invented.
pub(crate) fn rewrite_realtime_model_query(
    path_and_query: &str,
    client_model: &str,
    upstream_model: &str,
) -> String {
    let replacement = if upstream_model.is_empty() {
        client_model
    } else {
        upstream_model
    };
    if replacement.is_empty() {
        return path_and_query.to_string();
    }
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_string();
    };
    let mut changed = false;
    let query = query
        .split('&')
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            if query_name_is_model(name) && value != replacement {
                changed = true;
                format!("{name}={replacement}")
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    if changed {
        format!("{path}?{query}")
    } else {
        path_and_query.to_string()
    }
}

/// Match a query key without normalizing or re-encoding the query string.
/// Percent escapes are decoded strictly; malformed escapes do not match and
/// are preserved verbatim.  The original key spelling (including casing and
/// escapes) is retained when its value is replaced.
fn query_name_is_model(raw_name: &str) -> bool {
    let bytes = raw_name.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 >= bytes.len() {
                    return false;
                }
                let Some(high) = hex_digit(bytes[index + 1]) else {
                    return false;
                };
                let Some(low) = hex_digit(bytes[index + 2]) else {
                    return false;
                };
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    std::str::from_utf8(&decoded)
        .ok()
        .is_some_and(|name| name.eq_ignore_ascii_case("model"))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Drop `intent` / `architecture` so a Codex Desktop GET `/v1/realtime`
/// handshake cannot leak Quicksilver WebRTC markers onto standard Realtime WS.
pub(crate) fn strip_codex_live_query(path_and_query: &str) -> String {
    let Some((path, query)) = path_and_query.split_once('?') else {
        return path_and_query.to_string();
    };
    let pairs = query
        .split('&')
        .filter(|pair| {
            if pair.is_empty() {
                return false;
            }
            let name = pair.split_once('=').map_or(*pair, |(name, _)| name);
            !name.eq_ignore_ascii_case("intent") && !name.eq_ignore_ascii_case("architecture")
        })
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", pairs.join("&"))
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

/// WebFetch 分流时合成的服务端搜索工具类型。
///
/// 这里只能是固定值:WebFetch 请求本身不带 tools(那正是它的指纹之一),没有客户端
/// 声明可继承。WebSearch 分流不同 —— 那类请求自带 tools,`server_retrieval` 不改写
/// 它们,客户端声明的版本(含 CC 后续版本的新类型)会原样透传。
///
/// 目标上游是 ccc / Grok 的 Anthropic 兼容层,它们按工具名而非类型版本派发,所以
/// 这个值滞后于 Anthropic 官方新版(如 `web_search_20260209`)不影响检索执行。若将来
/// 要对接严格校验类型版本的上游,应改成按入口配置而不是继续加硬编码分支。
const SERVER_WEB_SEARCH_TOOL_TYPE: &str = "web_search_20250305";

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
                    "type": SERVER_WEB_SEARCH_TOOL_TYPE,
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
        let ephemeral = provider_auth_headers("ek_short_lived");
        assert_eq!(
            ephemeral,
            vec![("authorization", "Bearer ek_short_lived".into())]
        );
    }

    #[test]
    fn realtime_passthrough_uses_direct_cpa_bearer_auth() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("gpt-live-1-codex"),
            &[],
            "POST",
            "/v1/live",
            "cpa-key",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Realtime,
                body: b"v=0\r\n",
                content_type: Some("application/sdp"),
                stream: false,
            }),
        );
        assert_eq!(header(&build, "authorization"), vec!["Bearer cpa-key"]);
        assert!(header(&build, "x-api-key").is_empty());
    }

    #[test]
    fn client_declared_project_headers_never_reach_the_upstream() {
        // 这三个 header 是入站专用的项目归因通道(ClientDeclaredMetadata)。它们带着项目名和
        // 本机工作区路径,一旦跟着转发出去就等于把这些信息泄给上游中转站 —— 出站黑名单是
        // 唯一的拦截点,所以在这里钉死。
        let inbound = vec![
            (
                "X-Sumpter-Project".to_string(),
                "automode-proxy".to_string(),
            ),
            (
                "x-sumpter-workspace".to_string(),
                "/Users/kkl/.claude/automode-proxy".to_string(),
            ),
            (
                "X-Sumpter-Git-Remote".to_string(),
                "https://github.com/domoxiaojun/sumpter.git".to_string(),
            ),
            ("X-Sumpter-User".to_string(), "kkl".to_string()),
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
            RequestPurpose::Standard,
            None,
        );
        // 直接钉死黑名单常量:从名单里摘掉任一项都会立刻红,等价于负向验证。
        for name in [
            "x-sumpter-project",
            "x-sumpter-workspace",
            "x-sumpter-git-remote",
            "x-sumpter-user",
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
            RequestPurpose::Standard,
            None,
        );
        assert_eq!(build.request.path_and_query, "/v1/responses");
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert!(body.get("input").is_some());
        assert!(body.get("messages").is_none());
    }

    #[test]
    fn resource_content_passthrough_preserves_accept_and_omits_empty_content_type() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("m"),
            &[("accept".into(), "video/mp4".into())],
            "GET",
            "/v1/videos/video_123/content?variant=video",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Videos,
                body: &[],
                content_type: None,
                stream: false,
            }),
        );
        assert_eq!(
            build.request.path_and_query,
            "/v1/videos/video_123/content?variant=video"
        );
        assert_eq!(header(&build, "accept"), vec!["video/mp4"]);
        assert!(header(&build, "content-type").is_empty());
        assert_eq!(build.request.body, Vec::<u8>::new());
    }

    #[test]
    fn realtime_http_passthrough_preserves_sdp_accept_and_media_type() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("gpt-realtime"),
            &[
                ("accept".into(), "application/sdp".into()),
                ("content-type".into(), "application/sdp".into()),
            ],
            "POST",
            "/v1/realtime/calls?model=gpt-realtime",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Realtime,
                body: b"v=0\r\n",
                content_type: Some("application/sdp"),
                stream: false,
            }),
        );
        assert_eq!(header(&build, "accept"), vec!["application/sdp"]);
        assert_eq!(header(&build, "content-type"), vec!["application/sdp"]);
        assert_eq!(build.request.body, b"v=0\r\n");
    }

    #[test]
    fn realtime_query_model_follows_explicit_upstream_mapping() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("gpt-realtime"),
            &[],
            "POST",
            "/v1/realtime/calls?model=gpt-realtime&voice=alloy",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Realtime,
                body: br#"{"session":{"model":"gpt-realtime"}}"#,
                content_type: Some("application/json"),
                stream: false,
            }),
        );
        assert_eq!(
            build.request.path_and_query,
            "/v1/realtime/calls?model=up-model&voice=alloy"
        );
        let leaked = build_outbound(
            &ep,
            &request("gpt-live-1-codex"),
            &[],
            "POST",
            "/v1/realtime?model=claude-fable-5",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Realtime,
                body: b"v=0\r\n",
                content_type: Some("application/sdp"),
                stream: false,
            }),
        );
        assert_eq!(leaked.request.path_and_query, "/v1/realtime?model=up-model");
        let body: Value = serde_json::from_slice(&build.request.body).unwrap();
        assert_eq!(body["session"]["model"], "up-model");
    }

    #[test]
    fn realtime_query_rewrite_handles_encoded_duplicate_and_malformed_keys() {
        assert_eq!(
            rewrite_realtime_model_query(
                "/v1/realtime?MODEL=old&voice=alloy&m%6fdel=old2",
                "client",
                "upstream",
            ),
            "/v1/realtime?MODEL=upstream&voice=alloy&m%6fdel=upstream"
        );
        // A malformed escape is not a model key and must remain byte-for-byte
        // unchanged along with all unrelated query parameters.
        assert_eq!(
            rewrite_realtime_model_query("/v1/realtime?m%6odel=old&trace=1", "client", "upstream",),
            "/v1/realtime?m%6odel=old&trace=1"
        );
        assert_eq!(
            rewrite_realtime_model_query(
                "/v1/realtime?model=upstream&model=old",
                "client",
                "upstream",
            ),
            "/v1/realtime?model=upstream&model=upstream"
        );
    }

    #[test]
    fn websocket_sideband_query_strips_http_only_quicksilver_markers() {
        assert_eq!(
            strip_codex_live_query(
                "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver&architecture=avas&trace=1",
            ),
            "/v1/realtime?model=gpt-live-1-codex&trace=1"
        );
        assert_eq!(
            strip_codex_live_query("/v1/realtime?architecture=avas"),
            "/v1/realtime"
        );
    }

    #[test]
    fn raw_passthrough_does_not_guess_json_without_content_type() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let raw = br#"{"model":"m","signed":"  keep spacing  "}"#;
        let build = build_outbound(
            &ep,
            &request("m"),
            &[],
            "POST",
            "/vendor/request",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Raw,
                body: raw,
                content_type: None,
                stream: false,
            }),
        );
        assert_eq!(build.request.body, raw);
        assert!(header(&build, "content-type").is_empty());
    }

    #[test]
    fn raw_passthrough_preserves_duplicate_forwardable_headers() {
        let ep = endpoint(
            ProviderProtocol::OpenAI,
            ContextMode::Standard,
            ThinkingMode::Disabled,
        );
        let build = build_outbound(
            &ep,
            &request("m"),
            &[
                ("Accept".into(), "application/vnd.one".into()),
                ("accept".into(), "application/vnd.two".into()),
                ("X-Vendor-Signature".into(), "sig-a".into()),
                ("x-vendor-signature".into(), "sig-b".into()),
                ("Host".into(), "client.invalid".into()),
            ],
            "POST",
            "/vendor/request",
            "k",
            RequestPurpose::Standard,
            Some(PassthroughRequest {
                kind: PassthroughKind::Raw,
                body: b"payload",
                content_type: None,
                stream: false,
            }),
        );
        assert_eq!(
            header(&build, "accept"),
            vec!["application/vnd.one", "application/vnd.two"]
        );
        assert_eq!(header(&build, "x-vendor-signature"), vec!["sig-a", "sig-b"]);
        assert!(header(&build, "host").is_empty());
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
