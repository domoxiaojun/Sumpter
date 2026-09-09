//! SQLite-backed runtime statistics.
//!
//! The proxy hot path only updates the in-memory snapshot and enqueues a bounded
//! write message.  The SQLite connection belongs to the worker thread; admin
//! reads use short-lived read connections and never touch the proxy state lock.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::database::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sumpter_core::cache_read::CacheReadSummary;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::ResponseUsage;
use sumpter_core::events::{
    APPLE_EPOCH_OFFSET_SECS, ClientDeclaredMetadata, ClientKind, CodexMetadata, GrokMetadata,
    KIND_CLIENT, KIND_NOTIFY, KIND_UPSTREAM, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase,
    RuntimeFailureKind, RuntimeFailurePhase, RuntimeSnapshot, STATUS_CLIENT_DISCONNECTED,
    StreamTrace, codex_attribution_scope, codex_thread_class, local_user_from_workspace_path,
};
use sumpter_core::routing::{RESOURCE_ROUTING_MODEL, RequestPurpose, RouteMode};

pub(crate) const SCHEMA_VERSION: i64 = 5;
pub(crate) const PROJECTION_VERSION: i64 = 10;
const PROJECTION_BACKFILL_BATCH: usize = 500;
const BATCH_EVENTS: usize = 64;
const BATCH_BYTES: usize = 256 * 1024;
const PENDING_BYTES_LIMIT: usize = 4 * 1024 * 1024;
const PENDING_EVENTS_LIMIT: usize = 4096;
const BACKPRESSURE_BYTES: usize = 3 * 1024 * 1024;
const BACKPRESSURE_EVENTS: usize = 3072;
const RECENT_CHANGES_LIMIT: usize = 600;
/// 空闲 worker 的保留策略检查间隔。写入/启动/策略变更仍会立即补偿检查；
/// 这里的周期检查让低流量实例也能在时间窗口到期后及时轮换。
const RETENTION_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(60);
const TTFB_SLOW_MS: i64 = 5_000;
const TTFB_CRITICAL_MS: i64 = 15_000;
const DURATION_SLOW_MS: i64 = 3_000;
const DURATION_CRITICAL_MS: i64 = 6_000;
const MAX_PROJECT_WORKSPACE_PATHS: usize = 20;
const RETRY_INITIAL: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(30);

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(test)]
fn event_now() -> f64 {
    now() - APPLE_EPOCH_OFFSET_SECS
}

fn option_token<T: serde::Serialize>(value: Option<T>) -> Option<String> {
    value.and_then(|value| {
        serde_json::to_value(value)
            .ok()?
            .as_str()
            .map(ToOwned::to_owned)
    })
}

/// Analytics 查询筛选。值使用事件中已经脱敏/稳定化的维度键。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyticsFilter {
    pub client_kind: Option<String>,
    pub client_variant: Option<String>,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
    pub parent_thread_id: Option<String>,
    pub parent_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
    /// Stable endpoint identity, kept separate from the display name.
    pub endpoint_id: Option<String>,
    /// Stable project identity, kept separate from the display name.
    pub project_id: Option<String>,
    pub project: Option<String>,
    pub session_id: Option<String>,
    /// Optional explicit time bounds used by HTTP callers for the local
    /// calendar-day `today` range.  Dimension filters remain independent.
    pub from: Option<f64>,
    pub to: Option<f64>,
}

impl AnalyticsFilter {
    /// Normalize values at the storage boundary as well as in the HTTP
    /// handler.  Callers other than Admin (tests, Swift bridge, and future
    /// integrations) must get the same matching and reporting semantics.
    pub fn normalized(&self) -> Self {
        fn value(value: &Option<String>) -> Option<String> {
            value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
        }
        Self {
            client_kind: value(&self.client_kind),
            client_variant: value(&self.client_variant),
            agent_role: value(&self.agent_role),
            agent_name: value(&self.agent_name),
            parent_thread_id: value(&self.parent_thread_id),
            parent_turn_id: value(&self.parent_turn_id),
            root_turn_id: value(&self.root_turn_id),
            endpoint_id: value(&self.endpoint_id),
            project_id: value(&self.project_id),
            project: value(&self.project),
            session_id: value(&self.session_id),
            from: self.from.filter(|value| value.is_finite()),
            to: self.to.filter(|value| value.is_finite()),
        }
    }
}

