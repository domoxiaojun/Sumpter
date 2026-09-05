//! HTTP ingress: envelope conversion, access checks, request classification,
//! body consumption and route preparation. Bodies remain lazy until authorized;
//! protocol-specific normalization preserves the existing transport contract.

use super::Engine;
use super::catalog::is_local_models_path;
use super::catalog::local_models_response;
use super::context::ClientOut;
use super::context::INBOUND_REQUEST_CONTEXT;
use super::context::InboundRequestContext;
use super::context::bounded_request_method;
use super::context::bounded_request_path;
use super::context::detect_client_kind;
use super::context::header_value;
use super::context::inbound_auth_ok;
use super::context::observed_session_id;
use super::context::set_current_route_intent;
use super::http_response::error_response;
use super::http_response::json_response;
use super::http_response::method_not_allowed_response;
use super::payload::NativePassthroughFields;
use super::payload::apply_realtime_client_secret_session;
use super::payload::content_type_is_json;
use super::payload::content_type_is_multipart;
use super::payload::metadata_json_body_hint;
use super::payload::native_json_fields;
use super::payload::native_multipart_fields;
use super::payload::native_multipart_fields_with_default;
use super::payload::raw_body_model_hint;
use super::payload::raw_request_body_hint;
use super::payload::realtime_body_model;
use super::payload::realtime_body_model_hint;
use super::payload::rewrite_realtime_multipart_model;
use super::payload::valid_anthropic_messages_request;
use super::protocol::RealtimeRouteIntent;
use super::protocol::body_indicates_codex_live;
use super::protocol::classify_realtime_intent;
use super::protocol::decoded_query_value;
use super::protocol::is_openai_resource_tree;
use super::protocol::is_realtime_call_bootstrap_path;
use super::protocol::is_realtime_client_secret_path;
use super::protocol::is_resource_passthrough_kind;
use super::protocol::is_videos_lookup_path;
use super::protocol::native_passthrough_kind;
use super::protocol::path_without_query;
use super::protocol::realtime_call_path_error;
use super::protocol::realtime_client_secret_models_match;
use super::protocol::resolve_realtime_route_model;
use super::protocol::route_intent_for_path;
use super::protocol::source_format_for_passthrough;
use super::protocol::validate_realtime_call_target;
use crate::boundary::AccessDecision;
use crate::boundary::ControlRequestMeta;
use crate::boundary::PlatformRequest;
use crate::request_build;
use crate::request_build::PassthroughKind;
use axum::body::Body;
use axum::http::StatusCode;
use axum::http::{HeaderMap, Uri};
use axum::response::Response;
use serde_json::Value;
use serde_json::json;
use std::cell::RefCell;
use std::net::IpAddr;
use std::sync::Arc;
use sumpter_core::access;
use sumpter_core::bridge_in;
use sumpter_core::bridge_in::ClientDialect;
use sumpter_core::config::AppConfig;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::ClientDeclaredMetadata;
use sumpter_core::events::ClientKind;
use sumpter_core::events::CodexMetadata;
use sumpter_core::events::GrokMetadata;
use sumpter_core::events::message_tokens;
use sumpter_core::routing::RequestPurpose;
use sumpter_core::routing::RoutePlanner;
use sumpter_core::routing::RoutingRequest;
use sumpter_core::stream_terminal::SseDialect;

pub use crate::boundary::InboundRequest;

/// Return the request target including its query, defaulting to `/` for an
/// origin-form URI without a path-and-query component.
pub fn path_and_query(uri: &Uri) -> String {
    uri.path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| "/".to_string())
}

/// Convert an HTTP header map to the legacy ordered pair representation used
/// by the protocol adapters.  Values are lossy-decoded exactly as the old
/// server facade did; no body bytes are touched here.
pub fn header_pairs(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                String::from_utf8_lossy(value.as_bytes()).to_string(),
            )
        })
        .collect()
}

/// 请求体上限：server body-limit 与 Engine 实际读取共用，防止异常请求无限占用内存。
pub const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
impl Engine {
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
        let path_and_query = self::path_and_query(&uri);
        let headers = self::header_pairs(&headers);
        self.handle_request_parts(remote_ip, method.as_str(), &path_and_query, headers, body)
            .await
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

    pub(super) fn inbound_request_context(
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
        // CPA's Codex Live handler parses a multipart `session` part before
        // selecting an OAuth account and normalizes its model there.  A
        // Codex Desktop bootstrap can carry a leaked surrounding chat model
        // (for example `claude-fable-5`) in that JSON part.  The route model
        // above is already forced to `gpt-live-1-codex`, but forwarding the
        // stale multipart field would make CPA dispatch using the wrong
        // logical model.  Normalize only the Live session/model field; keep
        // SDP, MIME boundaries, and every unrelated part byte-for-byte.
        if kind == PassthroughKind::Realtime
            && fields.model == request_build::DEFAULT_CODEX_LIVE_MODEL
            && content_type
                .as_deref()
                .is_some_and(content_type_is_multipart)
        {
            body = rewrite_realtime_multipart_model(
                &body,
                content_type.as_deref().unwrap_or_default(),
                &fields.model,
            );
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
}
