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
use futures_util::StreamExt;
use futures_util::{Sink, SinkExt, Stream};
use serde_json::Value;
use serde_json::json;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
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
use tokio::sync::{mpsc, oneshot, watch};
use tokio_tungstenite::tungstenite::Error as WsError;

#[derive(Clone)]
pub(super) struct WebSocketEventContext {
    pub(super) client_upgraded: bool,
    pub(super) first_message_wait_ms: Option<i64>,
    pub(super) source_ip: Option<String>,
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
    client_closed: AtomicBool,
    upstream_closed: AtomicBool,
    pub(super) client_close_code: Mutex<Option<i64>>,
    pub(super) upstream_close_code: Mutex<Option<i64>>,
    pub(super) closed_by: Mutex<Option<String>>,
    pub(super) relay_error: Mutex<Option<String>>,
    pub(super) transport_error_kind: Mutex<Option<String>>,
    pub(super) last_event_type: Mutex<Option<String>>,
    pub(super) idle_timeout_ms: Mutex<Option<i64>>,
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
    pub(super) transport_error_kind: Option<String>,
    pub(super) last_event_type: Option<String>,
    pub(super) idle_timeout_ms: Option<i64>,
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

/// One pending business message per direction; small control queues cannot retain
/// arbitrary prompt bodies. Close stays in the business queue to preserve ordering.
#[derive(Clone)]
struct RelayQueue {
    data: mpsc::Sender<RelayData>,
    control: mpsc::Sender<ControlWrite>,
}

#[derive(Debug)]
enum RelayData {
    Frame(WebSocketMessage),
    End,
}

enum ControlWrite {
    Frame(WebSocketMessage),
    // Tungstenite already queued the Pong/Close reply. Flush, never replace it
    // with a manually generated Pong (which can overwrite the automatic reply).
    Flush(Option<oneshot::Sender<()>>),
}

#[derive(Clone, Copy)]
enum Peer {
    Client,
    Upstream,
}

impl Peer {
    fn name(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Upstream => "upstream",
        }
    }
    fn read_error(self) -> &'static str {
        match self {
            Self::Client => "client_receive_failed",
            Self::Upstream => "upstream_receive_failed",
        }
    }
    fn write_error(self) -> &'static str {
        match self {
            Self::Client => "upstream_to_client_send_failed",
            Self::Upstream => "client_to_upstream_send_failed",
        }
    }
    fn flush_error(self) -> &'static str {
        match self {
            Self::Client => "client_control_flush_failed",
            Self::Upstream => "upstream_control_flush_failed",
        }
    }
}

pub(super) fn transport_error_kind(error: &WsError) -> &'static str {
    use std::io::ErrorKind;
    use tokio_tungstenite::tungstenite::error::ProtocolError;
    match error {
        WsError::Io(error) => match error.kind() {
            ErrorKind::ConnectionReset => "connection_reset",
            ErrorKind::ConnectionAborted => "connection_aborted",
            ErrorKind::UnexpectedEof => "unexpected_eof",
            ErrorKind::BrokenPipe => "broken_pipe",
            ErrorKind::TimedOut => "timed_out",
            _ => "io_error",
        },
        WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake) => "eof_without_close",
        WsError::Protocol(_) => "protocol_error",
        WsError::Utf8(_) => "invalid_utf8",
        WsError::Tls(_) => "tls_error",
        WsError::Capacity(_) | WsError::WriteBufferFull(_) => "capacity_error",
        WsError::ConnectionClosed | WsError::AlreadyClosed => "connection_closed",
        _ => "transport_error",
    }
}

fn axum_transport_error_kind(error: axum::Error) -> &'static str {
    error
        .into_inner()
        .downcast_ref::<WsError>()
        .map(transport_error_kind)
        .unwrap_or("transport_error")
}

