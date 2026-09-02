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

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::{
    APPLE_EPOCH_OFFSET_SECS, ClientDeclaredMetadata, ClientKind, CodexMetadata, KIND_CLIENT,
    KIND_NOTIFY, KIND_UPSTREAM, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase,
    RuntimeFailureKind, RuntimeFailurePhase, RuntimeSnapshot, STATUS_CLIENT_DISCONNECTED,
    StreamTrace, codex_attribution_scope, codex_thread_class,
};
use sumpter_core::routing::{RESOURCE_ROUTING_MODEL, RequestPurpose, RouteMode};

const SCHEMA_VERSION: i64 = 3;
const PROJECTION_VERSION: i64 = 4;
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
    project_id: String,
    project_name: String,
    project_source: &'static str,
    codex_thread_class: Option<&'static str>,
    attribution_scope: Option<&'static str>,
    workspace_paths_json: String,
    endpoint_name: Option<String>,
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
    cache_creation_input_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    uncached_input_tokens: Option<i64>,
    processed_input_tokens: Option<i64>,
    processed_total_tokens: Option<i64>,
    token_accounting_semantics: &'static str,
    token_accounting_quality: &'static str,
    tool_calls_json: Option<String>,
}

impl EventProjection {
    fn from_event(event: &RuntimeEvent, payload_bytes: usize) -> Self {
        let usage = event
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.usage.as_ref());
        let input_tokens = usage.and_then(|usage| token_i64(usage.input_tokens));
        let output_tokens = usage.and_then(|usage| token_i64(usage.output_tokens));
        let cache_read_input_tokens =
            usage.and_then(|usage| token_i64(usage.cache_read_input_tokens));
        let cache_creation_input_tokens =
            usage.and_then(|usage| token_i64(usage.cache_creation_input_tokens));
        let reasoning_tokens = usage.and_then(|usage| token_i64(usage.reasoning_tokens));
        let token_accounting_semantics = match event
            .target_format
            .or(event.source_format)
            .map(|format| format.token())
        {
            Some("anthropic") => "independent",
            Some("openai") | Some("openai-responses") => "subset",
            _ => "unknown",
        };
        let (processed_input_tokens, uncached_input_tokens) =
            match (token_accounting_semantics, input_tokens) {
                ("independent", Some(input)) => (
                    Some(
                        input
                            .saturating_add(cache_read_input_tokens.unwrap_or(0))
                            .saturating_add(cache_creation_input_tokens.unwrap_or(0)),
                    ),
                    Some(input),
                ),
                ("subset", Some(input)) => (
                    Some(input),
                    Some(
                        input
                            .saturating_sub(cache_read_input_tokens.unwrap_or(0))
                            .saturating_sub(cache_creation_input_tokens.unwrap_or(0)),
                    ),
                ),
                (_, Some(input)) => (Some(input), Some(input)),
                _ => (None, None),
            };
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
            project_id,
            project_name,
            project_source,
            codex_thread_class,
            attribution_scope,
            workspace_paths_json,
            endpoint_name: event.endpoint_name.clone(),
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
                    .take(32)
                    .filter_map(|call| {
                        let value = call.trim();
                        (!value.is_empty() && value.len() <= 128).then_some(value.to_owned())
                    })
                    .collect::<Vec<_>>();
                (!bounded.is_empty()).then(|| serde_json::to_string(&bounded).unwrap_or_default())
            }),
        }
    }
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
    pub seq: i64,
    pub change_seq: i64,
    pub id: String,
    pub timestamp: f64,
    pub kind: String,
    #[serde(rename = "codexMetadata")]
    pub codex_metadata: Option<CodexMetadata>,
    /// 客户端 `X-Sumpter-*` 声明的项目归因。投影必须带上:macOS App 只从这个列表
    /// 端点加载事件(单事件详情仅在选中时拉),字段缺了那边就恒显示「未识别项目」。
    #[serde(rename = "clientDeclared")]
    pub client_declared: Option<ClientDeclaredMetadata>,
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
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    pub client_kind: Option<ClientKind>,
    pub request_purpose: Option<RequestPurpose>,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    pub endpoint_name: Option<String>,
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
        // 必须在 event 被逐字段移动之前算:两个投影值都要借用整个 event。
        let projected_name = project_base(&project_identity(&event));
        let projected_source = project_source(&event).to_string();
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
            seq,
            change_seq,
            id: event.id,
            timestamp: event.timestamp,
            kind: event.kind,
            project_name: Some(projected_name),
            project_source: Some(projected_source),
            codex_thread_class,
            attribution_scope,
            codex_metadata: event.codex_metadata,
            client_declared: event.client_declared,
            client_model: event.client_model,
            source_format: event.source_format,
            target_format: event.target_format,
            route_mode: event.route_mode,
            phase: event.phase,
            outcome: event.outcome,
            status_code: event.status_code,
            request_id: event.request_id,
            session_id: event.session_id,
            client_kind: event.client_kind,
            request_purpose: event.request_purpose,
            endpoint_id: event.endpoint_id,
            endpoint_name: event.endpoint_name,
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
}

fn setup_connection(connection: &mut Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA wal_autocheckpoint = 1000;
         PRAGMA cache_size = -2048;",
    )?;
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS runtime_counters (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             client_requests INTEGER NOT NULL,
             client_successes INTEGER NOT NULL,
             client_failures INTEGER NOT NULL,
             upstream_attempts INTEGER NOT NULL,
             upstream_successes INTEGER NOT NULL,
             upstream_failures INTEGER NOT NULL,
             failovers INTEGER NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_events (
             seq INTEGER PRIMARY KEY,
             change_seq INTEGER NOT NULL UNIQUE,
             event_id TEXT NOT NULL UNIQUE,
             request_id TEXT,
             timestamp REAL NOT NULL,
             kind TEXT NOT NULL,
             phase TEXT,
             outcome TEXT,
             status_code INTEGER NOT NULL,
             client_kind TEXT,
             request_purpose TEXT,
             endpoint_id TEXT,
             failure_kind TEXT,
             is_in_flight INTEGER NOT NULL,
             payload_json TEXT NOT NULL,
             created_at REAL NOT NULL,
             updated_at REAL NOT NULL
         );
         CREATE INDEX IF NOT EXISTS runtime_events_kind_seq ON runtime_events(kind, seq DESC);
         CREATE INDEX IF NOT EXISTS runtime_events_request_id ON runtime_events(request_id);
         CREATE INDEX IF NOT EXISTS runtime_events_timestamp ON runtime_events(timestamp);",
    )?;
    let previous_version = meta_i64(connection, "schema_version")?.unwrap_or(0);
    if previous_version > SCHEMA_VERSION {
        return Err(rusqlite::Error::InvalidParameterName(format!(
            "runtime schema {previous_version} is newer than supported {SCHEMA_VERSION}"
        )));
    }
    ensure_v2_schema(connection)?;
    let hourly_rollup_meta_exists = meta_i64(connection, "hourly_rollup_complete")?.is_some();
    let unprojected_exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events
                       WHERE projection_version>=0 AND projection_version<>?1 LIMIT 1)",
        params![PROJECTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let retained_event_count =
        connection.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
    let retained_from_seq = connection.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'), 1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let defaults = [
        ("reset_generation", "0".to_string()),
        ("next_seq", "1".to_string()),
        ("next_change_seq", "1".to_string()),
        ("history_generation", "0".to_string()),
        ("retained_from_seq", retained_from_seq.to_string()),
        ("projection_backfill_cursor", "0".to_string()),
        (
            "projection_backfill_complete",
            i64::from(!unprojected_exists).to_string(),
        ),
        ("projection_indexes_ready", "0".to_string()),
        ("projection_backfill_failed", "0".to_string()),
        ("hourly_rollup_complete", "0".to_string()),
        ("hourly_rollup_max_seq", "0".to_string()),
        (
            "hourly_rollup_history_generation",
            meta_i64(connection, "history_generation")?
                .unwrap_or(0)
                .to_string(),
        ),
        ("hourly_rollup_failed", "0".to_string()),
        ("user_deleted_events", "0".to_string()),
        ("user_deleted_requests", "0".to_string()),
        ("retained_event_count", retained_event_count.to_string()),
    ];
    for (key, value) in defaults {
        connection.execute(
            "INSERT INTO runtime_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",
            params![key, value],
        )?;
    }
    if unprojected_exists {
        set_meta(connection, "projection_backfill_complete", 0)?;
    }
    connection.execute(
        "INSERT INTO runtime_counters VALUES(1,0,0,0,0,0,0,0) ON CONFLICT(id) DO NOTHING",
        [],
    )?;
    connection.execute(
        "INSERT INTO runtime_retention(
            id,revision,updated_at
         ) VALUES(1,1,?1)
         ON CONFLICT(id) DO NOTHING",
        params![now()],
    )?;
    connection.execute(
        "INSERT INTO runtime_pricing_meta(id,revision,currency,updated_at)
         VALUES(1,1,'USD',?1) ON CONFLICT(id) DO NOTHING",
        params![now()],
    )?;
    if previous_version < SCHEMA_VERSION || !hourly_rollup_meta_exists {
        // Rollup columns and threshold semantics changed in v3. Rebuild all
        // retained buckets from projection columns; never read payload_json
        // for analytics.
        connection.execute("DELETE FROM runtime_hourly_rollups", [])?;
        connection.execute(
            "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
             SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
             FROM runtime_events
             WHERE is_in_flight=0 AND kind IN (?1,?2)",
            params![KIND_CLIENT, KIND_UPSTREAM],
        )?;
        let dirty_exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )?;
        set_meta(
            connection,
            "hourly_rollup_complete",
            i64::from(!unprojected_exists && !dirty_exists),
        )?;
    }
    set_meta(connection, "schema_version", SCHEMA_VERSION)?;
    if !unprojected_exists {
        set_meta(connection, "projection_backfill_complete", 1)?;
        if retained_event_count == 0 {
            create_projection_indexes(connection)?;
        }
    }
    Ok(())
}