/// Query-facing scalar projection of the bounded, already-sanitized event.
/// `payload_json` remains the detail source of truth, while list/analytics
/// queries use these columns and never need to deserialize every retained row.
#[derive(Debug, Clone)]
struct EventProjection {
    payload_bytes: i64,
    session_key: String,
    session_source: &'static str,
    /// 会话粘性归属键(affinity 哈希)。来自事件的 `stickyKey`;早期拒绝与
    /// WebSocket 合成键的事件为 NULL。按项目清除粘性归属时按它聚合。
    sticky_key: Option<String>,
    project_id: String,
    project_name: String,
    project_source: &'static str,
    local_user: Option<String>,
    codex_thread_class: Option<&'static str>,
    attribution_scope: Option<&'static str>,
    workspace_paths_json: String,
    endpoint_name: Option<String>,
    model_group_id: Option<String>,
    model_group_name: Option<String>,
    feature_rule_id: Option<String>,
    client_model: Option<String>,
    effective_model: Option<String>,
    upstream_model: Option<String>,
    failure_phase: Option<String>,
    source_format: Option<String>,
    target_format: Option<String>,
    route_mode: Option<String>,
    upstream_status_code: Option<i64>,
    duration_ms: i64,
    ttfb_ms: Option<i64>,
    failover: i64,
    stream_terminal: Option<String>,
    codex_metadata_present: i64,
    usage_present: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_read_state: String,
    cache_read_finality: String,
    cache_read_reason: Option<String>,
    hook_event: Option<String>,
    cache_creation_input_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    uncached_input_tokens: Option<i64>,
    processed_input_tokens: Option<i64>,
    processed_total_tokens: Option<i64>,
    token_accounting_semantics: &'static str,
    token_accounting_quality: &'static str,
    tool_calls_json: Option<String>,
    request_method: Option<String>,
    request_path: Option<String>,
    route_intent: Option<String>,
    client_variant: Option<String>,
    agent_role: Option<String>,
    agent_name: Option<String>,
    parent_thread_id: Option<String>,
    parent_turn_id: Option<String>,
    root_turn_id: Option<String>,
}

