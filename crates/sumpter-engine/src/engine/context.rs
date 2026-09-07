//! Request identity, client metadata, authorization headers and bounded context.

use crate::request_build::PassthroughKind;
use bytes::Bytes;
use std::cell::RefCell;
use sumpter_core::bridge_in::ClientDialect;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::ClientDeclaredMetadata;
use sumpter_core::events::ClientKind;
use sumpter_core::events::CodexMetadata;
use sumpter_core::events::GrokMetadata;
use sumpter_core::events::KIND_CLIENT;
use sumpter_core::events::KIND_UPSTREAM;
use sumpter_core::events::RuntimeEvent;
use sumpter_core::routing::RequestPurpose;
use sumpter_core::routing::RouteMode;
use sumpter_core::stream_terminal::SseDialect;

/// Context kept for the portion of an inbound task that can reject before a
/// `CompletionGuard` exists.  In particular, auth/body/route failures should
/// still say which HTTP surface rejected them without threading three extra
/// arguments through every validation branch.  The values are bounded and the
/// path never contains a query string.
#[derive(Clone)]
pub(super) struct InboundRequestContext {
    pub(super) method: String,
    pub(super) path: String,
    pub(super) route_intent: String,
    /// Bounded client/session identity captured before body parsing.  This is
    /// intentionally header-only at this layer; body metadata is merged by
    /// the request handler when it is available.
    pub(super) session_id: Option<String>,
    pub(super) grok_metadata: Option<GrokMetadata>,
}

tokio::task_local! {
    pub(super) static INBOUND_REQUEST_CONTEXT: RefCell<InboundRequestContext>;
}

/// Merge a preferred bounded Codex projection with a fallback. Callers put
/// first-frame body metadata before handshake compatibility headers. The
/// helper intentionally copies only identity/diagnostic fields that are safe
/// to retain; prompt and transport payload fields are never introduced here.
pub(super) fn merge_codex_metadata(
    base: Option<CodexMetadata>,
    overlay: Option<CodexMetadata>,
) -> Option<CodexMetadata> {
    let Some(mut overlay) = overlay else {
        return base;
    };
    let Some(mut base) = base else {
        return Some(overlay);
    };
    let inherits_parent = base.parent_thread_id.is_none();
    if base.parent_thread_id_inferred
        && overlay.parent_thread_id.is_some()
        && !overlay.parent_thread_id_inferred
    {
        // An authoritative header parent beats a parent inferred from a fork.
        base.parent_thread_id = overlay.parent_thread_id.clone();
        base.parent_thread_id_inferred = false;
    }
    macro_rules! fill {
        ($field:ident) => {
            if let Some(value) = overlay.$field.take() {
                match &base.$field {
                    Some(preferred) if preferred != &value => {
                        base.has_conflicts = true;
                        let conflict =
                            concat!(stringify!($field), ":websocketFallback").to_string();
                        if !base.conflicts.contains(&conflict) {
                            base.conflicts.push(conflict);
                        }
                    }
                    Some(_) => {}
                    None => base.$field = Some(value),
                }
            }
        };
    }
    fill!(session_id);
    fill!(thread_id);
    fill!(turn_id);
    fill!(installation_id);
    fill!(source_installation_id);
    fill!(agent_name);
    fill!(window_id);
    fill!(window_number);
    fill!(context_window_id);
    fill!(request_kind);
    fill!(forked_from_thread_id);
    fill!(forked_from_ordinal_exclusive);
    fill!(parent_thread_id);
    fill!(parent_turn_id);
    fill!(root_turn_id);
    fill!(subagent_header);
    fill!(subagent_kind);
    fill!(thread_source);
    fill!(turn_trigger);
    fill!(sandbox);
    fill!(sandbox_mode);
    fill!(auto_review_enabled);
    fill!(node_repl_auto_review_required);
    fill!(node_repl_disabled);
    fill!(turn_started_at_unix_ms);
    fill!(history_ingest_requested);
    fill!(compaction);
    fill!(originator);
    fill!(beta_features);
    fill!(memgen_request);
    fill!(responses_lite);
    fill!(ws_stream_request_start_ms);
    if base.workspaces.is_empty() {
        base.workspaces = overlay.workspaces.clone();
    }
    if base.source_workspace_paths.is_empty() {
        base.source_workspace_paths = overlay.source_workspace_paths.clone();
    }
    if base.tool_namespaces_info.is_empty() {
        base.tool_namespaces_info = overlay.tool_namespaces_info.clone();
    }
    if base.extras.is_empty() {
        base.extras = overlay.extras.clone();
    }
    if inherits_parent {
        base.parent_thread_id_inferred = overlay.parent_thread_id_inferred;
    }
    base.malformed |= overlay.malformed;
    base.truncated |= overlay.truncated;
    base.has_conflicts |= overlay.has_conflicts;
    base.is_subagent |= overlay.is_subagent;
    for conflict in &overlay.conflicts {
        if !base.conflicts.contains(conflict) {
            base.conflicts.push(conflict.clone());
        }
    }
    for source in &overlay.sources {
        if !base.sources.contains(source) {
            base.sources.push(source.clone());
        }
    }
    for field in &overlay.redacted_fields {
        if !base.redacted_fields.contains(field) {
            base.redacted_fields.push(field.clone());
        }
    }
    Some(base)
}