fn ensure_v2_schema(connection: &mut Connection) -> rusqlite::Result<()> {
    let existing = {
        let mut statement = connection.prepare("PRAGMA table_info(runtime_events)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let columns = [
        ("projection_version", "INTEGER NOT NULL DEFAULT 0"),
        ("payload_bytes", "INTEGER NOT NULL DEFAULT 0"),
        ("session_key", "TEXT"),
        ("session_source", "TEXT"),
        ("project_id", "TEXT"),
        ("project_name", "TEXT"),
        ("project_source", "TEXT"),
        ("workspace_paths_json", "TEXT"),
        ("endpoint_name", "TEXT"),
        ("feature_rule_id", "TEXT"),
        ("client_model", "TEXT"),
        ("effective_model", "TEXT"),
        ("upstream_model", "TEXT"),
        ("failure_phase", "TEXT"),
        ("source_format", "TEXT"),
        ("target_format", "TEXT"),
        ("route_mode", "TEXT"),
        ("upstream_status_code", "INTEGER"),
        ("duration_ms", "INTEGER"),
        ("ttfb_ms", "INTEGER"),
        ("failover", "INTEGER"),
        ("stream_terminal", "TEXT"),
        ("codex_metadata_present", "INTEGER"),
        ("usage_present", "INTEGER"),
        ("input_tokens", "INTEGER"),
        ("output_tokens", "INTEGER"),
        ("cache_read_input_tokens", "INTEGER"),
        ("cache_creation_input_tokens", "INTEGER"),
        ("reasoning_tokens", "INTEGER"),
        ("uncached_input_tokens", "INTEGER"),
        ("processed_input_tokens", "INTEGER"),
        ("processed_total_tokens", "INTEGER"),
        ("token_accounting_semantics", "TEXT"),
        ("token_accounting_quality", "TEXT"),
        ("tool_calls_json", "TEXT"),
        ("codex_thread_class", "TEXT"),
        ("attribution_scope", "TEXT"),
    ];
    let transaction = connection.transaction()?;
    for (name, definition) in columns {
        if !existing.contains(name) {
            transaction.execute_batch(&format!(
                "ALTER TABLE runtime_events ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS runtime_hourly_rollups (
             bucket_start INTEGER PRIMARY KEY,
             bucket_end INTEGER NOT NULL,
             max_seq INTEGER NOT NULL DEFAULT 0,
             client_requests INTEGER NOT NULL DEFAULT 0,
             client_successes INTEGER NOT NULL DEFAULT 0,
             client_failures INTEGER NOT NULL DEFAULT 0,
             client_cancelled INTEGER NOT NULL DEFAULT 0,
             failovers INTEGER NOT NULL DEFAULT 0,
             duration_ms_sum INTEGER NOT NULL DEFAULT 0,
             duration_count INTEGER NOT NULL DEFAULT 0,
             duration_slow_count INTEGER NOT NULL DEFAULT 0,
             duration_critical_count INTEGER NOT NULL DEFAULT 0,
             ttfb_ms_sum INTEGER NOT NULL DEFAULT 0,
             ttfb_count INTEGER NOT NULL DEFAULT 0,
             ttfb_slow_count INTEGER NOT NULL DEFAULT 0,
             ttfb_critical_count INTEGER NOT NULL DEFAULT 0,
             client_unknown_results INTEGER NOT NULL DEFAULT 0,
             failover_terminal_requests INTEGER NOT NULL DEFAULT 0,
             failover_recovered_requests INTEGER NOT NULL DEFAULT 0,
             upstream_attempts INTEGER NOT NULL DEFAULT 0,
             upstream_successes INTEGER NOT NULL DEFAULT 0,
             upstream_failures INTEGER NOT NULL DEFAULT 0,
             input_tokens INTEGER NOT NULL DEFAULT 0,
             output_tokens INTEGER NOT NULL DEFAULT 0,
             cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
             cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
             reasoning_tokens INTEGER NOT NULL DEFAULT 0,
             uncached_input_tokens INTEGER NOT NULL DEFAULT 0,
             processed_input_tokens INTEGER NOT NULL DEFAULT 0,
             processed_total_tokens INTEGER NOT NULL DEFAULT 0,
             usage_present_requests INTEGER NOT NULL DEFAULT 0,
             accounting_known_requests INTEGER NOT NULL DEFAULT 0,
             accounting_unknown_requests INTEGER NOT NULL DEFAULT 0,
             cache_read_reported_requests INTEGER NOT NULL DEFAULT 0,
             cache_read_hit_requests INTEGER NOT NULL DEFAULT 0,
             cache_eligible_requests INTEGER NOT NULL DEFAULT 0,
             cache_unknown_requests INTEGER NOT NULL DEFAULT 0,
             cache_read_token_numerator INTEGER NOT NULL DEFAULT 0,
             cache_read_token_denominator INTEGER NOT NULL DEFAULT 0,
             input_tokens_present INTEGER NOT NULL DEFAULT 0,
             output_tokens_present INTEGER NOT NULL DEFAULT 0,
             cache_read_input_tokens_present INTEGER NOT NULL DEFAULT 0,
             cache_creation_input_tokens_present INTEGER NOT NULL DEFAULT 0,
             reasoning_tokens_present INTEGER NOT NULL DEFAULT 0,
             cost_accounting_complete_requests INTEGER NOT NULL DEFAULT 0,
             cost_unknown_accounting_requests INTEGER NOT NULL DEFAULT 0,
             cost_numerator INTEGER NOT NULL DEFAULT 0,
             cost_priced_requests INTEGER NOT NULL DEFAULT 0,
             cost_unpriced_requests INTEGER NOT NULL DEFAULT 0,
             cost_unknown_requests INTEGER NOT NULL DEFAULT 0,
             cost_price_revision INTEGER,
             updated_at REAL NOT NULL DEFAULT 0
         );
         CREATE TABLE IF NOT EXISTS runtime_hourly_rollup_dirty (
             bucket_start INTEGER PRIMARY KEY
         );
         CREATE TABLE IF NOT EXISTS runtime_retention (
             id INTEGER PRIMARY KEY CHECK(id=1),
             revision INTEGER NOT NULL DEFAULT 1,
             updated_at REAL NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_pricing_meta (
             id INTEGER PRIMARY KEY CHECK(id=1),
             revision INTEGER NOT NULL DEFAULT 1,
             currency TEXT NOT NULL DEFAULT 'USD',
             updated_at REAL NOT NULL
         );
         CREATE TABLE IF NOT EXISTS runtime_model_prices (
             id INTEGER PRIMARY KEY,
             model_key TEXT NOT NULL,
             effective_from REAL NOT NULL,
             effective_to REAL,
             input_per_million_micros INTEGER,
             output_per_million_micros INTEGER,
             cache_read_per_million_micros INTEGER,
             cache_creation_per_million_micros INTEGER,
             created_at REAL NOT NULL,
             updated_at REAL NOT NULL,
             UNIQUE(model_key,effective_from),
             CHECK(effective_to IS NULL OR effective_to>effective_from),
             CHECK(input_per_million_micros IS NULL OR input_per_million_micros>=0),
             CHECK(output_per_million_micros IS NULL OR output_per_million_micros>=0),
             CHECK(cache_read_per_million_micros IS NULL OR cache_read_per_million_micros>=0),
             CHECK(cache_creation_per_million_micros IS NULL OR cache_creation_per_million_micros>=0)
         );
         CREATE INDEX IF NOT EXISTS runtime_model_prices_lookup_v2
             ON runtime_model_prices(model_key,effective_from DESC);",
    )?;
    let rollup_columns = {
        let mut statement = transaction.prepare("PRAGMA table_info(runtime_hourly_rollups)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<HashSet<_>, _>>()?
    };
    let required_rollup_columns = [
        ("bucket_end", "INTEGER NOT NULL DEFAULT 0"),
        ("max_seq", "INTEGER NOT NULL DEFAULT 0"),
        ("duration_slow_count", "INTEGER NOT NULL DEFAULT 0"),
        ("duration_critical_count", "INTEGER NOT NULL DEFAULT 0"),
        ("ttfb_slow_count", "INTEGER NOT NULL DEFAULT 0"),
        ("ttfb_critical_count", "INTEGER NOT NULL DEFAULT 0"),
        ("client_unknown_results", "INTEGER NOT NULL DEFAULT 0"),
        ("failover_terminal_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("failover_recovered_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("upstream_attempts", "INTEGER NOT NULL DEFAULT 0"),
        ("upstream_successes", "INTEGER NOT NULL DEFAULT 0"),
        ("upstream_failures", "INTEGER NOT NULL DEFAULT 0"),
        ("accounting_known_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("accounting_unknown_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cache_read_reported_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cache_read_hit_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cache_read_token_numerator", "INTEGER NOT NULL DEFAULT 0"),
        ("cache_read_token_denominator", "INTEGER NOT NULL DEFAULT 0"),
        ("input_tokens_present", "INTEGER NOT NULL DEFAULT 0"),
        ("output_tokens_present", "INTEGER NOT NULL DEFAULT 0"),
        (
            "cache_read_input_tokens_present",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "cache_creation_input_tokens_present",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("reasoning_tokens_present", "INTEGER NOT NULL DEFAULT 0"),
        (
            "cost_accounting_complete_requests",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "cost_unknown_accounting_requests",
            "INTEGER NOT NULL DEFAULT 0",
        ),
        ("cost_numerator", "INTEGER NOT NULL DEFAULT 0"),
        ("cost_priced_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cost_unpriced_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cost_unknown_requests", "INTEGER NOT NULL DEFAULT 0"),
        ("cost_price_revision", "INTEGER"),
        ("updated_at", "REAL NOT NULL DEFAULT 0"),
    ];
    for (name, definition) in required_rollup_columns {
        if !rollup_columns.contains(name) {
            transaction.execute_batch(&format!(
                "ALTER TABLE runtime_hourly_rollups ADD COLUMN {name} {definition};"
            ))?;
        }
    }
    // A v3 database may already have the rollup table while the cost columns
    // were introduced by a later v3 patch. Rebuild those buckets once so a
    // stale zero-cost row is never served as a complete trend.
    if !rollup_columns.contains("cost_numerator")
        || !rollup_columns.contains("cost_priced_requests")
        || !rollup_columns.contains("cost_unpriced_requests")
        || !rollup_columns.contains("cost_unknown_requests")
    {
        transaction.execute("DELETE FROM runtime_hourly_rollups", [])?;
        transaction.execute(
            "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
             SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
             FROM runtime_events WHERE is_in_flight=0 AND kind IN ('client','upstream')",
            [],
        )?;
        set_meta(&transaction, "hourly_rollup_complete", 0)?;
    }
    transaction.commit()
}

fn create_projection_indexes(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS runtime_events_inflight_change_v2
             ON runtime_events(change_seq) WHERE is_in_flight=1;
         CREATE INDEX IF NOT EXISTS runtime_events_outcome_seq_v2
             ON runtime_events(outcome,seq DESC) WHERE is_in_flight=0;
         CREATE INDEX IF NOT EXISTS runtime_events_time_seq_v2
             ON runtime_events(timestamp DESC,seq DESC) WHERE is_in_flight=0;
         CREATE INDEX IF NOT EXISTS runtime_events_session_time_v2
             ON runtime_events(session_key,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND session_key IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_project_time_v2
             ON runtime_events(project_id,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND project_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_endpoint_time_v2
             ON runtime_events(endpoint_id,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND endpoint_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_model_time_v2
             ON runtime_events(effective_model,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND effective_model IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_failure_time_v2
             ON runtime_events(failure_kind,failure_phase,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND outcome='failed';
         DROP INDEX IF EXISTS runtime_events_seq;
         DROP INDEX IF EXISTS runtime_events_change_seq;
         PRAGMA optimize;",
    )?;
    set_meta(connection, "projection_indexes_ready", 1)
}

fn hourly_bucket_start(timestamp: f64) -> i64 {
    (timestamp / 3_600.0).floor() as i64 * 3_600
}

fn mark_hourly_rollup_bucket(connection: &Connection, bucket_start: i64) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start) VALUES(?1)",
        params![bucket_start],
    )?;
    set_meta(connection, "hourly_rollup_complete", 0)
}

fn mark_event_hourly_rollup_dirty(
    connection: &Connection,
    event: &RuntimeEvent,
) -> rusqlite::Result<()> {
    if event.kind == KIND_CLIENT && !event.is_in_flight() {
        mark_hourly_rollup_bucket(connection, hourly_bucket_start(event.timestamp))?;
    }
    Ok(())
}

fn mark_request_hourly_rollups_dirty(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
         SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
         FROM runtime_events
         WHERE request_id=?1 AND kind=?2 AND is_in_flight=0",
        params![request_id, KIND_CLIENT],
    )?;
    set_meta(connection, "hourly_rollup_complete", 0)
}

#[derive(Default)]
struct HourlyRollupAccumulator {
    max_seq: i64,
    client_requests: i64,
    client_successes: i64,
    client_failures: i64,
    client_cancelled: i64,
    client_unknown_results: i64,
    failovers: i64,
    failover_terminal_requests: i64,
    failover_recovered_requests: i64,
    upstream_attempts: i64,
    upstream_successes: i64,
    upstream_failures: i64,
    duration_ms_sum: i64,
    duration_count: i64,
    duration_slow_count: i64,
    duration_critical_count: i64,
    ttfb_ms_sum: i64,
    ttfb_count: i64,
    ttfb_slow_count: i64,
    ttfb_critical_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
    uncached_input_tokens: i64,
    processed_input_tokens: i64,
    processed_total_tokens: i64,
    usage_present_requests: i64,
    accounting_known_requests: i64,
    accounting_unknown_requests: i64,
    cache_read_reported_requests: i64,
    cache_read_hit_requests: i64,
    cache_eligible_requests: i64,
    cache_unknown_requests: i64,
    cache_read_token_numerator: i64,
    cache_read_token_denominator: i64,
    input_tokens_present: i64,
    output_tokens_present: i64,
    cache_read_input_tokens_present: i64,
    cache_creation_input_tokens_present: i64,
    reasoning_tokens_present: i64,
    cost_accounting_complete_requests: i64,
    cost_unknown_accounting_requests: i64,
    cost_numerator: i128,
    cost_priced_requests: i64,
    cost_unpriced_requests: i64,
    cost_unknown_requests: i64,
}

#[derive(Clone)]
struct RollupPrice {
    endpoint_id: Option<String>,
    model_key: String,
    effective_from: f64,
    effective_to: Option<f64>,
    input_per_million_micros: Option<i64>,
    output_per_million_micros: Option<i64>,
    cache_read_per_million_micros: Option<i64>,
    cache_creation_per_million_micros: Option<i64>,
}

const SCOPED_PRICE_SEPARATOR: char = '\u{1f}';

fn split_rollup_price_key(value: String) -> (Option<String>, String) {
    match value.split_once(SCOPED_PRICE_SEPARATOR) {
        Some((endpoint_id, model_key)) if !endpoint_id.is_empty() && !model_key.is_empty() => {
            (Some(endpoint_id.to_owned()), model_key.to_owned())
        }
        _ => (None, value),
    }
}

fn load_rollup_prices(
    transaction: &rusqlite::Transaction<'_>,
) -> rusqlite::Result<(Option<i64>, Vec<RollupPrice>)> {
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_pricing_meta WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let mut statement = transaction.prepare(
        "SELECT model_key,effective_from,effective_to,input_per_million_micros,
                output_per_million_micros,cache_read_per_million_micros,
                cache_creation_per_million_micros
         FROM runtime_model_prices ORDER BY model_key,effective_from DESC,id DESC",
    )?;
    let prices = statement
        .query_map([], |row| {
            let (endpoint_id, model_key) = split_rollup_price_key(row.get(0)?);
            Ok(RollupPrice {
                endpoint_id,
                model_key,
                effective_from: row.get(1)?,
                effective_to: row.get(2)?,
                input_per_million_micros: row.get(3)?,
                output_per_million_micros: row.get(4)?,
                cache_read_per_million_micros: row.get(5)?,
                cache_creation_per_million_micros: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((revision, prices))
}

fn resolve_rollup_price<'a>(
    prices: &'a [RollupPrice],
    endpoint_id: Option<&str>,
    model: &str,
    timestamp: f64,
) -> Option<&'a RollupPrice> {
    let matches_timestamp = |price: &&RollupPrice| {
        price.model_key == model
            && timestamp >= price.effective_from
            && price.effective_to.is_none_or(|to| timestamp < to)
    };
    if let Some(endpoint_price) =
        endpoint_id
            .filter(|value| !value.is_empty())
            .and_then(|endpoint| {
                prices.iter().find(|price| {
                    price.endpoint_id.as_deref() == Some(endpoint) && matches_timestamp(price)
                })
            })
    {
        return Some(endpoint_price);
    }
    prices
        .iter()
        .find(|price| price.endpoint_id.is_none() && matches_timestamp(price))
}

// rollup 的一行成本由多个互不相关的维度共同决定;打包成结构体只会多一层
// 只在这里用到的类型。
#[allow(clippy::too_many_arguments)]
fn add_rollup_cost(
    accumulator: &mut HourlyRollupAccumulator,
    request_purpose: Option<&str>,
    timestamp: f64,
    endpoint_id: Option<&str>,
    effective_model: Option<&str>,
    uncached_input: Option<i64>,
    cache_read: Option<i64>,
    cache_creation: Option<i64>,
    output: Option<i64>,
    semantics: Option<&str>,
    quality: Option<&str>,
    prices: &[RollupPrice],
) {
    if request_purpose == Some("token_count") {
        return;
    }
    let accounting_known =
        matches!(semantics, Some("subset" | "independent")) && quality == Some("complete");
    let Some((uncached_input, cache_read, cache_creation, output)) = accounting_known
        .then_some((uncached_input, cache_read, cache_creation, output))
        .and_then(|values| match values {
            (Some(uncached), Some(read), Some(creation), Some(output)) => {
                Some((uncached.max(0), read.max(0), creation.max(0), output.max(0)))
            }
            _ => None,
        })
    else {
        accumulator.cost_unknown_accounting_requests = accumulator
            .cost_unknown_accounting_requests
            .saturating_add(1);
        accumulator.cost_unknown_requests = accumulator.cost_unknown_requests.saturating_add(1);
        return;
    };
    let Some(model) = effective_model else {
        accumulator.cost_unpriced_requests = accumulator.cost_unpriced_requests.saturating_add(1);
        return;
    };
    let Some(price) = resolve_rollup_price(prices, endpoint_id, model, timestamp) else {
        accumulator.cost_unpriced_requests = accumulator.cost_unpriced_requests.saturating_add(1);
        return;
    };
    let components = [
        (uncached_input, price.input_per_million_micros),
        (cache_read, price.cache_read_per_million_micros),
        (cache_creation, price.cache_creation_per_million_micros),
        (output, price.output_per_million_micros),
    ];
    let mut request_numerator = 0_i128;
    for (tokens, rate) in components {
        if tokens == 0 {
            continue;
        }
        let Some(rate) = rate else {
            accumulator.cost_unpriced_requests =
                accumulator.cost_unpriced_requests.saturating_add(1);
            return;
        };
        request_numerator =
            request_numerator.saturating_add((tokens as i128).saturating_mul(rate as i128));
    }
    accumulator.cost_numerator = accumulator.cost_numerator.saturating_add(request_numerator);
    accumulator.cost_priced_requests = accumulator.cost_priced_requests.saturating_add(1);
}

fn add_optional_token(total: &mut i64, presence: &mut i64, value: Option<i64>) {
    if let Some(value) = value {
        *total = total.saturating_add(value.max(0));
        *presence = presence.saturating_add(1);
    }
}

fn rebuild_hourly_rollup_bucket(
    connection: &mut Connection,
    bucket_start: i64,
) -> rusqlite::Result<bool> {
    let transaction = connection.transaction()?;
    let bucket_end = bucket_start.saturating_add(3_600);
    let mut accumulator = HourlyRollupAccumulator::default();
    let (price_revision, prices) = load_rollup_prices(&transaction)?;
    {
        let mut statement = transaction.prepare(
            "SELECT seq,kind,outcome,failover,duration_ms,ttfb_ms,request_purpose,usage_present,
                    input_tokens,output_tokens,cache_read_input_tokens,
                    cache_creation_input_tokens,reasoning_tokens,uncached_input_tokens,
                    processed_input_tokens,processed_total_tokens,
                    token_accounting_semantics,token_accounting_quality,endpoint_id,timestamp,effective_model
             FROM runtime_events
             WHERE projection_version=?1 AND is_in_flight=0 AND kind IN ('client','upstream')
               AND timestamp>=?2 AND timestamp<?3
             ORDER BY seq ASC",
        )?;
        let mut rows = statement.query(params![
            PROJECTION_VERSION,
            bucket_start as f64,
            bucket_end as f64
        ])?;
        while let Some(row) = rows.next()? {
            accumulator.max_seq = accumulator.max_seq.max(row.get::<_, i64>(0)?);
            let kind = row.get::<_, String>(1)?;
            if kind == KIND_UPSTREAM {
                accumulator.upstream_attempts = accumulator.upstream_attempts.saturating_add(1);
                match row.get::<_, Option<String>>(2)?.as_deref() {
                    Some("succeeded") => {
                        accumulator.upstream_successes =
                            accumulator.upstream_successes.saturating_add(1)
                    }
                    Some("failed") => {
                        accumulator.upstream_failures =
                            accumulator.upstream_failures.saturating_add(1)
                    }
                    _ => {}
                }
                continue;
            }
            accumulator.client_requests = accumulator.client_requests.saturating_add(1);
            let outcome = row.get::<_, Option<String>>(2)?;
            match outcome.as_deref() {
                Some("succeeded") => {
                    accumulator.client_successes = accumulator.client_successes.saturating_add(1)
                }
                Some("failed") => {
                    accumulator.client_failures = accumulator.client_failures.saturating_add(1)
                }
                Some("cancelled") => {
                    accumulator.client_cancelled = accumulator.client_cancelled.saturating_add(1)
                }
                _ => {
                    accumulator.client_unknown_results =
                        accumulator.client_unknown_results.saturating_add(1)
                }
            }
            let failover = row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0;
            if failover {
                accumulator.failovers = accumulator.failovers.saturating_add(1);
                if outcome.is_some() {
                    accumulator.failover_terminal_requests =
                        accumulator.failover_terminal_requests.saturating_add(1);
                    if outcome.as_deref() == Some("succeeded") {
                        accumulator.failover_recovered_requests =
                            accumulator.failover_recovered_requests.saturating_add(1);
                    }
                }
            }
            if let Some(value) = row.get::<_, Option<i64>>(4)?.filter(|value| *value >= 0) {
                accumulator.duration_ms_sum = accumulator.duration_ms_sum.saturating_add(value);
                accumulator.duration_count = accumulator.duration_count.saturating_add(1);
                if value > DURATION_SLOW_MS {
                    accumulator.duration_slow_count =
                        accumulator.duration_slow_count.saturating_add(1);
                }
                if value > DURATION_CRITICAL_MS {
                    accumulator.duration_critical_count =
                        accumulator.duration_critical_count.saturating_add(1);
                }
            }
            if let Some(value) = row.get::<_, Option<i64>>(5)?.filter(|value| *value >= 0) {
                accumulator.ttfb_ms_sum = accumulator.ttfb_ms_sum.saturating_add(value);
                accumulator.ttfb_count = accumulator.ttfb_count.saturating_add(1);
                if value > TTFB_SLOW_MS {
                    accumulator.ttfb_slow_count = accumulator.ttfb_slow_count.saturating_add(1);
                }
                if value > TTFB_CRITICAL_MS {
                    accumulator.ttfb_critical_count =
                        accumulator.ttfb_critical_count.saturating_add(1);
                }
            }
            let request_purpose = row.get::<_, Option<String>>(6)?;
            let usage_present = row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0;
            let input = row.get::<_, Option<i64>>(8)?;
            let output = row.get::<_, Option<i64>>(9)?;
            let cache_read = row.get::<_, Option<i64>>(10)?;
            let cache_creation = row.get::<_, Option<i64>>(11)?;
            let reasoning = row.get::<_, Option<i64>>(12)?;
            let uncached = row.get::<_, Option<i64>>(13)?;
            let processed_input = row.get::<_, Option<i64>>(14)?;
            let processed_total = row.get::<_, Option<i64>>(15)?;
            let semantics = row.get::<_, Option<String>>(16)?;
            let quality = row.get::<_, Option<String>>(17)?;
            let endpoint_id = row.get::<_, Option<String>>(18)?;
            let event_timestamp = row.get::<_, f64>(19)?;
            let effective_model = row.get::<_, Option<String>>(20)?;
            add_rollup_cost(
                &mut accumulator,
                request_purpose.as_deref(),
                event_timestamp,
                endpoint_id.as_deref(),
                effective_model.as_deref(),
                uncached,
                cache_read,
                cache_creation,
                output,
                semantics.as_deref(),
                quality.as_deref(),
                &prices,
            );
            if request_purpose.as_deref() == Some("token_count") || !usage_present {
                continue;
            }
            accumulator.usage_present_requests =
                accumulator.usage_present_requests.saturating_add(1);
            add_optional_token(
                &mut accumulator.input_tokens,
                &mut accumulator.input_tokens_present,
                input,
            );
            add_optional_token(
                &mut accumulator.output_tokens,
                &mut accumulator.output_tokens_present,
                output,
            );
            add_optional_token(
                &mut accumulator.cache_read_input_tokens,
                &mut accumulator.cache_read_input_tokens_present,
                cache_read,
            );
            add_optional_token(
                &mut accumulator.cache_creation_input_tokens,
                &mut accumulator.cache_creation_input_tokens_present,
                cache_creation,
            );
            add_optional_token(
                &mut accumulator.reasoning_tokens,
                &mut accumulator.reasoning_tokens_present,
                reasoning,
            );
            if let Some(value) = uncached {
                accumulator.uncached_input_tokens = accumulator
                    .uncached_input_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = processed_input {
                accumulator.processed_input_tokens = accumulator
                    .processed_input_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = processed_total {
                accumulator.processed_total_tokens = accumulator
                    .processed_total_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = cache_read {
                accumulator.cache_read_reported_requests =
                    accumulator.cache_read_reported_requests.saturating_add(1);
                if value > 0 {
                    accumulator.cache_read_hit_requests =
                        accumulator.cache_read_hit_requests.saturating_add(1);
                }
            }
            let accounting_known = matches!(semantics.as_deref(), Some("subset" | "independent"));
            if accounting_known {
                accumulator.accounting_known_requests =
                    accumulator.accounting_known_requests.saturating_add(1);
            } else {
                accumulator.accounting_unknown_requests =
                    accumulator.accounting_unknown_requests.saturating_add(1);
            }
            if accounting_known {
                if let (Some(cache_read), Some(processed_input)) = (cache_read, processed_input) {
                    accumulator.cache_eligible_requests =
                        accumulator.cache_eligible_requests.saturating_add(1);
                    accumulator.cache_read_token_numerator = accumulator
                        .cache_read_token_numerator
                        .saturating_add(cache_read.max(0));
                    accumulator.cache_read_token_denominator = accumulator
                        .cache_read_token_denominator
                        .saturating_add(processed_input.max(0));
                } else {
                    accumulator.cache_unknown_requests =
                        accumulator.cache_unknown_requests.saturating_add(1);
                }
            } else {
                accumulator.cache_unknown_requests =
                    accumulator.cache_unknown_requests.saturating_add(1);
            }
            let cost_complete = accounting_known
                && quality.as_deref() == Some("complete")
                && uncached.is_some()
                && cache_read.is_some()
                && cache_creation.is_some()
                && output.is_some();
            if cost_complete {
                accumulator.cost_accounting_complete_requests = accumulator
                    .cost_accounting_complete_requests
                    .saturating_add(1);
            } else {
                accumulator.cost_unknown_accounting_requests = accumulator
                    .cost_unknown_accounting_requests
                    .saturating_add(1);
            }
        }
    }
    if accumulator.client_requests == 0 {
        transaction.execute(
            "DELETE FROM runtime_hourly_rollups WHERE bucket_start=?1",
            params![bucket_start],
        )?;
    } else {
        transaction.execute(
            "INSERT OR REPLACE INTO runtime_hourly_rollups(
                bucket_start,bucket_end,max_seq,client_requests,client_successes,
                client_failures,client_cancelled,client_unknown_results,failovers,
                failover_terminal_requests,failover_recovered_requests,upstream_attempts,
                upstream_successes,upstream_failures,duration_ms_sum,duration_count,
                duration_slow_count,duration_critical_count,ttfb_ms_sum,ttfb_count,
                ttfb_slow_count,ttfb_critical_count,input_tokens,output_tokens,
                cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,
                uncached_input_tokens,processed_input_tokens,processed_total_tokens,
                usage_present_requests,accounting_known_requests,accounting_unknown_requests,
                cache_read_reported_requests,cache_read_hit_requests,cache_eligible_requests,
                cache_unknown_requests,cache_read_token_numerator,cache_read_token_denominator,
                input_tokens_present,output_tokens_present,cache_read_input_tokens_present,
                cache_creation_input_tokens_present,reasoning_tokens_present,
                cost_accounting_complete_requests,cost_unknown_accounting_requests,
                cost_numerator,cost_priced_requests,cost_unpriced_requests,cost_unknown_requests,
                cost_price_revision,updated_at
             ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
                ?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,
                ?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,
                ?48,?49,?50,?51,?52)",
            params![
                bucket_start,
                bucket_end,
                accumulator.max_seq,
                accumulator.client_requests,
                accumulator.client_successes,
                accumulator.client_failures,
                accumulator.client_cancelled,
                accumulator.client_unknown_results,
                accumulator.failovers,
                accumulator.failover_terminal_requests,
                accumulator.failover_recovered_requests,
                accumulator.upstream_attempts,
                accumulator.upstream_successes,
                accumulator.upstream_failures,
                accumulator.duration_ms_sum,
                accumulator.duration_count,
                accumulator.duration_slow_count,
                accumulator.duration_critical_count,
                accumulator.ttfb_ms_sum,
                accumulator.ttfb_count,
                accumulator.ttfb_slow_count,
                accumulator.ttfb_critical_count,
                accumulator.input_tokens,
                accumulator.output_tokens,
                accumulator.cache_read_input_tokens,
                accumulator.cache_creation_input_tokens,
                accumulator.reasoning_tokens,
                accumulator.uncached_input_tokens,
                accumulator.processed_input_tokens,
                accumulator.processed_total_tokens,
                accumulator.usage_present_requests,
                accumulator.accounting_known_requests,
                accumulator.accounting_unknown_requests,
                accumulator.cache_read_reported_requests,
                accumulator.cache_read_hit_requests,
                accumulator.cache_eligible_requests,
                accumulator.cache_unknown_requests,
                accumulator.cache_read_token_numerator,
                accumulator.cache_read_token_denominator,
                accumulator.input_tokens_present,
                accumulator.output_tokens_present,
                accumulator.cache_read_input_tokens_present,
                accumulator.cache_creation_input_tokens_present,
                accumulator.reasoning_tokens_present,
                accumulator.cost_accounting_complete_requests,
                accumulator.cost_unknown_accounting_requests,
                accumulator
                    .cost_numerator
                    .clamp(i64::MIN as i128, i64::MAX as i128) as i64,
                accumulator.cost_priced_requests,
                accumulator.cost_unpriced_requests,
                accumulator.cost_unknown_requests,
                price_revision,
                now(),
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM runtime_hourly_rollup_dirty WHERE bucket_start=?1",
        params![bucket_start],
    )?;
    let dirty_exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !dirty_exists && meta_i64(&transaction, "projection_backfill_complete")?.unwrap_or(0) == 1 {
        let max_seq = transaction.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM runtime_events WHERE is_in_flight=0",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        set_meta(&transaction, "hourly_rollup_max_seq", max_seq)?;
        set_meta(
            &transaction,
            "hourly_rollup_history_generation",
            meta_i64(&transaction, "history_generation")?.unwrap_or(0),
        )?;
        set_meta(&transaction, "hourly_rollup_complete", 1)?;
    }
    transaction.commit()?;
    Ok(dirty_exists)
}

fn projection_maintenance_needed(connection: &Connection) -> rusqlite::Result<bool> {
    let rollup_dirty = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(
        meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) == 0
            || meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) == 0
            || meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) == 0
            || rollup_dirty,
    )
}

fn backfill_projection_batch(connection: &mut Connection) -> rusqlite::Result<bool> {
    let transaction = connection.transaction()?;
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT seq,payload_json FROM runtime_events
             WHERE projection_version>=0 AND projection_version<>?1
             ORDER BY seq ASC LIMIT ?2",
        )?;
        statement
            .query_map(
                params![PROJECTION_VERSION, PROJECTION_BACKFILL_BATCH as i64],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    if rows.is_empty() {
        set_meta(&transaction, "projection_backfill_complete", 1)?;
        transaction.commit()?;
        return Ok(false);
    }
    let mut cursor = 0;
    let mut failed = 0i64;
    for (seq, payload) in &rows {
        cursor = cursor.max(*seq);
        match serde_json::from_str::<RuntimeEvent>(payload) {
            Ok(event) => {
                update_event_projection(&transaction, *seq, &event, payload)?;
                mark_event_hourly_rollup_dirty(&transaction, &event)?;
            }
            Err(_) => {
                failed += 1;
                transaction.execute(
                    "UPDATE runtime_events SET projection_version=-1,payload_bytes=?2 WHERE seq=?1",
                    params![seq, payload.len().min(i64::MAX as usize) as i64],
                )?;
            }
        }
    }
    set_meta(&transaction, "projection_backfill_cursor", cursor)?;
    if failed > 0 {
        let total = meta_i64(&transaction, "projection_backfill_failed")?
            .unwrap_or(0)
            .saturating_add(failed);
        set_meta(&transaction, "projection_backfill_failed", total)?;
    }
    let more = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events
                       WHERE projection_version>=0 AND projection_version<>?1 LIMIT 1)",
        params![PROJECTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !more {
        set_meta(&transaction, "projection_backfill_complete", 1)?;
    }
    transaction.commit()?;
    Ok(more)
}

fn advance_projection_indexes(connection: &Connection) -> rusqlite::Result<bool> {
    let indexes = [
        (
            "runtime_events_inflight_change_v2",
            "CREATE INDEX runtime_events_inflight_change_v2 ON runtime_events(change_seq) WHERE is_in_flight=1",
        ),
        (
            "runtime_events_outcome_seq_v2",
            "CREATE INDEX runtime_events_outcome_seq_v2 ON runtime_events(outcome,seq DESC) WHERE is_in_flight=0",
        ),
        (
            "runtime_events_time_seq_v2",
            "CREATE INDEX runtime_events_time_seq_v2 ON runtime_events(timestamp DESC,seq DESC) WHERE is_in_flight=0",
        ),
        (
            "runtime_events_session_time_v2",
            "CREATE INDEX runtime_events_session_time_v2 ON runtime_events(session_key,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND session_key IS NOT NULL",
        ),
        (
            "runtime_events_project_time_v2",
            "CREATE INDEX runtime_events_project_time_v2 ON runtime_events(project_id,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND project_id IS NOT NULL",
        ),
        (
            "runtime_events_endpoint_time_v2",
            "CREATE INDEX runtime_events_endpoint_time_v2 ON runtime_events(endpoint_id,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND endpoint_id IS NOT NULL",
        ),
        (
            "runtime_events_model_time_v2",
            "CREATE INDEX runtime_events_model_time_v2 ON runtime_events(effective_model,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND effective_model IS NOT NULL",
        ),
        (
            "runtime_events_failure_time_v2",
            "CREATE INDEX runtime_events_failure_time_v2 ON runtime_events(failure_kind,failure_phase,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND outcome='failed'",
        ),
    ];
    for (name, sql) in indexes {
        let exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
            params![name],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            connection.execute(sql, [])?;
            return Ok(true);
        }
    }
    connection.execute_batch(
        "DROP INDEX IF EXISTS runtime_events_seq;
         DROP INDEX IF EXISTS runtime_events_change_seq;
         PRAGMA optimize;",
    )?;
    set_meta(connection, "projection_indexes_ready", 1)?;
    Ok(false)
}

fn run_projection_maintenance(connection: &mut Connection) -> rusqlite::Result<bool> {
    if meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) == 0 {
        let more = backfill_projection_batch(connection)?;
        if more {
            return Ok(true);
        }
    }
    if meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) == 0 {
        let more = advance_projection_indexes(connection)?;
        if more {
            return Ok(true);
        }
    }
    let dirty_bucket = connection
        .query_row(
            "SELECT bucket_start FROM runtime_hourly_rollup_dirty ORDER BY bucket_start ASC LIMIT 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(bucket_start) = dirty_bucket {
        return rebuild_hourly_rollup_bucket(connection, bucket_start);
    }
    if meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) == 0 {
        let max_seq = connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM runtime_events WHERE is_in_flight=0",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        set_meta(connection, "hourly_rollup_max_seq", max_seq)?;
        set_meta(
            connection,
            "hourly_rollup_history_generation",
            meta_i64(connection, "history_generation")?.unwrap_or(0),
        )?;
        set_meta(connection, "hourly_rollup_complete", 1)?;
    }
    Ok(false)
}

fn check_existing_schema(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    let has_meta = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='runtime_meta'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    if !has_meta {
        return Ok(());
    }
    let version = meta_i64(&connection, "schema_version")
        .map_err(|error| error.to_string())?
        .unwrap_or(0);
    if version > SCHEMA_VERSION {
        return Err(format!(
            "runtime schema {version} is newer than supported {SCHEMA_VERSION}"
        ));
    }
    Ok(())
}

fn harden_database_file(path: &Path) {
    #[cfg(unix)]
    if let Ok(metadata) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

/// Resolve crash leftovers before exposing the first snapshot. A request that
/// never received headers is not useful history. A request that did receive
/// headers is conservatively finalized as failed (or cancelled for 499): a
/// daemon restart means no terminal success was observed, even when HTTP 2xx
/// headers had already arrived.
fn normalize_startup(connection: &mut Connection) -> rusqlite::Result<Vec<RuntimeChange>> {
    let transaction = connection.transaction()?;
    let stored_next_change_seq = meta_i64(&transaction, "next_change_seq")?.unwrap_or(1);
    let max_change_seq = transaction.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let mut next_change_seq = stored_next_change_seq.max(max_change_seq + 1).max(1);
    let mut retained_event_count = meta_i64(&transaction, "retained_event_count")?.unwrap_or(0);
    let mut normalized_changes = Vec::new();
    let mut statement = transaction.prepare(
        "SELECT seq,event_id,status_code,is_in_flight,payload_json
         FROM runtime_events WHERE is_in_flight = 1 ORDER BY change_seq ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (seq, event_id, status_code, is_in_flight, payload) in rows {
        if is_in_flight != 0 && status_code == 0 {
            transaction.execute(
                "DELETE FROM runtime_events WHERE event_id=?1",
                params![event_id],
            )?;
            retained_event_count = retained_event_count.saturating_sub(1);
            continue;
        }
        let mut event: RuntimeEvent = serde_json::from_str(&payload).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        event.phase = Some(RuntimeEventPhase::Completed);
        if status_code == STATUS_CLIENT_DISCONNECTED {
            event.outcome = Some(RuntimeEventOutcome::Cancelled);
            event
                .failure_kind
                .get_or_insert(RuntimeFailureKind::ClientCancelled);
        } else {
            event.outcome = Some(RuntimeEventOutcome::Failed);
            if (200..=399).contains(&status_code) {
                event
                    .failure_kind
                    .get_or_insert(RuntimeFailureKind::StreamInterrupted);
                event
                    .failure_phase
                    .get_or_insert(RuntimeFailurePhase::ResponseStream);
            }
        }
        let outcome = option_token(event.outcome);
        let failure_kind = option_token(event.failure_kind);
        let payload = serde_json::to_string(&event)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        transaction.execute(
            "UPDATE runtime_events SET change_seq=?2,is_in_flight=0,phase='completed',
             outcome=?3,failure_kind=?4,payload_json=?5,updated_at=?6 WHERE event_id=?1",
            params![
                event_id,
                next_change_seq,
                outcome,
                failure_kind,
                payload,
                now()
            ],
        )?;
        update_event_projection(&transaction, seq, &event, &payload)?;
        mark_event_hourly_rollup_dirty(&transaction, &event)?;
        normalized_changes.push(RuntimeChange {
            seq,
            change_seq: next_change_seq,
            event: event.clone(),
        });
        match event.kind.as_str() {
            KIND_CLIENT => {
                transaction.execute(
                    "UPDATE runtime_counters SET client_requests=client_requests+1,
                     client_failures=client_failures+?1,failovers=failovers+?2 WHERE id=1",
                    params![i64::from(event.is_failed()), i64::from(event.failover)],
                )?;
            }
            KIND_UPSTREAM => {
                transaction.execute(
                    "UPDATE runtime_counters SET upstream_attempts=upstream_attempts+1,
                     upstream_failures=upstream_failures+?1 WHERE id=1",
                    params![i64::from(event.is_failed())],
                )?;
            }
            _ => {}
        }
        next_change_seq += 1;
    }
    set_meta(&transaction, "next_change_seq", next_change_seq)?;
    set_meta(&transaction, "retained_event_count", retained_event_count)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER)
         FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    transaction.commit()?;
    Ok(normalized_changes)
}

fn meta_i64(connection: &Connection, key: &str) -> rusqlite::Result<Option<i64>> {
    let value = connection
        .query_row(
            "SELECT value FROM runtime_meta WHERE key=?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value.map(|value| value.parse::<i64>().unwrap_or_default()))
}

fn set_meta(connection: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO runtime_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value.to_string()],
    )?;
    Ok(())
}

fn set_meta_max(connection: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    let current = meta_i64(connection, key)?.unwrap_or(value);
    set_meta(connection, key, current.max(value))
}

fn set_storage_limit_meta(connection: &Connection, value: Option<i64>) -> rusqlite::Result<()> {
    match value {
        Some(value) => set_meta(connection, "storage_limit_bytes", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='storage_limit_bytes'",
                [],
            )?;
            Ok(())
        }
    }
}

fn set_retention_max_age_meta(connection: &Connection, value: Option<i64>) -> rusqlite::Result<()> {
    match value {
        Some(value) => set_meta(connection, "retention_max_age_days", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='retention_max_age_days'",
                [],
            )?;
            Ok(())
        }
    }
}

fn load_cached_storage(connection: &Connection) -> rusqlite::Result<CachedStorageMetrics> {
    let (
        event_count,
        completed_event_count,
        in_flight_event_count,
        oldest_event_at,
        newest_event_at,
        payload_bytes,
    ) = connection.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),
                MIN(timestamp),MAX(timestamp),COALESCE(SUM(payload_bytes),0)
         FROM runtime_events",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;
    let page_size = connection.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
    let page_count = connection.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?;
    let rollup_dirty_buckets = connection.query_row(
        "SELECT COUNT(*) FROM runtime_hourly_rollup_dirty",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let page_size = page_size.max(0) as u64;
    let page_count = page_count.max(0) as u64;
    let freelist_count = freelist_count.max(0) as u64;
    Ok(CachedStorageMetrics {
        event_count,
        completed_event_count,
        in_flight_event_count,
        oldest_event_at,
        newest_event_at,
        retained_from_seq: meta_i64(connection, "retained_from_seq")?.unwrap_or(1),
        payload_bytes: payload_bytes.max(0) as u64,
        live_bytes: page_count
            .saturating_sub(freelist_count)
            .saturating_mul(page_size),
        allocated_bytes: page_count.saturating_mul(page_size),
        schema_version: meta_i64(connection, "schema_version")?.unwrap_or(0),
        backfill_cursor: meta_i64(connection, "projection_backfill_cursor")?.unwrap_or(0),
        backfill_complete: meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) != 0,
        backfill_failed: meta_i64(connection, "projection_backfill_failed")?.unwrap_or(0),
        indexes_ready: meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) != 0,
        rollup_complete: meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) != 0,
        rollup_max_seq: meta_i64(connection, "hourly_rollup_max_seq")?.unwrap_or(0),
        rollup_history_generation: meta_i64(connection, "hourly_rollup_history_generation")?
            .unwrap_or(0),
        rollup_failed: meta_i64(connection, "hourly_rollup_failed")?.unwrap_or(0),
        rollup_dirty_buckets,
        user_deleted_events: meta_i64(connection, "user_deleted_events")?.unwrap_or(0),
        user_deleted_requests: meta_i64(connection, "user_deleted_requests")?.unwrap_or(0),
    })
}

fn load_state(connection: &Connection) -> rusqlite::Result<StoreState> {
    let counters = connection.query_row(
        "SELECT client_requests,client_successes,client_failures,upstream_attempts,
                upstream_successes,upstream_failures,failovers FROM runtime_counters WHERE id=1",
        [],
        |row| {
            Ok(RuntimeCounters {
                client_requests: row.get(0)?,
                client_successes: row.get(1)?,
                client_failures: row.get(2)?,
                upstream_attempts: row.get(3)?,
                upstream_successes: row.get(4)?,
                upstream_failures: row.get(5)?,
                failovers: row.get(6)?,
            })
        },
    )?;
    let max_seq = connection.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let max_change_seq = connection.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let next_seq = meta_i64(connection, "next_seq")?
        .unwrap_or(1)
        .max(max_seq + 1)
        .max(1);
    let next_change_seq = meta_i64(connection, "next_change_seq")?
        .unwrap_or(1)
        .max(max_change_seq + 1)
        .max(1);
    let reset_generation = meta_i64(connection, "reset_generation")?.unwrap_or(0);
    let history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0);
    let latest_event = connection
        .query_row(
            "SELECT payload_json FROM runtime_events ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|payload| serde_json::from_str(&payload))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(StoreState {
        next_seq,
        next_change_seq,
        reset_generation,
        history_generation,
        counters,
        active_sequences: HashMap::new(),
        recent_changes: VecDeque::new(),
        latest_event,
        last_commit_at: None,
        last_error: None,
        storage: load_cached_storage(connection)?,
        storage_refreshed_at: std::time::Instant::now(),
    })
}