impl EventProjection {
    fn from_event(event: &RuntimeEvent, payload_bytes: usize) -> Self {
        let usage = event
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.usage.as_ref());
        let cache_read = event.observed_cache_read();
        let input_tokens = usage.and_then(|usage| token_i64(usage.input_tokens));
        let output_tokens = usage.and_then(|usage| token_i64(usage.output_tokens));
        let cache_read_input_tokens =
            usage.and_then(|usage| token_i64(usage.cache_read_input_tokens));
        let cache_creation_input_tokens =
            usage.and_then(|usage| token_i64(usage.cache_creation_input_tokens));
        let reasoning_tokens = usage.and_then(|usage| token_i64(usage.reasoning_tokens));
        let (token_accounting_semantics, processed_input_tokens, uncached_input_tokens) =
            normalized_input_tokens(
                event.target_format.or(event.source_format),
                input_tokens,
                cache_read_input_tokens,
                cache_creation_input_tokens,
            );
        let processed_total_tokens =
            processed_input_tokens.map(|input| input.saturating_add(output_tokens.unwrap_or(0)));
        let token_accounting_quality = match usage {
            None => "unknown",
            Some(usage) if usage.input_tokens.is_some() && usage.output_tokens.is_some() => {
                "complete"
            }
            Some(_) => "partial",
        };
        let (project_id, project_name, project_source, workspace_paths_json) =
            event_project_projection(event);
        let is_codex_event = event.kind == KIND_CLIENT
            && (event.codex_metadata.is_some() || event.client_kind == Some(ClientKind::Codex));
        let (codex_thread_class, attribution_scope) = if is_codex_event {
            (
                Some(codex_thread_class(event.codex_metadata.as_ref()).as_str()),
                Some(event_attribution_scope(event).as_str()),
            )
        } else {
            (None, None)
        };
        let (session_key, session_source) = event_session_projection(event);
        Self {
            payload_bytes: payload_bytes.min(i64::MAX as usize) as i64,
            session_key,
            session_source,
            sticky_key: event
                .sticky_key
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(128).collect()),
            project_id,
            project_name,
            project_source,
            local_user: event_local_user(event),
            codex_thread_class,
            attribution_scope,
            workspace_paths_json,
            endpoint_name: event.endpoint_name.clone(),
            model_group_id: event.model_group_id.clone(),
            model_group_name: event.model_group_name.clone(),
            feature_rule_id: event.feature_rule_id.clone(),
            client_model: event.client_model.clone(),
            effective_model: event.effective_model.clone(),
            upstream_model: event.upstream_model.clone(),
            failure_phase: option_token(event.failure_phase),
            source_format: event.source_format.map(|value| value.token().to_owned()),
            target_format: event.target_format.map(|value| value.token().to_owned()),
            route_mode: option_token(event.route_mode),
            upstream_status_code: event.upstream_status_code,
            duration_ms: event.duration_ms,
            ttfb_ms: event.ttfb_ms,
            failover: i64::from(event.failover),
            stream_terminal: event
                .stream_trace
                .as_ref()
                .and_then(|trace| trace.terminal_event.clone()),
            codex_metadata_present: i64::from(event.codex_metadata.is_some()),
            usage_present: i64::from(usage.is_some()),
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            cache_read_state: option_token(Some(cache_read.state)).unwrap(),
            cache_read_finality: option_token(Some(cache_read.finality)).unwrap(),
            cache_read_reason: option_token(cache_read.reason),
            hook_event: event.hook_event.clone(),
            cache_creation_input_tokens,
            reasoning_tokens,
            uncached_input_tokens,
            processed_input_tokens,
            processed_total_tokens,
            token_accounting_semantics,
            token_accounting_quality,
            tool_calls_json: event.tool_calls.as_ref().and_then(|calls| {
                let bounded = calls
                    .iter()
                    .take(sumpter_core::stream_terminal::MAX_TOOL_CALLS)
                    .filter_map(|call| {
                        let value = call.trim();
                        (!value.is_empty()
                            && value.len() <= sumpter_core::stream_terminal::MAX_TOOL_NAME_BYTES
                            && !value.chars().any(char::is_control))
                        .then_some(value.to_owned())
                    })
                    .collect::<Vec<_>>();
                (!bounded.is_empty()).then(|| serde_json::to_string(&bounded).unwrap_or_default())
            }),
            request_method: event.request_method.clone(),
            request_path: event.request_path.clone(),
            route_intent: event.route_intent.clone(),
            client_variant: event
                .client_variant
                .clone()
                .or_else(|| Some(event.derived_client_variant().into())),
            agent_role: event
                .agent_role
                .clone()
                .or_else(|| Some(event.derived_agent_role().into())),
            agent_name: event.agent_name.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.agent_name.clone())
            }),
            parent_thread_id: event.parent_thread_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.parent_thread_id.clone())
            }),
            parent_turn_id: event.parent_turn_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.parent_turn_id.clone())
            }),
            root_turn_id: event.root_turn_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.root_turn_id.clone())
            }),
        }
    }
}

