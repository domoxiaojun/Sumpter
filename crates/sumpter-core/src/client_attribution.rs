//! Inbound-only observations. Never use these results to build or route requests.
//! Rules are versioned at ingestion; persisted events are never reclassified.
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, RuntimeEvent, codex_agent_role,
    codex_originator_variant,
};
use crate::routing::{RequestPurpose, RoutingRequest, inspector};

pub use crate::client_registry::CLIENTS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedRequestKind {
    Turn,
    Prewarm,
    Compaction,
    SessionTitle,
    Memory,
    Review,
    #[serde(rename = "websearch")]
    WebSearch,
    #[serde(rename = "webfetch")]
    WebFetch,
    Classifier,
    TokenCount,
    ImageGeneration,
    ImageEdit,
    AlphaSearch,
    Realtime,
    Unknown,
}

impl ObservedRequestKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Prewarm => "prewarm",
            Self::Compaction => "compaction",
            Self::SessionTitle => "session_title",
            Self::Memory => "memory",
            Self::Review => "review",
            Self::WebSearch => "websearch",
            Self::WebFetch => "webfetch",
            Self::Classifier => "classifier",
            Self::TokenCount => "token_count",
            Self::ImageGeneration => "image_generation",
            Self::ImageEdit => "image_edit",
            Self::AlphaSearch => "alpha_search",
            Self::Realtime => "realtime",
            Self::Unknown => "unknown",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        serde_json::from_value(Value::String(value.to_ascii_lowercase())).ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttributionEvidence {
    pub field: String,
    pub source: String,
    pub rule: String,
    /// Only bounded enum values, never prompts, credentials or raw headers.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientAttribution {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::client_identity::ObservedIdentity>,
    pub version: u8,
    pub evidence: Vec<AttributionEvidence>,
    pub conflicts: Vec<String>,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AttributedRequest {
    pub client_kind: ClientKind,
    pub client_variant: String,
    pub agent_role: String,
    pub agent_name: Option<String>,
    pub request_kind: ObservedRequestKind,
    pub attribution: ClientAttribution,
}

impl AttributedRequest {
    pub fn apply(&self, event: &mut RuntimeEvent) {
        event.client_kind = Some(self.client_kind);
        event.client_variant = Some(self.client_variant.clone());
        event.agent_role = Some(self.agent_role.clone());
        event.agent_name = self.agent_name.clone();
        event.request_kind = Some(self.request_kind);
        event.client_attribution = Some(self.attribution.clone());
    }
}

pub fn client(value: &str) -> Option<ClientKind> {
    CLIENTS
        .iter()
        .copied()
        .find(|kind| kind.as_str().eq_ignore_ascii_case(value))
}

pub fn variant(value: &str) -> Option<&'static str> {
    crate::client_registry::VARIANTS
        .iter()
        .copied()
        .find(|v| v.eq_ignore_ascii_case(value))
}

pub fn role(value: &str) -> Option<&'static str> {
    [
        "root",
        "subagent",
        "guardian",
        "review",
        "memory",
        "title",
        "automation",
        "system",
        "ambient",
        "unknown",
    ]
    .into_iter()
    .find(|v| v.eq_ignore_ascii_case(value))
}

#[derive(Default)]
struct Signals(Vec<(u8, AttributionEvidence)>);
impl Signals {
    fn add(&mut self, priority: u8, field: &str, source: &str, rule: &str, value: &str) {
        self.0.push((
            priority,
            AttributionEvidence {
                field: field.into(),
                source: source.into(),
                rule: rule.into(),
                value: value.into(),
            },
        ));
    }
    fn resolve(&self, field: &str, fallback: &str, report: &mut ClientAttribution) -> String {
        let signals: Vec<_> = self.0.iter().filter(|(_, e)| e.field == field).collect();
        let Some(priority) = signals.iter().map(|(p, _)| *p).max() else {
            return fallback.into();
        };
        let preferred: Vec<_> = signals.iter().filter(|(p, _)| *p == priority).collect();
        let value = &preferred[0].1.value;
        let ambiguous = preferred.iter().any(|(_, e)| e.value != *value);
        if ambiguous || signals.iter().any(|(_, e)| e.value != *value) {
            report.conflicts.push(field.into());
        }
        for (_, evidence) in signals {
            if !report.evidence.contains(evidence) {
                report.evidence.push(evidence.clone());
            }
        }
        if ambiguous {
            "unknown".into()
        } else {
            value.clone()
        }
    }
}