pub(super) fn is_codex_originator(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.contains("codex desktop") || value.contains("codex_cli_rs") || value.contains("codex-tui")
}

/// 入站 OpenAI 兼容层的客户端出口形态。
#[derive(Clone)]
pub(super) struct ClientOut {
    /// Legacy bridge dialect marker. The public data plane always uses Raw,
    /// leaving protocol interpretation to the upstream Provider.
    pub(super) dialect: Option<ClientDialect>,
    pub(super) passthrough_kind: PassthroughKind,
    pub(super) stream: bool,
    /// Some = 出站原样打上游；None = legacy internal bridge path.
    pub(super) passthrough: Option<Bytes>,
    pub(super) content_type: Option<String>,
    pub(super) terminal_dialect: SseDialect,
    /// Capture only bounded JSON responses of Realtime secret/session routes
    /// so returned `ek_…` credentials can authorize following calls/upgrades.
    pub(super) realtime_client_secret: bool,
}

pub(super) struct ClientMeta {
    pub(super) client_kind: ClientKind,
    pub(super) source_format: ProviderProtocol,
    pub(super) target_format: Option<ProviderProtocol>,
    pub(super) route_mode: Option<RouteMode>,
    pub(super) client_model: String,
    pub(super) effective_model: String,
    pub(super) feature_rule_id: Option<String>,
    pub(super) purpose: RequestPurpose,
    /// 形似 CC 内部辅助请求但未命中任何指纹(词表 `unmatched_no_tools`)。
    pub(super) unmatched_no_tools: bool,
    /// 有界客户端会话标识，用于跨客户端的会话统计筛选。
    pub(super) session_id: Option<String>,
    /// 本请求的粘性归属键（affinity 哈希）。随事件落库，供运维面按项目
    /// 清除粘性归属；早期拒绝与 WebSocket 合成键保持 None。
    pub(super) sticky_key: Option<String>,
    pub(super) codex_metadata: Option<CodexMetadata>,
    /// 客户端用 `X-Sumpter-*` 声明的项目归因；可信度低于 codex_metadata。
    pub(super) client_declared: Option<ClientDeclaredMetadata>,
    pub(super) grok_metadata: Option<GrokMetadata>,
    /// Bounded inbound request identity retained for stream/WS completion,
    /// which may be polled after the original HTTP task-local scope ends.
    pub(super) request_context: Option<InboundRequestContext>,
}