fn update_event_projection(
    connection: &Connection,
    seq: i64,
    event: &RuntimeEvent,
    payload: &str,
) -> rusqlite::Result<()> {
    let projection = EventProjection::from_event(event, payload.len());
    connection.execute(
        "UPDATE runtime_events SET
            projection_version=?2,payload_bytes=?3,session_key=?4,session_source=?5,
            project_id=?6,project_name=?7,project_source=?8,workspace_paths_json=?9,
            endpoint_name=?10,feature_rule_id=?11,client_model=?12,
            effective_model=?13,upstream_model=?14,failure_phase=?15,source_format=?16,
            target_format=?17,route_mode=?18,upstream_status_code=?19,duration_ms=?20,
            ttfb_ms=?21,failover=?22,stream_terminal=?23,codex_metadata_present=?24,
            usage_present=?25,input_tokens=?26,output_tokens=?27,
            cache_read_input_tokens=?28,cache_creation_input_tokens=?29,
            reasoning_tokens=?30,uncached_input_tokens=?31,processed_input_tokens=?32,
            processed_total_tokens=?33,token_accounting_semantics=?34,
            token_accounting_quality=?35,tool_calls_json=?36
            ,codex_thread_class=?37,attribution_scope=?38
         WHERE seq=?1",
        params![
            seq,
            PROJECTION_VERSION,
            projection.payload_bytes,
            projection.session_key,
            projection.session_source,
            projection.project_id,
            projection.project_name,
            projection.project_source,
            projection.workspace_paths_json,
            projection.endpoint_name,
            projection.feature_rule_id,
            projection.client_model,
            projection.effective_model,
            projection.upstream_model,
            projection.failure_phase,
            projection.source_format,
            projection.target_format,
            projection.route_mode,
            projection.upstream_status_code,
            projection.duration_ms,
            projection.ttfb_ms,
            projection.failover,
            projection.stream_terminal,
            projection.codex_metadata_present,
            projection.usage_present,
            projection.input_tokens,
            projection.output_tokens,
            projection.cache_read_input_tokens,
            projection.cache_creation_input_tokens,
            projection.reasoning_tokens,
            projection.uncached_input_tokens,
            projection.processed_input_tokens,
            projection.processed_total_tokens,
            projection.token_accounting_semantics,
            projection.token_accounting_quality,
            projection.tool_calls_json,
            projection.codex_thread_class,
            projection.attribution_scope,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Default)]
struct StorageRotation {
    deleted_event_ids: Vec<String>,
}

fn sqlite_live_bytes(connection: &Connection) -> rusqlite::Result<u64> {
    let page_size = connection.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
    let page_count = connection.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?;
    Ok((page_count.max(0) as u64)
        .saturating_sub(freelist_count.max(0) as u64)
        .saturating_mul(page_size.max(0) as u64))
}

/// Return the current event-time cutoff for the rolling retention window.
/// Runtime events use Apple reference-date seconds, while metadata timestamps
/// use Unix seconds; keeping this conversion at the storage boundary avoids
/// local-time and daylight-saving surprises.
fn retention_age_cutoff(connection: &Connection) -> rusqlite::Result<Option<f64>> {
    let Some(days) = meta_i64(connection, "retention_max_age_days")? else {
        return Ok(None);
    };
    if days < 1 {
        return Ok(None);
    }
    Ok(Some(
        now() - APPLE_EPOCH_OFFSET_SECS - (days as f64 * 86_400.0),
    ))
}

fn has_expired_completed_event(
    connection: &Connection,
    cutoff: Option<f64>,
) -> rusqlite::Result<bool> {
    let Some(cutoff) = cutoff else {
        return Ok(false);
    };
    connection.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM runtime_events AS candidate
             WHERE candidate.is_in_flight=0
               AND candidate.timestamp < ?1
               AND (
                   candidate.request_id IS NULL
                   OR trim(candidate.request_id)=''
                   OR NOT EXISTS(
                       SELECT 1 FROM runtime_events AS newer
                       WHERE newer.request_id=candidate.request_id
                         AND newer.timestamp >= ?1
                   )
               )
               AND (
                   candidate.request_id IS NULL
                   OR trim(candidate.request_id)=''
                   OR NOT EXISTS(
                       SELECT 1 FROM runtime_events AS active
                       WHERE active.request_id=candidate.request_id
                         AND active.is_in_flight=1
                   )
               )
             LIMIT 1
         )",
        params![cutoff],
        |row| row.get::<_, bool>(0),
    )
}

