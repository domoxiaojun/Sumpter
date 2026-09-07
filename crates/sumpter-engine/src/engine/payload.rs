//! Payload implementation for the shared engine.

use bytes::Bytes;
use serde_json::{Value, json};

use super::context::header_value;
use super::protocol::{json_object_value, path_without_query};
use crate::request_build::{self, PassthroughKind};

#[derive(Debug, PartialEq, Eq)]
pub(super) struct NativePassthroughFields {
    pub(super) model: String,
    pub(super) stream: bool,
}

pub(super) fn realtime_body_model(body: &[u8], content_type: Option<&str>) -> Option<String> {
    let content_type = content_type?;
    if content_type_is_multipart(content_type) {
        return realtime_multipart_model(body, content_type);
    }
    if !content_type_is_json(content_type) {
        return None;
    }
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let object = value.as_object()?;
    [
        json_object_value(object, "model"),
        json_object_value(object, "session")
            .and_then(Value::as_object)
            .and_then(|session| json_object_value(session, "model")),
    ]
    .into_iter()
    .flatten()
    .filter_map(Value::as_str)
    .map(str::trim)
    .find(|model| !model.is_empty())
    .map(str::to_string)
}

pub(super) fn realtime_body_model_hint(body: &[u8], content_type: Option<&str>) -> Option<String> {
    if content_type.is_some_and(content_type_is_json) {
        return realtime_body_model(body, content_type);
    }
    if content_type.is_some_and(content_type_is_multipart) {
        return realtime_multipart_model(body, content_type.unwrap_or_default());
    }
    None
}

/// Codex Live multipart requests carry the model in the JSON `session` part
/// (CPA's `modelFromJSON` gives that value precedence over a top-level model
/// part). Read it for routing and normalization without rebuilding any MIME
/// framing or binary content.
pub(super) fn realtime_multipart_model(body: &[u8], content_type: &str) -> Option<String> {
    let parts = multipart_parts(body, content_type).ok()?;
    let mut top_level = None;
    for (headers, field) in parts {
        let Some(name) = multipart_field_name(headers) else {
            continue;
        };
        match name.as_str() {
            "session" => {
                let value = serde_json::from_slice::<Value>(field).ok()?;
                if let Some(model) = value
                    .as_object()
                    .and_then(|object| object.get("model"))
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                {
                    return Some(model.to_string());
                }
            }
            "model" if top_level.is_none() => {
                top_level = std::str::from_utf8(field)
                    .ok()
                    .map(str::trim)
                    .filter(|model| !model.is_empty())
                    .map(str::to_string);
            }
            _ => {}
        }
    }
    top_level
}

