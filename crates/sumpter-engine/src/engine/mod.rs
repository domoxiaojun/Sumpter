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

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
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
use sumpter_core::config_store::{ConfigDir, StickySessionAssignment};
use sumpter_core::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, DiagnosticAttemptCapture,
    DiagnosticCaptureSnapshot, DiagnosticChunk, DiagnosticHeader, DiagnosticRequestCapture,
    KIND_CLIENT, KIND_UPSTREAM, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase,
    RuntimeFailureKind, RuntimeFailurePhase, RuntimeSnapshot, STATUS_CLIENT_DISCONNECTED,
    StreamTrace, message_tokens, unix_to_apple_epoch,
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
const SESSION_STICKY_TTL_SECS: f64 = 30.0 * 24.0 * 3600.0;
const SESSION_STICKY_PRUNE_INTERVAL_SECS: f64 = 60.0;
pub const DEFAULT_CAPTURE_MAX_BYTES: usize = 512 * 1024 * 1024;
const MAX_CAPTURE_INDEX_RECORDS: usize = 200;
const CAPTURE_STOP_MANUAL: &str = "manual";
const CAPTURE_STOP_CAPACITY: &str = "capacity_limit";

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
    upstream: NativeWebSocket,
}

#[derive(Debug)]
pub struct WebSocketPrepareError {
    status: u16,
    code: &'static str,
    message: String,
}