/// Keep the SQLite live page footprint under the configured limit and/or the
/// rolling age cutoff. Both dimensions are OR-ed: whichever condition is
/// reached first may remove the oldest eligible completed request group.
///
/// A request group is indivisible for rotation. If any row in the group is
/// still in flight, the entire group is protected; this prevents a partial
/// request chain from appearing in analytics or exports.
fn rotate_retention(connection: &Connection) -> rusqlite::Result<StorageRotation> {
    let capacity_limit = meta_i64(connection, "storage_limit_bytes")?
        .filter(|value| *value > 0)
        .map(|value| value as u64);
    let age_cutoff = retention_age_cutoff(connection)?;
    if capacity_limit.is_none() && age_cutoff.is_none() {
        return Ok(StorageRotation::default());
    }

    let mut rotation = StorageRotation::default();
    loop {
        let over_capacity = match capacity_limit {
            Some(limit) => sqlite_live_bytes(connection)? > limit,
            None => false,
        };
        let over_age = has_expired_completed_event(connection, age_cutoff)?;
        if !over_capacity && !over_age {
            break;
        }

        // When capacity is exceeded it is valid to evict the oldest eligible
        // group even if that group's events are newer than the age cutoff.
        // When only age is exceeded, require the whole group to be older than
        // the cutoff so no fresh row is removed as a side effect.
        let candidate = connection
            .query_row(
                "SELECT event_id,request_id,kind,timestamp
                 FROM runtime_events AS candidate
                 WHERE candidate.is_in_flight=0
                   AND (
                       candidate.request_id IS NULL
                       OR trim(candidate.request_id)=''
                       OR NOT EXISTS(
                           SELECT 1 FROM runtime_events AS active
                           WHERE active.request_id=candidate.request_id
                             AND active.is_in_flight=1
                       )
                   )
                   AND (
                       ?1=1
                       OR (
                           candidate.timestamp < ?2
                           AND (
                               candidate.request_id IS NULL
                               OR trim(candidate.request_id)=''
                               OR NOT EXISTS(
                                   SELECT 1 FROM runtime_events AS newer
                                   WHERE newer.request_id=candidate.request_id
                                     AND newer.timestamp >= ?2
                               )
                           )
                       )
                   )
                 ORDER BY candidate.timestamp ASC,candidate.seq ASC
                 LIMIT 1",
                params![i64::from(over_capacity), age_cutoff.unwrap_or(0.0)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((event_id, request_id, kind, timestamp)) = candidate else {
            // This is the expected best-effort outcome when every old group
            // still contains an in-flight row, or only SQLite's fixed schema
            // footprint is larger than the requested capacity.
            break;
        };

        let mut event_ids = Vec::new();
        if let Some(request_id) = request_id.filter(|value| !value.trim().is_empty()) {
            mark_request_hourly_rollups_dirty(connection, &request_id)?;
            let mut statement = connection.prepare(
                "SELECT event_id FROM runtime_events
                 WHERE request_id=?1 AND is_in_flight=0
                 ORDER BY seq ASC",
            )?;
            event_ids = statement
                .query_map(params![request_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
        } else {
            if kind == KIND_CLIENT {
                mark_hourly_rollup_bucket(connection, hourly_bucket_start(timestamp))?;
            }
            event_ids.push(event_id);
        }
        if event_ids.is_empty() {
            break;
        }
        for event_id in &event_ids {
            connection.execute(
                "DELETE FROM runtime_events WHERE event_id=?1 AND is_in_flight=0",
                params![event_id],
            )?;
        }
        rotation.deleted_event_ids.extend(event_ids);
    }

    if rotation.deleted_event_ids.is_empty() {
        return Ok(rotation);
    }
    let counters = counters_from_connection(connection)?;
    connection.execute(
        "UPDATE runtime_counters SET client_requests=?1,client_successes=?2,
         client_failures=?3,upstream_attempts=?4,upstream_successes=?5,
         upstream_failures=?6,failovers=?7 WHERE id=1",
        params![
            counters.client_requests,
            counters.client_successes,
            counters.client_failures,
            counters.upstream_attempts,
            counters.upstream_successes,
            counters.upstream_failures,
            counters.failovers,
        ],
    )?;
    let history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0) + 1;
    set_meta(connection, "history_generation", history_generation)?;
    set_meta(
        connection,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    let retained_event_count =
        connection.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
    set_meta(connection, "retained_event_count", retained_event_count)?;
    let retained_from_seq = connection.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER)
         FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(connection, "retained_from_seq", retained_from_seq)?;
    Ok(rotation)
}

fn rotate_retention_now(connection: &mut Connection) -> rusqlite::Result<StorageRotation> {
    let transaction = connection.transaction()?;
    let rotation = rotate_retention(&transaction)?;
    transaction.commit()?;
    Ok(rotation)
}

fn write_batch(
    connection: &mut Connection,
    batch: &[WriteMessage],
) -> rusqlite::Result<StorageRotation> {
    if batch.is_empty() {
        return Ok(StorageRotation::default());
    }
    let transaction = connection.transaction()?;
    let mut retained_event_count = meta_i64(&transaction, "retained_event_count")?.unwrap_or(0);
    for message in batch {
        let previous = transaction
            .query_row(
                "SELECT kind,is_in_flight,timestamp FROM runtime_events WHERE event_id=?1",
                params![message.event.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((kind, is_in_flight, timestamp)) = previous.as_ref()
            && kind == KIND_CLIENT
            && *is_in_flight == 0
        {
            mark_hourly_rollup_bucket(&transaction, hourly_bucket_start(*timestamp))?;
        }
        let payload = serde_json::to_string(&message.event)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let projection = EventProjection::from_event(&message.event, payload.len());
        let created_at = now();
        transaction.execute(
            "INSERT INTO runtime_events(
                seq,change_seq,event_id,request_id,timestamp,kind,phase,outcome,status_code,
                client_kind,request_purpose,endpoint_id,failure_kind,is_in_flight,payload_json,
                created_at,updated_at,projection_version,payload_bytes,session_key,session_source,
                project_id,project_name,project_source,workspace_paths_json,endpoint_name,
                feature_rule_id,client_model,effective_model,upstream_model,failure_phase,
                source_format,target_format,route_mode,upstream_status_code,duration_ms,ttfb_ms,
                failover,stream_terminal,codex_metadata_present,usage_present,input_tokens,
                output_tokens,cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,
                uncached_input_tokens,processed_input_tokens,processed_total_tokens,
                token_accounting_semantics,token_accounting_quality,tool_calls_json
                ,codex_thread_class,attribution_scope
             ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?16,
                ?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,
                ?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,
                ?47,?48,?49,?50,?51,?52,?53
             )
             ON CONFLICT(event_id) DO UPDATE SET
                change_seq=excluded.change_seq,payload_json=excluded.payload_json,
                request_id=excluded.request_id,timestamp=excluded.timestamp,kind=excluded.kind,
                phase=excluded.phase,outcome=excluded.outcome,status_code=excluded.status_code,
                client_kind=excluded.client_kind,request_purpose=excluded.request_purpose,
                endpoint_id=excluded.endpoint_id,failure_kind=excluded.failure_kind,
                is_in_flight=excluded.is_in_flight,updated_at=excluded.updated_at,
                projection_version=excluded.projection_version,payload_bytes=excluded.payload_bytes,
                session_key=excluded.session_key,session_source=excluded.session_source,
                project_id=excluded.project_id,project_name=excluded.project_name,
                project_source=excluded.project_source,
                workspace_paths_json=excluded.workspace_paths_json,
                endpoint_name=excluded.endpoint_name,
                feature_rule_id=excluded.feature_rule_id,client_model=excluded.client_model,
                effective_model=excluded.effective_model,upstream_model=excluded.upstream_model,
                failure_phase=excluded.failure_phase,source_format=excluded.source_format,
                target_format=excluded.target_format,route_mode=excluded.route_mode,
                upstream_status_code=excluded.upstream_status_code,duration_ms=excluded.duration_ms,
                ttfb_ms=excluded.ttfb_ms,failover=excluded.failover,
                stream_terminal=excluded.stream_terminal,
                codex_metadata_present=excluded.codex_metadata_present,
                usage_present=excluded.usage_present,input_tokens=excluded.input_tokens,
                output_tokens=excluded.output_tokens,
                cache_read_input_tokens=excluded.cache_read_input_tokens,
                cache_creation_input_tokens=excluded.cache_creation_input_tokens,
                reasoning_tokens=excluded.reasoning_tokens,
                uncached_input_tokens=excluded.uncached_input_tokens,
                processed_input_tokens=excluded.processed_input_tokens,
                processed_total_tokens=excluded.processed_total_tokens,
                token_accounting_semantics=excluded.token_accounting_semantics,
                token_accounting_quality=excluded.token_accounting_quality,
                tool_calls_json=excluded.tool_calls_json,
                codex_thread_class=excluded.codex_thread_class,
                attribution_scope=excluded.attribution_scope",
            params![
                message.seq,
                message.change_seq,
                message.event.id,
                message.event.request_id,
                message.event.timestamp,
                message.event.kind,
                option_token(message.event.phase),
                option_token(message.event.outcome),
                message.event.status_code,
                option_token(message.event.client_kind),
                option_token(message.event.request_purpose),
                message.event.endpoint_id,
                option_token(message.event.failure_kind),
                i64::from(message.event.is_in_flight()),
                payload,
                created_at,
                PROJECTION_VERSION,
                projection.payload_bytes,
                projection.session_key,
                projection.session_source,
                projection.project_id,
                projection.project_name,
                projection.project_source,
                projection.workspace_paths_json,
                projection.endpoint_name,
                projection.feature_rule_id,
                projection.client_model,
                projection.effective_model,
                projection.upstream_model,
                projection.failure_phase,
                projection.source_format,
                projection.target_format,
                projection.route_mode,
                projection.upstream_status_code,
                projection.duration_ms,
                projection.ttfb_ms,
                projection.failover,
                projection.stream_terminal,
                projection.codex_metadata_present,
                projection.usage_present,
                projection.input_tokens,
                projection.output_tokens,
                projection.cache_read_input_tokens,
                projection.cache_creation_input_tokens,
                projection.reasoning_tokens,
                projection.uncached_input_tokens,
                projection.processed_input_tokens,
                projection.processed_total_tokens,
                projection.token_accounting_semantics,
                projection.token_accounting_quality,
                projection.tool_calls_json,
                projection.codex_thread_class,
                projection.attribution_scope,
            ],
        )?;
        mark_event_hourly_rollup_dirty(&transaction, &message.event)?;
        if previous.is_none() {
            retained_event_count = retained_event_count.saturating_add(1);
        }
    }
    let latest = batch
        .iter()
        .max_by_key(|message| message.change_seq)
        .expect("non-empty batch");
    transaction.execute(
        "UPDATE runtime_counters SET client_requests=?2,client_successes=?3,
         client_failures=?4,upstream_attempts=?5,upstream_successes=?6,
         upstream_failures=?7,failovers=?8 WHERE id=1",
        params![
            1,
            latest.counters.client_requests,
            latest.counters.client_successes,
            latest.counters.client_failures,
            latest.counters.upstream_attempts,
            latest.counters.upstream_successes,
            latest.counters.upstream_failures,
            latest.counters.failovers,
        ],
    )?;
    let max_seq = batch.iter().map(|message| message.seq).max().unwrap_or(0);
    let max_change_seq = batch
        .iter()
        .map(|message| message.change_seq)
        .max()
        .unwrap_or(0);
    set_meta_max(&transaction, "next_seq", max_seq.max(1) + 1)?;
    set_meta_max(&transaction, "next_change_seq", max_change_seq.max(1) + 1)?;
    set_meta(&transaction, "retained_event_count", retained_event_count)?;
    let rotation = rotate_retention(&transaction)?;
    transaction.commit()?;
    Ok(rotation)
}

impl RuntimeStore {
    pub fn new(path: impl Into<PathBuf>) -> Result<(Self, RuntimeSnapshot), String> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        check_existing_schema(&path)?;
        let mut connection = Connection::open(&path).map_err(|error| error.to_string())?;
        setup_connection(&mut connection).map_err(|error| error.to_string())?;
        harden_database_file(&path);
        let normalized_changes =
            normalize_startup(&mut connection).map_err(|error| error.to_string())?;
        let startup_rotation =
            rotate_retention_now(&mut connection).map_err(|error| error.to_string())?;
        let rotated_ids = startup_rotation
            .deleted_event_ids
            .into_iter()
            .collect::<HashSet<_>>();
        let mut state = load_state(&connection).map_err(|error| error.to_string())?;
        for change in normalized_changes
            .into_iter()
            .filter(|change| !rotated_ids.contains(&change.event.id))
        {
            state.remember_change(change);
        }
        let snapshot = load_snapshot(&connection)?;
        let (sender, receiver) = mpsc::sync_channel::<Command>(PENDING_EVENTS_LIMIT);
        let worker_path = path.clone();
        let inner = Arc::new(Inner {
            path,
            sender,
            state: Mutex::new(state),
            pending_events: AtomicUsize::new(0),
            pending_bytes: AtomicUsize::new(0),
            backpressure: AtomicBool::new(false),
            hard_backpressure: AtomicBool::new(false),
            #[cfg(test)]
            fail_writes: AtomicBool::new(false),
        });
        // The worker must not keep an `Arc<Inner>` alive: `Inner` owns the
        // channel sender, so holding a strong reference here would create a
        // self-retaining cycle and leave one blocked thread per store.
        let worker_ref = Arc::downgrade(&inner);
        thread::Builder::new()
            .name("runtime-sqlite".into())
            .spawn(move || {
                let mut connection = match Connection::open(worker_path) {
                    Ok(connection) => connection,
                    Err(error) => {
                        if let Some(worker_inner) = worker_ref.upgrade() {
                            worker_inner.state.lock().unwrap().last_error = Some(error.to_string());
                        }
                        return;
                    }
                };
                if let Err(error) = setup_connection(&mut connection) {
                    if let Some(worker_inner) = worker_ref.upgrade() {
                        worker_inner.state.lock().unwrap().last_error = Some(error.to_string());
                    }
                    return;
                }
                let mut projection_maintenance =
                    projection_maintenance_needed(&connection).unwrap_or(true);
                let mut pending = PendingBatch::default();
                let mut retry_delay = RETRY_INITIAL;
                loop {
                    let command = if pending.is_empty() {
                        if projection_maintenance {
                            match receiver.recv_timeout(Duration::from_millis(25)) {
                                Ok(command) => command,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let Some(worker_inner) = worker_ref.upgrade() else {
                                        break;
                                    };
                                    match run_projection_maintenance(&mut connection) {
                                        Ok(more) => {
                                            projection_maintenance = more;
                                            let _ = refresh_cached_storage(
                                                &worker_inner,
                                                &connection,
                                                !more,
                                            );
                                        }
                                        Err(error) => {
                                            let failed =
                                                meta_i64(&connection, "hourly_rollup_failed")
                                                    .ok()
                                                    .flatten()
                                                    .unwrap_or(0)
                                                    .saturating_add(1);
                                            let _ = set_meta(
                                                &connection,
                                                "hourly_rollup_failed",
                                                failed,
                                            );
                                            worker_inner.state.lock().unwrap().last_error =
                                                Some(format!(
                                                    "runtime projection maintenance failed: {error}"
                                                ));
                                        }
                                    }
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        } else {
                            match receiver.recv_timeout(RETENTION_IDLE_CHECK_INTERVAL) {
                                Ok(command) => command,
                                Err(mpsc::RecvTimeoutError::Timeout) => {
                                    let Some(worker_inner) = worker_ref.upgrade() else {
                                        break;
                                    };
                                    match run_retention_maintenance(&worker_inner, &mut connection)
                                    {
                                        Ok(true) => projection_maintenance = true,
                                        Ok(false) => {}
                                        Err(error) => {
                                            worker_inner.state.lock().unwrap().last_error =
                                                Some(format!(
                                                    "runtime retention maintenance failed: {error}"
                                                ));
                                        }
                                    }
                                    continue;
                                }
                                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                            }
                        }
                    } else {
                        match receiver.recv_timeout(pending.flush_wait()) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let Some(worker_inner) = worker_ref.upgrade() else {
                                    break;
                                };
                                match commit_pending(&worker_inner, &mut connection, &mut pending) {
                                    Ok(()) => {
                                        retry_delay = RETRY_INITIAL;
                                        projection_maintenance = true;
                                    }
                                    Err(error) => {
                                        worker_inner.state.lock().unwrap().last_error =
                                            Some(error.to_string());
                                        thread::sleep(retry_delay);
                                        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX);
                                    }
                                }
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    };
                    match command {
                        Command::Write(message) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                break;
                            };
                            pending.upsert(message, &worker_inner);
                            if pending.len() >= BATCH_EVENTS || pending.bytes >= BATCH_BYTES {
                                match commit_pending(&worker_inner, &mut connection, &mut pending) {
                                    Ok(()) => {
                                        retry_delay = RETRY_INITIAL;
                                        projection_maintenance = true;
                                    }
                                    Err(error) => {
                                        worker_inner.state.lock().unwrap().last_error =
                                            Some(error.to_string());
                                        thread::sleep(retry_delay);
                                        retry_delay = retry_delay.saturating_mul(2).min(RETRY_MAX);
                                    }
                                }
                            }
                        }
                        Command::Flush(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string());
                            if result.is_ok() {
                                retry_delay = RETRY_INITIAL;
                                projection_maintenance = true;
                            } else if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                        }
                        Command::Reset(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| reset_database(&worker_inner, &mut connection))
                                    .map_err(|error| error.to_string());
                            let succeeded = result.is_ok();
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                            if succeeded {
                                projection_maintenance = true;
                                let _ = connection.execute_batch("PRAGMA wal_checkpoint(PASSIVE);");
                                if std::fs::metadata(&worker_inner.path)
                                    .map(|metadata| metadata.len() >= 64 * 1024 * 1024)
                                    .unwrap_or(false)
                                {
                                    let _ = connection.execute_batch("VACUUM;");
                                }
                            }
                        }
                        Command::Recreate(reply) => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| recreate_database(&worker_inner, &mut connection))
                                    .map_err(|error| error.to_string());
                            let succeeded = result.is_ok();
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                            if succeeded {
                                projection_maintenance = false;
                                let _ =
                                    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
                            }
                        }
                        Command::CleanupBefore { older_than, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| {
                                        cleanup_before_database(
                                            &worker_inner,
                                            &mut connection,
                                            older_than,
                                        )
                                    })
                                    .map_err(|error| error.to_string());
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::DeleteSession { session_id, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .and_then(|_| {
                                        delete_session_database(
                                            &worker_inner,
                                            &mut connection,
                                            &session_id,
                                        )
                                    })
                                    .map_err(|error| error.to_string());
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::SetRetention { update, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string())
                                    .and_then(|_| {
                                        set_retention_database(
                                            &worker_inner,
                                            &mut connection,
                                            update,
                                        )
                                    });
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            } else {
                                projection_maintenance = true;
                            }
                            let _ = reply.send(result);
                        }
                        Command::ReplacePricing { update, reply } => {
                            let Some(worker_inner) = worker_ref.upgrade() else {
                                let _ = reply.send(Err("runtime store closed".into()));
                                break;
                            };
                            let result =
                                commit_pending(&worker_inner, &mut connection, &mut pending)
                                    .map_err(|error| error.to_string())
                                    .and_then(|_| {
                                        replace_pricing_database(&mut connection, update)
                                    });
                            if let Err(error) = &result {
                                worker_inner.state.lock().unwrap().last_error = Some(error.clone());
                            }
                            let _ = reply.send(result);
                        }
                    }
                }
                if let Some(worker_inner) = worker_ref.upgrade() {
                    let _ = commit_pending(&worker_inner, &mut connection, &mut pending);
                }
            })
            .map_err(|error| error.to_string())?;
        Ok((Self { inner }, snapshot))
    }

    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    pub fn database_path(&self) -> &Path {
        &self.inner.path
    }

    pub fn recent_changes_for_request(&self, request_id: &str) -> Vec<RuntimeChange> {
        let request_id = request_id.trim();
        if request_id.is_empty() {
            return Vec::new();
        }
        self.inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .filter(|change| change.event.request_id.as_deref() == Some(request_id))
            .cloned()
            .collect()
    }

    #[cfg(test)]
    fn set_test_write_failure(&self, enabled: bool) {
        self.inner.fail_writes.store(enabled, Ordering::Release);
    }

    pub fn enqueue(
        &self,
        event: RuntimeEvent,
        counters: RuntimeCounters,
    ) -> Result<RuntimeChange, String> {
        let payload_size = serde_json::to_vec(&event)
            .map_err(|error| error.to_string())?
            .len();
        let mut state = self.inner.state.lock().unwrap();
        let previous_counters = state.counters;
        let previous_latest_event = state.latest_event.clone();
        let previous_active_sequence = state.active_sequences.get(&event.id).copied();
        let replacing_active = state.active_sequences.contains_key(&event.id);
        if self.inner.pending_bytes.load(Ordering::Acquire) + payload_size > PENDING_BYTES_LIMIT
            || (!replacing_active
                && self.inner.pending_events.load(Ordering::Acquire) >= PENDING_EVENTS_LIMIT)
        {
            self.inner.backpressure.store(true, Ordering::Release);
            self.inner.hard_backpressure.store(true, Ordering::Release);
            state.last_error = Some("runtime_storage_backpressure".into());
            return Err("runtime_storage_backpressure".into());
        }
        let seq = if let Some(seq) = state.active_sequences.get(&event.id).copied() {
            if !event.is_in_flight() {
                state.active_sequences.remove(&event.id);
            }
            seq
        } else {
            let seq = state.next_seq;
            state.next_seq += 1;
            if event.is_in_flight() {
                state.active_sequences.insert(event.id.clone(), seq);
            }
            seq
        };
        let change_seq = state.next_change_seq;
        state.next_change_seq += 1;
        state.counters = counters;
        state.latest_event = Some(event.clone());
        // Admission and reservation share the state lock. Without reserving
        // before releasing it, concurrent request threads could all pass the
        // hard-limit check and temporarily exceed the memory budget.
        self.inner.pending_events.fetch_add(1, Ordering::AcqRel);
        self.inner
            .pending_bytes
            .fetch_add(payload_size, Ordering::AcqRel);
        let message = WriteMessage {
            seq,
            change_seq,
            event: event.clone(),
            counters,
            bytes: payload_size,
        };
        if let Err(error) = self.inner.sender.try_send(Command::Write(message)) {
            self.inner.pending_events.fetch_sub(1, Ordering::AcqRel);
            self.inner
                .pending_bytes
                .fetch_sub(payload_size, Ordering::AcqRel);
            state.counters = previous_counters;
            state.latest_event = previous_latest_event;
            match previous_active_sequence {
                Some(previous) => {
                    state.active_sequences.insert(event.id.clone(), previous);
                }
                None => {
                    state.active_sequences.remove(&event.id);
                }
            }
            self.inner.backpressure.store(true, Ordering::Release);
            self.inner.hard_backpressure.store(true, Ordering::Release);
            state.last_error = Some(error.to_string());
            return Err(error.to_string());
        }
        drop(state);
        let change = RuntimeChange {
            seq,
            change_seq,
            event,
        };
        {
            let mut state = self.inner.state.lock().unwrap();
            state.remember_change(change.clone());
        }
        if self.inner.pending_bytes.load(Ordering::Acquire) >= BACKPRESSURE_BYTES
            || self.inner.pending_events.load(Ordering::Acquire) >= BACKPRESSURE_EVENTS
        {
            self.inner.backpressure.store(true, Ordering::Release);
        }
        Ok(change)
    }

    pub fn flush(&self) -> Result<(), String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Flush(sender))
            .map_err(|e| e.to_string())?;
        receiver.recv().map_err(|e| e.to_string())??;
        Ok(())
    }

    pub fn set_retention(
        &self,
        update: RuntimeRetentionUpdate,
    ) -> Result<RuntimeRetentionMutation, String> {
        validate_retention_update(&update)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::SetRetention {
                update,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn replace_pricing(
        &self,
        update: RuntimePricingUpdate,
    ) -> Result<RuntimePricingMutation, String> {
        validate_pricing_update(&update)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::ReplacePricing {
                update,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        receiver.recv().map_err(|error| error.to_string())?
    }

    pub fn reset(&self) -> Result<i64, String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Reset(sender))
            .map_err(|e| e.to_string())?;
        let generation = receiver.recv().map_err(|e| e.to_string())??;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = RuntimeCounters::default();
        state.latest_event = None;
        state.reset_generation = generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_error = None;
        self.inner.backpressure.store(false, Ordering::Release);
        self.inner.hard_backpressure.store(false, Ordering::Release);
        Ok(generation)
    }

    /// Replace the runtime statistics schema with a fresh current-version
    /// database. Unlike `reset`, this removes legacy table columns as well as
    /// all retained rows, while leaving diagnostic capture outside the scope
    /// of the operation.
    pub fn recreate(&self) -> Result<i64, String> {
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::Recreate(sender))
            .map_err(|e| e.to_string())?;
        let generation = receiver.recv().map_err(|e| e.to_string())??;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = RuntimeCounters::default();
        state.latest_event = None;
        state.reset_generation = generation;
        state.history_generation = state.history_generation.saturating_add(1);
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_error = None;
        self.inner.backpressure.store(false, Ordering::Release);
        self.inner.hard_backpressure.store(false, Ordering::Release);
        Ok(generation)
    }

    /// Preview a one-off age cleanup after flushing queued events, so the
    /// confirmation count is based on the same durable request groups the
    /// mutation will evaluate. The cutoff uses Apple reference-date seconds,
    /// matching every runtime event timestamp exposed by Admin.
    pub fn cleanup_before_preview(&self, older_than: f64) -> Result<RuntimeCleanupPreview, String> {
        validate_cleanup_cutoff(older_than)?;
        self.flush()?;
        let connection = read_connection(&self.inner.path)?;
        cleanup_before_preview_database(&connection, older_than).map_err(|error| error.to_string())
    }

    pub fn cleanup_before(&self, older_than: f64) -> Result<RuntimeCleanupMutation, String> {
        validate_cleanup_cutoff(older_than)?;
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::CleanupBefore {
                older_than,
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        let mutation = receiver.recv().map_err(|error| error.to_string())??;
        let refreshed =
            load_state(&read_connection(&self.inner.path)?).map_err(|error| error.to_string())?;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = refreshed.counters;
        state.latest_event = refreshed.latest_event;
        state.history_generation = mutation.history_generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_commit_at = Some(now());
        state.last_error = None;
        Ok(mutation)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<SessionMutation, String> {
        self.delete_session_confirmed(session_id, false)
    }

    /// Delete a session, optionally allowing the aggregate unidentified
    /// bucket.  The explicit confirmation is deliberately kept at the store
    /// boundary so non-HTTP callers cannot accidentally turn a UI affordance
    /// into a destructive all-unknown operation.
    pub fn delete_session_confirmed(
        &self,
        session_id: &str,
        confirm_unidentified: bool,
    ) -> Result<SessionMutation, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("不能删除空会话；请使用完整 sessionID/threadID".into());
        }
        if session_id == "unidentified_session" && !confirm_unidentified {
            return Err("不能删除未识别会话；请使用完整 sessionID/threadID".into());
        }
        let (sender, receiver) = mpsc::channel();
        self.inner
            .sender
            .send(Command::DeleteSession {
                session_id: session_id.to_owned(),
                reply: sender,
            })
            .map_err(|error| error.to_string())?;
        let mutation = receiver.recv().map_err(|error| error.to_string())??;
        let refreshed =
            load_state(&read_connection(&self.inner.path)?).map_err(|error| error.to_string())?;
        let mut state = self.inner.state.lock().unwrap();
        state.counters = refreshed.counters;
        state.latest_event = refreshed.latest_event;
        state.reset_generation = mutation.reset_generation;
        state.active_sequences.clear();
        state.recent_changes.clear();
        state.last_commit_at = Some(now());
        state.last_error = None;
        Ok(mutation)
    }

    pub fn export_session(&self, session_id: &str) -> Result<Value, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("sessionID 不能为空".into());
        }
        self.flush()?;
        export_session_json(&read_connection(&self.inner.path)?, session_id)
    }

    pub fn summary(&self) -> RuntimeSummary {
        let (
            reset_generation,
            history_generation,
            counters,
            latest_event,
            last_commit_at,
            last_error,
            storage,
        ) = {
            let state = self.inner.state.lock().unwrap();
            (
                state.reset_generation,
                state.history_generation,
                state.counters,
                state.latest_event.clone(),
                state.last_commit_at,
                state.last_error.clone(),
                state.storage.clone(),
            )
        };
        let (db_bytes, wal_bytes) = database_file_sizes(&self.inner.path);
        let pending_events = self.inner.pending_events.load(Ordering::Acquire);
        let pending_bytes = self.inner.pending_bytes.load(Ordering::Acquire);
        RuntimeSummary {
            api_version: 1,
            storage: RuntimeStorageStatus {
                backend: "sqlite",
                state: if self.inner.backpressure.load(Ordering::Acquire)
                    || pending_bytes >= BACKPRESSURE_BYTES
                    || pending_events >= BACKPRESSURE_EVENTS
                {
                    "backpressure"
                } else if last_error.is_some() {
                    "degraded"
                } else {
                    "ready"
                },
                pending_events,
                pending_bytes,
                event_count: storage.event_count,
                completed_event_count: storage.completed_event_count,
                in_flight_event_count: storage.in_flight_event_count,
                oldest_event_at: storage.oldest_event_at,
                newest_event_at: storage.newest_event_at,
                retained_from_seq: storage.retained_from_seq,
                payload_bytes: storage.payload_bytes,
                live_bytes: storage.live_bytes,
                allocated_bytes: storage.allocated_bytes,
                db_bytes,
                wal_bytes,
                schema_version: storage.schema_version,
                backfill_cursor: storage.backfill_cursor,
                backfill_complete: storage.backfill_complete,
                backfill_failed: storage.backfill_failed,
                indexes_ready: storage.indexes_ready,
                rollup_complete: storage.rollup_complete,
                rollup_max_seq: storage.rollup_max_seq,
                rollup_history_generation: storage.rollup_history_generation,
                rollup_failed: storage.rollup_failed,
                rollup_dirty_buckets: storage.rollup_dirty_buckets,
                user_deleted_events: storage.user_deleted_events,
                user_deleted_requests: storage.user_deleted_requests,
                last_commit_at,
                last_error,
            },
            reset_generation,
            history_generation,
            counters,
            latest_event,
        }
    }

    pub fn snapshot(&self) -> Result<RuntimeSnapshot, String> {
        load_snapshot(&read_connection(&self.inner.path)?)
    }

    pub fn is_backpressured(&self) -> bool {
        self.inner.hard_backpressure.load(Ordering::Acquire)
            || self.inner.pending_bytes.load(Ordering::Acquire) >= PENDING_BYTES_LIMIT
            || self.inner.pending_events.load(Ordering::Acquire) >= PENDING_EVENTS_LIMIT
    }

    // 事件查询的过滤条件就是这么多维,合并成 struct 会让调用方多写一层构造。
    #[allow(clippy::too_many_arguments)]
    pub fn events(
        &self,
        before_seq: Option<i64>,
        after_change_seq: Option<i64>,
        limit: usize,
        kind: Option<&str>,
        request_id: Option<&str>,
        outcome: Option<&str>,
        from: Option<f64>,
        to: Option<f64>,
    ) -> Result<Vec<RuntimeEventListItem>, String> {
        // Zero is the client-side sentinel for "no cursor". Treating it as
        // an actual cursor would return an empty/incorrect page (`seq < 0` or
        // the oldest in-memory changes) rather than the newest page expected
        // by the list API.
        let before_seq = before_seq.filter(|value| *value > 0);
        let after_change_seq = after_change_seq.filter(|value| *value > 0);
        let limit = limit.clamp(1, 200);
        let query_limit = limit + 1;
        let recent_changes = self
            .inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .cloned()
            .collect::<Vec<_>>();

        if let Some(cursor) = after_change_seq {
            let mut changes = recent_changes
                .into_iter()
                .filter(|change| change.change_seq > cursor)
                .filter(|change| {
                    let event = &change.event;
                    !before_seq.is_some_and(|value| change.seq >= value)
                        && !kind.is_some_and(|value| value != event.kind)
                        && !request_id
                            .is_some_and(|value| event.request_id.as_deref() != Some(value))
                        && !outcome
                            .is_some_and(|value| option_token(event.outcome) != Some(value.into()))
                        && !from.is_some_and(|value| event.timestamp < value)
                        && !to.is_some_and(|value| event.timestamp > value)
                })
                .map(|change| {
                    RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event)
                })
                .collect::<Vec<_>>();
            changes.sort_by_key(|item| item.change_seq);
            changes.truncate(query_limit);
            return Ok(changes);
        }

        let connection = read_connection(&self.inner.path)?;
        let order = if after_change_seq.is_some() {
            "change_seq ASC"
        } else {
            "seq DESC"
        };
        // Keep one positional parameter set for every branch.  This avoids
        // binding optional named parameters that are absent from a dynamically
        // assembled statement (rusqlite correctly rejects those bindings).
        let sql = format!(
            "SELECT seq,change_seq,payload_json FROM runtime_events
             WHERE (?1 IS NULL OR seq < ?1)
               AND (?2 IS NULL OR change_seq > ?2)
               AND (?3 IS NULL OR kind = ?3)
               AND (?4 IS NULL OR request_id = ?4)
               AND (?5 IS NULL OR outcome = ?5)
               AND (?6 IS NULL OR timestamp >= ?6)
               AND (?7 IS NULL OR timestamp <= ?7)
             ORDER BY {order} LIMIT ?8"
        );
        let mut statement = connection.prepare(&sql).map_err(|e| e.to_string())?;
        let mut rows = statement
            .query(params![
                before_seq,
                after_change_seq,
                kind,
                request_id,
                outcome,
                from,
                to,
                query_limit as i64,
            ])
            .map_err(|e| e.to_string())?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(|e| e.to_string())? {
            let seq: i64 = row.get(0).map_err(|e| e.to_string())?;
            let change_seq: i64 = row.get(1).map_err(|e| e.to_string())?;
            let payload: String = row.get(2).map_err(|e| e.to_string())?;
            let event = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
            result.push(RuntimeEventListItem::from_change(seq, change_seq, event));
        }
        let mut merged = result
            .into_iter()
            .map(|item| (item.id.clone(), item))
            .collect::<HashMap<_, _>>();
        for change in recent_changes {
            let event = &change.event;
            if before_seq.is_some_and(|value| change.seq >= value)
                || after_change_seq.is_some_and(|value| change.change_seq <= value)
                || kind.is_some_and(|value| value != event.kind)
                || request_id.is_some_and(|value| event.request_id.as_deref() != Some(value))
                || outcome.is_some_and(|value| option_token(event.outcome) != Some(value.into()))
                || from.is_some_and(|value| event.timestamp < value)
                || to.is_some_and(|value| event.timestamp > value)
            {
                continue;
            }
            let item =
                RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event);
            if merged
                .get(&item.id)
                .is_none_or(|current| item.change_seq > current.change_seq)
            {
                merged.insert(item.id.clone(), item);
            }
        }
        let mut result = merged.into_values().collect::<Vec<_>>();
        result.sort_by_key(|row| std::cmp::Reverse(row.seq));
        result.truncate(query_limit);
        Ok(result)
    }

    /// A cursor older than the oldest retained change cannot be completed by
    /// this in-memory change window. The caller must reload summary and the
    /// newest page instead of treating the current database row as every
    /// intermediate in-flight/completed update.
    pub fn change_cursor_valid(&self, cursor: Option<i64>) -> Result<bool, String> {
        let Some(cursor) = cursor.filter(|value| *value > 0) else {
            return Ok(true);
        };
        let (latest, recent_oldest) = {
            let state = self.inner.state.lock().unwrap();
            (
                state.next_change_seq.saturating_sub(1),
                state.recent_changes.front().map(|change| change.change_seq),
            )
        };
        if cursor > latest {
            return Ok(false);
        }
        Ok(recent_oldest.map_or(cursor == latest, |value| cursor >= value - 1))
    }

    pub fn event(&self, id: &str) -> Result<Option<RuntimeChange>, String> {
        if let Some(change) = self
            .inner
            .state
            .lock()
            .unwrap()
            .recent_changes
            .iter()
            .rev()
            .find(|change| change.event.id == id)
            .cloned()
        {
            return Ok(Some(change));
        }
        let connection = read_connection(&self.inner.path)?;
        connection
            .query_row(
                "SELECT seq,change_seq,payload_json FROM runtime_events WHERE event_id=?1",
                params![id],
                |row| {
                    let event: RuntimeEvent = serde_json::from_str(&row.get::<_, String>(2)?)
                        .map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                2,
                                rusqlite::types::Type::Text,
                                Box::new(error),
                            )
                        })?;
                    Ok(RuntimeChange {
                        seq: row.get(0)?,
                        change_seq: row.get(1)?,
                        event,
                    })
                },
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    pub fn analytics(&self, range: &str) -> Result<Value, String> {
        self.analytics_filtered(range, &AnalyticsFilter::default())
    }

    pub fn analytics_filtered(
        &self,
        range: &str,
        filter: &AnalyticsFilter,
    ) -> Result<Value, String> {
        let filter = filter.normalized();
        let query_filter = crate::runtime_query::RuntimeFilter {
            client_kind: filter.client_kind,
            endpoint_id: filter.endpoint_id,
            project_id: filter.project_id,
            project_name: filter.project,
            session_id: filter.session_id,
            from: filter.from,
            to: filter.to,
            ..crate::runtime_query::RuntimeFilter::default()
        };
        let summary = crate::runtime_query::analytics(&self.inner.path, range, &query_filter)
            .map_err(|error| error.to_string())?;
        serde_json::to_value(summary).map_err(|error| error.to_string())
    }
}

#[derive(Default)]
struct DimensionRow {
    attempts: i64,
    successes: i64,
    failures: i64,
    cancelled: i64,
    failovers: i64,
    duration_total: f64,
    duration_count: i64,
    ttfb_total: f64,
    ttfb_count: i64,
    event_ids: Vec<String>,
    token_usage: TokenUsage,
    /// Sanitized workspace suffixes carried by project rows only.  The core
    /// parser has already removed the absolute prefix before events reach the
    /// store, so this is safe to expose as a local Finder locator.
    workspace_paths: BTreeSet<String>,
    /// Only project rows populate this field.  It remains optional in the
    /// wire shape so older/non-project dimensions keep their compact schema.
    project_source: Option<String>,
}

#[derive(Default)]
struct SessionContext {
    projects: BTreeSet<String>,
    client_kinds: BTreeSet<String>,
}

#[derive(Default, Clone, Copy)]
struct TokenUsage {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
    uncached_input_tokens: i64,
    processed_input_tokens: i64,
    processed_total_tokens: i64,
    observed_requests: i64,
    cache_read_reported_requests: i64,
    cache_read_hit_requests: i64,
    cache_read_token_eligible_requests: i64,
    cache_read_token_unknown_requests: i64,
    cache_read_token_numerator: i64,
    cache_read_token_denominator: i64,
    semantics: TokenAccountingSemantics,
    quality: TokenAccountingQuality,
    usage_field_presence: UsageFieldPresence,
}