/// Reject an entire repeated field on malformed/oversized input, rather than
/// selecting an arbitrary first/last header. Different valid values conflict.
fn headers<'a>(
    all: &'a [(String, String)],
    name: &str,
    report: &mut ClientAttribution,
) -> Vec<&'a str> {
    let values: Vec<_> = all
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
        .collect();
    if values.len() > 8
        || values
            .iter()
            .any(|v| v.len() > 4096 || v.chars().any(char::is_control))
    {
        if !report.issues.iter().any(|v| v == name) {
            report.issues.push(name.into());
        }
        return vec![];
    }
    values
        .into_iter()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .collect()
}

/// Native product tokens must occur at UA product boundaries, not in comments
/// or unrelated names. Keep the legacy detector for routing compatibility.
fn ua_client(ua: &str) -> Option<ClientKind> {
    if let Some(kind) = crate::client_registry::product(ua)
        .as_deref()
        .and_then(crate::client_registry::kind_for_product)
    {
        if !matches!(kind, ClientKind::Pi | ClientKind::ClaudeCode) {
            return Some(kind);
        }
    }
    let value = ua.to_ascii_lowercase();
    let first = value.split_whitespace().next()?;
    if first.starts_with("grok-shell/")
        || first.starts_with("grok-pager/")
        || first.starts_with("grok-desktop/")
    {
        return Some(ClientKind::GrokBuild);
    }
    if first.starts_with("cursor-cli/") {
        return Some(ClientKind::Cursor);
    }
    let kind = ClientKind::detect(Some(ua), false);
    if kind == ClientKind::GrokBuild {
        return None;
    }
    // Legacy dash aliases are deliberately not wildcard product families.
    if matches!(
        kind,
        ClientKind::Cline | ClientKind::Continue | ClientKind::OpenCode
    ) && first.split_once('/').is_none_or(|(p, _)| {
        !matches!(
            p,
            "cline" | "cline-cli" | "continue" | "continue-cli" | "opencode" | "opencode-cli"
        )
    }) {
        return None;
    }
    if kind == ClientKind::Codex
        && !first.split_once('/').is_some_and(|(p, _)| {
            matches!(
                p,
                "codex"
                    | "codex_cli_rs"
                    | "codex-cli"
                    | "codex-tui"
                    | "codex_tui"
                    | "codex-exec"
                    | "codex_exec"
                    | "codex-vscode"
                    | "codex_vscode"
            )
        })
        && !value.starts_with("codex desktop/")
    {
        return None;
    }
    (kind != ClientKind::Unknown).then_some(kind)
}

fn ua_variant(kind: ClientKind, ua: &str) -> Option<&'static str> {
    let lower = ua.to_ascii_lowercase();
    match kind {
        ClientKind::Codex => {
            if lower.starts_with("codex desktop/")
                || lower.starts_with("codex/") && lower.contains(" desktop")
            {
                Some("desktop")
            } else if lower.starts_with("codex-tui/") || lower.starts_with("codex_tui/") {
                Some("tui")
            } else if lower.starts_with("codex_cli_rs/") || lower.starts_with("codex-cli/") {
                Some("cli")
            } else if lower.starts_with("codex-exec/") || lower.starts_with("codex_exec/") {
                Some("exec")
            } else if lower.starts_with("codex-vscode/") || lower.starts_with("codex_vscode/") {
                Some("vscode")
            } else {
                None
            }
        }
        ClientKind::ClaudeCode => {
            let entry = lower
                .split_once("(external,")?
                .1
                .split([')', ','])
                .next()?
                .trim();
            match entry {
                "cli" => Some("cli"),
                "claude-vscode" => Some("vscode"),
                "sdk-cli" | "sdk-ts" | "sdk-py" => Some("sdk"),
                "claude-code-github-action" => Some("backend"),
                _ => None,
            }
        }
        ClientKind::GeminiCli
            if lower.starts_with("cloudcodevscode/")
                && lower.contains("proxy_client=geminicli") =>
        {
            Some("vscode")
        }
        ClientKind::GeminiCli if lower.starts_with("geminicli/") => Some("cli"),
        ClientKind::GrokBuild if lower.starts_with("grok-desktop/") => Some("desktop"),
        ClientKind::GrokBuild
            if lower.starts_with("grok-shell/") || lower.starts_with("grok-pager/") =>
        {
            Some("cli")
        }
        ClientKind::Cursor
            if lower.starts_with("cursor-agent/") || lower.starts_with("cursor-cli/") =>
        {
            Some("cli")
        }
        ClientKind::Aider => Some("cli"),
        ClientKind::OpenWebUi => Some("web"),
        ClientKind::LiteLlm => Some("backend"),
        _ => None,
    }
}

