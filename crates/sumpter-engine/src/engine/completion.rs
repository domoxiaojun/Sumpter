//! Completion implementation for the shared engine.

use std::time::Instant;

use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::{
    KIND_CLIENT, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase, STATUS_CLIENT_DISCONNECTED,
    StreamTrace, message_tokens, unix_to_apple_epoch,
};
use sumpter_core::routing::PlannedEndpoint;
use sumpter_core::stream_terminal::{SseTerminal, SseTerminalTracker};

use crate::request_build::PassthroughKind;

use super::Engine;
use super::context::{ClientMeta, host_of};
use super::events::protocol_token;
use super::failure::{FailureInfo, StreamReadError};
use super::state::now_unix;

/// accepted 后挂在 guard 上的上游归属(收尾事件与消息 token 的数据源)。
#[derive(Clone)]
pub(super) struct UpstreamAttempt {
    pub(super) event_id: String,
    pub(super) endpoint: PlannedEndpoint,
    pub(super) started: Instant,
    /// 该次尝试自身的响应头延迟(ms):`started` → accepted。
    pub(super) ttfb_ms: i64,
    pub(super) is_failover: bool,
    pub(super) upstream_request_id: Option<String>,
    pub(super) retry_after_seconds: Option<f64>,
    pub(super) capture_attempt_id: String,
}

#[derive(Default)]
pub(super) struct StreamTraceState {
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