#[derive(Default, Clone, Copy)]
struct UsageFieldPresence {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TokenAccountingSemantics {
    #[default]
    Unknown,
    Subset,
    Independent,
    Mixed,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum TokenAccountingQuality {
    #[default]
    Unknown,
    Partial,
    Complete,
    Mixed,
}

impl TokenUsage {
    fn add_trace(&mut self, event: &RuntimeEvent) {
        if event.request_purpose == Some(RequestPurpose::TokenCount) {
            return;
        }
        let Some(usage) = event
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.usage.as_ref())
        else {
            return;
        };
        self.usage_field_presence.input_tokens += i64::from(usage.input_tokens.is_some());
        self.usage_field_presence.output_tokens += i64::from(usage.output_tokens.is_some());
        self.usage_field_presence.cache_read_input_tokens +=
            i64::from(usage.cache_read_input_tokens.is_some());
        self.usage_field_presence.cache_creation_input_tokens +=
            i64::from(usage.cache_creation_input_tokens.is_some());
        self.usage_field_presence.reasoning_tokens += i64::from(usage.reasoning_tokens.is_some());
        self.input_tokens = self
            .input_tokens
            .saturating_add(usage.input_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        self.output_tokens = self
            .output_tokens
            .saturating_add(usage.output_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        self.cache_read_input_tokens = self.cache_read_input_tokens.saturating_add(
            usage
                .cache_read_input_tokens
                .unwrap_or(0)
                .min(i64::MAX as u64) as i64,
        );
        self.cache_creation_input_tokens = self.cache_creation_input_tokens.saturating_add(
            usage
                .cache_creation_input_tokens
                .unwrap_or(0)
                .min(i64::MAX as u64) as i64,
        );
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(usage.reasoning_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        let input = usage.input_tokens.unwrap_or(0).min(i64::MAX as u64) as i64;
        let cache_read = usage
            .cache_read_input_tokens
            .unwrap_or(0)
            .min(i64::MAX as u64) as i64;
        let cache_write = usage
            .cache_creation_input_tokens
            .unwrap_or(0)
            .min(i64::MAX as u64) as i64;
        let (semantics, processed_input, uncached_input) = match event
            .target_format
            .or(event.source_format)
            .map(|format| format.token())
        {
            Some("anthropic") => (
                TokenAccountingSemantics::Independent,
                input.saturating_add(cache_read).saturating_add(cache_write),
                input,
            ),
            Some("openai") | Some("openai-responses") => (
                TokenAccountingSemantics::Subset,
                input,
                input.saturating_sub(cache_read).saturating_sub(cache_write),
            ),
            _ => (TokenAccountingSemantics::Unknown, input, input),
        };
        self.uncached_input_tokens = self.uncached_input_tokens.saturating_add(uncached_input);
        self.processed_input_tokens = self.processed_input_tokens.saturating_add(processed_input);
        self.processed_total_tokens = self
            .processed_total_tokens
            .saturating_add(processed_input)
            .saturating_add(usage.output_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        if let Some(cache_read) = usage.cache_read_input_tokens {
            let cache_read = cache_read.min(i64::MAX as u64) as i64;
            self.cache_read_reported_requests = self.cache_read_reported_requests.saturating_add(1);
            if cache_read > 0 {
                self.cache_read_hit_requests = self.cache_read_hit_requests.saturating_add(1);
            }
            if matches!(
                semantics,
                TokenAccountingSemantics::Subset | TokenAccountingSemantics::Independent
            ) && usage.input_tokens.is_some()
            {
                self.cache_read_token_eligible_requests =
                    self.cache_read_token_eligible_requests.saturating_add(1);
                self.cache_read_token_numerator = self
                    .cache_read_token_numerator
                    .saturating_add(cache_read.max(0));
                self.cache_read_token_denominator = self
                    .cache_read_token_denominator
                    .saturating_add(processed_input.max(0));
            } else {
                self.cache_read_token_unknown_requests =
                    self.cache_read_token_unknown_requests.saturating_add(1);
            }
        } else {
            self.cache_read_token_unknown_requests =
                self.cache_read_token_unknown_requests.saturating_add(1);
        }
        self.semantics = if self.observed_requests == 0 {
            semantics
        } else {
            merge_semantics(self.semantics, semantics)
        };
        let quality = if usage.input_tokens.is_some() && usage.output_tokens.is_some() {
            TokenAccountingQuality::Complete
        } else {
            TokenAccountingQuality::Partial
        };
        self.quality = merge_quality(self.quality, quality);
        self.observed_requests = self.observed_requests.saturating_add(1);
    }

    fn value(self) -> Value {
        json!({
            "inputTokens": self.input_tokens,
            "outputTokens": self.output_tokens,
            "cacheReadInputTokens": self.cache_read_input_tokens,
            "cacheCreationInputTokens": self.cache_creation_input_tokens,
            "reasoningTokens": self.reasoning_tokens,
            "uncachedInputTokens": self.uncached_input_tokens,
            "processedInputTokens": self.processed_input_tokens,
            "processedTotalTokens": self.processed_total_tokens,
            "totalTokens": self.input_tokens.saturating_add(self.output_tokens),
            "observedRequests": self.observed_requests,
            "cacheReadReportedRequests": self.cache_read_reported_requests,
            "cacheReadHitRequests": self.cache_read_hit_requests,
            "cacheReadTokenEligibleRequests": self.cache_read_token_eligible_requests,
            "cacheReadTokenUnknownRequests": self.cache_read_token_unknown_requests,
            "cacheReadTokenRate": (self.cache_read_token_denominator > 0).then(|| {
                (self.cache_read_token_numerator as f64
                    / self.cache_read_token_denominator as f64)
                    .clamp(0.0, 1.0)
            }),
            "cacheReadRequestRate": (self.cache_read_reported_requests > 0).then(|| {
                (self.cache_read_hit_requests as f64
                    / self.cache_read_reported_requests as f64)
                    .clamp(0.0, 1.0)
            }),
            "tokenAccountingSemantics": self.semantics.as_str(),
            "tokenAccountingQuality": self.quality.as_str(),
            "usageFieldPresence": {
                "inputTokens": self.usage_field_presence.input_tokens,
                "outputTokens": self.usage_field_presence.output_tokens,
                "cacheReadInputTokens": self.usage_field_presence.cache_read_input_tokens,
                "cacheCreationInputTokens": self.usage_field_presence.cache_creation_input_tokens,
                "reasoningTokens": self.usage_field_presence.reasoning_tokens,
            },
        })
    }
}

impl TokenAccountingSemantics {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Subset => "subset",
            Self::Independent => "independent",
            Self::Mixed => "mixed",
        }
    }
}

impl TokenAccountingQuality {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::Mixed => "mixed",
        }
    }
}

fn merge_semantics(
    current: TokenAccountingSemantics,
    next: TokenAccountingSemantics,
) -> TokenAccountingSemantics {
    // An unknown protocol cannot be safely folded into a known denominator.
    // Propagate it instead of labelling the aggregate merely `mixed`, which
    // would make the UI present a plausible but unverifiable hit rate.
    if current == TokenAccountingSemantics::Unknown || next == TokenAccountingSemantics::Unknown {
        return TokenAccountingSemantics::Unknown;
    }
    match (current, next) {
        (left, right) if left == right => left,
        _ => TokenAccountingSemantics::Mixed,
    }
}

fn merge_quality(
    current: TokenAccountingQuality,
    next: TokenAccountingQuality,
) -> TokenAccountingQuality {
    match (current, next) {
        (TokenAccountingQuality::Unknown, value) | (value, TokenAccountingQuality::Unknown) => {
            value
        }
        (left, right) if left == right => left,
        _ => TokenAccountingQuality::Mixed,
    }
}

#[derive(Default)]
struct AnalyticsAggregate {
    upstream_request_ids: HashSet<String>,
    request_endpoints: HashMap<String, String>,
    endpoints: BTreeMap<String, DimensionRow>,
    models: BTreeMap<String, DimensionRow>,
    client_kinds: BTreeMap<String, DimensionRow>,
    request_purposes: BTreeMap<String, DimensionRow>,
    feature_rules: BTreeMap<String, DimensionRow>,
    protocol_routes: BTreeMap<String, DimensionRow>,
    failure_kinds: BTreeMap<String, DimensionRow>,
    failure_phases: BTreeMap<String, DimensionRow>,
    upstream_statuses: BTreeMap<String, DimensionRow>,
    stream_terminals: BTreeMap<String, DimensionRow>,
    projects: BTreeMap<String, DimensionRow>,
    sessions: BTreeMap<String, DimensionRow>,
    session_context: BTreeMap<String, SessionContext>,
    tool_calls: BTreeMap<String, i64>,
    codex_metadata_present: i64,
    token_usage: TokenUsage,
    client_requests: i64,
    client_successes: i64,
    client_failures: i64,
    client_cancelled: i64,
    upstream_attempts: i64,
    upstream_successes: i64,
    upstream_failures: i64,
    failovers: i64,
}

impl AnalyticsAggregate {
    fn add(&mut self, event: &RuntimeEvent, project_keys: &HashMap<String, String>) {
        if event.kind == KIND_CLIENT {
            self.client_requests += 1;
            if event.is_succeeded() {
                self.client_successes += 1;
            } else if event.is_failed() {
                self.client_failures += 1;
            } else if event.is_cancelled() {
                self.client_cancelled += 1;
            }
            self.failovers += i64::from(event.failover);
        } else if event.kind == KIND_UPSTREAM {
            self.upstream_attempts += 1;
            if event.is_succeeded() {
                self.upstream_successes += 1;
            } else if event.is_failed() {
                self.upstream_failures += 1;
            }
        }
        if event.kind == KIND_CLIENT {
            let model = event
                .effective_model
                .clone()
                .or_else(|| event.client_model.clone())
                .or_else(|| event.upstream_model.clone())
                .unwrap_or_else(|| "unrecorded".into());
            // Keep the client dimension key identical to the filter/facet key;
            // otherwise the UI cannot select the legacy bucket it displays.
            let client_kind =
                option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into());
            let request_purpose =
                option_token(event.request_purpose).unwrap_or_else(|| "unrecorded".into());
            let feature_rule = event
                .feature_rule_id
                .clone()
                .unwrap_or_else(|| "none".into());
            let source = event
                .source_format
                .map(|value| value.token())
                .unwrap_or("unrecorded");
            let target = event
                .target_format
                .map(|value| value.token())
                .unwrap_or("unrecorded");
            let mode = option_token(event.route_mode).unwrap_or_else(|| "unrecorded".into());
            for (rows, key) in [
                (&mut self.models, model),
                (&mut self.client_kinds, client_kind),
                (&mut self.request_purposes, request_purpose),
                (&mut self.feature_rules, feature_rule),
                (
                    &mut self.protocol_routes,
                    format!("{source}->{target}:{mode}"),
                ),
            ] {
                update_dimension(rows.entry(key).or_default(), event);
            }
            // A completed client event and its upstream attempt(s) describe
            // the same request. Use upstream rows for endpoint counts so
            // failover attempts are visible without double-counting the
            // winning client row. Early routing failures have no upstream row
            // and keep the client-side endpoint as a fallback.
            let fallback_endpoint = event
                .endpoint_name
                .clone()
                .or_else(|| event.endpoint_id.clone())
                .unwrap_or_else(|| "unassigned".into());
            if let Some(request_id) = event
                .request_id
                .as_ref()
                .filter(|request_id| self.upstream_request_ids.contains(*request_id))
            {
                if let Some(endpoint) = self.request_endpoints.get(request_id) {
                    // Client usage is the authoritative response usage. Attribute
                    // it to the final successful endpoint (or the last attempt)
                    // without adding a duplicate endpoint attempt.
                    self.endpoints
                        .entry(endpoint.clone())
                        .or_default()
                        .token_usage
                        .add_trace(event);
                }
            } else {
                update_dimension(self.endpoints.entry(fallback_endpoint).or_default(), event);
            }
            for name in event.tool_calls.as_deref().unwrap_or_default() {
                *self.tool_calls.entry(name.clone()).or_default() += 1;
            }
            if let Some(trace) = &event.stream_trace {
                let terminal = trace
                    .terminal_event
                    .clone()
                    .unwrap_or_else(|| "unobserved".into());
                update_dimension(self.stream_terminals.entry(terminal).or_default(), event);
            }
            if event.codex_metadata.is_some() {
                self.codex_metadata_present += 1;
            }
            self.token_usage.add_trace(event);
            let project = project_key(event, project_keys);
            let project_row = self.projects.entry(project.clone()).or_default();
            add_workspace_paths(project_row, event);
            merge_project_source(&mut project_row.project_source, project_source(event));
            update_dimension(project_row, event);
            let session = session_key(event);
            update_dimension(self.sessions.entry(session.clone()).or_default(), event);
            let context = self.session_context.entry(session).or_default();
            context.projects.insert(project);
            context.client_kinds.insert(
                option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into()),
            );
        } else if event.kind == KIND_UPSTREAM {
            let endpoint = event
                .endpoint_name
                .clone()
                .or_else(|| event.endpoint_id.clone())
                .unwrap_or_else(|| "unassigned".into());
            update_dimension(self.endpoints.entry(endpoint).or_default(), event);
            let upstream_status = event
                .upstream_status_code
                .map(|status| status.to_string())
                .unwrap_or_else(|| "before_headers".into());
            update_dimension(
                self.upstream_statuses.entry(upstream_status).or_default(),
                event,
            );
        }
        if event.kind == KIND_CLIENT && event.is_failed() {
            let failure_kind =
                option_token(event.failure_kind).unwrap_or_else(|| "unrecorded".into());
            let failure_phase =
                option_token(event.failure_phase).unwrap_or_else(|| "unrecorded".into());
            update_dimension(self.failure_kinds.entry(failure_kind).or_default(), event);
            update_dimension(self.failure_phases.entry(failure_phase).or_default(), event);
        }
    }
}

fn build_request_endpoints(events: &[RuntimeEvent]) -> HashMap<String, String> {
    let mut result = HashMap::<String, (bool, String)>::new();
    for event in events.iter().filter(|event| event.kind == KIND_UPSTREAM) {
        let Some(request_id) = event.request_id.as_ref() else {
            continue;
        };
        let Some(endpoint) = event
            .endpoint_name
            .clone()
            .or_else(|| event.endpoint_id.clone())
        else {
            continue;
        };
        let succeeded = event.is_succeeded();
        match result.get(request_id) {
            Some((true, _)) if !succeeded => {}
            _ => {
                result.insert(request_id.clone(), (succeeded, endpoint));
            }
        }
    }
    result
        .into_iter()
        .map(|(request_id, (_, endpoint))| (request_id, endpoint))
        .collect()
}

fn update_dimension(row: &mut DimensionRow, event: &RuntimeEvent) {
    row.attempts += 1;
    if event.is_succeeded() {
        row.successes += 1;
    } else if event.is_failed() {
        row.failures += 1;
    } else if event.is_cancelled() {
        row.cancelled += 1;
    }
    row.failovers += i64::from(event.failover);
    if event.duration_ms >= 0 {
        row.duration_total += event.duration_ms as f64;
        row.duration_count += 1;
    }
    if let Some(ttfb) = event.ttfb_ms {
        row.ttfb_total += ttfb as f64;
        row.ttfb_count += 1;
    }
    if row.event_ids.len() < 3 && !row.event_ids.contains(&event.id) {
        row.event_ids.push(event.id.clone());
    }
    if event.kind == KIND_CLIENT {
        row.token_usage.add_trace(event);
    }
}

fn dimension_value(rows: &BTreeMap<String, DimensionRow>) -> Value {
    Value::Array(
        rows.iter()
            .map(|(name, row)| dimension_row_value(name, row))
            .collect(),
    )
}

fn dimension_row_value(name: &str, row: &DimensionRow) -> Value {
    let pending = row
        .attempts
        .saturating_sub(row.successes + row.failures + row.cancelled);
    let mut value = json!({
        "name": name,
        "attempts": row.attempts,
        "successes": row.successes,
        "failures": row.failures,
        "cancelled": row.cancelled,
        "pending": pending,
        "successRate": success_rate(row.successes, row.failures, row.cancelled),
                    "failovers": row.failovers,
                    "averageDurationMS": (row.duration_count > 0).then_some(row.duration_total / row.duration_count as f64),
                    "averageTTFBMS": (row.ttfb_count > 0).then_some(row.ttfb_total / row.ttfb_count as f64),
                    "eventIDs": row.event_ids,
                    "inputTokens": row.token_usage.input_tokens,
                    "outputTokens": row.token_usage.output_tokens,
                    "cacheReadInputTokens": row.token_usage.cache_read_input_tokens,
                    "cacheCreationInputTokens": row.token_usage.cache_creation_input_tokens,
                    "reasoningTokens": row.token_usage.reasoning_tokens,
                    "uncachedInputTokens": row.token_usage.uncached_input_tokens,
                    "processedInputTokens": row.token_usage.processed_input_tokens,
                    "processedTotalTokens": row.token_usage.processed_total_tokens,
                    "totalTokens": row.token_usage.input_tokens.saturating_add(row.token_usage.output_tokens),
                    "observedRequests": row.token_usage.observed_requests,
                    "tokenAccountingSemantics": row.token_usage.semantics.as_str(),
                    "tokenAccountingQuality": row.token_usage.quality.as_str(),
                    "usageFieldPresence": {
                        "inputTokens": row.token_usage.usage_field_presence.input_tokens,
                        "outputTokens": row.token_usage.usage_field_presence.output_tokens,
                        "cacheReadInputTokens": row.token_usage.usage_field_presence.cache_read_input_tokens,
                        "cacheCreationInputTokens": row.token_usage.usage_field_presence.cache_creation_input_tokens,
                        "reasoningTokens": row.token_usage.usage_field_presence.reasoning_tokens,
                    },
    });
    if let Some(source) = row.project_source.as_deref()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("projectSource".into(), Value::String(source.into()));
    }
    if !row.workspace_paths.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "workspacePaths".into(),
            Value::Array(
                row.workspace_paths
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    value
}

fn add_workspace_paths(row: &mut DimensionRow, event: &RuntimeEvent) {
    let codex_paths = event
        .codex_metadata
        .as_ref()
        .into_iter()
        .flat_map(|metadata| metadata.workspaces.keys().map(String::as_str));
    // 客户端声明的 workspace 同样只以脱敏后缀暴露,与 Codex 行同口径。
    let declared_path = event
        .client_declared
        .as_ref()
        .and_then(|declared| declared.workspace.as_deref());
    for path in codex_paths.chain(declared_path) {
        let Some(locator) = workspace_locator(path) else {
            continue;
        };
        if row.workspace_paths.len() < MAX_PROJECT_WORKSPACE_PATHS
            || row.workspace_paths.contains(&locator)
        {
            row.workspace_paths.insert(locator);
        }
    }
}

/// Re-apply the workspace redaction boundary before exposing a locator from
/// analytics. This also protects older persisted events that may predate the
/// core parser's path sanitization.
fn workspace_locator(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] => None,
        [only] if *only == "workspace" => None,
        [only] => Some((*only).to_string()),
        [parent, child] => Some(format!("{parent}/{child}")),
        _ => Some(format!(
            ".../{}/{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        )),
    }
}

fn merge_project_source(current: &mut Option<String>, next: &'static str) {
    match current.as_deref() {
        None => *current = Some(next.into()),
        Some(value) if value == next || value == "mixed" => {}
        Some(_) => *current = Some("mixed".into()),
    }
}

fn success_rate(successes: i64, failures: i64, cancelled: i64) -> Option<f64> {
    let completed = successes.saturating_add(failures).saturating_add(cancelled);
    (completed > 0).then_some(successes as f64 * 100.0 / completed as f64)
}

fn event_session_projection(event: &RuntimeEvent) -> (String, &'static str) {
    if let Some(value) = event
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return (value.to_owned(), "event");
    }
    if let Some(metadata) = event.codex_metadata.as_ref() {
        if let Some(value) = metadata
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return (value.to_owned(), "codex_session");
        }
        if let Some(value) = metadata
            .thread_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return (value.to_owned(), "codex_thread");
        }
    }
    ("unidentified_session".into(), "unidentified")
}

fn event_project_projection(event: &RuntimeEvent) -> (String, String, &'static str, String) {
    let identity = project_identity(event);
    let project_id = if identity == "unidentified_project" {
        identity.clone()
    } else {
        format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
    };
    let project_name = project_base(&identity);
    let source = project_source(event);
    let mut paths = BTreeSet::new();
    if let Some(metadata) = event.codex_metadata.as_ref() {
        for path in metadata.workspaces.keys() {
            if let Some(path) = workspace_locator(path) {
                paths.insert(path);
            }
        }
    }
    if let Some(path) = event
        .client_declared
        .as_ref()
        .and_then(|declared| declared.workspace.as_deref())
        .and_then(workspace_locator)
    {
        paths.insert(path);
    }
    let workspace_paths_json = serde_json::to_string(
        &paths
            .into_iter()
            .take(MAX_PROJECT_WORKSPACE_PATHS)
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    (project_id, project_name, source, workspace_paths_json)
}

fn project_identity(event: &RuntimeEvent) -> String {
    // Resource requests (for example Codex's `/v1/models` discovery call) do
    // not belong to a project. Keep them out of `unidentified_project` so the
    // UI does not present an internal capability probe as user traffic.
    if is_internal_resource_event(event) {
        return "internal_feature".into();
    }
    let codex_workspaces = event
        .codex_metadata
        .as_ref()
        .filter(|metadata| !metadata.workspaces.is_empty());
    if let Some(metadata) = codex_workspaces {
        return metadata
            .workspaces
            .iter()
            .map(|(path, workspace)| workspace_identity(path, workspace))
            .collect::<Vec<_>>()
            .join("|");
    }
    // Codex 没给结构化 workspace 时才看客户端自称的归因(Claude Code 走这条)。
    event
        .client_declared
        .as_ref()
        .and_then(declared_identity)
        .unwrap_or_else(|| "unidentified_project".into())
}

fn is_internal_resource_event(event: &RuntimeEvent) -> bool {
    event.kind == KIND_CLIENT && event.client_model.as_deref() == Some(RESOURCE_ROUTING_MODEL)
}

fn event_attribution_scope(event: &RuntimeEvent) -> sumpter_core::events::CodexAttributionScope {
    if is_internal_resource_event(event) {
        return sumpter_core::events::CodexAttributionScope::InternalFeature;
    }
    codex_attribution_scope(
        event.codex_metadata.as_ref(),
        event.client_declared.as_ref(),
    )
}

/// 客户端声明的项目身份。显式 project 名优先于 workspace 路径与 git remote,
/// 输出前缀与 [`workspace_identity`] 对齐,好让 `project_base` 用同一套末段规则。
fn declared_identity(declared: &sumpter_core::events::ClientDeclaredMetadata) -> Option<String> {
    let nonempty = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(project) = nonempty(&declared.project) {
        return Some(format!("declared:{project}"));
    }
    if let Some(workspace) = nonempty(&declared.workspace) {
        return Some(format!("path:{workspace}"));
    }
    nonempty(&declared.git_remote).map(|remote| {
        format!(
            "remote:{}",
            remote.trim_end_matches('/').trim_end_matches(".git")
        )
    })
}

/// Explain why the project key is trustworthy.  This is deliberately derived
/// only from the structured Codex workspace metadata; model names, paths in
/// prompts, and client guesses are never used as a source.
fn project_source(event: &RuntimeEvent) -> &'static str {
    if is_internal_resource_event(event) {
        return "internal_feature";
    }
    // 客户端自称的归因可信度低于 Codex 结构化采集,所以只在后者缺位时才作为来源,
    // 并且用独立词标出来,不能和 workspace_local 混为一类。
    let declared_source = || {
        if event
            .client_declared
            .as_ref()
            .and_then(declared_identity)
            .is_some()
        {
            "client_declared"
        } else {
            "missing_workspace_metadata"
        }
    };
    let Some(metadata) = event.codex_metadata.as_ref() else {
        return declared_source();
    };
    if metadata.workspaces.is_empty() {
        return declared_source();
    }
    if metadata.workspaces.len() > 1 {
        return "multiple_workspaces";
    }
    let (path, workspace) = metadata.workspaces.iter().next().expect("non-empty");
    if !path.trim().is_empty() {
        return "workspace_local";
    }
    if workspace
        .associated_remote_urls
        .values()
        .any(|remote| !remote.trim().is_empty())
    {
        return "workspace_remote_fallback";
    }
    "workspace_unidentified"
}

fn workspace_identity(
    path: &str,
    workspace: &sumpter_core::events::CodexWorkspaceMetadata,
) -> String {
    let local_path = path.trim();
    if !local_path.is_empty() {
        return format!("path:{local_path}");
    }
    workspace
        .associated_remote_urls
        .values()
        .next()
        .map(|remote| {
            format!(
                "remote:{}",
                remote.trim().trim_end_matches('/').trim_end_matches(".git")
            )
        })
        .unwrap_or_else(|| "unidentified_project".into())
}

fn project_base(identity: &str) -> String {
    if identity == "unidentified_project" {
        return identity.into();
    }
    if identity.contains('|') {
        return "multiple_workspaces".into();
    }
    identity
        .rsplit(['/', '\\', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or("workspace")
        .to_string()
}

fn project_short_identity(identity: &str) -> String {
    if identity.contains('|') {
        return identity
            .split('|')
            .map(project_short_identity)
            .collect::<Vec<_>>()
            .join(" + ");
    }
    let mut parts = identity
        .rsplit(['/', '\\', ':'])
        .filter(|part| !part.is_empty());
    let last = parts.next().unwrap_or("workspace");
    let parent = parts.next();
    parent
        .map(|value| format!("{value}/{last}"))
        .unwrap_or_else(|| last.to_string())
}

fn build_project_keys(events: &[RuntimeEvent]) -> HashMap<String, String> {
    let mut identities = HashSet::new();
    for event in events.iter().filter(|event| event.kind == KIND_CLIENT) {
        identities.insert(project_identity(event));
    }
    let mut bases = HashMap::<String, usize>::new();
    for identity in &identities {
        *bases.entry(project_base(identity)).or_default() += 1;
    }
    identities
        .into_iter()
        .map(|identity| {
            let base = project_base(&identity);
            let key = if bases.get(&base).copied().unwrap_or(0) <= 1 {
                base
            } else {
                project_short_identity(&identity)
            };
            (identity, key)
        })
        .collect()
}

fn project_key(event: &RuntimeEvent, project_keys: &HashMap<String, String>) -> String {
    let identity = project_identity(event);
    project_keys
        .get(&identity)
        .cloned()
        .unwrap_or_else(|| project_base(&identity))
}

fn session_key(event: &RuntimeEvent) -> String {
    event
        .session_id
        .clone()
        .or_else(|| {
            event.codex_metadata.as_ref().and_then(|metadata| {
                metadata
                    .session_id
                    .clone()
                    .or_else(|| metadata.thread_id.clone())
            })
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unidentified_session".into())
}

fn client_key(event: &RuntimeEvent) -> String {
    option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into())
}

impl AnalyticsFilter {
    pub fn is_active(&self) -> bool {
        self.client_kind.is_some()
            || self.endpoint_id.is_some()
            || self.project_id.is_some()
            || self.project.is_some()
            || self.session_id.is_some()
            || self.from.is_some()
            || self.to.is_some()
    }
}

fn refresh_cached_storage(
    inner: &Arc<Inner>,
    connection: &Connection,
    force: bool,
) -> rusqlite::Result<()> {
    let should_refresh = force
        || inner.state.lock().unwrap().storage_refreshed_at.elapsed() >= Duration::from_secs(1);
    if !should_refresh {
        return Ok(());
    }
    let storage = load_cached_storage(connection)?;
    let mut state = inner.state.lock().unwrap();
    state.storage = storage;
    state.history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0);
    state.storage_refreshed_at = std::time::Instant::now();
    Ok(())
}

fn reconcile_rotation_state(
    inner: &Arc<Inner>,
    connection: &Connection,
    rotation: &StorageRotation,
) -> rusqlite::Result<()> {
    if rotation.deleted_event_ids.is_empty() {
        return Ok(());
    }
    let refreshed = load_state(connection)?;
    let rotated = rotation.deleted_event_ids.iter().collect::<HashSet<_>>();
    let mut state = inner.state.lock().unwrap();
    state
        .recent_changes
        .retain(|change| !rotated.contains(&change.event.id));
    state.counters = refreshed.counters;
    state.latest_event = refreshed.latest_event;
    state.history_generation = refreshed.history_generation;
    Ok(())
}

fn run_retention_maintenance(
    inner: &Arc<Inner>,
    connection: &mut Connection,
) -> rusqlite::Result<bool> {
    let rotation = rotate_retention_now(connection)?;
    if rotation.deleted_event_ids.is_empty() {
        return Ok(false);
    }
    refresh_cached_storage(inner, connection, true)?;
    reconcile_rotation_state(inner, connection, &rotation)?;
    Ok(true)
}

fn commit_pending(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    pending: &mut PendingBatch,
) -> rusqlite::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    #[cfg(test)]
    if inner.fail_writes.load(Ordering::Acquire) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut batch = pending.take();
    batch.sort_by_key(|message| message.change_seq);
    let bytes: usize = batch.iter().map(|item| item.bytes).sum();
    let rotation = match write_batch(connection, &batch) {
        Ok(rotation) => rotation,
        Err(error) => {
            for message in batch {
                pending.messages.insert(message.event.id.clone(), message);
            }
            pending.bytes = bytes;
            pending.first_at.get_or_insert_with(std::time::Instant::now);
            return Err(error);
        }
    };
    refresh_cached_storage(inner, connection, !rotation.deleted_event_ids.is_empty())?;
    inner
        .pending_events
        .fetch_sub(batch.len(), Ordering::AcqRel);
    inner.pending_bytes.fetch_sub(bytes, Ordering::AcqRel);
    reconcile_rotation_state(inner, connection, &rotation)?;
    let mut state = inner.state.lock().unwrap();
    state.last_commit_at = Some(now());
    state.last_error = None;
    inner.backpressure.store(false, Ordering::Release);
    inner.hard_backpressure.store(false, Ordering::Release);
    Ok(())
}

fn validate_retention_update(update: &RuntimeRetentionUpdate) -> Result<(), String> {
    if update.expected_revision < 1 {
        return Err("expectedRevision 必须大于 0".into());
    }
    if update.max_age_days.is_some_and(|value| value < 1) {
        return Err("maxAgeDays 必须为空或至少为 1 天".into());
    }
    if update
        .storage_limit_bytes
        .is_some_and(|value| !(1_048_576..=i64::MAX).contains(&value))
    {
        return Err("storageLimitBytes 必须为空或至少为 1 MiB".into());
    }
    Ok(())
}

fn load_retention_mutation(connection: &Connection) -> Result<RuntimeRetentionMutation, String> {
    connection
        .query_row(
            "SELECT revision
             FROM runtime_retention WHERE id=1",
            [],
            |row| {
                Ok(RuntimeRetentionMutation {
                    revision: row.get(0)?,
                    max_age_days: meta_i64(connection, "retention_max_age_days")?,
                    storage_limit_bytes: meta_i64(connection, "storage_limit_bytes")?,
                })
            },
        )
        .map_err(|error| error.to_string())
}

fn set_retention_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    update: RuntimeRetentionUpdate,
) -> Result<RuntimeRetentionMutation, String> {
    validate_retention_update(&update)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_retention WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    if revision != update.expected_revision {
        return Err(format!(
            "retention_revision_conflict: expected {}, current {revision}",
            update.expected_revision
        ));
    }
    let next_revision = revision.saturating_add(1);
    transaction
        .execute(
            "UPDATE runtime_retention SET revision=?1,updated_at=?2 WHERE id=1",
            params![next_revision, now()],
        )
        .map_err(|error| error.to_string())?;
    set_retention_max_age_meta(&transaction, update.max_age_days)
        .map_err(|error| error.to_string())?;
    set_storage_limit_meta(&transaction, update.storage_limit_bytes)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    let rotation = rotate_retention_now(connection).map_err(|error| error.to_string())?;
    refresh_cached_storage(inner, connection, true).map_err(|error| error.to_string())?;
    reconcile_rotation_state(inner, connection, &rotation).map_err(|error| error.to_string())?;
    load_retention_mutation(connection)
}