/// A protocol fallback is not a product identity. Only known OpenAI API paths
/// justify `openai_compat`; Gemini, Anthropic and opaque paths stay unknown.
pub fn uses_openai_client_fallback(path: &str) -> bool {
    let path = path.split('?').next().unwrap_or(path);
    let path = path
        .strip_prefix("/v1")
        .filter(|rest| rest.starts_with('/'))
        .unwrap_or(path);
    path == "/models"
        || [
            "/chat/completions",
            "/responses",
            "/backend-api/codex/responses",
            "/completions",
            "/images",
            "/files",
            "/videos",
            "/realtime",
        ]
        .iter()
        .any(|prefix| {
            path == *prefix
                || path
                    .strip_prefix(*prefix)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
}

pub fn observe(
    method: &str,
    path: &str,
    all: &[(String, String)],
    body: Option<&Value>,
) -> AttributedRequest {
    let mut report = ClientAttribution {
        version: 2,
        identity: None,
        evidence: vec![],
        conflicts: vec![],
        issues: vec![],
    };
    let mut signals = Signals::default();
    let uas = headers(all, "user-agent", &mut report);
    for value in headers(all, "x-sumpter-client", &mut report) {
        if let Some(kind) = client(value) {
            signals.add(
                4,
                "clientKind",
                "declared",
                "x-sumpter-client",
                kind.as_str(),
            );
        }
    }
    for ua in &uas {
        if let Some(kind) = ua_client(ua) {
            signals.add(
                2,
                "clientKind",
                "user_agent",
                "product-token",
                kind.as_str(),
            );
        }
    }
    let codex = CodexMetadata::from_request(all, body);
    let duplicate_native = duplicate_native_metadata(all);
    let body_codex = (!duplicate_native.is_empty())
        .then(|| CodexMetadata::from_request(&[], body))
        .flatten();
    if let Some(metadata) = &codex {
        if metadata.malformed || metadata.truncated {
            report.issues.push("codexMetadata".into());
        }
        if metadata.has_conflicts {
            report.conflicts.push("codexMetadata".into());
        }
    }
    if let Some(originator) = codex.as_ref().and_then(|m| m.originator.as_deref()) {
        if codex_originator_variant(originator).is_some() {
            signals.add(3, "clientKind", "native", "codex-originator", "codex");
        } else if originator.eq_ignore_ascii_case("pi") {
            signals.add(3, "clientKind", "native", "pi-originator", "pi");
        }
    }
    if body_codex
        .as_ref()
        .and_then(|m| m.originator.as_ref())
        .is_none()
    {
        for metadata in &duplicate_native {
            if let Some(originator) = metadata.originator.as_deref() {
                if codex_originator_variant(originator).is_some() {
                    signals.add(3, "clientKind", "native", "duplicate-originator", "codex");
                } else if originator.eq_ignore_ascii_case("pi") {
                    signals.add(3, "clientKind", "native", "duplicate-originator", "pi");
                }
            }
        }
    }
    for value in headers(all, "x-grok-client-identifier", &mut report) {
        if matches!(
            value.to_ascii_lowercase().as_str(),
            "grok-shell" | "grok-pager" | "grok-desktop"
        ) {
            signals.add(
                3,
                "clientKind",
                "native",
                "grok-client-identifier",
                "grok_build",
            );
        }
    }
    let fallback = if uses_openai_client_fallback(path) {
        "openai_compat"
    } else {
        "unknown"
    };
    let ua = unique_label(all, "user-agent", 4096)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let codex_headers = unique_label(all, "x-codex-window-id", 128).is_some()
        || codex
            .as_ref()
            .is_some_and(|m| !m.malformed && !m.truncated && m.has_request_identity());
    let hosted =
        ua.starts_with("vscode_agent_host/") || (ua.starts_with("claude code/") && codex_headers);
    if hosted {
        signals.add(3, "clientKind", "native", "hosted-codex", "codex");
    }
    let selected = signals.resolve("clientKind", fallback, &mut report);
    let kind = client(&selected).unwrap_or(if selected == "openai_compat" {
        ClientKind::OpenaiCompat
    } else {
        ClientKind::Unknown
    });
    for ua in &uas {
        if ua_client(ua) == Some(kind)
            && let Some(value) = ua_variant(kind, ua)
        {
            signals.add(2, "clientVariant", "user_agent", "entrypoint", value);
        }
    }
    for value in headers(all, "x-sumpter-client-variant", &mut report) {
        if let Some(value) = variant(value) {
            signals.add(
                4,
                "clientVariant",
                "declared",
                "x-sumpter-client-variant",
                value,
            );
        }
    }
    for value in headers(all, "x-sumpter-agent-role", &mut report) {
        if let Some(value) = role(value) {
            signals.add(2, "agentRole", "declared", "x-sumpter-agent-role", value);
        }
    }
    for value in headers(all, "x-sumpter-request-kind", &mut report) {
        if let Some(value) = ObservedRequestKind::parse(value) {
            signals.add(
                2,
                "requestKind",
                "declared",
                "x-sumpter-request-kind",
                value.as_str(),
            );
        }
    }
    let mut agent_name = ClientDeclaredMetadata::from_headers(all).and_then(|m| m.agent_name);
    if kind == ClientKind::Codex
        && let Some(metadata) = &codex
    {
        if let Some(value) = metadata
            .originator
            .as_deref()
            .and_then(codex_originator_variant)
        {
            signals.add(3, "clientVariant", "native", "codex-originator", value);
        }
        let value = codex_agent_role(metadata);
        if value != "unknown" {
            signals.add(3, "agentRole", "native", "codex-role", value);
        }
        if let Some(value) = metadata
            .request_kind
            .as_deref()
            .and_then(ObservedRequestKind::parse)
        {
            signals.add(
                3,
                "requestKind",
                "native",
                "codex-request-kind",
                value.as_str(),
            );
        }
        agent_name = metadata.agent_name.clone().or(agent_name);
        for alternative in &duplicate_native {
            if body_codex
                .as_ref()
                .and_then(|m| m.originator.as_ref())
                .is_none()
                && let Some(value) = alternative
                    .originator
                    .as_deref()
                    .and_then(codex_originator_variant)
            {
                signals.add(3, "clientVariant", "native", "duplicate-originator", value);
            }
            if body_codex
                .as_ref()
                .map(codex_agent_role)
                .unwrap_or("unknown")
                == "unknown"
            {
                let role = codex_agent_role(alternative);
                if role != "unknown" {
                    signals.add(3, "agentRole", "native", "duplicate-codex-role", role);
                }
            }
            if body_codex
                .as_ref()
                .and_then(|m| m.request_kind.as_ref())
                .is_none()
                && let Some(value) = alternative
                    .request_kind
                    .as_deref()
                    .and_then(ObservedRequestKind::parse)
            {
                signals.add(
                    3,
                    "requestKind",
                    "native",
                    "duplicate-codex-request-kind",
                    value.as_str(),
                );
            }
        }
    }
    if kind == ClientKind::DeepSeekHarness {
        for value in headers(all, "x-deepseek-harness-compact", &mut report) {
            if value == "1" {
                signals.add(
                    3,
                    "requestKind",
                    "native",
                    "deepseek-compact-v1",
                    "compaction",
                );
            }
        }
        if body
            .and_then(|b| b.pointer("/dsh_session_log/version"))
            .and_then(Value::as_u64)
            == Some(1)
            && body
                .and_then(|b| b.pointer("/dsh_session_log/session/origin"))
                .and_then(Value::as_str)
                == Some("subagent")
        {
            signals.add(
                3,
                "agentRole",
                "native",
                "deepseek-session-origin-v1",
                "subagent",
            );
        }
    }
    if kind == ClientKind::ClaudeCode {
        let ids = headers(all, "x-claude-code-agent-id", &mut report);
        if ids.len() == 1 && !ids[0].is_empty() && ids[0].len() <= 128 {
            signals.add(3, "agentRole", "native", "claude-agent-id", "subagent");
        }
    }
    if let Some(metadata) = &codex {
        for raw in [
            metadata.request_kind.as_deref(),
            metadata.thread_source.as_deref(),
            metadata.subagent_header.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            if kind == ClientKind::Codex
                && metadata
                    .request_kind
                    .as_deref()
                    .is_none_or(|v| ObservedRequestKind::parse(v).is_none())
                && let Some(task) = native_task(raw)
            {
                signals.add(3, "requestKind", "native", "codex-task-alias", task);
            }
        }
    }
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    if method.eq_ignore_ascii_case("POST") || method.eq_ignore_ascii_case("GET") {
        let endpoint_kind = match path {
            "/v1/responses/compact" | "/responses/compact" => Some("compaction"),
            "/v1/messages/count_tokens" => Some("token_count"),
            "/v1/images/generations" => Some("image_generation"),
            "/v1/images/edits" => Some("image_edit"),
            "/v1/alpha/search" | "/alpha/search" | "/backend-api/codex/alpha/search" => {
                Some("alpha_search")
            }
            "/v1/realtime"
            | "/v1/realtime/sessions"
            | "/v1/realtime/client_secrets"
            | "/v1/realtime/calls" => Some("realtime"),
            _ if path.ends_with(":countTokens") => Some("token_count"),
            _ if path.ends_with("BidiGenerateContent") => Some("realtime"),
            _ => None,
        };
        if let Some(value) = endpoint_kind {
            signals.add(3, "requestKind", "endpoint", "dedicated-operation", value);
        }
    }
    if let Some(body) = body {
        fingerprints(kind, body, &mut signals);
    }
    let client_variant = signals.resolve("clientVariant", "unknown", &mut report);
    let agent_role = signals.resolve("agentRole", "unknown", &mut report);
    let request_kind =
        ObservedRequestKind::parse(&signals.resolve("requestKind", "unknown", &mut report))
            .unwrap_or(ObservedRequestKind::Unknown);
    let ua = unique_label(all, "user-agent", 4096).unwrap_or_default();
    let mut identity = crate::client_identity::ObservedIdentity {
        product: crate::client_registry::product(&ua),
        ..Default::default()
    };
    if kind == ClientKind::ClaudeCode {
        identity.agent_id = unique_label(all, "x-claude-code-agent-id", 128);
        identity.parent_agent_id = identity
            .agent_id
            .as_ref()
            .and_then(|_| unique_label(all, "x-claude-code-parent-agent-id", 128));
    }
    if identity.product.as_deref() == Some("vscode_agent_host")
        || (ua.to_ascii_lowercase().starts_with("claude code/") && codex_headers)
    {
        identity.host = Some(
            if identity.product.as_deref() == Some("vscode_agent_host") {
                "vscode"
            } else {
                "claude_code"
            }
            .into(),
        );
        identity.engine = Some("codex".into());
    } else if kind == ClientKind::Codex {
        identity.engine = Some("codex".into());
    }
    report.identity = Some(identity);
    AttributedRequest {
        client_kind: kind,
        client_variant,
        agent_role,
        agent_name,
        request_kind,
        attribution: report,
    }
}

// Equal-priority repeated native headers must not silently choose the first
// task. Body metadata keeps the established canonical-source precedence.
fn duplicate_native_metadata(all: &[(String, String)]) -> Vec<CodexMetadata> {
    let mut result = Vec::new();
    for name in ["originator", "x-codex-turn-metadata", "x-openai-subagent"] {
        let values: Vec<_> = all
            .iter()
            .filter(|(n, v)| {
                n.eq_ignore_ascii_case(name)
                    && v.len() <= 65_536
                    && !v.chars().any(char::is_control)
            })
            .map(|(_, v)| v)
            .take(8)
            .collect();
        if values.len() > 1 && values.iter().any(|v| v.trim() != values[0].trim()) {
            for value in values {
                if let Some(metadata) =
                    CodexMetadata::from_request(&[(name.into(), value.clone())], None)
                {
                    result.push(metadata);
                }
            }
        }
    }
    result
}

/// Only direct text blocks. Never descend into a tool result, document or history.
fn text(value: &Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return (text.len() <= 16_384).then(|| text.into());
    }
    let parts = value.as_array()?;
    if parts.len() > 16 {
        return None;
    }
    let mut out = String::new();
    for part in parts {
        if part
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|t| !matches!(t, "text" | "input_text"))
        {
            return None;
        }
        let part = part.get("text")?.as_str()?;
        if out.len() + part.len() + 1 > 16_384 {
            return None;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(part);
    }
    Some(out)
}

fn fingerprints(kind: ClientKind, body: &Value, signals: &mut Signals) {
    let messages = body
        .get("messages")
        .or_else(|| body.get("contents"))
        .or_else(|| body.get("input"))
        .and_then(Value::as_array);
    let mut system = body
        .get("system")
        .or_else(|| body.get("instructions"))
        .and_then(text)
        .or_else(|| body.pointer("/systemInstruction/parts").and_then(text))
        .unwrap_or_default();
    let Some(messages) = messages else { return };
    if system.is_empty() {
        for message in messages.iter().take(4) {
            if !matches!(
                message.get("role").and_then(Value::as_str),
                Some("system" | "developer")
            ) {
                break;
            }
            let Some(value) = message.get("content").and_then(text) else {
                return;
            };
            if system.len() + value.len() > 16_384 {
                return;
            }
            system.push_str(&value);
            system.push('\n');
        }
    }
    let Some(last) = messages
        .last()
        .filter(|m| m.get("role").and_then(Value::as_str) == Some("user"))
    else {
        return;
    };
    let Some(raw_ask) = last.get("content").or_else(|| last.get("parts")) else {
        return;
    };
    let Some(ask_prefix) = text_prefix(raw_ask) else {
        return;
    };
    let tools = body.get("tools").and_then(Value::as_array);
    let no_tools = body
        .get("tools")
        .is_none_or(|v| v.as_array().is_some_and(Vec::is_empty));
    let system = system.trim();
    let ask_prefix = ask_prefix.trim();
    let instructions = system
        .lines()
        .skip_while(|line| {
            line.trim().is_empty()
                || line.starts_with("x-anthropic-billing-header:")
                || *line == "You are Claude Code, Anthropic's official CLI for Claude."
        })
        .collect::<Vec<_>>()
        .join("\n");
    let leading = instructions.trim();
    let compact = no_tools && match kind {
        ClientKind::Pi => system.starts_with("You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified."),
        ClientKind::GeminiCli => system.starts_with("You are a specialized system component responsible for distilling chat history into a structured XML <state_snapshot>"),
        ClientKind::ClaudeCode => leading.starts_with("You are a helpful AI assistant tasked with summarizing conversations.") && ask_prefix.starts_with("Your task is to create a detailed summary of the conversation so far"),
        ClientKind::Codex => ask_prefix.starts_with("You are performing a CONTEXT CHECKPOINT COMPACTION.") && messages.len() == 1,
        _ => false,
    };
    if compact {
        signals.add(
            1,
            "requestKind",
            "fingerprint",
            "compaction-prompt-v1",
            "compaction",
        );
    }
    // Reuse the existing CC fingerprints on a small observation-only view.
    // One direct user message prevents a historical classifier/title match.
    if kind != ClientKind::ClaudeCode || messages.len() != 1 || tools.is_some_and(|t| t.len() > 1) {
        return;
    }
    let Some(ask) = text(raw_ask) else { return };
    let ask = ask.trim();
    let mut view = Map::new();
    view.insert("system".into(), Value::String(system.into()));
    view.insert(
        "messages".into(),
        serde_json::json!([{"role":"user","content":ask}]),
    );
    for key in ["tools", "tool_choice", "stop_sequences", "output_config"] {
        if let Some(value) = body.get(key) {
            let mut budget = 4096;
            if !bounded_json(value, 0, &mut budget) {
                return;
            }
            view.insert(key.into(), value.clone());
        }
    }
    if let Some(request) = RoutingRequest::from_value_owned(Value::Object(view)) {
        let purpose = inspector::request_purpose(&request);
        let observed = match purpose {
            RequestPurpose::SessionTitle => Some("session_title"),
            RequestPurpose::WebSearch => Some("websearch"),
            RequestPurpose::WebFetch => Some("webfetch"),
            RequestPurpose::Classifier => Some("classifier"),
            _ => None,
        };
        // Only the known leading instruction (after CC's bounded preamble),
        // never an arbitrary quote buried in another system instruction.
        let anchored = match purpose {
            RequestPurpose::SessionTitle => {
                leading.starts_with("Write the title in ")
                    || body
                        .pointer("/output_config/format/schema/properties/title/type")
                        .and_then(Value::as_str)
                        == Some("string")
            }
            RequestPurpose::WebSearch => {
                leading.starts_with("You are an assistant for performing a web search tool use")
            }
            RequestPurpose::Classifier => {
                leading.starts_with("You are a security monitor for autonomous AI coding agents.")
            }
            RequestPurpose::WebFetch => {
                system.starts_with("You are Claude Code, Anthropic's official CLI for Claude.")
                    || system.starts_with("x-anthropic-billing-header:")
            }
            _ => false,
        };
        if anchored && let Some(value) = observed {
            signals.add(
                1,
                "requestKind",
                "fingerprint",
                "claude-auxiliary-v1",
                value,
            );
        }
    }
}

// Compaction carries a large conversation. Inspect its direct opening only;
// never allocate or scan the conversation text to identify it.
fn text_prefix(value: &Value) -> Option<String> {
    let raw = if let Some(raw) = value.as_str() {
        raw
    } else {
        let parts = value.as_array()?;
        if parts.is_empty()
            || parts.len() > 16
            || parts.iter().any(|p| {
                p.get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|t| !matches!(t, "text" | "input_text"))
                    || p.get("text").and_then(Value::as_str).is_none()
            })
        {
            return None;
        }
        parts[0].get("text")?.as_str()?
    };
    Some(raw.chars().take(1024).collect())
}