/// Apply the session configuration bound to an ephemeral Realtime key to a
/// subsequent `/v1/realtime/calls` request.  This is the HTTP counterpart of
/// CPA's `session.update` WebSocket frame and preserves SDP callers as well as
/// JSON callers.
pub(super) fn apply_realtime_client_secret_session(
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

pub(super) fn realtime_client_secret_request_session(
    path_and_query: &str,
    body: &[u8],
) -> Option<Value> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    let path = path_without_query(path_and_query);
    let session = if matches!(
        path,
        "/v1/realtime/client_secrets"
            | "/realtime/client_secrets"
            | "/openai/v1/realtime/client_secrets"
            | "/backend-api/codex/realtime/client_secrets"
    ) {
        value.get("session")?.as_object()?.clone()
    } else if matches!(
        path,
        "/v1/realtime/sessions"
            | "/realtime/sessions"
            | "/openai/v1/realtime/sessions"
            | "/backend-api/codex/realtime/sessions"
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

pub(super) fn content_type_is_json(content_type: &str) -> bool {
    content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with("application/json")
}

pub(super) fn content_type_is_multipart(content_type: &str) -> bool {
    content_type
        .trim()
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
}

/// Whether an unknown Raw request is plausibly carrying a body.  We cannot
/// peek an `axum::Body` without consuming it, so use the HTTP framing headers
/// (and the body-capable method) as a bounded dispatch hint.  An explicit
/// `Content-Length: 0` is respected; a non-empty content type or chunked
/// transfer still gets to the native handler, where the model requirement is
/// enforced.  This keeps body-only JSON model requests working even when a
/// client forgot `Content-Type`, while GET/HEAD probes without a body remain a
/// cheap 404.
pub(super) fn raw_request_body_hint(method: &str, headers: &[(String, String)]) -> bool {
    if let Some(length) = header_value(headers, "content-length") {
        if let Ok(length) = length.trim().parse::<u64>() {
            if length > 0 {
                return true;
            }
            // An explicit zero Content-Length is authoritative for normal
            // requests. Transfer-Encoding is the one exception: chunked
            // framing may still carry a body and should reach the handler.
            return header_value(headers, "transfer-encoding")
                .is_some_and(|value| !value.trim().is_empty());
        }
        // Invalid framing must not make us discard a request that may contain
        // a model; the body limit and parser provide the final validation.
        return true;
    }
    if header_value(headers, "transfer-encoding").is_some_and(|value| !value.trim().is_empty())
        || header_value(headers, "content-type").is_some_and(|value| !value.trim().is_empty())
    {
        return true;
    }
    // Without framing headers there is no safe way to know whether an
    // unknown-path request actually carries a body without polling/consuming
    // it. Keep the ingress preflight cheap (and preserve a real 404 for
    // bodyless probes); clients that send a body will normally provide either
    // Content-Length or Transfer-Encoding, both handled above. The method is
    // intentionally not used as a guess here.
    let _ = method;
    false
}

/// Read only the top-level JSON `model` field used to select a Raw mapping.
/// The payload itself is never serialized back or changed.  Sniffing is
/// limited to a small prefix and only attempted for JSON-looking bytes (or an
/// explicit JSON content type), avoiding a full parse of opaque/binary uploads.
pub(super) const RAW_MODEL_SNIFF_BYTES: usize = 1024 * 1024;

/// Bounded JSON sniff used only for client attribution and route hints.  An
/// explicit non-JSON media type wins over a JSON-looking prefix; without a
/// media type we require the first non-whitespace byte to be `{`.
pub(super) fn metadata_json_body_hint(body: &[u8], content_type: Option<&str>) -> Option<Value> {
    pub(super) const MAX_BYTES: usize = sumpter_core::events::CODEX_METADATA_MAX_JSON_BYTES;
    if body.is_empty() || body.len() > MAX_BYTES {
        return None;
    }
    if let Some(content_type) = content_type {
        if !content_type_is_json(content_type) {
            return None;
        }
    } else if body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return None;
    }
    serde_json::from_slice::<Value>(body).ok()
}

pub(super) fn raw_body_model_hint(body: &[u8], content_type: Option<&str>) -> Option<String> {
    if body.is_empty() || body.len() > RAW_MODEL_SNIFF_BYTES {
        return None;
    }
    let first = body
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    if !content_type.is_some_and(content_type_is_json) && first != Some(b'{') {
        return None;
    }
    serde_json::from_slice::<Value>(body)
        .ok()?
        .as_object()?
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
}

pub(super) fn native_json_fields(
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
        PassthroughKind::GeminiGenerate => "",
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

pub(super) fn native_multipart_fields(
    body: &[u8],
    content_type: &str,
) -> Result<NativePassthroughFields, &'static str> {
    native_multipart_fields_with_default(body, content_type, "gpt-image-2")
}

pub(super) fn native_multipart_fields_with_default(
    body: &[u8],
    content_type: &str,
    default_model: &str,
) -> Result<NativePassthroughFields, &'static str> {
    let mut model = None;
    let mut stream = false;
    for (headers, field) in multipart_parts(body, content_type)? {
        if let Some(name) = multipart_field_name(headers)
            && matches!(name.as_str(), "model" | "stream")
        {
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
    }
    Ok(NativePassthroughFields {
        model: model.unwrap_or_else(|| default_model.to_string()),
        stream,
    })
}

#[derive(Clone, Copy)]
pub(super) struct MultipartDelimiter {
    pub(super) start: usize,
    pub(super) content_start: usize,
    pub(super) closing: bool,
}

pub(super) type MultipartPart<'a> = (&'a [u8], &'a [u8]);

pub(super) struct MultipartPartRange {
    pub(super) headers: std::ops::Range<usize>,
    pub(super) content: std::ops::Range<usize>,
}

/// Parse multipart framing without ever rebuilding the body. A boundary-like
/// byte sequence inside uploaded binary data is accepted as a delimiter only
/// when it begins a MIME line and is followed by a valid delimiter suffix.
/// The final closing delimiter is mandatory so a truncated upload cannot
/// silently yield a misleading routing model.
pub(super) fn multipart_parts<'a>(
    body: &'a [u8],
    content_type: &str,
) -> Result<Vec<MultipartPart<'a>>, &'static str> {
    multipart_part_ranges(body, content_type).map(|parts| {
        parts
            .into_iter()
            .map(|part| (&body[part.headers], &body[part.content]))
            .collect()
    })
}