fn validate_pricing_update(update: &RuntimePricingUpdate) -> Result<(), String> {
    if update.expected_revision < 1 {
        return Err("expectedRevision 必须大于 0".into());
    }
    let currency = update.currency.trim();
    if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err("currency 必须是三个大写 ASCII 字母".into());
    }
    if update.prices.len() > 2_000 {
        return Err("价格规则不能超过 2000 条".into());
    }
    let mut unique = HashSet::new();
    for price in &update.prices {
        if price
            .endpoint_id
            .as_deref()
            .is_some_and(|value| value.contains('\u{1f}'))
        {
            return Err("endpointID 不能包含保留分隔符".into());
        }
        let model = price.model_key.trim();
        if model.is_empty() || model.len() > 512 {
            return Err("modelKey 不能为空且不能超过 512 字节".into());
        }
        if model.contains('\u{1f}') {
            return Err("modelKey 不能包含保留分隔符".into());
        }
        if !price.effective_from.is_finite()
            || price.effective_to.is_some_and(|value| !value.is_finite())
            || price
                .effective_to
                .is_some_and(|value| value <= price.effective_from)
        {
            return Err("价格生效时间无效".into());
        }
        for rate in [
            price.input_per_million_micros,
            price.output_per_million_micros,
            price.cache_read_per_million_micros,
            price.cache_creation_per_million_micros,
        ] {
            if rate.is_some_and(|value| value < 0) {
                return Err("价格不能为负数".into());
            }
        }
        let storage_key = price
            .endpoint_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|endpoint| format!("{endpoint}\u{1f}{model}"))
            .unwrap_or_else(|| model.to_owned());
        if !unique.insert((storage_key, price.effective_from.to_bits())) {
            return Err("同一 modelKey 与 effectiveFrom 不能重复".into());
        }
    }
    Ok(())
}

fn replace_pricing_database(
    connection: &mut Connection,
    update: RuntimePricingUpdate,
) -> Result<RuntimePricingMutation, String> {
    validate_pricing_update(&update)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_pricing_meta WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    if revision != update.expected_revision {
        return Err(format!(
            "pricing_revision_conflict: expected {}, current {revision}",
            update.expected_revision
        ));
    }
    let next_revision = revision.saturating_add(1);
    transaction
        .execute("DELETE FROM runtime_model_prices", [])
        .map_err(|error| error.to_string())?;
    let updated_at = now();
    for price in &update.prices {
        transaction
            .execute(
                "INSERT INTO runtime_model_prices(
                    model_key,effective_from,effective_to,input_per_million_micros,
                    output_per_million_micros,cache_read_per_million_micros,
                    cache_creation_per_million_micros,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                params![
                    price
                        .endpoint_id
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .map(|endpoint| format!("{endpoint}\u{1f}{}", price.model_key.trim()))
                        .unwrap_or_else(|| price.model_key.trim().to_owned()),
                    price.effective_from,
                    price.effective_to,
                    price.input_per_million_micros,
                    price.output_per_million_micros,
                    price.cache_read_per_million_micros,
                    price.cache_creation_per_million_micros,
                    updated_at,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    let currency = update.currency.trim().to_owned();
    transaction
        .execute(
            "UPDATE runtime_pricing_meta SET revision=?1,currency=?2,updated_at=?3 WHERE id=1",
            params![next_revision, currency, updated_at],
        )
        .map_err(|error| error.to_string())?;
    // Pricing is part of the rollup contract.  Never serve a bucket whose
    // cost was computed with the previous revision: clear the materialized
    // rows, mark every retained terminal-event bucket dirty, and let the
    // normal worker rebuild it before a rollup-backed trend is considered
    // complete.  The original event projections remain untouched.
    transaction
        .execute("DELETE FROM runtime_hourly_rollups", [])
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
             SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
             FROM runtime_events WHERE is_in_flight=0 AND kind IN ('client','upstream')",
            [],
        )
        .map_err(|error| error.to_string())?;
    let dirty = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    set_meta(
        &transaction,
        "hourly_rollup_complete",
        if dirty { 0 } else { 1 },
    )
    .map_err(|error| error.to_string())?;
    set_meta(&transaction, "hourly_rollup_max_seq", 0).map_err(|error| error.to_string())?;
    set_meta(&transaction, "hourly_rollup_failed", 0).map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(RuntimePricingMutation {
        revision: next_revision,
        currency,
        price_count: update.prices.len(),
    })
}

fn reset_database(inner: &Arc<Inner>, connection: &mut Connection) -> Result<i64, rusqlite::Error> {
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM runtime_events", [])?;
    transaction.execute("DELETE FROM runtime_hourly_rollups", [])?;
    transaction.execute("DELETE FROM runtime_hourly_rollup_dirty", [])?;
    transaction.execute("UPDATE runtime_counters SET client_requests=0,client_successes=0,client_failures=0,upstream_attempts=0,upstream_successes=0,upstream_failures=0,failovers=0 WHERE id=1", [])?;
    let generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "reset_generation", generation)?;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let next_seq = meta_i64(&transaction, "next_seq")?.unwrap_or(1);
    set_meta(&transaction, "retained_from_seq", next_seq)?;
    set_meta(&transaction, "retained_event_count", 0)?;
    set_meta(&transaction, "projection_backfill_cursor", 0)?;
    set_meta(&transaction, "projection_backfill_complete", 1)?;
    set_meta(&transaction, "projection_backfill_failed", 0)?;
    set_meta(&transaction, "hourly_rollup_complete", 1)?;
    set_meta(&transaction, "hourly_rollup_max_seq", 0)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    set_meta(&transaction, "hourly_rollup_failed", 0)?;
    for key in ["user_deleted_events", "user_deleted_requests"] {
        set_meta(&transaction, key, 0)?;
    }
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    inner.state.lock().unwrap().last_commit_at = Some(now());
    Ok(generation)
}

fn validate_cleanup_cutoff(older_than: f64) -> Result<(), String> {
    if !older_than.is_finite() || older_than <= 0.0 {
        return Err("olderThan 必须是大于 0 的有限时间戳".into());
    }
    Ok(())
}

/// Select only complete request groups whose newest event is older than the
/// cutoff. Requests with any in-flight row remain wholly intact; rows without
/// a request ID are treated as independent events.
///
/// 一行是 `(event_id, request_id, kind, timestamp)`。
type ExpiredEventRow = (String, Option<String>, String, f64);

fn cleanup_before_ids(
    connection: &Connection,
    older_than: f64,
) -> rusqlite::Result<Vec<ExpiredEventRow>> {
    let mut statement = connection.prepare(
        "SELECT event_id,request_id,kind,timestamp
         FROM runtime_events AS candidate
         WHERE candidate.is_in_flight=0
           AND (
               candidate.request_id IS NULL
               OR trim(candidate.request_id)=''
               OR (
                   NOT EXISTS(
                       SELECT 1 FROM runtime_events AS newer
                       WHERE newer.request_id=candidate.request_id
                         AND newer.timestamp >= ?1
                   )
                   AND NOT EXISTS(
                       SELECT 1 FROM runtime_events AS active
                       WHERE active.request_id=candidate.request_id
                         AND active.is_in_flight=1
                   )
               )
           )
           AND candidate.timestamp < ?1
         ORDER BY candidate.timestamp ASC,candidate.seq ASC",
    )?;
    statement
        .query_map(params![older_than], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect()
}

fn cleanup_before_preview_database(
    connection: &Connection,
    older_than: f64,
) -> rusqlite::Result<RuntimeCleanupPreview> {
    let deletable = cleanup_before_ids(connection, older_than)?;
    let deletable_events = deletable.len() as i64;
    let deletable_requests = deletable
        .iter()
        .filter(|(_, _, kind, _)| kind == KIND_CLIENT)
        .map(|(event_id, request_id, _, _)| {
            request_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(event_id)
                .to_owned()
        })
        .collect::<HashSet<_>>()
        .len() as i64;
    let retained = connection.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(RuntimeCleanupPreview {
        older_than,
        deletable_events,
        deletable_requests,
        remaining_events: retained.saturating_sub(deletable_events),
    })
}

fn cleanup_before_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    older_than: f64,
) -> rusqlite::Result<RuntimeCleanupMutation> {
    let transaction = connection.transaction()?;
    let deletable = cleanup_before_ids(&transaction, older_than)?;
    if deletable.is_empty() {
        let remaining_events =
            transaction.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0);
        transaction.commit()?;
        return Ok(RuntimeCleanupMutation {
            older_than,
            deleted_events: 0,
            deleted_requests: 0,
            remaining_events,
            history_generation,
        });
    }
    let mut deleted_ids = Vec::with_capacity(deletable.len());
    for (event_id, request_id, kind, timestamp) in &deletable {
        if let Some(request_id) = request_id.as_deref().filter(|id| !id.trim().is_empty()) {
            mark_request_hourly_rollups_dirty(&transaction, request_id)?;
        } else if kind == KIND_CLIENT {
            mark_hourly_rollup_bucket(&transaction, hourly_bucket_start(*timestamp))?;
        }
        transaction.execute(
            "DELETE FROM runtime_events WHERE event_id=?1 AND is_in_flight=0",
            params![event_id],
        )?;
        deleted_ids.push(event_id.clone());
    }
    let counters = counters_from_connection(&transaction)?;
    transaction.execute(
        "UPDATE runtime_counters SET client_requests=?1,client_successes=?2,client_failures=?3,
         upstream_attempts=?4,upstream_successes=?5,upstream_failures=?6,failovers=?7 WHERE id=1",
        params![
            counters.client_requests,
            counters.client_successes,
            counters.client_failures,
            counters.upstream_attempts,
            counters.upstream_successes,
            counters.upstream_failures,
            counters.failovers,
        ],
    )?;
    let deleted_events = deleted_ids.len() as i64;
    let deleted_requests = deletable
        .iter()
        .filter(|(_, _, kind, _)| kind == KIND_CLIENT)
        .map(|(event_id, request_id, _, _)| {
            request_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(event_id)
                .to_owned()
        })
        .collect::<HashSet<_>>()
        .len() as i64;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let retained_events =
        transaction.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
    set_meta(&transaction, "retained_event_count", retained_events)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    let user_deleted_events = meta_i64(&transaction, "user_deleted_events")?
        .unwrap_or(0)
        .saturating_add(deleted_events);
    let user_deleted_requests = meta_i64(&transaction, "user_deleted_requests")?
        .unwrap_or(0)
        .saturating_add(deleted_requests);
    set_meta(&transaction, "user_deleted_events", user_deleted_events)?;
    set_meta(&transaction, "user_deleted_requests", user_deleted_requests)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    Ok(RuntimeCleanupMutation {
        older_than,
        deleted_events,
        deleted_requests,
        remaining_events: retained_events,
        history_generation,
    })
}

/// Drop and recreate every runtime-owned table in place. Keeping the same
/// path avoids invalidating readers while still guaranteeing that removed
/// legacy columns cannot survive the explicit user cutover.
fn recreate_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
) -> Result<i64, rusqlite::Error> {
    let previous_generation = meta_i64(connection, "reset_generation")?.unwrap_or(0);
    let previous_history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0);
    connection.execute_batch(
        "DROP TABLE IF EXISTS runtime_events;
         DROP TABLE IF EXISTS runtime_hourly_rollup_dirty;
         DROP TABLE IF EXISTS runtime_hourly_rollups;
         DROP TABLE IF EXISTS runtime_model_prices;
         DROP TABLE IF EXISTS runtime_pricing_meta;
         DROP TABLE IF EXISTS runtime_retention;
         DROP TABLE IF EXISTS runtime_counters;
         DROP TABLE IF EXISTS runtime_meta;",
    )?;
    setup_connection(connection)?;
    let generation = previous_generation.saturating_add(1);
    let history_generation = previous_history_generation.saturating_add(1);
    set_meta(connection, "reset_generation", generation)?;
    set_meta(connection, "history_generation", history_generation)?;
    set_meta(
        connection,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    // Reclaim pages from the removed tables so this is a real fresh database,
    // not merely a schema reset with old payload pages left on the freelist.
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    refresh_cached_storage(inner, connection, true)?;
    inner.state.lock().unwrap().last_commit_at = Some(now());
    Ok(generation)
}

fn decode_runtime_event(payload: &str) -> rusqlite::Result<RuntimeEvent> {
    serde_json::from_str(payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn counters_from_connection(connection: &Connection) -> rusqlite::Result<RuntimeCounters> {
    connection.query_row(
        "SELECT
            SUM(CASE WHEN kind=?1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND outcome='succeeded' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND outcome='failed' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 AND outcome='succeeded' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 AND outcome='failed' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND COALESCE(failover,0)<>0 THEN 1 ELSE 0 END)
         FROM runtime_events WHERE is_in_flight=0",
        params![KIND_CLIENT, KIND_UPSTREAM],
        |row| {
            Ok(RuntimeCounters {
                client_requests: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                client_successes: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                client_failures: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                upstream_attempts: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                upstream_successes: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                upstream_failures: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                failovers: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
            })
        },
    )
}

fn delete_session_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    session_id: &str,
) -> rusqlite::Result<SessionMutation> {
    let transaction = connection.transaction()?;
    let (client_event_ids, request_ids) = {
        let mut statement = transaction.prepare(
            "SELECT event_id,request_id FROM runtime_events
             WHERE kind=?1 AND session_key=?2",
        )?;
        let rows = statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        (
            rows.iter().map(|row| row.0.clone()).collect::<HashSet<_>>(),
            rows.into_iter()
                .filter_map(|row| row.1)
                .collect::<HashSet<_>>(),
        )
    };
    if client_event_ids.is_empty() {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    let mut delete_ids = client_event_ids.clone();
    for request_id in &request_ids {
        mark_request_hourly_rollups_dirty(&transaction, request_id)?;
        let mut statement =
            transaction.prepare("SELECT event_id FROM runtime_events WHERE request_id=?1")?;
        delete_ids.extend(
            statement
                .query_map(params![request_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    for event_id in &delete_ids {
        transaction.execute(
            "DELETE FROM runtime_events WHERE event_id=?1",
            params![event_id],
        )?;
    }
    let counters = counters_from_connection(&transaction)?;
    transaction.execute(
        "UPDATE runtime_counters SET client_requests=?1,client_successes=?2,client_failures=?3,
         upstream_attempts=?4,upstream_successes=?5,upstream_failures=?6,failovers=?7 WHERE id=1",
        params![
            counters.client_requests,
            counters.client_successes,
            counters.client_failures,
            counters.upstream_attempts,
            counters.upstream_successes,
            counters.upstream_failures,
            counters.failovers,
        ],
    )?;
    let generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "reset_generation", generation)?;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let remaining = meta_i64(&transaction, "retained_event_count")?
        .unwrap_or(0)
        .saturating_sub(delete_ids.len() as i64);
    set_meta(&transaction, "retained_event_count", remaining)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    let user_deleted_events = meta_i64(&transaction, "user_deleted_events")?
        .unwrap_or(0)
        .saturating_add(delete_ids.len() as i64);
    let user_deleted_requests = meta_i64(&transaction, "user_deleted_requests")?
        .unwrap_or(0)
        .saturating_add(client_event_ids.len() as i64);
    set_meta(&transaction, "user_deleted_events", user_deleted_events)?;
    set_meta(&transaction, "user_deleted_requests", user_deleted_requests)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    Ok(SessionMutation {
        reset_generation: generation,
        deleted_events: delete_ids.len() as i64,
        deleted_requests: client_event_ids.len() as i64,
    })
}

fn export_session_json(connection: &Connection, session_id: &str) -> Result<Value, String> {
    let request_ids = {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT request_id FROM runtime_events
                 WHERE kind=?1 AND session_key=?2 AND request_id IS NOT NULL",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let mut selected = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT seq,change_seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND session_key=?2 ORDER BY seq ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                Ok(RuntimeChange {
                    seq: row.get(0)?,
                    change_seq: row.get(1)?,
                    event: decode_runtime_event(&row.get::<_, String>(2)?)?,
                })
            })
            .map_err(|error| error.to_string())?;
        selected.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
    }
    for request_id in &request_ids {
        let mut statement = connection
            .prepare(
                "SELECT seq,change_seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND request_id=?2 ORDER BY seq ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![KIND_UPSTREAM, request_id], |row| {
                Ok(RuntimeChange {
                    seq: row.get(0)?,
                    change_seq: row.get(1)?,
                    event: decode_runtime_event(&row.get::<_, String>(2)?)?,
                })
            })
            .map_err(|error| error.to_string())?;
        selected.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
    }
    selected.sort_by_key(|change| change.seq);
    if selected.is_empty() {
        return Err("会话不存在".into());
    }
    let selected_events = selected
        .iter()
        .map(|change| change.event.clone())
        .collect::<Vec<_>>();
    let project_keys = build_project_keys(&selected_events);
    let request_endpoints = build_request_endpoints(
        &selected
            .iter()
            .map(|change| change.event.clone())
            .collect::<Vec<_>>(),
    );
    let mut aggregate = AnalyticsAggregate {
        upstream_request_ids: request_ids.clone(),
        request_endpoints,
        ..AnalyticsAggregate::default()
    };
    for change in &selected {
        aggregate.add(&change.event, &project_keys);
    }
    let projects = selected
        .iter()
        .filter(|change| change.event.kind == KIND_CLIENT)
        .map(|change| project_key(&change.event, &project_keys))
        .collect::<BTreeSet<_>>();
    let client_kinds = selected
        .iter()
        .filter(|change| change.event.kind == KIND_CLIENT)
        .map(|change| client_key(&change.event))
        .collect::<BTreeSet<_>>();
    let events = selected
        .iter()
        .map(|change| json!({"seq": change.seq, "changeSeq": change.change_seq, "event": change.event}))
        .collect::<Vec<_>>();
    Ok(json!({
        "format": "sumpter-session-export-v1",
        "exportedAt": now(),
        "sessionID": session_id,
        "projects": projects,
        "clientKinds": client_kinds,
        "eventCount": events.len(),
        "analytics": {
            "clientRequests": aggregate.client_requests,
            "clientSuccesses": aggregate.client_successes,
            "clientFailures": aggregate.client_failures,
            "clientCancelled": aggregate.client_cancelled,
            "clientPending": aggregate.client_requests.saturating_sub(aggregate.client_successes + aggregate.client_failures + aggregate.client_cancelled),
            "upstreamAttempts": aggregate.upstream_attempts,
            "upstreamSuccesses": aggregate.upstream_successes,
            "upstreamFailures": aggregate.upstream_failures,
            "failovers": aggregate.failovers,
            "tokenUsage": aggregate.token_usage.value(),
            "endpoints": dimension_value(&aggregate.endpoints),
        },
        "events": events,
    }))
}