/// One normalization contract for database projections and session exports.
fn normalized_input_tokens(
    protocol: Option<ProviderProtocol>,
    input: Option<i64>,
    read: Option<i64>,
    write: Option<i64>,
) -> (&'static str, Option<i64>, Option<i64>) {
    let semantics = match protocol {
        Some(ProviderProtocol::Anthropic) => "independent",
        Some(
            ProviderProtocol::OpenAI | ProviderProtocol::OpenAIResponses | ProviderProtocol::Gemini,
        ) => "subset",
        None => "unknown",
    };
    let (processed, uncached) = match (semantics, input) {
        ("independent", Some(input)) => (
            Some(
                input
                    .saturating_add(read.unwrap_or(0))
                    .saturating_add(write.unwrap_or(0)),
            ),
            Some(input),
        ),
        ("subset", Some(input)) => (
            Some(input),
            Some(
                input
                    .saturating_sub(read.unwrap_or(0))
                    .saturating_sub(write.unwrap_or(0))
                    .max(0),
            ),
        ),
        (_, input) => (input, input),
    };
    (semantics, processed, uncached)
}

fn token_i64(value: Option<u64>) -> Option<i64> {
    value.map(|value| value.min(i64::MAX as u64) as i64)
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCounters {
    pub client_requests: i64,
    pub client_successes: i64,
    pub client_failures: i64,
    pub upstream_attempts: i64,
    pub upstream_successes: i64,
    pub upstream_failures: i64,
    pub failovers: i64,
}

impl RuntimeCounters {
    pub fn from_snapshot(snapshot: &RuntimeSnapshot) -> Self {
        Self {
            client_requests: snapshot.client_requests,
            client_successes: snapshot.client_successes,
            client_failures: snapshot.client_failures,
            upstream_attempts: snapshot.upstream_attempts,
            upstream_successes: snapshot.upstream_successes,
            upstream_failures: snapshot.upstream_failures,
            failovers: snapshot.failovers,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeChange {
    pub seq: i64,
    pub change_seq: i64,
    pub event: RuntimeEvent,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEventListItem {
    pub session_source: Option<String>,
    pub hook_event: Option<String>,
    pub details_omitted: bool,
    pub cache_read: CacheReadSummary,
    pub usage_summary: Option<ResponseUsage>,
    pub seq: i64,
    pub change_seq: i64,
    pub id: String,
    pub timestamp: f64,
    pub kind: String,
    #[serde(rename = "clientVariant", skip_serializing_if = "Option::is_none")]
    pub client_variant: Option<String>,
    #[serde(rename = "agentRole", skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(rename = "agentName", skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(rename = "parentThreadId", skip_serializing_if = "Option::is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(rename = "parentTurnId", skip_serializing_if = "Option::is_none")]
    pub parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnId", skip_serializing_if = "Option::is_none")]
    pub root_turn_id: Option<String>,
    #[serde(rename = "codexMetadata")]
    pub codex_metadata: Option<CodexMetadata>,
    /// 客户端 `X-Sumpter-*` 声明的项目归因。投影必须带上:macOS App 只从这个列表
    /// 端点加载事件(单事件详情仅在选中时拉),字段缺了那边就恒显示「未识别项目」。
    #[serde(rename = "clientDeclared")]
    pub client_declared: Option<ClientDeclaredMetadata>,
    #[serde(rename = "grokMetadata")]
    pub grok_metadata: Option<GrokMetadata>,
    /// 服务端算好的项目归因(投影列)。分页列表走 SQLite 投影快路径,不解
    /// `payload_json`,因此 `codex_metadata` / `client_declared` 恒为 None ——
    /// 客户端若只按那两个字段判断,翻页拿到的行会全部显示「未识别项目」,而 SSE
    /// 推送的同一批事件却是好的(那条路径带完整字段)。这两个字段就是补这个缺口:
    /// 优先级(Codex 结构化 workspace > 客户端声明)已由服务端 `project_identity`
    /// 统一决定,客户端直接用,不要各自再推一遍。
    #[serde(rename = "projectName", skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    #[serde(rename = "projectSource", skip_serializing_if = "Option::is_none")]
    pub project_source: Option<String>,
    /// Wrapper 采集的本机用户名，只用于「本地(kkl)」展示，不进项目 identity。
    #[serde(rename = "localUser", skip_serializing_if = "Option::is_none")]
    pub local_user: Option<String>,
    #[serde(rename = "codexThreadClass", skip_serializing_if = "Option::is_none")]
    pub codex_thread_class: Option<String>,
    #[serde(rename = "attributionScope", skip_serializing_if = "Option::is_none")]
    pub attribution_scope: Option<String>,
    #[serde(rename = "clientModel")]
    pub client_model: Option<String>,
    #[serde(rename = "sourceFormat")]
    pub source_format: Option<ProviderProtocol>,
    #[serde(rename = "targetFormat")]
    pub target_format: Option<ProviderProtocol>,
    #[serde(rename = "routeMode")]
    pub route_mode: Option<RouteMode>,
    pub phase: Option<RuntimeEventPhase>,
    pub outcome: Option<RuntimeEventOutcome>,
    pub status_code: i64,
    #[serde(rename = "requestID")]
    pub request_id: Option<String>,
    #[serde(rename = "requestMethod")]
    pub request_method: Option<String>,
    #[serde(rename = "requestPath")]
    pub request_path: Option<String>,
    #[serde(rename = "routeIntent")]
    pub route_intent: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    pub client_kind: Option<ClientKind>,
    pub request_purpose: Option<RequestPurpose>,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    pub endpoint_name: Option<String>,
    #[serde(rename = "modelGroupID")]
    pub model_group_id: Option<String>,
    pub model_group_name: Option<String>,
    #[serde(rename = "featureRuleID")]
    pub feature_rule_id: Option<String>,
    pub effective_model: Option<String>,
    pub upstream_model: Option<String>,
    pub failure_kind: Option<RuntimeFailureKind>,
    #[serde(rename = "failurePhase")]
    pub failure_phase: Option<RuntimeFailurePhase>,
    #[serde(rename = "failureDetail")]
    pub failure_detail: Option<String>,
    pub message: Option<String>,
    #[serde(rename = "streamTrace")]
    pub stream_trace: Option<StreamTrace>,
    #[serde(rename = "toolCalls")]
    pub tool_calls: Option<Vec<String>>,
    #[serde(rename = "timeoutMS")]
    pub timeout_ms: Option<i64>,
    #[serde(rename = "upstreamHost")]
    pub upstream_host: Option<String>,
    #[serde(rename = "upstreamStatusCode")]
    pub upstream_status_code: Option<i64>,
    #[serde(rename = "upstreamRequestID")]
    pub upstream_request_id: Option<String>,
    #[serde(rename = "durationMS")]
    pub duration_ms: i64,
    #[serde(rename = "ttfbMS")]
    pub ttfb_ms: Option<i64>,
    pub failover: bool,
}

impl RuntimeEventListItem {
    /// Project a persisted runtime event into the public list/API shape.
    ///
    /// The shared engine also uses the same projection when SQLite is
    /// unavailable and it serves its in-memory fallback, so this constructor
    /// is intentionally public at the crate boundary rather than duplicated
    /// in each platform facade.
    pub fn from_change(seq: i64, change_seq: i64, event: RuntimeEvent) -> Self {
        let cache_read = event.observed_cache_read();
        let usage_summary = event
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.usage.clone());
        let session_source = event_session_projection(&event).1.to_owned();
        let derived_client_variant = event.derived_client_variant().to_owned();
        let derived_agent_role = event.derived_agent_role().to_owned();
        // 必须在 event 被逐字段移动之前算:两个投影值都要借用整个 event。
        let projected_name = project_base(&project_identity(&event));
        let projected_source = project_source(&event).to_string();
        let local_user = event_local_user(&event);
        let is_codex_event = event.kind == KIND_CLIENT
            && (event.codex_metadata.is_some() || event.client_kind == Some(ClientKind::Codex));
        let codex_thread_class = is_codex_event.then(|| {
            codex_thread_class(event.codex_metadata.as_ref())
                .as_str()
                .to_owned()
        });
        let attribution_scope =
            is_codex_event.then(|| event_attribution_scope(&event).as_str().to_owned());
        Self {
            cache_read,
            usage_summary,
            details_omitted: false,
            session_source: Some(session_source),
            hook_event: event.hook_event,
            seq,
            change_seq,
            id: event.id,
            timestamp: event.timestamp,
            kind: event.kind,
            client_variant: event
                .client_variant
                .clone()
                .or(Some(derived_client_variant)),
            agent_role: event.agent_role.clone().or(Some(derived_agent_role)),
            agent_name: event.agent_name.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.agent_name.clone())
            }),
            parent_thread_id: event.parent_thread_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.parent_thread_id.clone())
            }),
            parent_turn_id: event.parent_turn_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.parent_turn_id.clone())
            }),
            root_turn_id: event.root_turn_id.clone().or_else(|| {
                event
                    .codex_metadata
                    .as_ref()
                    .and_then(|m| m.root_turn_id.clone())
            }),
            project_name: Some(projected_name),
            project_source: Some(projected_source),
            local_user,
            codex_thread_class,
            attribution_scope,
            codex_metadata: event.codex_metadata,
            client_declared: event.client_declared,
            grok_metadata: event.grok_metadata,
            client_model: event.client_model,
            source_format: event.source_format,
            target_format: event.target_format,
            route_mode: event.route_mode,
            phase: event.phase,
            outcome: event.outcome,
            status_code: event.status_code,
            request_id: event.request_id,
            request_method: event.request_method,
            request_path: event.request_path,
            route_intent: event.route_intent,
            session_id: event.session_id,
            client_kind: event.client_kind,
            request_purpose: event.request_purpose,
            endpoint_id: event.endpoint_id,
            endpoint_name: event.endpoint_name,
            model_group_id: event.model_group_id,
            model_group_name: event.model_group_name,
            feature_rule_id: event.feature_rule_id,
            effective_model: event.effective_model,
            upstream_model: event.upstream_model,
            failure_kind: event.failure_kind,
            failure_phase: event.failure_phase,
            failure_detail: event.failure_detail,
            message: event.message,
            stream_trace: event.stream_trace,
            tool_calls: event.tool_calls,
            timeout_ms: event.timeout_ms,
            upstream_host: event.upstream_host,
            upstream_status_code: event.upstream_status_code,
            upstream_request_id: event.upstream_request_id,
            duration_ms: event.duration_ms,
            ttfb_ms: event.ttfb_ms,
            failover: event.failover,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStorageStatus {
    pub backend: &'static str,
    pub state: &'static str,
    pub pending_events: usize,
    pub pending_bytes: usize,
    pub event_count: i64,
    pub completed_event_count: i64,
    pub in_flight_event_count: i64,
    pub oldest_event_at: Option<f64>,
    pub newest_event_at: Option<f64>,
    pub retained_from_seq: i64,
    pub payload_bytes: u64,
    pub live_bytes: u64,
    pub allocated_bytes: u64,
    pub db_bytes: u64,
    pub wal_bytes: u64,
    pub schema_version: i64,
    pub backfill_cursor: i64,
    pub backfill_complete: bool,
    pub backfill_failed: i64,
    pub indexes_ready: bool,
    pub rollup_complete: bool,
    pub rollup_max_seq: i64,
    pub rollup_history_generation: i64,
    pub rollup_failed: i64,
    pub rollup_dirty_buckets: i64,
    pub user_deleted_events: i64,
    pub user_deleted_requests: i64,
    pub last_commit_at: Option<f64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSummary {
    pub api_version: u8,
    pub storage: RuntimeStorageStatus,
    pub reset_generation: i64,
    pub history_generation: i64,
    pub counters: RuntimeCounters,
    pub latest_event: Option<RuntimeEvent>,
}

#[derive(Debug, Clone)]
struct WriteMessage {
    seq: i64,
    change_seq: i64,
    event: RuntimeEvent,
    counters: RuntimeCounters,
    bytes: usize,
}

#[derive(Default)]
struct PendingBatch {
    messages: HashMap<String, WriteMessage>,
    bytes: usize,
    first_at: Option<std::time::Instant>,
}

impl PendingBatch {
    fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    fn len(&self) -> usize {
        self.messages.len()
    }

    fn upsert(&mut self, message: WriteMessage, inner: &Arc<Inner>) {
        if let Some(previous) = self
            .messages
            .insert(message.event.id.clone(), message.clone())
        {
            self.bytes = self.bytes.saturating_sub(previous.bytes);
            inner.pending_events.fetch_sub(1, Ordering::AcqRel);
            inner
                .pending_bytes
                .fetch_sub(previous.bytes, Ordering::AcqRel);
        }
        self.bytes += message.bytes;
        self.first_at.get_or_insert_with(std::time::Instant::now);
    }

    fn take(&mut self) -> Vec<WriteMessage> {
        self.first_at = None;
        self.bytes = 0;
        self.messages.drain().map(|(_, message)| message).collect()
    }

    fn flush_wait(&self) -> Duration {
        self.first_at
            .map(|started| Duration::from_secs(1).saturating_sub(started.elapsed()))
            .unwrap_or(Duration::from_secs(1))
    }
}

// Write 承载整条事件,比其它控制指令大得多。worker 队列每次只传一条命令,
// 装箱反而多一次堆分配和解引用,收益不抵成本。
#[allow(clippy::large_enum_variant)]
enum Command {
    Write(WriteMessage),
    Reset(mpsc::Sender<Result<i64, String>>),
    Recreate(mpsc::Sender<Result<i64, String>>),
    CleanupBefore {
        older_than: f64,
        reply: mpsc::Sender<Result<RuntimeCleanupMutation, String>>,
    },
    DeleteSession {
        session_id: String,
        reply: mpsc::Sender<Result<SessionMutation, String>>,
    },
    SetRetention {
        update: RuntimeRetentionUpdate,
        reply: mpsc::Sender<Result<RuntimeRetentionMutation, String>>,
    },
    ReplacePricing {
        update: RuntimePricingUpdate,
        reply: mpsc::Sender<Result<RuntimePricingMutation, String>>,
    },
    Flush(mpsc::Sender<Result<(), String>>),
    Shutdown,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMutation {
    pub reset_generation: i64,
    pub deleted_events: i64,
    pub deleted_requests: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCleanupPreview {
    pub older_than: f64,
    pub deletable_events: i64,
    pub deletable_requests: i64,
    pub remaining_events: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCleanupMutation {
    pub older_than: f64,
    pub deleted_events: i64,
    pub deleted_requests: i64,
    pub remaining_events: i64,
    pub history_generation: i64,
}

#[derive(Debug, Clone)]
pub struct RuntimeRetentionUpdate {
    pub expected_revision: i64,
    /// Optional rolling age limit in whole days. `None` disables this time
    /// dimension; the cutoff uses a rolling 24-hour window.
    pub max_age_days: Option<i64>,
    /// Optional SQLite live-storage limit. When reached, the oldest completed
    /// request history is rotated out; in-flight rows are never deleted.
    pub storage_limit_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRetentionMutation {
    pub revision: i64,
    pub max_age_days: Option<i64>,
    pub storage_limit_bytes: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RuntimeModelPriceInput {
    pub endpoint_id: Option<String>,
    pub model_key: String,
    pub effective_from: f64,
    pub effective_to: Option<f64>,
    pub input_per_million_micros: Option<i64>,
    pub output_per_million_micros: Option<i64>,
    pub cache_read_per_million_micros: Option<i64>,
    pub cache_creation_per_million_micros: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RuntimePricingUpdate {
    pub expected_revision: i64,
    pub currency: String,
    pub prices: Vec<RuntimeModelPriceInput>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimePricingMutation {
    pub revision: i64,
    pub currency: String,
    pub price_count: usize,
}

struct StoreState {
    next_seq: i64,
    next_change_seq: i64,
    reset_generation: i64,
    history_generation: i64,
    counters: RuntimeCounters,
    active_sequences: HashMap<String, i64>,
    recent_changes: VecDeque<RuntimeChange>,
    latest_event: Option<RuntimeEvent>,
    last_commit_at: Option<f64>,
    last_error: Option<String>,
    storage: CachedStorageMetrics,
    storage_refreshed_at: std::time::Instant,
}

#[derive(Debug, Clone, Default)]
struct CachedStorageMetrics {
    event_count: i64,
    completed_event_count: i64,
    in_flight_event_count: i64,
    oldest_event_at: Option<f64>,
    newest_event_at: Option<f64>,
    retained_from_seq: i64,
    payload_bytes: u64,
    live_bytes: u64,
    allocated_bytes: u64,
    schema_version: i64,
    backfill_cursor: i64,
    backfill_complete: bool,
    backfill_failed: i64,
    indexes_ready: bool,
    rollup_complete: bool,
    rollup_max_seq: i64,
    rollup_history_generation: i64,
    rollup_failed: i64,
    rollup_dirty_buckets: i64,
    user_deleted_events: i64,
    user_deleted_requests: i64,
}

impl StoreState {
    fn remember_change(&mut self, change: RuntimeChange) {
        self.recent_changes.push_back(change);
        while self.recent_changes.len() > RECENT_CHANGES_LIMIT {
            self.recent_changes.pop_front();
        }
    }
}

struct Inner {
    path: PathBuf,
    sender: mpsc::SyncSender<Command>,
    state: Mutex<StoreState>,
    pending_events: AtomicUsize,
    pending_bytes: AtomicUsize,
    backpressure: AtomicBool,
    hard_backpressure: AtomicBool,
    #[cfg(test)]
    fail_writes: AtomicBool,
}

#[derive(Clone)]
pub struct RuntimeStore {
    inner: Arc<Inner>,
    // Only public store handles own this guard; the worker never does.
    // The last handle waits for the writer and ORM connection to close.
    _worker: Arc<WorkerLifecycle>,
}

struct WorkerLifecycle {
    inner: Arc<Inner>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for WorkerLifecycle {
    fn drop(&mut self) {
        let _ = self.inner.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

mod analytics;
mod export;
mod maintenance;
mod models;
mod schema;
mod store_api;
mod worker;

use analytics::*;
use export::*;
use maintenance::*;
use schema::*;
use worker::*;

#[cfg(test)]
mod tests;

/// A database startup problem that both management clients can act on.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDatabaseIssue {
    pub code: &'static str,
    pub schema_version: i64,
    pub supported_schema_version: i64,
    pub projection_version: Option<i64>,
    pub supported_projection_version: i64,
    pub requires_recreate: bool,
    pub message: String,
}

impl RuntimeStore {
    pub fn database_issue(path: &Path) -> Result<Option<RuntimeDatabaseIssue>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|error| error.to_string())?;
        schema::database_issue_on(&connection)
    }

    /// Called only by the explicit management recreate operation, while no
    /// runtime worker exists. Recheck versions so a newer database is never erased.
    pub fn recreate_legacy(path: &Path) -> Result<(Self, RuntimeSnapshot), String> {
        let issue =
            Self::database_issue(path)?.ok_or("runtime database does not require recreation")?;
        if !issue.requires_recreate {
            return Err(issue.message);
        }
        let mut connection = Connection::open(path).map_err(|error| error.to_string())?;
        maintenance::rebuild_schema(&mut connection).map_err(|error| error.to_string())?;
        drop(connection);
        Self::new(path)
    }
}