    pub(super) fn record_response_summary(&mut self, tracker: &SseTerminalTracker) {
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
pub(super) struct CompletionGuard {
    pub(super) engine: Engine,
    client_event_id: String,
    pub(super) started: Instant,
    /// client in-flight 事件的原始 timestamp(apple 纪元秒):accepted 回填时原样保留。
    started_timestamp: f64,
    pub(super) meta: ClientMeta,
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
    pub(super) stream_trace: StreamTraceState,
    /// 响应头前失败/取消时，保留最后一个进入调度判断的真实入口。
    attempted_endpoint: Option<PlannedEndpoint>,
    upstream: Option<UpstreamAttempt>,
    done: bool,
}

impl CompletionGuard {
    pub(super) fn new(
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

    pub(super) fn mark_failover(&mut self) {
        self.failover = true;
    }

    pub(super) fn set_passthrough(&mut self, kind: Option<PassthroughKind>) {
        self.passthrough = kind;
    }

    pub(super) fn request_id(&self) -> &str {
        &self.client_event_id
    }

    pub(super) fn capture_attempt_id(&self) -> Option<&str> {
        self.upstream
            .as_ref()
            .map(|attempt| attempt.capture_attempt_id.as_str())
    }

    pub(super) fn set_status(&mut self, status: i64) {
        self.status = status;
    }

    pub(super) fn set_round(&mut self, round: i64) {
        self.rounds = round;
    }

    pub(super) fn note_endpoint(&mut self, endpoint: &PlannedEndpoint) {
        self.attempted_endpoint = Some(endpoint.clone());
    }

    pub(super) fn attach_upstream(&mut self, attempt: UpstreamAttempt) {
        self.upstream = Some(attempt);
    }

    /// accepted 后原地回填 client in-flight 事件的入口归属(upsert 不计数):
    /// 流式期间事件表的客户端行直接可见走的哪个入口,不用切「上游」筛选比对。
    /// timestamp 保持请求开始时刻;status 保留已收到的真实上游 HTTP 状态。
    /// 尚未收到响应头时仍为 0,只有这种进行中事件才表示“未收到响应头”。
    ///
    /// 同时钉住客户端视角的首字节耗时:此刻正是响应头到达。流式请求的 durationMS 是
    /// 「吐完最后一个字」,单看它分不清「上游卡住」和「正常长输出」,TTFB 才是那把尺。
    pub(super) fn record_streaming_started(&mut self, message: Option<String>) {
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

    pub(super) fn set_tool_calls(&mut self, calls: &[String]) {
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

    pub(super) fn record_stream_chunk(&mut self, bytes: usize) {
        self.stream_trace.record_chunk(bytes);
    }

    pub(super) fn record_stream_terminal(&mut self, terminal: &SseTerminal) {
        self.stream_trace.record_terminal(terminal);
    }

    /// 收尾消息的信息 token(词表 §5.1):
    /// `[bridge, deferred_rounds, unmatched_no_tools]`。
    /// bridge 仅在桥接真实生效(非 anthropic 协议且 accepted 2xx)时产出;
    /// unmatched_no_tools 只描述请求本身,不进 upstream 事件。
    fn attempt_tokens(&self) -> [Option<String>; 3] {
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
        [bridged, rounds, unmatched]
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
    pub(super) fn complete(&mut self, status: i64, message: Option<String>, failure: FailureInfo) {
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
    pub(super) fn complete_from_stream(&mut self, error: Option<StreamReadError>) {
        if self.done {
            return;
        }
        self.done = true;
        let [bridged, rounds, unmatched] = self.attempt_tokens();
        match error {
            None => {
                let client_message = message_tokens::join(&[bridged.clone(), rounds, unmatched]);
                let upstream_message = message_tokens::join(&[bridged]);
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
                let client_message =
                    message_tokens::join(&[bridged.clone(), Some(interrupted.clone()), unmatched])
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
                let upstream_message = message_tokens::join(&[bridged, Some(interrupted)]);
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
    pub(super) fn complete_from_protocol_terminal(&mut self, terminal: SseTerminal) {
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

        let [bridged, rounds, unmatched] = self.attempt_tokens();
        let client_message = message_tokens::join(&[bridged.clone(), rounds, unmatched]);
        let upstream_message = message_tokens::join(&[bridged]);
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
#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use sumpter_core::config::{AppConfig, EndpointProtocolMode, ProviderProtocol};
    use sumpter_core::events::{
        ClientKind, KIND_CLIENT, KIND_UPSTREAM, RuntimeEventOutcome, RuntimeEventPhase,
        RuntimeFailureKind, RuntimeFailurePhase, STATUS_CLIENT_DISCONNECTED, unix_to_apple_epoch,
    };
    use sumpter_core::routing::{PlannedEndpoint, RequestPurpose, RouteMode};
    use sumpter_core::stream_terminal::SseTerminal;

    use crate::engine::Engine;
    use crate::engine::context::ClientMeta;
    use crate::engine::state::now_unix;
    use crate::replay::ReplayTransport;

    use super::{CompletionGuard, UpstreamAttempt};

    fn guard() -> (Engine, CompletionGuard) {
        let engine = Engine::new(
            AppConfig::from_json("{}").unwrap(),
            None,
            Arc::new(ReplayTransport::new([])),
        );
        let guard = CompletionGuard::new(
            engine.clone(),
            "client-completion-test".into(),
            Instant::now(),
            unix_to_apple_epoch(now_unix()),
            ClientMeta {
                client_kind: ClientKind::ClaudeCode,
                source_format: ProviderProtocol::Anthropic,
                target_format: None,
                route_mode: None,
                client_model: "test-model".into(),
                effective_model: "test-model".into(),
                feature_rule_id: None,
                purpose: RequestPurpose::Standard,
                unmatched_no_tools: false,
                session_id: None,
                codex_metadata: None,
                client_declared: None,
                grok_metadata: None,
                request_context: None,
            },
        );
        (engine, guard)
    }

    #[test]
    fn protocol_completion_then_drop_counts_client_and_upstream_once() {
        let (engine, mut guard) = guard();
        guard.attach_upstream(UpstreamAttempt {
            event_id: "upstream-completion-test".into(),
            endpoint: PlannedEndpoint {
                endpoint_id: "test-endpoint".into(),
                endpoint_name: "Test endpoint".into(),
                base_url: "https://upstream.invalid".into(),
                configured_protocol: EndpointProtocolMode::Anthropic,
                source_format: ProviderProtocol::Anthropic,
                protocol: ProviderProtocol::Anthropic,
                route_mode: RouteMode::Native,
                routed_model: "test-model".into(),
                upstream_model: "test-model".into(),
                priority: 0,
                sticky_group: None,
                thinking: Default::default(),
                context: Default::default(),
                effort_override: None,
                failover_timeout_seconds: None,
                keep_alive: false,
            },
            started: Instant::now(),
            ttfb_ms: 1,
            is_failover: false,
            upstream_request_id: None,
            retry_after_seconds: None,
            capture_attempt_id: "capture-completion-test".into(),
        });
        guard.set_status(200);
        guard.record_streaming_started(None);
        guard.complete_from_protocol_terminal(SseTerminal::Completed);
        drop(guard);

        let snapshot = engine.runtime_snapshot();
        assert_eq!(snapshot.client_requests, 1);
        assert_eq!(snapshot.client_successes, 1);
        assert_eq!(snapshot.client_failures, 0);
        assert_eq!(snapshot.upstream_attempts, 1);
        assert_eq!(snapshot.upstream_successes, 1);
        assert_eq!(snapshot.upstream_failures, 0);
        assert_eq!(snapshot.recent_events.len(), 2);
        for kind in [KIND_CLIENT, KIND_UPSTREAM] {
            let events: Vec<_> = snapshot
                .recent_events
                .iter()
                .filter(|event| event.kind == kind)
                .collect();
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].outcome, Some(RuntimeEventOutcome::Succeeded));
            assert_eq!(events[0].phase, Some(RuntimeEventPhase::Completed));
            assert_eq!(events[0].status_code, 200);
        }
    }

    #[test]
    fn drop_before_endpoint_selection_records_only_one_cancelled_client() {
        let (engine, guard) = guard();
        drop(guard);

        let snapshot = engine.runtime_snapshot();
        assert_eq!(snapshot.client_requests, 1);
        assert_eq!(snapshot.client_successes, 0);
        assert_eq!(snapshot.client_failures, 0);
        assert_eq!(snapshot.upstream_attempts, 0);
        assert_eq!(snapshot.upstream_successes, 0);
        assert_eq!(snapshot.upstream_failures, 0);
        assert_eq!(snapshot.recent_events.len(), 1);
        let event = &snapshot.recent_events[0];
        assert_eq!(event.kind, KIND_CLIENT);
        assert_eq!(event.outcome, Some(RuntimeEventOutcome::Cancelled));
        assert_eq!(event.phase, Some(RuntimeEventPhase::Completed));
        assert_eq!(event.status_code, STATUS_CLIENT_DISCONNECTED);
        assert_eq!(
            event.failure_kind,
            Some(RuntimeFailureKind::ClientCancelled)
        );
        assert_eq!(
            event.failure_phase,
            Some(RuntimeFailurePhase::BeforeResponse)
        );
        assert!(event.endpoint_id.is_none());
        assert!(event.upstream_status_code.is_none());
    }
}