pub(super) async fn relay_native_websocket(
    socket: WebSocket,
    upstream: NativeWebSocket,
    initial_message: Option<WebSocketMessage>,
    shutdown: tokio_util::sync::CancellationToken,
    idle_timeout: Option<Duration>,
) -> WebSocketRelayMetrics {
    let started = Instant::now();
    let counters = Arc::new(WebSocketRelayCounters::default());
    let cancellation = tokio_util::sync::CancellationToken::new();
    let (activity, activity_rx) = watch::channel(
        initial_message
            .as_ref()
            .map(|_| tokio::time::Instant::now()),
    );
    let (client_tx, client_rx) = socket.split();
    let (upstream_tx, upstream_rx) = upstream.split();
    let (client_data_tx, client_data_rx) = mpsc::channel(1);
    let (client_control_tx, client_control_rx) = mpsc::channel(8);
    let (upstream_data_tx, upstream_data_rx) = mpsc::channel(1);
    let (upstream_control_tx, upstream_control_rx) = mpsc::channel(8);
    let client_queue = RelayQueue {
        data: client_data_tx,
        control: client_control_tx,
    };
    let upstream_queue = RelayQueue {
        data: upstream_data_tx,
        control: upstream_control_tx,
    };

    // Seed the first frame before starting readers, but do NOT await its network
    // write: upstream Ping/Close must remain readable even during this first send.
    if let Some(message) = initial_message {
        counters.observe_first_client_text(&message);
        upstream_queue
            .data
            .try_send(RelayData::Frame(message))
            .unwrap_or_else(|_| unreachable!("empty relay queue"));
    }
    let client_reader = relay_reader(
        client_rx.map(|item| item.map(Some).map_err(axum_transport_error_kind)),
        Peer::Client,
        &client_queue,
        &upstream_queue,
        &counters,
        &activity,
    );
    let upstream_reader = relay_reader(
        upstream_rx.map(|item| {
            item.map(tungstenite_to_axum)
                .map_err(|error| transport_error_kind(&error))
        }),
        Peer::Upstream,
        &upstream_queue,
        &client_queue,
        &counters,
        &activity,
    );
    let client_writer = relay_writer(
        client_tx.sink_map_err(axum_transport_error_kind),
        Peer::Client,
        client_data_rx,
        client_control_rx,
        &counters,
        &activity,
        &cancellation,
    );
    let upstream_writer = relay_writer(
        upstream_tx
            .with(|message| {
                std::future::ready(Ok::<_, WsError>(websocket_message_to_tungstenite(message)))
            })
            .sink_map_err(|error| transport_error_kind(&error)),
        Peer::Upstream,
        upstream_data_rx,
        upstream_control_rx,
        &counters,
        &activity,
        &cancellation,
    );
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => counters.record_transport_error("server_shutdown", "server", "server_shutdown"),
        _ = cancellation.cancelled() => {},
        _ = wait_for_idle(activity_rx, idle_timeout) => {
            *counters.idle_timeout_ms.lock().unwrap() = idle_timeout.map(|timeout| timeout.as_millis().min(i64::MAX as u128) as i64);
            counters.record_transport_error("idle_timeout", "relay_error", "idle_timeout");
        },
        _ = async { tokio::join!(client_reader, upstream_reader, client_writer, upstream_writer); } => {},
    }
    // Dropping all four futures releases sockets even if send/flush is blocked.
    counters.snapshot(started)
}

fn mark_activity(activity: &watch::Sender<Option<tokio::time::Instant>>, business: bool) {
    activity.send_modify(|last| {
        if business || last.is_some() {
            *last = Some(tokio::time::Instant::now());
        }
    });
}

async fn wait_for_idle(
    mut activity: watch::Receiver<Option<tokio::time::Instant>>,
    timeout: Option<Duration>,
) {
    let Some(timeout) = timeout else {
        return std::future::pending().await;
    };
    loop {
        let last = *activity.borrow_and_update();
        let Some(last) = last else {
            if activity.changed().await.is_err() {
                return std::future::pending().await;
            }
            continue;
        };
        let deadline = last + timeout;
        tokio::select! {
            biased;
            result = activity.changed() => if result.is_err() { return std::future::pending().await; },
            _ = tokio::time::sleep_until(deadline) => return,
        }
    }
}