pub(super) fn multipart_part_ranges(
    body: &[u8],
    content_type: &str,
) -> Result<Vec<MultipartPartRange>, &'static str> {
    let boundary = multipart_boundary(content_type).ok_or("multipart boundary is required")?;
    let marker = format!("--{boundary}").into_bytes();
    let mut delimiter = next_multipart_delimiter(body, &marker, 0)
        .ok_or("multipart opening boundary is missing")?;
    let mut parts = Vec::new();
    loop {
        if delimiter.closing {
            return Ok(parts);
        }
        let part_start = delimiter.content_start;
        let Some((header_offset, separator_len)) = multipart_header_end(&body[part_start..]) else {
            return Err("malformed multipart headers");
        };
        if header_offset > 16 * 1024 {
            return Err("multipart headers are too large");
        }
        let headers = part_start..part_start + header_offset;
        let content_start = part_start + header_offset + separator_len;
        let next = next_multipart_delimiter(body, &marker, content_start)
            .ok_or("multipart closing boundary is missing")?;
        let content_end =
            if next.start >= 2 && body.get(next.start - 2..next.start) == Some(b"\r\n") {
                next.start - 2
            } else if next.start >= 1 && body.get(next.start - 1) == Some(&b'\n') {
                next.start - 1
            } else {
                next.start
            };
        if content_end < content_start {
            return Err("malformed multipart body");
        }
        parts.push(MultipartPartRange {
            headers,
            content: content_start..content_end,
        });
        delimiter = next;
    }
}

