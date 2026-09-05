//! Runtime-event seam shared by the data plane and replay tooling.

pub use sumpter_core::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, GrokMetadata, RuntimeEvent,
    RuntimeEventOutcome, RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase,
    RuntimeSnapshot, StreamTrace,
};

/// Fields generated per request are intentionally excluded from conformance
/// comparisons.  Callers can use this projection before comparing old/new
/// observations while retaining the full event for normal runtime storage.
pub fn comparable_event(event: &RuntimeEvent) -> RuntimeEvent {
    let mut normalized = event.clone();
    normalized.id.clear();
    normalized.request_id = None;
    normalized.timestamp = 0.0;
    normalized.duration_ms = 0;
    normalized.ttfb_ms = None;
    // Stream timing is measured from the scheduler/clock and can differ by a
    // millisecond (or more) between two otherwise identical replays.  Keep
    // protocol facts such as frame count, byte count, terminal event, and
    // usage, but remove only those wall-clock projections from the comparison.
    if let Some(trace) = normalized.stream_trace.as_mut() {
        trace.max_chunk_gap_ms = None;
        trace.last_chunk_at_ms = None;
    }
    normalized
}

use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::{KIND_CLIENT, KIND_UPSTREAM, unix_to_apple_epoch};
use sumpter_core::routing::{PlannedEndpoint, RequestPurpose};

use crate::boundary::PlatformNotice;
use crate::runtime_store::RuntimeCounters;

use super::Engine;
use super::context::{
    InboundRequestContext, apply_current_request_context, bounded_failure_detail,
    current_request_context, detect_client_kind, host_of, is_codex_originator,
    merge_codex_metadata, retain_codex_metadata_for_client,
};
use super::protocol::decoded_query_value;
use super::state::now_unix;

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

pub(super) fn runtime_outcome_token(event: &RuntimeEvent) -> Option<&'static str> {
    match event.outcome {
        Some(RuntimeEventOutcome::Succeeded) => Some("succeeded"),
        Some(RuntimeEventOutcome::Failed) => Some("failed"),
        Some(RuntimeEventOutcome::Cancelled) => Some("cancelled"),
        None => None,
    }
}

pub(super) fn new_event_id() -> String {
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

pub(super) fn protocol_token(protocol: ProviderProtocol) -> &'static str {
    match protocol {
        ProviderProtocol::Anthropic => "anthropic",
        ProviderProtocol::OpenAI => "openai",
        ProviderProtocol::OpenAIResponses => "openai-responses",
    }
}
impl Engine {
    // -----------------------------------------------------------------------
    // 记账
    // -----------------------------------------------------------------------

    pub(super) fn record_event(&self, mut event: RuntimeEvent) {
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
    pub(super) fn complete_client(&self, mut event: RuntimeEvent, failed_message: Option<String>) {
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

    pub(super) fn complete_upstream(&self, mut event: RuntimeEvent) {
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
    pub(super) fn record_rejected_client_with_metadata(
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
    pub(super) fn record_rejected_websocket_with_metadata(
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

    #[allow(clippy::too_many_arguments)]
    pub(super) fn upstream_event(
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
}