impl WebSocketPrepareError {
    fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
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
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap_or_else(|_| Response::new(Body::empty()))
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
    ip_rotation: i64,
    /// affinityID → 调度组；稳定会话不超时并持久化，内容指纹仅进程内兼容。
    session_sticky: HashMap<String, SessionStickyEntry>,
    last_session_prune_at: f64,
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
        Self {
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
                    ip_rotation: 0,
                    session_sticky,
                    last_session_prune_at: now,
                }),
                dir,
                stats_writable: AtomicBool::new(stats_writable),
                session_affinity_flush: Mutex::new(()),
                session_affinity_dirty: AtomicBool::new(session_affinity_pruned),
                session_affinity_writable: AtomicBool::new(session_affinity_writable),
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
                live_sessions: Mutex::new(HashMap::new()),
                video_sessions: Mutex::new(HashMap::new()),
            }),
        }
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
        if call_id.is_empty()
            || call_id.len() > 256
            || call_id.chars().any(char::is_control)
            || endpoint_id.trim().is_empty()
        {
            return;
        }
        let now = now_unix();
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

    /// Return the originating endpoint for a sideband target. The outer
    /// `Option` indicates whether the target carried a call id; the inner
    /// value is `None` when that id is unknown or expired. Keeping those
    /// states distinct is important: an unknown Live id must not silently
    /// fall back to ordinary provider selection.
    fn live_session_endpoint(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        sessions.retain(|_, entry| entry.expires_at > now);
        Some(sessions.get(call_id).map(|entry| entry.endpoint_id.clone()))
    }

    fn live_session_model(&self, path_and_query: &str) -> Option<Option<String>> {
        let call_id = live_call_id_from_target(path_and_query)?;
        let now = now_unix();
        let mut sessions = self.inner.live_sessions.lock().unwrap();
        sessions.retain(|_, entry| entry.expires_at > now);
        Some(sessions.get(call_id).map(|entry| entry.model.clone()))
    }

    fn register_video_session(&self, video_id: &str, endpoint_id: &str, model: &str) {
        self.register_live_session_into(&self.inner.video_sessions, video_id, endpoint_id, model);
    }

    fn register_live_session_into(
        &self,
        store: &Mutex<HashMap<String, LiveSessionEntry>>,
        session_id: &str,
        endpoint_id: &str,
        model: &str,
    ) {
        let session_id = session_id.trim();
        if session_id.is_empty()
            || session_id.len() > 256
            || session_id.chars().any(char::is_control)
            || endpoint_id.trim().is_empty()
        {
            return;
        }
        let now = now_unix();
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
                expires_at: now + DEFAULT_LIVE_SESSION_TTL_SECS,
                endpoint_id: endpoint_id.trim().to_string(),
                model: model.trim().to_string(),
            },
        );
    }

    fn video_session_endpoint(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        sessions.retain(|_, entry| entry.expires_at > now);
        Some(
            sessions
                .get(video_id)
                .map(|entry| entry.endpoint_id.clone()),
        )
    }

    fn video_session_model(&self, path: &str) -> Option<Option<String>> {
        let video_id = video_id_from_path(path)?;
        let now = now_unix();
        let mut sessions = self.inner.video_sessions.lock().unwrap();
        sessions.retain(|_, entry| entry.expires_at > now);
        Some(sessions.get(video_id).map(|entry| entry.model.clone()))
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

    fn record_event(&self, event: RuntimeEvent) {
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
                event.codex_metadata = event.codex_metadata.or(client.codex_metadata);
                event.client_declared = event.client_declared.or(client.client_declared);
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
        let event_id = new_event_id();
        let event = RuntimeEvent {
            client_kind: Some(client_kind),
            codex_metadata,
            client_declared,
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
            failure_detail: Some(message.to_string()),
            failure_kind: Some(RuntimeFailureKind::ClientRequestRejected),
            failure_phase: Some(RuntimeFailurePhase::BeforeResponse),
            id: event_id.clone(),
            kind: KIND_CLIENT.into(),
            message: Some(message.to_string()),
            tool_calls: None,
            outcome: Some(RuntimeEventOutcome::Failed),
            phase: Some(RuntimeEventPhase::Completed),
            pool_id: None,
            request_purpose: purpose,
            request_id: Some(event_id),
            session_id: None,
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
        self.complete_client(event, Some(message.to_string()));
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

    /// Dial a standard Realtime/Live upstream before the adapter sends the
    /// downstream 101 response.  Responses WebSocket connections without a
    /// query model still use the deferred path because their model is carried
    /// by the first post-upgrade `response.create` frame.
    pub async fn prepare_realtime_websocket(
        &self,
        path_and_query: &str,
        headers: &[(String, String)],
    ) -> Result<PreparedWebSocket, WebSocketPrepareError> {
        let path = path_without_query(path_and_query);
        if !is_realtime_http_path(path) {
            return Err(WebSocketPrepareError::new(
                400,
                "invalid_request",
                "websocket path is not a Realtime endpoint",
            ));
        }
        let sideband_model = self
            .live_session_model(path_and_query)
            .flatten()
            .filter(|model| !model.trim().is_empty());
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| query_value(query, "model"))
            .filter(|model| !model.trim().is_empty())
            .map(str::to_string);
        let secret_model = self.realtime_client_secret_model(headers);
        let model = realtime_voice_route_model(
            "GET",
            path_and_query,
            query_model,
            sideband_model,
            secret_model,
        );
        let request = RoutingRequest::from_value(&json!({"model": model})).ok_or_else(|| {
            WebSocketPrepareError::new(400, "invalid_request", "invalid websocket model")
        })?;
        let upstream = self
            .connect_native_websocket(path, path_and_query, headers, &request)
            .await?;
        Ok(PreparedWebSocket { upstream })
    }

    /// Relay an already-handshaken upstream after the adapter completes the
    /// downstream upgrade.
    pub async fn handle_prepared_websocket(
        &self,
        socket: WebSocket,
        remote: Option<IpAddr>,
        prepared: PreparedWebSocket,
    ) {
        relay_native_websocket(socket, prepared.upstream, None).await;
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
        let mut initial_message = None;
        let query_model = path_and_query
            .split_once('?')
            .and_then(|(_, query)| query_value(query, "model"))
            .filter(|model| !model.trim().is_empty())
            .map(str::to_string);
        let sideband_model = self
            .live_session_model(&path_and_query)
            .flatten()
            .filter(|model| !model.trim().is_empty());
        let secret_model = self.realtime_client_secret_model(&headers);
        let model = if is_realtime_http_path(path) {
            Some(realtime_voice_route_model(
                "GET",
                &path_and_query,
                query_model,
                sideband_model,
                secret_model,
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
            let _ = send_websocket_json_error(
                &mut socket,
                "invalid_request",
                "invalid websocket model",
            )
            .await;
            return;
        };
        let upstream = match self
            .connect_native_websocket(path, &path_and_query, &headers, &request)
            .await
        {
            Ok(upstream) => upstream,
            Err(error) => {
                let _ = send_websocket_json_error(&mut socket, error.error_code(), error.message())
                    .await;
                return;
            }
        };
        relay_native_websocket(socket, upstream, initial_message).await;
        let _ = remote;
    }

    async fn connect_native_websocket(
        &self,
        path: &str,
        path_and_query: &str,
        headers: &[(String, String)],
        request: &RoutingRequest,
    ) -> Result<NativeWebSocket, WebSocketPrepareError> {
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
        let mut plan = match RoutePlanner::plan_for_passthrough(
            request,
            &self.config(),
            ProviderProtocol::OpenAI,
        ) {
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
            let codex_live =
                is_codex_live_route_model(&request.model) || is_codex_live_family_path(path);
            let config = self.config();
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
            && sumpter_core::model_name::clean(&expected_model)
                != sumpter_core::model_name::clean(&request.model)
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
        let mut last_error = None;
        for endpoint in &plan.endpoints {
            let Some(url) = websocket_url(&endpoint.base_url, path_and_query) else {
                last_error = Some(WebSocketPrepareError::new(
                    502,
                    "upstream_error",
                    "invalid realtime upstream URL",
                ));
                continue;
            };
            let url = if is_realtime_http_path(path) && !is_codex_live_family_path(path) {
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
            let mut upstream_request = match url.into_client_request().map_err(|error| {
                WebSocketPrepareError::new(502, "upstream_error", error.to_string())
            }) {
                Ok(request) => request,
                Err(error) => {
                    last_error = Some(error);
                    continue;
                }
            };
            for (name, value) in websocket_upstream_headers(headers) {
                if let Ok(name) = http::header::HeaderName::from_bytes(name.as_bytes())
                    && let Ok(value) = http::header::HeaderValue::from_str(&value)
                {
                    upstream_request.headers_mut().append(name, value);
                }
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
            match tokio_tungstenite::connect_async(upstream_request).await {
                Ok((mut upstream, _)) => {
                    if let Some(session) = session.as_ref()
                        && let Err(error) = send_realtime_session_update(
                            &mut upstream,
                            session,
                            &endpoint.upstream_model,
                        )
                        .await
                    {
                        last_error = Some(WebSocketPrepareError::new(
                            502,
                            "upstream_error",
                            format!("failed to apply Realtime session: {error}"),
                        ));
                        continue;
                    }
                    return Ok(upstream);
                }
                Err(error) => {
                    let (status, message) = websocket_connect_error(&error);
                    last_error = Some(WebSocketPrepareError::new(
                        status,
                        "upstream_error",
                        message,
                    ));
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            WebSocketPrepareError::new(502, "upstream_error", "upstream websocket unavailable")
        }))
    }

    async fn handle_request_parts(
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
            return error_response(
                StatusCode::FORBIDDEN,
                &[("error", self.inner.platform.status_denied_error())],
            );
        }

        if !path.starts_with("/__") && self.runtime_storage_backpressured() {
            return error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                &[("error", "runtime_storage_backpressure")],
            );
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
            | "/v1/realtime"
            | "/realtime"
            | "/openai/v1/realtime"
            | "/v1/realtime/calls"
            | "/realtime/calls"
            | "/openai/v1/realtime/calls"
            | "/v1/live"
            | "/live"
            | "/openai/v1/live" => {
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
                // 只认 query 里的 model:判定必须早于读 body(bodyless 的未知路径
                // 不能被迫分配 payload),所以 body 里的 model 不参与这道判定。
                if kind == PassthroughKind::Raw
                    && query
                        .and_then(|query| query_value(query, "model"))
                        .is_none_or(|model| model.trim().is_empty())
                {
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
        let client_kind = ClientKind::detect(header_value(&headers, "user-agent"), false);
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
        let client_kind = ClientKind::detect(header_value(&headers, "user-agent"), true);
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
        let client_kind = ClientKind::detect(
            header_value(&headers, "user-agent"),
            kind != PassthroughKind::ClaudeCountTokens,
        );
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
        // CPA uses the same Live handler for POST /v1/live and POST /v1/realtime.
        // Codex Desktop may also send the current chat model on those paths;
        // wrap the SDP/multipart bootstrap so routing cannot inherit it.
        if kind == PassthroughKind::Realtime
            && method.eq_ignore_ascii_case("POST")
            && should_normalize_codex_live_http(path_without_query(path_and_query))
        {
            let (normalized_body, normalized_content_type) =
                match prepare_codex_live_request(&body, content_type.as_deref().unwrap_or("")) {
                    Ok(request) => request,
                    Err(reason) => {
                        self.record_rejected_client_with_metadata(
                            400,
                            reason,
                            None,
                            Some(purpose),
                            client_kind,
                            header_codex_metadata.clone(),
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
                            header_codex_metadata.clone(),
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
        let metadata_body = content_type
            .as_deref()
            .is_some_and(content_type_is_json)
            .then(|| serde_json::from_slice::<Value>(&body).ok())
            .flatten();
        let codex_metadata = CodexMetadata::from_request(&headers, metadata_body.as_ref());
        let fields = if resource_intent {
            NativePassthroughFields {
                model: sumpter_core::routing::RESOURCE_ROUTING_MODEL.into(),
                stream: false,
            }
        } else if kind == PassthroughKind::Raw {
            let body_model = content_type
                .as_deref()
                .filter(|content_type| content_type_is_json(content_type))
                .and_then(|_| serde_json::from_slice::<Value>(&body).ok())
                .and_then(|value| {
                    value
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|model| !model.is_empty())
                        .map(str::to_string)
                });
            let model = path_and_query
                .split_once('?')
                .and_then(|(_, query)| query_value(query, "model"))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
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
                .and_then(|(_, query)| query_value(query, "model"))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
            let model = realtime_voice_route_model(
                method,
                path_and_query,
                query_model.or_else(|| realtime_body_model(&body, content_type.as_deref())),
                self.live_session_model(path_and_query).flatten(),
                self.realtime_client_secret_model(&headers),
            );
            NativePassthroughFields {
                model,
                stream: false,
            }
        } else if kind == PassthroughKind::Videos {
            let path = path_without_query(path_and_query);
            let requested = path_and_query
                .split_once('?')
                .and_then(|(_, query)| query_value(query, "model"))
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
                .or_else(|| realtime_body_model(&body, content_type.as_deref()));
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
        let resource_intent = client_out
            .as_ref()
            .is_some_and(|client| is_resource_passthrough_kind(client.passthrough_kind));
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
        let mut plan = match if resource_intent {
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
                        && is_codex_live_family_path(path_without_query(path_and_query))
                });
                let (status, error) = if is_live {
                    (StatusCode::SERVICE_UNAVAILABLE, "no_live_provider")
                } else if capability_error == Some("video") {
                    (StatusCode::SERVICE_UNAVAILABLE, "no_video_provider")
                } else if capability_error == Some("image") {
                    (StatusCode::BAD_REQUEST, "no_image_provider")
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
            let path = path_without_query(path_and_query);
            let codex_live = is_codex_live_route_model(&request.model)
                || is_codex_live_family_path(path)
                || is_codex_live_http_bootstrap(method, path);
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
            && sumpter_core::model_name::clean(&expected_model)
                != sumpter_core::model_name::clean(&request.model)
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
        let ordered = self.ordered_endpoints(&plan, &session_key);
        // 出站桥无法无损表达的请求不能悄悄降级成残缺翻译:把承接不了的入口从候选里
        // 剔除,failover 仍有机会落到能原生承接的入口;全都承接不了才回 400,并带上
        // 具体字段名 —— 否则这类失败在客户端侧完全不可诊断(只表现为模型不听话)。
        // native 入口按字节转发,不过出站桥,所以不参与这道校验。
        let mut translation_error: Option<bridge::TranslationError> = None;
        let ordered: Vec<PlannedEndpoint> = ordered
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
        if ordered.is_empty() {
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
        // 路由计划定型且粘性排序已完成：此时首选入口已经确定，虽然尚未真正
        // 发起网络尝试。先写入候选协议，accepted/failover 后仍由 guard 用实际
        // 胜出入口覆盖，避免长时间等待首响应时三元组一直为空。
        let preferred_endpoint = ordered.first();

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
            codex_metadata,
            client_declared: ClientDeclaredMetadata::from_headers(&headers),
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
            outcome: None,
            phase: Some(RuntimeEventPhase::InFlight),
            pool_id: None,
            request_purpose: Some(purpose),
            request_id: Some(client_event_id.clone()),
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
            ordered,
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
        if plan.endpoints.len() <= 1 {
            return plan.endpoints.clone();
        }
        let (sticky_preferred, mut groups) = {
            let state = self.inner.state.lock().unwrap();
            let preferred = state
                .session_sticky
                .get(&session_key.value)
                .map(|entry| entry.label.clone());
            let mut groups: Vec<(String, i64, usize)> = Vec::new();
            for (index, endpoint) in plan.endpoints.iter().enumerate() {
                let group = endpoint.scheduling_group().to_string();
                if let Some(existing) = groups.iter_mut().find(|item| item.0 == group) {
                    existing.1 = existing.1.min(endpoint.priority);
                } else {
                    groups.push((group, endpoint.priority, index));
                }
            }
            (preferred, groups)
        };
        groups.sort_by(|a, b| {
            let a_preferred = sticky_preferred.as_deref() == Some(a.0.as_str());
            let b_preferred = sticky_preferred.as_deref() == Some(b.0.as_str());
            b_preferred
                .cmp(&a_preferred)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut output: Vec<PlannedEndpoint> = Vec::with_capacity(plan.endpoints.len());
        for (group, _, _) in &groups {
            for endpoint in &plan.endpoints {
                if endpoint.scheduling_group() == group {
                    output.push(endpoint.clone());
                }
            }
        }
        output
    }

    #[allow(clippy::too_many_arguments)]
    async fn forward(
        &self,
        config: &AppConfig,
        request: &RoutingRequest,
        ordered: Vec<PlannedEndpoint>,
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
            let mut round_state = RoundState {
                retryable_failures: 0,
                sticky_retryable_failures: 0,
                retry_after_seconds: None,
                last_failure: None,
            };

            let mut endpoint_index = 0usize;
            let mut sticky_retry_number = 0i64;
            let mut sticky_pass_checkpoint = round_state.sticky_retryable_failures;
            while endpoint_index < ordered.len() {
                // 即使组内某个候选因桥能力被跳过，也要在进入其它组前执行边界判断。
                if sticky_endpoint_count > 0
                    && endpoint_index == sticky_endpoint_count
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
                let endpoint = &ordered[endpoint_index];
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

                let response_timeout = effective_response_timeout(retry, endpoint);
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
                        endpoint_index = ordered.len();
                    }
                } else {
                    consecutive_500_endpoint = None;
                    consecutive_500_retries = 0;
                }
                if sticky_endpoint_count > 0
                    && endpoint_index == sticky_endpoint_count
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
            let retry_allowed = round_state.retryable_failures > 0
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

            let failure = round_state
                .last_failure
                .unwrap_or_else(FailureInfo::endpoints_exhausted);
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
            let result = self
                .inner
                .transport
                .send_streaming(build.request, response_timeout.map(Duration::from_secs_f64))
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
                let timeout = response_timeout.map(Duration::from_secs_f64);
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
                    let result = transport.send_streaming(build.request, timeout).await;
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
                    let failure = FailureInfo::upstream_http(
                        response.status,
                        upstream_request_id(&response.headers),
                    );
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
        if (is_codex_live_path(path_without_query(path_and_query))
            || is_realtime_call_bootstrap_path(path_without_query(path_and_query)))
            && (200..=299).contains(&response.status)
            && let Some(call_id) = live_call_id_from_headers(&response.headers)
        {
            self.register_live_session(&call_id, &endpoint.endpoint_id, &endpoint.routed_model);
        }
        if is_videos_create_path(path_without_query(path_and_query))
            && (200..=299).contains(&response.status)
            && let Some(video_id) = video_id_from_headers(&response.headers)
        {
            self.register_video_session(&video_id, &endpoint.endpoint_id, &endpoint.routed_model);
        }
        let passthrough = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough.is_some());
        let bridging = !passthrough
            && endpoint.protocol != ProviderProtocol::Anthropic
            && response.status == 200;
        let upstream_request_id = upstream_request_id(&response.headers);
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
        self.record_event(in_flight);

        guard.attach_upstream(UpstreamAttempt {
            event_id: upstream_event_id,
            endpoint: endpoint.clone(),
            started: attempt_started,
            ttfb_ms: attempt_ttfb_ms,
            is_failover,
            pinned_ip,
            upstream_request_id,
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

        let idle_timeout = config
            .retry
            .stream_idle_timeout_seconds
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
            video_session_body: (is_videos_create_path(path_without_query(path_and_query))
                && (200..=299).contains(&response.status))
            .then(Vec::new),
            video_session_endpoint_id: is_videos_create_path(path_without_query(path_and_query))
                .then(|| endpoint.endpoint_id.clone()),
            video_session_model: is_videos_create_path(path_without_query(path_and_query))
                .then(|| endpoint.routed_model.clone()),
            idle_timeout,
            guard,
            finished: false,
        };
        let body_stream = futures_util::stream::unfold(state, |mut st| async move {
            if st.finished {
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
                        if let Some(secret_body) = st.realtime_secret_body.take() {
                            st.guard.engine.register_realtime_client_secret_from_body(
                                &secret_body,
                                st.realtime_secret_endpoint_id.as_deref(),
                                st.realtime_secret_model.as_deref(),
                                st.realtime_secret_session.take(),
                            );
                        }
                        if let Some(video_body) = st.video_session_body.take()
                            && let (Some(endpoint_id), Some(model)) = (
                                st.video_session_endpoint_id.as_deref(),
                                st.video_session_model.as_deref(),
                            )
                            && let Some(video_id) = video_id_from_json(&video_body)
                        {
                            st.guard
                                .engine
                                .register_video_session(&video_id, endpoint_id, model);
                        }
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
                        | "x-kekulv-project"
                        | "x-kekulv-workspace"
                        | "x-kekulv-git-remote"
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
) {
    let (mut downstream_tx, mut downstream_rx) = socket.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    if let Some(message) = initial_message
        && upstream_tx
            .send(websocket_message_to_tungstenite(message))
            .await
            .is_err()
    {
        let _ = downstream_tx.close().await;
        return;
    }
    let downstream_to_upstream = async {
        while let Some(Ok(message)) = downstream_rx.next().await {
            if upstream_tx
                .send(websocket_message_to_tungstenite(message))
                .await
                .is_err()
            {
                break;
            }
        }
        let _ = upstream_tx.close().await;
    };
    let upstream_to_downstream = async {
        while let Some(Ok(message)) = upstream_rx.next().await {
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
            if downstream_tx.send(converted).await.is_err() {
                break;
            }
        }
        let _ = downstream_tx.close().await;
    };
    tokio::select! {
        _ = downstream_to_upstream => {},
        _ = upstream_to_downstream => {},
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
        _ => (502, "upstream websocket unavailable".into()),
    }
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
    video_session_body: Option<Vec<u8>>,
    video_session_endpoint_id: Option<String>,
    video_session_model: Option<String>,
    idle_timeout: Option<Duration>,
    guard: CompletionGuard,
    finished: bool,
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
    headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
        .filter_map(|(_, value)| value.trim().parse::<f64>().ok())
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
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
            },
            StreamReadError::Upstream(error) => Self {
                kind: RuntimeFailureKind::StreamInterrupted,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(error.to_string()),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
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
        event.session_id = self.meta.session_id.clone();
        event.tool_calls = self.tool_calls_option();
        event.codex_metadata = self.meta.codex_metadata.clone();
        event.client_declared = self.meta.client_declared.clone();
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
                    FailureInfo::upstream_http(
                        self.status as u16,
                        self.upstream
                            .as_ref()
                            .and_then(|attempt| attempt.upstream_request_id.clone()),
                    )
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

/// Extract a stable client session identifier for analytics. Values are
/// bounded and rejected when they contain controls; request bodies are never
/// used as a fallback here.
fn observed_session_id(headers: &[(String, String)]) -> Option<String> {
    ["x-claude-code-session-id", "session_id", "session-id"]
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

#[cfg(test)]
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

fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then_some(value)
    })
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
        "/v1/live",
        "/live",
        "/openai/v1/live",
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

fn has_exact_codex_live_mapping(config: &AppConfig, endpoint_id: &str) -> bool {
    let live_model = request_build::DEFAULT_CODEX_LIVE_MODEL;
    config.endpoint(endpoint_id).is_some_and(|endpoint| {
        endpoint.mappings.iter().any(|mapping| {
            sumpter_core::model_name::clean(&mapping.client_pattern) == live_model
                && (mapping.upstream_model.trim().is_empty()
                    || sumpter_core::model_name::clean(&mapping.upstream_model) == live_model)
                && sumpter_core::capability::mapping_has_capability(
                    &mapping.capabilities,
                    &mapping.client_pattern,
                    sumpter_core::capability::ModelCapability::Live,
                )
        })
    })
}

fn has_exact_realtime_mapping(config: &AppConfig, endpoint_id: &str, model: &str) -> bool {
    let model = sumpter_core::model_name::clean(model);
    config.endpoint(endpoint_id).is_some_and(|endpoint| {
        endpoint
            .mappings
            .iter()
            .any(|mapping| sumpter_core::model_name::clean(&mapping.client_pattern) == model)
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
            | "/v1/videos/generations"
            | "/videos/generations"
            | "/openai/v1/videos/generations"
            | "/v1/videos/edits"
            | "/videos/edits"
            | "/openai/v1/videos/edits"
            | "/v1/videos/extensions"
            | "/videos/extensions"
            | "/openai/v1/videos/extensions"
    )
}

fn video_id_from_path(path: &str) -> Option<&str> {
    ["/v1/videos/", "/videos/", "/openai/v1/videos/"]
        .iter()
        .find_map(|prefix| path.strip_prefix(prefix))
        .map(|rest| rest.split('/').next().unwrap_or_default())
        .filter(|id| !id.is_empty() && !matches!(*id, "generations" | "edits" | "extensions"))
}

fn is_videos_lookup_path(path: &str) -> bool {
    video_id_from_path(path).is_some()
}

fn video_id_from_headers(headers: &[(String, String)]) -> Option<String> {
    headers.iter().rev().find_map(|(name, value)| {
        if !name.eq_ignore_ascii_case("location") {
            return None;
        }
        let path = path_without_query(value.trim());
        video_id_from_path(path).map(str::to_string)
    })
}

fn video_id_from_json(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    ["id", "request_id", "video_id"]
        .into_iter()
        .filter_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::trim)
        .find(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("id"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        })
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
    matches!(path, "/v1/live" | "/live" | "/openai/v1/live")
}

fn is_codex_live_root_realtime_path(path: &str) -> bool {
    matches!(path, "/v1/realtime" | "/realtime" | "/openai/v1/realtime")
}

fn is_codex_live_http_bootstrap(method: &str, path: &str) -> bool {
    method.eq_ignore_ascii_case("POST")
        && (is_codex_live_path(path)
            || is_codex_live_root_realtime_path(path)
            || is_realtime_call_bootstrap_path(path))
}

fn should_normalize_codex_live_http(path: &str) -> bool {
    is_codex_live_path(path) || is_codex_live_root_realtime_path(path)
}

fn is_codex_live_route_model(model: &str) -> bool {
    sumpter_core::model_name::clean(model) == request_build::DEFAULT_CODEX_LIVE_MODEL
}

/// CPA Live/Realtime bootstrap always selects Codex OAuth and maps empty or
/// standard Realtime names to `gpt-live-1-codex`. Sumpter selects providers by
/// mapping, so voice paths must not inherit the surrounding chat model.
fn realtime_voice_route_model(
    method: &str,
    path_and_query: &str,
    requested: Option<String>,
    sideband_model: Option<String>,
    secret_model: Option<String>,
) -> String {
    let path = path_without_query(path_and_query);
    if is_codex_live_sideband_target(path_and_query) {
        return sideband_model
            .filter(|model| !model.trim().is_empty())
            .or(secret_model)
            .unwrap_or_else(|| request_build::DEFAULT_CODEX_LIVE_MODEL.to_string());
    }
    if is_realtime_client_secret_path(path_and_query) {
        return requested
            .or(secret_model)
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| request_build::DEFAULT_REALTIME_MODEL.to_string());
    }
    if secret_model
        .as_deref()
        .is_some_and(|model| !model.trim().is_empty())
        && !is_codex_live_path(path)
        && !is_codex_live_family_path(path)
    {
        return requested
            .or(secret_model)
            .filter(|model| !model.trim().is_empty())
            .unwrap_or_else(|| request_build::DEFAULT_CODEX_LIVE_MODEL.to_string());
    }
    if is_codex_live_http_bootstrap(method, path)
        || is_codex_live_family_path(path)
        || is_codex_live_root_realtime_path(path)
        || is_realtime_http_path(path)
    {
        return request_build::DEFAULT_CODEX_LIVE_MODEL.to_string();
    }
    requested
        .or(secret_model)
        .filter(|model| !model.trim().is_empty())
        .unwrap_or_else(|| request_build::DEFAULT_CODEX_LIVE_MODEL.to_string())
}

fn is_codex_live_family_path(path: &str) -> bool {
    ["/v1/live", "/live", "/openai/v1/live"]
        .iter()
        .any(|prefix| is_path_or_child(path, prefix))
}

fn is_realtime_call_bootstrap_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/realtime/calls" | "/realtime/calls" | "/openai/v1/realtime/calls"
    )
}

fn live_call_id_from_path(path: &str) -> Option<&str> {
    [
        "/v1/live/",
        "/live/",
        "/openai/v1/live/",
        "/v1/realtime/calls/",
        "/realtime/calls/",
        "/openai/v1/realtime/calls/",
    ]
    .iter()
    .find_map(|prefix| path.strip_prefix(prefix))
    .map(|rest| rest.split('/').next().unwrap_or_default())
    .filter(|call_id| !call_id.is_empty())
}

/// CPA also exposes a frameless quicksilver sideband at
/// `/v1/realtime?intent=quicksilver&call_id=…`; treat it as the same session
/// target as `/v1/live/{call_id}` while leaving ordinary Realtime connections
/// untouched.
fn live_call_id_from_target(path_and_query: &str) -> Option<&str> {
    let path = path_without_query(path_and_query);
    if let Some(call_id) = live_call_id_from_path(path) {
        return Some(call_id);
    }
    if !is_realtime_http_path(path) || is_codex_live_family_path(path) {
        return None;
    }
    let query = path_and_query.split_once('?').map(|(_, query)| query)?;
    let intent = query_value(query, "intent")?;
    if !intent.eq_ignore_ascii_case("quicksilver") {
        return None;
    }
    query_value(query, "call_id").filter(|call_id| !call_id.trim().is_empty())
}

fn is_codex_live_sideband_target(path_and_query: &str) -> bool {
    let path = path_without_query(path_and_query);
    (is_codex_live_family_path(path) && live_call_id_from_target(path_and_query).is_some())
        || (is_realtime_http_path(path)
            && query_value(
                path_and_query
                    .split_once('?')
                    .map_or("", |(_, query)| query),
                "intent",
            )
            .is_some_and(|intent| intent.eq_ignore_ascii_case("quicksilver"))
            && live_call_id_from_target(path_and_query).is_some())
}

fn live_call_id_from_headers(headers: &[(String, String)]) -> Option<String> {
    headers.iter().rev().find_map(|(name, value)| {
        if name.eq_ignore_ascii_case("x-live-call-id")
            || name.eq_ignore_ascii_case("x-live-session")
            || name.eq_ignore_ascii_case("x-call-id")
        {
            return (!value.trim().is_empty()).then(|| value.trim().to_string());
        }
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
        live_call_id_from_target(&target)
            .map(str::to_string)
            .or_else(|| {
                (!location.contains('/') && !location.contains('?')).then(|| location.to_string())
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
            | "/v1/realtime/sessions"
            | "/realtime/sessions"
            | "/openai/v1/realtime/sessions"
            | "/v1/realtime/translations/client_secrets"
            | "/realtime/translations/client_secrets"
            | "/openai/v1/realtime/translations/client_secrets"
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
    let retry_delay_seconds = pass_through_retry_delay
        .then(|| {
            matches!(
                failure.kind,
                RuntimeFailureKind::ResponseTimeout
                    | RuntimeFailureKind::ConnectionFailed
                    | RuntimeFailureKind::UpstreamHttpStatus
            )
            .then_some(retry_delay_seconds)
            .flatten()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
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
    fn codex_live_sdp_is_wrapped_as_quicksilver_json() {
        let (body, content_type) =
            prepare_codex_live_request(b"v=0\r\na=setup:actpass\r\n", "application/sdp")
                .expect("valid SDP");
        assert_eq!(content_type, "application/json");
        let value: Value = serde_json::from_slice(&body).expect("JSON envelope");
        assert_eq!(value["sdp"], "v=0\r\na=setup:actpass\r\n");
        assert_eq!(value["session"]["type"], "quicksilver");
        assert_eq!(
            value["session"]["model"],
            request_build::DEFAULT_CODEX_LIVE_MODEL
        );
    }

    #[test]
    fn codex_live_multipart_preserves_session_fields_and_overrides_text_model() {
        let boundary = "live-boundary";
        let body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\n\r\nv=0\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\n\r\n{{\"instructions\":\"hello\",\"model\":\"claude-fable-5\"}}\r\n--{boundary}--\r\n"
        );
        let (body, content_type) = prepare_codex_live_request(
            body.as_bytes(),
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .expect("valid multipart Live request");
        assert_eq!(content_type, "application/json");
        let value: Value = serde_json::from_slice(&body).expect("JSON envelope");
        assert_eq!(value["sdp"], "v=0");
        assert_eq!(value["session"]["instructions"], "hello");
        assert_eq!(value["session"]["type"], "quicksilver");
        assert_eq!(
            value["session"]["model"],
            request_build::DEFAULT_CODEX_LIVE_MODEL
        );
    }

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
        assert_eq!(
            video_id_from_json(br#"{"id":"video_abc","object":"video"}"#).as_deref(),
            Some("video_abc")
        );
    }

    #[test]
    fn public_realtime_path_is_not_treated_as_codex_live() {
        assert!(is_codex_live_path("/v1/live"));
        assert!(!is_codex_live_path("/v1/realtime/calls"));
        assert!(is_codex_live_http_bootstrap("POST", "/v1/live"));
        assert!(is_codex_live_http_bootstrap("POST", "/v1/realtime"));
        assert!(is_codex_live_http_bootstrap("POST", "/v1/realtime/calls"));
        assert!(!is_codex_live_http_bootstrap("GET", "/v1/realtime"));
        assert!(!is_codex_live_http_bootstrap(
            "POST",
            "/v1/realtime/client_secrets"
        ));
        assert!(should_normalize_codex_live_http("/v1/realtime"));
        assert!(!should_normalize_codex_live_http("/v1/realtime/calls"));
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
            request_build::DEFAULT_CODEX_LIVE_MODEL
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
            live_call_id_from_target("/v1/live/call-path"),
            Some("call-path")
        );
        assert_eq!(
            live_call_id_from_target("/v1/realtime?intent=quicksilver&call_id=call-query"),
            Some("call-query")
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
        assert!(!is_codex_live_sideband_target(
            "/v1/realtime?call_id=ordinary-realtime"
        ));
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
            ("x-kekulv-workspace".into(), "/Users/example/project".into()),
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
                    || name.eq_ignore_ascii_case("x-kekulv-workspace"))
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
    [
        value.get("model"),
        value
            .get("session")
            .and_then(|session| session.get("model")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|model| !model.is_empty())
    .map(str::to_string)
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
    ) {
        value.get("session")?.as_object()?.clone()
    } else if matches!(
        path,
        "/v1/realtime/sessions" | "/realtime/sessions" | "/openai/v1/realtime/sessions"
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

/// Normalize Codex Desktop's Live/Quicksilver bootstrap to the JSON envelope
/// used by CPA. Called for `/v1/live` and POST `/v1/realtime`; `/v1/realtime/calls`
/// keeps the public SDP/JSON body and only the routing model is forced.
fn prepare_codex_live_request(
    body: &[u8],
    content_type: &str,
) -> Result<(Bytes, String), &'static str> {
    let media_type = content_type
        .split(';')
        .next()
        .map(str::trim)
        .unwrap_or_default()
        .to_ascii_lowercase();
    match media_type.as_str() {
        "application/sdp" | "text/plain" => {
            let sdp = std::str::from_utf8(body).map_err(|_| "Codex Live SDP must be UTF-8")?;
            if sdp.trim().is_empty() {
                return Err("Codex Live request requires an SDP offer");
            }
            encode_codex_live_envelope(sdp, None)
        }
        "multipart/form-data" => prepare_codex_live_multipart(body, content_type),
        "application/json" => {
            let mut value: Value =
                serde_json::from_slice(body).map_err(|_| "Codex Live JSON body is invalid")?;
            let object = value
                .as_object_mut()
                .ok_or("Codex Live JSON body must be an object")?;
            let sdp = object
                .get("sdp")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or("Codex Live request requires an SDP offer")?
                .to_string();
            let mut session = match object.remove("session") {
                Some(Value::Object(session)) => session,
                Some(_) => return Err("Codex Live session must be an object"),
                None => serde_json::Map::new(),
            };
            // The desktop voice route is Codex's quicksilver protocol even
            // when the surrounding text session currently uses another model.
            session.insert("type".into(), Value::String("quicksilver".into()));
            session.insert(
                "model".into(),
                Value::String(request_build::DEFAULT_CODEX_LIVE_MODEL.into()),
            );
            // A root model is not the Live session model. Removing it avoids
            // routing the call by a stale Claude text model from Codex headers.
            object.remove("model");
            object.insert("sdp".into(), Value::String(sdp));
            object.insert("session".into(), Value::Object(session));
            let encoded =
                serde_json::to_vec(&value).map_err(|_| "Codex Live JSON encoding failed")?;
            Ok((Bytes::from(encoded), "application/json".into()))
        }
        _ => Err("unsupported Content-Type for Codex Live request"),
    }
}

fn prepare_codex_live_multipart(
    body: &[u8],
    content_type: &str,
) -> Result<(Bytes, String), &'static str> {
    let boundary = multipart_boundary(content_type).ok_or("multipart boundary is required")?;
    let marker = format!("--{boundary}").into_bytes();
    let mut cursor = 0usize;
    let mut sdp = None;
    let mut session = None;
    while let Some(relative) = find_bytes(&body[cursor..], &marker) {
        let mut part_start = cursor + relative + marker.len();
        if body.get(part_start..part_start + 2) == Some(b"--") {
            break;
        }
        if body.get(part_start..part_start + 2) == Some(b"\r\n") {
            part_start += 2;
        } else if body.get(part_start) == Some(&b'\n') {
            part_start += 1;
        }
        let Some((header_offset, separator_len)) = multipart_header_end(&body[part_start..]) else {
            return Err("malformed multipart headers");
        };
        if header_offset > 16 * 1024 {
            return Err("multipart headers are too large");
        }
        let headers = &body[part_start..part_start + header_offset];
        let content_start = part_start + header_offset + separator_len;
        let Some(next_relative) = find_bytes(&body[content_start..], &marker) else {
            return Err("multipart closing boundary is missing");
        };
        let mut content_end = content_start + next_relative;
        while content_end > content_start && matches!(body[content_end - 1], b'\r' | b'\n') {
            content_end -= 1;
        }
        if let Some(name) = multipart_field_name(headers) {
            let field = &body[content_start..content_end];
            match name.as_str() {
                "sdp" => {
                    let value = std::str::from_utf8(field)
                        .map_err(|_| "Codex Live SDP must be UTF-8")?
                        .trim();
                    if value.is_empty() {
                        return Err("Codex Live request requires an SDP offer");
                    }
                    sdp = Some(value.to_string());
                }
                "session" => {
                    let value: Value = serde_json::from_slice(field)
                        .map_err(|_| "Codex Live session field must contain valid JSON")?;
                    session = Some(
                        value
                            .as_object()
                            .cloned()
                            .ok_or("Codex Live session must be an object")?,
                    );
                }
                _ => {}
            }
        }
        cursor = content_start + next_relative;
    }
    let sdp = sdp.ok_or("Codex Live multipart body requires an sdp field")?;
    encode_codex_live_envelope(&sdp, session)
}

fn encode_codex_live_envelope(
    sdp: &str,
    session: Option<serde_json::Map<String, Value>>,
) -> Result<(Bytes, String), &'static str> {
    let mut session = session.unwrap_or_default();
    session.insert("type".into(), Value::String("quicksilver".into()));
    session.insert(
        "model".into(),
        Value::String(request_build::DEFAULT_CODEX_LIVE_MODEL.into()),
    );
    let value = json!({
        "sdp": sdp,
        "session": session,
    });
    let encoded = serde_json::to_vec(&value).map_err(|_| "Codex Live JSON encoding failed")?;
    Ok((Bytes::from(encoded), "application/json".into()))
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
    let boundary = multipart_boundary(content_type).ok_or("multipart boundary is required")?;
    let mut model = None;
    let mut stream = false;
    let marker = format!("--{boundary}").into_bytes();
    let mut cursor = 0usize;
    while let Some(relative) = find_bytes(&body[cursor..], &marker) {
        let mut part_start = cursor + relative + marker.len();
        if body.get(part_start..part_start + 2) == Some(b"--") {
            break;
        }
        if body.get(part_start..part_start + 2) == Some(b"\r\n") {
            part_start += 2;
        } else if body.get(part_start) == Some(&b'\n') {
            part_start += 1;
        }
        let Some((header_offset, separator_len)) = multipart_header_end(&body[part_start..]) else {
            return Err("malformed multipart headers");
        };
        if header_offset > 16 * 1024 {
            return Err("multipart headers are too large");
        }
        let headers = &body[part_start..part_start + header_offset];
        let content_start = part_start + header_offset + separator_len;
        let Some(next_relative) = find_bytes(&body[content_start..], &marker) else {
            return Err("multipart closing boundary is missing");
        };
        let mut content_end = content_start + next_relative;
        while content_end > content_start && matches!(body[content_end - 1], b'\r' | b'\n') {
            content_end -= 1;
        }
        if let Some(name) = multipart_field_name(headers)
            && matches!(name.as_str(), "model" | "stream")
        {
            let field = &body[content_start..content_end];
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
        cursor = content_start + next_relative;
    }
    Ok(NativePassthroughFields {
        model: model.unwrap_or_else(|| "gpt-image-2".into()),
        stream,
    })
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
                .to_string()
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
