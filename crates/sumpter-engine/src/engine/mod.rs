//! 代理引擎:入站分发、鉴权、路由、failover/跨轮重试、流式 relay、桥接、
//! 统计与事件记账。对齐 Swift `ProxyEngine`(specs/spec-engine.md §1/§3/§5/§6)。
//!
//! 取消语义:axum 连接断开 → 响应体流被 drop → relay 状态(含上游连接)随之 drop,
//! 上游请求与重试循环立即撕停;`CompletionGuard` 在 Drop 时补记 499 事件
//! (不计成功也不计失败)。这是「退出 CC 后仍疯狂请求」历史事故的根治点。

/// Public module seams for the staged engine.  The implementation remains in
/// this `mod.rs` during the parallel migration; these seams let facades and
/// replay tooling depend on stable contracts without copying the engine.
pub mod capture;
pub mod events;
pub mod forward;
pub mod inbound;
pub mod lifecycle;

use std::cell::RefCell;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Body;
use axum::extract::ws::{Message as WebSocketMessage, WebSocket};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

use sumpter_core::bridge::{self, SseBridge};
use sumpter_core::bridge_in::{self, ClientDialect};
use sumpter_core::config::{AppConfig, ProviderProtocol, RetryPolicy};
use sumpter_core::config_store::{ConfigDir, ResourceBinding, StickySessionAssignment};
use sumpter_core::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, DiagnosticAttemptCapture,
    DiagnosticCaptureSnapshot, DiagnosticChunk, DiagnosticHeader, DiagnosticRequestCapture,
    GrokMetadata, KIND_CLIENT, KIND_UPSTREAM, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase,
    RuntimeFailureKind, RuntimeFailurePhase, RuntimeSnapshot, STATUS_CLIENT_DISCONNECTED,
    StreamTrace, WebSocketTrace, message_tokens, unix_to_apple_epoch,
};
use sumpter_core::routing::{
    PlannedEndpoint, RequestPurpose, RouteMode, RoutePlanError, RoutePlanner, RoutingRequest,
    inspector, sticky,
};
use sumpter_core::stream_terminal::{SseDialect, SseTerminal, SseTerminalTracker};
use sumpter_core::{access, scheduler};

use crate::boundary::{
    AccessDecision, ConfigReplacement, ControlRequestMeta, EngineCapabilities, EngineServices,
    InboundRequest, PlatformBoundary, PlatformNotice, PlatformRequest,
};
use crate::outbound::{TransportError, UpstreamTransport};
use crate::request_build::{self, PassthroughKind, PassthroughRequest};
use crate::runtime_query::{
    self, DimensionKind, DimensionPageQuery, ErrorPageQuery, EventPageQuery, ExportManifest,
    ExportQuery, RuntimeFilter, RuntimeQueryError, TrendQuery,
};
use crate::runtime_store::{
    AnalyticsFilter, RuntimeCleanupMutation, RuntimeCleanupPreview, RuntimeCounters,
    RuntimeEventListItem, RuntimePricingUpdate, RuntimeRetentionUpdate, RuntimeStore,
};

const IP_COOLDOWN_SECS: f64 = 60.0;
const SESSION_STICKY_MAX_ENTRIES: usize = 2000;
/// 跨轮重试退避：沿用旧 Python 版验证过的节奏，防止 0/0 无限重试在全故障时
/// 形成 busy loop。sleep future 被 drop 即取消，客户端断开不会留下后台重试。
const RETRY_BACKOFF_INITIAL_SECS: f64 = 0.5;
const RETRY_BACKOFF_FACTOR: f64 = 1.7;
const RETRY_BACKOFF_MAX_SECS: f64 = 30.0;
/// Native Realtime/Live bootstraps must not wait forever for response headers.
/// This is intentionally separate from the user-configurable ordinary request
/// timeout: `null` keeps the historical "client decides" behavior for text
/// and streaming APIs, while voice setup needs a proxy-side safety bound so a
/// dead upstream cannot leave an `inFlight` event forever.
const REALTIME_RESPONSE_TIMEOUT_SECS: f64 = 15.0;
/// A WebSocket upgrade has no HTTP response-body relay where the ordinary
/// response-header timeout can be applied, so protect the upstream TCP/TLS/
/// handshake independently.
const REALTIME_WEBSOCKET_CONNECT_TIMEOUT_SECS: f64 = 15.0;
/// Provider/model health is intentionally shorter-lived than the persisted
/// session affinity.  A transient 429/5xx should move traffic away from the
/// bad mapping, but it must not make a provider disappear until the sidecar
/// is restarted.
const PROVIDER_MODEL_COOLDOWN_INITIAL_SECS: f64 = 5.0;
const PROVIDER_MODEL_COOLDOWN_FACTOR: f64 = 2.0;
const PROVIDER_MODEL_COOLDOWN_MAX_SECS: f64 = 60.0;
const MAX_PROVIDER_MODEL_HEALTH: usize = 4096;
const MAX_RETRY_AFTER_SECS: f64 = 30.0;
const SESSION_STICKY_TTL_SECS: f64 = 30.0 * 24.0 * 3600.0;
const SESSION_STICKY_PRUNE_INTERVAL_SECS: f64 = 60.0;
pub const DEFAULT_CAPTURE_MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_CAPTURE_INDEX_RECORDS: usize = 200;
const MAX_WEBSOCKET_METADATA_FRAME_BYTES: usize = 64 * 1024;
const CAPTURE_STOP_MANUAL: &str = "manual";
const CAPTURE_STOP_CAPACITY: &str = "capacity_limit";

/// Context kept for the portion of an inbound task that can reject before a
/// `CompletionGuard` exists.  In particular, auth/body/route failures should
/// still say which HTTP surface rejected them without threading three extra
/// arguments through every validation branch.  The values are bounded and the
/// path never contains a query string.
#[derive(Clone)]
struct InboundRequestContext {
    method: String,
    path: String,
    route_intent: String,
    /// Bounded client/session identity captured before body parsing.  This is
    /// intentionally header-only at this layer; body metadata is merged by
    /// the request handler when it is available.
    session_id: Option<String>,
    grok_metadata: Option<GrokMetadata>,
}

tokio::task_local! {
    static INBOUND_REQUEST_CONTEXT: RefCell<InboundRequestContext>;
}

struct DiagnosticCaptureState {
    enabled: bool,
    started_at: Option<f64>,
    max_bytes: usize,
    captured_bytes: usize,
    limit_reached: bool,
    stop_reason: Option<String>,
    records: Vec<DiagnosticRequestCapture>,
    attempt_started: HashMap<String, Instant>,
}

/// 诊断捕获的轻量索引缓存。
///
/// 完整记录仍然保存在 `DiagnosticCaptureState` 中并按原样持久化；索引只
/// 保存最近 200 条元数据。把它拆成独立锁后，后台落盘复制 512 MB 明文时，
/// Admin 刷新不必等待完整记录锁，也不会把正文带回前端。
struct DiagnosticCaptureIndexCache {
    enabled: bool,
    started_at: Option<f64>,
    max_bytes: usize,
    captured_bytes: usize,
    limit_reached: bool,
    stop_reason: Option<String>,
    record_count: usize,
    records: Vec<Value>,
}

fn record_size(record: &DiagnosticRequestCapture) -> usize {
    record.request_id.len()
        + record.method.len()
        + record.path.len()
        + record.client_model.len()
        + record.effective_model.len()
        + record.feature_rule_id.as_ref().map_or(0, String::len)
        + record
            .client_declared
            .as_ref()
            .map_or(0, client_declared_size)
        + record.failure_detail.as_ref().map_or(0, String::len)
        + record.inbound_body.len()
        + record
            .inbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + record
            .attempts
            .iter()
            .map(diagnostic_attempt_size)
            .sum::<usize>()
        + record
            .client_chunks
            .iter()
            .map(|chunk| chunk.data.len())
            .sum::<usize>()
}

fn client_declared_size(declared: &ClientDeclaredMetadata) -> usize {
    declared.project.as_ref().map_or(0, String::len)
        + declared.workspace.as_ref().map_or(0, String::len)
        + declared.git_remote.as_ref().map_or(0, String::len)
        + declared.user.as_ref().map_or(0, String::len)
}

fn diagnostic_attempt_size(attempt: &DiagnosticAttemptCapture) -> usize {
    attempt.id.len()
        + attempt.endpoint_id.len()
        + attempt.endpoint_name.len()
        + attempt.protocol.len()
        + attempt.pinned_ip.as_ref().map_or(0, String::len)
        + attempt.outbound_method.len()
        + attempt.outbound_url.len()
        + attempt.outbound_body.len()
        + attempt.error.as_ref().map_or(0, String::len)
        + attempt
            .outbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + attempt
            .response_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + attempt
            .upstream_chunks
            .iter()
            .map(|chunk| chunk.data.len())
            .sum::<usize>()
}

fn diagnostic_attempt_response_size(attempt: &DiagnosticAttemptCapture) -> usize {
    attempt
        .response_headers
        .iter()
        .map(|header| header.name.len() + header.value.len())
        .sum::<usize>()
        + attempt.error.as_ref().map_or(0, String::len)
}

fn diagnostic_capture_size(records: &[DiagnosticRequestCapture]) -> usize {
    records.iter().map(record_size).sum()
}

fn diagnostic_capture_index_record(record: &DiagnosticRequestCapture) -> Value {
    json!({
        "requestID": record.request_id,
        "timestamp": record.timestamp,
        "method": record.method,
        "path": record.path,
        "clientKind": record.client_kind,
        "requestPurpose": record.request_purpose,
        "clientModel": record.client_model,
        "effectiveModel": record.effective_model,
        "featureRuleID": record.feature_rule_id,
        "sourceFormat": record.source_format,
        "targetFormat": record.target_format,
        "routeMode": record.route_mode,
        "completedAtMS": record.completed_at_ms,
        "statusCode": record.status_code,
        "outcome": record.outcome,
        "failureKind": record.failure_kind,
        "truncated": record.truncated,
        "attemptCount": record.attempts.len(),
        "clientChunkCount": record.client_chunks.len(),
    })
}

fn capture_index_cache_from_capture(
    capture: &DiagnosticCaptureState,
) -> DiagnosticCaptureIndexCache {
    DiagnosticCaptureIndexCache {
        enabled: capture.enabled,
        started_at: capture.started_at,
        max_bytes: capture.max_bytes,
        captured_bytes: capture.captured_bytes,
        limit_reached: capture.limit_reached,
        stop_reason: capture.stop_reason.clone(),
        record_count: capture.records.len(),
        records: capture
            .records
            .iter()
            .take(MAX_CAPTURE_INDEX_RECORDS)
            .map(diagnostic_capture_index_record)
            .collect(),
    }
}

fn sync_capture_index_status(
    cache: &mut DiagnosticCaptureIndexCache,
    capture: &DiagnosticCaptureState,
) {
    cache.enabled = capture.enabled;
    cache.started_at = capture.started_at;
    cache.max_bytes = capture.max_bytes;
    cache.captured_bytes = capture.captured_bytes;
    cache.limit_reached = capture.limit_reached;
    cache.stop_reason = capture.stop_reason.clone();
    cache.record_count = capture.records.len();
}

fn capture_snapshot(capture: &DiagnosticCaptureState) -> DiagnosticCaptureSnapshot {
    DiagnosticCaptureSnapshot {
        enabled: capture.enabled,
        started_at: capture.started_at,
        max_bytes: capture.max_bytes,
        captured_bytes: capture.captured_bytes,
        limit_reached: capture.limit_reached,
        stop_reason: capture.stop_reason.clone(),
        records: capture.records.clone(),
    }
}

fn diagnostic_text(bytes: &[u8], remaining: usize) -> (String, bool) {
    let take = bytes.len().min(remaining);
    let mut text = String::from_utf8_lossy(&bytes[..take]).into_owned();
    if text.len() > remaining {
        let mut end = remaining.min(text.len());
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
    }
    (text, take < bytes.len())
}

fn diagnostic_headers(
    headers: &[(String, String)],
    remaining: usize,
) -> (Vec<DiagnosticHeader>, bool) {
    let mut out = Vec::new();
    let mut left = remaining;
    for (name, value) in headers {
        if left == 0 {
            return (out, true);
        }
        let (name, nt) = diagnostic_text(name.as_bytes(), left);
        left = left.saturating_sub(name.len());
        let (value, vt) = diagnostic_text(value.as_bytes(), left);
        left = left.saturating_sub(value.len());
        out.push(DiagnosticHeader { name, value });
        if nt || vt {
            return (out, true);
        }
    }
    (out, false)
}

fn refresh_capture_usage(capture: &mut DiagnosticCaptureState, truncated: bool) {
    if truncated || capture.captured_bytes >= capture.max_bytes {
        capture.enabled = false;
        capture.limit_reached = true;
        capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
        capture.attempt_started.clear();
    }
}

impl Default for DiagnosticCaptureState {
    fn default() -> Self {
        Self {
            enabled: false,
            started_at: None,
            max_bytes: DEFAULT_CAPTURE_MAX_BYTES,
            captured_bytes: 0,
            limit_reached: false,
            stop_reason: None,
            records: Vec::new(),
            attempt_started: HashMap::new(),
        }
    }
}
#[derive(Clone)]
struct SessionStickyEntry {
    label: String,
    at: f64,
    persistent: bool,
}

/// 按 TTL + 上限清理会话归属，返回是否删除了需要同步回磁盘的持久条目。
fn prune_session_sticky(sticky: &mut HashMap<String, SessionStickyEntry>, now: f64) -> bool {
    let entries: Vec<(String, f64, bool)> = sticky
        .iter()
        .map(|(key, entry)| (key.clone(), entry.at, entry.persistent))
        .collect();
    let mut removed_persistent = false;
    for key in scheduler::session_sticky_evictions(
        &entries,
        now,
        SESSION_STICKY_TTL_SECS,
        SESSION_STICKY_MAX_ENTRIES,
    ) {
        removed_persistent |= sticky.remove(&key).is_some_and(|entry| entry.persistent);
    }
    removed_persistent
}
/// 请求体上限：server body-limit 与 Engine 实际读取共用，防止异常请求无限占用内存。
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// Realtime ephemeral keys are short-lived upstream credentials. Keep only a
/// bounded, in-memory allow-list so a listener token can remain enabled while
/// a client uses the `ek_…` returned by `/v1/realtime/client_secrets`.
const MAX_REALTIME_CLIENT_SECRETS: usize = 256;
const DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS: f64 = 300.0;
const MAX_LIVE_SESSIONS: usize = 256;
const DEFAULT_LIVE_SESSION_TTL_SECS: f64 = 900.0;
/// Video objects remain addressable while an asynchronous render is queued;
/// keep their binding for a day, independently from the short Live call TTL.
const DEFAULT_VIDEO_SESSION_TTL_SECS: f64 = 24.0 * 3600.0;
const MAX_RESOURCE_BINDING_ID_BYTES: usize = 128;
const MAX_RESOURCE_BINDING_TEXT_BYTES: usize = 256;

#[derive(Clone)]
struct RealtimeClientSecretEntry {
    expires_at: f64,
    endpoint_id: Option<String>,
    model: Option<String>,
    /// The session configuration returned by the upstream client-secret
    /// endpoint.  CPA replays this configuration with `session.update` when
    /// the ephemeral key opens a WebSocket, so voice/instructions survive the
    /// two-step browser flow.
    session: Option<Value>,
}

type NativeWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A Realtime/Live upstream connection that has completed its handshake.
/// Adapters hold the downstream upgrade until this value is ready, matching
/// CPA's behavior for upstream 401/404/429 responses.
pub struct PreparedWebSocket {
    connection: ConnectedWebSocket,
    context: WebSocketEventContext,
}

struct ConnectedWebSocket {
    upstream: Option<NativeWebSocket>,
    endpoint: PlannedEndpoint,
    attempt_count: u64,
    failover: bool,
    handshake_ttfb_ms: i64,
}

#[derive(Clone)]
struct WebSocketEventContext {
    request_id: String,
    request_path: String,
    route_intent: String,
    client_kind: ClientKind,
    model: String,
    codex_metadata: Option<CodexMetadata>,
    client_declared: Option<ClientDeclaredMetadata>,
    grok_metadata: Option<GrokMetadata>,
    session_id: Option<String>,
    started: Instant,
}

#[derive(Default)]
struct WebSocketRelayCounters {
    bytes_sent: AtomicU64,
    bytes_received: AtomicU64,
    client_message_count: AtomicU64,
    upstream_message_count: AtomicU64,
    failed: AtomicBool,
    abnormal_close: AtomicBool,
    client_close_code: Mutex<Option<i64>>,
    upstream_close_code: Mutex<Option<i64>>,
    closed_by: Mutex<Option<String>>,
    relay_error: Mutex<Option<String>>,
    first_client_text_seen: AtomicBool,
    first_client_codex_metadata: Mutex<Option<CodexMetadata>>,
}

struct WebSocketRelayMetrics {
    bytes_sent: u64,
    bytes_received: u64,
    client_message_count: u64,
    upstream_message_count: u64,
    client_close_code: Option<i64>,
    upstream_close_code: Option<i64>,
    closed_by: Option<String>,
    relay_error: Option<String>,
    abnormal_close: bool,
    failed: bool,
    first_client_codex_metadata: Option<CodexMetadata>,
    duration_ms: i64,
}

#[derive(Debug)]
pub struct WebSocketPrepareError {
    status: u16,
    code: &'static str,
    message: String,
    attempts: u64,
    endpoint: Option<PlannedEndpoint>,
    ttfb_ms: Option<i64>,
    retry_after_seconds: Option<f64>,
}

impl WebSocketPrepareError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            attempts: 0,
            endpoint: None,
            ttfb_ms: None,
            retry_after_seconds: None,
        }
    }

    fn with_attempt(
        mut self,
        endpoint: &PlannedEndpoint,
        attempts: u64,
        ttfb_ms: Option<i64>,
        retry_after_seconds: Option<f64>,
    ) -> Self {
        self.attempts = attempts;
        self.endpoint = Some(endpoint.clone());
        self.ttfb_ms = ttfb_ms;
        self.retry_after_seconds = retry_after_seconds;
        self
    }

    pub fn status_code(&self) -> u16 {
        self.status
    }

    pub fn error_code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub fn websocket_prepare_error_response(error: &WebSocketPrepareError) -> Response {
    let status = StatusCode::from_u16(error.status_code()).unwrap_or(StatusCode::BAD_GATEWAY);
    let body = serde_json::to_vec(&json!({
        "error": error.error_code(),
        "message": error.message(),
    }))
    .unwrap_or_else(|_| b"{\"error\":\"upstream_error\"}".to_vec());
    let mut response = Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()));
    // A failed WebSocket upgrade has no `CompletionGuard`/normal HTTP retry
    // response path, so preserve the bounded provider hint here as well.  A
    // client that receives a 429/5xx during the handshake should observe the
    // same Retry-After contract as an ordinary request.
    if let Some(seconds) = error
        .retry_after_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
    {
        let delay = seconds.ceil().min(MAX_RETRY_AFTER_SECS) as u64;
        if let Ok(value) = HeaderValue::from_str(&delay.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

#[derive(Clone)]
struct LiveSessionEntry {
    expires_at: f64,
    endpoint_id: String,
    /// Logical model selected during the bootstrap. Sideband requests often
    /// omit `model`; retaining it prevents them from being re-routed through
    /// the standard Realtime default.
    model: String,
}

/// 引擎对外广播（Linux Admin SSE 的事实源）。
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum EngineNotice {
    Event(RuntimeEvent),
    RuntimeChange {
        seq: i64,
        change_seq: i64,
        event: RuntimeEvent,
    },
    ConfigReloaded {
        generation: String,
    },
    StatsReset,
    ProxyState {
        running: bool,
        host: Option<String>,
        port: Option<u16>,
    },
    /// A platform handler may publish a UI-facing notice without exposing its
    /// implementation (Swift/AppKit or Linux service details) to the data
    /// plane.  The event itself is still recorded separately by the handler.
    PlatformNotice(PlatformNotice),
}

struct EngineState {
    runtime: RuntimeSnapshot,
    last_error: Option<String>,
    stats_durability_warning: Option<String>,
    ip_health: HashMap<String, scheduler::Health>,
    /// `(endpoint_id, routed_model)` health.  IP health alone cannot prevent
    /// a provider from repeatedly rejecting one model while serving another.
    provider_model_health: HashMap<(String, String), ProviderModelHealth>,
    ip_rotation: i64,
    /// affinityID → 调度组；稳定会话不超时并持久化，内容指纹仅进程内兼容。
    session_sticky: HashMap<String, SessionStickyEntry>,
    last_session_prune_at: f64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ProviderModelHealth {
    cooling_until: Option<f64>,
    consecutive_failures: u32,
    last_status: Option<u16>,
}

pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Clone for Engine {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

pub struct EngineInner {
    config: RwLock<Arc<AppConfig>>,
    generation: RwLock<String>,
    transport: Arc<dyn UpstreamTransport>,
    platform: Arc<dyn PlatformBoundary>,
    state: Mutex<EngineState>,
    dir: Option<ConfigDir>,
    stats_writable: AtomicBool,
    session_affinity_flush: Mutex<()>,
    session_affinity_dirty: AtomicBool,
    session_affinity_writable: AtomicBool,
    resource_bindings_flush: Mutex<()>,
    resource_bindings_dirty: AtomicBool,
    resource_bindings_writable: AtomicBool,
    capture_flush: Mutex<()>,
    capture_dirty: AtomicBool,
    capture_writable: AtomicBool,
    capture: Mutex<DiagnosticCaptureState>,
    capture_index: Mutex<DiagnosticCaptureIndexCache>,
    notices: tokio::sync::broadcast::Sender<EngineNotice>,
    started_at: Instant,
    runtime_store: Option<RuntimeStore>,
    runtime_write: Mutex<()>,
    realtime_client_secrets: Mutex<HashMap<String, RealtimeClientSecretEntry>>,
    live_sessions: Mutex<HashMap<String, LiveSessionEntry>>,
    video_sessions: Mutex<HashMap<String, LiveSessionEntry>>,
}

fn now_unix() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn valid_resource_binding_text(value: &str) -> bool {
    let trimmed = value.trim();
    value == trimmed
        && !trimmed.is_empty()
        && trimmed.len() <= MAX_RESOURCE_BINDING_TEXT_BYTES
        && !trimmed.chars().any(char::is_control)
}

fn valid_resource_binding_id(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= MAX_RESOURCE_BINDING_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn provider_model_key(endpoint: &PlannedEndpoint) -> (String, String) {
    // `routed_model` is the logical mapping selected by the client.  Keeping
    // that dimension (rather than only `upstream_model`) prevents a failure
    // of `grok-imagine-video` from cooling an unrelated mapping that happens
    // to share the same upstream alias.
    (
        endpoint.endpoint_id.clone(),
        endpoint.routed_model.trim().to_string(),
    )
}

fn provider_model_cooldown_seconds(consecutive_failures: u32) -> f64 {
    let exponent = consecutive_failures.saturating_sub(1).min(16) as i32;
    (PROVIDER_MODEL_COOLDOWN_INITIAL_SECS * PROVIDER_MODEL_COOLDOWN_FACTOR.powi(exponent))
        .min(PROVIDER_MODEL_COOLDOWN_MAX_SECS)
}

fn provider_model_cooling_until(
    health: &HashMap<(String, String), ProviderModelHealth>,
    endpoint: &PlannedEndpoint,
    now: f64,
) -> Option<f64> {
    health
        .get(&provider_model_key(endpoint))
        .and_then(|entry| entry.cooling_until)
        .filter(|until| until.is_finite() && *until > now)
}

fn runtime_outcome_token(event: &RuntimeEvent) -> Option<&'static str> {
    match event.outcome {
        Some(RuntimeEventOutcome::Succeeded) => Some("succeeded"),
        Some(RuntimeEventOutcome::Failed) => Some("failed"),
        Some(RuntimeEventOutcome::Cancelled) => Some("cancelled"),
        None => None,
    }
}

pub fn config_generation(config: &AppConfig) -> String {
    use md5::{Digest, Md5};
    let mut hasher = Md5::new();
    hasher.update(config.to_json_pretty().unwrap_or_default().as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn new_event_id() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    // 形状对齐 Swift UUID.uuidString(大写、8-4-4-4-12)。
    let h: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{}{}{}{}-{}{}-{}{}-{}{}-{}{}{}{}{}{}",
        h[0],
        h[1],
        h[2],
        h[3],
        h[4],
        h[5],
        h[6],
        h[7],
        h[8],
        h[9],
        h[10],
        h[11],
        h[12],
        h[13],
        h[14],
        h[15]
    )
}

impl Engine {
    pub fn new(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
    ) -> Self {
        Self::new_with_services(
            config,
            dir,
            EngineServices {
                transport,
                platform: Arc::new(crate::boundary::NoopPlatform),
            },
        )
    }

    /// Construct the non-generic shared engine with explicit boundary
    /// services.  The legacy `new` constructor above remains available for
    /// callers that only provide a transport; platform facades use this
    /// method to inject their control policy.
    pub fn new_with_services(
        config: AppConfig,
        dir: Option<ConfigDir>,
        services: EngineServices,
    ) -> Self {
        let generation = config_generation(&config);
        let (runtime_store, runtime, stats_writable, stats_error) = match dir.as_ref() {
            Some(dir) => match RuntimeStore::new(dir.root.join("runtime.sqlite3")) {
                Ok((store, runtime)) => (Some(store), runtime, true, None),
                Err(error) => (None, RuntimeSnapshot::default(), false, Some(error)),
            },
            None => (None, RuntimeSnapshot::default(), true, None),
        };
        let now = now_unix();
        let (session_sticky, session_affinity_writable, session_affinity_pruned) =
            match dir.as_ref().map(ConfigDir::load_session_affinity) {
                Some(Ok(assignments)) => {
                    let mut loaded: HashMap<String, SessionStickyEntry> = assignments
                        .into_iter()
                        .map(|(key, assignment)| {
                            (
                                key,
                                SessionStickyEntry {
                                    label: assignment.scheduling_group,
                                    at: assignment.updated_at,
                                    persistent: true,
                                },
                            )
                        })
                        .collect();
                    let pruned = prune_session_sticky(&mut loaded, now);
                    (loaded, true, pruned)
                }
                Some(Err(error)) => {
                    tracing::warn!("session_affinity.json 加载失败，本次运行拒绝覆盖: {error}");
                    (HashMap::new(), false, false)
                }
                None => (HashMap::new(), true, false),
            };
        let (live_sessions, video_sessions, resource_bindings_writable, resource_bindings_pruned) =
            match dir.as_ref().map(ConfigDir::load_resource_bindings) {
                Some(Ok(bindings)) => {
                    let mut live = HashMap::new();
                    let mut video = HashMap::new();
                    let mut pruned = false;
                    for (key, binding) in bindings {
                        if binding.expires_at <= now {
                            pruned = true;
                            continue;
                        }
                        let target = if let Some(id) = key.strip_prefix("live:") {
                            Some((&mut live, id))
                        } else if let Some(id) = key.strip_prefix("video:") {
                            Some((&mut video, id))
                        } else {
                            pruned = true;
                            None
                        };
                        if let Some((store, id)) = target {
                            store.insert(
                                id.to_string(),
                                LiveSessionEntry {
                                    expires_at: binding.expires_at,
                                    endpoint_id: binding.endpoint_id,
                                    model: binding.model,
                                },
                            );
                        }
                    }
                    (live, video, true, pruned)
                }
                Some(Err(error)) => {
                    tracing::warn!("resource_bindings.json 加载失败，本次运行拒绝覆盖: {error}");
                    (HashMap::new(), HashMap::new(), false, false)
                }
                None => (HashMap::new(), HashMap::new(), true, false),
            };
        let (capture, capture_writable) = match dir.as_ref().map(ConfigDir::load_diagnostic_capture)
        {
            Some(Ok(snapshot)) => {
                let mut capture = DiagnosticCaptureState {
                    enabled: false,
                    started_at: snapshot.started_at,
                    max_bytes: snapshot.max_bytes.max(1),
                    captured_bytes: 0,
                    limit_reached: snapshot.limit_reached,
                    stop_reason: snapshot.stop_reason,
                    records: snapshot.records,
                    attempt_started: HashMap::new(),
                };
                capture.captured_bytes = diagnostic_capture_size(&capture.records);
                if capture.captured_bytes >= capture.max_bytes {
                    capture.limit_reached = true;
                    capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
                }
                (capture, true)
            }
            Some(Err(error)) => {
                tracing::warn!("diagnostic_capture.json 加载失败，本次运行拒绝覆盖: {error}");
                (DiagnosticCaptureState::default(), false)
            }
            None => (DiagnosticCaptureState::default(), true),
        };
        let (notices, _) = tokio::sync::broadcast::channel(512);
        let engine = Self {
            inner: Arc::new(EngineInner {
                config: RwLock::new(Arc::new(config)),
                generation: RwLock::new(generation),
                transport: services.transport,
                platform: services.platform,
                state: Mutex::new(EngineState {
                    runtime,
                    last_error: stats_error,
                    stats_durability_warning: None,
                    ip_health: HashMap::new(),
                    provider_model_health: HashMap::new(),
                    ip_rotation: 0,
                    session_sticky,
                    last_session_prune_at: now,
                }),
                dir,
                stats_writable: AtomicBool::new(stats_writable),
                session_affinity_flush: Mutex::new(()),
                session_affinity_dirty: AtomicBool::new(session_affinity_pruned),
                session_affinity_writable: AtomicBool::new(session_affinity_writable),
                resource_bindings_flush: Mutex::new(()),
                resource_bindings_dirty: AtomicBool::new(resource_bindings_pruned),
                resource_bindings_writable: AtomicBool::new(resource_bindings_writable),
                capture_flush: Mutex::new(()),
                capture_dirty: AtomicBool::new(false),
                capture_writable: AtomicBool::new(capture_writable),
                notices,
                started_at: Instant::now(),
                capture_index: Mutex::new(capture_index_cache_from_capture(&capture)),
                capture: Mutex::new(capture),
                runtime_store,
                runtime_write: Mutex::new(()),
                realtime_client_secrets: Mutex::new(HashMap::new()),
                live_sessions: Mutex::new(live_sessions),
                video_sessions: Mutex::new(video_sessions),
            }),
        };
        // Loading is authoritative at construction time. If stale entries
        // were pruned, persist the cleaned snapshot immediately rather than
        // leaving expired bindings on disk until a background task starts or
        // an unrelated request happens to flush state.
        engine.flush_resource_bindings_if_dirty();
        engine
    }

    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<EngineNotice> {
        self.inner.notices.subscribe()
    }

    pub fn publish_proxy_state(&self, running: bool, host: Option<String>, port: Option<u16>) {
        let _ = self.inner.notices.send(EngineNotice::ProxyState {
            running,
            host,
            port,
        });
    }

    pub fn config(&self) -> Arc<AppConfig> {
        self.inner.config.read().unwrap().clone()
    }

    pub fn generation(&self) -> String {
        self.inner.generation.read().unwrap().clone()
    }

    pub fn uptime_seconds(&self) -> u64 {
        self.inner.started_at.elapsed().as_secs()
    }

    /// Remember an upstream Realtime ephemeral key without persisting or
    /// exposing it through runtime events. The expiration is bounded so a
    /// malformed/overly long upstream lifetime cannot turn it into a durable
    /// listener credential.
    fn register_realtime_client_secret(
        &self,
        token: &str,
        expires_at: Option<f64>,
        endpoint_id: Option<&str>,
        model: Option<&str>,
        session: Option<Value>,
    ) {
        let token = token.trim();
        if !token.starts_with("ek_") || token.len() > 512 || token.chars().any(char::is_control) {
            return;
        }
        let now = now_unix();
        let requested_expiry = expires_at
            .filter(|value| value.is_finite() && *value > now)
            .unwrap_or(now + DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS);
        let expiry = requested_expiry.min(now + DEFAULT_REALTIME_CLIENT_SECRET_TTL_SECS);
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        if secrets.len() >= MAX_REALTIME_CLIENT_SECRETS
            && !secrets.contains_key(token)
            && let Some(oldest) = secrets
                .iter()
                .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                .map(|(key, _)| key.clone())
        {
            secrets.remove(&oldest);
        }
        secrets.insert(
            token.to_string(),
            RealtimeClientSecretEntry {
                expires_at: expiry,
                endpoint_id: endpoint_id
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                model: model
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
                session,
            },
        );
    }

    fn realtime_client_secret_authorized(&self, headers: &[(String, String)]) -> bool {
        let Some(token) = realtime_ephemeral_token(headers) else {
            return false;
        };
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets
            .get(&token)
            .is_some_and(|entry| entry.expires_at > now)
    }

    fn realtime_client_secret_endpoint(&self, headers: &[(String, String)]) -> Option<String> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets
            .get(&token)
            .and_then(|entry| entry.endpoint_id.clone())
    }

    fn realtime_client_secret_model(&self, headers: &[(String, String)]) -> Option<String> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets.get(&token).and_then(|entry| entry.model.clone())
    }

    fn realtime_client_secret_session(&self, headers: &[(String, String)]) -> Option<Value> {
        let token = realtime_ephemeral_token(headers)?;
        let now = now_unix();
        let mut secrets = self.inner.realtime_client_secrets.lock().unwrap();
        secrets.retain(|_, value| value.expires_at > now);
        secrets.get(&token).and_then(|entry| entry.session.clone())
    }

    fn register_realtime_client_secret_from_body(
        &self,
        body: &[u8],
        endpoint_id: Option<&str>,
        model: Option<&str>,
        fallback_session: Option<Value>,
    ) {
        let Ok(value) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        let root_expiry = value.get("expires_at").and_then(json_timestamp);
        let candidate = value
            .get("value")
            .and_then(Value::as_str)
            .map(|token| (token, root_expiry))
            .or_else(|| {
                let secret = value.get("client_secret")?;
                if let Some(token) = secret.as_str() {
                    return Some((token, root_expiry));
                }
                let object = secret.as_object()?;
                let token = object.get("value").and_then(Value::as_str)?;
                Some((
                    token,
                    object
                        .get("expires_at")
                        .and_then(json_timestamp)
                        .or(root_expiry),
                ))
            });
        let session = value
            .get("session")
            .and_then(Value::as_object)
            .map(|session| {
                let mut session = session.clone();
                // The public client-secret response adds bookkeeping fields that
                // are not part of the session configuration CPA replays.
                for field in ["id", "object", "expires_at", "client_secret"] {
                    session.remove(field);
                }
                Value::Object(session)
            })
            .or(fallback_session);
        if let Some((token, expires_at)) = candidate {
            self.register_realtime_client_secret(token, expires_at, endpoint_id, model, session);
        }
    }

    fn register_live_session(&self, call_id: &str, endpoint_id: &str, model: &str) {
        let call_id = call_id.trim();
        let now = now_unix();
        if !valid_resource_binding_id(call_id)
            || !valid_resource_binding_text(endpoint_id)
            || !valid_resource_binding_text(model)
            || !DEFAULT_LIVE_SESSION_TTL_SECS.is_finite()
            || DEFAULT_LIVE_SESSION_TTL_SECS <= 0.0
            || !now.is_finite()
        {
            return;
        }
        if !(now + DEFAULT_LIVE_SESSION_TTL_SECS).is_finite() {
            return;
        }
        {
            let mut sessions = self.inner.live_sessions.lock().unwrap();
            sessions.retain(|_, entry| entry.expires_at > now);
            if sessions.len() >= MAX_LIVE_SESSIONS
                && !sessions.contains_key(call_id)
                && let Some(oldest) = sessions
                    .iter()
                    .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                    .map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
            sessions.insert(
                call_id.to_string(),
                LiveSessionEntry {
                    expires_at: now + DEFAULT_LIVE_SESSION_TTL_SECS,
                    endpoint_id: endpoint_id.trim().to_string(),
                    model: model.trim().to_string(),
                },
            );
        }
        self.inner
            .resource_bindings_dirty
            .store(true, Ordering::Release);
        self.flush_resource_bindings_if_dirty();
    }

    /// Return the originating endpoint for a sideband target. The outer
    /// `Option` indicates whether the target carried a call id; the inner
    /// value is `None` when that id is unknown or expired. Keeping those
    /// states distinct is important: an unknown Live id must not silently
    /// fall back to ordinary provider selection.
    fn live_session_endpoint(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(
            sessions
                .get(&call_id)
                .map(|entry| entry.endpoint_id.clone()),
        );
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    fn live_session_model(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(sessions.get(&call_id).map(|entry| entry.model.clone()));
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    fn register_video_session(&self, video_id: &str, endpoint_id: &str, model: &str) {
        self.register_live_session_into(
            &self.inner.video_sessions,
            video_id,
            endpoint_id,
            model,
            DEFAULT_VIDEO_SESSION_TTL_SECS,
        );
    }

    fn register_live_session_into(
        &self,
        store: &Mutex<HashMap<String, LiveSessionEntry>>,
        session_id: &str,
        endpoint_id: &str,
        model: &str,
        ttl_secs: f64,
    ) {
        let session_id = session_id.trim();
        if !valid_resource_binding_id(session_id)
            || !valid_resource_binding_text(endpoint_id)
            || !valid_resource_binding_text(model)
            || !ttl_secs.is_finite()
            || ttl_secs <= 0.0
        {
            return;
        }
        let now = now_unix();
        if !now.is_finite() || !(now + ttl_secs).is_finite() {
            return;
        }
        {
            let mut sessions = store.lock().unwrap();
            sessions.retain(|_, entry| entry.expires_at > now);
            if sessions.len() >= MAX_LIVE_SESSIONS
                && !sessions.contains_key(session_id)
                && let Some(oldest) = sessions
                    .iter()
                    .min_by(|(_, left), (_, right)| left.expires_at.total_cmp(&right.expires_at))
                    .map(|(key, _)| key.clone())
            {
                sessions.remove(&oldest);
            }
            sessions.insert(
                session_id.to_string(),
                LiveSessionEntry {
                    expires_at: now + ttl_secs,
                    endpoint_id: endpoint_id.trim().to_string(),
                    model: model.trim().to_string(),
                },
            );
        }
        self.inner
            .resource_bindings_dirty
            .store(true, Ordering::Release);
        self.flush_resource_bindings_if_dirty();
    }

    fn video_session_endpoint(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(
            sessions
                .get(video_id)
                .map(|entry| entry.endpoint_id.clone()),
        );
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    fn video_session_model(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        let before = sessions.len();
        sessions.retain(|_, entry| entry.expires_at > now);
        let pruned = sessions.len() != before;
        if pruned {
            self.inner
                .resource_bindings_dirty
                .store(true, Ordering::Release);
        }
        let result = Some(sessions.get(video_id).map(|entry| entry.model.clone()));
        drop(sessions);
        if pruned {
            self.flush_resource_bindings_if_dirty();
        }
        result
    }

    pub fn runtime_snapshot(&self) -> RuntimeSnapshot {
        self.inner.state.lock().unwrap().runtime.clone()
    }

    pub fn runtime_summary_value(&self) -> Value {
        let (counters, latest_event, recent_event_count) = {
            let state = self.inner.state.lock().unwrap();
            (
                RuntimeCounters::from_snapshot(&state.runtime),
                state.runtime.recent_events.first().cloned(),
                state.runtime.recent_events.len(),
            )
        };
        if let Some(store) = &self.inner.runtime_store {
            let mut summary = store.summary();
            // Memory is the real-time source of truth; storage fields describe
            // durability and may lag while a batch is pending.
            summary.counters = counters;
            summary.latest_event = latest_event;
            return serde_json::to_value(summary).unwrap_or(Value::Null);
        }
        json!({
            "apiVersion": 1,
            "storage": {
                "backend": "sqlite",
                "state": "degraded",
                "pendingEvents": 0,
                "eventCount": recent_event_count,
                "dbBytes": 0,
                "walBytes": 0,
                "lastCommitAt": Value::Null,
                "lastError": self.last_error(),
            },
            "resetGeneration": 0,
            "counters": counters,
            "latestEvent": latest_event,
        })
    }

    // 事件查询的过滤维度直接透传给 runtime,合并成 struct 只会多一层转换。
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_events(
        &self,
        before_seq: Option<i64>,
        after_change_seq: Option<i64>,
        limit: usize,
        kind: Option<&str>,
        request_id: Option<&str>,
        outcome: Option<&str>,
        from: Option<f64>,
        to: Option<f64>,
    ) -> Result<Value, String> {
        if let Some(store) = &self.inner.runtime_store {
            let cursor_valid = store.change_cursor_valid(after_change_seq)?;
            let mut events = store.events(
                before_seq,
                after_change_seq,
                limit,
                kind,
                request_id,
                outcome,
                from,
                to,
            )?;
            let page_limit = limit.clamp(1, 200);
            let has_more = events.len() > page_limit;
            events.truncate(page_limit);
            return Ok(json!({
                "events": events,
                "hasMore": has_more,
                "resetGeneration": store.summary().reset_generation,
                "cursorValid": cursor_valid,
            }));
        }
        let snapshot = self.runtime_snapshot();
        let events = snapshot
            .recent_events
            .into_iter()
            .enumerate()
            .filter(|(_, event)| kind.is_none_or(|value| value == event.kind))
            .filter(|(_, event)| {
                request_id.is_none_or(|value| event.request_id.as_deref() == Some(value))
            })
            .filter(|(_, event)| {
                outcome.is_none_or(|value| runtime_outcome_token(event) == Some(value))
            })
            .filter(|(_, event)| from.is_none_or(|value| event.timestamp >= value))
            .filter(|(_, event)| to.is_none_or(|value| event.timestamp <= value))
            .map(|(index, event)| {
                RuntimeEventListItem::from_change(index as i64 + 1, index as i64 + 1, event)
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"events": events.into_iter().take(limit.clamp(1, 200)).collect::<Vec<_>>(), "hasMore": false, "resetGeneration": 0, "cursorValid": false}),
        )
    }

    fn runtime_query_path(&self) -> Result<&std::path::Path, RuntimeQueryError> {
        self.inner
            .runtime_store
            .as_ref()
            .map(RuntimeStore::database_path)
            .ok_or_else(|| RuntimeQueryError::NotFound("runtime.sqlite3 不可用".into()))
    }

    fn runtime_query_value<T: serde::Serialize>(value: T) -> Result<Value, RuntimeQueryError> {
        serde_json::to_value(value)
            .map_err(|error| RuntimeQueryError::InvalidInput(error.to_string()))
    }

    pub fn runtime_events_page(&self, query: &EventPageQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::events_page(path, query)?)
    }

    pub fn runtime_request_chain(&self, request_id: &str) -> Result<Value, RuntimeQueryError> {
        let store = self
            .inner
            .runtime_store
            .as_ref()
            .ok_or_else(|| RuntimeQueryError::NotFound("runtime.sqlite3 不可用".into()))?;
        let recent = store.recent_changes_for_request(request_id);
        Self::runtime_query_value(runtime_query::request_chain_with_recent(
            store.database_path(),
            request_id,
            &recent,
        )?)
    }

    pub fn runtime_trends(&self, query: &TrendQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::trends(path, query)?)
    }

    pub fn runtime_facets(&self, filter: &RuntimeFilter) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::facets(path, filter)?)
    }

    pub fn runtime_error_groups(&self, query: &ErrorPageQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::error_groups(path, query)?)
    }

    pub fn runtime_dimension_page(
        &self,
        kind: DimensionKind,
        query: &DimensionPageQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::dimension_page(path, kind, query)?)
    }

    pub fn runtime_storage_details(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::storage_details(path)?)
    }

    pub fn runtime_pricing(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::pricing(path)?)
    }

    pub fn runtime_export_estimate(&self, query: &ExportQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::export_estimate(path, query)?)
    }

    pub fn runtime_stream_export<F>(
        &self,
        query: &ExportQuery,
        sink: F,
    ) -> Result<ExportManifest, RuntimeQueryError>
    where
        F: FnMut(Vec<u8>) -> Result<(), String>,
    {
        let path = self.runtime_query_path()?;
        runtime_query::stream_export(path, query, sink)
    }

    pub fn runtime_set_retention(&self, update: RuntimeRetentionUpdate) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法更新保留策略".into())
        })?;
        let mutation = store.set_retention(update)?;
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_cleanup_preview(&self, older_than: f64) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法预览清理范围".into())
        })?;
        let preview: RuntimeCleanupPreview = store.cleanup_before_preview(older_than)?;
        serde_json::to_value(preview).map_err(|error| error.to_string())
    }

    pub fn runtime_cleanup(&self, older_than: f64) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清理统计".into())
        })?;
        let mutation: RuntimeCleanupMutation = store.cleanup_before(older_than)?;
        let runtime = store.snapshot()?;
        self.inner.state.lock().unwrap().runtime = runtime;
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_replace_pricing(&self, update: RuntimePricingUpdate) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法更新价格表".into())
        })?;
        let mutation = store.replace_pricing(update)?;
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_event(&self, id: &str) -> Result<Option<Value>, String> {
        if let Some(store) = &self.inner.runtime_store {
            return store.event(id).map(|change| {
                change.map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
            });
        }
        Ok(self
            .runtime_snapshot()
            .recent_events
            .into_iter()
            .find(|event| event.id == id)
            .map(|event| json!({"seq": 0, "changeSeq": 0, "event": event})))
    }

    pub fn delete_runtime_session(&self, session_id: &str) -> Result<Value, String> {
        self.delete_runtime_session_confirmed(session_id, false)
    }

    pub fn delete_runtime_session_confirmed(
        &self,
        session_id: &str,
        confirm_unidentified: bool,
    ) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法删除会话".into())
        })?;
        let mutation = store.delete_session_confirmed(session_id, confirm_unidentified)?;
        let runtime = store.snapshot()?;
        let mut state = self.inner.state.lock().unwrap();
        state.runtime = runtime;
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn export_runtime_session(&self, session_id: &str) -> Result<Value, String> {
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法导出会话".into())
        })?;
        store.export_session(session_id)
    }

    pub fn runtime_analytics(&self, range: &str) -> Result<Value, String> {
        self.runtime_analytics_filtered(range, &AnalyticsFilter::default())
    }

    pub fn runtime_analytics_filtered(
        &self,
        range: &str,
        filter: &AnalyticsFilter,
    ) -> Result<Value, String> {
        let filter = filter.normalized();
        if let Some(store) = &self.inner.runtime_store {
            let query_filter = RuntimeFilter {
                client_kind: filter.client_kind.clone(),
                endpoint_id: filter.endpoint_id.clone(),
                project_id: filter.project_id.clone(),
                project_name: filter.project.clone(),
                session_id: filter.session_id.clone(),
                from: filter.from,
                to: filter.to,
                ..RuntimeFilter::default()
            };
            return runtime_query::analytics(store.database_path(), range, &query_filter)
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()));
        }
        let counters = RuntimeCounters::from_snapshot(&self.runtime_snapshot());
        let filtered = filter.is_active();
        let count = |value: i64| if filtered { 0 } else { value };
        Ok(json!({
            "range": range,
            "from": Value::Null,
            "clientRequests": count(counters.client_requests),
            "clientSuccesses": count(counters.client_successes),
            "clientFailures": count(counters.client_failures),
            "clientCancelled": 0,
            "clientPending": 0,
            "clientSuccessRate": Value::Null,
            "upstreamAttempts": count(counters.upstream_attempts),
            "upstreamSuccesses": count(counters.upstream_successes),
            "upstreamFailures": count(counters.upstream_failures),
            "failovers": count(counters.failovers),
            "averageDurationMS": Value::Null,
            "averageTTFBMS": Value::Null,
            "latencyBuckets": {"under1s": 0, "from1sTo3s": 0, "from3sTo6s": 0, "over6s": 0},
            "tokenUsage": {
                "inputTokens": 0, "outputTokens": 0, "cacheReadInputTokens": 0,
                "cacheCreationInputTokens": 0, "reasoningTokens": 0,
                "uncachedInputTokens": 0, "processedInputTokens": 0,
                "processedTotalTokens": 0, "totalTokens": 0, "observedRequests": 0,
                "tokenAccountingSemantics": "unknown", "tokenAccountingQuality": "unknown",
                "usageFieldPresence": {
                    "inputTokens": 0, "outputTokens": 0,
                    "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0,
                    "reasoningTokens": 0
                }
            },
            "endpoints": [], "models": [], "clientKinds": [], "requestPurposes": [],
            "featureRules": [], "protocolRoutes": [], "failureKinds": [],
            "failurePhases": [], "upstreamStatuses": [], "streamTerminals": [],
            "projects": [], "sessions": [], "toolCalls": [], "codexMetadataPresent": 0,
            "facets": {"clientKinds": [], "projects": [], "sessions": []},
            "skippedEvents": 0,
            "truncated": false,
            "filtersApplied": !filtered,
            "filterWarning": if filtered {
                Value::String("SQLite analytics unavailable; filters were not applied".into())
            } else {
                Value::Null
            },
            "appliedFilters": {
                "clientKind": filter.client_kind,
                "endpointID": filter.endpoint_id,
                "projectID": filter.project_id,
                "project": filter.project,
                "sessionID": filter.session_id,
            }
        }))
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.state.lock().unwrap().last_error.clone()
    }

    pub fn set_last_error(&self, message: Option<String>) {
        self.inner.state.lock().unwrap().last_error = message;
    }

    pub fn stats_writable(&self) -> bool {
        self.inner.runtime_store.as_ref().map_or_else(
            || self.inner.stats_writable.load(Ordering::Acquire),
            |store| store.summary().storage.state == "ready",
        )
    }

    pub fn runtime_storage_backpressured(&self) -> bool {
        self.inner
            .runtime_store
            .as_ref()
            .is_some_and(RuntimeStore::is_backpressured)
    }

    pub fn diagnostic_capture_snapshot(&self) -> DiagnosticCaptureSnapshot {
        capture_snapshot(&self.inner.capture.lock().unwrap())
    }

    pub fn diagnostic_capture_index(&self) -> Value {
        let cache = self.inner.capture_index.lock().unwrap();
        json!({"enabled": cache.enabled, "startedAt": cache.started_at, "maxBytes": cache.max_bytes,
            "capturedBytes": cache.captured_bytes, "limitReached": cache.limit_reached,
            "stopReason": cache.stop_reason, "recordCount": cache.record_count,
            "indexTruncated": cache.record_count > MAX_CAPTURE_INDEX_RECORDS,
            "records": cache.records.clone()})
    }

    /// 同步只读索引的状态字段，而不复制任何正文。
    fn sync_capture_index_status(&self, capture: &DiagnosticCaptureState) {
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
    }

    /// 新记录插入/删除会改变窗口，重建最多 200 条轻量元数据即可。
    fn sync_capture_index_window(&self, capture: &DiagnosticCaptureState) {
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
        cache.records = capture
            .records
            .iter()
            .take(MAX_CAPTURE_INDEX_RECORDS)
            .map(diagnostic_capture_index_record)
            .collect();
    }

    /// 更新窗口内的一条记录；窗口外的明文记录仍完整保留，但不需要为
    /// 不可见的索引分配 JSON。
    fn sync_capture_index_record(&self, capture: &DiagnosticCaptureState, request_id: &str) {
        let position = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id);
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
        match position {
            Some(position) if position < MAX_CAPTURE_INDEX_RECORDS => {
                let value = diagnostic_capture_index_record(&capture.records[position]);
                if let Some(existing) = cache.records.iter_mut().find(|entry| {
                    entry.get("requestID").and_then(Value::as_str) == Some(request_id)
                }) {
                    *existing = value;
                } else {
                    // Defensive recovery if a prior update was interrupted.
                    cache.records = capture
                        .records
                        .iter()
                        .take(MAX_CAPTURE_INDEX_RECORDS)
                        .map(diagnostic_capture_index_record)
                        .collect();
                }
            }
            _ => {
                cache.records.retain(|entry| {
                    entry.get("requestID").and_then(Value::as_str) != Some(request_id)
                });
            }
        }
    }

    pub fn diagnostic_capture_detail(&self, request_id: &str) -> Option<DiagnosticRequestCapture> {
        self.inner
            .capture
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|r| r.request_id == request_id)
            .cloned()
    }

    /// 为 Admin 详情端点提供已经序列化的 JSON，避免先构造 serde_json::Value 再二次编码。
    /// 记录仍在锁外序列化，避免大正文占用捕获写入锁；调用方只在用户明确选中请求时使用。
    /// 序列化失败必须与“未找到”区分，避免把损坏详情静默伪装成 404。
    pub fn diagnostic_capture_detail_json(
        &self,
        request_id: &str,
    ) -> Result<Option<Vec<u8>>, serde_json::Error> {
        let Some(record) = self.diagnostic_capture_detail(request_id) else {
            return Ok(None);
        };
        serde_json::to_vec(&record).map(Some)
    }

    /// 以记录为单位遍历当前捕获快照。调用方可以在每条记录后把字节写入
    /// 背压流，因此不会把整个 512 MiB 捕获再次聚合到一个 `Vec`。锁会在
    /// 导出期间保持，以确保新增/淘汰不会让同一次导出的记录顺序漂移；
    /// 导出是显式的低频操作，捕获写入只会短暂等待客户端背压。
    pub fn with_diagnostic_capture_records<F>(&self, mut visit: F) -> Result<(), String>
    where
        F: FnMut(&DiagnosticRequestCapture) -> Result<(), String>,
    {
        let capture = self
            .inner
            .capture
            .lock()
            .map_err(|_| "诊断捕获锁不可用".to_string())?;
        for record in &capture.records {
            visit(record)?;
        }
        Ok(())
    }

    /// 打开最近一次原子落盘的固定快照文件供 Admin 流式下载。
    /// 导出不主动触发一次近容量上限的 `serde_json::to_vec_pretty`，避免用户点击
    /// 下载时再次制造大内存峰值；后台持久化会持续更新这个文件。不接受外部路径，
    /// 避免把诊断导出变成任意文件读取端点。
    pub fn diagnostic_capture_export_file(&self) -> Result<std::fs::File, String> {
        if !self.inner.capture_writable.load(Ordering::Acquire) {
            return Err("诊断捕获快照加载失败，拒绝导出不可验证的原文件".into());
        }
        let path = self
            .inner
            .dir
            .as_ref()
            .map(|dir| dir.root.join("diagnostic_capture.json"))
            .ok_or_else(|| String::from("当前运行没有可导出的持久化配置目录"))?;
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("诊断捕获快照暂时不可读取: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("诊断捕获快照不是普通文件，拒绝导出".into());
        }
        let file =
            std::fs::File::open(&path).map_err(|error| format!("诊断捕获快照无法打开: {error}"))?;
        let opened_metadata = file
            .metadata()
            .map_err(|error| format!("诊断捕获快照无法检查: {error}"))?;
        if !opened_metadata.is_file() {
            return Err("诊断捕获快照不是普通文件，拒绝导出".into());
        }
        self.inner
            .platform
            .validate_opened_capture(&metadata, &opened_metadata)?;
        Ok(file)
    }

    pub fn set_diagnostic_capture(&self, enabled: bool, max_bytes: Option<usize>) {
        let mut capture = self.inner.capture.lock().unwrap();
        if enabled {
            if let Some(max_bytes) = max_bytes {
                capture.max_bytes = max_bytes.max(1);
            }
            capture.limit_reached = false;
            capture.stop_reason = None;
            capture.started_at = Some(now_unix());
            capture.enabled = capture.captured_bytes < capture.max_bytes;
            if !capture.enabled {
                capture.limit_reached = true;
                capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
            }
        } else {
            capture.enabled = false;
            capture.stop_reason = Some(CAPTURE_STOP_MANUAL.into());
            capture.attempt_started.clear();
        }
        self.sync_capture_index_status(&capture);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub fn clear_diagnostic_capture(&self) -> Result<(), String> {
        // Serialize explicit deletion with the background writer. A malformed
        // snapshot is protected from implicit overwrite at startup, but an
        // explicit DELETE is allowed to discard it and establish a valid empty
        // snapshot for the next daemon start.
        let _flush = self.inner.capture_flush.lock().unwrap();
        if !self.inner.capture_writable.load(Ordering::Acquire)
            && let Some(dir) = &self.inner.dir
        {
            dir.remove_diagnostic_capture()
                .map_err(|error| format!("清理损坏的 diagnostic_capture.json 失败: {error}"))?;
            self.inner.capture_writable.store(true, Ordering::Release);
        }
        let snapshot = {
            let mut capture = self.inner.capture.lock().unwrap();
            capture.records.clear();
            capture.attempt_started.clear();
            capture.captured_bytes = 0;
            capture.limit_reached = false;
            capture.stop_reason = None;
            if !capture.enabled {
                capture.started_at = None;
            }
            // Clear before cloning. Requests arriving after this lock is
            // released set dirty again, so their updates are not lost.
            self.inner.capture_dirty.store(false, Ordering::Release);
            capture_snapshot(&capture)
        };
        // The full snapshot above is intentionally retained for persistence;
        // the UI index can be updated independently without another body copy.
        let capture = self.inner.capture.lock().unwrap();
        self.sync_capture_index_window(&capture);
        drop(capture);
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        match dir.save_diagnostic_capture(&snapshot) {
            Ok(outcome) => match outcome.durability_warning() {
                Some(warning) => {
                    self.inner.capture_dirty.store(true, Ordering::Release);
                    Err(warning)
                }
                None => Ok(()),
            },
            Err(error) => {
                self.inner.capture_dirty.store(true, Ordering::Release);
                Err(format!("diagnostic_capture.json 落盘失败: {error}"))
            }
        }
    }

    fn capture_start(
        &self,
        request_id: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        meta: &ClientMeta,
    ) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        if let Some(index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        {
            let removed = capture.records.remove(index);
            capture.captured_bytes = capture.captured_bytes.saturating_sub(record_size(&removed));
        }
        let base_bytes = request_id.len()
            + method.len()
            + path.len()
            + meta.client_model.len()
            + meta.effective_model.len()
            + meta.feature_rule_id.as_ref().map_or(0, String::len);
        let available = capture.max_bytes.saturating_sub(capture.captured_bytes);
        if base_bytes > available {
            refresh_capture_usage(&mut capture, true);
            self.sync_capture_index_window(&capture);
            self.inner.capture_dirty.store(true, Ordering::Release);
            return;
        }
        let remaining = available - base_bytes;
        let (inbound_headers, headers_truncated) = diagnostic_headers(headers, remaining);
        let header_bytes = inbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>();
        let (inbound_body, body_truncated) =
            diagnostic_text(body, remaining.saturating_sub(header_bytes));
        let truncated = headers_truncated || body_truncated;
        let record = DiagnosticRequestCapture {
            request_id: request_id.into(),
            timestamp: now_unix(),
            method: method.into(),
            path: path.into(),
            inbound_headers,
            inbound_body,
            inbound_body_bytes: body.len() as u64,
            inbound_body_truncated: body_truncated,
            client_kind: meta.client_kind,
            request_purpose: meta.purpose,
            client_model: meta.client_model.clone(),
            effective_model: meta.effective_model.clone(),
            feature_rule_id: meta.feature_rule_id.clone(),
            client_declared: meta.client_declared.clone(),
            source_format: Some(meta.source_format),
            target_format: meta.target_format,
            route_mode: meta.route_mode,
            attempts: Vec::new(),
            client_chunks: Vec::new(),
            completed_at_ms: None,
            status_code: None,
            outcome: None,
            failure_kind: None,
            failure_detail: None,
            truncated,
        };
        capture.captured_bytes = capture.captured_bytes.saturating_add(record_size(&record));
        capture.records.insert(0, record);
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_window(&capture);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    fn capture_attempt_started(
        &self,
        request_id: &str,
        endpoint: &PlannedEndpoint,
        request: &crate::outbound::OutboundRequest,
        started: Instant,
    ) -> String {
        let id = new_event_id();
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return id;
        }
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return id;
        };
        let started_at_ms =
            ((now_unix() - capture.records[record_index].timestamp) * 1000.0).max(0.0) as i64;
        let protocol = protocol_token(endpoint.protocol).to_string();
        let base_bytes = id.len()
            + endpoint.endpoint_id.len()
            + endpoint.endpoint_name.len()
            + protocol.len()
            + request.pinned_ip.as_ref().map_or(0, String::len)
            + request.method.len();
        let available = capture.max_bytes.saturating_sub(capture.captured_bytes);
        if base_bytes > available {
            capture.records[record_index].truncated = true;
            refresh_capture_usage(&mut capture, true);
            self.sync_capture_index_record(&capture, request_id);
            self.inner.capture_dirty.store(true, Ordering::Release);
            return id;
        }
        let remaining = available - base_bytes;
        let outbound_url_raw = format!("{}{}", request.base_url, request.path_and_query);
        let (outbound_url, url_truncated) = diagnostic_text(outbound_url_raw.as_bytes(), remaining);
        let remaining = remaining.saturating_sub(outbound_url.len());
        let (outbound_headers, headers_truncated) = diagnostic_headers(&request.headers, remaining);
        let header_bytes = outbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>();
        let (outbound_body, body_truncated) =
            diagnostic_text(&request.body, remaining.saturating_sub(header_bytes));
        let truncated = url_truncated || headers_truncated || body_truncated;
        let attempt = DiagnosticAttemptCapture {
            id: id.clone(),
            endpoint_id: endpoint.endpoint_id.clone(),
            endpoint_name: endpoint.endpoint_name.clone(),
            protocol,
            source_format: Some(endpoint.source_format),
            target_format: Some(endpoint.protocol),
            route_mode: Some(endpoint.route_mode),
            pinned_ip: request.pinned_ip.clone(),
            started_at_ms,
            outbound_method: request.method.clone(),
            outbound_url,
            outbound_headers,
            outbound_body,
            outbound_body_bytes: request.body.len() as u64,
            outbound_body_truncated: body_truncated,
            response_status: None,
            response_headers: Vec::new(),
            upstream_chunks: Vec::new(),
            error: None,
            completed_at_ms: None,
        };
        capture.captured_bytes = capture
            .captured_bytes
            .saturating_add(diagnostic_attempt_size(&attempt));
        let record = &mut capture.records[record_index];
        record.truncated |= truncated;
        record.attempts.push(attempt);
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
        if capture.enabled {
            capture.attempt_started.insert(id.clone(), started);
        }
        id
    }

    fn capture_attempt_result(
        &self,
        request_id: &str,
        attempt_id: &str,
        result: &Result<crate::outbound::UpstreamResponse, TransportError>,
    ) {
        let mut capture = self.inner.capture.lock().unwrap();
        let elapsed_ms = capture
            .attempt_started
            .remove(attempt_id)
            .map(|started| started.elapsed().as_millis().min(i64::MAX as u128) as i64);
        let (response_status, response_headers, error_text, truncated) = if capture.enabled {
            let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
            match result {
                Ok(response) => {
                    let (headers, truncated) = diagnostic_headers(&response.headers, remaining);
                    (Some(response.status), Some(headers), None, truncated)
                }
                Err(error) => {
                    let (text, truncated) =
                        diagnostic_text(error.to_string().as_bytes(), remaining);
                    (None, None, Some(text), truncated)
                }
            }
        } else {
            (
                result.as_ref().ok().map(|response| response.status),
                None,
                None,
                false,
            )
        };
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let Some(attempt_index) = capture.records[record_index]
            .attempts
            .iter()
            .position(|attempt| attempt.id == attempt_id)
        else {
            return;
        };
        capture.records[record_index].truncated |= truncated;
        let (old_response_size, new_response_size) = {
            let attempt = &mut capture.records[record_index].attempts[attempt_index];
            let old_response_size = diagnostic_attempt_response_size(attempt);
            attempt.completed_at_ms = elapsed_ms.map(|elapsed| attempt.started_at_ms + elapsed);
            if let Some(status) = response_status {
                attempt.response_status = Some(status);
                if let Some(headers) = response_headers {
                    attempt.response_headers = headers;
                }
            } else {
                attempt.error = error_text;
            }
            (old_response_size, diagnostic_attempt_response_size(attempt))
        };
        if new_response_size >= old_response_size {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_add(new_response_size - old_response_size);
        } else {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_sub(old_response_size - new_response_size);
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    fn capture_upstream_chunk(&self, request_id: &str, attempt_id: &str, bytes: &[u8], at_ms: i64) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let Some(attempt_index) = capture.records[record_index]
            .attempts
            .iter()
            .position(|attempt| attempt.id == attempt_id)
        else {
            return;
        };
        let (data, truncated) = diagnostic_text(bytes, remaining);
        capture.records[record_index].truncated |= truncated;
        if !data.is_empty() {
            capture.captured_bytes = capture.captured_bytes.saturating_add(data.len());
            capture.records[record_index].attempts[attempt_index]
                .upstream_chunks
                .push(DiagnosticChunk {
                    at_ms,
                    bytes: bytes.len() as u64,
                    data,
                    truncated,
                });
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    fn capture_client_chunk(&self, request_id: &str, bytes: &[u8], at_ms: i64) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let (data, truncated) = diagnostic_text(bytes, remaining);
        capture.records[record_index].truncated |= truncated;
        if !data.is_empty() {
            capture.captured_bytes = capture.captured_bytes.saturating_add(data.len());
            capture.records[record_index]
                .client_chunks
                .push(DiagnosticChunk {
                    at_ms,
                    bytes: bytes.len() as u64,
                    data,
                    truncated,
                });
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    fn capture_finish(&self, event: &RuntimeEvent) {
        let mut capture = self.inner.capture.lock().unwrap();
        let request_id = event.request_id.as_deref().unwrap_or_default();
        let capture_enabled = capture.enabled;
        let mut remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let (failure_detail, mut truncated) = match event.failure_detail.as_deref() {
            _ if !capture_enabled => (None, false),
            Some(detail) => {
                let (captured, truncated) = diagnostic_text(detail.as_bytes(), remaining);
                remaining = remaining.saturating_sub(captured.len());
                (Some(captured), truncated)
            }
            None => (None, false),
        };
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let mut size_delta: isize = 0;
        let attempt_ids = {
            let record = &mut capture.records[record_index];
            let old_failure_detail_size = record.failure_detail.as_ref().map_or(0, String::len);
            let new_failure_detail_size = failure_detail.as_ref().map_or(0, String::len);
            size_delta += new_failure_detail_size as isize - old_failure_detail_size as isize;
            record.completed_at_ms = Some(event.duration_ms);
            record.status_code = Some(event.status_code);
            record.outcome = event.outcome;
            record.failure_kind = event.failure_kind;
            record.failure_detail = failure_detail;
            for attempt in &mut record.attempts {
                if attempt.completed_at_ms.is_none() {
                    let old_response_size = diagnostic_attempt_response_size(attempt);
                    attempt.completed_at_ms = Some(event.duration_ms);
                    if attempt.error.is_none() {
                        let (error, error_truncated) = if capture_enabled {
                            diagnostic_text(b"attempt cancelled before response headers", remaining)
                        } else {
                            (String::new(), false)
                        };
                        remaining = remaining.saturating_sub(error.len());
                        if !error.is_empty() {
                            attempt.error = Some(error);
                        }
                        truncated |= error_truncated;
                    }
                    let new_response_size = diagnostic_attempt_response_size(attempt);
                    size_delta += new_response_size as isize - old_response_size as isize;
                }
            }
            record.truncated |= truncated;
            record
                .attempts
                .iter()
                .map(|attempt| attempt.id.clone())
                .collect::<Vec<_>>()
        };
        if size_delta >= 0 {
            capture.captured_bytes = capture.captured_bytes.saturating_add(size_delta as usize);
        } else {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_sub((-size_delta) as usize);
        }
        for attempt_id in attempt_ids {
            capture.attempt_started.remove(&attempt_id);
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub fn config_dir(&self) -> Option<ConfigDir> {
        self.inner.dir.clone()
    }

    pub fn reset_runtime(&self) -> Result<i64, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清空统计".into())
        })?;
        let mut state = self.inner.state.lock().unwrap();
        let previous_runtime = state.runtime.clone();
        let previous_warning = state.stats_durability_warning.clone();
        let previous_error = state.last_error.clone();
        state.runtime = RuntimeSnapshot::default();
        state.stats_durability_warning = None;
        state.last_error = None;
        let generation = match store.reset() {
            Ok(generation) => generation,
            Err(error) => {
                state.runtime = previous_runtime;
                state.stats_durability_warning = previous_warning;
                state.last_error = previous_error;
                return Err(error);
            }
        };
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        Ok(generation)
    }

    pub fn recreate_runtime(&self) -> Result<i64, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.as_ref().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法重置数据库".into())
        })?;
        let mut state = self.inner.state.lock().unwrap();
        let previous_runtime = state.runtime.clone();
        let previous_warning = state.stats_durability_warning.clone();
        let previous_error = state.last_error.clone();
        state.runtime = RuntimeSnapshot::default();
        state.stats_durability_warning = None;
        state.last_error = None;
        let generation = match store.recreate() {
            Ok(generation) => generation,
            Err(error) => {
                state.runtime = previous_runtime;
                state.stats_durability_warning = previous_warning;
                state.last_error = previous_error;
                return Err(error);
            }
        };
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        Ok(generation)
    }

    /// session affinity 的后台防抖任务。SQLite 自己的专用 worker 已按
    /// 1 秒/条数/字节阈值提交，不能在 Tokio 请求线程重复 fsync。
    pub fn spawn_stats_flusher(&self) {
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let flush_engine = engine.clone();
                if let Err(error) = tokio::task::spawn_blocking(move || {
                    flush_engine.flush_session_affinity_if_dirty();
                    flush_engine.flush_resource_bindings_if_dirty();
                    flush_engine.flush_diagnostic_capture_if_dirty();
                })
                .await
                {
                    tracing::warn!("后台持久化任务异常退出: {error}");
                }
            }
        });
    }

    pub fn flush_diagnostic_capture_if_dirty(&self) {
        if !self.inner.capture_writable.load(Ordering::Acquire)
            || !self.inner.capture_dirty.swap(false, Ordering::AcqRel)
        {
            return;
        }
        if let Err(error) = self.flush_diagnostic_capture() {
            self.inner.capture_dirty.store(true, Ordering::Release);
            tracing::warn!("诊断捕获落盘失败: {error}");
        }
    }

    pub fn flush_diagnostic_capture(&self) -> Result<(), String> {
        if !self.inner.capture_writable.load(Ordering::Acquire) {
            return Err("diagnostic_capture.json 加载失败，本次运行拒绝覆盖原文件".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.capture_flush.lock().unwrap();
        let snapshot = {
            let capture = self.inner.capture.lock().unwrap();
            capture_snapshot(&capture)
        };
        match dir.save_diagnostic_capture(&snapshot) {
            Ok(outcome) => match outcome.durability_warning() {
                Some(warning) => Err(warning),
                None => Ok(()),
            },
            Err(error) => Err(format!("diagnostic_capture.json 落盘失败: {error}")),
        }
    }

    pub fn flush_stats_if_dirty(&self) {
        if let Some(store) = &self.inner.runtime_store
            && let Err(error) = store.flush()
        {
            self.set_last_error(Some(error.clone()));
            tracing::warn!("runtime.sqlite3 flush failed: {error}");
        }
    }

    pub fn flush_stats(&self) -> Result<(), String> {
        self.inner
            .runtime_store
            .as_ref()
            .map_or(Ok(()), RuntimeStore::flush)
    }

    pub fn flush_session_affinity_if_dirty(&self) {
        if !self.inner.session_affinity_writable.load(Ordering::Acquire)
            || !self
                .inner
                .session_affinity_dirty
                .swap(false, Ordering::AcqRel)
        {
            return;
        }
        if let Err(error) = self.flush_session_affinity() {
            self.inner
                .session_affinity_dirty
                .store(true, Ordering::Release);
            tracing::warn!("session_affinity.json 落盘失败: {error}");
        }
    }

    pub fn flush_session_affinity(&self) -> Result<(), String> {
        if !self.inner.session_affinity_writable.load(Ordering::Acquire) {
            return Err("session_affinity.json 加载失败，本次运行拒绝覆盖".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.session_affinity_flush.lock().unwrap();
        let assignments = self
            .inner
            .state
            .lock()
            .unwrap()
            .session_sticky
            .iter()
            .filter(|(_, entry)| entry.persistent)
            .map(|(key, entry)| {
                (
                    key.clone(),
                    StickySessionAssignment {
                        scheduling_group: entry.label.clone(),
                        updated_at: entry.at,
                    },
                )
            })
            .collect();
        dir.save_session_affinity(&assignments)
            .map_err(|error| error.to_string())
    }

    pub fn flush_resource_bindings_if_dirty(&self) {
        if !self
            .inner
            .resource_bindings_writable
            .load(Ordering::Acquire)
            || !self
                .inner
                .resource_bindings_dirty
                .swap(false, Ordering::AcqRel)
        {
            return;
        }
        // Snapshotting and writing happen without holding the session maps.
        // A registration can race with the write; consume the dirty bit only
        // after a successful snapshot and immediately flush again when a
        // mutation was observed during the write.  On failure restore the bit
        // so the background flusher can retry instead of silently losing the
        // binding across a restart.
        loop {
            if let Err(error) = self.flush_resource_bindings() {
                self.inner
                    .resource_bindings_dirty
                    .store(true, Ordering::Release);
                tracing::warn!("resource_bindings.json 落盘失败: {error}");
                return;
            }
            if !self
                .inner
                .resource_bindings_dirty
                .swap(false, Ordering::AcqRel)
            {
                return;
            }
        }
    }

    pub fn flush_resource_bindings(&self) -> Result<(), String> {
        if !self
            .inner
            .resource_bindings_writable
            .load(Ordering::Acquire)
        {
            return Err("resource_bindings.json 加载失败，本次运行拒绝覆盖".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.resource_bindings_flush.lock().unwrap();
        let now = now_unix();
        let mut bindings = HashMap::new();
        {
            let sessions = self.inner.live_sessions.lock().unwrap();
            for (id, entry) in sessions.iter().filter(|(_, entry)| entry.expires_at > now) {
                bindings.insert(
                    format!("live:{id}"),
                    ResourceBinding {
                        endpoint_id: entry.endpoint_id.clone(),
                        model: entry.model.clone(),
                        expires_at: entry.expires_at,
                    },
                );
            }
        }
        {
            let sessions = self.inner.video_sessions.lock().unwrap();
            for (id, entry) in sessions.iter().filter(|(_, entry)| entry.expires_at > now) {
                bindings.insert(
                    format!("video:{id}"),
                    ResourceBinding {
                        endpoint_id: entry.endpoint_id.clone(),
                        model: entry.model.clone(),
                        expires_at: entry.expires_at,
                    },
                );
            }
        }
        match dir.save_resource_bindings(&bindings) {
            Ok(outcome) => outcome.durability_warning().map_or(Ok(()), Err),
            Err(error) => Err(error.to_string()),
        }
    }

    pub fn replace_config(&self, config: AppConfig) -> (String, Vec<String>) {
        let warnings = sumpter_core::warnings::evaluate(&config);
        let generation = config_generation(&config);
        *self.inner.config.write().unwrap() = Arc::new(config);
        *self.inner.generation.write().unwrap() = generation.clone();
        let _ = self.inner.notices.send(EngineNotice::ConfigReloaded {
            generation: generation.clone(),
        });
        (generation, warnings)
    }

    /// Reload a validated schema-v6 configuration from the configured
    /// directory.  The old in-memory configuration remains active when
    /// loading, validation, or migration verification fails.
    pub fn reload_config(&self) -> Result<ConfigReplacement, String> {
        let Some(dir) = &self.inner.dir else {
            return Err("no_reload_handler".into());
        };
        let loaded = dir
            .load_config_with_notice()
            .map_err(|error| error.to_string())?;
        let (generation, warnings) = self.replace_config(loaded.config.normalized());
        if let Some(notice) = loaded.migration_notice {
            self.publish_platform_notice(PlatformNotice::Migration(notice));
        }
        Ok(ConfigReplacement {
            generation,
            warnings,
        })
    }

    // -----------------------------------------------------------------------
    // 记账
    // -----------------------------------------------------------------------

    fn record_event(&self, mut event: RuntimeEvent) {
        apply_current_request_context(&mut event);
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let snapshot = {
            let mut state = self.inner.state.lock().unwrap();
            state.runtime.upsert_event(event.clone());
            state.runtime.clone()
        };
        self.enqueue_runtime_change(event.clone(), &snapshot);
        let _ = self.inner.notices.send(EngineNotice::Event(event));
    }

    fn enqueue_runtime_change(&self, event: RuntimeEvent, snapshot: &RuntimeSnapshot) {
        let Some(store) = &self.inner.runtime_store else {
            // SQLite 初始化/版本检查失败时，内存窗口和实时 SSE 仍然可用。
            // 零游标要求客户端重新读取 summary 与最新事件页，不能把它
            // 当成可持久化的 changeSeq 延续。
            let _ = self.inner.notices.send(EngineNotice::RuntimeChange {
                seq: 0,
                change_seq: 0,
                event,
            });
            return;
        };
        match store.enqueue(event.clone(), RuntimeCounters::from_snapshot(snapshot)) {
            Ok(change) => {
                let _ = self.inner.notices.send(EngineNotice::RuntimeChange {
                    seq: change.seq,
                    change_seq: change.change_seq,
                    event: change.event,
                });
            }
            Err(error) => {
                self.set_last_error(Some(error));
                // The in-memory/SSE path remains live even while persistence is
                // degraded. Sequence zero explicitly tells clients to resync.
                let _ = self.inner.notices.send(EngineNotice::RuntimeChange {
                    seq: 0,
                    change_seq: 0,
                    event,
                });
            }
        }
    }

    /// 完成一条 client 事件并按 outcome 计数(一次请求恒计数一次；取消不计成败；
    /// succeeded 清 lastError 自愈；旧事件才回退状态码；failover 每请求最多 +1)。
    fn complete_client(&self, mut event: RuntimeEvent, failed_message: Option<String>) {
        apply_current_request_context(&mut event);
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        // `phase=None` is reserved for legacy stats.json; this path only emits
        // newly finalized events, including rejections and cancellations.
        event.phase = Some(RuntimeEventPhase::Completed);
        self.capture_finish(&event);
        let snapshot = {
            let mut state = self.inner.state.lock().unwrap();
            state.runtime.upsert_event(event.clone());
            state.runtime.client_requests += 1;
            if event.failover {
                state.runtime.failovers += 1;
            }
            if event.is_succeeded() {
                state.runtime.client_successes += 1;
                state.last_error = None;
            } else if event.is_failed() {
                state.runtime.client_failures += 1;
                if let Some(message) = failed_message {
                    state.last_error = Some(message);
                }
            }
            state.runtime.clone()
        };
        self.enqueue_runtime_change(event.clone(), &snapshot);
        let _ = self.inner.notices.send(EngineNotice::Event(event));
    }

    fn complete_upstream(&self, mut event: RuntimeEvent) {
        apply_current_request_context(&mut event);
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        event.phase = Some(RuntimeEventPhase::Completed);
        // Pre-response failures are recorded before `CompletionGuard` owns an
        // upstream attempt. Recover request-level routing/Codex metadata from
        // the in-flight client event so transport/429/tool-compatibility
        // failures are as diagnosable as accepted responses.
        if let Some(request_id) = event.request_id.as_deref() {
            let request_metadata = {
                let state = self.inner.state.lock().unwrap();
                state
                    .runtime
                    .recent_events
                    .iter()
                    .find(|candidate| {
                        candidate.kind == KIND_CLIENT
                            && candidate.request_id.as_deref() == Some(request_id)
                    })
                    .cloned()
            };
            if let Some(client) = request_metadata {
                event.client_model = event.client_model.or(client.client_model);
                event.source_format = event.source_format.or(client.source_format);
                event.target_format = event.target_format.or(client.target_format);
                event.route_mode = event.route_mode.or(client.route_mode);
                event.effective_model = event.effective_model.or(client.effective_model);
                event.feature_rule_id = event.feature_rule_id.or(client.feature_rule_id);
                event.session_id = event.session_id.or(client.session_id);
                event.request_method = event.request_method.or(client.request_method);
                event.request_path = event.request_path.or(client.request_path);
                event.route_intent = event.route_intent.or(client.route_intent);
                event.codex_metadata = event.codex_metadata.or(client.codex_metadata);
                event.client_declared = event.client_declared.or(client.client_declared);
                event.grok_metadata = event.grok_metadata.or(client.grok_metadata);
            }
        }
        let snapshot = {
            let mut state = self.inner.state.lock().unwrap();
            state.runtime.upsert_event(event.clone());
            state.runtime.upstream_attempts += 1;
            if event.is_succeeded() {
                state.runtime.upstream_successes += 1;
            } else if event.is_failed() {
                state.runtime.upstream_failures += 1;
            }
            state.runtime.clone()
        };
        self.enqueue_runtime_change(event.clone(), &snapshot);
        let _ = self.inner.notices.send(EngineNotice::Event(event));
    }

    /// 请求未进入转发就被拒(入站鉴权/请求体解析/路由规划失败):记一条完成态 client
    /// 事件并按失败计数(Swift 版语义;Rust 首版漏掉,事件表看不到这类失败)。
    /// 无 pool/endpoint 归属;message 走词表(specs/spec-engine.md §5.1)。
    #[allow(clippy::too_many_arguments)]
    fn record_rejected_client_with_metadata(
        &self,
        status: i64,
        message: &str,
        client_model: Option<String>,
        purpose: Option<RequestPurpose>,
        client_kind: ClientKind,
        codex_metadata: Option<CodexMetadata>,
        client_declared: Option<ClientDeclaredMetadata>,
        source_format: Option<ProviderProtocol>,
    ) {
        self.record_rejected_client_with_context(
            status,
            message,
            client_model,
            purpose,
            client_kind,
            codex_metadata,
            client_declared,
            source_format,
            current_request_context(),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn record_rejected_client_with_context(
        &self,
        status: i64,
        message: &str,
        client_model: Option<String>,
        purpose: Option<RequestPurpose>,
        client_kind: ClientKind,
        codex_metadata: Option<CodexMetadata>,
        client_declared: Option<ClientDeclaredMetadata>,
        source_format: Option<ProviderProtocol>,
        request_context: Option<InboundRequestContext>,
    ) {
        let event_id = new_event_id();
        let (request_method, request_path, route_intent, context_session_id, grok_metadata) =
            request_context
                .map(|context| {
                    (
                        Some(context.method),
                        Some(context.path),
                        Some(context.route_intent),
                        context.session_id,
                        context.grok_metadata,
                    )
                })
                .unwrap_or((None, None, None, None, None));
        // A body/header Codex metadata projection is safe to use for
        // attribution even when the caller passed a stale header-only kind.
        // Never let it override a positively identified Claude/Grok client.
        let client_kind = if matches!(client_kind, ClientKind::OpenaiCompat | ClientKind::Unknown)
            && codex_metadata
                .as_ref()
                .and_then(|metadata| metadata.originator.as_deref())
                .is_some_and(is_codex_originator)
        {
            ClientKind::Codex
        } else {
            client_kind
        };
        let session_id = context_session_id
            .or_else(|| {
                grok_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.session_id.clone().or(metadata.conv_id.clone()))
            })
            .or_else(|| {
                codex_metadata
                    .as_ref()
                    .and_then(|metadata| metadata.session_id.clone())
            });
        let message = bounded_failure_detail(message);
        let event = RuntimeEvent {
            client_kind: Some(client_kind),
            codex_metadata: retain_codex_metadata_for_client(client_kind, codex_metadata),
            client_declared,
            grok_metadata,
            client_model,
            // 即使请求在解析/规划前被拒,入口路径已经足以确定入站方言。
            // target/route 仍保持 None,因为没有真实上游尝试。
            source_format,
            target_format: None,
            route_mode: None,
            duration_ms: 0,
            effective_model: None,
            endpoint_id: None,
            endpoint_name: None,
            failover: false,
            feature_rule_id: None,
            failure_detail: Some(message.clone()),
            failure_kind: Some(RuntimeFailureKind::ClientRequestRejected),
            failure_phase: Some(RuntimeFailurePhase::BeforeResponse),
            id: event_id.clone(),
            kind: KIND_CLIENT.into(),
            message: Some(message.clone()),
            tool_calls: None,
            outcome: Some(RuntimeEventOutcome::Failed),
            phase: Some(RuntimeEventPhase::Completed),
            pool_id: None,
            request_purpose: purpose,
            request_id: Some(event_id),
            request_method,
            request_path,
            route_intent,
            session_id,
            status_code: status,
            timestamp: unix_to_apple_epoch(now_unix()),
            // 请求在规划/鉴权阶段就被拒,从未 accepted。
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: None,
            upstream_model: None,
            upstream_request_id: None,
            upstream_status_code: None,
        };
        self.complete_client(event, Some(message));
    }

    /// Record an early WebSocket rejection before a CompletionGuard exists.
    /// The event keeps only bounded path/method/intent metadata and never
    /// stores query values, frame bodies, SDP, or credentials.
    pub fn record_rejected_websocket(
        &self,
        path_and_query: &str,
        headers: &[(String, String)],
        status: u16,
        message: &str,
    ) {
        self.record_rejected_websocket_with_metadata(
            path_and_query,
            headers,
            status,
            message,
            None,
            None,
        );
    }

    /// Variant used after a Responses WebSocket first frame has been buffered.
    /// Codex Desktop may put the only `originator`/`session_id` marker in that
    /// frame; preserving the bounded metadata keeps an early model/path
    /// rejection attributable without storing the frame body.
    fn record_rejected_websocket_with_metadata(
        &self,
        path_and_query: &str,
        headers: &[(String, String)],
        status: u16,
        message: &str,
        frame_metadata: Option<CodexMetadata>,
        frame_model: Option<String>,
    ) {
        let context = self.inbound_request_context("GET", path_and_query, headers);
        let header_metadata = CodexMetadata::from_request(headers, None);
        let codex_metadata = merge_codex_metadata(header_metadata, frame_metadata);
        let client_kind = codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.originator.as_deref())
            .filter(|originator| is_codex_originator(originator))
            .map_or_else(|| detect_client_kind(headers, true), |_| ClientKind::Codex);
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| decoded_query_value(query, "model"))
            .filter(|model| !model.trim().is_empty());
        let frame_model = frame_model.and_then(|model| {
            let model = model.trim();
            (!model.is_empty() && model.len() <= 256 && !model.chars().any(char::is_control))
                .then(|| model.to_string())
        });
        self.record_rejected_client_with_context(
            i64::from(status),
            message,
            query_model.or(frame_model),
            Some(RequestPurpose::Standard),
            client_kind,
            codex_metadata,
            ClientDeclaredMetadata::from_headers(headers),
            Some(ProviderProtocol::OpenAI),
            Some(context),
        );
    }

    fn touch_ip(&self, ip: &str, connected: bool, now: f64) {
        let mut state = self.inner.state.lock().unwrap();
        let entry = state.ip_health.entry(ip.to_string()).or_default();
        if connected {
            entry.last_success = Some(now);
            entry.cooling_until = None;
        } else {
            entry.cooling_until = Some(now + IP_COOLDOWN_SECS);
        }
    }

    fn note_provider_model_success(&self, endpoint: &PlannedEndpoint) {
        let key = provider_model_key(endpoint);
        self.inner
            .state
            .lock()
            .unwrap()
            .provider_model_health
            .remove(&key);
    }

    fn note_provider_model_failure(
        &self,
        endpoint: &PlannedEndpoint,
        status: Option<u16>,
        retry_after: Option<f64>,
        now: f64,
    ) {
        let key = provider_model_key(endpoint);
        let mut state = self.inner.state.lock().unwrap();
        // Expired entries do not need to survive another failure and pruning
        // here bounds the map even on a high-cardinality raw route workload.
        state.provider_model_health.retain(|_, health| {
            health
                .cooling_until
                .is_some_and(|until| until.is_finite() && until > now)
        });
        if state.provider_model_health.len() >= MAX_PROVIDER_MODEL_HEALTH
            && !state.provider_model_health.contains_key(&key)
            && let Some(oldest) = state
                .provider_model_health
                .iter()
                .min_by(|(_, left), (_, right)| {
                    left.cooling_until
                        .unwrap_or(f64::INFINITY)
                        .total_cmp(&right.cooling_until.unwrap_or(f64::INFINITY))
                })
                .map(|(key, _)| key.clone())
        {
            state.provider_model_health.remove(&oldest);
        }
        let health = state.provider_model_health.entry(key).or_default();
        health.consecutive_failures = health.consecutive_failures.saturating_add(1);
        health.last_status = status;
        let exponential = provider_model_cooldown_seconds(health.consecutive_failures);
        let retry_after = retry_after
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .unwrap_or(0.0)
            .min(MAX_RETRY_AFTER_SECS);
        let until = now + exponential.max(retry_after);
        health.cooling_until = Some(
            health
                .cooling_until
                .filter(|existing| existing.is_finite() && *existing > now)
                .map_or(until, |existing| existing.max(until)),
        );
    }

    fn touch_session_success(
        &self,
        group: &str,
        session_key: &sticky::SessionKey,
        initial_group: &str,
        eligible_groups: &[String],
        sticky_enabled: bool,
        now: f64,
    ) {
        let mut state = self.inner.state.lock().unwrap();
        // 统一 Provider 的其它调度组一旦成功立即改绑。
        let replace = sticky_enabled
            && scheduler::should_replace_sticky_assignment(
                state
                    .session_sticky
                    .get(&session_key.value)
                    .map(|entry| entry.label.as_str()),
                group,
                initial_group,
                eligible_groups,
            );
        if replace {
            state.session_sticky.insert(
                session_key.value.clone(),
                SessionStickyEntry {
                    label: group.to_string(),
                    at: now,
                    persistent: session_key.persistent,
                },
            );
            if session_key.persistent {
                self.inner
                    .session_affinity_dirty
                    .store(true, Ordering::Release);
            }
        }
        let needs_prune = now - state.last_session_prune_at >= SESSION_STICKY_PRUNE_INTERVAL_SECS
            || state.session_sticky.len() > SESSION_STICKY_MAX_ENTRIES;
        if needs_prune {
            let removed_persistent = prune_session_sticky(&mut state.session_sticky, now);
            state.last_session_prune_at = now;
            if removed_persistent {
                self.inner
                    .session_affinity_dirty
                    .store(true, Ordering::Release);
            }
        }
        drop(state);
        if session_key.persistent
            && replace
            && let Err(error) = self.flush_session_affinity()
        {
            self.inner
                .session_affinity_dirty
                .store(true, Ordering::Release);
            tracing::warn!("session_affinity.json 即时落盘失败: {error}");
        }
    }

    /// 稳定 Claude 会话在首次出站前就建立固定归属，避免请求尚未成功时进程退出导致漂移。
    /// 配置删除/改名使旧组失效时，允许把归属更新为当前计划的首选组。
    fn ensure_session_assignment(
        &self,
        session_key: &sticky::SessionKey,
        initial_group: &str,
        eligible_groups: &[String],
        now: f64,
    ) {
        if !session_key.persistent || initial_group.is_empty() {
            return;
        }
        let mut state = self.inner.state.lock().unwrap();
        let replace = scheduler::should_replace_sticky_assignment(
            state
                .session_sticky
                .get(&session_key.value)
                .map(|entry| entry.label.as_str()),
            initial_group,
            initial_group,
            eligible_groups,
        );
        if !replace {
            return;
        }
        state.session_sticky.insert(
            session_key.value.clone(),
            SessionStickyEntry {
                label: initial_group.to_string(),
                at: now,
                persistent: true,
            },
        );
        self.inner
            .session_affinity_dirty
            .store(true, Ordering::Release);
        drop(state);
        if let Err(error) = self.flush_session_affinity() {
            tracing::warn!("session_affinity.json 首次归属落盘失败: {error}");
        }
    }

    // -----------------------------------------------------------------------
    // 入站分发
    // -----------------------------------------------------------------------

    /// Compatibility entry point used by the legacy server adapters.  New
    /// callers should pass the typed [`InboundRequest`] envelope so method,
    /// URI and headers retain their HTTP semantics at the boundary.
    pub async fn handle_request(
        &self,
        remote: Option<IpAddr>,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: impl Into<Body>,
    ) -> Response {
        self.handle_request_parts(remote, method, path_and_query, headers, body.into())
            .await
    }

    /// Typed HTTP entry point for the unified engine.  Conversion to the
    /// existing string-based adapter happens only at the last possible point;
    /// importantly, the body is moved untouched and remains lazy.
    pub async fn handle_inbound_request(&self, request: InboundRequest) -> Response {
        let InboundRequest {
            method,
            uri,
            headers,
            remote_ip,
            body,
        } = request;
        let path_and_query = inbound::path_and_query(&uri);
        let headers = inbound::header_pairs(&headers);
        self.handle_request_parts(remote_ip, method.as_str(), &path_and_query, headers, body)
            .await
    }

    /// Authenticate a WebSocket upgrade before accepting it.  The normal HTTP
    /// path performs the same CIDR/token checks in `handle_request_parts`; a
    /// dedicated helper keeps rejected upgrades at HTTP 401/403 instead of
    /// returning a misleading 101 and only failing after the first frame.
    pub fn authorize_websocket(
        &self,
        remote: Option<IpAddr>,
        headers: &[(String, String)],
    ) -> bool {
        self.authorize_websocket_with_ephemeral(remote, headers, false)
    }

    /// Realtime accepts the short-lived `ek_…` key returned by
    /// `/v1/realtime/client_secrets` in addition to the listener token.
    pub fn authorize_realtime_websocket(
        &self,
        remote: Option<IpAddr>,
        headers: &[(String, String)],
    ) -> bool {
        self.authorize_websocket_with_ephemeral(remote, headers, true)
    }

    fn authorize_websocket_with_ephemeral(
        &self,
        remote: Option<IpAddr>,
        headers: &[(String, String)],
        allow_ephemeral: bool,
    ) -> bool {
        let config = self.config();
        access::is_allowed(
            remote.map(|ip| ip.to_string()).as_deref(),
            &config.listener.allowed_cidrs,
        ) && (!config.listener.has_inbound_auth()
            || inbound_auth_ok(headers, &config.listener.auth_token)
            || (allow_ephemeral && self.realtime_client_secret_authorized(headers)))
    }

    fn record_websocket_prepare_failure(
        &self,
        context: &WebSocketEventContext,
        error: &WebSocketPrepareError,
    ) {
        let status = if error.status == 0 {
            502
        } else {
            i64::from(error.status)
        };
        // Keep the upstream event's status at zero when no HTTP handshake
        // response existed. The client still receives a conventional 502,
        // but diagnostics must not claim that the provider returned HTTP 502.
        let upstream_status = i64::from(error.status);
        let mut failure = if error.status > 0 {
            FailureInfo::upstream_http(error.status, None)
        } else {
            FailureInfo {
                kind: RuntimeFailureKind::ConnectionFailed,
                phase: RuntimeFailurePhase::BeforeResponse,
                detail: Some(error.message.clone()),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: error.retry_after_seconds,
            }
        };
        failure.detail = Some(error.message.clone());
        failure.retry_after_seconds = error.retry_after_seconds;
        let trace = websocket_trace(
            error.status.into(),
            0,
            0,
            0,
            0,
            None,
            None,
            None,
            None,
            false,
            error.attempts,
        );
        let endpoint = error.endpoint.as_ref();
        let mut client = websocket_client_event(
            context,
            endpoint,
            status,
            error.attempts > 1,
            Some(trace.clone()),
            Some(&failure),
        );
        client.message = Some(error.message.clone());
        // Keep the client-side latency distinct from an upstream attempt's
        // TTFB.  When the provider returned an HTTP response, the attempt
        // TTFB is the best bounded handshake signal available; when the
        // connection failed before response headers it must remain unknown.
        client.ttfb_ms = error.ttfb_ms;
        self.complete_client(client, Some(error.message.clone()));
        if let Some(endpoint) = endpoint {
            let upstream_duration = error.ttfb_ms.unwrap_or_else(|| {
                context.started.elapsed().as_millis().min(i64::MAX as u128) as i64
            });
            let mut upstream = self.upstream_event(
                endpoint,
                upstream_status,
                upstream_duration,
                error.attempts > 1,
                Some("websocket handshake failed".into()),
                RequestPurpose::Standard,
                context.client_kind,
                &context.request_id,
            );
            upstream.codex_metadata = context.codex_metadata.clone();
            upstream.client_declared = context.client_declared.clone();
            upstream.grok_metadata = context.grok_metadata.clone();
            upstream.request_method = Some("GET".into());
            upstream.request_path = Some(context.request_path.clone());
            upstream.route_intent = Some(context.route_intent.clone());
            upstream.client_model = Some(context.model.clone());
            upstream.effective_model = Some(context.model.clone());
            upstream.ttfb_ms = error.ttfb_ms;
            upstream.stream_trace = Some(trace);
            failure.apply_to(&mut upstream);
            self.complete_upstream(upstream);
        }
    }

    fn record_websocket_completion(
        &self,
        context: &WebSocketEventContext,
        connection: &ConnectedWebSocket,
        metrics: &WebSocketRelayMetrics,
    ) {
        let context = websocket_context_with_first_frame(
            context,
            metrics.first_client_codex_metadata.as_ref(),
        );
        let status = 101_i64;
        let trace = websocket_trace(
            101,
            metrics.bytes_sent,
            metrics.bytes_received,
            metrics.client_message_count,
            metrics.upstream_message_count,
            metrics.client_close_code,
            metrics.upstream_close_code,
            metrics.closed_by.clone(),
            metrics.relay_error.clone(),
            metrics.abnormal_close,
            connection.attempt_count,
        );
        let failure = metrics.failed.then(|| FailureInfo {
            kind: RuntimeFailureKind::StreamInterrupted,
            phase: RuntimeFailurePhase::ResponseStream,
            detail: Some("websocket relay interrupted".into()),
            timeout_ms: None,
            upstream_status_code: Some(status),
            upstream_request_id: None,
            retry_after_seconds: None,
        });
        let mut upstream = self.upstream_event(
            &connection.endpoint,
            status,
            metrics.duration_ms,
            connection.failover,
            Some("websocket relay".into()),
            RequestPurpose::Standard,
            context.client_kind,
            &context.request_id,
        );
        upstream.codex_metadata = context.codex_metadata.clone();
        upstream.client_declared = context.client_declared.clone();
        upstream.grok_metadata = context.grok_metadata.clone();
        upstream.request_method = Some("GET".into());
        upstream.request_path = Some(context.request_path.clone());
        upstream.route_intent = Some(context.route_intent.clone());
        upstream.client_model = Some(context.model.clone());
        upstream.effective_model = Some(context.model.clone());
        upstream.ttfb_ms = Some(connection.handshake_ttfb_ms);
        upstream.stream_trace = Some(trace.clone());
        if let Some(failure) = &failure {
            failure.apply_to(&mut upstream);
        } else {
            upstream.outcome = Some(RuntimeEventOutcome::Succeeded);
        }
        self.complete_upstream(upstream);

        let mut client = websocket_client_event(
            &context,
            Some(&connection.endpoint),
            status,
            connection.failover,
            Some(trace),
            failure.as_ref(),
        );
        client.duration_ms = metrics.duration_ms;
        client.ttfb_ms = Some(connection.handshake_ttfb_ms);
        self.complete_client(
            client,
            failure.as_ref().and_then(|failure| failure.detail.clone()),
        );
    }

    /// Dial a standard Realtime/Live upstream before the adapter sends the
    /// downstream 101 response.  Responses WebSocket connections without a
    /// query model still use the deferred path because their model is carried
    /// by the first post-upgrade `response.create` frame.
    pub async fn prepare_realtime_websocket(
        &self,
        path_and_query: &str,
        headers: &[(String, String)],
    ) -> Result<PreparedWebSocket, WebSocketPrepareError> {
        let started = Instant::now();
        let path = path_without_query(path_and_query);
        if !is_realtime_http_path(path) {
            let error = WebSocketPrepareError::new(
                400,
                "invalid_request",
                "websocket path is not a Realtime endpoint",
            );
            self.record_rejected_websocket(path_and_query, headers, error.status, error.message());
            return Err(error);
        }
        if let Err(error) = validate_realtime_call_target(path_and_query) {
            let (status, code, message) = realtime_call_path_error(error);
            let prepared = WebSocketPrepareError::new(status.as_u16(), code, message);
            self.record_rejected_websocket(path_and_query, headers, prepared.status, message);
            return Err(prepared);
        }
        let sideband_model = self
            .live_session_model(path_and_query)
            .flatten()
            .filter(|model| !model.trim().is_empty());
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| decoded_query_value(query, "model"))
            .filter(|model| !model.trim().is_empty());
        let secret_model = self.realtime_client_secret_model(headers);
        let client_kind = detect_client_kind(headers, true);
        let intent = classify_realtime_intent(
            "GET",
            path_and_query,
            query_model.as_deref(),
            sideband_model.as_deref(),
            secret_model.as_deref(),
            client_kind,
            false,
        );
        let model = resolve_realtime_route_model(
            &self.config(),
            "GET",
            path_and_query,
            query_model,
            sideband_model,
            secret_model,
            intent,
        );
        let request = match RoutingRequest::from_value(&json!({"model": model})) {
            Some(request) => request,
            None => {
                let error =
                    WebSocketPrepareError::new(400, "invalid_request", "invalid websocket model");
                self.record_rejected_websocket(
                    path_and_query,
                    headers,
                    error.status,
                    error.message(),
                );
                return Err(error);
            }
        };
        let context =
            websocket_event_context(path_and_query, headers, &request.model, intent, started);
        match self
            .connect_native_websocket(path, path_and_query, headers, &request)
            .await
        {
            Ok(connection) => Ok(PreparedWebSocket {
                connection,
                context,
            }),
            Err(error) => {
                self.record_websocket_prepare_failure(&context, &error);
                Err(error)
            }
        }
    }

    /// Relay an already-handshaken upstream after the adapter completes the
    /// downstream upgrade.
    pub async fn handle_prepared_websocket(
        &self,
        socket: WebSocket,
        remote: Option<IpAddr>,
        prepared: PreparedWebSocket,
    ) {
        let mut connection = prepared.connection;
        let upstream = connection
            .upstream
            .take()
            .expect("prepared websocket owns an upstream connection");
        let metrics = relay_native_websocket(socket, upstream, None).await;
        self.record_websocket_completion(&prepared.context, &connection, &metrics);
        let _ = remote;
    }

    /// Compatibility wrapper for callers that name the Responses route. The
    /// data plane still uses the same protocol-agnostic WebSocket relay.
    pub async fn handle_responses_websocket(
        &self,
        socket: WebSocket,
        remote: Option<IpAddr>,
        path_and_query: String,
        headers: Vec<(String, String)>,
    ) {
        self.handle_native_websocket(socket, remote, path_and_query, headers)
            .await;
    }

    /// Compatibility wrapper for callers that name the Realtime/Live route.
    /// Text, binary, ping/pong and close frames are forwarded unchanged.
    pub async fn handle_realtime_websocket(
        &self,
        socket: WebSocket,
        remote: Option<IpAddr>,
        path_and_query: String,
        headers: Vec<(String, String)>,
    ) {
        self.handle_native_websocket(socket, remote, path_and_query, headers)
            .await;
    }

    /// Relay any upgraded data-plane connection. Provider selection uses only
    /// configured mappings; the original path/query and every WebSocket frame
    /// are forwarded unchanged.
    pub async fn handle_websocket(
        &self,
        socket: WebSocket,
        remote: Option<IpAddr>,
        path_and_query: String,
        headers: Vec<(String, String)>,
    ) {
        self.handle_native_websocket(socket, remote, path_and_query, headers)
            .await;
    }

    async fn handle_native_websocket(
        &self,
        mut socket: WebSocket,
        remote: Option<IpAddr>,
        path_and_query: String,
        headers: Vec<(String, String)>,
    ) {
        let path = path_without_query(&path_and_query);
        if let Err(error) = validate_realtime_call_target(&path_and_query) {
            let (status, code, message) = realtime_call_path_error(error);
            self.record_rejected_websocket(&path_and_query, &headers, status.as_u16(), message);
            let _ = send_websocket_json_error(&mut socket, code, message).await;
            return;
        }
        let mut initial_message = None;
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| decoded_query_value(query, "model"))
            .filter(|model| !model.trim().is_empty());
        let sideband_model = self
            .live_session_model(&path_and_query)
            .flatten()
            .filter(|model| !model.trim().is_empty());
        let secret_model = self.realtime_client_secret_model(&headers);
        let model = if is_realtime_http_path(path) {
            let client_kind = detect_client_kind(&headers, true);
            let intent = classify_realtime_intent(
                "GET",
                &path_and_query,
                query_model.as_deref(),
                sideband_model.as_deref(),
                secret_model.as_deref(),
                client_kind,
                false,
            );
            Some(resolve_realtime_route_model(
                &self.config(),
                "GET",
                &path_and_query,
                query_model,
                sideband_model,
                secret_model,
                intent,
            ))
        } else if query_model.is_some() {
            query_model
        } else if is_responses_websocket_path(path) {
            // CPA selects a Responses WebSocket from the first
            // `response.create` frame when the query has no model. Buffer
            // that frame so it can still be relayed after route planning.
            let frame_model = match socket.recv().await {
                Some(Ok(message)) => {
                    let model = websocket_message_model(&message);
                    initial_message = Some(message);
                    model
                }
                Some(Err(error)) => {
                    self.record_rejected_websocket(
                        &path_and_query,
                        &headers,
                        400,
                        &format!("invalid websocket frame: {error}"),
                    );
                    let _ = send_websocket_json_error(
                        &mut socket,
                        "invalid_request",
                        &format!("invalid websocket frame: {error}"),
                    )
                    .await;
                    return;
                }
                None => return,
            };
            frame_model.or(sideband_model).or(secret_model)
        } else {
            None
        };
        let Some(model) = model else {
            let frame_metadata = initial_message
                .as_ref()
                .and_then(websocket_message_codex_metadata);
            self.record_rejected_websocket_with_metadata(
                &path_and_query,
                &headers,
                400,
                "websocket model is required (query model or first response.create frame)",
                frame_metadata,
                initial_message.as_ref().and_then(websocket_message_model),
            );
            let _ = send_websocket_json_error(
                &mut socket,
                "invalid_request",
                "websocket model is required (query model or first response.create frame)",
            )
            .await;
            return;
        };
        let request = RoutingRequest::from_value(&json!({"model": model}));
        let Some(request) = request else {
            let frame_metadata = initial_message
                .as_ref()
                .and_then(websocket_message_codex_metadata);
            self.record_rejected_websocket_with_metadata(
                &path_and_query,
                &headers,
                400,
                "invalid websocket model",
                frame_metadata,
                initial_message.as_ref().and_then(websocket_message_model),
            );
            let _ = send_websocket_json_error(
                &mut socket,
                "invalid_request",
                "invalid websocket model",
            )
            .await;
            return;
        };
        let intent = if is_realtime_http_path(path) {
            classify_realtime_intent(
                "GET",
                &path_and_query,
                Some(&request.model),
                self.live_session_model(&path_and_query)
                    .flatten()
                    .as_deref(),
                self.realtime_client_secret_model(&headers).as_deref(),
                detect_client_kind(&headers, true),
                false,
            )
        } else {
            RealtimeRouteIntent::StandardRealtime
        };
        let context = websocket_event_context(
            &path_and_query,
            &headers,
            &request.model,
            intent,
            Instant::now(),
        );
        // Responses WebSocket clients may put the only Codex identity marker
        // on the buffered `response.create` frame.  Merge the bounded
        // metadata before dialing upstream so handshake failures are
        // attributed the same way as successful relays.
        let first_frame_metadata = initial_message
            .as_ref()
            .and_then(websocket_message_codex_metadata);
        let context = websocket_context_with_first_frame(&context, first_frame_metadata.as_ref());
        let mut connection = match self
            .connect_native_websocket(path, &path_and_query, &headers, &request)
            .await
        {
            Ok(connection) => connection,
            Err(error) => {
                self.record_websocket_prepare_failure(&context, &error);
                let _ = send_websocket_json_error(&mut socket, error.error_code(), error.message())
                    .await;
                return;
            }
        };
        let upstream = connection
            .upstream
            .take()
            .expect("connected websocket owns an upstream connection");
        let metrics = relay_native_websocket(socket, upstream, initial_message).await;
        self.record_websocket_completion(&context, &connection, &metrics);
        let _ = remote;
    }

    async fn connect_native_websocket(
        &self,
        path: &str,
        path_and_query: &str,
        headers: &[(String, String)],
        request: &RoutingRequest,
    ) -> Result<ConnectedWebSocket, WebSocketPrepareError> {
        if let Err(error) = validate_realtime_call_target(path_and_query) {
            let (status, code, message) = realtime_call_path_error(error);
            return Err(WebSocketPrepareError::new(status.as_u16(), code, message));
        }
        if matches!(self.live_session_endpoint(path_and_query), Some(None)) {
            return Err(WebSocketPrepareError::new(
                if is_codex_live_sideband_target(path_and_query) {
                    410
                } else {
                    404
                },
                if is_codex_live_sideband_target(path_and_query) {
                    "live_session_expired"
                } else {
                    "realtime_call_not_found"
                },
                "Realtime session is unknown or expired",
            ));
        }
        let config = self.config();
        let mut plan = match if is_realtime_http_path(path) {
            RoutePlanner::plan_for_capability(
                request,
                &config,
                ProviderProtocol::OpenAI,
                sumpter_core::capability::ModelCapability::Live,
            )
        } else {
            RoutePlanner::plan_for_passthrough(request, &config, ProviderProtocol::OpenAI)
        } {
            Ok(plan) => plan,
            Err(error) => {
                return Err(WebSocketPrepareError::new(
                    if is_codex_live_family_path(path) {
                        503
                    } else {
                        400
                    },
                    if is_codex_live_family_path(path) {
                        "no_live_provider"
                    } else {
                        "invalid_request"
                    },
                    error.to_string(),
                ));
            }
        };
        // Anthropic endpoints cannot speak the OpenAI Realtime/Quicksilver
        // wire protocol. Keep voice traffic on an exact mapping only.
        if is_realtime_http_path(path) {
            let client_kind = detect_client_kind(headers, true);
            let query_model = path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"));
            let sideband_model = self.live_session_model(path_and_query).flatten();
            let secret_model = self.realtime_client_secret_model(headers);
            let intent = classify_realtime_intent(
                "GET",
                path_and_query,
                query_model.as_deref(),
                sideband_model.as_deref(),
                secret_model.as_deref(),
                client_kind,
                false,
            );
            let codex_live = intent == RealtimeRouteIntent::CodexLive;
            plan.endpoints.retain(|endpoint| {
                endpoint.protocol != ProviderProtocol::Anthropic
                    && has_exact_realtime_mapping(&config, &endpoint.endpoint_id, &request.model)
                    && (!codex_live || has_exact_codex_live_mapping(&config, &endpoint.endpoint_id))
            });
            if plan.endpoints.is_empty() {
                return Err(WebSocketPrepareError::new(
                    if codex_live { 503 } else { 400 },
                    if codex_live {
                        "no_live_provider"
                    } else {
                        "no_compatible_protocol"
                    },
                    "no Realtime-compatible Provider",
                ));
            }
        }
        if let Some(session) = self.live_session_endpoint(path_and_query) {
            match session {
                Some(endpoint_id) => {
                    plan.endpoints
                        .retain(|endpoint| endpoint.endpoint_id == endpoint_id);
                    if plan.endpoints.is_empty() {
                        return Err(WebSocketPrepareError::new(
                            if is_codex_live_sideband_target(path_and_query) {
                                410
                            } else {
                                404
                            },
                            if is_codex_live_sideband_target(path_and_query) {
                                "live_session_expired"
                            } else {
                                "realtime_call_not_found"
                            },
                            "Realtime session is no longer available on its originating endpoint",
                        ));
                    }
                }
                None => {
                    return Err(WebSocketPrepareError::new(
                        if is_codex_live_sideband_target(path_and_query) {
                            410
                        } else {
                            404
                        },
                        if is_codex_live_sideband_target(path_and_query) {
                            "live_session_expired"
                        } else {
                            "realtime_call_not_found"
                        },
                        "Realtime session is unknown or expired",
                    ));
                }
            }
        }
        if let Some(expected_model) = self.realtime_client_secret_model(headers)
            && !realtime_client_secret_models_match(&expected_model, &request.model)
        {
            return Err(WebSocketPrepareError::new(
                403,
                "realtime_client_secret_scope_mismatch",
                "Realtime client secret is not valid for the requested model",
            ));
        }
        if let Some(endpoint_id) = self.realtime_client_secret_endpoint(headers) {
            plan.endpoints
                .retain(|endpoint| endpoint.endpoint_id == endpoint_id);
            if plan.endpoints.is_empty() {
                return Err(WebSocketPrepareError::new(
                    403,
                    "realtime_client_secret_scope_mismatch",
                    "Realtime client secret is not valid for the requested endpoint",
                ));
            }
        }
        let ephemeral = realtime_ephemeral_token(headers);
        let session = self.realtime_client_secret_session(headers);
        let route_intent = classify_realtime_intent(
            "GET",
            path_and_query,
            path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"))
                .as_deref(),
            self.live_session_model(path_and_query).flatten().as_deref(),
            self.realtime_client_secret_model(headers).as_deref(),
            detect_client_kind(headers, true),
            false,
        );
        let ordering_key = sticky::SessionKey {
            value: format!("websocket:{}", request.model),
            persistent: false,
        };
        let ordered = self.ordered_endpoints(&plan, &ordering_key);
        if ordered.is_empty()
            && let Some(retry_after) = self.provider_model_cooldown_retry_after(&plan.endpoints)
        {
            let mut error = WebSocketPrepareError::new(
                503,
                "provider_cooldown",
                "all compatible providers are cooling down",
            );
            error.retry_after_seconds = Some(retry_after);
            return Err(error);
        }
        let mut last_error = None;
        let mut attempt_count = 0_u64;
        for endpoint in &ordered {
            attempt_count = attempt_count.saturating_add(1);
            let attempt_started = Instant::now();
            // avas/quicksilver query markers are WebRTC HTTP-only.  Forwarding
            // them on GET `/v1/realtime` makes OpenAI/CPA reply
            // `invalid_architecture` instead of opening a standard Realtime WS.
            let outbound_path_and_query = request_build::strip_codex_live_query(path_and_query);
            let Some(url) = websocket_url(&endpoint.base_url, &outbound_path_and_query) else {
                let error = WebSocketPrepareError::new(
                    0,
                    "upstream_error",
                    "invalid realtime upstream URL",
                )
                .with_attempt(endpoint, attempt_count, None, None);
                last_error = Some(error);
                continue;
            };
            let url = if is_realtime_http_path(path)
                && route_intent == RealtimeRouteIntent::StandardRealtime
            {
                request_build::rewrite_realtime_model_query(
                    &url,
                    &request.model,
                    &endpoint.upstream_model,
                )
            } else {
                url
            };
            let api_key = self
                .config()
                .endpoint(&endpoint.endpoint_id)
                .map(|endpoint| endpoint.api_key.clone())
                .unwrap_or_default();
            let mut upstream_request = match url
                .into_client_request()
                .map_err(|error| WebSocketPrepareError::new(0, "upstream_error", error.to_string()))
            {
                Ok(request) => request,
                Err(error) => {
                    last_error = Some(error.with_attempt(endpoint, attempt_count, None, None));
                    continue;
                }
            };
            for (name, value) in websocket_upstream_headers(headers) {
                if route_intent == RealtimeRouteIntent::StandardRealtime
                    && name.eq_ignore_ascii_case("openai-alpha")
                {
                    continue;
                }
                if let Ok(name) = http::header::HeaderName::from_bytes(name.as_bytes())
                    && let Ok(value) = http::header::HeaderValue::from_str(&value)
                {
                    upstream_request.headers_mut().append(name, value);
                }
            }
            if route_intent == RealtimeRouteIntent::CodexLive {
                // Match CPA's Live protocol identity even when the client did
                // not include the optional headers.  This is scoped to the
                // classified Live connection; standard Realtime remains free
                // of the Quicksilver alpha marker.
                upstream_request.headers_mut().insert(
                    http::header::HeaderName::from_static("originator"),
                    http::header::HeaderValue::from_static("Codex Desktop"),
                );
                upstream_request.headers_mut().insert(
                    http::header::HeaderName::from_static("openai-alpha"),
                    http::header::HeaderValue::from_static("quicksilver=v2"),
                );
            }
            let provider_headers = ephemeral
                .as_deref()
                .map(|token| vec![("authorization", format!("Bearer {token}"))])
                .unwrap_or_else(|| request_build::provider_auth_headers(&api_key));
            for (name, value) in provider_headers {
                if let Ok(name) = http::header::HeaderName::from_bytes(name.as_bytes())
                    && let Ok(value) = http::header::HeaderValue::from_str(&value)
                {
                    upstream_request.headers_mut().insert(name, value);
                }
            }
            let connect_result = tokio::time::timeout(
                Duration::from_secs_f64(REALTIME_WEBSOCKET_CONNECT_TIMEOUT_SECS),
                tokio_tungstenite::connect_async(upstream_request),
            )
            .await;
            match connect_result {
                Ok(Ok((mut upstream, _))) => {
                    if let Some(session) = session.as_ref()
                        && let Err(error) = send_realtime_session_update(
                            &mut upstream,
                            session,
                            &endpoint.upstream_model,
                        )
                        .await
                    {
                        last_error = Some(
                            WebSocketPrepareError::new(
                                0,
                                "upstream_error",
                                format!("failed to apply Realtime session: {error}"),
                            )
                            .with_attempt(
                                endpoint,
                                attempt_count,
                                Some(attempt_started.elapsed().as_millis() as i64),
                                None,
                            ),
                        );
                        self.note_provider_model_failure(endpoint, None, None, now_unix());
                        continue;
                    }
                    self.note_provider_model_success(endpoint);
                    return Ok(ConnectedWebSocket {
                        upstream: Some(upstream),
                        endpoint: endpoint.clone(),
                        attempt_count,
                        failover: attempt_count > 1,
                        handshake_ttfb_ms: attempt_started.elapsed().as_millis() as i64,
                    });
                }
                Ok(Err(error)) => {
                    let (status, message) = websocket_connect_error(&error);
                    let retry_after = websocket_connect_retry_after(&error);
                    if status == 0 || RetryPolicy::is_endpoint_retryable_status(status) {
                        self.note_provider_model_failure(
                            endpoint,
                            (status > 0).then_some(status),
                            retry_after,
                            now_unix(),
                        );
                    }
                    let prepared_error =
                        WebSocketPrepareError::new(status, "upstream_error", message).with_attempt(
                            endpoint,
                            attempt_count,
                            (status > 0).then(|| attempt_started.elapsed().as_millis() as i64),
                            retry_after,
                        );
                    if status > 0 && !RetryPolicy::is_endpoint_retryable_status(status) {
                        return Err(prepared_error);
                    }
                    last_error = Some(prepared_error);
                }
                Err(_) => {
                    self.note_provider_model_failure(endpoint, None, None, now_unix());
                    last_error = Some(
                        WebSocketPrepareError::new(
                            0,
                            "upstream_error",
                            "upstream websocket connect timed out",
                        )
                        .with_attempt(endpoint, attempt_count, None, None),
                    );
                }
            }
        }
        let error = last_error.unwrap_or_else(|| {
            WebSocketPrepareError::new(0, "upstream_error", "upstream websocket unavailable")
        });
        Err(error)
    }

    async fn handle_request_parts(
        &self,
        remote: Option<IpAddr>,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: Body,
    ) -> Response {
        let context = self.inbound_request_context(method, path_and_query, &headers);
        INBOUND_REQUEST_CONTEXT
            .scope(
                RefCell::new(context),
                self.handle_request_parts_scoped(remote, method, path_and_query, headers, body),
            )
            .await
    }

    fn inbound_request_context(
        &self,
        method: &str,
        path_and_query: &str,
        headers: &[(String, String)],
    ) -> InboundRequestContext {
        let path = bounded_request_path(path_without_query(path_and_query));
        let client_kind = detect_client_kind(headers, true);
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| decoded_query_value(query, "model"));
        let route_intent =
            route_intent_for_path(method, path_and_query, query_model.as_deref(), client_kind);
        InboundRequestContext {
            method: bounded_request_method(method),
            path,
            route_intent: route_intent.to_string(),
            session_id: observed_session_id(headers),
            grok_metadata: GrokMetadata::from_headers(headers),
        }
    }

    async fn handle_request_parts_scoped(
        &self,
        remote: Option<IpAddr>,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: Body,
    ) -> Response {
        let path = path_and_query
            .split_once('?')
            .map_or(path_and_query, |(path, _)| path);
        let query = path_and_query.split_once('?').map(|(_, query)| query);
        let remote_text = remote.map(|ip| ip.to_string());

        // CIDR 对所有路径最先执行;环回恒放行。
        let config = self.config();
        if !access::is_allowed(remote_text.as_deref(), &config.listener.allowed_cidrs) {
            self.record_rejected_client_with_metadata(
                403,
                "client_forbidden",
                None,
                Some(RequestPurpose::Standard),
                detect_client_kind(&headers, true),
                CodexMetadata::from_request(&headers, None),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::OpenAI),
            );
            return error_response(StatusCode::FORBIDDEN, &[("error", "client_forbidden")]);
        }

        // Platform actions are identified after the network boundary but
        // before any body read.  The platform implementation owns its token
        // check and may consume the body only after that check succeeds.
        if let Some(action) = self.inner.platform.platform_action(method, path) {
            let request = PlatformRequest {
                method: method.to_string(),
                path_and_query: path_and_query.to_string(),
                headers,
                remote_ip: remote,
                body,
            };
            return self
                .inner
                .platform
                .handle_platform_action(action, request, Arc::new(self.clone()))
                .await;
        }

        // `/__status` is the one control read shared by both binaries.  Its
        // policy (loopback on Linux, loopback-or-control-token on macOS) is
        // injected by the platform file; no platform branch appears in the
        // request router below.
        if path == "/__status"
            && self.inner.platform.authorize_status(&ControlRequestMeta {
                remote_ip: remote,
                query,
                headers: &headers,
            }) == AccessDecision::Deny
        {
            self.record_rejected_client_with_metadata(
                403,
                self.inner.platform.status_denied_error(),
                None,
                Some(RequestPurpose::Standard),
                detect_client_kind(&headers, true),
                CodexMetadata::from_request(&headers, None),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::OpenAI),
            );
            return error_response(
                StatusCode::FORBIDDEN,
                &[("error", self.inner.platform.status_denied_error())],
            );
        }

        if !path.starts_with("/__") && self.runtime_storage_backpressured() {
            self.record_rejected_client_with_metadata(
                503,
                "runtime_storage_backpressure",
                None,
                Some(RequestPurpose::Standard),
                detect_client_kind(&headers, true),
                CodexMetadata::from_request(&headers, None),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::OpenAI),
            );
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &[("error", "runtime_storage_backpressure")],
            );
        }

        // Validate CPA's call-resource grammar before the dynamic Realtime
        // dispatcher can mistake a malformed child for a standard Realtime
        // resource.  The path is the only input echoed into diagnostics;
        // query values and request bodies remain untouched.
        if let Err(error) = validate_realtime_call_target(path_and_query) {
            let (status, code, message) = realtime_call_path_error(error);
            self.record_rejected_client_with_metadata(
                status.as_u16().into(),
                message,
                None,
                Some(RequestPurpose::Standard),
                detect_client_kind(&headers, true),
                CodexMetadata::from_request(&headers, None),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::OpenAI),
            );
            return error_response(status, &[("error", code), ("message", message)]);
        }

        // `/v1/models` is a local capability directory. Codex Desktop probes
        // this path without a model field; forwarding it as a resource request
        // sent the catalog to whichever non-Anthropic provider happened to be
        // first. Serve configured mappings instead so the visible list does
        // not depend on provider order, and so Codex `client_version` gets
        // the `{models:[...]}` shape rather than a foreign OpenAI catalog.
        // Claim the complete models route tree here. Otherwise POST
        // `/v1/models` and malformed nested paths fall through to the generic
        // resource/raw dispatcher and can leak a request to a Provider.
        if is_openai_resource_tree(path, "models") {
            if config.listener.has_inbound_auth()
                && !inbound_auth_ok(&headers, &config.listener.auth_token)
            {
                let client_kind = detect_client_kind(&headers, true);
                self.record_rejected_client_with_metadata(
                    401,
                    message_tokens::INBOUND_AUTH_REQUIRED,
                    None,
                    Some(RequestPurpose::Standard),
                    client_kind,
                    CodexMetadata::from_request(&headers, None),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(ProviderProtocol::OpenAI),
                );
                return error_response(
                    StatusCode::UNAUTHORIZED,
                    &[("error", "inbound_auth_required")],
                );
            }
            if !is_local_models_path(path) {
                self.record_rejected_client_with_metadata(
                    404,
                    "model path not found",
                    None,
                    Some(RequestPurpose::Standard),
                    detect_client_kind(&headers, true),
                    CodexMetadata::from_request(&headers, None),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(ProviderProtocol::OpenAI),
                );
                return error_response(
                    StatusCode::NOT_FOUND,
                    &[("error", "not_found"), ("path", path)],
                );
            }
            if !method.eq_ignore_ascii_case("GET") {
                self.record_rejected_client_with_metadata(
                    405,
                    "method not allowed for models",
                    None,
                    Some(RequestPurpose::Standard),
                    detect_client_kind(&headers, true),
                    CodexMetadata::from_request(&headers, None),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(ProviderProtocol::OpenAI),
                );
                return method_not_allowed_response("GET");
            }
            return local_models_response(&config, path, query, &headers);
        }

        match path {
            "/v1/messages" => {
                self.handle_messages(&config, method, path_and_query, headers, body)
                    .await
            }
            "/v1/chat/completions" | "/chat/completions" => {
                self.handle_openai_inbound(
                    &config,
                    ClientDialect::Chat,
                    false,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/responses" | "/responses" | "/backend-api/codex/responses" => {
                self.handle_openai_inbound(
                    &config,
                    ClientDialect::Responses,
                    false,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/responses/compact"
            | "/responses/compact"
            | "/backend-api/codex/responses/compact" => {
                self.handle_openai_inbound(
                    &config,
                    ClientDialect::Responses,
                    true,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/completions" | "/completions" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::Completions,
                    RequestPurpose::Standard,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/messages/count_tokens" | "/messages/count_tokens" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::ClaudeCountTokens,
                    RequestPurpose::TokenCount,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/images/generations"
            | "/images/generations"
            | "/backend-api/codex/images/generations" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::ImagesGenerations,
                    RequestPurpose::ImageGeneration,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/images/edits" | "/images/edits" | "/backend-api/codex/images/edits" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::ImagesEdits,
                    RequestPurpose::ImageEdit,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/v1/alpha/search" | "/alpha/search" | "/backend-api/codex/alpha/search" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::AlphaSearch,
                    RequestPurpose::AlphaSearch,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            // OpenAI Realtime/Live HTTP bootstrap is a native data-plane
            // request. Keep the exact method, SDP/JSON body, and path so the
            // configured Provider remains the protocol authority.
            "/v1/realtime/client_secrets"
            | "/realtime/client_secrets"
            | "/openai/v1/realtime/client_secrets"
            | "/backend-api/codex/realtime/client_secrets"
            | "/v1/realtime"
            | "/realtime"
            | "/openai/v1/realtime"
            | "/backend-api/codex/realtime"
            | "/v1/realtime/calls"
            | "/realtime/calls"
            | "/openai/v1/realtime/calls"
            | "/backend-api/codex/realtime/calls"
            | "/v1/live"
            | "/live"
            | "/openai/v1/live"
            | "/backend-api/codex/live" => {
                self.handle_native_openai_passthrough(
                    &config,
                    PassthroughKind::Realtime,
                    RequestPurpose::Standard,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            "/__status" => self.handle_status(&config),
            "/__runtime" => error_response(StatusCode::NOT_FOUND, &[("error", "not_found")]),
            // 管理写操作仅存在于固定 loopback Admin listener，数据面不留旁路。
            "/__reset-stats" | "/__reload" => {
                error_response(StatusCode::NOT_FOUND, &[("error", "not_found")])
            }
            // CPA exposes dynamic Realtime/Live, Files, Videos and Models
            // paths.  Dispatch them by intent instead of maintaining a
            // brittle allow-list for every resource-id/control suffix.
            _ if !path.starts_with("/__") => {
                let kind = native_passthrough_kind(path);
                // Raw 是「转发任意上游路径」的兜底,只有请求自己带得出 model 时才
                // 成立 —— 没有 model 就选不出入口。不带 model 的未知路径就是未知
                // 路径:直接 404,既不做鉴权也不碰请求体,否则 /healthz、/favicon.ico
                // 这类探测会先吃到 401/400 并计进「被拒客户端」统计,把那块面板淹掉。
                //
                // Query model is the cheap path.  JSON raw requests may carry
                // the model only in their body, so let those reach the native
                // handler (which already enforces MAX_BODY_BYTES and validates
                // the field).  Opaque/non-JSON unknown paths still short-cut
                // to 404 without consuming arbitrary probe payloads.
                let query_model = query
                    .and_then(|query| decoded_query_value(query, "model"))
                    .filter(|model| !model.trim().is_empty());
                let body_hint = raw_request_body_hint(method, &headers);
                if kind == PassthroughKind::Raw && query_model.is_none() && !body_hint {
                    return error_response(
                        StatusCode::NOT_FOUND,
                        &[("error", "not_found"), ("path", path)],
                    );
                }
                self.handle_native_openai_passthrough(
                    &config,
                    kind,
                    RequestPurpose::Standard,
                    method,
                    path_and_query,
                    headers,
                    body,
                )
                .await
            }
            _ => error_response(
                StatusCode::NOT_FOUND,
                &[("error", "not_found"), ("path", path)],
            ),
        }
    }

    fn handle_status(&self, config: &AppConfig) -> Response {
        let state = self.inner.state.lock().unwrap();
        // 【Rust 修正】authToken 脱敏,不回明文。
        let body = json!({
            "running": true,
            "runtimeApiVersion": 1,
            "listener": {
                "host": config.listener.host,
                "port": config.listener.port,
                "allowedCIDRs": config.listener.allowed_cidrs,
                "authToken": if config.listener.auth_token.is_empty() { "" } else { "***" },
            },
            "providers": config.endpoints.len(),
            "endpoints": config.endpoints.len(),
            "runtime": state.runtime,
            "lastError": state.last_error,
            "generation": *self.inner.generation.read().unwrap(),
        });
        json_response(StatusCode::OK, &body)
    }

    // -----------------------------------------------------------------------
    // /v1/messages
    // -----------------------------------------------------------------------

    async fn handle_messages(
        &self,
        config: &AppConfig,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: Body,
    ) -> Response {
        let mut client_kind = detect_client_kind(&headers, false);
        let header_codex_metadata = CodexMetadata::from_request(&headers, None);
        // 入站 auth(仅本端点):x-api-key 精确或 Bearer;空 token 不校验。
        if config.listener.has_inbound_auth()
            && !inbound_auth_ok(&headers, &config.listener.auth_token)
        {
            self.record_rejected_client_with_metadata(
                401,
                message_tokens::INBOUND_AUTH_REQUIRED,
                None,
                None,
                client_kind,
                header_codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::Anthropic),
            );
            return error_response(
                StatusCode::UNAUTHORIZED,
                &[("error", "inbound_auth_required")],
            );
        }
        let body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                let message = error.to_string();
                self.record_rejected_client_with_metadata(
                    400,
                    &message,
                    None,
                    None,
                    client_kind,
                    header_codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(ProviderProtocol::Anthropic),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "bad_request"), ("message", &message)],
                );
            }
        };

        let Ok(parsed) = serde_json::from_slice::<Value>(&body) else {
            self.record_rejected_client_with_metadata(
                400,
                message_tokens::BODY_NOT_JSON,
                None,
                None,
                client_kind,
                header_codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::Anthropic),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[
                    ("error", "invalid_request"),
                    ("message", "body is not JSON"),
                ],
            );
        };
        let codex_metadata = CodexMetadata::from_request(&headers, Some(&parsed));
        if let Some(originator) = codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.originator.as_deref())
        {
            client_kind = ClientKind::detect_with_originator(
                header_value(&headers, "user-agent"),
                Some(originator),
                false,
            );
        }
        if !valid_anthropic_messages_request(&parsed) {
            self.record_rejected_client_with_metadata(
                400,
                message_tokens::ANTHROPIC_REQUEST_INVALID,
                None,
                None,
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(ProviderProtocol::Anthropic),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[
                    ("error", "invalid_request"),
                    (
                        "message",
                        "model must be a non-empty string and messages must be an array",
                    ),
                ],
            );
        }
        let request = RoutingRequest::from_value(&parsed)
            .expect("validated Anthropic Messages request is a JSON object");

        self.handle_planned(
            config,
            request,
            ProviderProtocol::Anthropic,
            headers,
            body,
            method,
            path_and_query,
            None,
            None,
            client_kind,
            codex_metadata,
        )
        .await
    }

    // -----------------------------------------------------------------------
    // 入站 OpenAI 兼容层(/v1/chat/completions、/v1/responses):Codex 等
    // OpenAI 系客户端接入后先转换为 Anthropic Messages，再复用同一
    // 路由、粘性、failover 与可取消重试管线；响应在 relay 中转回客户端方言。
    // -----------------------------------------------------------------------

    async fn handle_openai_inbound(
        &self,
        config: &AppConfig,
        dialect: ClientDialect,
        compact: bool,
        _path_and_query: &str,
        headers: Vec<(String, String)>,
        body: Body,
    ) -> Response {
        let mut client_kind = detect_client_kind(&headers, true);
        let source_format = match dialect {
            ClientDialect::Chat => ProviderProtocol::OpenAI,
            ClientDialect::Responses => ProviderProtocol::OpenAIResponses,
        };
        let request_purpose = compact.then_some(RequestPurpose::Compact);
        let header_codex_metadata = CodexMetadata::from_request(&headers, None);
        if config.listener.has_inbound_auth()
            && !inbound_auth_ok(&headers, &config.listener.auth_token)
        {
            self.record_rejected_client_with_metadata(
                401,
                message_tokens::INBOUND_AUTH_REQUIRED,
                None,
                request_purpose,
                client_kind,
                header_codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::UNAUTHORIZED,
                &[("error", "inbound_auth_required")],
            );
        }
        let body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                let message = error.to_string();
                self.record_rejected_client_with_metadata(
                    400,
                    &message,
                    None,
                    request_purpose,
                    client_kind,
                    header_codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "bad_request"), ("message", &message)],
                );
            }
        };
        let Ok(parsed) = serde_json::from_slice::<Value>(&body) else {
            self.record_rejected_client_with_metadata(
                400,
                message_tokens::BODY_NOT_JSON,
                None,
                request_purpose,
                client_kind,
                header_codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[
                    ("error", "invalid_request"),
                    ("message", "body is not JSON"),
                ],
            );
        };
        let codex_metadata = CodexMetadata::from_request(&headers, Some(&parsed));
        if let Some(originator) = codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.originator.as_deref())
        {
            client_kind = ClientKind::detect_with_originator(
                header_value(&headers, "user-agent"),
                Some(originator),
                true,
            );
        }
        let client_stream = !compact && bridge_in::client_wants_stream(&parsed);
        let converted = if compact {
            parsed
                .get("model")
                .and_then(Value::as_str)
                .filter(|model| !model.trim().is_empty())
                .map(|model| json!({"model": model}))
                .ok_or_else(|| "missing model".to_string())
        } else {
            match dialect {
                ClientDialect::Chat => bridge_in::chat_to_anthropic(&parsed),
                ClientDialect::Responses => bridge_in::responses_to_anthropic(&parsed),
            }
        };
        let converted = match converted {
            Ok(value) => value,
            Err(reason) => {
                self.record_rejected_client_with_metadata(
                    400,
                    &format!("{}{reason}", message_tokens::INBOUND_CONVERT_FAILED_PREFIX),
                    None,
                    request_purpose,
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "invalid_request"), ("message", &reason)],
                );
            }
        };
        let Some(request) = RoutingRequest::from_value(&converted) else {
            self.record_rejected_client_with_metadata(
                400,
                message_tokens::BODY_NOT_OBJECT,
                None,
                request_purpose,
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[
                    ("error", "invalid_request"),
                    ("message", "body is not an object"),
                ],
            );
        };
        self.handle_planned(
            config,
            request,
            source_format,
            headers,
            body.clone(),
            "POST",
            "/v1/messages",
            Some(ClientOut {
                dialect: Some(dialect),
                passthrough_kind: match (dialect, compact) {
                    (ClientDialect::Chat, _) => PassthroughKind::Chat,
                    (ClientDialect::Responses, true) => PassthroughKind::ResponsesCompact,
                    (ClientDialect::Responses, false) => PassthroughKind::Responses,
                },
                stream: client_stream,
                passthrough: Some(body),
                content_type: Some("application/json".into()),
                terminal_dialect: match dialect {
                    ClientDialect::Chat => SseDialect::OpenAiChat,
                    ClientDialect::Responses => SseDialect::OpenAiResponses,
                },
                realtime_client_secret: false,
            }),
            request_purpose,
            client_kind,
            codex_metadata,
        )
        .await
    }

    /// Images、Legacy Completions、Claude Count Tokens 与 Codex Alpha Search
    /// 没有可安全复用的跨方言桥。它们始终保留原生请求/响应，只借用公共的模型池、
    /// 粘性、鉴权、首响应前 failover 与事件管线。multipart 图片编辑只读取短文本
    /// 路由字段，上传内容从不重建。
    #[allow(clippy::too_many_arguments)]
    async fn handle_native_openai_passthrough(
        &self,
        config: &AppConfig,
        kind: PassthroughKind,
        purpose: RequestPurpose,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: Body,
    ) -> Response {
        let mut client_kind =
            detect_client_kind(&headers, kind != PassthroughKind::ClaudeCountTokens);
        let source_format = source_format_for_passthrough(kind);
        let header_codex_metadata = CodexMetadata::from_request(&headers, None);
        let realtime_ephemeral_auth = kind == PassthroughKind::Realtime
            && !is_realtime_client_secret_path(path_and_query)
            && self.realtime_client_secret_authorized(&headers);
        if config.listener.has_inbound_auth()
            && !inbound_auth_ok(&headers, &config.listener.auth_token)
            && !realtime_ephemeral_auth
        {
            self.record_rejected_client_with_metadata(
                401,
                message_tokens::INBOUND_AUTH_REQUIRED,
                None,
                Some(purpose),
                client_kind,
                header_codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::UNAUTHORIZED,
                &[("error", "inbound_auth_required")],
            );
        }
        let mut body = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
            Ok(body) => body,
            Err(error) => {
                let message = error.to_string();
                self.record_rejected_client_with_metadata(
                    400,
                    &message,
                    None,
                    Some(purpose),
                    client_kind,
                    header_codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "bad_request"), ("message", &message)],
                );
            }
        };
        let mut content_type = header_value(&headers, "content-type").map(str::to_string);
        let resource_intent = is_resource_passthrough_kind(kind);
        // Known JSON endpoints historically accepted an omitted media type;
        // preserve that compatibility while keeping Raw and empty resource
        // requests genuinely opaque (not guessed as JSON).
        if content_type.is_none()
            && !body.is_empty()
            && !matches!(kind, PassthroughKind::Raw)
            && !resource_intent
        {
            content_type = Some("application/json".into());
        }
        // Resolve the client identity from the *original* payload before any
        // Live/Realtime compatibility envelope is applied.  Codex Desktop
        // sometimes puts `originator` only in
        // `client_metadata.x-codex-turn-metadata`; waiting until after the
        // SDP/body normalization made the request look like a generic
        // openai_compat client during route classification and rejection
        // accounting.  Keep this metadata as the authoritative attribution
        // source for the whole request; normalization must never erase it.
        // Metadata is observational and must not make an opaque request less
        // opaque.  For JSON requests we parse a bounded body; when a client
        // omitted Content-Type we only sniff a JSON-looking object prefix so
        // Codex Desktop's body-only `originator` still contributes to
        // attribution.  The original bytes are retained for the relay.
        let metadata_body_before_normalization =
            metadata_json_body_hint(&body, content_type.as_deref());
        let codex_metadata =
            CodexMetadata::from_request(&headers, metadata_body_before_normalization.as_ref());
        if let Some(originator) = codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.originator.as_deref())
        {
            client_kind = ClientKind::detect_with_originator(
                header_value(&headers, "user-agent"),
                Some(originator),
                kind != PassthroughKind::ClaudeCountTokens,
            );
        }
        // An ephemeral client secret is scoped to the model requested when it
        // was issued.  Check an explicit query/body model before the Live
        // normalizer can replace it with `gpt-live-1-codex`; otherwise a
        // conflicting model could be hidden by the protocol bootstrap rewrite.
        if kind == PassthroughKind::Realtime
            && let Some(expected_model) = self.realtime_client_secret_model(&headers)
        {
            let explicit_model = path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"))
                .or_else(|| realtime_body_model_hint(&body, content_type.as_deref()));
            if let Some(explicit_model) = explicit_model
                && !realtime_client_secret_models_match(&expected_model, &explicit_model)
            {
                let message = "Realtime client secret is not valid for the requested model";
                self.record_rejected_client_with_metadata(
                    403,
                    message,
                    Some(explicit_model),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::FORBIDDEN,
                    &[
                        ("error", "realtime_client_secret_scope_mismatch"),
                        ("message", message),
                    ],
                );
            }
        }
        // Live/Realtime intent affects provider selection only.  CPA already
        // owns the Codex WebRTC/Quicksilver protocol adapter, so converting a
        // multipart or SDP bootstrap here would make the second proxy see a
        // different request than a direct Codex -> CPA connection.  Keep the
        // original path, media type and body; only the routing model below is
        // forced away from a leaked surrounding chat model.
        if kind == PassthroughKind::Realtime
            && is_realtime_call_bootstrap_path(path_without_query(path_and_query))
            && let Some(session) = self.realtime_client_secret_session(&headers)
        {
            let (normalized_body, normalized_content_type) =
                match apply_realtime_client_secret_session(
                    &body,
                    content_type.as_deref().unwrap_or(""),
                    &session,
                ) {
                    Ok(value) => value,
                    Err(reason) => {
                        self.record_rejected_client_with_metadata(
                            400,
                            reason,
                            None,
                            Some(purpose),
                            client_kind,
                            codex_metadata.clone(),
                            ClientDeclaredMetadata::from_headers(&headers),
                            Some(source_format),
                        );
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &[("error", "invalid_request"), ("message", reason)],
                        );
                    }
                };
            body = normalized_body;
            content_type = Some(normalized_content_type);
        }
        let fields = if resource_intent {
            NativePassthroughFields {
                model: sumpter_core::routing::RESOURCE_ROUTING_MODEL.into(),
                stream: false,
            }
        } else if kind == PassthroughKind::Raw {
            // Raw requests are relayed byte-for-byte.  We only perform a
            // bounded, read-only JSON sniff to discover a top-level `model`
            // used for selecting the mapping; the original bytes are passed
            // unchanged to `request_build`.
            let body_model = raw_body_model_hint(&body, content_type.as_deref());
            let model = path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"))
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
                .or(body_model);
            let Some(model) = model else {
                let reason = "model is required for raw passthrough";
                self.record_rejected_client_with_metadata(
                    400,
                    reason,
                    None,
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "invalid_request"), ("message", reason)],
                );
            };
            NativePassthroughFields {
                model,
                stream: false,
            }
        } else if kind == PassthroughKind::Realtime {
            // CPA Live/Realtime bootstrap always uses Codex OAuth and maps
            // empty/`gpt-realtime*` names to `gpt-live-1-codex`. Sumpter
            // selects providers by mapping, so voice bootstrap must not inherit
            // the surrounding chat model (the original fable-5 failure).
            let query_model = path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"))
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty());
            let body_model = realtime_body_model(&body, content_type.as_deref());
            let sideband_model = self.live_session_model(path_and_query).flatten();
            let secret_model = self.realtime_client_secret_model(&headers);
            let intent = classify_realtime_intent(
                method,
                path_and_query,
                query_model.as_deref().or(body_model.as_deref()),
                sideband_model.as_deref(),
                secret_model.as_deref(),
                client_kind,
                body_indicates_codex_live(&body, content_type.as_deref()),
            );
            set_current_route_intent(match intent {
                RealtimeRouteIntent::CodexLive => "live",
                RealtimeRouteIntent::StandardRealtime => "realtime",
            });
            let model = resolve_realtime_route_model(
                config,
                method,
                path_and_query,
                query_model.or_else(|| realtime_body_model(&body, content_type.as_deref())),
                sideband_model,
                secret_model,
                intent,
            );
            NativePassthroughFields {
                model,
                stream: false,
            }
        } else if kind == PassthroughKind::Videos {
            let path = path_without_query(path_and_query);
            let multipart_model = if content_type
                .as_deref()
                .is_some_and(content_type_is_multipart)
            {
                match native_multipart_fields_with_default(
                    &body,
                    content_type.as_deref().unwrap_or(""),
                    "",
                ) {
                    Ok(fields) => Some(fields.model).filter(|model| !model.trim().is_empty()),
                    Err(reason) => {
                        self.record_rejected_client_with_metadata(
                            400,
                            reason,
                            None,
                            Some(purpose),
                            client_kind,
                            codex_metadata.clone(),
                            ClientDeclaredMetadata::from_headers(&headers),
                            Some(source_format),
                        );
                        return error_response(
                            StatusCode::BAD_REQUEST,
                            &[("error", "invalid_request"), ("message", reason)],
                        );
                    }
                }
            } else {
                None
            };
            let requested = path_and_query
                .split_once('?')
                .and_then(|(_, query)| decoded_query_value(query, "model"))
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
                .or_else(|| realtime_body_model(&body, content_type.as_deref()))
                .or(multipart_model);
            let model = if is_videos_lookup_path(path) {
                self.video_session_model(path)
                    .flatten()
                    .filter(|model| !model.trim().is_empty())
                    .or(requested)
                    .or_else(|| {
                        RoutePlanner::default_model_for_capability(
                            config,
                            sumpter_core::capability::ModelCapability::Video,
                        )
                    })
            } else {
                requested.or_else(|| {
                    RoutePlanner::default_model_for_capability(
                        config,
                        sumpter_core::capability::ModelCapability::Video,
                    )
                })
            };
            let Some(model) = model.filter(|model| !model.trim().is_empty()) else {
                let message = "no video-capable Provider";
                self.record_rejected_client_with_metadata(
                    503,
                    message,
                    None,
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::SERVICE_UNAVAILABLE,
                    &[("error", "no_video_provider"), ("message", message)],
                );
            };
            NativePassthroughFields {
                model,
                stream: false,
            }
        } else if content_type.as_deref().is_some_and(content_type_is_json) {
            match native_json_fields(&body, kind) {
                Ok(fields) => fields,
                Err(reason) => {
                    self.record_rejected_client_with_metadata(
                        400,
                        message_tokens::BODY_NOT_JSON,
                        None,
                        Some(purpose),
                        client_kind,
                        codex_metadata.clone(),
                        ClientDeclaredMetadata::from_headers(&headers),
                        Some(source_format),
                    );
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &[("error", "invalid_request"), ("message", reason)],
                    );
                }
            }
        } else if kind == PassthroughKind::ImagesEdits
            && content_type
                .as_deref()
                .is_some_and(content_type_is_multipart)
        {
            match native_multipart_fields(&body, content_type.as_deref().unwrap_or("")) {
                Ok(fields) => fields,
                Err(reason) => {
                    self.record_rejected_client_with_metadata(
                        400,
                        reason,
                        None,
                        Some(purpose),
                        client_kind,
                        codex_metadata.clone(),
                        ClientDeclaredMetadata::from_headers(&headers),
                        Some(source_format),
                    );
                    return error_response(
                        StatusCode::BAD_REQUEST,
                        &[("error", "invalid_request"), ("message", reason)],
                    );
                }
            }
        } else {
            let reason = "unsupported Content-Type for native OpenAI endpoint";
            self.record_rejected_client_with_metadata(
                400,
                reason,
                None,
                Some(purpose),
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[("error", "invalid_request"), ("message", reason)],
            );
        };

        let mut fields = fields;
        if matches!(
            kind,
            PassthroughKind::ImagesGenerations | PassthroughKind::ImagesEdits
        ) && !config.matches_model(&fields.model)
            && let Some(fallback) = RoutePlanner::default_model_for_capability(
                config,
                sumpter_core::capability::ModelCapability::Image,
            )
        {
            fields.model = fallback;
        }
        let routing_value = json!({
            "model": fields.model,
            "stream": fields.stream,
        });
        let request = RoutingRequest::from_value(&routing_value)
            .expect("native routing metadata is always a JSON object");
        self.handle_planned(
            config,
            request,
            source_format,
            headers,
            body.clone(),
            method,
            path_and_query,
            Some(ClientOut {
                dialect: None,
                passthrough_kind: kind,
                stream: fields.stream,
                passthrough: Some(body),
                content_type,
                terminal_dialect: match kind {
                    PassthroughKind::ImagesGenerations | PassthroughKind::ImagesEdits => {
                        SseDialect::OpenAiImages
                    }
                    PassthroughKind::Completions | PassthroughKind::Chat => SseDialect::OpenAiChat,
                    PassthroughKind::ClaudeCountTokens => SseDialect::Anthropic,
                    PassthroughKind::Responses
                    | PassthroughKind::ResponsesCompact
                    | PassthroughKind::AlphaSearch
                    | PassthroughKind::Raw
                    | PassthroughKind::Files
                    | PassthroughKind::Videos
                    | PassthroughKind::Realtime
                    | PassthroughKind::Models => SseDialect::OpenAiResponses,
                },
                realtime_client_secret: is_realtime_client_secret_path(path_and_query),
            }),
            Some(purpose),
            client_kind,
            codex_metadata,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn handle_planned(
        &self,
        config: &AppConfig,
        request: RoutingRequest,
        source_format: ProviderProtocol,
        headers: Vec<(String, String)>,
        inbound_body: Bytes,
        method: &str,
        path_and_query: &str,
        client_out: Option<ClientOut>,
        purpose_override: Option<RequestPurpose>,
        client_kind: ClientKind,
        codex_metadata: Option<CodexMetadata>,
    ) -> Response {
        let purpose = purpose_override.unwrap_or_else(|| inspector::request_purpose(&request));
        // 该形状告警只用于 Claude Code 的 Anthropic 内部辅助请求。
        // Codex/Responses 转成 Anthropic 后同样可能是「system + 单条 user + 无 tools」,
        // 但这是正常的 OpenAI 请求形状，不能误报为 Claude 指纹失配。
        let unmatched_no_tools =
            client_kind == ClientKind::ClaudeCode && inspector::is_unmatched_no_tools(&request);
        let media_passthrough = client_out.as_ref().is_some_and(|client| {
            matches!(
                client.passthrough_kind,
                PassthroughKind::ImagesGenerations
                    | PassthroughKind::ImagesEdits
                    | PassthroughKind::Videos
                    | PassthroughKind::Files
                    | PassthroughKind::Realtime
                    | PassthroughKind::Models
                    | PassthroughKind::Raw
            )
        });
        if !media_passthrough && is_media_only_conversation_model(&request.model) {
            let message = "image and video models are only supported on /v1/images and /v1/videos";
            self.record_rejected_client_with_metadata(
                400,
                message,
                Some(request.model.clone()),
                Some(purpose),
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[("error", "invalid_request"), ("message", message)],
            );
        }
        // dialect 入站(OpenAI chat/Responses)有两种归宿:落到同协议上游可以按字节
        // 透传,落到 Anthropic 上游必须走翻译面。所以不能一看到 passthrough body 就
        // 认定透传 —— `plan_for_passthrough` 不做协议 gate,会把 route_mode 恒定压成
        // Native,翻译面永远走不到,客户端拿到的是上游原始 SSE 而不是自己协议的响应。
        //
        // 先按协议 gate 规划一次探真实 route_mode,只有首选入口确实是 Native 才透传。
        // 多出来的是一次纯计算(无 IO),真正的规划在下面按同样的输入再做一次。
        let mut client_out = client_out;
        let passthrough_intent = client_out.as_ref().is_some_and(|client| {
            client.passthrough.is_some()
                && (client.dialect.is_none()
                    || RoutePlanner::plan_for_source(&request, config, source_format)
                        .ok()
                        .and_then(|plan| {
                            plan.endpoints.first().map(|endpoint| endpoint.route_mode)
                        })
                        == Some(RouteMode::Native))
        });
        // 判定为翻译面时把 passthrough body 一并清掉:下游还有多处直接看
        // `client.passthrough`(最关键的是 `bridging`,它决定响应 content-type 与
        // 是否启用客户端桥),留着会让那些地方继续以为这是按字节透传。
        if !passthrough_intent && let Some(client) = &mut client_out {
            client.passthrough = None;
        }
        if matches!(self.live_session_endpoint(path_and_query), Some(None)) {
            let is_live = is_codex_live_sideband_target(path_and_query);
            let message = "Realtime session is unknown or expired";
            let status = if is_live {
                StatusCode::GONE
            } else {
                StatusCode::NOT_FOUND
            };
            let error = if is_live {
                "live_session_expired"
            } else {
                "realtime_call_not_found"
            };
            self.record_rejected_client_with_metadata(
                status.as_u16().into(),
                message,
                Some(request.model.clone()),
                Some(purpose),
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(status, &[("error", error), ("message", message)]);
        }
        let passthrough_kind = client_out.as_ref().map(|client| client.passthrough_kind);
        let mut plan = match if passthrough_kind == Some(PassthroughKind::Files) {
            RoutePlanner::plan_for_resource_capability(
                config,
                source_format,
                sumpter_core::capability::ModelCapability::Files,
            )
        } else if passthrough_kind == Some(PassthroughKind::Models) {
            RoutePlanner::plan_for_resource(config, source_format)
        } else if matches!(
            passthrough_kind,
            Some(PassthroughKind::ImagesGenerations | PassthroughKind::ImagesEdits)
        ) {
            RoutePlanner::plan_for_capability(
                &request,
                config,
                source_format,
                sumpter_core::capability::ModelCapability::Image,
            )
        } else if passthrough_kind == Some(PassthroughKind::Videos) {
            RoutePlanner::plan_for_capability(
                &request,
                config,
                source_format,
                sumpter_core::capability::ModelCapability::Video,
            )
        } else if passthrough_kind == Some(PassthroughKind::Realtime) {
            // Both public Realtime and Codex Live use the voice capability;
            // the classifier below decides the protocol variant, while the
            // planner must select a capability-qualified mapping first.
            RoutePlanner::plan_for_capability(
                &request,
                config,
                source_format,
                sumpter_core::capability::ModelCapability::Live,
            )
        } else if passthrough_intent {
            RoutePlanner::plan_for_passthrough(&request, config, source_format)
        } else {
            RoutePlanner::plan_for_source(&request, config, source_format)
        } {
            Ok(plan) => plan,
            Err(e) => {
                let capability_error = match &e {
                    RoutePlanError::NoProviderForCapability { capability } => {
                        Some(capability.as_str())
                    }
                    _ => None,
                };
                let is_live = client_out.as_ref().is_some_and(|client| {
                    client.passthrough_kind == PassthroughKind::Realtime
                        && classify_realtime_intent(
                            method,
                            path_and_query,
                            Some(&request.model),
                            None,
                            None,
                            client_kind,
                            false,
                        ) == RealtimeRouteIntent::CodexLive
                });
                let (status, error) = if is_live {
                    (StatusCode::SERVICE_UNAVAILABLE, "no_live_provider")
                } else if capability_error == Some("video") {
                    (StatusCode::SERVICE_UNAVAILABLE, "no_video_provider")
                } else if capability_error == Some("image") {
                    (StatusCode::BAD_REQUEST, "no_image_provider")
                } else if capability_error == Some("files") {
                    (StatusCode::SERVICE_UNAVAILABLE, "no_files_provider")
                } else {
                    (StatusCode::BAD_REQUEST, "route_planning")
                };
                self.record_rejected_client_with_metadata(
                    status.as_u16().into(),
                    &e.to_string(),
                    Some(request.model.clone()),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(status, &[("error", error), ("message", &e.to_string())]);
            }
        };
        let realtime_intent = client_out.as_ref().is_some_and(|client| {
            client.passthrough.is_some() && client.passthrough_kind == PassthroughKind::Realtime
        });
        if realtime_intent {
            // A text-only Anthropic endpoint must never receive an OpenAI
            // Realtime/Quicksilver request merely because it has a broad or
            // stale model mapping (the original fable-5 failure mode).
            let codex_live = classify_realtime_intent(
                method,
                path_and_query,
                Some(&request.model),
                self.live_session_model(path_and_query).flatten().as_deref(),
                self.realtime_client_secret_model(&headers).as_deref(),
                client_kind,
                false,
            ) == RealtimeRouteIntent::CodexLive;
            plan.endpoints.retain(|endpoint| {
                endpoint.protocol != ProviderProtocol::Anthropic
                    && has_exact_realtime_mapping(config, &endpoint.endpoint_id, &request.model)
                    && (!codex_live || has_exact_codex_live_mapping(config, &endpoint.endpoint_id))
            });
            if plan.endpoints.is_empty() {
                let is_live = codex_live;
                let message = "no Realtime-compatible Provider";
                self.record_rejected_client_with_metadata(
                    if is_live { 503 } else { 400 },
                    message,
                    Some(request.model.clone()),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    if is_live {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::BAD_REQUEST
                    },
                    &[
                        (
                            "error",
                            if is_live {
                                "no_live_provider"
                            } else {
                                "no_compatible_protocol"
                            },
                        ),
                        ("message", message),
                    ],
                );
            }
        }
        if let Some(session) = self.live_session_endpoint(path_and_query) {
            let is_live = is_codex_live_sideband_target(path_and_query);
            match session {
                Some(endpoint_id) => {
                    plan.endpoints
                        .retain(|endpoint| endpoint.endpoint_id == endpoint_id);
                    if plan.endpoints.is_empty() {
                        let message =
                            "Realtime session is no longer available on its originating endpoint";
                        self.record_rejected_client_with_metadata(
                            410,
                            message,
                            Some(request.model.clone()),
                            Some(purpose),
                            client_kind,
                            codex_metadata.clone(),
                            ClientDeclaredMetadata::from_headers(&headers),
                            Some(source_format),
                        );
                        return error_response(
                            StatusCode::GONE,
                            &[
                                (
                                    "error",
                                    if is_live {
                                        "live_session_expired"
                                    } else {
                                        "realtime_call_not_found"
                                    },
                                ),
                                ("message", message),
                            ],
                        );
                    }
                }
                None => {
                    let message = "Realtime session is unknown or expired";
                    let status = if is_live {
                        StatusCode::GONE
                    } else {
                        StatusCode::NOT_FOUND
                    };
                    let error = if is_live {
                        "live_session_expired"
                    } else {
                        "realtime_call_not_found"
                    };
                    self.record_rejected_client_with_metadata(
                        status.as_u16().into(),
                        message,
                        Some(request.model.clone()),
                        Some(purpose),
                        client_kind,
                        codex_metadata.clone(),
                        ClientDeclaredMetadata::from_headers(&headers),
                        Some(source_format),
                    );
                    return error_response(status, &[("error", error), ("message", message)]);
                }
            }
        }
        if passthrough_kind == Some(PassthroughKind::Videos) {
            let video_path = path_without_query(path_and_query);
            if is_videos_lookup_path(video_path) {
                match self.video_session_endpoint(video_path) {
                    Some(Some(endpoint_id)) => {
                        plan.endpoints
                            .retain(|endpoint| endpoint.endpoint_id == endpoint_id);
                        if plan.endpoints.is_empty() {
                            let message =
                                "Video session is no longer available on its originating endpoint";
                            self.record_rejected_client_with_metadata(
                                410,
                                message,
                                Some(request.model.clone()),
                                Some(purpose),
                                client_kind,
                                codex_metadata.clone(),
                                ClientDeclaredMetadata::from_headers(&headers),
                                Some(source_format),
                            );
                            return error_response(
                                StatusCode::GONE,
                                &[("error", "video_session_expired"), ("message", message)],
                            );
                        }
                    }
                    Some(None) => {
                        let message = "Video session is unknown or expired";
                        self.record_rejected_client_with_metadata(
                            404,
                            message,
                            Some(request.model.clone()),
                            Some(purpose),
                            client_kind,
                            codex_metadata.clone(),
                            ClientDeclaredMetadata::from_headers(&headers),
                            Some(source_format),
                        );
                        return error_response(
                            StatusCode::NOT_FOUND,
                            &[("error", "video_session_not_found"), ("message", message)],
                        );
                    }
                    None => {}
                }
            }
        }
        if passthrough_intent
            && is_realtime_http_path(path_without_query(path_and_query))
            && let Some(expected_model) = self.realtime_client_secret_model(&headers)
            && !realtime_client_secret_models_match(&expected_model, &request.model)
        {
            let message = "Realtime client secret is not valid for the requested model";
            self.record_rejected_client_with_metadata(
                403,
                message,
                Some(request.model.clone()),
                Some(purpose),
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::FORBIDDEN,
                &[
                    ("error", "realtime_client_secret_scope_mismatch"),
                    ("message", message),
                ],
            );
        }
        if passthrough_intent
            && is_realtime_http_path(path_without_query(path_and_query))
            && let Some(endpoint_id) = self.realtime_client_secret_endpoint(&headers)
        {
            plan.endpoints
                .retain(|endpoint| endpoint.endpoint_id == endpoint_id);
            if plan.endpoints.is_empty() {
                let message = "Realtime client secret is not valid for the requested endpoint";
                self.record_rejected_client_with_metadata(
                    403,
                    message,
                    Some(request.model.clone()),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::FORBIDDEN,
                    &[
                        ("error", "realtime_client_secret_scope_mismatch"),
                        ("message", message),
                    ],
                );
            }
        }

        if let Some(required) = client_out
            .as_ref()
            .and_then(|client| required_native_protocol(client.passthrough_kind))
        {
            plan.endpoints.retain(|endpoint| {
                endpoint.route_mode == RouteMode::Native && endpoint.protocol == required
            });
            if plan.endpoints.is_empty() {
                let message = format!("no compatible Provider for {}", required.token());
                self.record_rejected_client_with_metadata(
                    400,
                    &message,
                    Some(request.model.clone()),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "no_compatible_protocol"), ("message", &message)],
                );
            }
        }

        // Raw data-plane requests bypass the legacy bridge capability checker:
        // the selected Provider owns the protocol contract and receives the
        // original request. The branch remains for internal non-raw callers.
        if client_out
            .as_ref()
            .is_none_or(|client| client.dialect.is_some())
        {
            plan.endpoints.retain(|endpoint| {
                endpoint.route_mode == RouteMode::Native
                    || translation_supported(
                        source_format,
                        endpoint,
                        &request,
                        &inbound_body,
                        purpose,
                    )
                    .is_ok()
            });
            if plan.endpoints.is_empty() {
                let message = format!("no compatible Provider for {}", source_format.token());
                self.record_rejected_client_with_metadata(
                    400,
                    &message,
                    Some(request.model.clone()),
                    Some(purpose),
                    client_kind,
                    codex_metadata.clone(),
                    ClientDeclaredMetadata::from_headers(&headers),
                    Some(source_format),
                );
                return error_response(
                    StatusCode::BAD_REQUEST,
                    &[("error", "no_compatible_protocol"), ("message", &message)],
                );
            }
        }

        // 「首选是 Translated 就清 client.passthrough」的补救原本在这里,已前移到
        // passthrough_intent:排在 plan 之后时,route_mode 已经被 plan_for_passthrough
        // 压成 Native,条件永不成立。

        let claude_session_id = header_value(&headers, "x-claude-code-session-id")
            // Codex 等 OpenAI 客户端通常使用 session_id,纳入同一稳定粘性键。
            .or_else(|| header_value(&headers, "session_id"));
        let observed_session_id = observed_session_id(&headers);
        let session_identity = sticky::resolve_session_identity(&request, claude_session_id);
        let sticky_key = sticky::StickyKey::new(
            session_identity,
            plan.effective_model.clone(),
            plan.feature_rule_id.clone(),
        );
        let session_key = sticky_key.session_key();
        // 出站桥无法无损表达的请求不能悄悄降级成残缺翻译:把承接不了的入口从候选里
        // 剔除,failover 仍有机会落到能原生承接的入口;全都承接不了才回 400,并带上
        // 具体字段名 —— 否则这类失败在客户端侧完全不可诊断(只表现为模型不听话)。
        // native 入口按字节转发,不过出站桥,所以不参与这道校验。这里刻意从完整
        // `plan.endpoints` 开始，而不是先套 endpoint/model 冷却；forward 会在首轮和
        // 每个后续轮次重新应用冷却，避免一个尚在冷却的入口从候选集中永久消失。
        let mut translation_error: Option<bridge::TranslationError> = None;
        let candidates: Vec<PlannedEndpoint> = plan
            .endpoints
            .clone()
            .into_iter()
            .filter(|endpoint| {
                if endpoint.route_mode != RouteMode::Translated {
                    return true;
                }
                let server_retrieval =
                    request_build::server_retrieval_enabled(endpoint, &request, purpose);
                match bridge::check_anthropic_translation(
                    &request,
                    endpoint.protocol,
                    server_retrieval,
                ) {
                    Ok(()) => true,
                    Err(error) => {
                        translation_error.get_or_insert(error);
                        false
                    }
                }
            })
            .collect();
        if candidates.is_empty() {
            let message = translation_error
                .map(|error| error.to_string())
                .unwrap_or_else(|| "no endpoint can serve this request".into());
            self.record_rejected_client_with_metadata(
                400,
                &message,
                Some(request.model.clone()),
                Some(purpose),
                client_kind,
                codex_metadata.clone(),
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[("error", "unsupported_translation"), ("message", &message)],
            );
        }
        // Apply the persistent provider/model cooldown at request admission.
        // Once admitted, this request's explicit retry/failover policy must be
        // allowed to run to completion; otherwise its first retryable response
        // would quarantine every candidate and short-circuit recovery.
        let initial_ordered = self.ordered_endpoint_candidates(&candidates, &session_key, true);
        // 路由计划定型且粘性排序已完成：此时首选入口已经确定，虽然尚未真正
        // 发起网络尝试。先写入候选协议，accepted/failover 后仍由 guard 用实际
        // 胜出入口覆盖，避免长时间等待首响应时三元组一直为空。
        let preferred_endpoint = initial_ordered.first();

        // 路由定型后先插 in-flight client 事件(不计数),长流请求即时可见。
        let client_event_id = new_event_id();
        let client_started = Instant::now();
        // 事件与 guard 共享同一开始时刻:accepted 回填时 timestamp 必须原样保留,
        // UI 用它显示流式「已持续秒数」,变了会清零重走。
        let client_timestamp = unix_to_apple_epoch(now_unix());
        let client_meta = ClientMeta {
            client_kind,
            source_format,
            target_format: preferred_endpoint.map(|endpoint| endpoint.protocol),
            route_mode: preferred_endpoint.map(|endpoint| endpoint.route_mode),
            client_model: plan.client_model.clone(),
            effective_model: plan.effective_model.clone(),
            feature_rule_id: plan.feature_rule_id.clone(),
            purpose,
            unmatched_no_tools,
            session_id: observed_session_id.clone(),
            codex_metadata: retain_codex_metadata_for_client(client_kind, codex_metadata),
            client_declared: ClientDeclaredMetadata::from_headers(&headers),
            grok_metadata: GrokMetadata::from_headers(&headers),
            request_context: current_request_context(),
        };
        self.capture_start(
            &client_event_id,
            method,
            path_and_query,
            &headers,
            &inbound_body,
            &client_meta,
        );
        self.record_event(RuntimeEvent {
            client_kind: Some(client_kind),
            client_model: Some(plan.client_model.clone()),
            source_format: Some(source_format),
            target_format: preferred_endpoint.map(|endpoint| endpoint.protocol),
            route_mode: preferred_endpoint.map(|endpoint| endpoint.route_mode),
            duration_ms: 0,
            effective_model: Some(plan.effective_model.clone()),
            endpoint_id: None,
            endpoint_name: None,
            failover: false,
            feature_rule_id: plan.feature_rule_id.clone(),
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: client_event_id.clone(),
            kind: KIND_CLIENT.into(),
            message: None,
            tool_calls: None,
            codex_metadata: client_meta.codex_metadata.clone(),
            client_declared: client_meta.client_declared.clone(),
            grok_metadata: client_meta.grok_metadata.clone(),
            outcome: None,
            phase: Some(RuntimeEventPhase::InFlight),
            pool_id: None,
            request_purpose: Some(purpose),
            request_id: Some(client_event_id.clone()),
            request_method: client_meta
                .request_context
                .as_ref()
                .map(|context| context.method.clone()),
            request_path: client_meta
                .request_context
                .as_ref()
                .map(|context| context.path.clone()),
            route_intent: client_meta
                .request_context
                .as_ref()
                .map(|context| context.route_intent.clone()),
            session_id: observed_session_id,
            status_code: 0,
            timestamp: client_timestamp,
            // 尚未选定入口,首字节由 accepted 时的回填补上。
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: None,
            upstream_model: Some(plan.effective_model.clone()),
            upstream_request_id: None,
            upstream_status_code: None,
        });

        let guard = CompletionGuard::new(
            self.clone(),
            client_event_id,
            client_started,
            client_timestamp,
            client_meta,
        );

        self.forward(
            config,
            &request,
            candidates,
            session_key,
            guard,
            method,
            path_and_query,
            &headers,
            client_out,
        )
        .await
    }

    fn ordered_endpoints(
        &self,
        plan: &sumpter_core::routing::RoutePlan,
        session_key: &sticky::SessionKey,
    ) -> Vec<PlannedEndpoint> {
        self.ordered_endpoint_candidates(&plan.endpoints, session_key, true)
    }

    /// Apply provider/model cooldown (when requested) and sticky-group
    /// ordering to a stable route candidate list. Cooldown is an admission
    /// gate for new requests; an already admitted request may explicitly
    /// retry the same candidate according to its retry policy.
    fn ordered_endpoint_candidates(
        &self,
        candidates: &[PlannedEndpoint],
        session_key: &sticky::SessionKey,
        respect_cooldown: bool,
    ) -> Vec<PlannedEndpoint> {
        let now = now_unix();
        let (eligible_endpoints, sticky_preferred) = {
            let mut state = self.inner.state.lock().unwrap();
            state.provider_model_health.retain(|_, health| {
                health
                    .cooling_until
                    .is_some_and(|until| until.is_finite() && until > now)
            });
            let mut available = Vec::new();
            for endpoint in candidates {
                if !respect_cooldown
                    || provider_model_cooling_until(&state.provider_model_health, endpoint, now)
                        .is_none()
                {
                    available.push(endpoint.clone());
                }
            }
            // Never admit a new request to a cooling provider/model pair. If
            // every candidate is cooling, the caller returns a bounded
            // 503/Retry-After instead of defeating the health gate by probing
            // the earliest recovering endpoint immediately.
            let eligible = available;
            let preferred = state
                .session_sticky
                .get(&session_key.value)
                .map(|entry| entry.label.clone());
            (eligible, preferred)
        };
        if eligible_endpoints.is_empty() {
            // An empty result is meaningful: every candidate is currently
            // cooling down.  Returning the original list here would bypass
            // the admission check in `forward`/WebSocket preparation and
            // immediately probe the very provider we just quarantined.
            return Vec::new();
        }
        let mut groups: Vec<(String, i64, usize, f64)> = Vec::new();
        for (index, endpoint) in eligible_endpoints.iter().enumerate() {
            let group = endpoint.scheduling_group().to_string();
            if let Some(existing) = groups.iter_mut().find(|item| item.0 == group) {
                existing.1 = existing.1.min(endpoint.priority);
            } else {
                groups.push((group, endpoint.priority, index, f64::INFINITY));
            }
        }
        groups.sort_by(|a, b| {
            let a_preferred = sticky_preferred.as_deref() == Some(a.0.as_str());
            let b_preferred = sticky_preferred.as_deref() == Some(b.0.as_str());
            b_preferred
                .cmp(&a_preferred)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut output: Vec<PlannedEndpoint> = Vec::with_capacity(candidates.len());
        for (group, _, _, _) in &groups {
            for endpoint in &eligible_endpoints {
                if endpoint.scheduling_group() == group {
                    output.push(endpoint.clone());
                }
            }
        }
        output
    }

    /// Return the shortest remaining provider/model cooldown only when every
    /// candidate is currently cooling. `None` means at least one candidate is
    /// dispatchable (or the route has no candidates at all).
    fn provider_model_cooldown_retry_after(&self, candidates: &[PlannedEndpoint]) -> Option<f64> {
        if candidates.is_empty() {
            return None;
        }
        let now = now_unix();
        let state = self.inner.state.lock().unwrap();
        let mut shortest = None;
        for endpoint in candidates {
            let until = provider_model_cooling_until(&state.provider_model_health, endpoint, now)?;
            let remaining = (until - now).clamp(0.0, MAX_RETRY_AFTER_SECS);
            shortest = Some(shortest.map_or(remaining, |current: f64| current.min(remaining)));
        }
        shortest
    }

    fn provider_cooldown_response(
        &self,
        mut guard: CompletionGuard,
        retry_after: f64,
        message: &str,
    ) -> Response {
        let request_id = guard.request_id().to_string();
        let failure = FailureInfo::provider_cooldown(retry_after);
        guard.complete(503, Some(message.to_string()), failure.clone());
        // Cooldown is a local admission decision, so Retry-After remains
        // mandatory even when the user's ordinary upstream retry-delay
        // passthrough option is disabled.
        proxy_failure_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "provider_cooldown",
            &request_id,
            &failure,
            None,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    async fn forward(
        &self,
        config: &AppConfig,
        request: &RoutingRequest,
        candidates: Vec<PlannedEndpoint>,
        session_key: sticky::SessionKey,
        mut guard: CompletionGuard,
        method: &str,
        path_and_query: &str,
        headers: &[(String, String)],
        client_out: Option<ClientOut>,
    ) -> Response {
        let retry = &config.retry;
        let request_id = guard.request_id().to_string();
        let client_stream = client_out.as_ref().map_or_else(
            || {
                request
                    .raw
                    .get("stream")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            },
            |client| client.stream,
        );
        let passthrough_active = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough.is_some());
        guard.set_passthrough(
            client_out
                .as_ref()
                .and_then(|client| client.passthrough.as_ref().map(|_| client.passthrough_kind)),
        );
        // Keep the full, translation-compatible candidate set. Cooldown is a
        // dispatch-time concern and is intentionally recomputed below for
        // every retry round; passing an already-filtered vector here would
        // make a cooling endpoint disappear until process restart.
        let ordered = self.ordered_endpoint_candidates(&candidates, &session_key, true);
        if ordered.is_empty()
            && let Some(retry_after) = self.provider_model_cooldown_retry_after(&candidates)
        {
            return self.provider_cooldown_response(
                guard,
                retry_after,
                "all compatible providers are cooling down",
            );
        }
        let initial_group = ordered
            .first()
            .map(|endpoint| endpoint.scheduling_group().to_string())
            .unwrap_or_default();
        let eligible_groups = ordered.iter().fold(Vec::new(), |mut groups, endpoint| {
            let group = endpoint.scheduling_group().to_string();
            if !groups.contains(&group) {
                groups.push(group);
            }
            groups
        });
        // 所有 Provider 入口统一参与会话粘性与调度组故障转移。
        let sticky_enabled = !ordered.is_empty();
        if sticky_enabled {
            self.ensure_session_assignment(
                &session_key,
                &initial_group,
                &eligible_groups,
                now_unix(),
            );
        }
        // `ordered_endpoints` 保证调度组连续。粘性组的每一轮先完整尝试，再按
        // sessionStickyRetries 重新执行整个组；HTTP 500 由独立次数控制，不触发整组重试。
        let sticky_endpoint_count = if sticky_enabled {
            ordered
                .iter()
                .take_while(|endpoint| endpoint.scheduling_group() == initial_group)
                .count()
        } else {
            0
        };
        let sticky_retry_limit = if sticky_endpoint_count > 0 {
            retry.session_sticky_retries.max(0)
        } else {
            0
        };
        let forward_started = Instant::now();
        let mut last_attempt_endpoint: Option<String> = None;
        let mut round: i64 = 0;
        let mut consecutive_500_endpoint: Option<String> = None;
        let mut consecutive_500_retries = 0_i64;
        // chat 桥 stop_reason 回映射依据(见 bridge::OpenAiStreamBridge)。
        let declared_stop_sequences = request
            .raw
            .get("stop_sequences")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|a| !a.is_empty());
        let realtime_secret_request_session = client_out
            .as_ref()
            .filter(|client| client.realtime_client_secret)
            .and_then(|client| client.passthrough.as_ref())
            .and_then(|body| realtime_client_secret_request_session(path_and_query, body));

        loop {
            round += 1;
            guard.set_round(round);
            // This is still the same admitted request. Do not re-apply the
            // cross-request cooldown here: configured deferred/sticky retries
            // must be able to probe the provider again and recover from a
            // transient 429/5xx. The cooldown remains active for the next
            // request admission.
            let round_ordered = self.ordered_endpoint_candidates(&candidates, &session_key, false);
            let round_sticky_endpoint_count = if sticky_enabled {
                round_ordered
                    .iter()
                    .take_while(|endpoint| endpoint.scheduling_group() == initial_group)
                    .count()
            } else {
                0
            };
            let mut round_state = RoundState {
                retryable_failures: 0,
                sticky_retryable_failures: 0,
                retry_after_seconds: None,
                last_failure: None,
            };

            let mut endpoint_index = 0usize;
            let mut sticky_retry_number = 0i64;
            let mut sticky_pass_checkpoint = round_state.sticky_retryable_failures;
            while endpoint_index < round_ordered.len() {
                // 即使组内某个候选因桥能力被跳过，也要在进入其它组前执行边界判断。
                if round_sticky_endpoint_count > 0
                    && endpoint_index == round_sticky_endpoint_count
                    && sticky_retry_number < sticky_retry_limit
                    && round_state.sticky_retryable_failures > sticky_pass_checkpoint
                {
                    sticky_retry_number += 1;
                    let delay =
                        retry_backoff_delay(sticky_retry_number, round_state.retry_after_seconds);
                    tokio::time::sleep(delay).await;
                    sticky_pass_checkpoint = round_state.sticky_retryable_failures;
                    endpoint_index = 0;
                    continue;
                }
                let endpoint = &round_ordered[endpoint_index];
                endpoint_index += 1;
                // client 完成事件只有一个协议三元组：尚未 accepted 时记录最后一个
                // 实际进入调度判断的入口；accepted 后 attach_upstream 会覆盖为胜出入口。
                guard.note_endpoint(endpoint);
                // openai 系入口 + 请求带 tools → 跳过(桥接会丢工具)。
                // WebSearch 是唯一的安全例外：它由严格用途指纹和最终
                // TargetFormat 决定服务端搜索构造，不依赖入口额外声明。
                let server_retrieval =
                    request_build::server_retrieval_enabled(endpoint, request, guard.meta.purpose);
                if !passthrough_active
                    && endpoint.protocol != ProviderProtocol::Anthropic
                    && !request.tools.is_empty()
                    && !server_retrieval
                {
                    self.complete_upstream(self.upstream_event(
                        endpoint,
                        400,
                        0,
                        false,
                        Some(message_tokens::OPENAI_TOOLS_UNSUPPORTED.into()),
                        guard.meta.purpose,
                        guard.meta.client_kind,
                        &request_id,
                    ));
                    continue;
                }
                // 【Rust 变更】空 key = 无鉴权上游(本地 LLM/内网中转):照常转发、
                // 不发鉴权头(见 request_build)。Swift 老版是 401 missing_secret 终止;
                // 配置疑似遗漏仍由 ConfigWarnings 提示,不再阻断请求。
                let configured_api_key = config
                    .endpoint(&endpoint.endpoint_id)
                    .map(|e| e.api_key.clone())
                    .unwrap_or_default();
                // A registered `ek_…` token is a valid Realtime credential in
                // its own right. Preserve it for native HTTP bootstrap calls
                // instead of silently replacing it with the endpoint key;
                // ordinary APIs always use the configured Provider key.
                let api_key = if is_realtime_http_path(path_without_query(path_and_query)) {
                    realtime_ephemeral_token(headers).unwrap_or(configured_api_key)
                } else {
                    configured_api_key
                };

                let is_failover = last_attempt_endpoint
                    .as_ref()
                    .is_some_and(|prev| *prev != endpoint.endpoint_id);
                last_attempt_endpoint = Some(endpoint.endpoint_id.clone());
                if is_failover {
                    guard.mark_failover();
                }

                // pinned 候选:exclusive 只用列表;否则追加 DNS 兜底;按 IP 健康重排。
                let mut candidates: Vec<Option<String>> =
                    endpoint.pinned_ips.iter().cloned().map(Some).collect();
                if !endpoint.pinned_ip_exclusive || candidates.is_empty() {
                    candidates.push(None);
                }
                let candidates = {
                    let mut state = self.inner.state.lock().unwrap();
                    state.ip_rotation += 1;
                    let rotation = state.ip_rotation;
                    scheduler::ordered_pinned_ips(
                        &candidates,
                        &state.ip_health,
                        now_unix(),
                        rotation,
                    )
                };

                let response_timeout = effective_response_timeout_for_request(
                    retry,
                    endpoint,
                    client_out
                        .as_ref()
                        .is_some_and(|client| client.passthrough_kind == PassthroughKind::Realtime),
                );
                let concurrency = retry.pinned_ip_concurrency.max(1) as usize;
                let capture_realtime_secret = client_out
                    .as_ref()
                    .is_some_and(|client| client.realtime_client_secret);
                let passthrough = client_out.as_ref().and_then(|client| {
                    client.passthrough.as_ref().map(|body| PassthroughRequest {
                        kind: client.passthrough_kind,
                        body: body.as_ref(),
                        content_type: client.content_type.as_deref(),
                        stream: client.stream,
                    })
                });

                // 候选 >1 且并发 >1 → pinned IP 赛跑;否则串行逐个尝试。
                let attempt = if candidates.len() > 1 && concurrency > 1 {
                    self.race_candidates(
                        endpoint,
                        request,
                        headers,
                        method,
                        path_and_query,
                        &api_key,
                        candidates,
                        concurrency,
                        response_timeout,
                        is_failover,
                        guard.meta.purpose,
                        guard.meta.client_kind,
                        &request_id,
                        passthrough,
                        &mut round_state,
                    )
                    .await
                } else {
                    self.serial_candidates(
                        endpoint,
                        request,
                        headers,
                        method,
                        path_and_query,
                        &api_key,
                        candidates,
                        response_timeout,
                        is_failover,
                        guard.meta.purpose,
                        guard.meta.client_kind,
                        &request_id,
                        passthrough,
                        &mut round_state,
                    )
                    .await
                };

                if let Some((response, attempt_started, winner_ip, capture_attempt_id)) = attempt {
                    // accepted:头未回写前的重试机会到此为止。
                    if response.status == 200 {
                        self.touch_session_success(
                            endpoint.scheduling_group(),
                            &session_key,
                            &initial_group,
                            &eligible_groups,
                            sticky_enabled,
                            now_unix(),
                        );
                    }
                    return self.relay(
                        config,
                        endpoint,
                        response,
                        guard,
                        method,
                        path_and_query,
                        attempt_started,
                        is_failover,
                        winner_ip,
                        capture_attempt_id,
                        declared_stop_sequences,
                        server_retrieval,
                        client_out,
                        client_stream,
                        capture_realtime_secret,
                        realtime_secret_request_session.clone(),
                    );
                }
                // HTTP 500 使用独立的入口内重试次数，不参与 sessionStickyRetries。
                // 达到次数后默认继续遍历下一个入口；关闭 failoverOn500 时在当前入口终止。
                let last_was_500 = round_state
                    .last_failure
                    .as_ref()
                    .is_some_and(|failure| failure.upstream_status_code == Some(500));
                if last_was_500 {
                    if consecutive_500_endpoint.as_deref() != Some(endpoint.endpoint_id.as_str()) {
                        consecutive_500_endpoint = Some(endpoint.endpoint_id.clone());
                        consecutive_500_retries = 0;
                    }
                    if consecutive_500_retries < retry.max_500_retries.max(0) {
                        consecutive_500_retries += 1;
                        endpoint_index -= 1;
                        let delay = retry_backoff_delay(
                            consecutive_500_retries,
                            round_state.retry_after_seconds,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                    consecutive_500_retries = 0;
                    if !retry.failover_on_500 {
                        // 保留当前入口的最终 500，跳过其它入口与跨轮重试。
                        endpoint_index = round_ordered.len();
                    }
                } else {
                    consecutive_500_endpoint = None;
                    consecutive_500_retries = 0;
                }
                if round_sticky_endpoint_count > 0
                    && endpoint_index == round_sticky_endpoint_count
                    && sticky_retry_number < sticky_retry_limit
                    && round_state.sticky_retryable_failures > sticky_pass_checkpoint
                {
                    sticky_retry_number += 1;
                    let delay =
                        retry_backoff_delay(sticky_retry_number, round_state.retry_after_seconds);
                    // 复用跨轮退避，避免原入口故障时瞬间轰击；sleep 可随客户端取消。
                    tokio::time::sleep(delay).await;
                    sticky_pass_checkpoint = round_state.sticky_retryable_failures;
                    endpoint_index = 0;
                }
            }

            // 整轮结束：只要出现过明确可重试 HTTP 状态或首响应前网络故障，就可
            // 跨轮重试。历史字段 maxDeferredRounds 保留磁盘兼容，现约束全部可重试故障。
            // Realtime/Live bootstrap requests create or negotiate a call and
            // must terminate after the endpoint candidates in this round have
            // been exhausted.  Allowing the ordinary `maxDeferredRounds: 0`
            // default here would turn a dead voice Provider into an endless
            // sequence of retries, leaving the client waiting forever.
            let realtime_request = client_out
                .as_ref()
                .is_some_and(|client| client.passthrough_kind == PassthroughKind::Realtime);
            let retry_allowed = !realtime_request
                && round_state.retryable_failures > 0
                && (retry.max_deferred_rounds <= 0 || round < retry.max_deferred_rounds)
                && (retry.max_retry_duration_seconds <= 0.0
                    || forward_started.elapsed().as_secs_f64() < retry.max_retry_duration_seconds);
            if retry_allowed {
                let delay = retry_backoff_delay(round, round_state.retry_after_seconds);
                let fits_time_cap = retry.max_retry_duration_seconds <= 0.0
                    || forward_started.elapsed().as_secs_f64() + delay.as_secs_f64()
                        < retry.max_retry_duration_seconds;
                if fits_time_cap {
                    tokio::time::sleep(delay).await;
                    continue;
                }
            }

            let mut failure = round_state
                .last_failure
                .unwrap_or_else(FailureInfo::endpoints_exhausted);
            // If the final transport failure followed a retryable response,
            // retain the largest bounded provider hint for the client-facing
            // error instead of dropping it at the round boundary.
            if failure.retry_after_seconds.is_none() {
                failure.retry_after_seconds = round_state.retry_after_seconds;
            }
            let (status, error, message) = match failure.upstream_status_code {
                Some(upstream_status) => (
                    StatusCode::from_u16(upstream_status as u16).unwrap_or(StatusCode::BAD_GATEWAY),
                    message_tokens::UPSTREAM_RETRYABLE_STATUS,
                    message_tokens::UPSTREAM_RETRYABLE_STATUS.to_string(),
                ),
                None => (
                    StatusCode::BAD_GATEWAY,
                    "upstream_unavailable",
                    match failure.kind {
                        RuntimeFailureKind::EndpointsExhausted => {
                            message_tokens::ALL_ENDPOINTS_FAILED.to_string()
                        }
                        RuntimeFailureKind::ResponseTimeout => "timeout".into(),
                        RuntimeFailureKind::ConnectionFailed => format!(
                            "connection failed: {}",
                            failure
                                .detail
                                .as_deref()
                                .unwrap_or("unknown transport error")
                        ),
                        RuntimeFailureKind::InvalidResponse => format!(
                            "invalid response: {}",
                            failure
                                .detail
                                .as_deref()
                                .unwrap_or("unknown response error")
                        ),
                        _ => failure
                            .detail
                            .clone()
                            .unwrap_or_else(|| message_tokens::ALL_ENDPOINTS_FAILED.into()),
                    },
                ),
            };
            let request_id = guard.request_id().to_string();
            guard.complete(status.as_u16() as i64, Some(message), failure.clone());
            return proxy_failure_response(
                status,
                error,
                &request_id,
                &failure,
                retry.retry_delay_seconds,
                retry.pass_through_retry_delay,
            );
        }
    }

    /// 串行逐候选尝试。返回 accepted 的响应(带胜出 pinned IP),或 None(全部候选
    /// 可重试/失败,结果记入 round_state)。
    #[allow(clippy::too_many_arguments)]
    async fn serial_candidates(
        &self,
        endpoint: &PlannedEndpoint,
        request: &RoutingRequest,
        headers: &[(String, String)],
        method: &str,
        path_and_query: &str,
        api_key: &str,
        candidates: Vec<Option<String>>,
        response_timeout: Option<f64>,
        is_failover: bool,
        purpose: RequestPurpose,
        client_kind: ClientKind,
        request_id: &str,
        passthrough: Option<PassthroughRequest<'_>>,
        round_state: &mut RoundState,
    ) -> Option<(
        crate::outbound::UpstreamResponse,
        Instant,
        Option<String>,
        String,
    )> {
        for candidate in candidates {
            let build = request_build::build_outbound(
                endpoint,
                request,
                headers,
                method,
                path_and_query,
                api_key,
                candidate.clone(),
                purpose,
                passthrough,
            );
            let attempt_started = Instant::now();
            let capture_id =
                self.capture_attempt_started(request_id, endpoint, &build.request, attempt_started);
            let result = send_streaming_with_deadline(
                &self.inner.transport,
                build.request,
                response_timeout,
            )
            .await;
            self.capture_attempt_result(request_id, &capture_id, &result);
            match self.note_attempt(
                endpoint,
                &candidate,
                attempt_started,
                is_failover,
                purpose,
                client_kind,
                request_id,
                response_timeout,
                result,
                round_state,
            ) {
                Some(response) => return Some((response, attempt_started, candidate, capture_id)),
                None => continue,
            }
        }
        None
    }

    /// pinned IP 并发赛跑:按并发度分批同发,第一个拿到**非可重试响应头**的候选独占回写;
    /// 胜者产生时其余在途候选随 futures drop 被撕掉,**零记账**(不进事件、不打冷却);
    /// 胜者出现前自然完成的失败/可重试候选正常记账。
    #[allow(clippy::too_many_arguments)]
    async fn race_candidates(
        &self,
        endpoint: &PlannedEndpoint,
        request: &RoutingRequest,
        headers: &[(String, String)],
        method: &str,
        path_and_query: &str,
        api_key: &str,
        candidates: Vec<Option<String>>,
        concurrency: usize,
        response_timeout: Option<f64>,
        is_failover: bool,
        purpose: RequestPurpose,
        client_kind: ClientKind,
        request_id: &str,
        passthrough: Option<PassthroughRequest<'_>>,
        round_state: &mut RoundState,
    ) -> Option<(
        crate::outbound::UpstreamResponse,
        Instant,
        Option<String>,
        String,
    )> {
        use futures_util::stream::FuturesUnordered;

        for batch in candidates.chunks(concurrency) {
            let mut in_flight = FuturesUnordered::new();
            for candidate in batch {
                let build = request_build::build_outbound(
                    endpoint,
                    request,
                    headers,
                    method,
                    path_and_query,
                    api_key,
                    candidate.clone(),
                    purpose,
                    passthrough,
                );
                let transport = self.inner.transport.clone();
                let capture_engine = self.clone();
                let capture_request_id = request_id.to_string();
                let capture_endpoint = endpoint.clone();
                let candidate = candidate.clone();
                in_flight.push(async move {
                    let started = Instant::now();
                    let capture_request = build.request.clone();
                    let capture_id = capture_engine.capture_attempt_started(
                        &capture_request_id,
                        &capture_endpoint,
                        &capture_request,
                        started,
                    );
                    let result =
                        send_streaming_with_deadline(&transport, build.request, response_timeout)
                            .await;
                    capture_engine.capture_attempt_result(
                        &capture_request_id,
                        &capture_id,
                        &result,
                    );
                    (candidate, started, capture_id, result)
                });
            }
            while let Some((candidate, started, capture_id, result)) = in_flight.next().await {
                if let Some(response) = self.note_attempt(
                    endpoint,
                    &candidate,
                    started,
                    is_failover,
                    purpose,
                    client_kind,
                    request_id,
                    response_timeout,
                    result,
                    round_state,
                ) {
                    // drop in_flight → 掐掉批内其余候选,输家零记账。
                    return Some((response, started, candidate, capture_id));
                }
            }
        }
        None
    }

    /// 单次尝试的统一记账:accepted 返回响应;可重试/失败记入 round_state 并返回 None。
    /// message 走词表(specs/spec-engine.md §5.1):pinned 候选带 `pinned <ip>` token,
    /// 传输失败再拼错误串——Swift 版只记其一,排查直连时缺哪个 IP 失败的信息。
    #[allow(clippy::too_many_arguments)]
    fn note_attempt(
        &self,
        endpoint: &PlannedEndpoint,
        candidate: &Option<String>,
        attempt_started: Instant,
        is_failover: bool,
        purpose: RequestPurpose,
        client_kind: ClientKind,
        request_id: &str,
        response_timeout: Option<f64>,
        result: Result<crate::outbound::UpstreamResponse, TransportError>,
        round_state: &mut RoundState,
    ) -> Option<crate::outbound::UpstreamResponse> {
        let now = now_unix();
        let pinned_token = candidate.as_deref().map(message_tokens::pinned);
        match result {
            Err(e) => {
                if let Some(ip) = candidate {
                    self.touch_ip(ip, false, now);
                }
                // Only retryable connection/timeout failures describe
                // provider health. A deterministic InvalidResponse is a
                // local request/decoder contract failure and must not cool an
                // otherwise healthy endpoint+model pair.
                if e.is_retryable_before_response() {
                    self.note_provider_model_failure(endpoint, None, None, now);
                }
                let failure = FailureInfo::from_transport(&e, response_timeout);
                let mut event = self.upstream_event(
                    endpoint,
                    502,
                    attempt_started.elapsed().as_millis() as i64,
                    is_failover,
                    message_tokens::join(&[pinned_token, Some(e.to_string())]),
                    purpose,
                    client_kind,
                    request_id,
                );
                failure.apply_to(&mut event);
                self.complete_upstream(event);
                if e.is_retryable_before_response() {
                    round_state.retryable_failures += 1;
                    round_state.sticky_retryable_failures += 1;
                }
                round_state.last_failure = Some(failure);
                None
            }
            Ok(response) => {
                if let Some(ip) = candidate {
                    self.touch_ip(ip, true, now);
                }
                if RetryPolicy::is_endpoint_retryable_status(response.status) {
                    let attempt_ttfb_ms = attempt_started.elapsed().as_millis() as i64;
                    let retry_after = retry_after_seconds(&response.headers);
                    self.note_provider_model_failure(
                        endpoint,
                        Some(response.status),
                        retry_after,
                        now,
                    );
                    let failure = FailureInfo::upstream_http(
                        response.status,
                        upstream_request_id(&response.headers),
                    );
                    let mut failure = failure;
                    failure.retry_after_seconds = retry_after;
                    let mut event = self.upstream_event(
                        endpoint,
                        response.status as i64,
                        attempt_ttfb_ms,
                        is_failover,
                        pinned_token,
                        purpose,
                        client_kind,
                        request_id,
                    );
                    failure.apply_to(&mut event);
                    event.ttfb_ms = Some(attempt_ttfb_ms);
                    self.complete_upstream(event);
                    if response.status != 500 {
                        round_state.retryable_failures += 1;
                        round_state.sticky_retryable_failures += 1;
                    }
                    round_state.note_retry_after(&response.headers);
                    round_state.last_failure = Some(failure);
                    return None;
                }
                // Any response header (including a non-retryable 4xx) proves
                // the endpoint is reachable for this model; clear an old
                // cooldown before relaying it to the client.
                self.note_provider_model_success(endpoint);
                Some(response)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn upstream_event(
        &self,
        endpoint: &PlannedEndpoint,
        status: i64,
        duration_ms: i64,
        failover: bool,
        message: Option<String>,
        purpose: RequestPurpose,
        client_kind: ClientKind,
        request_id: &str,
    ) -> RuntimeEvent {
        RuntimeEvent {
            client_kind: Some(client_kind),
            codex_metadata: None,
            client_declared: None,
            grok_metadata: None,
            client_model: None,
            source_format: Some(endpoint.source_format),
            target_format: Some(endpoint.protocol),
            route_mode: Some(endpoint.route_mode),
            duration_ms,
            effective_model: Some(endpoint.routed_model.clone()),
            endpoint_id: Some(endpoint.endpoint_id.clone()),
            endpoint_name: Some(endpoint.endpoint_name.clone()),
            failover,
            feature_rule_id: None,
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: new_event_id(),
            kind: KIND_UPSTREAM.into(),
            message,
            tool_calls: None,
            outcome: Some(if (200..=399).contains(&status) {
                RuntimeEventOutcome::Succeeded
            } else {
                RuntimeEventOutcome::Failed
            }),
            phase: Some(RuntimeEventPhase::Completed),
            pool_id: None,
            request_purpose: Some(purpose),
            request_id: Some(request_id.to_string()),
            request_method: None,
            request_path: None,
            route_intent: None,
            session_id: None,
            status_code: status,
            timestamp: unix_to_apple_epoch(now_unix()),
            // 收到响应头的尝试由调用方(relay / retryable 分支)覆盖;
            // 响应头前失败的尝试保持 None。
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: host_of(&endpoint.base_url),
            upstream_model: Some(endpoint.upstream_model.clone()),
            upstream_request_id: None,
            upstream_status_code: None,
        }
    }

    /// accepted 之后的 relay:直连剥 hop-by-hop 原样转;桥接改写 200 + SSE 头。
    /// 完成/中断/断开的记账全部挂在响应体流的生命周期上。
    #[allow(clippy::too_many_arguments)]
    fn relay(
        &self,
        config: &AppConfig,
        endpoint: &PlannedEndpoint,
        response: crate::outbound::UpstreamResponse,
        mut guard: CompletionGuard,
        method: &str,
        path_and_query: &str,
        attempt_started: Instant,
        is_failover: bool,
        pinned_ip: Option<String>,
        capture_attempt_id: String,
        request_declared_stop_sequences: bool,
        server_retrieval: bool,
        client_out: Option<ClientOut>,
        client_stream: bool,
        capture_realtime_secret: bool,
        realtime_secret_request_session: Option<Value>,
    ) -> Response {
        let live_bootstrap = is_live_bootstrap_request(method, path_and_query);
        let video_create = is_videos_create_request(method, path_and_query);
        let live_header_id = if live_bootstrap && (200..=299).contains(&response.status) {
            live_call_id_from_headers(&response.headers)
        } else {
            None
        };
        if live_bootstrap
            && (200..=299).contains(&response.status)
            && let Some(call_id) = live_header_id.as_deref()
        {
            self.register_live_session(call_id, &endpoint.endpoint_id, &endpoint.routed_model);
        }
        let video_header_id = if video_create && (200..=299).contains(&response.status) {
            video_id_from_headers(&response.headers)
        } else {
            None
        };
        if let Some(video_id) = video_header_id.as_deref() {
            self.register_video_session(video_id, &endpoint.endpoint_id, &endpoint.routed_model);
        }
        let passthrough = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough.is_some());
        let bridging = !passthrough
            && endpoint.protocol != ProviderProtocol::Anthropic
            && response.status == 200;
        let upstream_request_id = upstream_request_id(&response.headers);
        let response_retry_after = retry_after_seconds(&response.headers);
        let request_id = guard.request_id().to_string();

        // in-flight upstream 事件:响应头一到即可见;消息先带上直连/桥接信息,
        // 完成时由 guard 按同一词表重建覆盖。
        let upstream_event_id = new_event_id();
        let in_flight_message = message_tokens::join(&[
            pinned_ip.as_deref().map(message_tokens::pinned),
            match client_out
                .as_ref()
                .filter(|client| client.passthrough.is_some())
            {
                Some(client) => (response.status == 200).then(|| {
                    format!(
                        "{}{}",
                        message_tokens::PASSTHROUGH_PREFIX,
                        client.passthrough_kind.token()
                    )
                }),
                None => bridging.then(|| {
                    format!(
                        "{}{}",
                        message_tokens::BRIDGE_PREFIX,
                        protocol_token(endpoint.protocol)
                    )
                }),
            },
        ]);
        // 进到 relay 就代表响应头已到:此刻的 elapsed 即该次尝试的首字节耗时。
        let attempt_ttfb_ms = attempt_started.elapsed().as_millis() as i64;
        let mut in_flight = self.upstream_event(
            endpoint,
            response.status as i64,
            0,
            is_failover,
            in_flight_message.clone(),
            guard.meta.purpose,
            guard.meta.client_kind,
            &request_id,
        );
        in_flight.id = upstream_event_id.clone();
        in_flight.phase = Some(RuntimeEventPhase::InFlight);
        in_flight.outcome = None;
        in_flight.ttfb_ms = Some(attempt_ttfb_ms);
        in_flight.upstream_status_code = Some(response.status as i64);
        in_flight.upstream_request_id = upstream_request_id.clone();
        in_flight.client_model = Some(guard.meta.client_model.clone());
        in_flight.effective_model = Some(guard.meta.effective_model.clone());
        in_flight.feature_rule_id = guard.meta.feature_rule_id.clone();
        in_flight.codex_metadata = guard.meta.codex_metadata.clone();
        in_flight.client_declared = guard.meta.client_declared.clone();
        in_flight.grok_metadata = guard.meta.grok_metadata.clone();
        self.record_event(in_flight);

        guard.attach_upstream(UpstreamAttempt {
            event_id: upstream_event_id,
            endpoint: endpoint.clone(),
            started: attempt_started,
            ttfb_ms: attempt_ttfb_ms,
            is_failover,
            pinned_ip,
            upstream_request_id,
            retry_after_seconds: response_retry_after,
            capture_attempt_id,
        });
        guard.set_status(response.status as i64);
        // client in-flight 事件同步补上入口归属与直连/桥接信息。
        // unmatched_no_tools 是请求自身属性(与入口无关),只进 client 侧;
        // 这里就带上,免得流式期间不显示、完成后才冒出来。
        let client_in_flight_message = message_tokens::join(&[
            in_flight_message,
            guard
                .meta
                .unmatched_no_tools
                .then(|| message_tokens::UNMATCHED_NO_TOOLS.to_string()),
        ]);
        guard.record_streaming_started(client_in_flight_message);

        let bridge: Option<Box<dyn SseBridge>> = if bridging {
            let message_id = bridge::new_message_id();
            // 如果还要经过客户端方言桥,上游桥必须继续输出 Anthropic SSE;
            // 客户端桥才有事件可消费。只有单桥(Anthropic 入站)的非流式请求
            // 才允许由上游桥直接聚合成 JSON。
            let bridge_stream = client_stream
                || client_out
                    .as_ref()
                    .is_some_and(|client| client.passthrough.is_none());
            // 请求侧是否声明 stop_sequences:chat 桥回映射 stop_reason 用。
            let declared_stops = request_declared_stop_sequences;
            // 服务端检索模式：WebSearch/WebFetch 的上游搜索调用和引用需桥回 Anthropic SSE。
            match endpoint.protocol {
                ProviderProtocol::OpenAI => {
                    Some(Box::new(bridge::OpenAiStreamBridge::new_with_stream(
                        message_id,
                        endpoint.upstream_model.clone(),
                        declared_stops,
                        server_retrieval,
                        bridge_stream,
                    )))
                }
                ProviderProtocol::OpenAIResponses => {
                    Some(Box::new(bridge::ResponsesStreamBridge::new_with_stream(
                        message_id,
                        endpoint.upstream_model.clone(),
                        server_retrieval,
                        bridge_stream,
                    )))
                }
                ProviderProtocol::Anthropic => None,
            }
        } else {
            None
        };

        // 客户端方言桥:统一 Anthropic SSE → chat/Responses SSE 或非流式 JSON。
        let client_bridge: Option<Box<dyn SseBridge>> = match (&client_out, response.status) {
            (Some(client), 200) if client.passthrough.is_none() => {
                let message_id = bridge::new_message_id();
                Some(
                    match client.dialect.expect("bridged OpenAI client has a dialect") {
                        ClientDialect::Chat => Box::new(bridge_in::ChatClientBridge::new(
                            message_id,
                            endpoint.upstream_model.clone(),
                            client.stream,
                        )) as Box<dyn SseBridge>,
                        ClientDialect::Responses => {
                            Box::new(bridge_in::ResponsesClientBridge::new(
                                message_id,
                                endpoint.upstream_model.clone(),
                                client.stream,
                            ))
                        }
                    },
                )
            }
            _ => None,
        };
        let client_streaming =
            client_bridge.is_some() && client_out.as_ref().is_some_and(|client| client.stream);
        let client_json = client_bridge.is_some() && !client_streaming;
        // Observe both SSE and non-stream JSON at the client boundary. The
        // latter is required for non-stream tool calls and structured failed /
        // incomplete statuses; the observer only emits a terminal once the
        // complete bounded JSON object has arrived.
        // Raw relay must never terminate early based on a JSON field that only
        // happens to resemble a known protocol terminal event. Known native
        // protocol paths still need their terminal tracker for correct stream
        // lifecycle accounting; only the explicit Raw marker disables it.
        let raw_passthrough = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough_kind == PassthroughKind::Raw);
        let terminal_tracker = (!raw_passthrough && response.status == 200).then(|| {
            let dialect = client_out
                .as_ref()
                .map(|client| client.terminal_dialect)
                .unwrap_or(SseDialect::Anthropic);
            SseTerminalTracker::new(dialect)
        });
        let upstream_summary_tracker = (response.status == 200).then(|| {
            let dialect = if passthrough {
                client_out
                    .as_ref()
                    .map(|client| client.terminal_dialect)
                    .unwrap_or(SseDialect::Anthropic)
            } else {
                match endpoint.protocol {
                    ProviderProtocol::Anthropic => SseDialect::Anthropic,
                    ProviderProtocol::OpenAI => SseDialect::OpenAiChat,
                    ProviderProtocol::OpenAIResponses => SseDialect::OpenAiResponses,
                }
            };
            SseTerminalTracker::new(dialect)
        });

        let realtime_request = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough_kind == PassthroughKind::Realtime);
        let idle_timeout =
            effective_stream_idle_timeout_for_request(&config.retry, realtime_request)
                .map(Duration::from_secs_f64);
        // Native conversation endpoints have a JSON-level terminal contract
        // even when the caller requested `stream: false`.  A 200 response
        // whose body is malformed, truncated, or still in progress must not
        // be counted as a successful request merely because the transport
        // reached EOF.  Keep the stricter check scoped to Messages and native
        // Chat/Responses; compact, images, completions, count_tokens and
        // alpha-search use independent unary schemas without a shared
        // terminal field.
        let strict_terminal = response.status == 200
            && if passthrough && !client_stream {
                client_out.as_ref().is_some_and(|client| {
                    matches!(
                        client.passthrough_kind,
                        PassthroughKind::Chat | PassthroughKind::Responses
                    )
                })
            } else if !passthrough && client_out.is_none() && !client_stream {
                // Native Anthropic Messages (`/v1/messages`).
                true
            } else {
                passthrough && client_stream
            };
        let state = RelayState {
            upstream: response.stream,
            bridge,
            client_bridge,
            terminal_tracker,
            upstream_summary_tracker,
            observe_sse: client_stream,
            upstream_observe_sse: client_stream || bridging || client_json || client_streaming,
            strict_terminal,
            realtime_secret_body: (capture_realtime_secret
                && (200..=299).contains(&response.status))
            .then(Vec::new),
            realtime_secret_endpoint_id: capture_realtime_secret
                .then(|| endpoint.endpoint_id.clone()),
            realtime_secret_model: capture_realtime_secret.then(|| endpoint.routed_model.clone()),
            realtime_secret_session: realtime_secret_request_session,
            live_session_body: (live_bootstrap
                && (200..=299).contains(&response.status)
                && live_header_id.is_none())
            .then(Vec::new),
            live_session_endpoint_id: live_bootstrap.then(|| endpoint.endpoint_id.clone()),
            live_session_model: live_bootstrap.then(|| endpoint.routed_model.clone()),
            video_session_body: (video_create
                && (200..=299).contains(&response.status)
                && video_header_id.is_none())
            .then(Vec::new),
            video_session_endpoint_id: video_create.then(|| endpoint.endpoint_id.clone()),
            video_session_model: video_create.then(|| endpoint.routed_model.clone()),
            idle_timeout,
            guard,
            finished: false,
        };
        let body_stream = futures_util::stream::unfold(state, |mut st| async move {
            if st.finished {
                // A protocol terminal may have been observed in the same
                // chunk that was returned to the client.  In that case the
                // next poll reaches this fast path instead of the EOF arm;
                // finalize bounded response metadata here as an idempotent
                // safety net so Live/Video bindings (and client secrets) are
                // not lost.
                st.finalize_response_metadata();
                return None;
            }
            loop {
                let next = read_next(&mut st).await;
                match next {
                    Ok(Some(chunk)) => {
                        st.guard.record_stream_chunk(chunk.len());
                        if let Some(secret_body) = &mut st.realtime_secret_body {
                            const MAX_SECRET_RESPONSE_BYTES: usize = 64 * 1024;
                            let remaining =
                                MAX_SECRET_RESPONSE_BYTES.saturating_sub(secret_body.len());
                            if remaining > 0 {
                                secret_body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                            }
                        }
                        if let Some(video_body) = &mut st.video_session_body {
                            const MAX_VIDEO_RESPONSE_BYTES: usize = 64 * 1024;
                            let remaining =
                                MAX_VIDEO_RESPONSE_BYTES.saturating_sub(video_body.len());
                            if remaining > 0 {
                                video_body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                            }
                        }
                        if let Some(live_body) = &mut st.live_session_body {
                            const MAX_LIVE_RESPONSE_BYTES: usize = 64 * 1024;
                            let remaining = MAX_LIVE_RESPONSE_BYTES.saturating_sub(live_body.len());
                            if remaining > 0 {
                                live_body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
                            }
                        }
                        // Register stateful resource bindings at first
                        // observation rather than waiting for EOF.  A client
                        // may disconnect immediately after receiving the
                        // creation object; the binding must still be usable
                        // for its subsequent lookup/sideband request.
                        st.observe_resource_metadata();
                        if let Some(attempt_id) = st.guard.capture_attempt_id() {
                            st.guard.engine.capture_upstream_chunk(
                                st.guard.request_id(),
                                attempt_id,
                                &chunk,
                                st.guard.started.elapsed().as_millis() as i64,
                            );
                        }
                        if let Some(tracker) = &mut st.upstream_summary_tracker {
                            if st.upstream_observe_sse {
                                let _ = tracker.push(&chunk);
                            } else {
                                tracker.observe_json(&chunk);
                            }
                            st.guard.stream_trace.record_response_summary(tracker);
                        }
                        let mut out = match &mut st.bridge {
                            Some(bridge) => bridge.feed(&chunk),
                            None => chunk.to_vec(),
                        };
                        if let Some(client) = &mut st.client_bridge {
                            out = client.feed(&out);
                        }
                        let terminal = st.terminal_tracker.as_mut().and_then(|tracker| {
                            if st.observe_sse {
                                tracker.push(&out)
                            } else {
                                tracker.observe_json(&out)
                            }
                        });
                        if let Some(tracker) = &st.terminal_tracker {
                            st.guard.set_tool_calls(tracker.tool_calls());
                        }
                        if let Some(terminal) = terminal {
                            // 协议终止事件已经包含在本次返回给客户端的字节中。先完成
                            // 记账，再让下一次 poll 结束 body；客户端此时释放流不再是 499。
                            st.finished = true;
                            st.guard.record_stream_terminal(&terminal);
                            st.guard.complete_from_protocol_terminal(terminal);
                            // `finished` is set before the chunk is yielded,
                            // so the next poll would otherwise skip the EOF
                            // finalizer.  Register IDs immediately while the
                            // complete terminal chunk is still available.
                            st.finalize_response_metadata();
                        }
                        if !out.is_empty() {
                            st.guard.engine.capture_client_chunk(
                                st.guard.request_id(),
                                &out,
                                st.guard.started.elapsed().as_millis() as i64,
                            );
                        }
                        if out.is_empty() {
                            continue;
                        }
                        return Some((Ok(Bytes::from(out)), st));
                    }
                    Ok(None) => {
                        st.finished = true;
                        st.finalize_response_metadata();
                        let mut tail = st.bridge.as_mut().map(|b| b.finish()).unwrap_or_default();
                        if let Some(client) = &mut st.client_bridge {
                            let mut client_tail = client.feed(&tail);
                            client_tail.extend(client.finish());
                            tail = client_tail;
                        }
                        let terminal = st.terminal_tracker.as_mut().and_then(|tracker| {
                            if st.observe_sse {
                                tracker.push(&tail)
                            } else {
                                tracker.observe_json(&tail)
                            }
                        });
                        if let Some(tracker) = &st.terminal_tracker {
                            st.guard.set_tool_calls(tracker.tool_calls());
                        }
                        if let Some(terminal) = terminal {
                            st.guard.record_stream_terminal(&terminal);
                            st.guard.complete_from_protocol_terminal(terminal);
                        } else if st.strict_terminal {
                            st.guard
                                .complete_from_stream(Some(StreamReadError::MissingTerminal));
                        } else {
                            st.guard.complete_from_stream(None);
                        }
                        if !tail.is_empty() {
                            st.guard.engine.capture_client_chunk(
                                st.guard.request_id(),
                                &tail,
                                st.guard.started.elapsed().as_millis() as i64,
                            );
                        }
                        if tail.is_empty() {
                            return None;
                        }
                        return Some((Ok(Bytes::from(tail)), st));
                    }
                    Err(e) => {
                        st.finished = true;
                        // If a provider closed after sending a complete JSON
                        // object but before a protocol terminal/EOF, the
                        // bounded parsers can still recover a resource ID.
                        // They are deliberately idempotent and only accept a
                        // validated ID, so attempting finalization here is
                        // safe and avoids losing a binding on a late stream
                        // error.
                        st.finalize_response_metadata();
                        let display = e.to_string();
                        st.guard.complete_from_stream(Some(e));
                        return Some((Err(std::io::Error::other(display)), st));
                    }
                }
            }
        });

        let mut builder = Response::builder();
        builder = builder.header("x-sumpter-request-id", &request_id);
        if client_json || (bridging && !client_stream) {
            // 任一方向的非流式协议桥都必须返回单个 JSON 对象。
            builder = builder
                .status(StatusCode::OK)
                .header("content-type", "application/json");
        } else if bridging || client_streaming {
            builder = builder
                .status(StatusCode::OK)
                .header("content-type", "text/event-stream; charset=utf-8")
                .header("cache-control", "no-cache");
        } else {
            builder =
                builder.status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::OK));
            for (name, value) in &response.headers {
                if is_hop_by_hop(name) || name == "content-length" || name == "x-sumpter-request-id"
                {
                    continue;
                }
                if let (Ok(n), Ok(v)) = (
                    HeaderName::from_bytes(name.as_bytes()),
                    HeaderValue::from_str(value),
                ) {
                    builder = builder.header(n, v);
                }
            }
        }
        builder
            .body(Body::from_stream(body_stream))
            .unwrap_or_else(|_| {
                error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &[("error", "stream_failed")],
                )
            })
    }
}

#[async_trait::async_trait]
impl EngineCapabilities for Engine {
    fn runtime_snapshot(&self) -> RuntimeSnapshot {
        Engine::runtime_snapshot(self)
    }

    fn replace_config(&self, config: AppConfig) -> Result<ConfigReplacement, String> {
        let (generation, warnings) = Engine::replace_config(self, config);
        Ok(ConfigReplacement {
            generation,
            warnings,
        })
    }

    fn reload_config(&self) -> Result<ConfigReplacement, String> {
        Engine::reload_config(self)
    }

    fn record_platform_event(&self, event: RuntimeEvent) {
        self.record_event(event);
    }

    fn publish_platform_notice(&self, notice: PlatformNotice) {
        let _ = self
            .inner
            .notices
            .send(EngineNotice::PlatformNotice(notice));
    }
}

fn websocket_http_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    headers
        .iter()
        .filter(|(name, _)| {
            let lower = name.to_ascii_lowercase();
            !matches!(
                lower.as_str(),
                "connection"
                    | "upgrade"
                    | "host"
                    | "content-length"
                    | "sec-websocket-key"
                    | "sec-websocket-version"
                    | "sec-websocket-extensions"
                    | "sec-websocket-protocol"
            )
        })
        .cloned()
        .collect()
}

/// Headers copied to a native upstream WebSocket.  Downstream listener
/// credentials must never be forwarded, and provider auth is injected from
/// the selected endpoint below; omitting them also prevents duplicate
/// `Authorization` headers when the client used the same name for both hops.
fn websocket_upstream_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
    let mut forwarded = websocket_http_headers(headers);
    // Subprotocol is part of the WebSocket handshake contract (Realtime
    // providers may use it for version negotiation), unlike the hop-by-hop
    // transport headers removed above.
    if let Some(protocol) = header_value(headers, "sec-websocket-protocol") {
        forwarded.push(("sec-websocket-protocol".into(), protocol.to_string()));
    }
    forwarded
        .into_iter()
        .filter(|(name, _)| {
            !name.eq_ignore_ascii_case("authorization")
                && !name.eq_ignore_ascii_case("x-api-key")
                && !matches!(
                    name.to_ascii_lowercase().as_str(),
                    "x-sumpter-project"
                        | "x-sumpter-workspace"
                        | "x-sumpter-git-remote"
                        | "x-sumpter-user"
                )
        })
        .collect()
}

fn realtime_ephemeral_token(headers: &[(String, String)]) -> Option<String> {
    headers.iter().rev().find_map(|(name, value)| {
        let candidate = if name.eq_ignore_ascii_case("authorization") {
            let value = value.trim();
            value
                .get(..7)
                .filter(|prefix| prefix.eq_ignore_ascii_case("bearer "))
                .map(|_| value[7..].trim())
        } else if name.eq_ignore_ascii_case("x-api-key") {
            Some(value.trim())
        } else {
            None
        }?;
        (candidate.starts_with("ek_")
            && candidate.len() <= 512
            && !candidate.chars().any(char::is_control))
        .then(|| candidate.to_string())
    })
}

async fn send_websocket_json_error(
    socket: &mut WebSocket,
    error_type: &str,
    message: &str,
) -> Result<(), axum::Error> {
    let payload = serde_json::to_string(&json!({
        "type": "error",
        "error": {"type": error_type, "message": message}
    }))
    .unwrap_or_else(|_| "{\"type\":\"error\",\"error\":{\"type\":\"server_error\"}}".into());
    socket.send(WebSocketMessage::Text(payload.into())).await
}

async fn relay_native_websocket(
    socket: WebSocket,
    upstream: NativeWebSocket,
    initial_message: Option<WebSocketMessage>,
) -> WebSocketRelayMetrics {
    let started = Instant::now();
    let counters = Arc::new(WebSocketRelayCounters::default());
    let (mut downstream_tx, mut downstream_rx) = socket.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    if let Some(message) = initial_message {
        counters.observe_first_client_text(&message);
        let bytes = websocket_message_size(&message);
        if upstream_tx
            .send(websocket_message_to_tungstenite(message))
            .await
            .is_err()
        {
            counters.record_error("initial_frame_send_failed", "relay_error");
            let _ = downstream_tx.close().await;
            return counters.snapshot(started);
        }
        counters.bytes_sent.fetch_add(bytes, Ordering::Relaxed);
        counters
            .client_message_count
            .fetch_add(1, Ordering::Relaxed);
    }
    let cancellation = tokio_util::sync::CancellationToken::new();
    let downstream_counters = counters.clone();
    let downstream_cancel = cancellation.clone();
    let downstream_to_upstream = async {
        loop {
            let next = tokio::select! {
                _ = downstream_cancel.cancelled() => None,
                message = downstream_rx.next() => message,
            };
            let Some(next) = next else {
                // A clean WebSocket close frame is surfaced as a message;
                // EOF without one is an abnormal client-side termination.
                if !downstream_cancel.is_cancelled() {
                    downstream_counters.record_error("client_eof_without_close", "client");
                }
                break;
            };
            let message = match next {
                Ok(message) => message,
                Err(_) => {
                    downstream_counters.record_error("client_receive_failed", "client");
                    break;
                }
            };
            downstream_counters.observe_first_client_text(&message);
            let bytes = websocket_message_size(&message);
            let close_code = axum_close_code(&message);
            let is_close = matches!(&message, WebSocketMessage::Close(_));
            let converted = websocket_message_to_tungstenite(message);
            if upstream_tx.send(converted).await.is_err() {
                downstream_counters.record_error("client_to_upstream_send_failed", "relay_error");
                break;
            }
            downstream_counters
                .bytes_sent
                .fetch_add(bytes, Ordering::Relaxed);
            downstream_counters
                .client_message_count
                .fetch_add(1, Ordering::Relaxed);
            if is_close {
                if let Some(code) = close_code {
                    *downstream_counters.client_close_code.lock().unwrap() = Some(code);
                }
                downstream_counters.set_closed_by("client");
                break;
            }
        }
        let _ = upstream_tx.close().await;
        downstream_cancel.cancel();
    };
    let upstream_counters = counters.clone();
    let upstream_cancel = cancellation.clone();
    let upstream_to_downstream = async {
        loop {
            let next = tokio::select! {
                _ = upstream_cancel.cancelled() => None,
                message = upstream_rx.next() => message,
            };
            let Some(next) = next else {
                if !upstream_cancel.is_cancelled() {
                    upstream_counters.record_error("upstream_eof_without_close", "upstream");
                }
                break;
            };
            let message = match next {
                Ok(message) => message,
                Err(_) => {
                    upstream_counters.record_error("upstream_receive_failed", "upstream");
                    break;
                }
            };
            let (bytes, close_code) = tungstenite_message_stats(&message);
            let converted = match message {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    WebSocketMessage::Text(text.to_string().into())
                }
                tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                    WebSocketMessage::Binary(bytes.to_vec().into())
                }
                tokio_tungstenite::tungstenite::Message::Ping(bytes) => {
                    WebSocketMessage::Ping(bytes.to_vec().into())
                }
                tokio_tungstenite::tungstenite::Message::Pong(bytes) => {
                    WebSocketMessage::Pong(bytes.to_vec().into())
                }
                tokio_tungstenite::tungstenite::Message::Close(frame) => {
                    WebSocketMessage::Close(frame.map(|frame| axum::extract::ws::CloseFrame {
                        code: frame.code.into(),
                        reason: frame.reason.to_string().into(),
                    }))
                }
                tokio_tungstenite::tungstenite::Message::Frame(_) => continue,
            };
            let is_close = matches!(&converted, WebSocketMessage::Close(_));
            if is_close {
                if let Some(code) = close_code {
                    *upstream_counters.upstream_close_code.lock().unwrap() = Some(code);
                }
                upstream_counters.set_closed_by("upstream");
            }
            if downstream_tx.send(converted).await.is_err() {
                upstream_counters.record_error("upstream_to_client_send_failed", "relay_error");
                break;
            }
            upstream_counters
                .bytes_received
                .fetch_add(bytes, Ordering::Relaxed);
            upstream_counters
                .upstream_message_count
                .fetch_add(1, Ordering::Relaxed);
            if is_close {
                break;
            }
        }
        let _ = downstream_tx.close().await;
        upstream_cancel.cancel();
    };
    let (_downstream_result, _upstream_result) =
        tokio::join!(downstream_to_upstream, upstream_to_downstream,);
    counters.snapshot(started)
}

impl WebSocketRelayCounters {
    fn observe_first_client_text(&self, message: &WebSocketMessage) {
        if !matches!(message, WebSocketMessage::Text(_))
            || self.first_client_text_seen.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let Some(metadata) = websocket_message_codex_metadata(message) else {
            return;
        };
        *self.first_client_codex_metadata.lock().unwrap() = Some(metadata);
    }

    fn set_closed_by(&self, side: &str) {
        let mut closed_by = self.closed_by.lock().unwrap();
        if closed_by.is_none() {
            *closed_by = Some(side.to_string());
        }
    }

    fn record_error(&self, detail: &str, closed_by: &str) {
        self.failed.store(true, Ordering::Release);
        self.abnormal_close.store(true, Ordering::Release);
        self.set_closed_by(closed_by);
        let mut relay_error = self.relay_error.lock().unwrap();
        if relay_error.is_none() {
            *relay_error = Some(detail.to_string());
        }
    }

    fn snapshot(&self, started: Instant) -> WebSocketRelayMetrics {
        WebSocketRelayMetrics {
            bytes_sent: self.bytes_sent.load(Ordering::Relaxed),
            bytes_received: self.bytes_received.load(Ordering::Relaxed),
            client_message_count: self.client_message_count.load(Ordering::Relaxed),
            upstream_message_count: self.upstream_message_count.load(Ordering::Relaxed),
            client_close_code: *self.client_close_code.lock().unwrap(),
            upstream_close_code: *self.upstream_close_code.lock().unwrap(),
            closed_by: self.closed_by.lock().unwrap().clone(),
            relay_error: self.relay_error.lock().unwrap().clone(),
            abnormal_close: self.abnormal_close.load(Ordering::Acquire),
            failed: self.failed.load(Ordering::Acquire),
            first_client_codex_metadata: self.first_client_codex_metadata.lock().unwrap().clone(),
            duration_ms: started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        }
    }
}

fn axum_close_code(message: &WebSocketMessage) -> Option<i64> {
    match message {
        WebSocketMessage::Close(frame) => frame.as_ref().map(|frame| frame.code as i64),
        _ => None,
    }
}

fn websocket_message_size(message: &WebSocketMessage) -> u64 {
    match message {
        WebSocketMessage::Text(text) => text.len() as u64,
        WebSocketMessage::Binary(bytes)
        | WebSocketMessage::Ping(bytes)
        | WebSocketMessage::Pong(bytes) => bytes.len() as u64,
        WebSocketMessage::Close(frame) => frame
            .as_ref()
            .map_or(0, |frame| 2 + frame.reason.len() as u64),
    }
}

/// Parse only the first bounded client text frame for attribution. The frame
/// is never retained; this returns the same redacted Codex metadata projection
/// used by HTTP request bodies. Responses WebSocket implementations have used
/// both a top-level envelope and a nested `response` object, so accept either
/// shape without inspecting arbitrary frame fields.
fn websocket_message_codex_metadata(message: &WebSocketMessage) -> Option<CodexMetadata> {
    let WebSocketMessage::Text(text) = message else {
        return None;
    };
    if text.len() > MAX_WEBSOCKET_METADATA_FRAME_BYTES {
        return None;
    }
    let value = serde_json::from_str::<Value>(text).ok()?;
    let direct = CodexMetadata::from_request(&[], Some(&value));
    let nested = value
        .get("response")
        .filter(|response| response.is_object())
        .and_then(|response| CodexMetadata::from_request(&[], Some(response)));
    match (direct, nested) {
        (Some(mut direct), Some(nested)) => {
            if direct.originator.is_none() {
                direct.originator = nested.originator;
            }
            if direct.session_id.is_none() {
                direct.session_id = nested.session_id;
            }
            if direct.thread_id.is_none() {
                direct.thread_id = nested.thread_id;
            }
            if direct.turn_id.is_none() {
                direct.turn_id = nested.turn_id;
            }
            Some(direct)
        }
        (Some(metadata), None) | (None, Some(metadata)) => Some(metadata),
        (None, None) => None,
    }
}

fn tungstenite_message_stats(
    message: &tokio_tungstenite::tungstenite::Message,
) -> (u64, Option<i64>) {
    match message {
        tokio_tungstenite::tungstenite::Message::Text(text) => (text.len() as u64, None),
        tokio_tungstenite::tungstenite::Message::Binary(bytes)
        | tokio_tungstenite::tungstenite::Message::Ping(bytes)
        | tokio_tungstenite::tungstenite::Message::Pong(bytes) => (bytes.len() as u64, None),
        tokio_tungstenite::tungstenite::Message::Close(frame) => (
            frame
                .as_ref()
                .map_or(0, |frame| frame.reason.len() as u64 + 2),
            frame.as_ref().map(|frame| i64::from(u16::from(frame.code))),
        ),
        tokio_tungstenite::tungstenite::Message::Frame(_) => (0, None),
    }
}

async fn send_realtime_session_update(
    upstream: &mut NativeWebSocket,
    session: &Value,
    upstream_model: &str,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let Some(mut session) = session.as_object().cloned() else {
        return Ok(());
    };
    for field in ["id", "object", "expires_at", "client_secret"] {
        session.remove(field);
    }
    if !upstream_model.trim().is_empty() {
        session.insert(
            "model".into(),
            Value::String(upstream_model.trim().to_string()),
        );
    }
    let payload = serde_json::to_string(&json!({
        "type": "session.update",
        "session": session,
    }))
    .map_err(|error| {
        tokio_tungstenite::tungstenite::Error::Io(std::io::Error::other(error.to_string()))
    })?;
    upstream
        .send(tokio_tungstenite::tungstenite::Message::Text(
            payload.into(),
        ))
        .await
}

fn websocket_connect_error(error: &tokio_tungstenite::tungstenite::Error) -> (u16, String) {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => (
            response.status().as_u16(),
            "upstream websocket handshake rejected".into(),
        ),
        // Zero means there was no upstream HTTP response. The downstream
        // adapter still returns 502, while runtime attribution keeps this as
        // a connection failure with null upstreamStatusCode/ttfbMS instead of
        // fabricating an upstream HTTP 502.
        _ => (0, "upstream websocket unavailable".into()),
    }
}

fn websocket_connect_retry_after(error: &tokio_tungstenite::tungstenite::Error) -> Option<f64> {
    let tokio_tungstenite::tungstenite::Error::Http(response) = error else {
        return None;
    };
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            Some((name.as_str().to_string(), value.to_str().ok()?.to_string()))
        })
        .collect::<Vec<_>>();
    retry_after_seconds(&headers)
}

fn websocket_event_context(
    path_and_query: &str,
    headers: &[(String, String)],
    model: &str,
    intent: RealtimeRouteIntent,
    started: Instant,
) -> WebSocketEventContext {
    let path = path_without_query(path_and_query);
    let route_intent = if is_realtime_http_path(path) {
        match intent {
            RealtimeRouteIntent::CodexLive => "live",
            RealtimeRouteIntent::StandardRealtime => "realtime",
        }
    } else if is_responses_websocket_path(path) {
        "responses_websocket"
    } else {
        "websocket"
    };
    let client_kind = detect_client_kind(headers, true);
    WebSocketEventContext {
        request_id: new_event_id(),
        request_path: bounded_request_path(path),
        route_intent: route_intent.into(),
        client_kind,
        model: model.trim().to_string(),
        codex_metadata: retain_codex_metadata_for_client(
            client_kind,
            CodexMetadata::from_request(headers, None),
        ),
        client_declared: ClientDeclaredMetadata::from_headers(headers),
        grok_metadata: GrokMetadata::from_headers(headers),
        session_id: observed_session_id(headers),
        started,
    }
}

/// Merge the bounded metadata projection observed in a first client frame
/// into the handshake context. A Responses WebSocket may use a generic
/// browser-like User-Agent and put `originator` only in `response.create`;
/// that frame must still be attributable as Codex without persisting its
/// prompt or raw JSON. Handshake identity remains authoritative when both
/// sources disagree.
fn websocket_context_with_first_frame(
    context: &WebSocketEventContext,
    frame: Option<&CodexMetadata>,
) -> WebSocketEventContext {
    let Some(frame) = frame else {
        return context.clone();
    };
    let mut merged = context.clone();
    merged.codex_metadata = merge_codex_metadata(merged.codex_metadata.take(), Some(frame.clone()));
    if merged.session_id.is_none() {
        merged.session_id = merged
            .codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.session_id.clone());
    }
    if matches!(
        merged.client_kind,
        ClientKind::OpenaiCompat | ClientKind::Unknown
    ) && merged
        .codex_metadata
        .as_ref()
        .and_then(|metadata| metadata.originator.as_deref())
        .is_some_and(is_codex_originator)
    {
        merged.client_kind = ClientKind::Codex;
    }
    merged
}

/// Merge two bounded Codex metadata projections without allowing a later
/// WebSocket frame to overwrite authoritative handshake/header values.  The
/// helper intentionally copies only identity/diagnostic fields that are safe
/// to retain; prompt and transport payload fields are never introduced here.
fn merge_codex_metadata(
    base: Option<CodexMetadata>,
    overlay: Option<CodexMetadata>,
) -> Option<CodexMetadata> {
    let Some(overlay) = overlay else {
        return base;
    };
    let Some(mut base) = base else {
        return Some(overlay);
    };
    if base.originator.is_none() {
        base.originator = overlay.originator.clone();
    }
    if base.session_id.is_none() {
        base.session_id = overlay.session_id.clone();
    }
    if base.thread_id.is_none() {
        base.thread_id = overlay.thread_id.clone();
    }
    if base.turn_id.is_none() {
        base.turn_id = overlay.turn_id.clone();
    }
    base.malformed |= overlay.malformed;
    base.truncated |= overlay.truncated;
    base.has_conflicts |= overlay.has_conflicts;
    base.is_subagent |= overlay.is_subagent;
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

fn is_codex_originator(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.contains("codex desktop") || value.contains("codex_cli_rs") || value.contains("codex-tui")
}

#[allow(clippy::too_many_arguments)]
fn websocket_trace(
    handshake_status: i64,
    bytes_sent: u64,
    bytes_received: u64,
    client_message_count: u64,
    upstream_message_count: u64,
    client_close_code: Option<i64>,
    upstream_close_code: Option<i64>,
    closed_by: Option<String>,
    relay_error: Option<String>,
    abnormal_close: bool,
    attempt_count: u64,
) -> StreamTrace {
    StreamTrace {
        chunk_count: None,
        bytes_received: None,
        max_chunk_gap_ms: None,
        last_chunk_at_ms: None,
        terminal_event: None,
        usage: None,
        stop_reason: None,
        websocket_trace: Some(WebSocketTrace {
            handshake_status: Some(handshake_status),
            bytes_sent: Some(bytes_sent),
            bytes_received: Some(bytes_received),
            client_message_count: Some(client_message_count),
            upstream_message_count: Some(upstream_message_count),
            // Keep the historical aggregate field for old UI versions: an
            // upstream close is more authoritative, otherwise use the client
            // close code.
            close_code: upstream_close_code.or(client_close_code),
            client_close_code,
            upstream_close_code,
            closed_by,
            relay_error,
            abnormal_close: Some(abnormal_close),
            attempt_count: Some(attempt_count),
        }),
    }
}

fn websocket_client_event(
    context: &WebSocketEventContext,
    endpoint: Option<&PlannedEndpoint>,
    status: i64,
    failover: bool,
    stream_trace: Option<StreamTrace>,
    failure: Option<&FailureInfo>,
) -> RuntimeEvent {
    let mut event = RuntimeEvent {
        client_kind: Some(context.client_kind),
        codex_metadata: context.codex_metadata.clone(),
        client_declared: context.client_declared.clone(),
        grok_metadata: context.grok_metadata.clone(),
        client_model: Some(context.model.clone()),
        source_format: Some(ProviderProtocol::OpenAI),
        target_format: endpoint.map(|endpoint| endpoint.protocol),
        route_mode: endpoint.map(|endpoint| endpoint.route_mode),
        duration_ms: context.started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        effective_model: Some(context.model.clone()),
        endpoint_id: endpoint.map(|endpoint| endpoint.endpoint_id.clone()),
        endpoint_name: endpoint.map(|endpoint| endpoint.endpoint_name.clone()),
        failover,
        feature_rule_id: None,
        failure_detail: None,
        failure_kind: None,
        failure_phase: None,
        id: context.request_id.clone(),
        kind: KIND_CLIENT.into(),
        message: None,
        tool_calls: None,
        outcome: Some(if (200..=399).contains(&status) {
            RuntimeEventOutcome::Succeeded
        } else {
            RuntimeEventOutcome::Failed
        }),
        phase: Some(RuntimeEventPhase::Completed),
        pool_id: None,
        request_purpose: Some(RequestPurpose::Standard),
        request_id: Some(context.request_id.clone()),
        request_method: Some("GET".into()),
        request_path: Some(context.request_path.clone()),
        route_intent: Some(context.route_intent.clone()),
        session_id: context.session_id.clone(),
        status_code: status,
        timestamp: unix_to_apple_epoch(now_unix()),
        ttfb_ms: None,
        stream_trace,
        timeout_ms: None,
        upstream_host: endpoint.and_then(|endpoint| host_of(&endpoint.base_url)),
        upstream_model: endpoint
            .map(|endpoint| endpoint.upstream_model.clone())
            .or_else(|| Some(context.model.clone())),
        upstream_request_id: None,
        upstream_status_code: (status > 0).then_some(status),
    };
    if let Some(failure) = failure {
        failure.apply_to(&mut event);
    }
    event
}

fn websocket_url(base_url: &str, path_and_query: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(base_url).ok()?;
    let scheme = match parsed.scheme() {
        "http" => "ws",
        "https" => "wss",
        "ws" | "wss" => parsed.scheme(),
        _ => return None,
    };
    let host = parsed.host_str()?;
    let port = parsed
        .port()
        .map(|port| format!(":{port}"))
        .unwrap_or_default();
    let path = request_build::openai_resource_path(parsed.path(), path_and_query);
    Some(format!("{scheme}://{host}{port}{path}"))
}

struct RelayState {
    upstream: BoxStream<'static, Result<Bytes, TransportError>>,
    bridge: Option<Box<dyn SseBridge>>,
    /// 入站 OpenAI 兼容层的客户端方言桥(Anthropic SSE → chat/Responses)。
    client_bridge: Option<Box<dyn SseBridge>>,
    /// 观察客户端实际收到的 SSE，不改变转发字节。
    terminal_tracker: Option<SseTerminalTracker>,
    /// 观察协议转换前的上游响应，保证 usage 是 Provider 实际返回值。
    upstream_summary_tracker: Option<SseTerminalTracker>,
    observe_sse: bool,
    upstream_observe_sse: bool,
    /// 严格协议边界：流式响应必须有 SSE 终止事件，严格非流式对话 JSON
    /// 必须有可识别的终态；EOF 只能作为其他旧路径的兼容兜底。
    strict_terminal: bool,
    /// Bounded response capture used only for Realtime client-secret
    /// registration; ordinary resource payloads remain byte-for-byte streamed.
    realtime_secret_body: Option<Vec<u8>>,
    realtime_secret_endpoint_id: Option<String>,
    realtime_secret_model: Option<String>,
    realtime_secret_session: Option<Value>,
    live_session_body: Option<Vec<u8>>,
    live_session_endpoint_id: Option<String>,
    live_session_model: Option<String>,
    video_session_body: Option<Vec<u8>>,
    video_session_endpoint_id: Option<String>,
    video_session_model: Option<String>,
    idle_timeout: Option<Duration>,
    guard: CompletionGuard,
    finished: bool,
}

impl RelayState {
    /// Inspect a newly received resource chunk as soon as it arrives.  Live
    /// and Video providers are not consistent about framing: some return one
    /// JSON object, others emit SSE/NDJSON and may split the id across many
    /// chunks.  The bounded capture buffers let us recognize all of those
    /// forms without changing the bytes sent to the caller.
    fn observe_resource_metadata(&mut self) {
        if let Some(body) = self.video_session_body.as_deref()
            && let Some(video_id) = video_id_from_payload(body)
            && let (Some(endpoint_id), Some(model)) = (
                self.video_session_endpoint_id.as_deref(),
                self.video_session_model.as_deref(),
            )
        {
            let endpoint_id = endpoint_id.to_string();
            let model = model.to_string();
            self.video_session_body = None;
            self.guard
                .engine
                .register_video_session(&video_id, &endpoint_id, &model);
        }
        if let Some(body) = self.live_session_body.as_deref()
            && let Some(call_id) = live_call_id_from_payload(body)
            && let (Some(endpoint_id), Some(model)) = (
                self.live_session_endpoint_id.as_deref(),
                self.live_session_model.as_deref(),
            )
        {
            let endpoint_id = endpoint_id.to_string();
            let model = model.to_string();
            self.live_session_body = None;
            self.guard
                .engine
                .register_live_session(&call_id, &endpoint_id, &model);
        }
    }

    /// Finalize bounded response metadata exactly once.
    ///
    /// Native resource responses can be delivered as a single chunk that
    /// already contains a protocol terminal event.  The relay marks itself
    /// finished before yielding that chunk, which means a later poll may not
    /// reach the ordinary EOF branch.  Taking each capture buffer here makes
    /// registration independent of chunking and keeps duplicate header/body
    /// observations harmless.
    fn finalize_response_metadata(&mut self) {
        if let Some(secret_body) = self.realtime_secret_body.take() {
            self.guard.engine.register_realtime_client_secret_from_body(
                &secret_body,
                self.realtime_secret_endpoint_id.as_deref(),
                self.realtime_secret_model.as_deref(),
                self.realtime_secret_session.take(),
            );
        }
        if let Some(video_body) = self.video_session_body.take()
            && let (Some(endpoint_id), Some(model)) = (
                self.video_session_endpoint_id.as_deref(),
                self.video_session_model.as_deref(),
            )
            && let Some(video_id) = video_id_from_payload(&video_body)
        {
            self.guard
                .engine
                .register_video_session(&video_id, endpoint_id, model);
        }
        if let Some(live_body) = self.live_session_body.take()
            && let (Some(endpoint_id), Some(model)) = (
                self.live_session_endpoint_id.as_deref(),
                self.live_session_model.as_deref(),
            )
            && let Some(call_id) = live_call_id_from_payload(&live_body)
        {
            self.guard
                .engine
                .register_live_session(&call_id, endpoint_id, model);
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum StreamReadError {
    #[error("stream idle timeout")]
    IdleTimeout(Duration),
    #[error("{0}")]
    Upstream(TransportError),
    #[error("missing protocol terminal event")]
    MissingTerminal,
}

async fn read_next(state: &mut RelayState) -> Result<Option<Bytes>, StreamReadError> {
    let next = state.upstream.next();
    match state.idle_timeout {
        Some(deadline) => match tokio::time::timeout(deadline, next).await {
            Ok(item) => item.transpose().map_err(StreamReadError::Upstream),
            Err(_) => Err(StreamReadError::IdleTimeout(deadline)),
        },
        None => next.await.transpose().map_err(StreamReadError::Upstream),
    }
}

/// 首响应截止 = min(全局 responseTimeout, 映射级 failoverTimeout);皆 None 则不限。
fn effective_response_timeout(retry: &RetryPolicy, endpoint: &PlannedEndpoint) -> Option<f64> {
    match (
        retry.response_timeout_seconds,
        endpoint.failover_timeout_seconds,
    ) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Resolve the response-header deadline for one outbound request.  Ordinary
/// requests preserve the configured `null` semantics; native Realtime/Live
/// requests always receive a bounded fallback so a provider that accepts the
/// TCP connection but never emits headers cannot strand the client request.
fn effective_response_timeout_for_request(
    retry: &RetryPolicy,
    endpoint: &PlannedEndpoint,
    realtime_request: bool,
) -> Option<f64> {
    let configured = effective_response_timeout(retry, endpoint);
    if !realtime_request {
        return configured;
    }
    Some(
        configured
            .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
            .unwrap_or(REALTIME_RESPONSE_TIMEOUT_SECS),
    )
}

/// Realtime/Live HTTP bootstraps are short setup exchanges, not long-lived
/// token streams.  Give their response body the same bounded idle guard when
/// the global stream setting is `null`; otherwise a provider that sends 200
/// headers and then stalls would still leave the client waiting forever.
fn effective_stream_idle_timeout_for_request(
    retry: &RetryPolicy,
    realtime_request: bool,
) -> Option<f64> {
    if realtime_request {
        Some(
            retry
                .stream_idle_timeout_seconds
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .unwrap_or(REALTIME_RESPONSE_TIMEOUT_SECS),
        )
    } else {
        retry.stream_idle_timeout_seconds
    }
}

/// Enforce the response-header deadline at the engine boundary as well as in
/// the concrete transport.  The built-in reqwest transport already honors the
/// argument, but keeping this guard here makes the contract robust for replay
/// and platform test transports too: a provider future that ignores the hint
/// still cannot keep a Live request in flight forever.
async fn send_streaming_with_deadline(
    transport: &Arc<dyn UpstreamTransport>,
    request: crate::outbound::OutboundRequest,
    timeout_seconds: Option<f64>,
) -> Result<crate::outbound::UpstreamResponse, TransportError> {
    let timeout = timeout_seconds.and_then(|seconds| {
        (seconds.is_finite() && seconds >= 0.0).then(|| Duration::from_secs_f64(seconds))
    });
    let send = transport.send_streaming(request, timeout);
    match timeout {
        Some(deadline) => tokio::time::timeout(deadline, send)
            .await
            .map_err(|_| TransportError::Timeout)?,
        None => send.await,
    }
}

/// 入站 OpenAI 兼容层的客户端出口形态。
#[derive(Clone)]
struct ClientOut {
    /// Legacy bridge dialect marker. The public data plane always uses Raw,
    /// leaving protocol interpretation to the upstream Provider.
    dialect: Option<ClientDialect>,
    passthrough_kind: PassthroughKind,
    stream: bool,
    /// Some = 出站原样打上游；None = legacy internal bridge path.
    passthrough: Option<Bytes>,
    content_type: Option<String>,
    terminal_dialect: SseDialect,
    /// Capture only bounded JSON responses of Realtime secret/session routes
    /// so returned `ek_…` credentials can authorize following calls/upgrades.
    realtime_client_secret: bool,
}

/// Legacy internal callers can request a native protocol. Raw data-plane
/// requests do not use this filter and are always relayed to the selected
/// Provider, regardless of its protocol label.
fn source_format_for_passthrough(kind: PassthroughKind) -> ProviderProtocol {
    match kind {
        PassthroughKind::ClaudeCountTokens => ProviderProtocol::Anthropic,
        PassthroughKind::Responses
        | PassthroughKind::ResponsesCompact
        | PassthroughKind::AlphaSearch => ProviderProtocol::OpenAIResponses,
        PassthroughKind::Chat
        | PassthroughKind::Completions
        | PassthroughKind::ImagesGenerations
        | PassthroughKind::ImagesEdits
        | PassthroughKind::Files
        | PassthroughKind::Videos
        | PassthroughKind::Realtime
        | PassthroughKind::Models
        | PassthroughKind::Raw => ProviderProtocol::OpenAI,
    }
}

fn required_native_protocol(kind: PassthroughKind) -> Option<ProviderProtocol> {
    match kind {
        PassthroughKind::Raw => None,
        PassthroughKind::ClaudeCountTokens => Some(ProviderProtocol::Anthropic),
        PassthroughKind::Completions => Some(ProviderProtocol::OpenAI),
        PassthroughKind::ResponsesCompact | PassthroughKind::AlphaSearch => {
            Some(ProviderProtocol::OpenAIResponses)
        }
        // Resource and Realtime paths are opaque native requests. Any fixed
        // endpoint protocol may receive them; the upstream owns that
        // endpoint contract and the proxy must not reject it by label.
        PassthroughKind::Files
        | PassthroughKind::Videos
        | PassthroughKind::Realtime
        | PassthroughKind::Models => None,
        PassthroughKind::ImagesGenerations | PassthroughKind::ImagesEdits => None,
        PassthroughKind::Chat | PassthroughKind::Responses => None,
    }
}

fn translation_supported(
    source_format: ProviderProtocol,
    endpoint: &PlannedEndpoint,
    request: &RoutingRequest,
    inbound_body: &[u8],
    purpose: RequestPurpose,
) -> Result<(), bridge::TranslationError> {
    if endpoint.route_mode == RouteMode::Native {
        return Ok(());
    }
    match source_format {
        ProviderProtocol::Anthropic => bridge::check_anthropic_translation(
            request,
            endpoint.protocol,
            request_build::server_retrieval_enabled(endpoint, request, purpose),
        ),
        ProviderProtocol::OpenAI | ProviderProtocol::OpenAIResponses => {
            let body = serde_json::from_slice::<Value>(inbound_body)
                .map_err(|_| bridge::TranslationError::InvalidInput("body is not JSON".into()))?;
            match source_format {
                ProviderProtocol::OpenAI => bridge_in::check_chat_to_anthropic(&body),
                ProviderProtocol::OpenAIResponses => bridge_in::check_responses_to_anthropic(&body),
                ProviderProtocol::Anthropic => unreachable!(),
            }?;
            bridge::check_anthropic_translation(
                request,
                endpoint.protocol,
                request_build::server_retrieval_enabled(endpoint, request, purpose),
            )
        }
    }
}

struct ClientMeta {
    client_kind: ClientKind,
    source_format: ProviderProtocol,
    target_format: Option<ProviderProtocol>,
    route_mode: Option<RouteMode>,
    client_model: String,
    effective_model: String,
    feature_rule_id: Option<String>,
    purpose: RequestPurpose,
    /// 形似 CC 内部辅助请求但未命中任何指纹(词表 `unmatched_no_tools`)。
    unmatched_no_tools: bool,
    /// 有界客户端会话标识，用于跨客户端的会话统计筛选。
    session_id: Option<String>,
    codex_metadata: Option<CodexMetadata>,
    /// 客户端用 `X-Sumpter-*` 声明的项目归因；可信度低于 codex_metadata。
    client_declared: Option<ClientDeclaredMetadata>,
    grok_metadata: Option<GrokMetadata>,
    /// Bounded inbound request identity retained for stream/WS completion,
    /// which may be polled after the original HTTP task-local scope ends.
    request_context: Option<InboundRequestContext>,
}

/// 一轮 failover 遍历的聚合状态。
struct RoundState {
    retryable_failures: u64,
    /// 除 HTTP 500 外触发 `sessionStickyRetries` 的故障数。
    sticky_retryable_failures: u64,
    /// 当前轮上游返回的数字 Retry-After，取最大值并与指数退避共同生效。
    retry_after_seconds: Option<f64>,
    /// 最后一次真实失败决定最终 API 反馈；不能让较早的 HTTP 502 覆盖随后发生的
    /// DNS/TCP/TLS 失败，也不能把本地传输错误冒充成“上游返回 502”。
    last_failure: Option<FailureInfo>,
}

impl RoundState {
    fn note_retry_after(&mut self, headers: &[(String, String)]) {
        let Some(seconds) = retry_after_seconds(headers) else {
            return;
        };
        self.retry_after_seconds = Some(
            self.retry_after_seconds
                .map_or(seconds, |current| current.max(seconds)),
        );
    }
}

fn retry_after_seconds(headers: &[(String, String)]) -> Option<f64> {
    retry_after_seconds_at(headers, now_unix())
}

/// Parse both RFC 7231 delta-seconds and HTTP-date forms. Values are bounded
/// before they enter retry sleeps or client responses, so a malicious upstream
/// cannot stall the sidecar for hours. The `now` parameter keeps unit tests
/// deterministic and makes the date-vs-delta distinction explicit.
fn retry_after_seconds_at(headers: &[(String, String)], now: f64) -> Option<f64> {
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .filter_map(|(_, value)| {
            let value = value.trim();
            let seconds = value
                .parse::<f64>()
                .ok()
                .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
                .or_else(|| {
                    let timestamp = httpdate::parse_http_date(value)
                        .ok()?
                        .duration_since(UNIX_EPOCH)
                        .ok()?
                        .as_secs_f64();
                    Some((timestamp - now).max(0.0))
                })?;
            Some(seconds.min(MAX_RETRY_AFTER_SECS))
        })
        .reduce(f64::max)
}

fn retry_backoff_delay(round: i64, retry_after_seconds: Option<f64>) -> Duration {
    let exponent = (round - 1).clamp(0, 64) as i32;
    let exponential = (RETRY_BACKOFF_INITIAL_SECS * RETRY_BACKOFF_FACTOR.powi(exponent))
        .min(RETRY_BACKOFF_MAX_SECS);
    let retry_after = retry_after_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or(0.0)
        .min(RETRY_BACKOFF_MAX_SECS);
    Duration::from_secs_f64(exponential.max(retry_after))
}

#[derive(Clone)]
struct FailureInfo {
    kind: RuntimeFailureKind,
    phase: RuntimeFailurePhase,
    detail: Option<String>,
    timeout_ms: Option<i64>,
    upstream_status_code: Option<i64>,
    upstream_request_id: Option<String>,
    retry_after_seconds: Option<f64>,
}

impl FailureInfo {
    fn from_transport(error: &TransportError, response_timeout: Option<f64>) -> Self {
        let (kind, detail, timeout_ms) = match error {
            TransportError::Timeout => (
                RuntimeFailureKind::ResponseTimeout,
                Some(
                    "upstream response headers were not received before the effective deadline"
                        .into(),
                ),
                response_timeout.map(timeout_ms),
            ),
            TransportError::ConnectionFailed(detail) => (
                RuntimeFailureKind::ConnectionFailed,
                Some(detail.clone()),
                None,
            ),
            TransportError::InvalidResponse(detail) => (
                RuntimeFailureKind::InvalidResponse,
                Some(detail.clone()),
                None,
            ),
        };
        Self {
            kind,
            phase: RuntimeFailurePhase::BeforeResponse,
            detail,
            timeout_ms,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    fn upstream_http(status: u16, request_id: Option<String>) -> Self {
        Self {
            kind: RuntimeFailureKind::UpstreamHttpStatus,
            phase: RuntimeFailurePhase::ResponseHeaders,
            detail: Some(format!("upstream returned HTTP {status}")),
            timeout_ms: None,
            upstream_status_code: Some(status as i64),
            upstream_request_id: request_id,
            retry_after_seconds: None,
        }
    }

    fn endpoints_exhausted() -> Self {
        Self {
            kind: RuntimeFailureKind::EndpointsExhausted,
            phase: RuntimeFailurePhase::BeforeResponse,
            detail: Some("no eligible upstream endpoint produced a response".into()),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    fn provider_cooldown(retry_after_seconds: f64) -> Self {
        Self {
            kind: RuntimeFailureKind::EndpointsExhausted,
            phase: RuntimeFailurePhase::BeforeResponse,
            detail: Some("all compatible providers are cooling down".into()),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: Some(retry_after_seconds),
        }
    }

    fn from_stream(error: &StreamReadError) -> Self {
        match error {
            StreamReadError::IdleTimeout(deadline) => Self {
                kind: RuntimeFailureKind::StreamIdleTimeout,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(
                    "upstream response stream exceeded the configured idle deadline".into(),
                ),
                timeout_ms: Some(deadline.as_millis().min(i64::MAX as u128) as i64),
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
            StreamReadError::Upstream(error) => Self {
                kind: RuntimeFailureKind::StreamInterrupted,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(error.to_string()),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
            StreamReadError::MissingTerminal => Self {
                kind: RuntimeFailureKind::StreamInterrupted,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(
                    "upstream response stream ended before a protocol terminal event".into(),
                ),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
        }
    }

    fn from_protocol_terminal(terminal: SseTerminal) -> Option<Self> {
        let (kind, detail) = match terminal {
            SseTerminal::Completed => return None,
            SseTerminal::Incomplete { detail } => {
                (RuntimeFailureKind::UpstreamResponseIncomplete, detail)
            }
            SseTerminal::Failed { detail } => (RuntimeFailureKind::UpstreamResponseFailed, detail),
        };
        Some(Self {
            kind,
            phase: RuntimeFailurePhase::ResponseStream,
            detail: Some(detail),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        })
    }

    fn client_cancelled(response_started: bool) -> Self {
        Self {
            kind: RuntimeFailureKind::ClientCancelled,
            phase: if response_started {
                RuntimeFailurePhase::ResponseStream
            } else {
                RuntimeFailurePhase::BeforeResponse
            },
            detail: Some("client disconnected or cancelled the request".into()),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    fn apply_to(&self, event: &mut RuntimeEvent) {
        event.outcome = Some(RuntimeEventOutcome::Failed);
        event.failure_kind = Some(self.kind);
        event.failure_phase = Some(self.phase);
        event.failure_detail = self.detail.clone();
        event.timeout_ms = self.timeout_ms;
        event.upstream_status_code = self.upstream_status_code;
        event.upstream_request_id = self.upstream_request_id.clone();
    }
}

fn timeout_ms(seconds: f64) -> i64 {
    Duration::from_secs_f64(seconds)
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn protocol_token(protocol: ProviderProtocol) -> &'static str {
    match protocol {
        ProviderProtocol::Anthropic => "anthropic",
        ProviderProtocol::OpenAI => "openai",
        ProviderProtocol::OpenAIResponses => "openai-responses",
    }
}

/// accepted 后挂在 guard 上的上游归属(收尾事件与消息 token 的数据源)。
#[derive(Clone)]
struct UpstreamAttempt {
    event_id: String,
    endpoint: PlannedEndpoint,
    started: Instant,
    /// 该次尝试自身的响应头延迟(ms):`started` → accepted。
    ttfb_ms: i64,
    is_failover: bool,
    pinned_ip: Option<String>,
    upstream_request_id: Option<String>,
    retry_after_seconds: Option<f64>,
    capture_attempt_id: String,
}

#[derive(Default)]
struct StreamTraceState {
    started: Option<Instant>,
    last_chunk_at: Option<Instant>,
    chunk_count: u64,
    bytes_received: u64,
    max_chunk_gap_ms: i64,
    terminal_event: Option<String>,
    usage: Option<sumpter_core::events::ResponseUsage>,
    stop_reason: Option<String>,
}

impl StreamTraceState {
    fn start(&mut self) {
        self.started.get_or_insert_with(Instant::now);
    }

    fn record_chunk(&mut self, bytes: usize) {
        let now = Instant::now();
        self.started.get_or_insert(now);
        if let Some(last) = self.last_chunk_at {
            self.max_chunk_gap_ms = self
                .max_chunk_gap_ms
                .max(now.duration_since(last).as_millis().min(i64::MAX as u128) as i64);
        }
        self.last_chunk_at = Some(now);
        self.chunk_count = self.chunk_count.saturating_add(1);
        self.bytes_received = self.bytes_received.saturating_add(bytes as u64);
    }

    fn record_terminal(&mut self, terminal: &SseTerminal) {
        self.terminal_event = Some(
            match terminal {
                SseTerminal::Completed => "completed",
                SseTerminal::Incomplete { .. } => "incomplete",
                SseTerminal::Failed { .. } => "failed",
            }
            .to_string(),
        );
    }

    fn record_response_summary(&mut self, tracker: &SseTerminalTracker) {
        self.usage = tracker.usage();
        self.stop_reason = tracker.stop_reason().map(str::to_string);
    }

    fn snapshot(&self) -> Option<StreamTrace> {
        let started = self.started?;
        Some(StreamTrace {
            chunk_count: Some(self.chunk_count),
            bytes_received: Some(self.bytes_received),
            max_chunk_gap_ms: (self.chunk_count > 1).then_some(self.max_chunk_gap_ms),
            last_chunk_at_ms: self.last_chunk_at.map(|last| {
                last.duration_since(started)
                    .as_millis()
                    .min(i64::MAX as u128) as i64
            }),
            terminal_event: self.terminal_event.clone(),
            usage: self.usage.clone(),
            stop_reason: self.stop_reason.clone(),
            websocket_trace: None,
        })
    }
}

/// 一次客户端请求的完成守卫:
/// - 正常完成 → 按最终状态记 client + upstream 事件并计数;
/// - 流中断 → 记中断信息;
/// - **Drop 而未完成 = 客户端断开** → client 记 499；upstream 保留真实 HTTP 状态，
///   两者 outcome 都是 cancelled，不计成功也不计失败。
struct CompletionGuard {
    engine: Engine,
    client_event_id: String,
    started: Instant,
    /// client in-flight 事件的原始 timestamp(apple 纪元秒):accepted 回填时原样保留。
    started_timestamp: f64,
    meta: ClientMeta,
    failover: bool,
    status: i64,
    /// forward 循环当前轮数(>1 = 经历过可重试故障跨轮)。
    rounds: i64,
    /// 入站原生透传生效时的端点类别(未经任何桥接)。
    passthrough: Option<PassthroughKind>,
    /// 客户端视角的首字节耗时(ms):请求进来 → 某个入口 accepted。
    /// 含 failover、退避与跨轮重跑的全部等待,所以与 upstream 事件的同名字段口径不同。
    /// None = 全轮耗尽也没 accepted。
    ttfb_ms: Option<i64>,
    tool_calls: Vec<String>,
    streaming_message: Option<String>,
    stream_trace: StreamTraceState,
    /// 响应头前失败/取消时，保留最后一个进入调度判断的真实入口。
    attempted_endpoint: Option<PlannedEndpoint>,
    upstream: Option<UpstreamAttempt>,
    done: bool,
}

impl CompletionGuard {
    fn new(
        engine: Engine,
        client_event_id: String,
        started: Instant,
        started_timestamp: f64,
        meta: ClientMeta,
    ) -> Self {
        Self {
            engine,
            client_event_id,
            started,
            started_timestamp,
            meta,
            failover: false,
            status: 0,
            rounds: 1,
            passthrough: None,
            ttfb_ms: None,
            tool_calls: Vec::new(),
            streaming_message: None,
            stream_trace: StreamTraceState {
                started: Some(started),
                ..StreamTraceState::default()
            },
            attempted_endpoint: None,
            upstream: None,
            done: false,
        }
    }

    fn mark_failover(&mut self) {
        self.failover = true;
    }

    fn set_passthrough(&mut self, kind: Option<PassthroughKind>) {
        self.passthrough = kind;
    }

    fn request_id(&self) -> &str {
        &self.client_event_id
    }

    fn capture_attempt_id(&self) -> Option<&str> {
        self.upstream
            .as_ref()
            .map(|attempt| attempt.capture_attempt_id.as_str())
    }

    fn set_status(&mut self, status: i64) {
        self.status = status;
    }

    fn set_round(&mut self, round: i64) {
        self.rounds = round;
    }

    fn note_endpoint(&mut self, endpoint: &PlannedEndpoint) {
        self.attempted_endpoint = Some(endpoint.clone());
    }

    fn attach_upstream(&mut self, attempt: UpstreamAttempt) {
        self.upstream = Some(attempt);
    }

    /// accepted 后原地回填 client in-flight 事件的入口归属(upsert 不计数):
    /// 流式期间事件表的客户端行直接可见走的哪个入口,不用切「上游」筛选比对。
    /// timestamp 保持请求开始时刻;status 保留已收到的真实上游 HTTP 状态。
    /// 尚未收到响应头时仍为 0,只有这种进行中事件才表示“未收到响应头”。
    ///
    /// 同时钉住客户端视角的首字节耗时:此刻正是响应头到达。流式请求的 durationMS 是
    /// 「吐完最后一个字」,单看它分不清「上游卡住」和「正常长输出」,TTFB 才是那把尺。
    fn record_streaming_started(&mut self, message: Option<String>) {
        self.streaming_message = message.clone();
        if self.ttfb_ms.is_none() {
            self.ttfb_ms = Some(self.started.elapsed().as_millis() as i64);
        }
        self.stream_trace.start();
        // client_event 使用当前真实状态构造入口/上游字段;随后覆盖生命周期字段,
        // 因为 statusCode 与 phase 是两个独立维度:HTTP 200 可以仍处于 in-flight。
        let mut event = self.client_event(self.status, message);
        event.outcome = None;
        event.phase = Some(RuntimeEventPhase::InFlight);
        event.timestamp = self.started_timestamp;
        self.engine.record_event(event);
    }

    fn set_tool_calls(&mut self, calls: &[String]) {
        if calls == self.tool_calls.as_slice() {
            return;
        }
        self.tool_calls = calls.to_vec();
        if self.upstream.is_some() && !self.done {
            self.record_streaming_started(self.streaming_message.clone());
        }
    }

    fn tool_calls_option(&self) -> Option<Vec<String>> {
        (!self.tool_calls.is_empty()).then(|| self.tool_calls.clone())
    }

    fn record_stream_chunk(&mut self, bytes: usize) {
        self.stream_trace.record_chunk(bytes);
    }

    fn record_stream_terminal(&mut self, terminal: &SseTerminal) {
        self.stream_trace.record_terminal(terminal);
    }

    /// 收尾消息的信息 token(词表 §5.1):
    /// `[pinned, bridge, deferred_rounds, unmatched_no_tools]`。
    /// bridge 仅在桥接真实生效(非 anthropic 协议且 accepted 2xx)时产出;
    /// unmatched_no_tools 只描述请求本身,不进 upstream 事件。
    fn attempt_tokens(&self) -> [Option<String>; 4] {
        let pinned = self
            .upstream
            .as_ref()
            .and_then(|u| u.pinned_ip.as_deref().map(message_tokens::pinned));
        let bridged = match self.passthrough {
            Some(kind) => Some(format!(
                "{}{}",
                message_tokens::PASSTHROUGH_PREFIX,
                kind.token()
            )),
            None => self.upstream.as_ref().and_then(|u| {
                (u.endpoint.protocol != ProviderProtocol::Anthropic
                    && (200..=399).contains(&self.status))
                .then(|| {
                    format!(
                        "{}{}",
                        message_tokens::BRIDGE_PREFIX,
                        protocol_token(u.endpoint.protocol)
                    )
                })
            }),
        };
        let rounds = (self.rounds > 1)
            .then(|| format!("{}{}", message_tokens::DEFERRED_ROUNDS_PREFIX, self.rounds));
        let unmatched = self
            .meta
            .unmatched_no_tools
            .then(|| message_tokens::UNMATCHED_NO_TOOLS.to_string());
        [pinned, bridged, rounds, unmatched]
    }

    fn client_event(&self, status: i64, message: Option<String>) -> RuntimeEvent {
        let upstream = self.upstream.as_ref();
        let endpoint = upstream
            .map(|attempt| &attempt.endpoint)
            .or(self.attempted_endpoint.as_ref());
        RuntimeEvent {
            client_kind: Some(self.meta.client_kind),
            client_model: Some(self.meta.client_model.clone()),
            source_format: Some(self.meta.source_format),
            target_format: endpoint
                .map(|endpoint| endpoint.protocol)
                .or(self.meta.target_format),
            route_mode: endpoint
                .map(|endpoint| endpoint.route_mode)
                .or(self.meta.route_mode),
            duration_ms: self.started.elapsed().as_millis() as i64,
            effective_model: Some(self.meta.effective_model.clone()),
            endpoint_id: endpoint.map(|endpoint| endpoint.endpoint_id.clone()),
            endpoint_name: endpoint.map(|endpoint| endpoint.endpoint_name.clone()),
            failover: self.failover,
            feature_rule_id: self.meta.feature_rule_id.clone(),
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: self.client_event_id.clone(),
            kind: KIND_CLIENT.into(),
            message,
            tool_calls: self.tool_calls_option(),
            codex_metadata: self.meta.codex_metadata.clone(),
            client_declared: self.meta.client_declared.clone(),
            grok_metadata: self.meta.grok_metadata.clone(),
            outcome: if status == 0 {
                None
            } else if status == STATUS_CLIENT_DISCONNECTED {
                Some(RuntimeEventOutcome::Cancelled)
            } else if (200..=399).contains(&status) {
                Some(RuntimeEventOutcome::Succeeded)
            } else {
                Some(RuntimeEventOutcome::Failed)
            },
            phase: Some(if status == 0 {
                RuntimeEventPhase::InFlight
            } else {
                RuntimeEventPhase::Completed
            }),
            pool_id: None,
            request_purpose: Some(self.meta.purpose),
            request_id: Some(self.client_event_id.clone()),
            request_method: self
                .meta
                .request_context
                .as_ref()
                .map(|context| context.method.clone()),
            request_path: self
                .meta
                .request_context
                .as_ref()
                .map(|context| context.path.clone()),
            route_intent: self
                .meta
                .request_context
                .as_ref()
                .map(|context| context.route_intent.clone()),
            session_id: self.meta.session_id.clone(),
            status_code: status,
            timestamp: unix_to_apple_epoch(now_unix()),
            ttfb_ms: self.ttfb_ms,
            stream_trace: self.stream_trace.snapshot(),
            timeout_ms: None,
            upstream_host: endpoint.and_then(|endpoint| host_of(&endpoint.base_url)),
            upstream_model: endpoint
                .map(|endpoint| endpoint.upstream_model.clone())
                .or_else(|| Some(self.meta.effective_model.clone())),
            upstream_request_id: upstream.and_then(|u| u.upstream_request_id.clone()),
            upstream_status_code: upstream.and_then(|_| (self.status > 0).then_some(self.status)),
        }
    }

    fn complete_upstream_event(
        &mut self,
        status: i64,
        message: Option<String>,
        outcome: RuntimeEventOutcome,
        failure: Option<&FailureInfo>,
    ) {
        let Some(attempt) = self.upstream.clone() else {
            return;
        };
        let mut event = self.engine.upstream_event(
            &attempt.endpoint,
            status,
            attempt.started.elapsed().as_millis() as i64,
            attempt.is_failover,
            message,
            self.meta.purpose,
            self.meta.client_kind,
            &self.client_event_id,
        );
        event.id = attempt.event_id;
        event.phase = Some(RuntimeEventPhase::Completed);
        event.ttfb_ms = Some(attempt.ttfb_ms);
        if let Some(failure) = failure {
            failure.apply_to(&mut event);
        }
        event.client_model = Some(self.meta.client_model.clone());
        event.effective_model = Some(self.meta.effective_model.clone());
        event.feature_rule_id = self.meta.feature_rule_id.clone();
        if let Some(context) = &self.meta.request_context {
            event.request_method = Some(context.method.clone());
            event.request_path = Some(context.path.clone());
            event.route_intent = Some(context.route_intent.clone());
        }
        event.session_id = self.meta.session_id.clone();
        event.tool_calls = self.tool_calls_option();
        event.codex_metadata = self.meta.codex_metadata.clone();
        event.client_declared = self.meta.client_declared.clone();
        event.grok_metadata = self.meta.grok_metadata.clone();
        event.stream_trace = self.stream_trace.snapshot();
        event.outcome = Some(outcome);
        event.upstream_status_code = (self.status > 0).then_some(self.status);
        event.upstream_request_id = attempt.upstream_request_id;
        self.engine.complete_upstream(event);
    }

    /// 无上游(规划/鉴权阶段失败)或统一收尾。
    fn complete(&mut self, status: i64, message: Option<String>, failure: FailureInfo) {
        if self.done {
            return;
        }
        self.done = true;
        let mut event = self.client_event(status, message.clone());
        failure.apply_to(&mut event);
        let failed_message = message.or_else(|| failure.detail.clone());
        self.engine.complete_client(event, failed_message);
    }

    /// 流生命周期收尾:None = 正常流尽;Some(err) = 上游中断/idle 超时。
    /// 先构造 client 事件(还需要 upstream 的入口信息)再完成 upstream 事件。
    fn complete_from_stream(&mut self, error: Option<StreamReadError>) {
        if self.done {
            return;
        }
        self.done = true;
        let [pinned, bridged, rounds, unmatched] = self.attempt_tokens();
        match error {
            None => {
                let client_message =
                    message_tokens::join(&[pinned.clone(), bridged.clone(), rounds, unmatched]);
                let upstream_message = message_tokens::join(&[pinned, bridged]);
                let mut event = self.client_event(self.status, client_message);
                let failure = (!(200..=399).contains(&self.status)).then(|| {
                    let mut failure = FailureInfo::upstream_http(
                        self.status as u16,
                        self.upstream
                            .as_ref()
                            .and_then(|attempt| attempt.upstream_request_id.clone()),
                    );
                    failure.retry_after_seconds = self
                        .upstream
                        .as_ref()
                        .and_then(|attempt| attempt.retry_after_seconds);
                    failure
                });
                if let Some(failure) = &failure {
                    failure.apply_to(&mut event);
                }
                let outcome = if failure.is_some() {
                    RuntimeEventOutcome::Failed
                } else {
                    RuntimeEventOutcome::Succeeded
                };
                self.complete_upstream_event(
                    self.status,
                    upstream_message,
                    outcome,
                    failure.as_ref(),
                );
                self.engine
                    .complete_client(event, failure.and_then(|failure| failure.detail));
            }
            Some(err) => {
                let interrupted = format!("{}{err}", message_tokens::STREAM_INTERRUPTED_PREFIX);
                let client_message = message_tokens::join(&[
                    pinned.clone(),
                    bridged.clone(),
                    Some(interrupted.clone()),
                    unmatched,
                ])
                .unwrap_or_else(|| interrupted.clone());
                // 已回写响应头；保留真实 HTTP 状态，最终失败由 outcome/failureKind 表达。
                let mut failure = FailureInfo::from_stream(&err);
                failure.upstream_status_code = (self.status > 0).then_some(self.status);
                failure.upstream_request_id = self
                    .upstream
                    .as_ref()
                    .and_then(|attempt| attempt.upstream_request_id.clone());
                failure.retry_after_seconds = self
                    .upstream
                    .as_ref()
                    .and_then(|attempt| attempt.retry_after_seconds);
                let mut event = self.client_event(self.status, Some(client_message.clone()));
                failure.apply_to(&mut event);
                let upstream_message = message_tokens::join(&[pinned, bridged, Some(interrupted)]);
                self.complete_upstream_event(
                    self.status,
                    upstream_message,
                    RuntimeEventOutcome::Failed,
                    Some(&failure),
                );
                self.engine.complete_client(event, Some(client_message));
            }
        }
    }

    /// 客户端可见协议已经明确结束。`completed` 与正常 EOF 共用成功收尾；
    /// `incomplete` / `failed` 保留 HTTP 200，但以结构化最终失败进入统计。
    fn complete_from_protocol_terminal(&mut self, terminal: SseTerminal) {
        let Some(mut failure) = FailureInfo::from_protocol_terminal(terminal) else {
            self.complete_from_stream(None);
            return;
        };
        if self.done {
            return;
        }
        self.done = true;
        failure.upstream_status_code = (self.status > 0).then_some(self.status);
        failure.upstream_request_id = self
            .upstream
            .as_ref()
            .and_then(|attempt| attempt.upstream_request_id.clone());
        failure.retry_after_seconds = self
            .upstream
            .as_ref()
            .and_then(|attempt| attempt.retry_after_seconds);

        let [pinned, bridged, rounds, unmatched] = self.attempt_tokens();
        let client_message =
            message_tokens::join(&[pinned.clone(), bridged.clone(), rounds, unmatched]);
        let upstream_message = message_tokens::join(&[pinned, bridged]);
        let mut event = self.client_event(self.status, client_message);
        failure.apply_to(&mut event);
        self.complete_upstream_event(
            self.status,
            upstream_message,
            RuntimeEventOutcome::Failed,
            Some(&failure),
        );
        self.engine.complete_client(event, failure.detail.clone());
    }
}

impl Drop for CompletionGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        // 客户端断开:499,不计成功也不计失败;上游连接随 relay drop 已撕掉。
        let failure = FailureInfo::client_cancelled(self.upstream.is_some());
        let mut event = self.client_event(
            STATUS_CLIENT_DISCONNECTED,
            Some("client_disconnected: request cancelled".into()),
        );
        failure.apply_to(&mut event);
        event.outcome = Some(RuntimeEventOutcome::Cancelled);
        self.complete_upstream_event(
            self.status,
            Some("client_disconnected".into()),
            RuntimeEventOutcome::Cancelled,
            Some(&failure),
        );
        self.engine.complete_client(event, None);
    }
}

fn host_of(base_url: &str) -> Option<String> {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
}

/// 仅采纳明确的上游追踪头；不扫描任意响应头，避免把 Cookie/鉴权信息带入事件。
fn upstream_request_id(headers: &[(String, String)]) -> Option<String> {
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

fn inbound_auth_ok(headers: &[(String, String)], token: &str) -> bool {
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("x-api-key") && value == token {
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

fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .rev()
        .find(|(header, value)| header.eq_ignore_ascii_case(name) && !value.trim().is_empty())
        .map(|(_, value)| value.as_str())
}

fn detect_client_kind(headers: &[(String, String)], openai_inbound: bool) -> ClientKind {
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

fn bounded_request_method(method: &str) -> String {
    let value = method.trim();
    let value = if value.is_empty() { "UNKNOWN" } else { value };
    value
        .chars()
        .take(16)
        .collect::<String>()
        .to_ascii_uppercase()
}

fn bounded_request_path(path: &str) -> String {
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

fn bounded_failure_detail(detail: &str) -> String {
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

fn current_request_context() -> Option<InboundRequestContext> {
    INBOUND_REQUEST_CONTEXT
        .try_with(|context| context.borrow().clone())
        .ok()
}

fn set_current_route_intent(intent: &str) {
    let _ = INBOUND_REQUEST_CONTEXT.try_with(|context| {
        context.borrow_mut().route_intent = intent.to_string();
    });
}

fn apply_current_request_context(event: &mut RuntimeEvent) {
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

/// Stable high-level route intent used in runtime diagnostics.  This is kept
/// deliberately independent from `RequestPurpose`: a normal Realtime call
/// and a Codex Live call have the same business purpose but different provider
/// capability requirements.
fn route_intent_for_path(
    method: &str,
    path_and_query: &str,
    query_model: Option<&str>,
    client_kind: ClientKind,
) -> &'static str {
    let path = path_without_query(path_and_query);
    if path == "/v1/messages/count_tokens" || path == "/messages/count_tokens" {
        return "token_count";
    }
    if is_realtime_http_path(path) {
        return if classify_realtime_intent(
            method,
            path_and_query,
            query_model,
            None,
            None,
            client_kind,
            false,
        ) == RealtimeRouteIntent::CodexLive
        {
            "live"
        } else {
            "realtime"
        };
    }
    if is_openai_resource_tree(path, "videos") {
        return "videos";
    }
    if is_openai_resource_tree(path, "files") {
        return "files";
    }
    if is_openai_resource_tree(path, "models") {
        return "models";
    }
    if matches!(
        path,
        "/v1/images/generations" | "/images/generations" | "/backend-api/codex/images/generations"
    ) {
        return "image";
    }
    if matches!(
        path,
        "/v1/images/edits" | "/images/edits" | "/backend-api/codex/images/edits"
    ) {
        return "image_edit";
    }
    if matches!(
        path,
        "/v1/responses"
            | "/responses"
            | "/backend-api/codex/responses"
            | "/v1/responses/compact"
            | "/responses/compact"
            | "/backend-api/codex/responses/compact"
    ) {
        return "responses";
    }
    if matches!(path, "/v1/chat/completions" | "/chat/completions") {
        return "chat";
    }
    if matches!(path, "/v1/completions" | "/completions") {
        return "completions";
    }
    if matches!(
        path,
        "/v1/alpha/search" | "/alpha/search" | "/backend-api/codex/alpha/search"
    ) {
        return "alpha_search";
    }
    if matches!(path, "/v1/messages" | "/messages") {
        return "messages";
    }
    if method.eq_ignore_ascii_case("OPTIONS") {
        return "options";
    }
    "raw"
}

/// Extract a stable client session identifier for analytics. Values are
/// bounded and rejected when they contain controls; request bodies are never
/// used as a fallback here.
fn retain_codex_metadata_for_client(
    client_kind: ClientKind,
    metadata: Option<CodexMetadata>,
) -> Option<CodexMetadata> {
    let metadata = metadata?;
    if client_kind == ClientKind::GrokBuild && !metadata.has_request_identity() {
        None
    } else {
        Some(metadata)
    }
}

fn observed_session_id(headers: &[(String, String)]) -> Option<String> {
    [
        "x-claude-code-session-id",
        "x-grok-session-id",
        "x-grok-conv-id",
        "session_id",
        "session-id",
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

fn supports_extended_reasoning_levels(client_version: &str) -> bool {
    let trimmed = client_version.trim().trim_start_matches('v');
    if trimmed.is_empty() {
        return true;
    }
    let parts = trimmed.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return true;
    }
    let Some(major) = parts[0].parse::<u64>().ok() else {
        return true;
    };
    let Some(minor) = parts[1].parse::<u64>().ok() else {
        return true;
    };
    let patch = parts
        .get(2)
        .and_then(|part| part.parse::<u64>().ok())
        .unwrap_or(0);
    (major, minor, patch) >= (0, 144, 0)
}

fn percent_decode_query_component(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => {
                out.push(' ');
                index += 1;
            }
            b'%' if index + 2 < bytes.len() => {
                let hi = bytes[index + 1];
                let lo = bytes[index + 2];
                if let (Some(high), Some(low)) = (from_hex_digit(hi), from_hex_digit(lo)) {
                    out.push(char::from((high << 4) | low));
                    index += 3;
                } else {
                    out.push('%');
                    index += 1;
                }
            }
            byte => {
                out.push(char::from(byte));
                index += 1;
            }
        }
    }
    out
}

fn from_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key || percent_decode_query_component(name) == key).then_some(value)
    })
}

fn decoded_query_value(query: &str, key: &str) -> Option<String> {
    query_value(query, key).map(percent_decode_query_component)
}

fn path_without_query(path_and_query: &str) -> &str {
    path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path)
}

fn json_timestamp(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<f64>().ok())
        })
        .filter(|timestamp| timestamp.is_finite())
}

fn is_realtime_http_path(path: &str) -> bool {
    [
        "/v1/realtime",
        "/realtime",
        "/openai/v1/realtime",
        "/backend-api/codex/realtime",
        "/v1/live",
        "/live",
        "/openai/v1/live",
        "/backend-api/codex/live",
    ]
    .iter()
    .any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn is_responses_websocket_path(path: &str) -> bool {
    [
        "/v1/responses",
        "/responses",
        "/backend-api/codex/responses",
    ]
    .iter()
    .any(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn is_path_or_child(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn is_resource_passthrough_kind(kind: PassthroughKind) -> bool {
    matches!(kind, PassthroughKind::Files | PassthroughKind::Models)
}

fn is_local_models_path(path: &str) -> bool {
    if [
        "/v1/models",
        "/models",
        "/openai/v1/models",
        "/backend-api/codex/models",
    ]
    .contains(&path)
    {
        return true;
    }
    [
        "/v1/models/",
        "/models/",
        "/openai/v1/models/",
        "/backend-api/codex/models/",
    ]
    .iter()
    .any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    })
}

fn local_model_id_from_path(path: &str) -> Option<&str> {
    [
        "/v1/models/",
        "/models/",
        "/openai/v1/models/",
        "/backend-api/codex/models/",
    ]
    .iter()
    .find_map(|prefix| path.strip_prefix(prefix))
    .filter(|id| !id.is_empty() && !id.contains('/'))
}

#[derive(Clone)]
struct LocalModelEntry {
    id: String,
    capabilities: Vec<sumpter_core::capability::ModelCapability>,
}

fn collect_local_models(
    config: &AppConfig,
    requested_capability: Option<sumpter_core::capability::ModelCapability>,
    requested_id: Option<&str>,
) -> Vec<LocalModelEntry> {
    let mut models =
        std::collections::BTreeMap::<String, Vec<sumpter_core::capability::ModelCapability>>::new();
    for endpoint in config.endpoints.iter().filter(|endpoint| endpoint.enabled) {
        let mut concrete_models = std::collections::BTreeSet::new();
        for mapping in &endpoint.mappings {
            let pattern = mapping.client_pattern.trim();
            let cleaned_pattern = sumpter_core::model_name::clean(pattern);
            if cleaned_pattern.contains('*') {
                for model in endpoint
                    .catalog
                    .as_ref()
                    .into_iter()
                    .flat_map(|catalog| catalog.models.iter())
                    .map(|model| sumpter_core::model_name::clean(model))
                    .filter(|model| {
                        !model.is_empty()
                            && !model.contains('*')
                            && sumpter_core::model_name::pattern_matches(&cleaned_pattern, model)
                    })
                {
                    concrete_models.insert(model);
                }
            } else {
                let model = sumpter_core::capability::canonical_model_from_pattern(pattern);
                if !model.is_empty() && !model.contains('*') {
                    concrete_models.insert(model);
                }
            }
        }

        for model in concrete_models {
            if requested_id.is_some_and(|id| id != model) {
                continue;
            }
            // Directory classification must use the same capability-aware
            // precedence as the planner.  A precise text mapping must not
            // hide a broader image/video/live mapping for the same logical
            // model, and a mapping can intentionally advertise more than one
            // capability.  Resolve each capability independently, then union
            // only the capabilities that are actually routable.
            let wanted_capabilities = requested_capability
                .map(|wanted| vec![wanted])
                .unwrap_or_else(|| {
                    vec![
                        sumpter_core::capability::ModelCapability::Text,
                        sumpter_core::capability::ModelCapability::Image,
                        sumpter_core::capability::ModelCapability::Video,
                        sumpter_core::capability::ModelCapability::Live,
                        sumpter_core::capability::ModelCapability::Files,
                    ]
                });
            let mut resolved_capabilities = Vec::new();
            for wanted in wanted_capabilities {
                let Some(mapping) = endpoint.mapping_for_capability(&model, wanted) else {
                    continue;
                };
                let capabilities = sumpter_core::capability::capabilities_for_model(
                    &mapping.capabilities,
                    &mapping.client_pattern,
                    &model,
                );
                for capability in capabilities {
                    if !resolved_capabilities.contains(&capability) {
                        resolved_capabilities.push(capability);
                    }
                }
            }
            if !resolved_capabilities.is_empty() {
                let entry = models.entry(model).or_default();
                for capability in resolved_capabilities {
                    if !entry.contains(&capability) {
                        entry.push(capability);
                    }
                }
            }
        }
    }
    models
        .into_iter()
        .map(|(id, mut capabilities)| {
            capabilities.sort_by_key(|capability| capability.as_str());
            LocalModelEntry { id, capabilities }
        })
        .collect()
}

fn openai_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "object": "model",
        "created": 0,
        "owned_by": "sumpter",
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
    })
}

fn is_media_only_conversation_model(model: &str) -> bool {
    let capabilities = sumpter_core::capability::inferred_capabilities(model);
    let media = capabilities.iter().any(|capability| {
        matches!(
            capability,
            sumpter_core::capability::ModelCapability::Image
                | sumpter_core::capability::ModelCapability::Video
        )
    });
    media && !capabilities.contains(&sumpter_core::capability::ModelCapability::Text)
}

fn is_codex_chat_model(capabilities: &[sumpter_core::capability::ModelCapability]) -> bool {
    capabilities.contains(&sumpter_core::capability::ModelCapability::Text)
        && !capabilities.iter().any(|capability| {
            matches!(
                capability,
                sumpter_core::capability::ModelCapability::Image
                    | sumpter_core::capability::ModelCapability::Video
                    | sumpter_core::capability::ModelCapability::Live
            )
        })
}

fn codex_reasoning_description(level: &str) -> &'static str {
    match level {
        "none" => "No reasoning",
        "minimal" => "Fastest responses with minimal reasoning",
        "low" => "Fast responses with lighter reasoning",
        "medium" => "Balances speed and reasoning depth for everyday tasks",
        "high" => "Greater reasoning depth for complex problems",
        "xhigh" => "Extra high reasoning depth for complex problems",
        "max" => "Maximum available reasoning depth for complex problems",
        _ => "ultra",
    }
}

fn codex_reasoning_levels(client_version: &str) -> Vec<Value> {
    let mut levels = vec!["none", "minimal", "low", "medium", "high", "xhigh"];
    if supports_extended_reasoning_levels(client_version) {
        levels.extend(["max", "ultra"]);
    }
    levels
        .into_iter()
        .map(|effort| {
            json!({
                "effort": effort,
                "description": codex_reasoning_description(effort),
            })
        })
        .collect()
}

fn codex_model_entry(model: &LocalModelEntry, client_version: &str) -> Value {
    let mut entry = json!({
        "slug": model.id,
        "display_name": model.id,
        "description": model.id,
        "prefer_websockets": false,
        "service_tiers": [],
    });
    if is_codex_chat_model(&model.capabilities) {
        entry["input_modalities"] = json!(["text"]);
        entry["supported_reasoning_levels"] = json!(codex_reasoning_levels(client_version));
        entry["default_reasoning_level"] = json!("medium");
    } else {
        entry["visibility"] = json!("hide");
    }
    entry
}

fn grok_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "model": model.id,
        "name": model.id,
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
        "api_backend": if is_codex_chat_model(&model.capabilities) {
            "responses"
        } else {
            "chat"
        },
    })
}

fn anthropic_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "type": "model",
        "display_name": model.id,
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
    })
}

fn local_models_json(
    config: &AppConfig,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
) -> Result<Value, StatusCode> {
    let requested_capability = query
        .and_then(|query| decoded_query_value(query, "capability"))
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "text" => Some(sumpter_core::capability::ModelCapability::Text),
            "image" => Some(sumpter_core::capability::ModelCapability::Image),
            "video" => Some(sumpter_core::capability::ModelCapability::Video),
            "live" | "realtime" => Some(sumpter_core::capability::ModelCapability::Live),
            "files" => Some(sumpter_core::capability::ModelCapability::Files),
            _ => None,
        });
    let requested_id = local_model_id_from_path(path);
    let models = collect_local_models(config, requested_capability, requested_id);
    if let Some(id) = requested_id {
        let Some(model) = models.iter().find(|model| model.id == id) else {
            return Err(StatusCode::NOT_FOUND);
        };
        return Ok(openai_model_object(model));
    }
    if let Some(client_version) =
        query.and_then(|query| decoded_query_value(query, "client_version"))
    {
        return Ok(json!({
            "models": models
                .iter()
                .map(|model| codex_model_entry(model, &client_version))
                .collect::<Vec<_>>(),
        }));
    }
    let user_agent = header_value(headers, "user-agent").unwrap_or("");
    let grok_shell = user_agent.to_ascii_lowercase().contains("grok-shell");
    let claude_cli = user_agent.starts_with("claude-cli/")
        || header_value(headers, "anthropic-version").is_some();
    if grok_shell {
        return Ok(json!({
            "object": "list",
            "data": models.iter().map(grok_model_object).collect::<Vec<_>>(),
        }));
    }
    if claude_cli {
        let data = models
            .iter()
            .map(anthropic_model_object)
            .collect::<Vec<_>>();
        let first_id = data.first().and_then(|value| value["id"].as_str());
        let last_id = data.last().and_then(|value| value["id"].as_str());
        return Ok(json!({
            "data": data,
            "has_more": false,
            "first_id": first_id,
            "last_id": last_id,
        }));
    }
    Ok(json!({
        "object": "list",
        "data": models.iter().map(openai_model_object).collect::<Vec<_>>(),
    }))
}

fn local_models_response(
    config: &AppConfig,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
) -> Response {
    match local_models_json(config, path, query, headers) {
        Ok(body) => json_response(StatusCode::OK, &body),
        Err(StatusCode::NOT_FOUND) => {
            error_response(StatusCode::NOT_FOUND, &[("error", "model_not_found")])
        }
        Err(status) => error_response(status, &[("error", "model_not_found")]),
    }
}

fn has_exact_codex_live_mapping(config: &AppConfig, endpoint_id: &str) -> bool {
    let live_model = request_build::DEFAULT_CODEX_LIVE_MODEL;
    config.endpoint(endpoint_id).is_some_and(|endpoint| {
        // Capability filtering must happen before mapping precedence.  This
        // accepts an explicitly declared `gpt-live-*`/`*` Live mapping while
        // still rejecting a text-only mapping that merely shares the model
        // prefix; the endpoint is eligible only for the Live surface.
        endpoint
            .mapping_for_capability(live_model, sumpter_core::capability::ModelCapability::Live)
            .is_some()
    })
}

fn has_exact_realtime_mapping(config: &AppConfig, endpoint_id: &str, model: &str) -> bool {
    let model = sumpter_core::model_name::clean(model);
    config.endpoint(endpoint_id).is_some_and(|endpoint| {
        endpoint
            .mapping_for_capability(&model, sumpter_core::capability::ModelCapability::Live)
            .is_some()
    })
}

/// Map the CPA-compatible dynamic HTTP surface to a native relay intent.
/// Unknown non-control paths intentionally become Raw so vendor-specific
/// resources do not require another engine release just to be reachable.
fn native_passthrough_kind(path: &str) -> PassthroughKind {
    if is_openai_resource_tree(path, "models") {
        PassthroughKind::Models
    } else if is_openai_resource_tree(path, "files") {
        PassthroughKind::Files
    } else if is_openai_resource_tree(path, "videos") {
        PassthroughKind::Videos
    } else if is_realtime_http_path(path) {
        PassthroughKind::Realtime
    } else {
        PassthroughKind::Raw
    }
}

fn is_openai_resource_tree(path: &str, resource: &str) -> bool {
    [
        format!("/v1/{resource}"),
        format!("/{resource}"),
        format!("/openai/v1/{resource}"),
        format!("/backend-api/codex/{resource}"),
    ]
    .iter()
    .any(|prefix| is_path_or_child(path, prefix))
}

fn is_videos_create_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/videos"
            | "/videos"
            | "/openai/v1/videos"
            | "/backend-api/codex/videos"
            | "/v1/videos/generations"
            | "/videos/generations"
            | "/openai/v1/videos/generations"
            | "/backend-api/codex/videos/generations"
            | "/v1/videos/edits"
            | "/videos/edits"
            | "/openai/v1/videos/edits"
            | "/backend-api/codex/videos/edits"
            | "/v1/videos/extensions"
            | "/videos/extensions"
            | "/openai/v1/videos/extensions"
            | "/backend-api/codex/videos/extensions"
    )
}

fn is_videos_create_request(method: &str, path_and_query: &str) -> bool {
    method.eq_ignore_ascii_case("POST") && is_videos_create_path(path_without_query(path_and_query))
}

fn is_live_bootstrap_path(path: &str) -> bool {
    is_codex_live_path(path) || is_realtime_call_bootstrap_path(path)
}

/// Identify an HTTP response that creates a Live call. The `/v1/realtime`
/// root is shared with public Realtime, so it is considered a bootstrap only
/// when the request was classified as Live and carries no existing call id.
/// Keeping this method-aware prevents a standard GET/POST Realtime resource
/// response from being persisted as a new Live binding.
fn is_live_bootstrap_request(method: &str, path_and_query: &str) -> bool {
    let path = path_without_query(path_and_query);
    if is_live_bootstrap_path(path) {
        return method.eq_ignore_ascii_case("POST")
            && (is_realtime_call_bootstrap_path(path)
                || live_call_id_from_target(path_and_query).is_none());
    }
    method.eq_ignore_ascii_case("POST")
        && is_realtime_root_path(path)
        && live_call_id_from_target(path_and_query).is_none()
        && current_request_context().is_some_and(|context| context.route_intent == "live")
}

fn video_id_from_path(path: &str) -> Option<&str> {
    [
        "/v1/videos/",
        "/videos/",
        "/openai/v1/videos/",
        "/backend-api/codex/videos/",
    ]
    .iter()
    .find_map(|prefix| path.strip_prefix(prefix))
    .map(|rest| rest.split('/').next().unwrap_or_default())
    .filter(|id| !id.is_empty() && !matches!(*id, "generations" | "edits" | "extensions"))
}

fn is_videos_lookup_path(path: &str) -> bool {
    video_id_from_path(path).is_some()
}

fn video_id_from_headers(headers: &[(String, String)]) -> Option<String> {
    // Header names have a semantic priority independent of wire order. Some
    // providers emit both an internal session id and a public video id; the
    // latter must win even when it appeared earlier in the response.
    for header_name in ["x-video-id", "x-video-session-id"] {
        if let Some((_, value)) = headers.iter().rev().find(|(name, value)| {
            name.eq_ignore_ascii_case(header_name) && !value.trim().is_empty()
        }) && let Some(id) = valid_video_resource_id(value)
        {
            return Some(id);
        }
    }
    headers.iter().rev().find_map(|(name, value)| {
        if !name.eq_ignore_ascii_case("location") {
            return None;
        }
        let location = value.trim();
        let path = reqwest::Url::parse(location)
            .map(|url| url.path().to_string())
            .unwrap_or_else(|_| path_without_query(location).to_string());
        video_id_from_path(&path)
            .and_then(valid_video_resource_id)
            .or_else(|| {
                (!location.contains('/') && !location.contains('?'))
                    .then(|| valid_video_resource_id(location))
                    .flatten()
            })
    })
}

fn valid_video_resource_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| value.to_string())
}

fn video_id_from_json(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    ["id", "request_id", "video_id"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find_map(valid_video_resource_id)
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(Value::as_str)
                .and_then(valid_video_resource_id)
        })
}

/// Extract a resource id from a bounded response payload that may contain a
/// plain JSON object, SSE `data:` records, or newline-delimited JSON.  The
/// parser accepts a JSON value prefix so trailing SSE framing does not make a
/// complete object look malformed.  Callers cap the accumulated payload at
/// 64 KiB; the scan itself is additionally bounded to avoid adversarial work.
fn resource_id_from_payload<F>(payload: &[u8], mut extract: F) -> Option<String>
where
    F: FnMut(&Value) -> Option<String>,
{
    const MAX_CANDIDATE_OFFSETS: usize = 2048;
    let mut offsets = Vec::with_capacity(32);
    offsets.push(0);
    if let Ok(text) = std::str::from_utf8(payload) {
        // SSE data records can span multiple physical lines.  Start parsing
        // immediately after every `data:` marker as well as at each line
        // boundary for NDJSON and ordinary pretty-printed JSON.
        for (offset, _) in text.match_indices("data:") {
            if offsets.len() >= MAX_CANDIDATE_OFFSETS {
                break;
            }
            offsets.push(offset + "data:".len());
        }
        if offsets.len() < MAX_CANDIDATE_OFFSETS {
            for (offset, byte) in payload.iter().enumerate() {
                if *byte == b'\n' && offset + 1 < payload.len() {
                    offsets.push(offset + 1);
                    if offsets.len() >= MAX_CANDIDATE_OFFSETS {
                        break;
                    }
                }
            }
        }
    }
    offsets.sort_unstable();
    offsets.dedup();
    for offset in offsets {
        let candidate = payload.get(offset..).unwrap_or_default();
        let candidate = trim_ascii_whitespace(candidate);
        if candidate.is_empty() || candidate.starts_with(b"[DONE]") {
            continue;
        }
        let mut deserializer = serde_json::Deserializer::from_slice(candidate);
        let Ok(value) = <Value as serde::Deserialize>::deserialize(&mut deserializer) else {
            continue;
        };
        if let Some(id) = extract(&value) {
            return Some(id);
        }
    }
    None
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some(byte) = bytes.first() {
        if !byte.is_ascii_whitespace() {
            break;
        }
        bytes = &bytes[1..];
    }
    bytes
}

fn video_id_from_payload(body: &[u8]) -> Option<String> {
    resource_id_from_payload(body, |value| {
        let encoded = serde_json::to_vec(value).ok()?;
        video_id_from_json(&encoded)
    })
}

fn live_call_id_from_json(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let from_object = |object: &serde_json::Map<String, Value>| {
        // Explicit call fields are authoritative and follow CPA's bounded
        // `[A-Za-z0-9_-]{1,128}` contract (including `rtc_*` IDs). A generic
        // `id` is accepted only for a call-shaped response to avoid binding a
        // request/job/video id as a Live session by accident.
        let explicit = ["call_id", "callId", "call"]
            .into_iter()
            .find_map(|key| object.get(key).and_then(Value::as_str))
            .and_then(valid_live_resource_id);
        explicit.or_else(|| {
            let call_shape = object
                .get("object")
                .and_then(Value::as_str)
                .is_some_and(|object| {
                    let lower = object.to_ascii_lowercase();
                    lower.contains("call") || lower.contains("realtime")
                })
                || object.contains_key("sdp")
                || object.contains_key("session");
            call_shape
                .then(|| object.get("id").and_then(Value::as_str))
                .flatten()
                .and_then(valid_live_resource_id)
        })
    };
    value.as_object().and_then(from_object).or_else(|| {
        value
            .get("data")
            .and_then(Value::as_object)
            .and_then(from_object)
    })
}

fn live_call_id_from_payload(body: &[u8]) -> Option<String> {
    resource_id_from_payload(body, |value| {
        let encoded = serde_json::to_vec(value).ok()?;
        live_call_id_from_json(&encoded)
    })
}

fn valid_live_resource_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| value.to_string())
}

fn websocket_message_model(message: &WebSocketMessage) -> Option<String> {
    let WebSocketMessage::Text(text) = message else {
        return None;
    };
    let value = serde_json::from_str::<Value>(text.as_str()).ok()?;
    [
        value.get("model"),
        value
            .get("session")
            .and_then(|session| session.get("model")),
        value
            .get("response")
            .and_then(|response| response.get("model")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|model| !model.is_empty())
    .map(str::to_string)
}

fn websocket_message_to_tungstenite(
    message: WebSocketMessage,
) -> tokio_tungstenite::tungstenite::Message {
    match message {
        WebSocketMessage::Text(text) => {
            tokio_tungstenite::tungstenite::Message::Text(text.as_str().to_string().into())
        }
        WebSocketMessage::Binary(bytes) => {
            tokio_tungstenite::tungstenite::Message::Binary(bytes.to_vec().into())
        }
        WebSocketMessage::Ping(bytes) => {
            tokio_tungstenite::tungstenite::Message::Ping(bytes.to_vec().into())
        }
        WebSocketMessage::Pong(bytes) => {
            tokio_tungstenite::tungstenite::Message::Pong(bytes.to_vec().into())
        }
        WebSocketMessage::Close(frame) => {
            let frame = frame.map(
                |frame| tokio_tungstenite::tungstenite::protocol::CloseFrame {
                    code: frame.code.into(),
                    reason: frame.reason.to_string().into(),
                },
            );
            tokio_tungstenite::tungstenite::Message::Close(frame)
        }
    }
}

fn is_codex_live_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/live" | "/live" | "/openai/v1/live" | "/backend-api/codex/live"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeRouteIntent {
    CodexLive,
    StandardRealtime,
}

/// Distinguish the public OpenAI Realtime surface from Codex's Quicksilver
/// bootstrap.  The distinction is intentionally method/path first, matching
/// CPA's route table:
///
/// * POST `/v1/live`, `/v1/realtime`, and `/v1/realtime/calls` are Live
///   bootstraps;
/// * GET `/v1/realtime` without a Quicksilver marker/call id is standard
///   Realtime (including a Codex client's WebSocket handshake);
/// * call-id children and explicit Quicksilver markers are Live sideband;
/// * client-secrets/sessions/transcription/translation/control paths are
///   standard Realtime resource calls, never a Live bootstrap.
///
/// Model and client identity are only compatibility hints after those route
/// rules.  In particular, a leaked Claude/chat model cannot turn a generic
/// request into Live, and Codex identity cannot turn a public GET WebSocket
/// into Live merely because its UA is Codex.
fn classify_realtime_intent(
    method: &str,
    path_and_query: &str,
    requested: Option<&str>,
    sideband_model: Option<&str>,
    secret_model: Option<&str>,
    _client_kind: ClientKind,
    body_codex_live: bool,
) -> RealtimeRouteIntent {
    let path = path_without_query(path_and_query);
    let query = path_and_query
        .split_once('?')
        .map_or("", |(_, query)| query);
    let is_post = method.eq_ignore_ascii_case("POST");
    let is_get = method.eq_ignore_ascii_case("GET");

    // `/live` is a dedicated Codex Live family.  Its children are sideband
    // resources and therefore remain Live even when the method is GET/POST.
    if is_codex_live_family_path(path) {
        return RealtimeRouteIntent::CodexLive;
    }

    // Realtime call children are Live sideband; the root is a Live bootstrap
    // on POST.  CPA registers this route with the Codex Live handler, while
    // the public standard Realtime WebSocket uses GET on the same root.
    // Keep this method/path rule independent of the surrounding chat model or
    // UA: Codex Desktop has been observed sending `model=claude-*` here, and
    // allowing that value to choose the route recreates the Fable failure.
    if is_realtime_call_path_prefix(path) {
        return RealtimeRouteIntent::CodexLive;
    }
    if is_realtime_call_bootstrap_path(path) && is_post {
        return RealtimeRouteIntent::CodexLive;
    }
    if is_realtime_root_path(path) && is_post {
        return RealtimeRouteIntent::CodexLive;
    }

    // These endpoints issue/operate standard Realtime credentials/sessions.
    // Keep them out of the Live bootstrap branch even if a body happens to
    // contain a Codex-looking metadata field.
    if is_realtime_control_path(path) {
        return RealtimeRouteIntent::StandardRealtime;
    }

    // The WebSocket root is the public Realtime surface.  A GET handshake is
    // never upgraded to Codex Live because of a leaked chat model, Codex UA,
    // `intent=quicksilver`, or `architecture=avas`; only an explicit call id
    // turns it into a Live sideband target.  Path validation runs before this
    // classifier and rejects an empty/illegal call id with 400.
    if is_get && is_realtime_root_path(path) {
        return if live_call_id_from_target(path_and_query).is_some() {
            RealtimeRouteIntent::CodexLive
        } else {
            RealtimeRouteIntent::StandardRealtime
        };
    }

    let quicksilver_marker = decoded_query_value(query, "intent")
        .is_some_and(|value| value.eq_ignore_ascii_case("quicksilver"))
        || decoded_query_value(query, "architecture")
            .is_some_and(|value| value.eq_ignore_ascii_case("avas"));
    if quicksilver_marker || is_codex_live_sideband_target(path_and_query) {
        return RealtimeRouteIntent::CodexLive;
    }

    // A JSON quicksilver envelope is a positive protocol marker, but only on
    // a bootstrap-capable POST route.  Do not let an arbitrary standard
    // Realtime control payload switch protocol families.
    if is_post && is_realtime_bootstrap_root(path) && body_codex_live {
        return RealtimeRouteIntent::CodexLive;
    }

    let explicit_model = requested
        .or(sideband_model)
        .or(secret_model)
        .map(sumpter_core::model_name::clean)
        .filter(|model| !model.is_empty());
    if let Some(model) = explicit_model {
        if model == request_build::DEFAULT_CODEX_LIVE_MODEL {
            return RealtimeRouteIntent::CodexLive;
        }
        // Public Realtime model families (including older gpt-4o sessions)
        // stay on the standard protocol, even when the caller is Codex.
        if model == request_build::DEFAULT_REALTIME_MODEL
            || model.starts_with("gpt-realtime-")
            || model.contains("realtime-preview")
            || model == "gpt-4o"
            || model == "gpt-4o-mini"
        {
            return RealtimeRouteIntent::StandardRealtime;
        }
    }

    RealtimeRouteIntent::StandardRealtime
}

fn realtime_route_model(
    method: &str,
    path_and_query: &str,
    requested: Option<String>,
    sideband_model: Option<String>,
    secret_model: Option<String>,
    intent: RealtimeRouteIntent,
) -> String {
    if intent == RealtimeRouteIntent::CodexLive {
        if is_codex_live_sideband_target(path_and_query) {
            return sideband_model
                .filter(|model| !model.trim().is_empty())
                .or(secret_model)
                .unwrap_or_else(|| request_build::DEFAULT_CODEX_LIVE_MODEL.to_string());
        }
        // `/v1/live` and a model-less quicksilver bootstrap are always bound
        // to the private Codex Live model.  A client-secret call may instead
        // carry a standard model such as `gpt-4o`; CPA keeps that logical
        // model and only rewrites it through the configured mapping.  Keep
        // that distinction here so the secret scope check does not compare a
        // client model with an unrelated forced model.
        let candidate = requested
            .or(sideband_model)
            .or(secret_model)
            .filter(|model| !model.trim().is_empty());
        let Some(candidate) = candidate else {
            return request_build::DEFAULT_CODEX_LIVE_MODEL.to_string();
        };
        let cleaned = sumpter_core::model_name::clean(&candidate);
        return if is_codex_live_family_path(path_without_query(path_and_query))
            || cleaned == request_build::DEFAULT_REALTIME_MODEL
            || cleaned.starts_with("gpt-realtime-")
            || cleaned.contains("realtime-preview")
            || cleaned == request_build::DEFAULT_CODEX_LIVE_MODEL
        {
            request_build::DEFAULT_CODEX_LIVE_MODEL.to_string()
        } else if cleaned.starts_with("gpt-4o") || cleaned.contains("live") {
            // Standard Realtime sessions and explicitly named custom Live
            // models remain selectable when the configuration has a matching
            // capability mapping.  The config-aware resolver below falls
            // back to the private Live mapping when they do not.
            candidate
        } else {
            // A leaked chat model (Claude/Fable, GPT text, etc.) must never
            // select a text provider for a Live bootstrap.
            request_build::DEFAULT_CODEX_LIVE_MODEL.to_string()
        };
    }
    requested
        .or(secret_model)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| {
            let _ = method;
            request_build::DEFAULT_REALTIME_MODEL.to_string()
        })
}

/// Client-secret sessions expose a public Realtime model while the Codex
/// OAuth surface may normalize that model to `gpt-live-1-codex`.  Treat those
/// two names as the same authorization scope only for known Realtime model
/// families; an arbitrary text model must still fail closed.
fn realtime_client_secret_models_match(expected: &str, requested: &str) -> bool {
    let expected = sumpter_core::model_name::clean(expected);
    let requested = sumpter_core::model_name::clean(requested);
    if expected.is_empty() || requested.is_empty() {
        return false;
    }
    if expected == requested {
        return true;
    }
    let expected_realtime = is_standard_realtime_model_name(&expected)
        || expected == request_build::DEFAULT_CODEX_LIVE_MODEL;
    let requested_realtime = is_standard_realtime_model_name(&requested)
        || requested == request_build::DEFAULT_CODEX_LIVE_MODEL;
    expected_realtime && requested_realtime
}

fn is_standard_realtime_model_name(model: &str) -> bool {
    sumpter_core::capability::is_realtime_model_name(model)
}

/// Resolve a logical Realtime/Live model against the configured capability
/// catalog.  CPA accepts a standard client-facing model (for example
/// `gpt-4o`) but may send it through the Codex Live OAuth surface.  If that
/// logical model has no `live` mapping in this installation, use the explicit
/// `gpt-live-1-codex` mapping instead of allowing the request to fall into a
/// text-only wildcard or an arbitrary first endpoint.
fn resolve_realtime_route_model(
    config: &AppConfig,
    method: &str,
    path_and_query: &str,
    requested: Option<String>,
    sideband_model: Option<String>,
    secret_model: Option<String>,
    intent: RealtimeRouteIntent,
) -> String {
    let candidate = realtime_route_model(
        method,
        path_and_query,
        requested,
        sideband_model,
        secret_model,
        intent,
    );
    let has_mapping = |model: &str| {
        config.endpoints.iter().any(|endpoint| {
            endpoint.enabled
                && endpoint
                    .mapping_for_capability(model, sumpter_core::capability::ModelCapability::Live)
                    .is_some()
        })
    };
    if has_mapping(&candidate) {
        return candidate;
    }
    if intent == RealtimeRouteIntent::CodexLive
        && let Some(default) = RoutePlanner::default_model_for_capability(
            config,
            sumpter_core::capability::ModelCapability::Live,
        )
    {
        return default;
    }
    candidate
}

/// Test helper for a Codex voice bootstrap. Production paths pass the detected
/// client identity to `classify_realtime_intent` directly.
#[cfg(test)]
fn realtime_voice_route_model(
    method: &str,
    path_and_query: &str,
    requested: Option<String>,
    sideband_model: Option<String>,
    secret_model: Option<String>,
) -> String {
    let intent = classify_realtime_intent(
        method,
        path_and_query,
        requested.as_deref(),
        sideband_model.as_deref(),
        secret_model.as_deref(),
        ClientKind::Codex,
        false,
    );
    realtime_route_model(
        method,
        path_and_query,
        requested,
        sideband_model,
        secret_model,
        intent,
    )
}

fn body_indicates_codex_live(body: &[u8], content_type: Option<&str>) -> bool {
    let Some(value) = metadata_json_body_hint(body, content_type) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let session = json_object_value(object, "session").and_then(Value::as_object);
    json_object_value(object, "intent")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("quicksilver"))
        || json_object_value(object, "architecture")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("avas"))
        || session
            .and_then(|session| json_object_value(session, "type"))
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("quicksilver"))
        || session
            .and_then(|session| json_object_value(session, "model"))
            .and_then(Value::as_str)
            .is_some_and(|value| {
                sumpter_core::model_name::clean(value) == request_build::DEFAULT_CODEX_LIVE_MODEL
            })
        || json_object_value(object, "client_metadata")
            .and_then(Value::as_object)
            .and_then(|metadata| json_object_value(metadata, "originator"))
            .and_then(Value::as_str)
            .is_some_and(|value| {
                let value = value.to_ascii_lowercase();
                value.contains("codex desktop")
                    || value.contains("codex_cli_rs")
                    || value.contains("codex-tui")
            })
}

fn json_object_value<'a>(
    object: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Option<&'a Value> {
    object.get(key).or_else(|| {
        object
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
    })
}

fn is_codex_live_family_path(path: &str) -> bool {
    [
        "/v1/live",
        "/live",
        "/openai/v1/live",
        "/backend-api/codex/live",
    ]
    .iter()
    .any(|prefix| is_path_or_child(path, prefix))
}

fn is_realtime_root_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/realtime" | "/realtime" | "/openai/v1/realtime" | "/backend-api/codex/realtime"
    )
}

fn is_realtime_bootstrap_root(path: &str) -> bool {
    is_realtime_root_path(path) || is_realtime_call_bootstrap_path(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RealtimeCallPathError {
    InvalidId,
    UnsupportedAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RealtimeCallTarget {
    call_id: String,
    action: Option<String>,
}

/// Parse and validate CPA's `/realtime/calls/:call_id[/action]` surface and
/// root `?call_id=` sideband form. `Ok(None)` means the target is not a call
/// resource. Percent decoding is deliberately strict: malformed escapes,
/// decoded separators and control bytes are rejected rather than falling
/// through to an unrelated Realtime route.
fn validate_realtime_call_target(
    path_and_query: &str,
) -> Result<Option<RealtimeCallTarget>, RealtimeCallPathError> {
    let path = path_without_query(path_and_query);
    for prefix in [
        "/v1/realtime/calls/",
        "/realtime/calls/",
        "/openai/v1/realtime/calls/",
        "/backend-api/codex/realtime/calls/",
        "/v1/live/",
        "/live/",
        "/openai/v1/live/",
        "/backend-api/codex/live/",
    ] {
        let Some(rest) = path.strip_prefix(prefix) else {
            continue;
        };
        let mut segments = rest.split('/');
        let raw_id = segments.next().unwrap_or_default();
        let Some(call_id) =
            percent_decode_path_segment(raw_id).and_then(|value| valid_live_resource_id(&value))
        else {
            return Err(RealtimeCallPathError::InvalidId);
        };
        let action = segments.next();
        if segments.next().is_some()
            || action.is_some_and(|action| {
                action.is_empty() || !matches!(action, "hangup" | "accept" | "reject" | "refer")
            })
        {
            return Err(RealtimeCallPathError::UnsupportedAction);
        }
        return Ok(Some(RealtimeCallTarget {
            call_id,
            action: action.map(str::to_string),
        }));
    }

    if is_realtime_root_path(path)
        && let Some(query) = path_and_query.split_once('?').map(|(_, query)| query)
        && query_has_key(query, "call_id")
    {
        let Some(call_id) = strict_decoded_query_value(query, "call_id")
            .and_then(|value| valid_live_resource_id(&value))
        else {
            return Err(RealtimeCallPathError::InvalidId);
        };
        return Ok(Some(RealtimeCallTarget {
            call_id,
            action: None,
        }));
    }
    Ok(None)
}

fn realtime_call_path_error(
    error: RealtimeCallPathError,
) -> (StatusCode, &'static str, &'static str) {
    match error {
        RealtimeCallPathError::InvalidId => (
            StatusCode::BAD_REQUEST,
            "invalid_realtime_call_id",
            "Realtime call id is invalid",
        ),
        RealtimeCallPathError::UnsupportedAction => (
            StatusCode::NOT_FOUND,
            "unsupported_realtime_call_action",
            "Realtime call action is not supported",
        ),
    }
}

fn query_has_key(query: &str, key: &str) -> bool {
    query.split('&').any(|pair| {
        let (raw_name, _) = pair.split_once('=').unwrap_or((pair, ""));
        strict_percent_decode(raw_name, true).as_deref() == Some(key)
    })
}

fn strict_decoded_query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        (strict_percent_decode(raw_name, true).as_deref() == Some(key))
            .then(|| strict_percent_decode(raw_value, true))
            .flatten()
    })
}

fn percent_decode_path_segment(value: &str) -> Option<String> {
    let decoded = strict_percent_decode(value, false)?;
    (!decoded.contains('/') && !decoded.contains('\\')).then_some(decoded)
}

fn strict_percent_decode(value: &str, plus_as_space: bool) -> Option<String> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' if plus_as_space => {
                decoded.push(b' ');
                index += 1;
            }
            b'%' => {
                if index + 2 >= bytes.len() {
                    return None;
                }
                let high = from_hex_digit(bytes[index + 1])?;
                let low = from_hex_digit(bytes[index + 2])?;
                decoded.push((high << 4) | low);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

fn is_realtime_call_path_prefix(path: &str) -> bool {
    [
        "/v1/realtime/calls/",
        "/realtime/calls/",
        "/openai/v1/realtime/calls/",
        "/backend-api/codex/realtime/calls/",
    ]
    .iter()
    .any(|prefix| path.starts_with(prefix))
}

#[cfg(test)]
fn unsupported_realtime_call_action(path: &str) -> bool {
    validate_realtime_call_target(path) == Err(RealtimeCallPathError::UnsupportedAction)
}

/// Realtime credential/session and SIP/control surfaces.  They share the
/// `live` capability in the mapping catalog, but are not Codex Live
/// bootstraps and must never be wrapped as Quicksilver.
fn is_realtime_control_path(path: &str) -> bool {
    [
        "/v1/realtime/client_secrets",
        "/realtime/client_secrets",
        "/openai/v1/realtime/client_secrets",
        "/backend-api/codex/realtime/client_secrets",
        "/v1/realtime/sessions",
        "/realtime/sessions",
        "/openai/v1/realtime/sessions",
        "/backend-api/codex/realtime/sessions",
        "/v1/realtime/transcription_sessions",
        "/realtime/transcription_sessions",
        "/openai/v1/realtime/transcription_sessions",
        "/backend-api/codex/realtime/transcription_sessions",
        "/v1/realtime/translations",
        "/realtime/translations",
        "/openai/v1/realtime/translations",
        "/backend-api/codex/realtime/translations",
        "/v1/realtime/translations/client_secrets",
        "/realtime/translations/client_secrets",
        "/openai/v1/realtime/translations/client_secrets",
        "/backend-api/codex/realtime/translations/client_secrets",
    ]
    .iter()
    .any(|prefix| is_path_or_child(path, prefix))
}

fn is_realtime_call_bootstrap_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/realtime/calls"
            | "/realtime/calls"
            | "/openai/v1/realtime/calls"
            | "/backend-api/codex/realtime/calls"
    )
}

/// CPA treats any valid `call_id` on the Realtime root as a sideband target
/// (the query `intent=quicksilver` is not required).  Keep that behavior so a
/// sideband cannot accidentally open a fresh standard Realtime session.
fn live_call_id_from_target(path_and_query: &str) -> Option<String> {
    validate_realtime_call_target(path_and_query)
        .ok()
        .flatten()
        .map(|target| target.call_id)
}

fn is_codex_live_sideband_target(path_and_query: &str) -> bool {
    let path = path_without_query(path_and_query);
    (is_codex_live_family_path(path) && live_call_id_from_target(path_and_query).is_some())
        || (is_realtime_root_path(path) && live_call_id_from_target(path_and_query).is_some())
}

fn live_call_id_from_headers(headers: &[(String, String)]) -> Option<String> {
    for header_name in ["x-live-call-id", "x-live-session", "x-call-id"] {
        if let Some((_, value)) = headers.iter().rev().find(|(name, value)| {
            name.eq_ignore_ascii_case(header_name) && !value.trim().is_empty()
        }) && let Some(id) = valid_live_resource_id(value)
        {
            return Some(id);
        }
    }
    headers.iter().rev().find_map(|(name, value)| {
        if !name.eq_ignore_ascii_case("location") {
            return None;
        }
        let location = value.trim();
        let target = reqwest::Url::parse(location)
            .map(|url| {
                let mut target = url.path().to_string();
                if let Some(query) = url.query() {
                    target.push('?');
                    target.push_str(query);
                }
                target
            })
            .unwrap_or_else(|_| location.to_string());
        live_call_id_from_target(&target).or_else(|| {
            (!location.contains('/') && !location.contains('?'))
                .then(|| valid_live_resource_id(location))
                .flatten()
        })
    })
}

fn is_realtime_client_secret_path(path_and_query: &str) -> bool {
    let path = path_without_query(path_and_query);
    matches!(
        path,
        "/v1/realtime/client_secrets"
            | "/realtime/client_secrets"
            | "/openai/v1/realtime/client_secrets"
            | "/backend-api/codex/realtime/client_secrets"
            | "/v1/realtime/sessions"
            | "/realtime/sessions"
            | "/openai/v1/realtime/sessions"
            | "/backend-api/codex/realtime/sessions"
            | "/v1/realtime/translations/client_secrets"
            | "/realtime/translations/client_secrets"
            | "/openai/v1/realtime/translations/client_secrets"
            | "/backend-api/codex/realtime/translations/client_secrets"
    )
}

fn is_hop_by_hop(name: &str) -> bool {
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

pub fn error_response(status: StatusCode, pairs: &[(&str, &str)]) -> Response {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    json_response(status, &Value::Object(map))
}

fn method_not_allowed_response(allow: &str) -> Response {
    let mut response = error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        &[("error", "method_not_allowed")],
    );
    if let Ok(value) = HeaderValue::from_str(allow) {
        response.headers_mut().insert("allow", value);
    }
    response
}

fn proxy_failure_response(
    status: StatusCode,
    error: &str,
    request_id: &str,
    failure: &FailureInfo,
    retry_delay_seconds: Option<f64>,
    pass_through_retry_delay: bool,
) -> Response {
    let mut body = serde_json::Map::new();
    body.insert("error".into(), Value::String(error.into()));
    body.insert("requestID".into(), Value::String(request_id.into()));
    body.insert(
        "failureKind".into(),
        Value::String(failure.kind.as_str().into()),
    );
    body.insert(
        "failurePhase".into(),
        Value::String(failure.phase.as_str().into()),
    );
    if let Some(detail) = &failure.detail {
        body.insert("message".into(), Value::String(detail.clone()));
    }
    if let Some(timeout_ms) = failure.timeout_ms {
        body.insert("timeoutMS".into(), Value::Number(timeout_ms.into()));
    }
    if let Some(upstream_status) = failure.upstream_status_code {
        body.insert(
            "upstreamStatusCode".into(),
            Value::Number(upstream_status.into()),
        );
    }
    if let Some(upstream_request_id) = &failure.upstream_request_id {
        body.insert(
            "upstreamRequestID".into(),
            Value::String(upstream_request_id.clone()),
        );
    }
    let configured_retry_delay =
        retry_delay_seconds.filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    let provider_retry_delay = failure
        .retry_after_seconds
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    let retry_delay_seconds = pass_through_retry_delay
        .then(|| {
            matches!(
                failure.kind,
                RuntimeFailureKind::ResponseTimeout
                    | RuntimeFailureKind::ConnectionFailed
                    | RuntimeFailureKind::UpstreamHttpStatus
                    | RuntimeFailureKind::EndpointsExhausted
            )
            .then(|| match (configured_retry_delay, provider_retry_delay) {
                (Some(configured), Some(provider)) => Some(configured.max(provider)),
                (Some(configured), None) => Some(configured),
                (None, Some(provider)) => Some(provider),
                (None, None) => None,
            })
            .flatten()
        })
        .flatten();
    if let Some(seconds) = retry_delay_seconds {
        body.insert(
            "retry_delay".into(),
            serde_json::Number::from_f64(seconds)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    let mut response = json_response(status, &Value::Object(body));
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-sumpter-request-id", value);
    }
    if let Some(seconds) = retry_delay_seconds {
        // 标准 Retry-After 只接受整数 delay-seconds；向上取整避免客户端过早重试。
        let delay = seconds.ceil().min(u64::MAX as f64) as u64;
        if let Ok(value) = HeaderValue::from_str(&delay.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

pub fn json_response(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .body(Body::from(serde_json::to_vec(body).unwrap_or_default()))
        .unwrap_or_default()
}

pub fn json_bytes_response(status: StatusCode, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(Body::from(body))
        .unwrap_or_default()
}

#[cfg(test)]
mod retry_delay_tests {
    use super::*;

    fn assert_seconds(actual: Duration, expected: f64) {
        assert!(
            (actual.as_secs_f64() - expected).abs() < 0.001,
            "expected {expected}s, got {}s",
            actual.as_secs_f64()
        );
    }

    #[test]
    fn retry_after_uses_largest_valid_numeric_header() {
        let headers = vec![
            ("Retry-After".into(), "2.5".into()),
            ("retry-after".into(), "7".into()),
            ("Retry-After".into(), "Wed, 21 Oct 2015 07:28:00 GMT".into()),
            ("Retry-After".into(), "-3".into()),
            ("Retry-After".into(), "NaN".into()),
        ];
        assert_eq!(retry_after_seconds(&headers), Some(7.0));
    }

    #[test]
    fn retry_backoff_is_exponential_and_capped_with_retry_after() {
        assert_seconds(retry_backoff_delay(1, None), 0.5);
        assert_seconds(retry_backoff_delay(2, None), 0.85);
        assert_seconds(retry_backoff_delay(3, None), 1.445);
        assert_seconds(retry_backoff_delay(1, Some(7.0)), 7.0);
        assert_seconds(retry_backoff_delay(1, Some(45.0)), 30.0);
        assert_seconds(retry_backoff_delay(100, None), 30.0);
        assert_seconds(retry_backoff_delay(1, Some(-1.0)), 0.5);
        assert_seconds(retry_backoff_delay(1, Some(f64::NAN)), 0.5);
    }

    #[test]
    fn observed_session_id_is_trimmed_bounded_and_rejects_controls() {
        let valid = vec![(
            "X-Claude-Code-Session-Id".into(),
            "  claude-session  ".into(),
        )];
        assert_eq!(
            observed_session_id(&valid).as_deref(),
            Some("claude-session")
        );

        let grok = vec![("x-grok-session-id".into(), "  grok-sess  ".into())];
        assert_eq!(observed_session_id(&grok).as_deref(), Some("grok-sess"));
        let grok_conv = vec![("x-grok-conv-id".into(), "conv-only".into())];
        assert_eq!(
            observed_session_id(&grok_conv).as_deref(),
            Some("conv-only")
        );

        let control = vec![("session_id".into(), "bad\nsession".into())];
        assert_eq!(observed_session_id(&control), None);

        let long = vec![("session-id".into(), "会".repeat(100))];
        let value = observed_session_id(&long).unwrap();
        assert!(value.len() <= 256);
        assert!(value.is_char_boundary(value.len()));
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;

    #[test]
    fn video_lookup_paths_extract_resource_ids() {
        assert_eq!(
            video_id_from_path("/v1/videos/video_123/content"),
            Some("video_123")
        );
        assert_eq!(
            video_id_from_path("/openai/v1/videos/video_abc"),
            Some("video_abc")
        );
        assert_eq!(video_id_from_path("/v1/videos/generations"), None);
        assert!(is_videos_create_path("/v1/videos"));
        assert!(is_videos_create_path("/v1/videos/generations"));
        assert!(!is_videos_create_path("/v1/videos/video_123"));
        assert!(is_videos_create_request("POST", "/v1/videos"));
        assert!(is_videos_create_request("POST", "/v1/videos/edits"));
        assert!(!is_videos_create_request("GET", "/v1/videos"));
        assert!(!is_videos_create_request("GET", "/v1/videos/generations"));
        assert!(!is_videos_create_request("POST", "/v1/videos/video_123"));
        assert_eq!(
            video_id_from_json(br#"{"id":"video_abc","object":"video"}"#).as_deref(),
            Some("video_abc")
        );
        assert_eq!(
            video_id_from_headers(&[("Location".into(), "video_abc".into())]).as_deref(),
            Some("video_abc")
        );
        assert_eq!(
            video_id_from_headers(&[
                ("X-Video-ID".into(), "video_public".into()),
                ("X-Video-Session-ID".into(), "session_internal".into()),
            ])
            .as_deref(),
            Some("video_public")
        );
        assert_eq!(
            video_id_from_headers(&[
                ("X-Video-Session-ID".into(), "session_internal".into()),
                ("X-Video-ID".into(), "video_public".into()),
            ])
            .as_deref(),
            Some("video_public")
        );
    }

    #[tokio::test]
    async fn realtime_root_live_bootstrap_is_the_only_root_that_registers_a_call() {
        INBOUND_REQUEST_CONTEXT
            .scope(
                RefCell::new(InboundRequestContext {
                    method: "POST".into(),
                    path: "/v1/realtime".into(),
                    route_intent: "live".into(),
                    session_id: None,
                    grok_metadata: None,
                }),
                async {
                    assert!(is_live_bootstrap_request(
                        "POST",
                        "/v1/realtime?intent=quicksilver"
                    ));
                    assert!(!is_live_bootstrap_request("GET", "/v1/realtime"));
                    assert!(!is_live_bootstrap_request(
                        "POST",
                        "/v1/realtime?intent=quicksilver&call_id=rtc_1"
                    ));
                },
            )
            .await;
    }

    #[test]
    fn videos_multipart_model_is_used_for_routing_without_rebuilding_body() {
        let boundary = "video-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\ncat\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video-1.5\r\n--{boundary}--\r\n"
        );
        let fields = native_multipart_fields_with_default(
            body.as_bytes(),
            &format!("multipart/form-data; boundary=\"{boundary}\""),
            "",
        )
        .expect("valid video multipart");
        assert_eq!(fields.model, "grok-imagine-video-1.5");
        assert!(!fields.stream);
    }

    #[test]
    fn multipart_parser_requires_real_line_delimiters_and_closing_boundary() {
        let boundary = "safe-boundary";
        // The uploaded bytes contain boundary-looking text, but neither
        // occurrence is a valid delimiter (bad line position/suffix). The
        // parser must retain the complete binary field and still find the
        // actual closing delimiter.
        let mut body =
            format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n")
                .into_bytes();
        body.extend_from_slice(b"binary--safe-boundaryX\r\n");
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        let fields = native_multipart_fields_with_default(
            &body,
            &format!("multipart/form-data; boundary={boundary}"),
            "",
        )
        .expect("valid multipart");
        assert_eq!(fields.model, "grok-imagine-video");

        let truncated = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video\r\n"
        );
        assert_eq!(
            native_multipart_fields_with_default(
                truncated.as_bytes(),
                &format!("multipart/form-data; boundary={boundary}"),
                "",
            )
            .unwrap_err(),
            "multipart closing boundary is missing"
        );
    }

    #[test]
    fn multipart_field_names_are_case_insensitive() {
        let boundary = "case-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"MODEL\"\r\n\r\ngrok-imagine-video\r\n--{boundary}--\r\n"
        );
        let fields = native_multipart_fields_with_default(
            body.as_bytes(),
            &format!("multipart/form-data; boundary={boundary}"),
            "",
        )
        .expect("valid multipart");
        assert_eq!(fields.model, "grok-imagine-video");
    }

    #[test]
    fn raw_dispatch_hints_require_framing_and_bound_model_sniffing() {
        assert!(raw_request_body_hint(
            "POST",
            &[("content-length".into(), "12".into())]
        ));
        assert!(!raw_request_body_hint("POST", &[]));
        assert!(!raw_request_body_hint(
            "GET",
            &[("content-length".into(), "0".into())]
        ));
        assert!(raw_request_body_hint(
            "POST",
            &[("transfer-encoding".into(), "chunked".into())]
        ));

        assert_eq!(
            raw_body_model_hint(br#" {"model":"vendor-model","x":1} "#, None).as_deref(),
            Some("vendor-model")
        );
        assert_eq!(raw_body_model_hint(b"\0{\"model\":\"nope\"}", None), None);
        assert_eq!(
            raw_body_model_hint(&vec![b' '; RAW_MODEL_SNIFF_BYTES + 1], None),
            None
        );
    }

    #[test]
    fn websocket_close_metrics_have_one_byte_contract_and_eof_is_failure() {
        let axum_close = WebSocketMessage::Close(Some(axum::extract::ws::CloseFrame {
            code: 1000,
            reason: "bye".into(),
        }));
        assert_eq!(websocket_message_size(&axum_close), 5);
        assert_eq!(websocket_message_size(&WebSocketMessage::Close(None)), 0);

        let tungstenite_close = tokio_tungstenite::tungstenite::Message::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "bye".into(),
            },
        ));
        assert_eq!(
            tungstenite_message_stats(&tungstenite_close),
            (5, Some(1000))
        );

        let counters = WebSocketRelayCounters::default();
        counters.record_error("upstream_eof_without_close", "upstream");
        let metrics = counters.snapshot(Instant::now());
        assert!(metrics.failed);
        assert!(metrics.abnormal_close);
        assert_eq!(metrics.closed_by.as_deref(), Some("upstream"));
        assert_eq!(
            metrics.relay_error.as_deref(),
            Some("upstream_eof_without_close")
        );
    }

    #[test]
    fn websocket_non_http_connect_error_has_no_upstream_status() {
        let (status, _) = websocket_connect_error(&tokio_tungstenite::tungstenite::Error::Io(
            std::io::Error::other("dial failed"),
        ));
        assert_eq!(status, 0);
    }

    #[test]
    fn websocket_first_frame_originator_upgrades_generic_attribution() {
        let frame = WebSocketMessage::Text(
            r#"{"type":"response.create","model":"gpt-4o","originator":"Codex Desktop"}"#.into(),
        );
        let metadata = websocket_message_codex_metadata(&frame).expect("frame metadata");
        let context = WebSocketEventContext {
            request_id: "request".into(),
            request_path: "/v1/responses".into(),
            route_intent: "responses_websocket".into(),
            client_kind: ClientKind::OpenaiCompat,
            model: "gpt-4o".into(),
            codex_metadata: None,
            client_declared: None,
            grok_metadata: None,
            session_id: None,
            started: Instant::now(),
        };
        let merged = websocket_context_with_first_frame(&context, Some(&metadata));
        assert_eq!(merged.client_kind, ClientKind::Codex);
        assert_eq!(
            merged
                .codex_metadata
                .as_ref()
                .and_then(|metadata| metadata.originator.as_deref()),
            Some("Codex Desktop")
        );
    }

    #[test]
    fn body_only_codex_originator_is_used_without_content_type() {
        let body = br#"{"clientMetadata":{"Originator":"Codex Desktop"}}"#;
        let value = metadata_json_body_hint(body, None).expect("JSON body hint");
        let metadata = CodexMetadata::from_request(&[], Some(&value)).expect("metadata");
        assert_eq!(metadata.originator.as_deref(), Some("Codex Desktop"));
        assert_eq!(
            ClientKind::detect_with_originator(
                Some("Mozilla/5.0"),
                metadata.originator.as_deref(),
                true,
            ),
            ClientKind::Codex
        );
        // An explicit non-JSON media type remains opaque even if its bytes
        // happen to start with a JSON object.
        assert!(metadata_json_body_hint(body, Some("application/sdp")).is_none());
    }

    #[test]
    fn live_path_uses_the_same_strict_call_target_validator() {
        let target = validate_realtime_call_target("/v1/live/rtc_1").expect("valid target");
        assert_eq!(target.unwrap().call_id, "rtc_1");
        let target = validate_realtime_call_target("/openai/v1/live/rtc-1/hangup")
            .expect("valid action target")
            .expect("target");
        assert_eq!(target.action.as_deref(), Some("hangup"));
        assert_eq!(
            validate_realtime_call_target("/v1/live/rtc%2F1"),
            Err(RealtimeCallPathError::InvalidId)
        );
        assert_eq!(
            validate_realtime_call_target("/v1/live/rtc_1/content"),
            Err(RealtimeCallPathError::UnsupportedAction)
        );
        assert_eq!(
            validate_realtime_call_target("/v1/live/"),
            Err(RealtimeCallPathError::InvalidId)
        );
    }

    #[test]
    fn resource_bindings_survive_engine_restart_and_expired_entries_are_pruned() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-engine-resource-bindings-{}",
            new_event_id()
        ));
        let dir = ConfigDir::new(root.clone());
        let first = Engine::new(
            AppConfig::bootstrap(),
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        first.register_live_session("rtc_restart", "cpa", "gpt-live-1-codex");
        first.register_video_session("video_restart", "grok", "grok-imagine-video");
        assert!(dir.load_resource_bindings().unwrap().len() == 2);
        drop(first);

        let second = Engine::new(
            AppConfig::bootstrap(),
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        assert_eq!(
            second.live_session_endpoint("/v1/live/rtc_restart"),
            Some(Some("cpa".into()))
        );
        assert_eq!(
            second.live_session_model("/v1/live/rtc_restart"),
            Some(Some("gpt-live-1-codex".into()))
        );
        assert_eq!(
            second.video_session_endpoint("/v1/videos/video_restart/content"),
            Some(Some("grok".into()))
        );
        drop(second);

        let mut expired = std::collections::HashMap::new();
        expired.insert(
            "live:expired".into(),
            ResourceBinding {
                endpoint_id: "cpa".into(),
                model: "gpt-live-1-codex".into(),
                expires_at: 1.0,
            },
        );
        let _ = dir.save_resource_bindings(&expired).unwrap();
        let third = Engine::new(
            AppConfig::bootstrap(),
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        assert_eq!(third.live_session_endpoint("/v1/live/expired"), Some(None));
        assert!(dir.load_resource_bindings().unwrap().is_empty());
        drop(third);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn public_realtime_path_is_not_treated_as_codex_live() {
        assert!(is_codex_live_path("/v1/live"));
        assert!(!is_codex_live_path("/v1/realtime/calls"));
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/live",
                None,
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime",
                Some(request_build::DEFAULT_REALTIME_MODEL),
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "GET",
                "/v1/realtime?model=gpt-realtime",
                Some(request_build::DEFAULT_REALTIME_MODEL),
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::StandardRealtime
        );
        assert_eq!(
            classify_realtime_intent(
                "GET",
                "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver",
                Some(request_build::DEFAULT_CODEX_LIVE_MODEL),
                None,
                None,
                ClientKind::Codex,
                true,
            ),
            RealtimeRouteIntent::StandardRealtime
        );
        assert_eq!(
            classify_realtime_intent(
                "GET",
                "/v1/realtime?architecture=avas",
                None,
                None,
                None,
                ClientKind::Codex,
                false,
            ),
            RealtimeRouteIntent::StandardRealtime
        );
        assert_eq!(
            classify_realtime_intent(
                "GET",
                "/v1/realtime?call_id=call-1",
                None,
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime/calls",
                None,
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime?model=claude-fable-5",
                Some("claude-fable-5"),
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime?model=claude-fable-5",
                Some("claude-fable-5"),
                None,
                None,
                ClientKind::Codex,
                false,
            ),
            RealtimeRouteIntent::CodexLive
        );
        assert_eq!(
            classify_realtime_intent(
                "GET",
                "/v1/realtime?model=claude-fable-5",
                Some("claude-fable-5"),
                None,
                None,
                ClientKind::Codex,
                false,
            ),
            RealtimeRouteIntent::StandardRealtime
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime/client_secrets",
                Some("gpt-4o"),
                None,
                None,
                ClientKind::OpenaiCompat,
                false,
            ),
            RealtimeRouteIntent::StandardRealtime
        );
        assert_eq!(
            classify_realtime_intent(
                "POST",
                "/v1/realtime/calls/call-1/hangup",
                Some(request_build::DEFAULT_CODEX_LIVE_MODEL),
                None,
                None,
                ClientKind::Codex,
                true,
            ),
            RealtimeRouteIntent::CodexLive
        );
    }

    #[test]
    fn voice_bootstrap_ignores_leaked_chat_models() {
        assert_eq!(
            realtime_voice_route_model(
                "POST",
                "/v1/realtime?model=claude-fable-5",
                Some("claude-fable-5".into()),
                None,
                None,
            ),
            request_build::DEFAULT_CODEX_LIVE_MODEL
        );
        assert_eq!(
            realtime_voice_route_model(
                "GET",
                "/v1/realtime?model=claude-fable-5",
                Some("claude-fable-5".into()),
                None,
                None,
            ),
            "claude-fable-5"
        );
        assert_eq!(
            realtime_voice_route_model(
                "POST",
                "/v1/realtime/client_secrets",
                Some("gpt-4o".into()),
                None,
                None,
            ),
            "gpt-4o"
        );
        assert_eq!(
            realtime_voice_route_model(
                "GET",
                "/v1/realtime?model=gpt-4o",
                Some("gpt-4o".into()),
                None,
                Some("gpt-4o".into()),
            ),
            "gpt-4o"
        );
    }

    #[test]
    fn cpa_live_targets_accept_header_and_quicksilver_call_ids() {
        assert_eq!(
            live_call_id_from_headers(&[("X-Live-Session".into(), "call-header".into())])
                .as_deref(),
            Some("call-header")
        );
        assert_eq!(
            live_call_id_from_headers(&[
                ("X-Call-ID".into(), "fallback-call".into()),
                ("X-Live-Session".into(), "live-session".into()),
                ("X-Live-Call-ID".into(), "live-call".into()),
            ])
            .as_deref(),
            Some("live-call")
        );
        assert_eq!(
            live_call_id_from_target("/v1/live/call-path").as_deref(),
            Some("call-path")
        );
        assert_eq!(
            live_call_id_from_target("/v1/realtime?intent=quicksilver&call_id=call-query")
                .as_deref(),
            Some("call-query")
        );
        assert_eq!(
            live_call_id_from_target("/v1/realtime?intent=quicksilver&call_id=rtc_1").as_deref(),
            Some("rtc_1")
        );
        assert_eq!(
            live_call_id_from_headers(&[(
                "Location".into(),
                "https://provider.invalid/v1/realtime?intent=quicksilver&call_id=call-location"
                    .into(),
            )])
            .as_deref(),
            Some("call-location")
        );
        assert!(is_codex_live_sideband_target(
            "/v1/realtime?intent=quicksilver&call_id=call-query"
        ));
        assert!(is_codex_live_sideband_target(
            "/v1/realtime?call_id=ordinary-realtime"
        ));
        assert!(!unsupported_realtime_call_action(
            "/v1/realtime/calls/rtc_1/hangup"
        ));
        assert!(!unsupported_realtime_call_action(
            "/v1/realtime/calls/rtc_1"
        ));
        assert!(unsupported_realtime_call_action(
            "/v1/realtime/calls/rtc_1/unknown"
        ));
        assert_eq!(
            validate_realtime_call_target("/v1/realtime/calls/%%%").unwrap_err(),
            RealtimeCallPathError::InvalidId
        );
        assert_eq!(
            validate_realtime_call_target("/v1/realtime/calls/").unwrap_err(),
            RealtimeCallPathError::InvalidId
        );
        assert_eq!(
            validate_realtime_call_target("/v1/realtime/calls/call-1/hangup/extra").unwrap_err(),
            RealtimeCallPathError::UnsupportedAction
        );
        assert_eq!(
            validate_realtime_call_target("/v1/realtime?call_id=call%2F1").unwrap_err(),
            RealtimeCallPathError::InvalidId
        );
        let target = validate_realtime_call_target("/v1/realtime/calls/call%2D1/hangup")
            .expect("decoded call target")
            .expect("call target");
        assert_eq!(target.call_id, "call-1");
        assert_eq!(target.action.as_deref(), Some("hangup"));
        assert_eq!(
            live_call_id_from_json(br#"{"object":"realtime.call","id":"rtc_1"}"#).as_deref(),
            Some("rtc_1")
        );
    }

    #[test]
    fn realtime_body_model_reads_session_model_without_guessing_text_model() {
        assert_eq!(
            realtime_body_model(
                br#"{"session":{"model":"gpt-realtime"}}"#,
                Some("application/json")
            )
            .as_deref(),
            Some("gpt-realtime")
        );
        assert_eq!(realtime_body_model(b"{}", Some("application/sdp")), None);
    }

    #[test]
    fn resource_paths_preserve_inbound_aliases_and_avoid_duplicate_base_v1() {
        assert_eq!(
            request_build::openai_resource_path("", "/files/file_123/content?x=1"),
            "/files/file_123/content?x=1"
        );
        assert_eq!(
            request_build::openai_resource_path("/v1", "/v1/videos/video_123"),
            "/v1/videos/video_123"
        );
        assert_eq!(
            request_build::openai_resource_path("/gateway", "/openai/v1/files"),
            "/gateway/openai/v1/files"
        );
        assert_eq!(
            request_build::openai_resource_path("/gateway/v1", "/v1foo"),
            "/gateway/v1/v1foo"
        );
    }

    #[test]
    fn realtime_websocket_does_not_forward_listener_or_project_headers() {
        let forwarded = websocket_upstream_headers(&[
            ("authorization".into(), "Bearer listener-secret".into()),
            ("x-sumpter-project".into(), "private-workspace".into()),
            ("x-sumpter-user".into(), "kkl".into()),
            ("openai-beta".into(), "realtime=v1".into()),
            ("sec-websocket-protocol".into(), "realtime".into()),
        ]);
        assert!(
            !forwarded
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        );
        assert!(
            !forwarded
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("x-sumpter-project")
                    || name.eq_ignore_ascii_case("x-sumpter-user"))
        );
        assert!(forwarded.iter().any(
            |(name, value)| name.eq_ignore_ascii_case("openai-beta") && value == "realtime=v1"
        ));
        assert!(forwarded.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("sec-websocket-protocol") && value == "realtime"
        }));
    }

    #[test]
    fn realtime_websocket_url_preserves_base_path_without_duplicate_v1() {
        assert_eq!(
            websocket_url(
                "https://provider.invalid/v1",
                "/v1/realtime?model=gpt-realtime"
            ),
            Some("wss://provider.invalid/v1/realtime?model=gpt-realtime".into())
        );
        assert_eq!(
            websocket_url("http://127.0.0.1:9000", "/realtime"),
            Some("ws://127.0.0.1:9000/realtime".into())
        );
        assert_eq!(
            websocket_url(
                "https://provider.invalid/v1",
                "/backend-api/codex/responses?model=gpt-4o"
            ),
            Some("wss://provider.invalid/v1/backend-api/codex/responses?model=gpt-4o".into())
        );
        assert_eq!(
            websocket_url(
                "https://ccc.domob.org",
                &request_build::strip_codex_live_query(
                    "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver&architecture=avas",
                ),
            ),
            Some("wss://ccc.domob.org/v1/realtime?model=gpt-live-1-codex".into())
        );
    }

    #[test]
    fn codex_model_reasoning_levels_follow_client_version() {
        assert!(!supports_extended_reasoning_levels("0.143.9"));
        assert!(supports_extended_reasoning_levels("0.144.0"));
        assert!(supports_extended_reasoning_levels("v0.149.1"));
        assert!(supports_extended_reasoning_levels(""));
        assert!(supports_extended_reasoning_levels("latest"));
        assert!(supports_extended_reasoning_levels("0"));
    }

    fn catalog_config() -> AppConfig {
        AppConfig::from_json(
            r#"{
              "schemaVersion": 6,
              "listener": {"host":"127.0.0.1","port":0},
              "endpoints": [{
                "id": "cpa",
                "name": "CPA",
                "baseURL": "https://cpa.example.invalid",
                  "apiKey": "sk",
                  "protocol": "openai",
                  "enabled": true,
                  "catalog": {
                    "models": ["gpt-4o", "gpt-5.6-sol", "gpt-image-2", "gpt-ignored-other"]
                  },
                  "mappings": [
                  {"clientPattern": "gpt-5.6-sol", "upstreamModel": "gpt-5.6-sol"},
                  {"clientPattern": "gpt-4o", "upstreamModel": "gpt-4o"},
                  {"clientPattern": "*", "upstreamModel": "star"},
                  {"clientPattern": "gpt-image-2", "upstreamModel": "gpt-image-2"}
                ]
              }]
            }"#,
        )
        .expect("catalog fixture")
        .normalized()
    }

    #[test]
    fn local_models_catalog_uses_openai_list_shape() {
        let json = local_models_json(&catalog_config(), "/v1/models", None, &[]).expect("catalog");
        assert_eq!(json["object"], "list");
        let ids: Vec<&str> = json["data"]
            .as_array()
            .expect("data")
            .iter()
            .filter_map(|model| model["id"].as_str())
            .collect();
        assert_eq!(
            ids,
            vec!["gpt-4o", "gpt-5.6-sol", "gpt-ignored-other", "gpt-image-2"]
        );
        assert!(json["data"][0].get("endpointIDs").is_none());
        assert_eq!(json["data"][0]["capabilities"], json!(["text"]));
        assert_eq!(json["data"][0]["owned_by"], "sumpter");
    }

    #[test]
    fn local_models_catalog_uses_codex_client_version_shape() {
        let json = local_models_json(
            &catalog_config(),
            "/v1/models",
            Some("client_version=0.149.1"),
            &[],
        )
        .expect("catalog");
        assert!(json.get("data").is_none());
        let models = json["models"].as_array().expect("models");
        let chat = models
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .expect("chat model");
        assert_eq!(chat["display_name"], "gpt-5.6-sol");
        assert_eq!(chat["default_reasoning_level"], "medium");
        let gpt4o = models
            .iter()
            .find(|model| model["slug"] == "gpt-4o")
            .expect("gpt-4o stays a chat model");
        assert_ne!(
            gpt4o.get("visibility").and_then(Value::as_str),
            Some("hide")
        );
        let efforts: Vec<&str> = chat["supported_reasoning_levels"]
            .as_array()
            .expect("levels")
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect();
        assert!(efforts.contains(&"xhigh"));
        assert!(efforts.contains(&"max"));
        let image = models
            .iter()
            .find(|model| model["slug"] == "gpt-image-2")
            .expect("image model");
        assert_eq!(image["visibility"], "hide");
        assert!(models.iter().all(|model| model["slug"] != "*"));
    }

    #[test]
    fn local_models_catalog_hides_extended_reasoning_for_old_codex() {
        let json = local_models_json(
            &catalog_config(),
            "/v1/models",
            Some("client_version=0.143.9"),
            &[],
        )
        .expect("catalog");
        let chat = json["models"]
            .as_array()
            .expect("models")
            .iter()
            .find(|model| model["slug"] == "gpt-5.6-sol")
            .expect("chat model");
        let efforts: Vec<&str> = chat["supported_reasoning_levels"]
            .as_array()
            .expect("levels")
            .iter()
            .filter_map(|level| level["effort"].as_str())
            .collect();
        assert!(efforts.contains(&"xhigh"));
        assert!(!efforts.contains(&"max"));
        assert!(!efforts.contains(&"ultra"));
    }

    #[test]
    fn local_models_catalog_lookup_and_unknown_id() {
        let found = local_models_json(&catalog_config(), "/v1/models/gpt-5.6-sol", None, &[])
            .expect("known model");
        assert_eq!(found["id"], "gpt-5.6-sol");
        assert_eq!(
            local_models_json(&catalog_config(), "/v1/models/not-a-model", None, &[]).unwrap_err(),
            StatusCode::NOT_FOUND
        );
        let cursor = local_models_json(&catalog_config(), "/v1/models", Some("cursor=next"), &[])
            .expect("cursor stays local");
        assert_eq!(cursor["object"], "list");
        let grok = local_models_json(
            &catalog_config(),
            "/v1/models",
            None,
            &[("user-agent".into(), "grok-shell/1.0".into())],
        )
        .expect("grok catalog");
        assert_eq!(grok["data"][0]["api_backend"], "responses");
        let claude = local_models_json(
            &catalog_config(),
            "/v1/models",
            None,
            &[("anthropic-version".into(), "2023-06-01".into())],
        )
        .expect("anthropic catalog");
        assert_eq!(claude["has_more"], false);
        assert!(claude["data"].as_array().is_some());
        let decoded = local_models_json(
            &catalog_config(),
            "/v1/models",
            Some("client_version=0.149.1"),
            &[],
        )
        .expect("plain version");
        let encoded = local_models_json(
            &catalog_config(),
            "/v1/models",
            Some("client_version=0%2E149%2E1"),
            &[],
        )
        .expect("encoded version");
        assert_eq!(decoded["models"][0]["default_reasoning_level"], "medium");
        assert_eq!(encoded["models"][0]["default_reasoning_level"], "medium");
    }

    #[test]
    fn local_models_catalog_resolves_each_capability_independently() {
        let config: AppConfig = serde_json::from_value::<AppConfig>(json!({
            "schemaVersion": 6,
            "listener": {"host": "127.0.0.1", "port": 0},
            "endpoints": [{
                "id": "mixed",
                "name": "mixed",
                "baseURL": "https://provider.invalid",
                "protocol": "openai",
                "enabled": true,
                "apiKey": "",
                "catalog": {"models": ["gpt-image-2"]},
                "mappings": [
                    {"clientPattern": "gpt-image-2", "capabilities": ["text"]},
                    {"clientPattern": "gpt-image-*", "capabilities": ["image"]}
                ]
            }]
        }))
        .expect("catalog config")
        .normalized();
        let all = local_models_json(&config, "/v1/models", None, &[]).expect("catalog");
        let entry = all["data"]
            .as_array()
            .and_then(|models| models.iter().find(|model| model["id"] == "gpt-image-2"))
            .expect("image model");
        let capabilities = entry["capabilities"].as_array().expect("capabilities");
        assert!(capabilities.iter().any(|value| value == "text"));
        assert!(capabilities.iter().any(|value| value == "image"));

        let image_only = local_models_json(&config, "/v1/models", Some("capability=image"), &[])
            .expect("image catalog");
        assert_eq!(image_only["data"][0]["id"], "gpt-image-2");
    }
}
#[derive(Debug, PartialEq, Eq)]
struct NativePassthroughFields {
    model: String,
    stream: bool,
}

fn realtime_body_model(body: &[u8], content_type: Option<&str>) -> Option<String> {
    let content_type = content_type?;
    if !content_type_is_json(content_type) {
        return None;
    }
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let object = value.as_object()?;
    [
        json_object_value(object, "model"),
        json_object_value(object, "session")
            .and_then(Value::as_object)
            .and_then(|session| json_object_value(session, "model")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|model| !model.is_empty())
    .map(str::to_string)
}

fn realtime_body_model_hint(body: &[u8], content_type: Option<&str>) -> Option<String> {
    if content_type.is_some_and(content_type_is_json) {
        return realtime_body_model(body, content_type);
    }
    if content_type.is_some_and(content_type_is_multipart) {
        return native_multipart_fields_with_default(body, content_type.unwrap_or_default(), "")
            .ok()
            .map(|fields| fields.model)
            .filter(|model| !model.trim().is_empty());
    }
    None
}

/// Apply the session configuration bound to an ephemeral Realtime key to a
/// subsequent `/v1/realtime/calls` request.  This is the HTTP counterpart of
/// CPA's `session.update` WebSocket frame and preserves SDP callers as well as
/// JSON callers.
fn apply_realtime_client_secret_session(
    body: &[u8],
    content_type: &str,
    session: &Value,
) -> Result<(Bytes, String), &'static str> {
    let media_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(media_type.as_str(), "application/sdp" | "text/plain") {
        let sdp = std::str::from_utf8(body).map_err(|_| "Realtime SDP must be UTF-8")?;
        if sdp.trim().is_empty() {
            return Err("Realtime call request requires an SDP offer");
        }
        let value = json!({"sdp": sdp, "session": session});
        let encoded = serde_json::to_vec(&value).map_err(|_| "Realtime session encoding failed")?;
        return Ok((Bytes::from(encoded), "application/json".into()));
    }
    if media_type.is_empty() || media_type == "application/json" {
        let mut value: Value = serde_json::from_slice(body)
            .map_err(|_| "Realtime call request body must be valid JSON")?;
        let object = value
            .as_object_mut()
            .ok_or("Realtime call request body must be an object")?;
        object.insert("session".into(), session.clone());
        let encoded = serde_json::to_vec(&value).map_err(|_| "Realtime session encoding failed")?;
        return Ok((Bytes::from(encoded), "application/json".into()));
    }
    Err("Realtime client secrets require an SDP or JSON call request")
}

fn realtime_client_secret_request_session(path_and_query: &str, body: &[u8]) -> Option<Value> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let path = path_without_query(path_and_query);
    let session = if matches!(
        path,
        "/v1/realtime/client_secrets"
            | "/realtime/client_secrets"
            | "/openai/v1/realtime/client_secrets"
            | "/backend-api/codex/realtime/client_secrets"
    ) {
        value.get("session")?.as_object()?.clone()
    } else if matches!(
        path,
        "/v1/realtime/sessions"
            | "/realtime/sessions"
            | "/openai/v1/realtime/sessions"
            | "/backend-api/codex/realtime/sessions"
    ) {
        value.as_object()?.clone()
    } else {
        return None;
    };
    let mut session = session;
    for field in ["id", "object", "expires_at", "client_secret"] {
        session.remove(field);
    }
    Some(Value::Object(session))
}

fn content_type_is_json(content_type: &str) -> bool {
    content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with("application/json")
}

fn content_type_is_multipart(content_type: &str) -> bool {
    content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
}

/// Whether an unknown Raw request is plausibly carrying a body.  We cannot
/// peek an `axum::Body` without consuming it, so use the HTTP framing headers
/// (and the body-capable method) as a bounded dispatch hint.  An explicit
/// `Content-Length: 0` is respected; a non-empty content type or chunked
/// transfer still gets to the native handler, where the model requirement is
/// enforced.  This keeps body-only JSON model requests working even when a
/// client forgot `Content-Type`, while GET/HEAD probes without a body remain a
/// cheap 404.
fn raw_request_body_hint(method: &str, headers: &[(String, String)]) -> bool {
    if let Some(length) = header_value(headers, "content-length") {
        if let Ok(length) = length.trim().parse::<u64>() {
            if length > 0 {
                return true;
            }
            // An explicit zero Content-Length is authoritative for normal
            // requests. Transfer-Encoding is the one exception: chunked
            // framing may still carry a body and should reach the handler.
            return header_value(headers, "transfer-encoding")
                .is_some_and(|value| !value.trim().is_empty());
        }
        // Invalid framing must not make us discard a request that may contain
        // a model; the body limit and parser provide the final validation.
        return true;
    }
    if header_value(headers, "transfer-encoding").is_some_and(|value| !value.trim().is_empty())
        || header_value(headers, "content-type").is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    // Without framing headers there is no safe way to know whether an
    // unknown-path request actually carries a body without polling/consuming
    // it. Keep the ingress preflight cheap (and preserve a real 404 for
    // bodyless probes); clients that send a body will normally provide either
    // Content-Length or Transfer-Encoding, both handled above. The method is
    // intentionally not used as a guess here.
    let _ = method;
    false
}

/// Read only the top-level JSON `model` field used to select a Raw mapping.
/// The payload itself is never serialized back or changed.  Sniffing is
/// limited to a small prefix and only attempted for JSON-looking bytes (or an
/// explicit JSON content type), avoiding a full parse of opaque/binary uploads.
const RAW_MODEL_SNIFF_BYTES: usize = 1024 * 1024;

/// Bounded JSON sniff used only for client attribution and route hints.  An
/// explicit non-JSON media type wins over a JSON-looking prefix; without a
/// media type we require the first non-whitespace byte to be `{`.
fn metadata_json_body_hint(body: &[u8], content_type: Option<&str>) -> Option<Value> {
    const MAX_BYTES: usize = sumpter_core::events::CODEX_METADATA_MAX_JSON_BYTES;
    if body.is_empty() || body.len() > MAX_BYTES {
        return None;
    }
    if let Some(content_type) = content_type {
        if !content_type_is_json(content_type) {
            return None;
        }
    } else if body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return None;
    }
    serde_json::from_slice::<Value>(body).ok()
}

fn raw_body_model_hint(body: &[u8], content_type: Option<&str>) -> Option<String> {
    if body.is_empty() || body.len() > RAW_MODEL_SNIFF_BYTES {
        return None;
    }
    let first = body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if !content_type.is_some_and(content_type_is_json) && first != Some(b'{') {
        return None;
    }
    serde_json::from_slice::<Value>(body)
        .ok()?
        .as_object()?
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

fn native_json_fields(
    body: &[u8],
    kind: PassthroughKind,
) -> Result<NativePassthroughFields, &'static str> {
    let value: Value = serde_json::from_slice(body).map_err(|_| "body is not JSON")?;
    let object = value.as_object().ok_or("body is not an object")?;
    let default_model = match kind {
        PassthroughKind::ImagesGenerations | PassthroughKind::ImagesEdits => "gpt-image-2",
        PassthroughKind::Realtime => {
            // Live is normalized before this helper and carries its model in
            // `session.model`; standard Realtime gets its own deterministic
            // default rather than inheriting an arbitrary text mapping.
            request_build::DEFAULT_REALTIME_MODEL
        }
        _ => "",
    };
    let model = object
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            // Realtime client-secrets requests carry the model under
            // `session.model`; retain that shape while still using it for
            // endpoint mapping.
            (kind == PassthroughKind::Realtime)
                .then(|| object.get("session"))
                .flatten()
                .and_then(Value::as_object)
                .and_then(|session| session.get("model"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
        })
        .unwrap_or(default_model)
        .to_string();
    if model.is_empty() {
        return Err("model is required");
    }
    Ok(NativePassthroughFields {
        model,
        stream: matches!(
            kind,
            PassthroughKind::Completions
                | PassthroughKind::ImagesGenerations
                | PassthroughKind::ImagesEdits
        ) && object
            .get("stream")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn native_multipart_fields(
    body: &[u8],
    content_type: &str,
) -> Result<NativePassthroughFields, &'static str> {
    native_multipart_fields_with_default(body, content_type, "gpt-image-2")
}

fn native_multipart_fields_with_default(
    body: &[u8],
    content_type: &str,
    default_model: &str,
) -> Result<NativePassthroughFields, &'static str> {
    let mut model = None;
    let mut stream = false;
    for (headers, field) in multipart_parts(body, content_type)? {
        if let Some(name) = multipart_field_name(headers)
            && matches!(name.as_str(), "model" | "stream")
        {
            if field.len() > 1024 {
                return Err("multipart routing field is too large");
            }
            let value = std::str::from_utf8(field)
                .map_err(|_| "multipart routing field is not UTF-8")?
                .trim();
            match name.as_str() {
                "model" if !value.is_empty() => model = Some(value.to_string()),
                "stream" => {
                    stream = matches!(
                        value.to_ascii_lowercase().as_str(),
                        "1" | "true" | "yes" | "on"
                    )
                }
                _ => {}
            }
        }
    }
    Ok(NativePassthroughFields {
        model: model.unwrap_or_else(|| default_model.to_string()),
        stream,
    })
}

#[derive(Clone, Copy)]
struct MultipartDelimiter {
    start: usize,
    content_start: usize,
    closing: bool,
}

type MultipartPart<'a> = (&'a [u8], &'a [u8]);

/// Parse multipart framing without ever rebuilding the body. A boundary-like
/// byte sequence inside uploaded binary data is accepted as a delimiter only
/// when it begins a MIME line and is followed by a valid delimiter suffix.
/// The final closing delimiter is mandatory so a truncated upload cannot
/// silently yield a misleading routing model.
fn multipart_parts<'a>(
    body: &'a [u8],
    content_type: &str,
) -> Result<Vec<MultipartPart<'a>>, &'static str> {
    let boundary = multipart_boundary(content_type).ok_or("multipart boundary is required")?;
    let marker = format!("--{boundary}").into_bytes();
    let mut delimiter = next_multipart_delimiter(body, &marker, 0)
        .ok_or("multipart opening boundary is missing")?;
    let mut parts = Vec::new();
    loop {
        if delimiter.closing {
            return Ok(parts);
        }
        let part_start = delimiter.content_start;
        let Some((header_offset, separator_len)) = multipart_header_end(&body[part_start..]) else {
            return Err("malformed multipart headers");
        };
        if header_offset > 16 * 1024 {
            return Err("multipart headers are too large");
        }
        let headers = &body[part_start..part_start + header_offset];
        let content_start = part_start + header_offset + separator_len;
        let next = next_multipart_delimiter(body, &marker, content_start)
            .ok_or("multipart closing boundary is missing")?;
        let content_end =
            if next.start >= 2 && body.get(next.start - 2..next.start) == Some(b"\r\n") {
                next.start - 2
            } else if next.start >= 1 && body.get(next.start - 1) == Some(&b'\n') {
                next.start - 1
            } else {
                next.start
            };
        if content_end < content_start {
            return Err("malformed multipart body");
        }
        parts.push((headers, &body[content_start..content_end]));
        delimiter = next;
    }
}

fn next_multipart_delimiter(body: &[u8], marker: &[u8], from: usize) -> Option<MultipartDelimiter> {
    if marker.is_empty() || from > body.len() {
        return None;
    }
    let mut cursor = from;
    while cursor <= body.len().saturating_sub(marker.len()) {
        let relative = find_bytes(&body[cursor..], marker)?;
        let start = cursor + relative;
        let at_line_start = start == 0
            || (start >= 2 && body.get(start - 2..start) == Some(b"\r\n"))
            || (start >= 1 && body.get(start - 1) == Some(&b'\n'));
        if !at_line_start {
            cursor = start.saturating_add(1);
            continue;
        }
        let suffix = start + marker.len();
        if body.get(suffix..suffix + 2) == Some(b"--") {
            let after = suffix + 2;
            if after == body.len()
                || body.get(after..after + 2) == Some(b"\r\n")
                || body.get(after) == Some(&b'\n')
            {
                return Some(MultipartDelimiter {
                    start,
                    content_start: after,
                    closing: true,
                });
            }
        } else if body.get(suffix..suffix + 2) == Some(b"\r\n") {
            return Some(MultipartDelimiter {
                start,
                content_start: suffix + 2,
                closing: false,
            });
        } else if body.get(suffix) == Some(&b'\n') {
            return Some(MultipartDelimiter {
                start,
                content_start: suffix + 1,
                closing: false,
            });
        }
        cursor = start.saturating_add(1);
    }
    None
}

fn multipart_boundary(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .skip(1)
        .find_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            name.trim().eq_ignore_ascii_case("boundary").then(|| {
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string()
            })
        })
        .filter(|value| !value.is_empty())
}

fn multipart_header_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let crlf = find_bytes(bytes, b"\r\n\r\n").map(|offset| (offset, 4));
    let lf = find_bytes(bytes, b"\n\n").map(|offset| (offset, 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn multipart_field_name(headers: &[u8]) -> Option<String> {
    let headers = std::str::from_utf8(headers).ok()?;
    let disposition = headers.lines().find_map(|line| {
        let (name, value) = line.trim_end_matches('\r').split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("content-disposition")
            .then_some(value)
    })?;
    disposition.split(';').skip(1).find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.trim().eq_ignore_ascii_case("name").then(|| {
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_ascii_lowercase()
        })
    })
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty() && haystack.len() >= needle.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

fn valid_anthropic_messages_request(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.trim().is_empty())
            && object.get("messages").is_some_and(Value::is_array)
    })
}