async fn relay_reader<R>(
    mut reader: R,
    peer: Peer,
    local: &RelayQueue,
    remote: &RelayQueue,
    counters: &WebSocketRelayCounters,
    activity: &watch::Sender<Option<tokio::time::Instant>>,
) where
    R: Stream<Item = Result<Option<WebSocketMessage>, &'static str>> + Unpin,
{
    loop {
        let message = match reader.next().await {
            Some(Ok(Some(message))) => message,
            Some(Ok(None)) => continue,
            next => {
                let kind = next.and_then(Result::err).unwrap_or("eof_without_close");
                counters.record_transport_error(peer.read_error(), peer.name(), kind);
                // Deliver already-read frames before reporting a transport end.
                // Otherwise a fast EOF can discard the final response chunk.
                let _ = remote.data.send(RelayData::End).await;
                return;
            }
        };
        mark_activity(
            activity,
            matches!(
                message,
                WebSocketMessage::Text(_) | WebSocketMessage::Binary(_)
            ),
        );
        if matches!(peer, Peer::Client) {
            counters.observe_first_client_text(&message);
        } else {
            counters.observe_upstream_event(&message);
        }
        if matches!(message, WebSocketMessage::Close(_)) {
            counters.record_close(peer, axum_close_code(&message));
            let (done, flushed) = oneshot::channel();
            if local
                .control
                .send(ControlWrite::Flush(Some(done)))
                .await
                .is_err()
            {
                return;
            }
            let _ = flushed.await;
            // Unlike Ping/Pong, Close must not overtake preceding payloads.
            let _ = remote.data.send(RelayData::Frame(message)).await;
            return;
        }
        if matches!(message, WebSocketMessage::Ping(_))
            && local.control.send(ControlWrite::Flush(None)).await.is_err()
        {
            return;
        }
        let result = if matches!(
            message,
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_)
        ) {
            remote
                .control
                .send(ControlWrite::Frame(message))
                .await
                .map_err(|_| ())
        } else {
            remote
                .data
                .send(RelayData::Frame(message))
                .await
                .map_err(|_| ())
        };
        if result.is_err() {
            return;
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn relay_writer<S>(
    mut sink: S,
    peer: Peer,
    mut data: mpsc::Receiver<RelayData>,
    mut control: mpsc::Receiver<ControlWrite>,
    counters: &WebSocketRelayCounters,
    activity: &watch::Sender<Option<tokio::time::Instant>>,
    cancellation: &tokio_util::sync::CancellationToken,
) where
    S: Sink<WebSocketMessage, Error = &'static str> + Unpin,
{
    loop {
        let command = tokio::select! {
            biased;
            Some(command) = control.recv() => command,
            Some(message) = data.recv() => match message {
                RelayData::Frame(message) => ControlWrite::Frame(message),
                RelayData::End => { cancellation.cancel(); return; }
            },
            else => return,
        };
        let message = match command {
            ControlWrite::Flush(done) => {
                if let Err(kind) = sink.flush().await {
                    // A completed automatic Close reply is a normal library result.
                    if kind != "connection_closed" || done.is_none() {
                        counters.record_transport_error(peer.flush_error(), peer.name(), kind);
                        cancellation.cancel();
                        return;
                    }
                }
                if let Some(done) = done {
                    let _ = done.send(());
                }
                continue;
            }
            ControlWrite::Frame(message) => message,
        };
        let bytes = websocket_message_size(&message);
        let is_business = matches!(
            message,
            WebSocketMessage::Text(_) | WebSocketMessage::Binary(_)
        );
        let is_close = matches!(message, WebSocketMessage::Close(_));
        if counters.peer_closed(peer) {
            if is_close {
                cancellation.cancel();
                return;
            }
            continue;
        }
        if let Err(kind) = sink.send(message).await {
            counters.record_transport_error(peer.write_error(), peer.name(), kind);
            cancellation.cancel();
            return;
        }
        mark_activity(activity, is_business);
        let (byte_counter, message_counter) = match peer {
            Peer::Client => (&counters.bytes_received, &counters.upstream_message_count),
            Peer::Upstream => (&counters.bytes_sent, &counters.client_message_count),
        };
        byte_counter.fetch_add(bytes, Ordering::Relaxed);
        message_counter.fetch_add(1, Ordering::Relaxed);
        if is_close {
            cancellation.cancel();
            return;
        }
    }
}

fn tungstenite_to_axum(
    message: tokio_tungstenite::tungstenite::Message,
) -> Option<WebSocketMessage> {
    use tokio_tungstenite::tungstenite::Message;
    Some(match message {
        Message::Text(text) => WebSocketMessage::Text(text.to_string().into()),
        Message::Binary(bytes) => WebSocketMessage::Binary(bytes),
        Message::Ping(bytes) => WebSocketMessage::Ping(bytes),
        Message::Pong(bytes) => WebSocketMessage::Pong(bytes),
        Message::Close(frame) => {
            WebSocketMessage::Close(frame.map(|frame| axum::extract::ws::CloseFrame {
                code: frame.code.into(),
                reason: frame.reason.to_string().into(),
            }))
        }
        Message::Frame(_) => return None,
    })
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

    fn peer_closed(&self, peer: Peer) -> bool {
        match peer {
            Peer::Client => &self.client_closed,
            Peer::Upstream => &self.upstream_closed,
        }
        .load(Ordering::Acquire)
    }

    fn record_close(&self, peer: Peer, code: Option<i64>) {
        match peer {
            Peer::Client => &self.client_closed,
            Peer::Upstream => &self.upstream_closed,
        }
        .store(true, Ordering::Release);
        let target = match peer {
            Peer::Client => &self.client_close_code,
            Peer::Upstream => &self.upstream_close_code,
        };
        *target.lock().unwrap() = code;
        self.set_closed_by(peer.name());
        if !matches!(code, None | Some(1000 | 1001)) {
            self.record_transport_error("abnormal_close", peer.name(), "abnormal_close");
        }
    }

    fn observe_upstream_event(&self, message: &WebSocketMessage) {
        if matches!(
            message,
            WebSocketMessage::Ping(_) | WebSocketMessage::Pong(_) | WebSocketMessage::Close(_)
        ) {
            return;
        }
        *self.last_event_type.lock().unwrap() = None;
        let WebSocketMessage::Text(text) = message else {
            return;
        };
        if text.len() > MAX_WEBSOCKET_METADATA_FRAME_BYTES {
            return;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return;
        };
        // Never store arbitrary type strings: a peer could place prompt/secret
        // text there. Unknown/oversize events clear the previous type.
        let event = value
            .get("type")
            .and_then(Value::as_str)
            .and_then(|kind| match kind {
                "response.created"
                | "response.in_progress"
                | "response.completed"
                | "response.done"
                | "response.failed"
                | "response.incomplete"
                | "response.output_item.added"
                | "response.output_item.done"
                | "response.output_text.delta"
                | "response.output_text.done"
                | "response.content_part.added"
                | "response.content_part.done"
                | "response.function_call_arguments.delta"
                | "response.function_call_arguments.done"
                | "response.reasoning_summary_text.delta"
                | "response.reasoning_summary_text.done"
                | "response.steer.accepted"
                | "response.steer.failed"
                | "response.steer.pending"
                | "response.interrupted"
                | "codex.response.metadata"
                | "codex.rate_limits"
                | "error" => Some(kind.to_string()),
                _ => None,
            });
        *self.last_event_type.lock().unwrap() = event;
    }

    pub(super) fn record_transport_error(&self, detail: &str, side: &str, kind: &str) {
        self.record_error(detail, side);
        let mut target = self.transport_error_kind.lock().unwrap();
        if target.is_none() {
            *target = Some(kind.into());
        }
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
            transport_error_kind: self.transport_error_kind.lock().unwrap().clone(),
            last_event_type: self.last_event_type.lock().unwrap().clone(),
            idle_timeout_ms: *self.idle_timeout_ms.lock().unwrap(),
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
    merge_codex_metadata(direct, nested)
}

#[cfg(test)]
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

/// `source_ip` 已按转发头规则解析(与 HTTP 入口同一函数),调用方从
/// `Engine::inbound_request_context` 取得后传入,握手与中继事件共用一份。
pub(super) fn websocket_event_context(
    source_ip: Option<String>,
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
        client_upgraded: false,
        first_message_wait_ms: None,
        source_ip,
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
/// prompt or raw JSON. Frame body metadata takes priority over handshake
/// compatibility headers, matching the HTTP parser's precedence.
pub(super) fn websocket_context_with_first_frame(
    context: &WebSocketEventContext,
    frame: Option<&CodexMetadata>,
) -> WebSocketEventContext {
    let Some(frame) = frame else {
        return context.clone();
    };
    let mut merged = context.clone();
    // pi session identity comes from its handshake headers. Responses frame
    // metadata must not relabel pi as Codex or override explicit attribution.
    if merged.client_kind == ClientKind::Pi {
        return merged;
    }
    merged.codex_metadata = merge_codex_metadata(Some(frame.clone()), merged.codex_metadata.take());
    merged.session_id = merged
        .codex_metadata
        .as_ref()
        .and_then(|metadata| metadata.session_id.clone())
        .or(merged.session_id);
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
    client_handshake_status: Option<i64>,
    stage: &str,
    first_message_wait_ms: Option<i64>,
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
        tool_calls_truncated: false,
        cache_read_evidence: None,
        chunk_count: None,
        bytes_received: None,
        max_chunk_gap_ms: None,
        last_chunk_at_ms: None,
        terminal_event: None,
        usage: None,
        stop_reason: None,
        websocket_trace: Some(WebSocketTrace {
            client_handshake_status,
            stage: Some(stage.into()),
            first_message_wait_ms,
            handshake_status: (handshake_status > 0).then_some(handshake_status),
            bytes_sent: Some(bytes_sent),
            bytes_received: Some(bytes_received),
            client_message_count: Some(client_message_count),
            upstream_message_count: Some(upstream_message_count),
            client_close_code,
            upstream_close_code,
            closed_by,
            relay_error,
            abnormal_close: Some(abnormal_close),
            attempt_count: Some(attempt_count),
            ..Default::default()
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
        source_ip: context.source_ip.clone(),
        session_source: None,
        hook_event: None,
        cache_read: None,
        client_kind: Some(context.client_kind),
        client_variant: None,
        agent_role: None,
        agent_name: None,
        parent_thread_id: None,
        parent_turn_id: None,
        root_turn_id: None,
        codex_metadata: context.codex_metadata.clone(),
        client_declared: context.client_declared.clone(),
        grok_metadata: context.grok_metadata.clone(),
        client_model: (!context.model.is_empty()).then(|| context.model.clone()),
        source_format: Some(ProviderProtocol::OpenAI),
        target_format: endpoint.map(|endpoint| endpoint.protocol),
        route_mode: endpoint.map(|endpoint| endpoint.route_mode),
        duration_ms: context.started.elapsed().as_millis().min(i64::MAX as u128) as i64,
        effective_model: (!context.model.is_empty()).then(|| context.model.clone()),
        endpoint_id: endpoint.map(|endpoint| endpoint.endpoint_id.clone()),
        endpoint_name: endpoint.map(|endpoint| endpoint.endpoint_name.clone()),
        model_group_id: None,
        model_group_name: None,
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
        request_purpose: Some(RequestPurpose::Standard),
        request_id: Some(context.request_id.clone()),
        request_method: Some("GET".into()),
        request_path: Some(context.request_path.clone()),
        route_intent: Some(context.route_intent.clone()),
        session_id: context.session_id.clone(),
        // WebSocket 排序键是合成的 websocket:{model},不是会话粘性归属。
        sticky_key: None,
        status_code: status,
        timestamp: unix_to_apple_epoch(now_unix()),
        ttfb_ms: None,
        stream_trace,
        timeout_ms: None,
        upstream_host: endpoint.and_then(|endpoint| host_of(&endpoint.base_url)),
        upstream_model: endpoint.map(|endpoint| endpoint.upstream_model.clone()),
        upstream_request_id: None,
        upstream_status_code: endpoint.and_then(|_| (status > 0).then_some(status)),
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

#[cfg(test)]
mod relay_tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn idle_deadline_is_optional_waits_for_business_and_resets_on_control_activity() {
        let (activity, rx) = watch::channel(None);
        let deadline = wait_for_idle(rx.clone(), Some(Duration::from_secs(10)));
        tokio::pin!(deadline);
        assert!(
            tokio::time::timeout(Duration::from_secs(100), &mut deadline)
                .await
                .is_err()
        );
        mark_activity(&activity, false);
        assert!(
            tokio::time::timeout(Duration::from_secs(100), &mut deadline)
                .await
                .is_err()
        );
        mark_activity(&activity, true);
        assert!(
            tokio::time::timeout(Duration::from_secs(9), &mut deadline)
                .await
                .is_err()
        );
        mark_activity(&activity, false);
        assert!(
            tokio::time::timeout(Duration::from_secs(9), &mut deadline)
                .await
                .is_err()
        );
        tokio::time::timeout(Duration::from_secs(2), deadline)
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(3600), wait_for_idle(rx, None))
                .await
                .is_err()
        );
    }

    #[test]
    fn event_observer_does_not_store_untrusted_types_or_stale_large_frame_metadata() {
        let counters = WebSocketRelayCounters::default();
        for text in [
            r#"{"type":"response.completed","secret":"PRIVATE"}"#.to_string(),
            r#"{"type":"PRIVATE_EVENT"}"#.to_string(),
            format!(
                r#"{{"type":"response.completed","secret":"{}"}}"#,
                "PRIVATE".repeat(20_000)
            ),
            "PRIVATE_INVALID_JSON".into(),
        ] {
            counters.observe_upstream_event(&WebSocketMessage::Text(text.into()));
            assert!(!format!("{:?}", counters.last_event_type.lock().unwrap()).contains("PRIVATE"));
        }
        assert!(counters.last_event_type.lock().unwrap().is_none());
    }

    #[test]
    fn transport_categories_use_typed_errors_and_trace_remains_backward_compatible() {
        use tokio_tungstenite::tungstenite::error::ProtocolError;
        for (error, expected) in [
            (
                WsError::Io(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "PRIVATE",
                )),
                "connection_reset",
            ),
            (
                WsError::Protocol(ProtocolError::ResetWithoutClosingHandshake),
                "eof_without_close",
            ),
            (WsError::Utf8("PRIVATE".into()), "invalid_utf8"),
        ] {
            assert_eq!(transport_error_kind(&error), expected);
        }
        let old: WebSocketTrace =
            serde_json::from_str(r#"{"stage":"relay","handshakeStatus":101}"#).unwrap();
        assert!(
            old.transport_error_kind.is_none()
                && old.last_event_type.is_none()
                && old.idle_timeout_ms.is_none()
        );
        let encoded = serde_json::to_value(&old).unwrap();
        assert!(encoded.get("transportErrorKind").is_none());
        let trace = WebSocketTrace {
            transport_error_kind: Some("connection_reset".into()),
            last_event_type: Some("response.completed".into()),
            idle_timeout_ms: Some(20),
            ..old
        };
        assert_eq!(
            serde_json::from_value::<WebSocketTrace>(serde_json::to_value(&trace).unwrap())
                .unwrap(),
            trace
        );
    }

    // A controllable sink separates actual write failures from automatic
    // control flush failures, which are not reproducible reliably with TCP RST.
    struct FailedSink;
    impl Sink<WebSocketMessage> for FailedSink {
        type Error = &'static str;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(
            self: std::pin::Pin<&mut Self>,
            _: WebSocketMessage,
        ) -> Result<(), Self::Error> {
            Err("broken_pipe")
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Err("connection_reset"))
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn priority_control_flush_and_payload_write_failures_keep_the_peer_and_category() {
        for peer in [Peer::Client, Peer::Upstream] {
            for control_failure in [true, false] {
                let (data, data_rx) = mpsc::channel(1);
                let (control, control_rx) = mpsc::channel(1);
                // Both are ready: control flush must win over payload writes.
                data.send(RelayData::Frame(WebSocketMessage::Text("PRIVATE".into())))
                    .await
                    .unwrap();
                if control_failure {
                    control.send(ControlWrite::Flush(None)).await.ok().unwrap();
                }
                let counters = WebSocketRelayCounters::default();
                let cancellation = tokio_util::sync::CancellationToken::new();
                let (activity, _) = watch::channel(None);
                relay_writer(
                    FailedSink,
                    peer,
                    data_rx,
                    control_rx,
                    &counters,
                    &activity,
                    &cancellation,
                )
                .await;
                assert!(cancellation.is_cancelled());
                assert_eq!(
                    counters.closed_by.lock().unwrap().as_deref(),
                    Some(peer.name())
                );
                assert_eq!(
                    counters.relay_error.lock().unwrap().as_deref(),
                    Some(if control_failure {
                        peer.flush_error()
                    } else {
                        peer.write_error()
                    })
                );
                assert_eq!(
                    counters.transport_error_kind.lock().unwrap().as_deref(),
                    Some(if control_failure {
                        "connection_reset"
                    } else {
                        "broken_pipe"
                    })
                );
                assert_eq!(counters.bytes_sent.load(Ordering::Relaxed), 0);
                assert_eq!(counters.bytes_received.load(Ordering::Relaxed), 0);
            }
        }
    }
}