fn read_connection(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA cache_size = -2048;")
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

fn load_snapshot(connection: &Connection) -> Result<RuntimeSnapshot, String> {
    let mut snapshot = RuntimeSnapshot {
        client_requests: 0,
        client_successes: 0,
        client_failures: 0,
        failovers: 0,
        recent_events: Vec::new(),
        upstream_attempts: 0,
        upstream_successes: 0,
        upstream_failures: 0,
    };
    let counters = connection.query_row("SELECT client_requests,client_successes,client_failures,upstream_attempts,upstream_successes,upstream_failures,failovers FROM runtime_counters WHERE id=1", [], |row| Ok(RuntimeCounters { client_requests: row.get(0)?, client_successes: row.get(1)?, client_failures: row.get(2)?, upstream_attempts: row.get(3)?, upstream_successes: row.get(4)?, upstream_failures: row.get(5)?, failovers: row.get(6)? })).map_err(|e| e.to_string())?;
    snapshot.client_requests = counters.client_requests;
    snapshot.client_successes = counters.client_successes;
    snapshot.client_failures = counters.client_failures;
    snapshot.upstream_attempts = counters.upstream_attempts;
    snapshot.upstream_successes = counters.upstream_successes;
    snapshot.upstream_failures = counters.upstream_failures;
    snapshot.failovers = counters.failovers;
    let mut recent = Vec::new();
    for kind in [KIND_CLIENT, KIND_UPSTREAM, KIND_NOTIFY] {
        let mut statement = connection
            .prepare(
                "SELECT seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND is_in_flight=0 ORDER BY seq DESC LIMIT 200",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![kind], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            recent.push(row.map_err(|error| error.to_string())?);
        }
    }
    recent.sort_by_key(|(seq, _)| *seq);
    for (_, payload) in recent {
        let event: RuntimeEvent =
            serde_json::from_str(&payload).map_err(|error| error.to_string())?;
        snapshot.upsert_event(event);
    }
    Ok(snapshot)
}

fn database_file_sizes(path: &Path) -> (u64, u64) {
    let db_bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal).map(|meta| meta.len()).unwrap_or(0);
    (db_bytes, wal_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "sumpter-runtime-store-{label}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn event(
        id: &str,
        kind: &str,
        status_code: i64,
        phase: RuntimeEventPhase,
        outcome: Option<RuntimeEventOutcome>,
        timestamp: f64,
    ) -> RuntimeEvent {
        serde_json::from_value(json!({
            "id": id,
            "kind": kind,
            "durationMS": 25,
            "failover": false,
            "statusCode": status_code,
            "timestamp": timestamp,
            "phase": phase,
            "outcome": outcome,
        }))
        .unwrap()
    }

    fn remove_test_dir(path: &Path) {
        for _ in 0..20 {
            if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn unknown_token_accounting_is_not_reported_as_mixed() {
        assert_eq!(
            merge_semantics(
                TokenAccountingSemantics::Subset,
                TokenAccountingSemantics::Unknown
            )
            .as_str(),
            "unknown"
        );
        assert_eq!(
            merge_semantics(
                TokenAccountingSemantics::Unknown,
                TokenAccountingSemantics::Independent
            )
            .as_str(),
            "unknown"
        );
        assert_eq!(
            merge_semantics(
                TokenAccountingSemantics::Subset,
                TokenAccountingSemantics::Independent
            )
            .as_str(),
            "mixed"
        );
    }

    #[test]
    fn cleanup_before_deletes_only_complete_old_request_groups() {
        let dir = test_dir("cleanup-before");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let now = event_now();
        for (id, request_id, kind, timestamp, phase) in [
            (
                "old-client",
                Some("old-request"),
                KIND_CLIENT,
                now - 200.0,
                RuntimeEventPhase::Completed,
            ),
            (
                "old-upstream",
                Some("old-request"),
                KIND_UPSTREAM,
                now - 190.0,
                RuntimeEventPhase::Completed,
            ),
            (
                "mixed-client",
                Some("mixed-request"),
                KIND_CLIENT,
                now - 200.0,
                RuntimeEventPhase::Completed,
            ),
            (
                "mixed-upstream",
                Some("mixed-request"),
                KIND_UPSTREAM,
                now - 10.0,
                RuntimeEventPhase::Completed,
            ),
            (
                "active-client",
                Some("active-request"),
                KIND_CLIENT,
                now - 200.0,
                RuntimeEventPhase::Completed,
            ),
            (
                "active-upstream",
                Some("active-request"),
                KIND_UPSTREAM,
                now - 190.0,
                RuntimeEventPhase::InFlight,
            ),
        ] {
            let mut value = event(
                id,
                kind,
                200,
                phase,
                if phase == RuntimeEventPhase::Completed {
                    Some(RuntimeEventOutcome::Succeeded)
                } else {
                    None
                },
                timestamp,
            );
            value.request_id = request_id.map(str::to_owned);
            store.enqueue(value, RuntimeCounters::default()).unwrap();
        }
        store.flush().unwrap();

        let cutoff = now - 100.0;
        let preview = store.cleanup_before_preview(cutoff).unwrap();
        assert_eq!(preview.deletable_events, 2);
        assert_eq!(preview.deletable_requests, 1);
        let mutation = store.cleanup_before(cutoff).unwrap();
        assert_eq!(mutation.deleted_events, 2);
        assert_eq!(mutation.deleted_requests, 1);
        assert!(store.event("old-client").unwrap().is_none());
        assert!(store.event("old-upstream").unwrap().is_none());
        assert!(store.event("mixed-client").unwrap().is_some());
        assert!(store.event("active-upstream").unwrap().is_some());
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn schema_bootstrap_is_private_and_integral() {
        let dir = test_dir("schema");
        let path = dir.join("runtime.sqlite3");
        let (store, snapshot) = RuntimeStore::new(&path).unwrap();
        assert_eq!(snapshot, RuntimeSnapshot::default());
        store.flush().unwrap();
        let summary = store.summary();
        assert_eq!(summary.storage.schema_version, 3);
        assert!(summary.storage.backfill_complete);
        assert!(summary.storage.indexes_ready);
        assert!(summary.storage.rollup_complete);
        assert_eq!(summary.storage.rollup_dirty_buckets, 0);

        let connection = Connection::open(&path).unwrap();
        assert_eq!(meta_i64(&connection, "schema_version").unwrap(), Some(3));
        assert_eq!(
            connection
                .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
        for table in [
            "runtime_hourly_rollups",
            "runtime_hourly_rollup_dirty",
            "runtime_retention",
            "runtime_pricing_meta",
            "runtime_model_prices",
        ] {
            assert!(
                connection
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM sqlite_master
                         WHERE type='table' AND name=?1)",
                        [table],
                        |row| row.get::<_, bool>(0),
                    )
                    .unwrap(),
                "missing table {table}"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(connection);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn schema_v2_migrates_in_batches_and_quarantines_bad_payloads() {
        let dir = test_dir("schema-v2-migration");
        let path = dir.join("runtime.sqlite3");
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','1');
                 CREATE TABLE runtime_counters(
                    id INTEGER PRIMARY KEY,client_requests INTEGER NOT NULL,
                    client_successes INTEGER NOT NULL,client_failures INTEGER NOT NULL,
                    upstream_attempts INTEGER NOT NULL,upstream_successes INTEGER NOT NULL,
                    upstream_failures INTEGER NOT NULL,failovers INTEGER NOT NULL);
                 INSERT INTO runtime_counters VALUES(1,0,0,0,0,0,0,0);
                 CREATE TABLE runtime_events(
                    seq INTEGER PRIMARY KEY,change_seq INTEGER NOT NULL UNIQUE,
                    event_id TEXT NOT NULL UNIQUE,request_id TEXT,timestamp REAL NOT NULL,
                    kind TEXT NOT NULL,phase TEXT,outcome TEXT,status_code INTEGER NOT NULL,
                    client_kind TEXT,request_purpose TEXT,endpoint_id TEXT,failure_kind TEXT,
                    is_in_flight INTEGER NOT NULL,payload_json TEXT NOT NULL,
                    created_at REAL NOT NULL,updated_at REAL NOT NULL);",
            )
            .unwrap();
        let mut valid = event(
            "legacy-valid",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        valid.session_id = Some("legacy-session".into());
        let valid_payload = serde_json::to_string(&valid).unwrap();
        connection
            .execute(
                "INSERT INTO runtime_events VALUES(
                    1,1,'legacy-valid',NULL,?1,'client','completed','succeeded',200,
                    NULL,NULL,NULL,NULL,0,?2,0,0)",
                params![valid.timestamp, valid_payload],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO runtime_events VALUES(
                    2,2,'legacy-bad',NULL,?1,'client','completed','failed',500,
                    NULL,NULL,NULL,NULL,0,'{bad-json',0,0)",
                params![valid.timestamp],
            )
            .unwrap();

        setup_connection(&mut connection).unwrap();
        assert_eq!(meta_i64(&connection, "schema_version").unwrap(), Some(3));
        assert_eq!(
            connection
                .query_row(
                    "SELECT projection_version FROM runtime_events WHERE seq=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        while run_projection_maintenance(&mut connection).unwrap() {}
        assert_eq!(
            connection
                .query_row(
                    "SELECT projection_version FROM runtime_events WHERE seq=1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            PROJECTION_VERSION
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT projection_version FROM runtime_events WHERE seq=2",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            -1
        );
        assert_eq!(
            meta_i64(&connection, "projection_backfill_failed").unwrap(),
            Some(1)
        );
        assert_eq!(
            meta_i64(&connection, "projection_indexes_ready").unwrap(),
            Some(1)
        );
        assert_eq!(
            meta_i64(&connection, "hourly_rollup_complete").unwrap(),
            Some(1)
        );
        assert!(!run_projection_maintenance(&mut connection).unwrap());
        drop(connection);
        remove_test_dir(&dir);
    }

    #[test]
    fn projection_preserves_null_zero_and_protocol_cache_semantics() {
        use sumpter_core::config::ProviderProtocol;

        let dir = test_dir("projection-token-semantics");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut openai = event(
            "projection-openai",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        openai.target_format = Some(ProviderProtocol::OpenAIResponses);
        openai.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 100, "outputTokens": 0, "cacheReadInputTokens": 60}
        }))
        .ok();
        store.enqueue(openai, RuntimeCounters::default()).unwrap();

        let mut anthropic = event(
            "projection-anthropic",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        anthropic.target_format = Some(ProviderProtocol::Anthropic);
        anthropic.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 10, "cacheReadInputTokens": 5, "cacheCreationInputTokens": 2}
        }))
        .ok();
        store
            .enqueue(anthropic, RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();
        let connection = Connection::open(&path).unwrap();
        let openai_row = connection
            .query_row(
                "SELECT output_tokens,cache_creation_input_tokens,processed_input_tokens,
                        uncached_input_tokens,token_accounting_semantics,
                        token_accounting_quality FROM runtime_events WHERE event_id=?1",
                ["projection-openai"],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(openai_row.0, Some(0), "显式 0 不能退化成 NULL");
        assert_eq!(openai_row.1, None, "上游缺失字段必须保留 NULL");
        assert_eq!(openai_row.2, Some(100));
        assert_eq!(openai_row.3, Some(40));
        assert_eq!(openai_row.4, "subset");
        assert_eq!(openai_row.5, "complete");
        let anthropic_row = connection
            .query_row(
                "SELECT output_tokens,processed_input_tokens,uncached_input_tokens,
                        token_accounting_semantics,token_accounting_quality
                 FROM runtime_events WHERE event_id=?1",
                ["projection-anthropic"],
                |row| {
                    Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(anthropic_row.0, None);
        assert_eq!(anthropic_row.1, Some(17));
        assert_eq!(anthropic_row.2, Some(10));
        assert_eq!(anthropic_row.3, "independent");
        assert_eq!(anthropic_row.4, "partial");
        drop(connection);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn sqlite_roundtrip_preserves_source_session_attribution() {
        let dir = test_dir("source-session-roundtrip");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut value = event(
            "source-session-event",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.session_id = Some("session-header-source".into());
        value.codex_metadata = serde_json::from_value(json!({
            "installationID": "sha256:display-only",
            "sourceInstallationID": "install-source-value",
            "sessionID": "codex-session-source",
            "threadID": "thread-source",
            "turnID": "turn-source",
            "sourceWorkspacePaths": ["/Users/kkl/Documents/automode-proxy"],
        }))
        .ok();
        value.client_declared = serde_json::from_value(json!({
            "project": ".../automode-proxy",
            "workspace": ".../.claude/automode-proxy",
            "sourceProject": "automode-proxy",
            "sourceWorkspace": "/Users/kkl/.claude/automode-proxy",
        }))
        .ok();
        store.enqueue(value, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();
        drop(store);

        let (reopened, _) = RuntimeStore::new(&path).unwrap();
        let stored = reopened
            .event("source-session-event")
            .unwrap()
            .expect("source event survives reopen")
            .event;
        let codex = stored.codex_metadata.expect("codex source metadata");
        assert_eq!(
            codex.source_installation_id.as_deref(),
            Some("install-source-value")
        );
        assert_eq!(codex.session_id.as_deref(), Some("codex-session-source"));
        assert_eq!(codex.thread_id.as_deref(), Some("thread-source"));
        assert_eq!(codex.turn_id.as_deref(), Some("turn-source"));
        assert_eq!(
            codex.source_workspace_paths,
            vec!["/Users/kkl/Documents/automode-proxy"]
        );
        let declared = stored.client_declared.expect("client source metadata");
        assert_eq!(declared.source_project.as_deref(), Some("automode-proxy"));
        assert_eq!(
            declared.source_workspace.as_deref(),
            Some("/Users/kkl/.claude/automode-proxy")
        );

        let connection = Connection::open(&path).unwrap();
        let payload: String = connection
            .query_row(
                "SELECT payload_json FROM runtime_events WHERE event_id=?1",
                ["source-session-event"],
                |row| row.get(0),
            )
            .unwrap();
        assert!(payload.contains("install-source-value"));
        assert!(payload.contains("/Users/kkl/.claude/automode-proxy"));
        drop(connection);
        drop(reopened);
        remove_test_dir(&dir);
    }

    #[test]
    fn pending_upsert_keeps_seq_and_advances_change_seq() {
        let dir = test_dir("upsert");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let first = store
            .enqueue(
                event(
                    "event-1",
                    KIND_CLIENT,
                    0,
                    RuntimeEventPhase::InFlight,
                    None,
                    event_now(),
                ),
                RuntimeCounters::default(),
            )
            .unwrap();
        let second = store
            .enqueue(
                event(
                    "event-1",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        assert_eq!(first.seq, second.seq);
        assert!(second.change_seq > first.change_seq);
        store.flush().unwrap();
        let stored = store.event("event-1").unwrap().unwrap();
        assert_eq!(stored.seq, first.seq);
        assert_eq!(stored.change_seq, second.change_seq);
        assert_eq!(stored.event.phase, Some(RuntimeEventPhase::Completed));
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn hourly_rollup_waits_for_terminal_event_and_never_double_counts_upsert() {
        use sumpter_core::config::ProviderProtocol;

        let dir = test_dir("hourly-rollup-upsert");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let timestamp = event_now();
        let mut in_flight = event(
            "rollup-upsert",
            KIND_CLIENT,
            0,
            RuntimeEventPhase::InFlight,
            None,
            timestamp,
        );
        in_flight.target_format = Some(ProviderProtocol::OpenAIResponses);
        store
            .enqueue(in_flight.clone(), RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();

        in_flight.phase = Some(RuntimeEventPhase::Completed);
        in_flight.outcome = Some(RuntimeEventOutcome::Succeeded);
        in_flight.status_code = 200;
        in_flight.duration_ms = 120;
        in_flight.ttfb_ms = Some(30);
        in_flight.stream_trace = serde_json::from_value(json!({
            "usage": {
                "inputTokens": 100,
                "outputTokens": 20,
                "cacheReadInputTokens": 60,
                "cacheCreationInputTokens": 0
            }
        }))
        .ok();
        store
            .enqueue(
                in_flight,
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        store.flush().unwrap();
        for _ in 0..200 {
            let connection = Connection::open(&path).unwrap();
            if meta_i64(&connection, "hourly_rollup_complete").unwrap() == Some(1) {
                break;
            }
            drop(connection);
            thread::sleep(Duration::from_millis(10));
        }
        let connection = Connection::open(&path).unwrap();
        let row = connection
            .query_row(
                "SELECT client_requests,client_successes,duration_count,duration_ms_sum,
                        input_tokens,output_tokens,cache_read_input_tokens,
                        processed_input_tokens,cache_read_token_numerator,
                        cache_read_token_denominator
                 FROM runtime_hourly_rollups WHERE bucket_start=?1",
                params![hourly_bucket_start(timestamp)],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, i64>(6)?,
                        row.get::<_, i64>(7)?,
                        row.get::<_, i64>(8)?,
                        row.get::<_, i64>(9)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row, (1, 1, 1, 120, 100, 20, 60, 100, 60, 100));
        assert_eq!(
            meta_i64(&connection, "hourly_rollup_max_seq").unwrap(),
            Some(1)
        );
        drop(connection);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn hourly_rollup_price_prefers_endpoint_rule_and_falls_back_to_global() {
        let make_price = |endpoint_id: Option<&str>, input_rate| RollupPrice {
            endpoint_id: endpoint_id.map(ToOwned::to_owned),
            model_key: "rollup-model".into(),
            effective_from: 0.0,
            effective_to: None,
            input_per_million_micros: Some(input_rate),
            output_per_million_micros: None,
            cache_read_per_million_micros: None,
            cache_creation_per_million_micros: None,
        };
        let prices = vec![make_price(None, 1), make_price(Some("endpoint-a"), 9)];
        assert_eq!(
            resolve_rollup_price(&prices, Some("endpoint-a"), "rollup-model", 10.0)
                .and_then(|price| price.input_per_million_micros),
            Some(9)
        );
        assert_eq!(
            resolve_rollup_price(&prices, Some("endpoint-b"), "rollup-model", 10.0)
                .and_then(|price| price.input_per_million_micros),
            Some(1)
        );
        assert_eq!(
            split_rollup_price_key("endpoint-a\u{1f}rollup-model".into()),
            (Some("endpoint-a".into()), "rollup-model".into())
        );
    }

    #[test]
    fn retention_and_pricing_mutations_are_serialized_by_the_worker() {
        let dir = test_dir("retention-pricing-worker");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        for index in 0..3 {
            let mut value = event(
                &format!("retained-{index}"),
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now() + index as f64,
            );
            value.request_id = Some(format!("request-{index}"));
            store.enqueue(value, RuntimeCounters::default()).unwrap();
        }
        store.flush().unwrap();
        let retention = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: None,
                storage_limit_bytes: None,
            })
            .unwrap();
        assert_eq!(retention.revision, 2);
        let with_storage_limit = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 2,
                max_age_days: None,
                storage_limit_bytes: Some(8 * 1_048_576),
            })
            .unwrap();
        assert_eq!(with_storage_limit.revision, 3);
        assert_eq!(with_storage_limit.storage_limit_bytes, Some(8 * 1_048_576));
        assert!(
            store
                .set_retention(RuntimeRetentionUpdate {
                    expected_revision: 1,
                    max_age_days: None,
                    storage_limit_bytes: None,
                })
                .unwrap_err()
                .contains("retention_revision_conflict")
        );

        let pricing = store
            .replace_pricing(RuntimePricingUpdate {
                expected_revision: 1,
                currency: "USD".into(),
                prices: vec![RuntimeModelPriceInput {
                    endpoint_id: None,
                    model_key: "gpt-test".into(),
                    effective_from: 0.0,
                    effective_to: None,
                    input_per_million_micros: Some(1_000_000),
                    output_per_million_micros: Some(2_000_000),
                    cache_read_per_million_micros: Some(100_000),
                    cache_creation_per_million_micros: None,
                }],
            })
            .unwrap();
        assert_eq!(pricing.revision, 2);
        assert_eq!(pricing.price_count, 1);
        assert!(
            store
                .replace_pricing(RuntimePricingUpdate {
                    expected_revision: 1,
                    currency: "USD".into(),
                    prices: Vec::new(),
                })
                .unwrap_err()
                .contains("pricing_revision_conflict")
        );
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            3
        );
        assert_eq!(
            connection
                .query_row("SELECT model_key FROM runtime_model_prices", [], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap(),
            "gpt-test"
        );
        drop(connection);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn storage_limit_rotates_oldest_completed_request_groups_and_preserves_in_flight() {
        let dir = test_dir("storage-limit-rotation");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let old_timestamp = event_now() - 10_000.0;
        let mut old_client = event(
            "rotation-old-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            old_timestamp,
        );
        old_client.request_id = Some("rotation-old-request".into());
        old_client.message = Some("x".repeat(1_200_000));
        let mut old_upstream = old_client.clone();
        old_upstream.id = "rotation-old-upstream".into();
        old_upstream.kind = KIND_UPSTREAM.into();
        store
            .enqueue(old_client, RuntimeCounters::default())
            .unwrap();
        store
            .enqueue(old_upstream, RuntimeCounters::default())
            .unwrap();

        let mut retained = event(
            "rotation-retained",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        retained.request_id = Some("rotation-retained-request".into());
        store
            .enqueue(
                retained,
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();

        let mut in_flight = event(
            "rotation-in-flight",
            KIND_CLIENT,
            0,
            RuntimeEventPhase::InFlight,
            None,
            event_now() + 1.0,
        );
        in_flight.request_id = Some("rotation-live-request".into());
        store
            .enqueue(in_flight, RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();

        let mutation = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: None,
                storage_limit_bytes: Some(1_048_576),
            })
            .unwrap();
        assert_eq!(mutation.storage_limit_bytes, Some(1_048_576));
        assert!(store.event("rotation-old-client").unwrap().is_none());
        assert!(store.event("rotation-old-upstream").unwrap().is_none());
        assert!(store.event("rotation-retained").unwrap().is_some());
        assert!(store.event("rotation-in-flight").unwrap().is_some());
        let summary = store.summary();
        assert_eq!(summary.storage.in_flight_event_count, 1);
        assert_eq!(summary.counters.client_requests, 1);
        assert_eq!(summary.counters.client_successes, 1);

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn max_age_rotates_expired_completed_request_groups_and_keeps_fresh_rows() {
        let dir = test_dir("max-age-rotation");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let old_timestamp = event_now() - (3.0 * 86_400.0);
        let mut old_client = event(
            "age-old-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            old_timestamp,
        );
        old_client.request_id = Some("age-old-request".into());
        let mut old_upstream = old_client.clone();
        old_upstream.id = "age-old-upstream".into();
        old_upstream.kind = KIND_UPSTREAM.into();
        store
            .enqueue(old_client, RuntimeCounters::default())
            .unwrap();
        store
            .enqueue(old_upstream, RuntimeCounters::default())
            .unwrap();

        let mut fresh = event(
            "age-fresh-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        fresh.request_id = Some("age-fresh-request".into());
        store.enqueue(fresh, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let mutation = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: Some(1),
                storage_limit_bytes: None,
            })
            .unwrap();
        assert_eq!(mutation.max_age_days, Some(1));
        assert_eq!(mutation.storage_limit_bytes, None);
        assert!(store.event("age-old-client").unwrap().is_none());
        assert!(store.event("age-old-upstream").unwrap().is_none());
        assert!(store.event("age-fresh-client").unwrap().is_some());
        assert_eq!(store.summary().storage.event_count, 1);

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn max_age_protects_an_entire_request_group_while_any_event_is_in_flight() {
        let dir = test_dir("max-age-in-flight-group");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let old_timestamp = event_now() - (3.0 * 86_400.0);
        let request_id = "age-live-request";

        let mut completed = event(
            "age-live-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            old_timestamp,
        );
        completed.request_id = Some(request_id.into());
        store
            .enqueue(completed, RuntimeCounters::default())
            .unwrap();

        let mut active = event(
            "age-live-upstream",
            KIND_UPSTREAM,
            0,
            RuntimeEventPhase::InFlight,
            None,
            old_timestamp,
        );
        active.request_id = Some(request_id.into());
        store.enqueue(active, RuntimeCounters::default()).unwrap();

        let mut unrelated = event(
            "age-unrelated-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            old_timestamp,
        );
        unrelated.request_id = Some("age-unrelated-request".into());
        store
            .enqueue(unrelated, RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();

        store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: Some(1),
                storage_limit_bytes: None,
            })
            .unwrap();

        assert!(store.event("age-live-client").unwrap().is_some());
        assert!(store.event("age-live-upstream").unwrap().is_some());
        assert!(store.event("age-unrelated-client").unwrap().is_none());

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn max_age_can_be_disabled_without_deleting_existing_events() {
        let dir = test_dir("max-age-disable");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut old = event(
            "age-disabled-client",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now() - (3.0 * 86_400.0),
        );
        old.request_id = Some("age-disabled-request".into());
        store.enqueue(old, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let enabled = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: Some(1),
                storage_limit_bytes: None,
            })
            .unwrap();
        assert_eq!(enabled.max_age_days, Some(1));
        assert!(store.event("age-disabled-client").unwrap().is_none());

        let disabled = store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 2,
                max_age_days: None,
                storage_limit_bytes: None,
            })
            .unwrap();
        assert_eq!(disabled.max_age_days, None);

        let mut second_old = event(
            "age-disabled-client-2",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now() - (3.0 * 86_400.0),
        );
        second_old.request_id = Some("age-disabled-request-2".into());
        store
            .enqueue(second_old, RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();
        assert!(store.event("age-disabled-client-2").unwrap().is_some());

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn startup_detects_legacy_retention_without_migration() {
        let dir = test_dir("retention-startup-migration");
        let path = dir.join("runtime.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','3');
                 CREATE TABLE runtime_retention(
                    id INTEGER PRIMARY KEY CHECK(id=1),
                    revision INTEGER NOT NULL DEFAULT 1,
                    max_events INTEGER,
                    max_age_days INTEGER,
                    max_live_bytes INTEGER,
                    evicted_events INTEGER NOT NULL DEFAULT 0,
                    evicted_requests INTEGER NOT NULL DEFAULT 0,
                    last_pruned_at REAL,
                    over_limit INTEGER NOT NULL DEFAULT 0,
                    updated_at REAL NOT NULL);
                 INSERT INTO runtime_retention VALUES(1,7,10000,30,1048576,4,2,1234.0,1,1234.0);",
            )
            .unwrap();
        drop(connection);
        let (store, _) = RuntimeStore::new(&path).unwrap();

        let mut connection = Connection::open(&path).unwrap();
        connection.busy_timeout(Duration::from_secs(5)).unwrap();
        setup_connection(&mut connection).unwrap();

        let retention = connection
            .query_row(
                "SELECT revision
                 FROM runtime_retention WHERE id=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(
            retention, 7,
            "旧 revision 可保留，但旧自动清理值不得转成新容量提醒"
        );
        let legacy = crate::runtime_query::storage_details(&path).unwrap();
        assert!(legacy.legacy_retention_detected);
        assert_eq!(legacy.retention.storage_limit_bytes, None);

        drop(connection);
        store.reset().unwrap();
        let after_reset = crate::runtime_query::storage_details(&path).unwrap();
        assert!(after_reset.legacy_retention_detected);

        store.recreate().unwrap();
        let after_recreate = crate::runtime_query::storage_details(&path).unwrap();
        assert!(!after_recreate.legacy_retention_detected);
        let connection = Connection::open(&path).unwrap();
        let columns = connection
            .prepare("PRAGMA table_info(runtime_retention)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(columns, vec!["id", "revision", "updated_at"]);

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn reset_clears_rows_without_reusing_sequences_or_touching_stats_json() {
        let dir = test_dir("reset");
        let path = dir.join("runtime.sqlite3");
        let stats_path = dir.join("stats.json");
        std::fs::write(&stats_path, b"{\"archived\":true}\n").unwrap();
        let before = std::fs::metadata(&stats_path).unwrap().modified().unwrap();
        let before_bytes = std::fs::read(&stats_path).unwrap();
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let first = store
            .enqueue(
                event(
                    "event-before-reset",
                    KIND_CLIENT,
                    500,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Failed),
                    event_now(),
                ),
                RuntimeCounters {
                    client_requests: 1,
                    client_failures: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        store.flush().unwrap();
        assert_eq!(store.reset().unwrap(), 1);
        assert!(store.event("event-before-reset").unwrap().is_none());
        let second = store
            .enqueue(
                event(
                    "event-after-reset",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        assert!(second.seq > first.seq);
        assert!(second.change_seq > first.change_seq);
        store.flush().unwrap();
        assert_eq!(std::fs::read(&stats_path).unwrap(), before_bytes);
        assert_eq!(
            std::fs::metadata(&stats_path).unwrap().modified().unwrap(),
            before
        );
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn startup_normalizes_crash_leftovers_conservatively() {
        let dir = test_dir("startup");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        store
            .enqueue(
                event(
                    "no-headers",
                    KIND_CLIENT,
                    0,
                    RuntimeEventPhase::InFlight,
                    None,
                    event_now(),
                ),
                RuntimeCounters::default(),
            )
            .unwrap();
        store
            .enqueue(
                event(
                    "headers-seen",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::InFlight,
                    None,
                    event_now(),
                ),
                RuntimeCounters::default(),
            )
            .unwrap();
        store.flush().unwrap();
        drop(store);
        thread::sleep(Duration::from_millis(20));

        let (reopened, snapshot) = RuntimeStore::new(&path).unwrap();
        assert!(reopened.event("no-headers").unwrap().is_none());
        let normalized = reopened.event("headers-seen").unwrap().unwrap();
        assert_eq!(normalized.event.phase, Some(RuntimeEventPhase::Completed));
        assert_eq!(normalized.event.outcome, Some(RuntimeEventOutcome::Failed));
        assert_eq!(
            normalized.event.failure_kind,
            Some(RuntimeFailureKind::StreamInterrupted)
        );
        assert_eq!(snapshot.client_requests, 1);
        assert_eq!(snapshot.client_failures, 1);
        drop(reopened);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_uses_apple_epoch_and_streams_aggregation() {
        let dir = test_dir("analytics");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut value = event(
            "recent-event",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.endpoint_id = Some("primary-id".into());
        value.endpoint_name = Some("primary".into());
        value.target_format = Some(sumpter_core::config::ProviderProtocol::Anthropic);
        value.tool_calls = Some(vec!["shell".into()]);
        value.stream_trace = Some(
            serde_json::from_value(json!({
                "usage": {
                    "inputTokens": 12,
                    "outputTokens": 7,
                    "cacheReadInputTokens": 4,
                    "cacheCreationInputTokens": 2
                }
            }))
            .unwrap(),
        );
        value.codex_metadata = Some(
            serde_json::from_value(json!({
                "workspaces": {
                    ".../demo/automode-proxy": {
                        "associatedRemoteURLs": {
                            "origin": "https://github.com/example/sumpter.git"
                        }
                    }
                }
            }))
            .unwrap(),
        );
        store
            .enqueue(
                value,
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        store.flush().unwrap();
        let analytics = store.analytics("24h").unwrap();
        assert_eq!(analytics["clientRequests"], 1);
        assert_eq!(analytics["clientSuccesses"], 1);
        assert_eq!(analytics["endpoints"][0]["name"], "primary");
        assert_eq!(analytics["toolCalls"][0]["name"], "shell");
        assert_eq!(analytics["tokenUsage"]["inputTokens"], 12);
        assert_eq!(analytics["tokenUsage"]["outputTokens"], 7);
        assert_eq!(analytics["tokenUsage"]["totalTokens"], 19);
        assert_eq!(analytics["tokenUsage"]["processedInputTokens"], 18);
        assert_eq!(analytics["tokenUsage"]["processedTotalTokens"], 25);
        assert!(
            (analytics["tokenUsage"]["cacheReadTokenRate"]
                .as_f64()
                .unwrap()
                - (4.0 / 18.0))
                .abs()
                < 0.000_001
        );
        assert_eq!(analytics["tokenUsage"]["cacheReadRequestRate"], 1.0);
        assert_eq!(analytics["tokenUsage"]["cacheReadTokenEligibleRequests"], 1);
        assert_eq!(
            analytics["tokenUsage"]["tokenAccountingSemantics"],
            "independent"
        );
        assert_eq!(analytics["tokenUsage"]["observedRequests"], 1);
        assert_eq!(analytics["projects"][0]["name"], "automode-proxy");
        assert_eq!(analytics["projects"][0]["totalTokens"], 19);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_filters_client_project_session_and_uses_protocol_token_semantics() {
        use sumpter_core::config::ProviderProtocol;

        let dir = test_dir("analytics-filters");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();

        let mut claude = event(
            "client-claude",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        claude.request_id = Some("request-claude".into());
        claude.session_id = Some("session-claude-header".into());
        claude.client_kind = Some(ClientKind::ClaudeCode);
        claude.target_format = Some(ProviderProtocol::Anthropic);
        claude.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 14, "outputTokens": 4, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 13503}
        })).ok();
        claude.codex_metadata = serde_json::from_value(json!({
            "sessionID": "session-claude-metadata",
            "workspaces": {".../projects/project-a": {}}
        }))
        .ok();
        store.enqueue(claude, RuntimeCounters::default()).unwrap();

        let mut upstream = event(
            "upstream-claude",
            KIND_UPSTREAM,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        upstream.request_id = Some("request-claude".into());
        upstream.endpoint_name = Some("agent-domo".into());
        store.enqueue(upstream, RuntimeCounters::default()).unwrap();

        let mut codex = event(
            "client-codex",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        codex.request_id = Some("request-codex".into());
        codex.client_kind = Some(ClientKind::Codex);
        codex.target_format = Some(ProviderProtocol::OpenAIResponses);
        codex.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 100, "outputTokens": 20, "cacheReadInputTokens": 60, "cacheCreationInputTokens": 0, "reasoningTokens": 5}
        })).ok();
        codex.codex_metadata = serde_json::from_value(json!({
            "sessionID": "session-codex",
            "workspaces": {".../projects/project-b": {}}
        }))
        .ok();
        store.enqueue(codex, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let all = store.analytics("24h").unwrap();
        assert_eq!(all["tokenUsage"]["processedInputTokens"], 13_617);
        assert_eq!(all["tokenUsage"]["processedTotalTokens"], 13_641);
        assert_eq!(all["tokenUsage"]["uncachedInputTokens"], 54);
        assert_eq!(all["tokenUsage"]["reasoningTokens"], 5);
        assert_eq!(all["tokenUsage"]["tokenAccountingSemantics"], "mixed");
        assert_eq!(all["facets"]["clientKinds"].as_array().unwrap().len(), 2);

        let claude_only = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    client_kind: Some("claude_code".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(claude_only["clientRequests"], 1);
        assert_eq!(claude_only["upstreamAttempts"], 1);
        assert_eq!(claude_only["tokenUsage"]["processedInputTokens"], 13_517);
        assert_eq!(claude_only["projects"][0]["name"], "project-a");

        let claude_session = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    session_id: Some("session-claude-header".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(claude_session["clientRequests"], 1);
        assert_eq!(claude_session["upstreamAttempts"], 1);
        assert_eq!(
            claude_session["sessions"][0]["name"],
            "session-claude-header"
        );
        assert_eq!(
            claude_session["sessions"][0]["projects"],
            json!(["project-a"])
        );
        assert_eq!(
            claude_session["sessions"][0]["clientKinds"],
            json!(["claude_code"])
        );

        let codex_session = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    project: Some("project-b".into()),
                    session_id: Some("session-codex".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(codex_session["clientRequests"], 1);
        assert_eq!(codex_session["upstreamAttempts"], 0);
        assert_eq!(codex_session["tokenUsage"]["processedInputTokens"], 100);
        assert_eq!(codex_session["sessions"][0]["name"], "session-codex");
        assert_eq!(
            codex_session["sessions"][0]["projects"],
            json!(["project-b"])
        );

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_attributes_client_declared_projects_below_codex_workspaces() {
        let dir = test_dir("analytics-client-declared");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();

        // 只有客户端声明(Claude Code 的典型情形):项目名用声明值,来源标 client_declared,
        // 工作区仍只暴露脱敏尾段。
        let mut declared = event(
            "declared-only",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        declared.client_kind = Some(ClientKind::ClaudeCode);
        declared.client_declared = serde_json::from_value(json!({
            "project": "automode-proxy",
            "workspace": ".../.claude/automode-proxy",
            "gitRemote": "https://github.com/domoxiaojun/sumpter.git"
        }))
        .ok();
        store.enqueue(declared, RuntimeCounters::default()).unwrap();

        // 两者都在时 Codex 结构化 workspace 必须胜出,声明值不得覆盖它。
        let mut both = event(
            "codex-wins",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        both.client_kind = Some(ClientKind::Codex);
        both.codex_metadata = serde_json::from_value(json!({
            "workspaces": {"/Users/kkl/Documents/projects/codex-demo": {}}
        }))
        .ok();
        both.client_declared = serde_json::from_value(json!({"project": "declared-loser"})).ok();
        store.enqueue(both, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let analytics = store.analytics("24h").unwrap();
        let projects = analytics["projects"].as_array().expect("projects");
        let row = |name: &str| {
            projects
                .iter()
                .find(|row| row["name"] == name)
                .unwrap_or_else(|| panic!("缺少项目行 {name}: {projects:?}"))
                .clone()
        };

        let declared_row = row("automode-proxy");
        assert_eq!(declared_row["projectSource"], "client_declared");
        assert_eq!(
            declared_row["workspacePaths"],
            json!([".../.claude/automode-proxy"]),
            "声明的工作区只以脱敏尾段暴露"
        );

        let codex_row = row("codex-demo");
        assert_eq!(
            codex_row["projectSource"], "workspace_local",
            "Codex 结构化 workspace 优先,可信度词不被声明值降级"
        );
        assert!(
            !projects.iter().any(|row| row["name"] == "declared-loser"),
            "声明值不得在 Codex workspace 存在时另立项目行"
        );
    }

    #[test]
    fn analytics_reports_filter_state_usage_presence_and_project_source() {
        let dir = test_dir("analytics-contract-fields");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();

        let mut local = event(
            "contract-local",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        local.client_kind = Some(ClientKind::Codex);
        local.endpoint_id = Some("endpoint-local".into());
        local.stream_trace = serde_json::from_value(json!({
            "usage": {
                "inputTokens": 0,
                "outputTokens": 0,
                "cacheReadInputTokens": 0
            }
        }))
        .ok();
        local.codex_metadata = serde_json::from_value(json!({
            "workspaces": {"/Users/kkl/Documents/projects/local-demo": {}}
        }))
        .ok();
        store.enqueue(local, RuntimeCounters::default()).unwrap();

        let mut missing = event(
            "contract-missing",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        missing.client_kind = Some(ClientKind::Codex);
        missing.endpoint_id = Some("endpoint-other".into());
        missing.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 3}
        }))
        .ok();
        store.enqueue(missing, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let analytics = store.analytics("24h").unwrap();
        assert_eq!(analytics["filtersApplied"], true);
        assert_eq!(analytics["filterWarning"], Value::Null);
        assert_eq!(
            analytics["appliedFilters"],
            json!({
                "clientKind": null, "endpointID": null, "projectID": null, "project": null, "sessionID": null
            })
        );
        assert_eq!(analytics["tokenUsage"]["observedRequests"], 2);
        assert_eq!(
            analytics["tokenUsage"]["usageFieldPresence"],
            json!({
                "inputTokens": 2,
                "outputTokens": 1,
                "cacheReadInputTokens": 1,
                "cacheCreationInputTokens": 0,
                "reasoningTokens": 0
            })
        );
        let local_row = analytics["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "local-demo")
            .unwrap();
        assert_eq!(local_row["projectSource"], "workspace_local");
        assert_eq!(
            local_row["workspacePaths"],
            json!([".../projects/local-demo"])
        );
        let missing_row = analytics["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "unidentified_project")
            .unwrap();
        assert_eq!(missing_row["projectSource"], "missing_workspace_metadata");

        let filtered = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    client_kind: Some(" codex ".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(filtered["filtersApplied"], true);
        assert_eq!(filtered["appliedFilters"]["clientKind"], "codex");
        let endpoint_filtered = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    endpoint_id: Some("endpoint-local".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(endpoint_filtered["clientRequests"], 1);
        assert_eq!(
            endpoint_filtered["appliedFilters"]["endpointID"],
            "endpoint-local"
        );
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn unidentified_session_requires_explicit_confirmation_to_delete() {
        let dir = test_dir("delete-unidentified");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut unknown = event(
            "unknown-session-event",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        unknown.request_id = Some("unknown-session-request".into());
        store.enqueue(unknown, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();
        assert!(store.delete_session("unidentified_session").is_err());
        let mutation = store
            .delete_session_confirmed("unidentified_session", true)
            .unwrap();
        assert_eq!(mutation.deleted_requests, 1);
        assert_eq!(mutation.deleted_events, 1);
        assert_eq!(store.analytics("all").unwrap()["clientRequests"], 0);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_keeps_multiple_sessions_in_one_local_project_separate() {
        let dir = test_dir("analytics-session-project");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        for (id, session) in [
            ("client-session-a", "session-a"),
            ("client-session-b", "session-b"),
        ] {
            let mut value = event(
                id,
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            );
            value.session_id = Some(session.into());
            value.codex_metadata = serde_json::from_value(json!({
                "workspaces": {".../automode-proxy": {"associatedRemoteURLs": {"origin": "https://github.com/example/sumpter.git"}}}
            })).ok();
            store.enqueue(value, RuntimeCounters::default()).unwrap();
        }
        store.flush().unwrap();
        let analytics = store.analytics("24h").unwrap();
        let sessions = analytics["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(
            sessions
                .iter()
                .all(|row| row["projects"] == json!(["automode-proxy"]))
        );
        assert!(
            sessions
                .iter()
                .all(|row| row["clientKinds"] == json!(["unrecorded_client"]))
        );

        let selected_session = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    project: Some("automode-proxy".into()),
                    session_id: Some("session-a".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(selected_session["clientRequests"], 1);
        assert_eq!(
            selected_session["facets"]["sessions"],
            json!([
                {"value": "session-a", "count": 1},
                {"value": "session-b", "count": 1}
            ]),
            "session facet must ignore the selected session while respecting the project"
        );
        assert_eq!(
            selected_session["facets"]["projects"],
            json!([{"value": "automode-proxy", "count": 1}]),
            "project facet must respect the selected session"
        );

        let incompatible_selection = store
            .analytics_filtered(
                "24h",
                &AnalyticsFilter {
                    client_kind: Some("claude_code".into()),
                    project: Some("automode-proxy".into()),
                    session_id: Some("session-a".into()),
                    ..AnalyticsFilter::default()
                },
            )
            .unwrap();
        assert_eq!(incompatible_selection["clientRequests"], 0);
        assert_eq!(
            incompatible_selection["facets"]["clientKinds"],
            json!([
                {"value": "claude_code", "count": 0},
                {"value": "unrecorded_client", "count": 1}
            ])
        );
        assert_eq!(
            incompatible_selection["facets"]["projects"],
            json!([{"value": "automode-proxy", "count": 0}])
        );
        assert_eq!(
            incompatible_selection["facets"]["sessions"],
            json!([{"value": "session-a", "count": 0}])
        );
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_attributes_client_usage_to_final_endpoint_without_duplicate_attempts() {
        let dir = test_dir("analytics-endpoint-usage");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut client = event(
            "client-endpoint-usage",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        client.request_id = Some("request-endpoint-usage".into());
        client.session_id = Some("session-endpoint-usage".into());
        client.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 10, "outputTokens": 2, "cacheReadInputTokens": 4}
        }))
        .ok();
        store.enqueue(client, RuntimeCounters::default()).unwrap();

        for (id, endpoint, outcome, status) in [
            (
                "upstream-endpoint-a",
                "entry-a",
                RuntimeEventOutcome::Failed,
                502,
            ),
            (
                "upstream-endpoint-b",
                "entry-b",
                RuntimeEventOutcome::Succeeded,
                200,
            ),
        ] {
            let mut upstream = event(
                id,
                KIND_UPSTREAM,
                status,
                RuntimeEventPhase::Completed,
                Some(outcome),
                event_now(),
            );
            upstream.request_id = Some("request-endpoint-usage".into());
            upstream.endpoint_id = Some(endpoint.into());
            upstream.endpoint_name = Some(endpoint.into());
            store.enqueue(upstream, RuntimeCounters::default()).unwrap();
        }
        store.flush().unwrap();
        let analytics = store.analytics("24h").unwrap();
        assert_eq!(analytics["clientSuccessRate"], 100.0);
        assert_eq!(analytics["clientPending"], 0);
        assert_eq!(analytics["endpoints"].as_array().unwrap().len(), 2);
        assert_eq!(analytics["endpoints"][0]["name"], "entry-a");
        assert_eq!(analytics["endpoints"][0]["inputTokens"], 0);
        assert_eq!(analytics["endpoints"][1]["name"], "entry-b");
        assert_eq!(analytics["endpoints"][1]["attempts"], 1);
        assert_eq!(analytics["endpoints"][1]["inputTokens"], 10);
        assert_eq!(analytics["endpoints"][1]["cacheReadInputTokens"], 4);
        let mutation = store.delete_session("session-endpoint-usage").unwrap();
        assert_eq!(mutation.deleted_requests, 1);
        assert_eq!(mutation.deleted_events, 3);
        assert_eq!(store.analytics("24h").unwrap()["clientRequests"], 0);
        assert!(store.export_session("session-endpoint-usage").is_err());
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_attributes_usage_to_last_endpoint_when_all_attempts_fail() {
        let dir = test_dir("analytics-last-failed-endpoint-usage");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut client = event(
            "client-last-failed-usage",
            KIND_CLIENT,
            502,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Failed),
            event_now(),
        );
        client.request_id = Some("request-last-failed-usage".into());
        client.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 7, "outputTokens": 1}
        }))
        .ok();
        store.enqueue(client, RuntimeCounters::default()).unwrap();
        for (id, endpoint) in [
            ("upstream-failed-a", "entry-a"),
            ("upstream-failed-b", "entry-b"),
        ] {
            let mut upstream = event(
                id,
                KIND_UPSTREAM,
                502,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Failed),
                event_now(),
            );
            upstream.request_id = Some("request-last-failed-usage".into());
            upstream.endpoint_id = Some(endpoint.into());
            upstream.endpoint_name = Some(endpoint.into());
            store.enqueue(upstream, RuntimeCounters::default()).unwrap();
        }
        store.flush().unwrap();
        let analytics = store.analytics("24h").unwrap();
        assert_eq!(analytics["endpoints"][0]["name"], "entry-a");
        assert_eq!(analytics["endpoints"][1]["name"], "entry-b");
        assert_eq!(analytics["endpoints"][0]["inputTokens"], 0);
        assert_eq!(analytics["endpoints"][1]["inputTokens"], 7);
        assert_eq!(analytics["endpoints"][1]["outputTokens"], 1);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_keeps_unknown_protocol_unverifiable() {
        use sumpter_core::config::ProviderProtocol;

        let dir = test_dir("analytics-mixed-unknown");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();

        let mut unknown = event(
            "unknown-protocol",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        unknown.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 3, "outputTokens": 1}
        }))
        .ok();
        store.enqueue(unknown, RuntimeCounters::default()).unwrap();

        let mut anthropic = event(
            "known-protocol",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        anthropic.target_format = Some(ProviderProtocol::Anthropic);
        anthropic.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 5, "outputTokens": 2}
        }))
        .ok();
        store
            .enqueue(anthropic, RuntimeCounters::default())
            .unwrap();
        store.flush().unwrap();

        let analytics = store.analytics("24h").unwrap();
        assert_eq!(
            analytics["tokenUsage"]["tokenAccountingSemantics"],
            "unknown"
        );

        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn event_pages_support_keyset_cursors_filters_and_change_replay() {
        let dir = test_dir("pages");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let base = event_now();

        let mut first = event(
            "client-a",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            base - 2.0,
        );
        first.request_id = Some("request-a".into());
        let first_change = store.enqueue(first, RuntimeCounters::default()).unwrap();

        let mut second = event(
            "client-b",
            KIND_CLIENT,
            0,
            RuntimeEventPhase::InFlight,
            None,
            base - 1.0,
        );
        second.request_id = Some("request-b".into());
        let second_in_flight = store
            .enqueue(second.clone(), RuntimeCounters::default())
            .unwrap();
        second.status_code = 500;
        second.phase = Some(RuntimeEventPhase::Completed);
        second.outcome = Some(RuntimeEventOutcome::Failed);
        second.failure_kind = Some(RuntimeFailureKind::UpstreamHttpStatus);
        let second_completed = store.enqueue(second, RuntimeCounters::default()).unwrap();

        let mut third = event(
            "upstream-a",
            KIND_UPSTREAM,
            503,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Failed),
            base,
        );
        third.request_id = Some("request-a".into());
        let third_change = store.enqueue(third, RuntimeCounters::default()).unwrap();
        store.flush().unwrap();

        let newest = store
            .events(None, None, 2, None, None, None, None, None)
            .unwrap();
        assert_eq!(newest.len(), 3, "store returns limit + 1 for hasMore");
        assert_eq!(newest[0].id, "upstream-a");
        assert_eq!(newest[1].id, "client-b");

        let zero_cursor = store
            .events(Some(0), Some(0), 2, None, None, None, None, None)
            .unwrap();
        assert_eq!(zero_cursor[0].id, "upstream-a");

        let older = store
            .events(Some(newest[1].seq), None, 50, None, None, None, None, None)
            .unwrap();
        assert_eq!(
            older
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["client-a"]
        );

        let filtered = store
            .events(
                None,
                None,
                50,
                Some(KIND_CLIENT),
                Some("request-b"),
                Some("failed"),
                Some(base - 1.5),
                Some(base - 0.5),
            )
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "client-b");

        let changes = store
            .events(
                None,
                Some(first_change.change_seq),
                50,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(
            changes
                .iter()
                .map(|item| item.change_seq)
                .collect::<Vec<_>>(),
            vec![
                second_in_flight.change_seq,
                second_completed.change_seq,
                third_change.change_seq,
            ]
        );
        assert_eq!(changes[0].seq, second_in_flight.seq);
        assert_eq!(changes[1].seq, second_in_flight.seq);
        assert!(
            store
                .change_cursor_valid(Some(first_change.change_seq))
                .unwrap()
        );
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn change_window_keeps_the_newest_changes_and_rejects_expired_cursors() {
        let dir = test_dir("change-window");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        let mut first_change_seq = 0;
        let mut latest_change_seq = 0;
        for index in 0..(RECENT_CHANGES_LIMIT + 5) {
            let change = store
                .enqueue(
                    event(
                        &format!("window-{index}"),
                        KIND_CLIENT,
                        200,
                        RuntimeEventPhase::Completed,
                        Some(RuntimeEventOutcome::Succeeded),
                        event_now(),
                    ),
                    RuntimeCounters::default(),
                )
                .unwrap();
            if index == 0 {
                first_change_seq = change.change_seq;
            }
            latest_change_seq = change.change_seq;
        }
        assert!(!store.change_cursor_valid(Some(first_change_seq)).unwrap());
        assert!(
            store
                .change_cursor_valid(Some(latest_change_seq - 1))
                .unwrap()
        );
        let latest = store
            .events(
                None,
                Some(latest_change_seq - 1),
                50,
                None,
                None,
                None,
                None,
                None,
            )
            .unwrap();
        assert_eq!(latest.len(), 1);
        assert_eq!(latest[0].change_seq, latest_change_seq);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn analytics_excludes_in_flight_rows() {
        let dir = test_dir("analytics-in-flight");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        store
            .enqueue(
                event(
                    "pending-client",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::InFlight,
                    None,
                    event_now(),
                ),
                RuntimeCounters::default(),
            )
            .unwrap();
        store.flush().unwrap();
        let analytics = store.analytics("all").unwrap();
        assert_eq!(analytics["clientRequests"], 0);
        assert_eq!(analytics["latencyBuckets"]["under1s"], 0);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn newer_schema_is_rejected_before_writes() {
        let dir = test_dir("future-schema");
        let path = dir.join("runtime.sqlite3");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','4');",
            )
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();
        let error = match RuntimeStore::new(&path) {
            Ok(_) => panic!("future schema must be rejected"),
            Err(error) => error,
        };
        assert!(error.contains("newer than supported"));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        remove_test_dir(&dir);
    }

    #[test]
    fn successful_commit_clears_degraded_and_backpressure_state() {
        let dir = test_dir("recovery");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        store.inner.backpressure.store(true, Ordering::Release);
        store.inner.hard_backpressure.store(true, Ordering::Release);
        store.inner.state.lock().unwrap().last_error = Some("temporary failure".into());
        store
            .enqueue(
                event(
                    "recovery-event",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        store.flush().unwrap();
        let summary = store.summary();
        assert_eq!(summary.storage.state, "ready");
        assert!(!store.is_backpressured());
        assert!(summary.storage.last_error.is_none());
        assert_eq!(summary.storage.pending_events, 0);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn failed_flush_keeps_pending_and_recovers_without_losing_the_event() {
        let dir = test_dir("write-failure");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        store.set_test_write_failure(true);
        store
            .enqueue(
                event(
                    "write-failure-event",
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters {
                    client_requests: 1,
                    client_successes: 1,
                    ..RuntimeCounters::default()
                },
            )
            .unwrap();
        assert!(store.flush().is_err());
        let degraded = store.summary();
        assert_eq!(degraded.storage.state, "degraded");
        assert!(degraded.storage.pending_events > 0);

        store.set_test_write_failure(false);
        store.flush().unwrap();
        let recovered = store.summary();
        assert_eq!(recovered.storage.state, "ready");
        assert_eq!(recovered.storage.pending_events, 0);
        assert!(store.event("write-failure-event").unwrap().is_some());
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn hard_backpressure_is_bounded_and_clears_after_recovery() {
        let dir = test_dir("backpressure");
        let path = dir.join("runtime.sqlite3");
        let (store, _) = RuntimeStore::new(&path).unwrap();
        store.set_test_write_failure(true);
        let mut rejected = false;
        for index in 0..(PENDING_EVENTS_LIMIT + 128) {
            let result = store.enqueue(
                event(
                    &format!("backpressure-{index}"),
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters::default(),
            );
            if result.is_err() {
                rejected = true;
                break;
            }
        }
        assert!(rejected, "pending hard limit must reject new events");
        assert!(store.is_backpressured());
        let summary = store.summary();
        assert_eq!(summary.storage.state, "backpressure");
        assert!(summary.storage.pending_events <= PENDING_EVENTS_LIMIT);

        store.set_test_write_failure(false);
        store.flush().unwrap();
        assert!(!store.is_backpressured());
        assert_eq!(store.summary().storage.pending_events, 0);
        drop(store);
        remove_test_dir(&dir);
    }

    #[test]
    fn pending_batch_resets_local_byte_accounting_after_take() {
        let mut pending = PendingBatch::default();
        let message = WriteMessage {
            seq: 1,
            change_seq: 1,
            event: event(
                "pending-bytes",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            counters: RuntimeCounters::default(),
            bytes: 17,
        };
        let inner = Arc::new(Inner {
            path: PathBuf::from("pending-bytes.sqlite3"),
            sender: mpsc::sync_channel(1).0,
            state: Mutex::new(StoreState {
                next_seq: 1,
                next_change_seq: 1,
                reset_generation: 0,
                history_generation: 0,
                counters: RuntimeCounters::default(),
                active_sequences: HashMap::new(),
                recent_changes: VecDeque::new(),
                latest_event: None,
                last_commit_at: None,
                last_error: None,
                storage: CachedStorageMetrics::default(),
                storage_refreshed_at: std::time::Instant::now(),
            }),
            pending_events: AtomicUsize::new(0),
            pending_bytes: AtomicUsize::new(0),
            backpressure: AtomicBool::new(false),
            hard_backpressure: AtomicBool::new(false),
            #[cfg(test)]
            fail_writes: AtomicBool::new(false),
        });
        pending.upsert(message, &inner);
        assert_eq!(pending.bytes, 17);
        assert_eq!(pending.take().len(), 1);
        assert_eq!(pending.bytes, 0);
    }

    /// 列表投影是 macOS App 的主加载路径(单事件详情只在选中时才拉),漏掉客户端声明
    /// 那边就恒显示「未识别项目」。同时钉住 codexMetadata 仍在:两个归因来源并存,
    /// 不是互斥的。
    #[test]
    fn list_item_wire_carries_client_declared_project_attribution() {
        let mut value = event(
            "wire-declared",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.client_declared = serde_json::from_value(json!({
            "project": "automode-proxy",
            "workspace": ".../.claude/automode-proxy",
            "gitRemote": "https://github.com/domoxiaojun/sumpter.git",
        }))
        .ok();
        let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();

        assert_eq!(wire["clientDeclared"]["project"], "automode-proxy");
        assert_eq!(
            wire["clientDeclared"]["workspace"],
            ".../.claude/automode-proxy"
        );
        assert_eq!(
            wire["clientDeclared"]["gitRemote"],
            "https://github.com/domoxiaojun/sumpter.git"
        );
        assert!(
            wire.get("client_declared").is_none(),
            "列表投影只能是 camelCase 形状"
        );
    }

    #[test]
    fn list_item_wire_carries_codex_thread_scope() {
        let mut value = event(
            "wire-ambient",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.client_kind = Some(ClientKind::Codex);
        value.codex_metadata = serde_json::from_value(json!({
            "threadSource": "ambient_suggestion_safety"
        }))
        .ok();
        let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
        assert_eq!(wire["codexThreadClass"], "ambient");
        assert_eq!(wire["attributionScope"], "internal_feature");
    }

    #[test]
    fn resource_requests_are_not_attributed_to_unidentified_project() {
        let mut value = event(
            "wire-resource",
            KIND_CLIENT,
            499,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Cancelled),
            event_now(),
        );
        value.client_kind = Some(ClientKind::Codex);
        value.client_model = Some(RESOURCE_ROUTING_MODEL.into());
        value.codex_metadata = serde_json::from_value(json!({
            "originator": "Codex Desktop"
        }))
        .ok();

        let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
        assert_eq!(wire["projectName"], "internal_feature");
        assert_eq!(wire["projectSource"], "internal_feature");
        assert_eq!(wire["attributionScope"], "internal_feature");
    }

    #[test]
    fn list_item_wire_preserves_id_and_ms_acronyms() {
        let mut value = event(
            "wire-acronyms",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.request_id = Some("request-wire".into());
        value.session_id = Some("session-wire".into());
        value.endpoint_id = Some("endpoint-wire".into());
        value.pool_id = Some("primary".into());
        value.message = Some("bridge openai-responses".into());
        value.duration_ms = 42;
        value.ttfb_ms = Some(7);
        let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();

        assert_eq!(wire["requestID"], "request-wire");
        assert_eq!(wire["sessionID"], "session-wire");
        assert_eq!(wire["endpointID"], "endpoint-wire");
        assert_eq!(wire["message"], "bridge openai-responses");
        assert_eq!(wire["durationMS"], 42);
        assert_eq!(wire["ttfbMS"], 7);
        assert!(
            wire.get("poolID").is_none(),
            "新列表 wire 不得输出废弃 poolID"
        );
        for legacy in ["requestId", "endpointId", "durationMs", "ttfbMs"] {
            assert!(wire.get(legacy).is_none(), "unexpected wire key {legacy}");
        }
    }

    #[test]
    fn runtime_change_wire_omits_legacy_pool_id_from_detail_and_sse_shapes() {
        let mut value = event(
            "wire-detail-pool",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.pool_id = Some("primary".into());
        let wire = serde_json::to_value(RuntimeChange {
            seq: 7,
            change_seq: 8,
            event: value,
        })
        .unwrap();
        assert!(wire["event"].get("poolID").is_none());
        assert!(wire["event"].get("poolId").is_none());
    }
}
