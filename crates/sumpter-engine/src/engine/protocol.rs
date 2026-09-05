//! Protocol implementation for the shared engine.

use axum::http::StatusCode;
use serde_json::Value;

use sumpter_core::bridge;
use sumpter_core::bridge_in;
use sumpter_core::config::{AppConfig, ProviderProtocol};
use sumpter_core::events::ClientKind;
use sumpter_core::routing::{
    PlannedEndpoint, RequestPurpose, RouteMode, RoutePlanner, RoutingRequest,
};

use super::context::current_request_context;
use super::payload::metadata_json_body_hint;
use crate::request_build::{self, PassthroughKind};

/// Legacy internal callers can request a native protocol. Raw data-plane
/// requests do not use this filter and are always relayed to the selected
/// Provider, regardless of its protocol label.
pub(super) fn source_format_for_passthrough(kind: PassthroughKind) -> ProviderProtocol {
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

pub(super) fn required_native_protocol(kind: PassthroughKind) -> Option<ProviderProtocol> {
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

pub(super) fn translation_supported(
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

/// Stable high-level route intent used in runtime diagnostics.  This is kept
/// deliberately independent from `RequestPurpose`: a normal Realtime call
/// and a Codex Live call have the same business purpose but different provider
/// capability requirements.
pub(super) fn route_intent_for_path(
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

pub(super) fn percent_decode_query_component(value: &str) -> String {
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

pub(super) fn from_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(super) fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key || percent_decode_query_component(name) == key).then_some(value)
    })
}

pub(super) fn decoded_query_value(query: &str, key: &str) -> Option<String> {
    query_value(query, key).map(percent_decode_query_component)
}

pub(super) fn path_without_query(path_and_query: &str) -> &str {
    path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path)
}

pub(super) fn json_timestamp(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| {
            value
                .as_str()
                .and_then(|raw| raw.trim().parse::<f64>().ok())
        })
        .filter(|timestamp| timestamp.is_finite())
}

