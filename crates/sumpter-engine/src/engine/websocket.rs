//! Native WebSocket authorization, route selection and upstream handshakes.
//! Prepared connections retain the existing adapter upgrade contract; frame
//! forwarding and completion metrics live in `websocket_relay`.

use super::Engine;
use super::context::detect_client_kind;
use super::context::header_value;
use super::context::inbound_auth_ok;
use super::dispatch::MAX_RETRY_AFTER_SECS;
use super::failure::FailureInfo;
use super::protocol::RealtimeRouteIntent;
use super::protocol::classify_realtime_intent;
use super::protocol::decoded_query_value;
use super::protocol::has_exact_codex_live_mapping;
use super::protocol::has_exact_realtime_mapping;
use super::protocol::is_codex_live_family_path;
use super::protocol::is_codex_live_sideband_target;
use super::protocol::is_realtime_http_path;
use super::protocol::is_responses_websocket_path;
use super::protocol::path_without_query;
use super::protocol::realtime_call_path_error;
use super::protocol::realtime_client_secret_models_match;
use super::protocol::resolve_realtime_route_model;
use super::protocol::validate_realtime_call_target;
use super::sessions::realtime_ephemeral_token;
use super::state::now_unix;
use super::websocket_relay::WebSocketEventContext;
use super::websocket_relay::WebSocketRelayMetrics;
use super::websocket_relay::relay_native_websocket;
use super::websocket_relay::send_realtime_session_update;
use super::websocket_relay::send_websocket_json_error;
use super::websocket_relay::websocket_client_event;
use super::websocket_relay::websocket_connect_error;
use super::websocket_relay::websocket_connect_retry_after;
use super::websocket_relay::websocket_context_with_first_frame;
use super::websocket_relay::websocket_event_context;
use super::websocket_relay::websocket_message_codex_metadata;
use super::websocket_relay::websocket_message_model;
use super::websocket_relay::websocket_trace;
use crate::request_build;
use axum::body::Body;
use axum::extract::ws::WebSocket;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;
use std::net::IpAddr;
use std::time::Duration;
use std::time::Instant;
use sumpter_core::access;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::config::RetryPolicy;
use sumpter_core::events::RuntimeEventOutcome;
use sumpter_core::events::RuntimeFailureKind;
use sumpter_core::events::RuntimeFailurePhase;
use sumpter_core::routing::PlannedEndpoint;
use sumpter_core::routing::RequestPurpose;
use sumpter_core::routing::RoutePlanner;
use sumpter_core::routing::RoutingRequest;
use sumpter_core::routing::sticky;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

/// A WebSocket upgrade has no HTTP response-body relay where the ordinary
/// response-header timeout can be applied, so protect the upstream TCP/TLS/
/// handshake independently.
const REALTIME_WEBSOCKET_CONNECT_TIMEOUT_SECS: f64 = 15.0;
pub(super) const MAX_WEBSOCKET_METADATA_FRAME_BYTES: usize = 64 * 1024;

pub(super) type NativeWebSocket =
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
pub(super) fn websocket_upstream_headers(headers: &[(String, String)]) -> Vec<(String, String)> {
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
                        | "x-sumpter-client"
                        | "x-sumpter-session-id"
                        | "x-sumpter-attribution-encoding"
                        | "x-sumpter-workspace"
                        | "x-sumpter-git-remote"
                        | "x-sumpter-user"
                )
        })
        .collect()
}

pub(super) fn websocket_url(base_url: &str, path_and_query: &str) -> Option<String> {
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
impl Engine {
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
        if self.runtime_database_issue().is_some() {
            return false;
        }
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
            upstream.session_id = context.session_id.clone();
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
        upstream.session_id = context.session_id.clone();
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
        // Local provider health must not synthesize a 503. Always attempt the
        // compatible upstream and preserve its handshake status/error.
        let ordered = self.ordered_endpoint_candidates(&plan.endpoints, &ordering_key, false, true);
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
            let mut upstream_headers = websocket_upstream_headers(headers);
            request_build::apply_user_agent(
                &mut upstream_headers,
                &endpoint.user_agent,
                endpoint.protocol,
                true,
            );
            for (name, value) in upstream_headers {
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
}