fn bounded_json(value: &Value, depth: usize, budget: &mut usize) -> bool {
    if depth > 10 || *budget == 0 {
        return false;
    }
    *budget -= 1;
    match value {
        Value::String(s) => {
            if s.len() > *budget {
                false
            } else {
                *budget -= s.len();
                true
            }
        }
        Value::Array(a) => a.iter().all(|v| bounded_json(v, depth + 1, budget)),
        Value::Object(o) => o.iter().all(|(k, v)| {
            if k.len() > *budget {
                false
            } else {
                *budget -= k.len();
                bounded_json(v, depth + 1, budget)
            }
        }),
        _ => true,
    }
}

fn unique_label(all: &[(String, String)], name: &str, max: usize) -> Option<String> {
    let values: Vec<_> = all
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
        .collect();
    if values.len() != 1 {
        return None;
    }
    let v = values[0];
    (!v.is_empty() && v.len() <= max && !v.chars().any(char::is_control)).then(|| v.to_owned())
}
pub(crate) fn native_task(value: &str) -> Option<&'static str> {
    Some(match value {
        "compact" | "compaction" => "compaction",
        "title" | "thread_title" | "thread_title_reconsideration" | "title_generation" => {
            "session_title"
        }
        "memory" | "memgen" | "memory_consolidation" => "memory",
        "review" | "automated_review" | "auto_review" | "guardian" | "guardian_review" => "review",
        "web_search" => "websearch",
        _ => return None,
    })
}
/// The caller supplies the identity of a successfully matched enabled token.
/// Neither token strings nor unauthenticated relay headers grant attribution.
pub fn observe_configured(
    method: &str,
    path: &str,
    all: &[(String, String)],
    body: Option<&Value>,
    settings: &crate::client_identity::ClientIdentitySettings,
    token: Option<&crate::config::InboundTokenIdentity>,
) -> AttributedRequest {
    let mut result = observe(method, path, all, body);
    let product = result
        .attribution
        .identity
        .as_ref()
        .and_then(|i| i.product.as_deref());
    let alias = product.and_then(|p| settings.ua_aliases.get(p));
    let binding = token.and_then(|t| settings.token_bindings.get(&t.id));
    let candidate = binding
        .map(|b| b.client.as_str())
        .or(alias.map(String::as_str));
    if let Some(candidate) = candidate {
        let can_fill = matches!(
            result.client_kind,
            ClientKind::Unknown | ClientKind::OpenaiCompat
        ) && !result
            .attribution
            .conflicts
            .iter()
            .any(|v| v == "clientKind");
        if can_fill {
            if let Some(kind) = client(candidate) {
                fill_known_client(&mut result, kind, method, path, all, body);
            }
            let identity = result
                .attribution
                .identity
                .get_or_insert_with(Default::default);
            if binding.is_some() {
                identity.raw_product = identity.product.clone();
                identity.product = Some(candidate.into());
            }
        } else if client(candidate) != Some(result.client_kind) {
            result.attribution.conflicts.push("clientKind".into());
        }
        if let Some(b) = binding {
            if (can_fill || client(&b.client) == Some(result.client_kind))
                && result.client_variant == "unknown"
                && !result
                    .attribution
                    .conflicts
                    .iter()
                    .any(|v| v == "clientVariant")
            {
                if let Some(v) = &b.variant {
                    result.client_variant = v.clone();
                }
            }
        }
        result.attribution.evidence.push(AttributionEvidence {
            field: "clientKind".into(),
            source: if binding.is_some() {
                "token_binding"
            } else {
                "configured_ua"
            }
            .into(),
            rule: "server-config".into(),
            value: candidate.into(),
        });
    }
    let has_forwarded = all.iter().any(|(n, _)| {
        n.to_ascii_lowercase().starts_with("x-sumpter-forwarded-")
            || n.eq_ignore_ascii_case("x-sumpter-relay-source")
    });
    if has_forwarded {
        let source = binding.and_then(|b| b.relay_source.as_deref());
        let declared_source = unique_label(all, "x-sumpter-relay-source", 64);
        let forwarded = unique_label(all, "x-sumpter-forwarded-client", 64)
            .filter(|v| crate::client_registry::valid_product(v));
        if let (Some(source), Some(product)) = (source, forwarded) {
            if declared_source.as_deref() == Some(source) {
                let can_fill = matches!(
                    result.client_kind,
                    ClientKind::Unknown | ClientKind::OpenaiCompat
                ) && !result
                    .attribution
                    .conflicts
                    .iter()
                    .any(|v| v == "clientKind");
                if can_fill {
                    if let Some(kind) = client(&product) {
                        fill_known_client(&mut result, kind, method, path, all, body);
                    }
                    let identity = result
                        .attribution
                        .identity
                        .get_or_insert_with(Default::default);
                    if identity.raw_product.is_none() {
                        identity.raw_product = identity.product.clone();
                    }
                    identity.product = Some(product.clone());
                } else if client(&product) != Some(result.client_kind) {
                    result.attribution.conflicts.push("clientKind".into());
                }
                let identity = result
                    .attribution
                    .identity
                    .get_or_insert_with(Default::default);
                identity.relay_source = Some(source.into());
                identity.session_id = unique_label(all, "x-sumpter-forwarded-session", 128)
                    .map(|id| format!("relay:{source}:{id}"));
                result.attribution.evidence.push(AttributionEvidence {
                    field: "clientKind".into(),
                    source: "trusted_relay".into(),
                    rule: "authenticated-relay-token".into(),
                    value: product,
                });
            } else {
                result.attribution.issues.push("relaySourceMismatch".into());
            }
        } else {
            result.attribution.issues.push("untrustedRelay".into());
        }
    }
    result.attribution.conflicts.sort();
    result.attribution.conflicts.dedup();
    result
}