pub(super) fn is_realtime_http_path(path: &str) -> bool {
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

pub(super) fn is_responses_websocket_path(path: &str) -> bool {
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

pub(super) fn is_path_or_child(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub(super) fn is_resource_passthrough_kind(kind: PassthroughKind) -> bool {
    matches!(kind, PassthroughKind::Files | PassthroughKind::Models)
}

pub(super) fn has_exact_codex_live_mapping(config: &AppConfig, endpoint_id: &str) -> bool {
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

pub(super) fn has_exact_realtime_mapping(
    config: &AppConfig,
    endpoint_id: &str,
    model: &str,
) -> bool {
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
pub(super) fn native_passthrough_kind(path: &str) -> PassthroughKind {
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

pub(super) fn is_openai_resource_tree(path: &str, resource: &str) -> bool {
    [
        format!("/v1/{resource}"),
        format!("/{resource}"),
        format!("/openai/v1/{resource}"),
        format!("/backend-api/codex/{resource}"),
    ]
    .iter()
    .any(|prefix| is_path_or_child(path, prefix))
}

pub(super) fn is_videos_create_path(path: &str) -> bool {
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

pub(super) fn is_videos_create_request(method: &str, path_and_query: &str) -> bool {
    method.eq_ignore_ascii_case("POST") && is_videos_create_path(path_without_query(path_and_query))
}

pub(super) fn is_live_bootstrap_path(path: &str) -> bool {
    is_codex_live_path(path) || is_realtime_call_bootstrap_path(path)
}

/// Identify an HTTP response that creates a Live call. The `/v1/realtime`
/// root is shared with public Realtime, so it is considered a bootstrap only
/// when the request was classified as Live and carries no existing call id.
/// Keeping this method-aware prevents a standard GET/POST Realtime resource
/// response from being persisted as a new Live binding.
pub(super) fn is_live_bootstrap_request(method: &str, path_and_query: &str) -> bool {
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

pub(super) fn video_id_from_path(path: &str) -> Option<&str> {
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

pub(super) fn is_videos_lookup_path(path: &str) -> bool {
    video_id_from_path(path).is_some()
}

pub(super) fn video_id_from_headers(headers: &[(String, String)]) -> Option<String> {
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

pub(super) fn valid_video_resource_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| value.to_string())
}

pub(super) fn video_id_from_json(body: &[u8]) -> Option<String> {
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
pub(super) fn resource_id_from_payload<F>(payload: &[u8], mut extract: F) -> Option<String>
where
    F: FnMut(&Value) -> Option<String>,
{
    pub(super) const MAX_CANDIDATE_OFFSETS: usize = 2048;
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

pub(super) fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while let Some(byte) = bytes.first() {
        if !byte.is_ascii_whitespace() {
            break;
        }
        bytes = &bytes[1..];
    }
    bytes
}

pub(super) fn video_id_from_payload(body: &[u8]) -> Option<String> {
    resource_id_from_payload(body, |value| {
        let encoded = serde_json::to_vec(value).ok()?;
        video_id_from_json(&encoded)
    })
}

pub(super) fn live_call_id_from_json(body: &[u8]) -> Option<String> {
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

pub(super) fn live_call_id_from_payload(body: &[u8]) -> Option<String> {
    resource_id_from_payload(body, |value| {
        let encoded = serde_json::to_vec(value).ok()?;
        live_call_id_from_json(&encoded)
    })
}

pub(super) fn valid_live_resource_id(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')))
    .then(|| value.to_string())
}

pub(super) fn is_codex_live_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/live" | "/live" | "/openai/v1/live" | "/backend-api/codex/live"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RealtimeRouteIntent {
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
pub(super) fn classify_realtime_intent(
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
    // Codex Desktop uses the compatibility `/backend-api/codex/realtime`
    // surface for its voice WebSocket.  CPA handles that surface as Codex
    // Live, even when the downstream request is a GET handshake.  Treating
    // it as public Standard Realtime sends `gpt-live-1-codex` to the wrong
    // upstream protocol and results in a 401 during the handshake.
    if path == "/backend-api/codex/realtime" {
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

pub(super) fn realtime_route_model(
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
pub(super) fn realtime_client_secret_models_match(expected: &str, requested: &str) -> bool {
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

pub(super) fn is_standard_realtime_model_name(model: &str) -> bool {
    sumpter_core::capability::is_realtime_model_name(model)
}

/// Resolve a logical Realtime/Live model against the configured capability
/// catalog.  CPA accepts a standard client-facing model (for example
/// `gpt-4o`) but may send it through the Codex Live OAuth surface.  If that
/// logical model has no `live` mapping in this installation, use the explicit
/// `gpt-live-1-codex` mapping instead of allowing the request to fall into a
/// text-only wildcard or an arbitrary first endpoint.
pub(super) fn resolve_realtime_route_model(
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
pub(super) fn realtime_voice_route_model(
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

pub(super) fn body_indicates_codex_live(body: &[u8], content_type: Option<&str>) -> bool {
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

pub(super) fn json_object_value<'a>(
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

pub(super) fn is_codex_live_family_path(path: &str) -> bool {
    [
        "/v1/live",
        "/live",
        "/openai/v1/live",
        "/backend-api/codex/live",
    ]
    .iter()
    .any(|prefix| is_path_or_child(path, prefix))
}

pub(super) fn is_realtime_root_path(path: &str) -> bool {
    matches!(
        path,
        "/v1/realtime" | "/realtime" | "/openai/v1/realtime" | "/backend-api/codex/realtime"
    )
}

pub(super) fn is_realtime_bootstrap_root(path: &str) -> bool {
    is_realtime_root_path(path) || is_realtime_call_bootstrap_path(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RealtimeCallPathError {
    InvalidId,
    UnsupportedAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RealtimeCallTarget {
    pub(super) call_id: String,
    pub(super) action: Option<String>,
}

/// Parse and validate CPA's `/realtime/calls/:call_id[/action]` surface and
/// root `?call_id=` sideband form. `Ok(None)` means the target is not a call
/// resource. Percent decoding is deliberately strict: malformed escapes,
/// decoded separators and control bytes are rejected rather than falling
/// through to an unrelated Realtime route.
pub(super) fn validate_realtime_call_target(
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

pub(super) fn realtime_call_path_error(
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

pub(super) fn query_has_key(query: &str, key: &str) -> bool {
    query.split('&').any(|pair| {
        let (raw_name, _) = pair.split_once('=').unwrap_or((pair, ""));
        strict_percent_decode(raw_name, true).as_deref() == Some(key)
    })
}

pub(super) fn strict_decoded_query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (raw_name, raw_value) = pair.split_once('=').unwrap_or((pair, ""));
        (strict_percent_decode(raw_name, true).as_deref() == Some(key))
            .then(|| strict_percent_decode(raw_value, true))
            .flatten()
    })
}

pub(super) fn percent_decode_path_segment(value: &str) -> Option<String> {
    let decoded = strict_percent_decode(value, false)?;
    (!decoded.contains('/') && !decoded.contains('\\')).then_some(decoded)
}

pub(super) fn strict_percent_decode(value: &str, plus_as_space: bool) -> Option<String> {
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

pub(super) fn is_realtime_call_path_prefix(path: &str) -> bool {
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
pub(super) fn unsupported_realtime_call_action(path: &str) -> bool {
    validate_realtime_call_target(path) == Err(RealtimeCallPathError::UnsupportedAction)
}

/// Realtime credential/session and SIP/control surfaces.  They share the
/// `live` capability in the mapping catalog, but are not Codex Live
/// bootstraps and must never be wrapped as Quicksilver.
pub(super) fn is_realtime_control_path(path: &str) -> bool {
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

pub(super) fn is_realtime_call_bootstrap_path(path: &str) -> bool {
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
pub(super) fn live_call_id_from_target(path_and_query: &str) -> Option<String> {
    validate_realtime_call_target(path_and_query)
        .ok()
        .flatten()
        .map(|target| target.call_id)
}

pub(super) fn is_codex_live_sideband_target(path_and_query: &str) -> bool {
    let path = path_without_query(path_and_query);
    (is_codex_live_family_path(path) && live_call_id_from_target(path_and_query).is_some())
        || (is_realtime_root_path(path) && live_call_id_from_target(path_and_query).is_some())
}

pub(super) fn live_call_id_from_headers(headers: &[(String, String)]) -> Option<String> {
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

pub(super) fn is_realtime_client_secret_path(path_and_query: &str) -> bool {
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
