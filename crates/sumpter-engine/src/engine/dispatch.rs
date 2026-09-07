//! Dispatch implementation for the shared engine.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, UNIX_EPOCH};

use axum::http::StatusCode;
use axum::response::Response;
use bytes::Bytes;
use serde_json::Value;
use sumpter_core::bridge;
use sumpter_core::config::{AppConfig, ProviderProtocol, RetryPolicy};
use sumpter_core::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, GrokMetadata, KIND_CLIENT, RuntimeEvent,
    RuntimeEventPhase, RuntimeFailureKind, message_tokens, unix_to_apple_epoch,
};
use sumpter_core::routing::{
    PlannedEndpoint, RequestPurpose, RouteMode, RoutePlanError, RoutePlanner, RoutingRequest,
    inspector, sticky,
};

use crate::outbound::{TransportError, UpstreamTransport};
use crate::request_build::{self, PassthroughKind, PassthroughRequest};

use super::Engine;
use super::catalog::is_media_only_conversation_model;
use super::completion::CompletionGuard;
use super::context::{
    ClientMeta, ClientOut, current_request_context, observed_session_id,
    retain_codex_metadata_for_client, upstream_request_id,
};
use super::events::new_event_id;
use super::failure::FailureInfo;
use super::http_response::{error_response, proxy_failure_response};
use super::payload::realtime_client_secret_request_session;
use super::protocol::{
    RealtimeRouteIntent, classify_realtime_intent, has_exact_codex_live_mapping,
    has_exact_realtime_mapping, is_codex_live_family_path, is_codex_live_sideband_target,
    is_live_bootstrap_request, is_realtime_http_path, is_videos_lookup_path, path_without_query,
    realtime_client_secret_models_match, required_native_protocol, translation_supported,
};
use super::sessions::realtime_ephemeral_token;
use super::state::now_unix;

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
/// Provider/model health is intentionally shorter-lived than the persisted
/// session affinity.  A transient 429/5xx should move traffic away from the
/// bad mapping, but it must not make a provider disappear until the sidecar
/// is restarted.
const PROVIDER_MODEL_COOLDOWN_INITIAL_SECS: f64 = 5.0;
const PROVIDER_MODEL_COOLDOWN_FACTOR: f64 = 2.0;
const PROVIDER_MODEL_COOLDOWN_MAX_SECS: f64 = 60.0;
const MAX_PROVIDER_MODEL_HEALTH: usize = 4096;
pub(super) const MAX_RETRY_AFTER_SECS: f64 = 30.0;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct ProviderModelHealth {
    cooling_until: Option<f64>,
    consecutive_failures: u32,
    last_status: Option<u16>,
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
pub(super) fn effective_stream_idle_timeout_for_request(
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

pub(super) fn retry_after_seconds(headers: &[(String, String)]) -> Option<f64> {
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

pub(super) fn retry_backoff_delay(round: i64, retry_after_seconds: Option<f64>) -> Duration {
    let exponent = (round - 1).clamp(0, 64) as i32;
    let exponential = (RETRY_BACKOFF_INITIAL_SECS * RETRY_BACKOFF_FACTOR.powi(exponent))
        .min(RETRY_BACKOFF_MAX_SECS);
    let retry_after = retry_after_seconds
        .filter(|seconds| seconds.is_finite() && *seconds >= 0.0)
        .unwrap_or(0.0)
        .min(RETRY_BACKOFF_MAX_SECS);
    Duration::from_secs_f64(exponential.max(retry_after))
}
impl Engine {
    pub(super) fn note_provider_model_success(&self, endpoint: &PlannedEndpoint) {
        let key = provider_model_key(endpoint);
        self.inner
            .state
            .lock()
            .unwrap()
            .provider_model_health
            .remove(&key);
    }

    fn clear_realtime_unauthorized_cooldowns(&self, candidates: &[PlannedEndpoint]) {
        if candidates.is_empty() {
            return;
        }
        let candidate_keys: Vec<(String, String)> =
            candidates.iter().map(provider_model_key).collect();
        self.inner
            .state
            .lock()
            .unwrap()
            .provider_model_health
            .retain(|key, health| {
                !(health.last_status == Some(401)
                    && candidate_keys.iter().any(|candidate| candidate == key))
            });
    }

    pub(super) fn note_provider_model_failure(
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

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_planned(
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
                    | PassthroughKind::GeminiGenerate
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
        if passthrough_kind == Some(PassthroughKind::GeminiGenerate)
            && plan
                .endpoints
                .iter()
                .any(|endpoint| !request_build::valid_gemini_model(&endpoint.upstream_model))
        {
            let message = "Gemini upstream model must be a model ID or models/<ID>";
            self.record_rejected_client_with_metadata(
                400,
                message,
                Some(request.model.clone()),
                Some(purpose),
                client_kind,
                None,
                ClientDeclaredMetadata::from_headers(&headers),
                Some(source_format),
            );
            return error_response(
                StatusCode::BAD_REQUEST,
                &[("error", "invalid_gemini_mapping"), ("message", message)],
            );
        }
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

        let observed_session_id = observed_session_id(&headers);
        let session_identity =
            sticky::resolve_session_identity(&request, observed_session_id.as_deref());
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
            sticky_key: Some(session_key.value.clone()),
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
            model_group_id: None,
            model_group_name: None,
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
            sticky_key: Some(session_key.value.clone()),
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

    pub(super) fn ordered_endpoints(
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
        let mut groups: Vec<(String, usize, i64, usize)> = Vec::new();
        for (index, endpoint) in eligible_endpoints.iter().enumerate() {
            let group = endpoint.scheduling_group().to_string();
            if let Some(existing) = groups.iter_mut().find(|item| item.0 == group) {
                existing.2 = existing.2.min(endpoint.priority);
            } else {
                groups.push((group, endpoint.model_group_rank, endpoint.priority, index));
            }
        }
        groups.sort_by(|a, b| {
            let a_preferred = sticky_preferred.as_deref() == Some(a.0.as_str());
            let b_preferred = sticky_preferred.as_deref() == Some(b.0.as_str());
            b_preferred
                .cmp(&a_preferred)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.3.cmp(&b.3))
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
    pub(super) fn provider_model_cooldown_retry_after(
        &self,
        candidates: &[PlannedEndpoint],
    ) -> Option<f64> {
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
        let realtime_request = client_out
            .as_ref()
            .is_some_and(|client| client.passthrough_kind == PassthroughKind::Realtime);
        // Only a Codex Live/Quicksilver bootstrap is safe to replay inside
        // the same admitted request.  Public Realtime control/sideband
        // requests must remain single-shot because replaying them can create
        // duplicate sessions or mutate an already established call.
        let live_bootstrap_request =
            realtime_request && is_live_bootstrap_request(method, path_and_query);
        guard.set_passthrough(
            client_out
                .as_ref()
                .and_then(|client| client.passthrough.as_ref().map(|_| client.passthrough_kind)),
        );
        if realtime_request {
            // A prior binary may have recorded a Live 401 before the
            // account-level cooldown rule was installed. Clear only those
            // stale entries at admission so a hot reload can recover without
            // weakening cooldowns created by 429/5xx/transport failures.
            self.clear_realtime_unauthorized_cooldowns(&candidates);
        }
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
        let sticky_retry_limit =
            if sticky_endpoint_count > 0 && (!realtime_request || live_bootstrap_request) {
                // CPA's Live handler owns OAuth-account selection.  Replaying a
                // failed bootstrap gives CPA a chance to exclude the revoked
                // OAuth credential and select the next account, while retaining
                // the configured finite bound.  Other Realtime requests remain
                // single-shot (see `live_bootstrap_request` above).
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
                // A registered `ek_…` token is a valid *standard Realtime*
                // credential in its own right.  The dedicated Codex Live
                // family (`/v1/live`) is different: CPA authenticates that
                // route with the endpoint API key and selects its OAuth
                // account internally.  Forwarding a Codex client's unrelated
                // `ek_…` token to `/v1/live` makes CPA reject the request (or
                // bypass its account selection) even when the endpoint key
                // and OAuth pool are valid.  Preserve ephemeral credentials
                // only on the public Realtime surface; ordinary APIs always
                // use the configured Provider key.
                let path = path_without_query(path_and_query);
                let registered_realtime_secret = (is_realtime_http_path(path)
                    && !is_codex_live_family_path(path))
                .then(|| {
                    self.realtime_client_secret_authorized(headers)
                        .then(|| realtime_ephemeral_token(headers))
                        .flatten()
                })
                .flatten();
                let api_key = if let Some(secret) = registered_realtime_secret {
                    secret
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

                let response_timeout = effective_response_timeout_for_request(
                    retry,
                    endpoint,
                    client_out
                        .as_ref()
                        .is_some_and(|client| client.passthrough_kind == PassthroughKind::Realtime),
                );
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
                let build = request_build::build_outbound(
                    endpoint,
                    request,
                    headers,
                    method,
                    path_and_query,
                    &api_key,
                    guard.meta.purpose,
                    passthrough,
                );
                let attempt_started = Instant::now();
                let capture_attempt_id = self.capture_attempt_started(
                    &request_id,
                    endpoint,
                    &build.request,
                    attempt_started,
                );
                let result = send_streaming_with_deadline(
                    &self.inner.transport,
                    build.request,
                    response_timeout,
                )
                .await;
                self.capture_attempt_result(&request_id, &capture_attempt_id, &result);
                let attempt = self.note_attempt(
                    endpoint,
                    attempt_started,
                    is_failover,
                    guard.meta.purpose,
                    guard.meta.client_kind,
                    &request_id,
                    response_timeout,
                    result,
                    realtime_request,
                    &mut round_state,
                );

                if let Some(response) = attempt {
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

    /// 单次尝试的统一记账:accepted 返回响应;可重试/失败记入 round_state 并返回 None。
    /// 失败事件保留上游状态、传输错误与重试上下文。
    #[allow(clippy::too_many_arguments)]
    fn note_attempt(
        &self,
        endpoint: &PlannedEndpoint,
        attempt_started: Instant,
        is_failover: bool,
        purpose: RequestPurpose,
        client_kind: ClientKind,
        request_id: &str,
        response_timeout: Option<f64>,
        result: Result<crate::outbound::UpstreamResponse, TransportError>,
        realtime_request: bool,
        round_state: &mut RoundState,
    ) -> Option<crate::outbound::UpstreamResponse> {
        let now = now_unix();
        match result {
            Err(e) => {
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
                    Some(e.to_string()),
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
                if RetryPolicy::is_endpoint_retryable_status(response.status) {
                    let attempt_ttfb_ms = attempt_started.elapsed().as_millis() as i64;
                    let retry_after = retry_after_seconds(&response.headers);
                    // A CPA Live/Realtime 401 belongs to the OAuth account
                    // selected inside CPA, not to the CPA endpoint or model
                    // mapping. CPA marks that account unavailable and selects
                    // another account on the next client bootstrap. Cooling
                    // the whole endpoint here prevents that request from ever
                    // reaching CPA and turns the next attempt into a local
                    // 503. Keep the 401 event/client response, but delegate
                    // account health to CPA. Other statuses and non-Realtime
                    // requests retain the ordinary provider/model cooldown.
                    if !(realtime_request && response.status == 401) {
                        self.note_provider_model_failure(
                            endpoint,
                            Some(response.status),
                            retry_after,
                            now,
                        );
                    }
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
                        None,
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
}