/// Apply product-specific native rules only after a missing identity was filled.
fn fill_known_client(
    result: &mut AttributedRequest,
    kind: ClientKind,
    method: &str,
    path: &str,
    all: &[(String, String)],
    body: Option<&Value>,
) {
    result.client_kind = kind;
    let mut declared = all.to_vec();
    declared.retain(|(n, _)| !n.eq_ignore_ascii_case("x-sumpter-client"));
    declared.push(("x-sumpter-client".into(), kind.as_str().into()));
    let recovered = observe(method, path, &declared, body);
    result.client_variant = recovered.client_variant;
    result.agent_role = recovered.agent_role;
    result.agent_name = recovered.agent_name;
    result.request_kind = recovered.request_kind;
    if let Some(identity) = result.attribution.identity.as_mut() {
        if let Some(native) = recovered.attribution.identity {
            identity.engine = native.engine;
            identity.agent_id = native.agent_id;
            identity.parent_agent_id = native.parent_agent_id;
        }
    }
    for e in recovered
        .attribution
        .evidence
        .into_iter()
        .filter(|e| e.field != "clientKind")
    {
        if !result.attribution.evidence.contains(&e) {
            result.attribution.evidence.push(e);
        }
    }
    result
        .attribution
        .conflicts
        .extend(recovered.attribution.conflicts);
}