/// Normalize a Codex Live multipart model without reconstructing the request.
/// CPA performs this normalization in its Live handler before selecting an
/// OAuth account.  Sumpter must do the same when the client leaked a chat
/// model into the `session` part, otherwise CPA can select using that stale
/// model even though the route was classified as Live.
pub(super) fn rewrite_realtime_multipart_model(
    body: &[u8],
    content_type: &str,
    target: &str,
) -> Bytes {
    if target.trim().is_empty() {
        return Bytes::copy_from_slice(body);
    }
    let Ok(parts) = multipart_part_ranges(body, content_type) else {
        // Let the upstream parser report malformed multipart framing.  This
        // helper is observational unless a valid JSON/model field is found.
        return Bytes::copy_from_slice(body);
    };

    let mut replacements: Vec<(std::ops::Range<usize>, Vec<u8>)> = Vec::new();
    for part in parts {
        let name = multipart_field_name(&body[part.headers.clone()]);
        match name.as_deref() {
            Some("model") => {
                let value = &body[part.content.clone()];
                let trimmed_start = value
                    .iter()
                    .position(|byte| !byte.is_ascii_whitespace())
                    .unwrap_or(value.len());
                let trimmed_end = value
                    .iter()
                    .rposition(|byte| !byte.is_ascii_whitespace())
                    .map_or(trimmed_start, |index| index + 1);
                if trimmed_start < trimmed_end {
                    let mut replacement = Vec::with_capacity(value.len());
                    replacement.extend_from_slice(&value[..trimmed_start]);
                    replacement.extend_from_slice(target.as_bytes());
                    replacement.extend_from_slice(&value[trimmed_end..]);
                    replacements.push((part.content, replacement));
                }
            }
            Some("session") => {
                let value = &body[part.content.clone()];
                let Ok(mut session) = serde_json::from_slice::<Value>(value) else {
                    continue;
                };
                let Some(object) = session.as_object_mut() else {
                    continue;
                };
                let Some(model) = object.get("model").and_then(Value::as_str) else {
                    continue;
                };
                if model == target {
                    continue;
                }
                object.insert("model".into(), json!(target));
                let Ok(replacement) = serde_json::to_vec(&session) else {
                    continue;
                };
                replacements.push((part.content, replacement));
            }
            _ => {}
        }
    }
    if replacements.is_empty() {
        return Bytes::copy_from_slice(body);
    }
    replacements.sort_by_key(|(range, _)| range.start);
    let mut output = Vec::with_capacity(
        body.len()
            + replacements
                .iter()
                .map(|(range, replacement)| replacement.len().saturating_sub(range.len()))
                .sum::<usize>(),
    );
    let mut cursor = 0;
    for (range, replacement) in replacements {
        if range.start < cursor || range.end > body.len() {
            return Bytes::copy_from_slice(body);
        }
        output.extend_from_slice(&body[cursor..range.start]);
        output.extend_from_slice(&replacement);
        cursor = range.end;
    }
    output.extend_from_slice(&body[cursor..]);
    Bytes::from(output)
}

pub(super) fn next_multipart_delimiter(
    body: &[u8],
    marker: &[u8],
    from: usize,
) -> Option<MultipartDelimiter> {
    if marker.is_empty() || from > body.len() {
        return None;
    }
    let mut cursor = from;
    while cursor <= body.len().saturating_sub(marker.len()) {
        let relative = find_bytes(&body[cursor..], marker)?;
        let start = cursor + relative;
        let at_line_start = start == 0
            || (start >= 2 && body.get(start - 2..start) == Some(b"\r\n"))
            || (start >= 1 && body.get(start - 1) == Some(&b'\n'));
        if !at_line_start {
            cursor = start.saturating_add(1);
            continue;
        }
        let suffix = start + marker.len();
        if body.get(suffix..suffix + 2) == Some(b"--") {
            let after = suffix + 2;
            if after == body.len()
                || body.get(after..after + 2) == Some(b"\r\n")
                || body.get(after) == Some(&b'\n')
            {
                return Some(MultipartDelimiter {
                    start,
                    content_start: after,
                    closing: true,
                });
            }
        } else if body.get(suffix..suffix + 2) == Some(b"\r\n") {
            return Some(MultipartDelimiter {
                start,
                content_start: suffix + 2,
                closing: false,
            });
        } else if body.get(suffix) == Some(&b'\n') {
            return Some(MultipartDelimiter {
                start,
                content_start: suffix + 1,
                closing: false,
            });
        }
        cursor = start.saturating_add(1);
    }
    None
}

pub(super) fn multipart_boundary(content_type: &str) -> Option<String> {
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

pub(super) fn multipart_header_end(bytes: &[u8]) -> Option<(usize, usize)> {
    let crlf = find_bytes(bytes, b"\r\n\r\n").map(|offset| (offset, 4));
    let lf = find_bytes(bytes, b"\n\n").map(|offset| (offset, 2));
    match (crlf, lf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

pub(super) fn multipart_field_name(headers: &[u8]) -> Option<String> {
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
                .to_ascii_lowercase()
        })
    })
}

pub(super) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty() && haystack.len() >= needle.len())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

pub(super) fn valid_anthropic_messages_request(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|model| !model.trim().is_empty())
            && object.get("messages").is_some_and(Value::is_array)
    })
}
