//! Http relay implementation for the shared engine.

use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::Value;
use sumpter_core::bridge::{self, SseBridge};
use sumpter_core::bridge_in::{self, ClientDialect};
use sumpter_core::config::{AppConfig, ProviderProtocol};
use sumpter_core::events::{RuntimeEventPhase, message_tokens};
use sumpter_core::routing::PlannedEndpoint;
use sumpter_core::stream_terminal::{SseDialect, SseTerminalTracker};

use crate::outbound::TransportError;
use crate::request_build::PassthroughKind;

use super::Engine;
use super::completion::{CompletionGuard, UpstreamAttempt};
use super::context::{ClientOut, is_hop_by_hop, upstream_request_id};
use super::dispatch::{effective_stream_idle_timeout_for_request, retry_after_seconds};
use super::events::{new_event_id, protocol_token};
use super::failure::StreamReadError;
use super::http_response::error_response;
use super::protocol::{
    is_live_bootstrap_request, is_videos_create_request, live_call_id_from_headers,
    live_call_id_from_payload, video_id_from_headers, video_id_from_payload,
};

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
impl Engine {
    /// accepted 之后的 relay:直连剥 hop-by-hop 原样转;桥接改写 200 + SSE 头。
    /// 完成/中断/断开的记账全部挂在响应体流的生命周期上。
    #[allow(clippy::too_many_arguments)]
    pub(super) fn relay(
        &self,
        config: &AppConfig,
        endpoint: &PlannedEndpoint,
        response: crate::outbound::UpstreamResponse,
        mut guard: CompletionGuard,
        method: &str,
        path_and_query: &str,
        attempt_started: Instant,
        is_failover: bool,
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
                ProviderProtocol::Gemini => None,
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
                    ProviderProtocol::Gemini => SseDialect::Gemini,
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
                        PassthroughKind::Chat
                            | PassthroughKind::Responses
                            | PassthroughKind::GeminiGenerate
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
                            st.guard.record_response_summary(tracker);
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
                        if let Some(tracker) = &mut st.upstream_summary_tracker {
                            let _ = tracker.finish();
                            st.guard.record_response_summary(tracker);
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
                        let terminal = terminal.or_else(|| {
                            st.terminal_tracker
                                .as_mut()
                                .and_then(SseTerminalTracker::finish)
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