pub(super) fn host_of(base_url: &str) -> Option<String> {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// 仅采纳明确的上游追踪头；不扫描任意响应头，避免把 Cookie/鉴权信息带入事件。
pub(super) fn upstream_request_id(headers: &[(String, String)]) -> Option<String> {
    const CANDIDATES: &[&str] = &[
        "x-request-id",
        "request-id",
        "x-amzn-requestid",
        "x-correlation-id",
        "cf-ray",
    ];
    CANDIDATES.iter().find_map(|name| {
        let value = header_value(headers, name)?.trim();
        if value.is_empty() || value.chars().any(char::is_control) {
            return None;
        }
        Some(value.chars().take(256).collect())
    })
}

pub(super) fn inbound_auth_ok(headers: &[(String, String)], token: &str) -> bool {
    for (name, value) in headers {
        if (name.eq_ignore_ascii_case("x-api-key") || name.eq_ignore_ascii_case("x-goog-api-key"))
            && value == token
        {
            return true;
        }
        if name.eq_ignore_ascii_case("authorization") {
            let value = value.trim();
            if let Some(prefix) = value.get(..7)
                && prefix.eq_ignore_ascii_case("bearer ")
                && value[7..].trim() == token
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(header, value)| header.eq_ignore_ascii_case(name) && !value.trim().is_empty())
        .map(|(_, value)| value.as_str())
}

pub(super) fn detect_client_kind(headers: &[(String, String)], openai_inbound: bool) -> ClientKind {
    if header_value(headers, "x-sumpter-client")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("pi"))
    {
        return ClientKind::Pi;
    }
    // `Originator` is not consistently emitted as a standalone header by
    // Codex Desktop.  The same value commonly lives in the bounded canonical
    // `x-codex-turn-metadata` header, so use the parser as a transport-level
    // fallback before applying the dialect default.  This helper is used by
    // early rejection and WebSocket paths where the request body is not yet
    // available; body-aware handlers merge their parsed metadata below.
    let canonical_originator = CodexMetadata::from_request(headers, None)
        .and_then(|metadata| metadata.originator)
        .or_else(|| header_value(headers, "originator").map(str::to_string));
    ClientKind::detect_with_originator(
        header_value(headers, "user-agent"),
        canonical_originator.as_deref(),
        openai_inbound,
    )
}

pub(super) fn bounded_request_method(method: &str) -> String {
    let value = method.trim();
    let value = if value.is_empty() { "UNKNOWN" } else { value };
    value
        .chars()
        .take(16)
        .collect::<String>()
        .to_ascii_uppercase()
}

pub(super) fn bounded_request_path(path: &str) -> String {
    let value = if path.is_empty() { "/" } else { path };
    let mut end = value.len().min(1024);
    while !value.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }
    let value = &value[..end];
    if value.chars().any(char::is_control) {
        "/<invalid>".into()
    } else {
        value.to_string()
    }
}

pub(super) fn bounded_failure_detail(detail: &str) -> String {
    let mut out = detail
        .chars()
        .filter(|character| !character.is_control() || *character == '\t')
        .take(2048)
        .collect::<String>();
    if out.is_empty() {
        out.push_str("request rejected");
    }
    out
}

pub(super) fn current_request_context() -> Option<InboundRequestContext> {
    INBOUND_REQUEST_CONTEXT
        .try_with(|context| context.borrow().clone())
        .ok()
}

pub(super) fn set_current_route_intent(intent: &str) {
    let _ = INBOUND_REQUEST_CONTEXT.try_with(|context| {
        context.borrow_mut().route_intent = intent.to_string();
    });
}

pub(super) fn apply_current_request_context(event: &mut RuntimeEvent) {
    if !matches!(event.kind.as_str(), KIND_CLIENT | KIND_UPSTREAM) {
        return;
    }
    let Some(context) = current_request_context() else {
        return;
    };
    event.request_method.get_or_insert(context.method);
    event.request_path.get_or_insert(context.path);
    event.route_intent.get_or_insert(context.route_intent);
    if event.session_id.is_none() {
        event.session_id = context.session_id;
    }
    if event.grok_metadata.is_none() {
        event.grok_metadata = context.grok_metadata;
    }
}

/// Extract a stable client session identifier for analytics. Values are
/// bounded and rejected when they contain controls; request bodies are never
/// used as a fallback here.
pub(super) fn retain_codex_metadata_for_client(
    client_kind: ClientKind,
    metadata: Option<CodexMetadata>,
) -> Option<CodexMetadata> {
    if client_kind == ClientKind::Pi {
        return None;
    }
    let metadata = metadata?;
    if client_kind == ClientKind::GrokBuild && !metadata.has_request_identity() {
        None
    } else {
        Some(metadata)
    }
}

pub(super) fn observed_session_id(headers: &[(String, String)]) -> Option<String> {
    [
        "x-sumpter-session-id",
        "x-claude-code-session-id",
        "x-grok-session-id",
        "x-grok-conv-id",
        "session_id",
        "session-id",
        "x-session-id",
        "x-session-affinity",
    ]
    .iter()
    .find_map(|name| {
        let value = header_value(headers, name)?.trim();
        if value.is_empty() || value.chars().any(char::is_control) {
            return None;
        }
        let mut end = value.len().min(256);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        Some(value[..end].to_string())
    })
}

pub(super) fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}
