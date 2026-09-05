//! Bidirectional WebSocket frames, close metrics and bounded event metadata.

use super::context::bounded_request_path;
use super::context::detect_client_kind;
use super::context::host_of;
use super::context::is_codex_originator;
use super::context::merge_codex_metadata;
use super::context::observed_session_id;
use super::context::retain_codex_metadata_for_client;
use super::dispatch::retry_after_seconds;
use super::events::new_event_id;
use super::failure::FailureInfo;
use super::protocol::RealtimeRouteIntent;
use super::protocol::is_realtime_http_path;
use super::protocol::is_responses_websocket_path;
use super::protocol::path_without_query;
use super::state::now_unix;
use super::websocket::MAX_WEBSOCKET_METADATA_FRAME_BYTES;
use super::websocket::NativeWebSocket;
use axum::extract::ws::Message as WebSocketMessage;
use axum::extract::ws::WebSocket;
use futures_util::SinkExt;
use futures_util::StreamExt;
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Instant;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::ClientDeclaredMetadata;
use sumpter_core::events::ClientKind;
use sumpter_core::events::CodexMetadata;
use sumpter_core::events::GrokMetadata;
use sumpter_core::events::KIND_CLIENT;
use sumpter_core::events::RuntimeEvent;
use sumpter_core::events::RuntimeEventOutcome;
use sumpter_core::events::RuntimeEventPhase;
use sumpter_core::events::StreamTrace;
use sumpter_core::events::WebSocketTrace;
use sumpter_core::events::unix_to_apple_epoch;
use sumpter_core::routing::PlannedEndpoint;
use sumpter_core::routing::RequestPurpose;

#[derive(Clone)]
pub(super) struct WebSocketEventContext {
    pub(super) request_id: String,
    pub(super) request_path: String,
    pub(super) route_intent: String,
    pub(super) client_kind: ClientKind,
    pub(super) model: String,
    pub(super) codex_metadata: Option<CodexMetadata>,
    pub(super) client_declared: Option<ClientDeclaredMetadata>,
    pub(super) grok_metadata: Option<GrokMetadata>,
    pub(super) session_id: Option<String>,
    pub(super) started: Instant,
}

#[derive(Default)]
pub(super) struct WebSocketRelayCounters {
    pub(super) bytes_sent: AtomicU64,
    pub(super) bytes_received: AtomicU64,
    pub(super) client_message_count: AtomicU64,
    pub(super) upstream_message_count: AtomicU64,
    pub(super) failed: AtomicBool,
    pub(super) abnormal_close: AtomicBool,
    pub(super) client_close_code: Mutex<Option<i64>>,
    pub(super) upstream_close_code: Mutex<Option<i64>>,
    pub(super) closed_by: Mutex<Option<String>>,
    pub(super) relay_error: Mutex<Option<String>>,
    pub(super) first_client_text_seen: AtomicBool,
    pub(super) first_client_codex_metadata: Mutex<Option<CodexMetadata>>,
}

pub(super) struct WebSocketRelayMetrics {
    pub(super) bytes_sent: u64,
    pub(super) bytes_received: u64,
    pub(super) client_message_count: u64,
    pub(super) upstream_message_count: u64,
    pub(super) client_close_code: Option<i64>,
    pub(super) upstream_close_code: Option<i64>,
    pub(super) closed_by: Option<String>,
    pub(super) relay_error: Option<String>,
    pub(super) abnormal_close: bool,
    pub(super) failed: bool,
    pub(super) first_client_codex_metadata: Option<CodexMetadata>,
    pub(super) duration_ms: i64,
}

pub(super) async fn send_websocket_json_error(
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

pub(super) async fn relay_native_websocket(
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
    pub(super) fn observe_first_client_text(&self, message: &WebSocketMessage) {
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

    pub(super) fn set_closed_by(&self, side: &str) {
        let mut closed_by = self.closed_by.lock().unwrap();
        if closed_by.is_none() {
            *closed_by = Some(side.to_string());
        }
    }

    pub(super) fn record_error(&self, detail: &str, closed_by: &str) {
        self.failed.store(true, Ordering::Release);
        self.abnormal_close.store(true, Ordering::Release);
        self.set_closed_by(closed_by);
        let mut relay_error = self.relay_error.lock().unwrap();
        if relay_error.is_none() {
            *relay_error = Some(detail.to_string());
        }
    }

    pub(super) fn snapshot(&self, started: Instant) -> WebSocketRelayMetrics {
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

pub(super) fn axum_close_code(message: &WebSocketMessage) -> Option<i64> {
    match message {
        WebSocketMessage::Close(frame) => frame.as_ref().map(|frame| frame.code as i64),
        _ => None,
    }
}

pub(super) fn websocket_message_size(message: &WebSocketMessage) -> u64 {
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
pub(super) fn websocket_message_codex_metadata(
    message: &WebSocketMessage,
) -> Option<CodexMetadata> {
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

pub(super) fn tungstenite_message_stats(
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

pub(super) async fn send_realtime_session_update(
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

pub(super) fn websocket_connect_error(
    error: &tokio_tungstenite::tungstenite::Error,
) -> (u16, String) {
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

pub(super) fn websocket_connect_retry_after(
    error: &tokio_tungstenite::tungstenite::Error,
) -> Option<f64> {
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

pub(super) fn websocket_event_context(
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
pub(super) fn websocket_context_with_first_frame(
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

#[allow(clippy::too_many_arguments)]
pub(super) fn websocket_trace(
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

pub(super) fn websocket_client_event(
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

pub(super) fn websocket_message_model(message: &WebSocketMessage) -> Option<String> {
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

pub(super) fn websocket_message_to_tungstenite(
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
