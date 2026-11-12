//! Export-only redaction. Original captures never acquire projection fields.
use serde_json::{Value, json};
use std::io::{self, Write};
use sumpter_core::events::{DiagnosticChunk, DiagnosticHeader, DiagnosticRequestCapture};
use tokio_util::sync::CancellationToken;

pub const DEFAULT_MAX_PARSE_MIB: u16 = 64;

fn check_cancel(cancel: &CancellationToken) -> io::Result<()> {
    if cancel.is_cancelled() {
        Err(io::Error::other("export cancelled"))
    } else {
        Ok(())
    }
}

fn field<W: Write, T: serde::Serialize + ?Sized>(
    writer: &mut W,
    first: &mut bool,
    name: &str,
    value: &T,
) -> io::Result<()> {
    if !*first {
        writer.write_all(b",")?;
    }
    *first = false;
    serde_json::to_writer(&mut *writer, name)?;
    writer.write_all(b":")?;
    serde_json::to_writer(&mut *writer, value)?;
    Ok(())
}

fn header(headers: &[DiagnosticHeader], name: &str) -> Option<String> {
    let values: Vec<_> = headers
        .iter()
        .filter(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
        .collect();
    (!values.is_empty()).then(|| values.join(","))
}
fn compressed(headers: &[DiagnosticHeader]) -> bool {
    header(headers, "content-encoding").is_some_and(|value| {
        value
            .split(',')
            .any(|part| !part.trim().eq_ignore_ascii_case("identity"))
    })
}

fn redact_json_checked(value: &mut Value, cancel: &CancellationToken) -> io::Result<()> {
    check_cancel(cancel)?;
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if sensitive_capture_key(key) {
                    *value = Value::String("[REDACTED]".into());
                } else {
                    redact_json_checked(value, cancel)?;
                }
            }
        }
        Value::Array(array) => {
            for value in array {
                redact_json_checked(value, cancel)?;
            }
        }
        Value::String(text) => *text = redact_text_inner(text, Some(cancel))?,
        _ => {}
    }
    Ok(())
}

struct CancelWriter<'a> {
    bytes: &'a mut Vec<u8>,
    cancel: &'a CancellationToken,
}
impl Write for CancelWriter<'_> {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        check_cancel(self.cancel)?;
        let n = input.len().min(64 * 1024);
        self.bytes.try_reserve(n).map_err(io::Error::other)?;
        self.bytes.extend_from_slice(&input[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        check_cancel(self.cancel)
    }
}

struct CancelReader<'a> {
    remaining: &'a [u8],
    cancel: &'a CancellationToken,
}
impl io::Read for CancelReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        check_cancel(self.cancel)?;
        let n = output.len().min(self.remaining.len());
        output[..n].copy_from_slice(&self.remaining[..n]);
        self.remaining = &self.remaining[n..];
        Ok(n)
    }
}
fn parse_json(body: &str, cancel: &CancellationToken) -> Result<Value, serde_json::Error> {
    let reader = io::BufReader::with_capacity(
        64 * 1024,
        CancelReader {
            remaining: body.as_bytes(),
            cancel,
        },
    );
    serde_json::from_reader(reader)
}

fn body_text(
    body: &str,
    truncated: bool,
    encoded: bool,
    max: usize,
    cancel: &CancellationToken,
) -> io::Result<(String, Value)> {
    check_cancel(cancel)?;
    let reason = if truncated {
        Some("truncated")
    } else if encoded {
        Some("compressed_capture_unrecoverable")
    } else if body.len() > max {
        Some("parse_limit_exceeded")
    } else if body.contains('\u{fffd}') {
        Some("invalid_encoding")
    } else {
        None
    };
    if let Some(reason) = reason {
        return Ok((
            "[OMITTED_BODY]".into(),
            json!({"status":"omitted","omissionReason":reason}),
        ));
    }
    if body.is_empty() {
        return Ok((String::new(), json!({"status":"complete"})));
    }
    let Ok(mut value) = parse_json(body, cancel) else {
        check_cancel(cancel)?;
        return Ok((
            "[OMITTED_BODY]".into(),
            json!({"status":"omitted","omissionReason":"invalid_json"}),
        ));
    };
    redact_json_checked(&mut value, cancel)?;
    let mut encoded = Vec::new();
    serde_json::to_writer(
        CancelWriter {
            bytes: &mut encoded,
            cancel,
        },
        &value,
    )?;
    let text = String::from_utf8(encoded).map_err(io::Error::other)?;
    Ok((text, json!({"status":"complete"})))
}

fn write_chunk_metadata<W: Write>(
    writer: &mut W,
    chunks: &[DiagnosticChunk],
    cancel: &CancellationToken,
) -> io::Result<()> {
    writer.write_all(b"[")?;
    for (index, chunk) in chunks.iter().enumerate() {
        check_cancel(cancel)?;
        if index > 0 {
            writer.write_all(b",")?;
        }
        // Deliberately no data field, including on malformed/partial captures.
        serde_json::to_writer(
            &mut *writer,
            &json!({"atMS":chunk.at_ms,"bytes":chunk.bytes,"truncated":chunk.truncated}),
        )?;
    }
    writer.write_all(b"]")
}

fn write_projection<W: Write>(
    writer: &mut W,
    chunks: &[DiagnosticChunk],
    headers: &[DiagnosticHeader],
    truncated: bool,
    max: usize,
    cancel: &CancellationToken,
) -> io::Result<()> {
    check_cancel(cancel)?;
    if compressed(headers) {
        return Ok(serde_json::to_writer(
            writer,
            &json!({"format":"unknown","status":"omitted","omissionReason":"compressed_capture_unrecoverable"}),
        )?);
    }
    let content_type = header(headers, "content-type");
    let prefix: String = chunks
        .iter()
        .flat_map(|chunk| chunk.data.chars())
        .take(128)
        .collect();
    let sse = content_type
        .as_ref()
        .is_some_and(|value| value.to_ascii_lowercase().contains("text/event-stream"))
        || (content_type.is_none()
            && ["data:", "event:", ":", "id:", "retry:"]
                .iter()
                .any(|marker| prefix.trim_start_matches('\u{feff}').starts_with(marker)));
    if sse {
        return write_sse(writer, chunks, truncated, max, cancel);
    }
    let size = chunks
        .iter()
        .try_fold(0_usize, |size, chunk| size.checked_add(chunk.data.len()));
    let invalid_size = size.is_none_or(|size| size > max);
    let partial = truncated
        || chunks
            .iter()
            .any(|chunk| chunk.truncated || chunk.bytes != chunk.data.len() as u64);
    if invalid_size || partial {
        return Ok(serde_json::to_writer(
            writer,
            &json!({"format":"json","status":"omitted","omissionReason":if partial {"truncated_or_invalid_encoding"} else {"parse_limit_exceeded"}}),
        )?);
    }
    let mut body = String::new();
    if body.try_reserve_exact(size.unwrap_or(0)).is_err() {
        return Ok(serde_json::to_writer(
            writer,
            &json!({"format":"json","status":"omitted","omissionReason":"allocation_failed"}),
        )?);
    }
    for chunk in chunks {
        check_cancel(cancel)?;
        body.push_str(&chunk.data);
    }
    let (text, mut state) = body_text(&body, false, false, max, cancel)?;
    state["format"] = json!("json");
    if state["status"] == "complete" {
        state["body"] = json!(text);
    }
    serde_json::to_writer(writer, &state)?;
    Ok(())
}

fn sse_event(raw: &[u8], oversized: bool, cancel: &CancellationToken) -> io::Result<Value> {
    check_cancel(cancel)?;
    if oversized {
        return Ok(json!({"status":"omitted","omissionReason":"event_parse_limit_exceeded"}));
    }
    let Ok(text) = std::str::from_utf8(raw) else {
        return Ok(json!({"status":"omitted","omissionReason":"invalid_encoding"}));
    };
    if text.contains('\u{fffd}') {
        return Ok(json!({"status":"omitted","omissionReason":"invalid_encoding"}));
    }
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut data = String::new();
    let mut kind = None;
    let mut id = None;
    let mut has_data = false;
    for line in text.lines() {
        if line.starts_with(':') {
            continue;
        }
        let (name, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match name {
            "data" => {
                if has_data {
                    data.push('\n');
                }
                data.push_str(value);
                has_data = true;
            }
            "event" => kind = Some(redact_text_inner(value, Some(cancel))?),
            "id" => id = Some(redact_text_inner(value, Some(cancel))?),
            _ => {}
        }
    }
    let mut value = if !has_data {
        Value::Null
    } else if data.trim() == "[DONE]" {
        json!("[DONE]")
    } else {
        match parse_json(&data, cancel) {
            Ok(value) => value,
            Err(_) => {
                check_cancel(cancel)?;
                return Ok(json!({"status":"omitted","omissionReason":"invalid_event_json"}));
            }
        }
    };
    redact_json_checked(&mut value, cancel)?;
    Ok(json!({"status":"complete","event":kind,"id":id,"data":value}))
}

fn write_sse<W: Write>(
    writer: &mut W,
    chunks: &[DiagnosticChunk],
    truncated: bool,
    max: usize,
    cancel: &CancellationToken,
) -> io::Result<()> {
    writer.write_all(b"{\"format\":\"sse\",\"events\":[")?;
    let mut event = Vec::new();
    let mut event_bytes = 0_usize;
    let mut line_bytes = 0_usize;
    let mut previous_cr = false;
    let mut first = true;
    let mut completed = 0_u64;
    let mut omitted = 0_u64;
    let mut remaining_reason = None;
    'chunks: for chunk in chunks {
        for (offset, &byte) in chunk.data.as_bytes().iter().enumerate() {
            if offset % 8192 == 0 {
                check_cancel(cancel)?;
            }
            if byte == b'\n' && previous_cr {
                previous_cr = false;
                continue;
            }
            previous_cr = byte == b'\r';
            if byte == b'\r' || byte == b'\n' {
                if line_bytes == 0 {
                    if event_bytes > 0 {
                        let result = sse_event(&event, event_bytes > max, cancel)?;
                        if result["status"] == "complete" {
                            completed += 1;
                        } else {
                            omitted += 1;
                        }
                        if !first {
                            writer.write_all(b",")?;
                        }
                        first = false;
                        serde_json::to_writer(&mut *writer, &result)?;
                    }
                    event.clear();
                    event_bytes = 0;
                } else {
                    event_bytes = event_bytes.saturating_add(1);
                    if event_bytes <= max {
                        event.push(b'\n');
                    }
                }
                line_bytes = 0;
            } else {
                line_bytes = line_bytes.saturating_add(1);
                event_bytes = event_bytes.saturating_add(1);
                if event_bytes <= max {
                    event.push(byte);
                }
            }
        }
        if chunk.truncated || chunk.bytes != chunk.data.len() as u64 {
            // A gap may contain newlines; do not guess subsequent event boundaries.
            remaining_reason = Some("capture_gap_remaining_omitted");
            break 'chunks;
        }
    }
    if event_bytes > 0 || truncated {
        remaining_reason.get_or_insert("incomplete_event_or_capture");
    }
    if remaining_reason.is_some() {
        omitted += 1;
    }
    writer.write_all(b"],")?;
    let status = if omitted == 0 {
        "complete"
    } else if completed == 0 {
        "omitted"
    } else {
        "partial"
    };
    write!(
        writer,
        "\"status\":\"{status}\",\"completedEvents\":{completed},\"omittedEvents\":{omitted},\"omissionReason\":"
    )?;
    serde_json::to_writer(&mut *writer, &remaining_reason)?;
    writer.write_all(b"}")
}

/// Serialize metadata and one body/event at a time. No raw record is converted
/// to Value (or cloned into an owned string) before the parse ceiling is checked.
pub fn write_capture<W: Write>(
    writer: &mut W,
    record: &DiagnosticRequestCapture,
    max_parse_bytes: usize,
    cancel: &CancellationToken,
) -> io::Result<()> {
    let mut metadata = json!({
        "requestID": record.request_id, "timestamp":record.timestamp, "method":record.method, "path":record.path,
        "inboundHeaders":record.inbound_headers, "inboundBodyBytes":record.inbound_body_bytes, "inboundBodyTruncated":record.inbound_body_truncated,
        "clientKind":record.client_kind, "requestPurpose":record.request_purpose, "clientModel":record.client_model, "effectiveModel":record.effective_model,
        "featureRuleID":record.feature_rule_id, "clientDeclared":record.client_declared, "sourceFormat":record.source_format, "targetFormat":record.target_format,
        "routeMode":record.route_mode, "completedAtMS":record.completed_at_ms, "statusCode":record.status_code, "outcome":record.outcome,
        "failureKind":record.failure_kind, "failureDetail":record.failure_detail, "truncated":record.truncated, "websocketTrace":record.websocket_trace,
    });
    redact_capture_value(&mut metadata);
    writer.write_all(b"{")?;
    let mut first = true;
    for (name, value) in metadata.as_object().unwrap() {
        field(writer, &mut first, name, value)?;
    }
    let (body, status) = body_text(
        &record.inbound_body,
        record.inbound_body_truncated,
        compressed(&record.inbound_headers),
        max_parse_bytes,
        cancel,
    )?;
    field(writer, &mut first, "inboundBody", &body)?;
    field(writer, &mut first, "inboundBodyRedaction", &status)?;
    writer.write_all(b",\"attempts\":[")?;
    for (index, attempt) in record.attempts.iter().enumerate() {
        check_cancel(cancel)?;
        if index > 0 {
            writer.write_all(b",")?;
        }
        let mut metadata = json!({"id":attempt.id,"endpointID":attempt.endpoint_id,"endpointName":attempt.endpoint_name,"protocol":attempt.protocol,
            "sourceFormat":attempt.source_format,"targetFormat":attempt.target_format,"routeMode":attempt.route_mode,"startedAtMS":attempt.started_at_ms,
            "outboundMethod":attempt.outbound_method,"outboundURL":attempt.outbound_url,"outboundHeaders":attempt.outbound_headers,
            "outboundBodyBytes":attempt.outbound_body_bytes,"outboundBodyTruncated":attempt.outbound_body_truncated,"responseStatus":attempt.response_status,
            "responseHeaders":attempt.response_headers,"error":attempt.error,"completedAtMS":attempt.completed_at_ms});
        redact_capture_value(&mut metadata);
        writer.write_all(b"{")?;
        let mut first = true;
        for (name, value) in metadata.as_object().unwrap() {
            field(writer, &mut first, name, value)?;
        }
        let (body, status) = body_text(
            &attempt.outbound_body,
            attempt.outbound_body_truncated,
            compressed(&attempt.outbound_headers),
            max_parse_bytes,
            cancel,
        )?;
        field(writer, &mut first, "outboundBody", &body)?;
        field(writer, &mut first, "outboundBodyRedaction", &status)?;
        writer.write_all(b",\"upstreamChunks\":")?;
        write_chunk_metadata(writer, &attempt.upstream_chunks, cancel)?;
        writer.write_all(b",\"redactedResponseBody\":")?;
        write_projection(
            writer,
            &attempt.upstream_chunks,
            &attempt.response_headers,
            record.truncated,
            max_parse_bytes,
            cancel,
        )?;
        writer.write_all(b"}")?;
    }
    writer.write_all(b"],\"clientChunks\":")?;
    write_chunk_metadata(writer, &record.client_chunks, cancel)?;
    writer.write_all(b",\"redactedClientBody\":")?;
    // Client response headers were not persisted. Use only a native attempt's
    // headers; otherwise sniff the exported bytes. Ambiguous compressed data is
    // always omitted rather than interpreted as trustworthy text.
    let headers = record
        .attempts
        .last()
        .filter(|attempt| attempt.route_mode != Some(sumpter_core::routing::RouteMode::Translated))
        .map(|attempt| attempt.response_headers.as_slice())
        .unwrap_or(&[]);
    write_projection(
        writer,
        &record.client_chunks,
        headers,
        record.truncated,
        max_parse_bytes,
        cancel,
    )?;
    writer.write_all(b"}")
}

pub fn sensitive_capture_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_'], "");
    if key == "token" {
        return true;
    }
    [
        "authorization",
        "proxyauthorization",
        "cookie",
        "setcookie",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "clientsecret",
        "password",
        "passwd",
        "secret",
        "privatekey",
        "signature",
        "webhooksecret",
    ]
    .iter()
    .any(|needle| key == *needle || key.contains(needle))
}

pub fn sensitive_header_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    sensitive_capture_key(&name)
        || name.contains("token")
        || name.contains("auth")
        || name == "proxy-authenticate"
        || name == "www-authenticate"
}

pub fn redact_url_query(raw: &str) -> String {
    let Some(question) = raw.find('?') else {
        return raw.to_string();
    };
    let (prefix, query_and_fragment) = raw.split_at(question + 1);
    let (query, fragment) = query_and_fragment
        .find('#')
        .map(|offset| query_and_fragment.split_at(offset))
        .unwrap_or((query_and_fragment, ""));
    let redacted = query
        .split('&')
        .map(|pair| {
            let Some(equal) = pair.find('=') else {
                return pair.to_string();
            };
            let (key, value) = pair.split_at(equal);
            let decoded_key = reqwest::Url::parse(&format!("https://redaction.invalid/?{key}="))
                .ok()
                .and_then(|url| url.query_pairs().next().map(|(key, _)| key.into_owned()))
                .unwrap_or_else(|| key.to_string())
                .to_ascii_lowercase();
            if decoded_key == "key"
                || sensitive_capture_key(&decoded_key)
                || decoded_key.contains("token")
            {
                format!("{key}=[REDACTED]")
            } else {
                format!("{key}{value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{prefix}{redacted}{fragment}")
}

pub fn redact_text_secrets(text: &str) -> String {
    redact_text_inner(text, None).unwrap_or_else(|_| "[OMITTED_TEXT]".into())
}

fn redact_text_inner(text: &str, cancel: Option<&CancellationToken>) -> io::Result<String> {
    const MARKERS: &[&[u8]] = &[
        b"Bearer ",
        b"Basic ",
        b"token=",
        b"access_token=",
        b"api_key=",
        b"apiKey=",
    ];
    let bytes = text.as_bytes();
    let mut output = String::with_capacity(text.len());
    let mut cursor = 0;
    let mut copied = 0;
    while cursor < bytes.len() {
        if cursor % 8192 == 0
            && let Some(cancel) = cancel
        {
            check_cancel(cancel)?;
        }
        if let Some(marker) = MARKERS.iter().find(|marker| {
            bytes
                .get(cursor..cursor + marker.len())
                .is_some_and(|part| part.eq_ignore_ascii_case(marker))
        }) {
            let start = cursor + marker.len();
            let end = text[start..]
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '&' | ',' | '"' | '\''))
                .map_or(text.len(), |offset| start + offset);
            output.push_str(&text[copied..start]);
            output.push_str("[REDACTED]");
            cursor = end;
            copied = end;
        } else {
            cursor += 1;
        }
    }
    output.push_str(&text[copied..]);
    Ok(output)
}

pub fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if sensitive_capture_key(key) {
                    *child = Value::String("[REDACTED]".into());
                } else {
                    redact_json_value(child);
                }
            }
        }
        Value::Array(array) => array.iter_mut().for_each(redact_json_value),
        Value::String(text) => *text = redact_text_secrets(text),
        _ => {}
    }
}

pub fn redact_body_text(body: &str) -> String {
    if body.len() > 64 * 1024 * 1024 || body.contains('\u{fffd}') {
        return "[OMITTED_BODY]".into();
    }
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json_value(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| "[REDACTED_BODY]".into())
    } else {
        "[OMITTED_BODY]".into()
    }
}

pub fn redact_capture_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                redact_capture_value(item);
            }
        }
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                match key.as_str() {
                    "inboundHeaders" | "outboundHeaders" | "responseHeaders" => {
                        if let Some(headers) = child.as_array_mut() {
                            for header in headers {
                                if let Some(header) = header.as_object_mut() {
                                    let name =
                                        header.get("name").and_then(Value::as_str).unwrap_or("");
                                    if sensitive_header_name(name) {
                                        header.insert(
                                            "value".into(),
                                            Value::String("[REDACTED]".into()),
                                        );
                                    } else if let Some(header_value) = header.get_mut("value")
                                        && let Some(text) = header_value.as_str()
                                    {
                                        *header_value = Value::String(redact_text_secrets(
                                            &redact_url_query(text),
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    "data" => {
                        *child = Value::String("[OMITTED_CHUNK]".into());
                    }
                    "inboundBody" | "outboundBody" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_body_text(text));
                        }
                    }
                    "outboundURL" | "path" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_url_query(text));
                        }
                    }
                    "error" | "failureDetail" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_text_secrets(text));
                        }
                    }
                    _ => redact_capture_value(child),
                }
            }
        }
        Value::String(text) => *text = redact_text_secrets(text),
        _ => {}
    }
}

#[cfg(test)]
mod security_tests {
    use super::*;
    fn chunks(parts: &[&str]) -> Vec<DiagnosticChunk> {
        parts
            .iter()
            .map(|text| DiagnosticChunk {
                at_ms: 1,
                bytes: text.len() as u64,
                data: (*text).into(),
                truncated: false,
            })
            .collect()
    }
    fn project(chunks: &[DiagnosticChunk], sse: bool, max: usize) -> Value {
        let headers = vec![DiagnosticHeader {
            name: "content-type".into(),
            value: if sse {
                "text/event-stream"
            } else {
                "application/json"
            }
            .into(),
        }];
        let mut output = Vec::new();
        write_projection(
            &mut output,
            chunks,
            &headers,
            false,
            max,
            &CancellationToken::new(),
        )
        .unwrap();
        serde_json::from_slice(&output).unwrap()
    }
    #[test]
    fn json_reassembles_secrets_and_fails_closed_at_limits_or_invalid_encoding() {
        let source = chunks(&[
            r#"{"password":"synth"#,
            r#"etic","text":"bEaReR credential","input_tokens":3}"#,
        ]);
        let saved = source.clone();
        let length = source.iter().map(|chunk| chunk.data.len()).sum();
        let value = project(&source, false, length);
        assert_eq!(value["status"], "complete");
        let body: Value = serde_json::from_str(value["body"].as_str().unwrap()).unwrap();
        assert_eq!(body["password"], "[REDACTED]");
        assert_eq!(body["text"], "bEaReR [REDACTED]");
        assert_eq!(body["input_tokens"], 3);
        assert_eq!(project(&source, false, length - 1)["status"], "omitted");
        for invalid in [r#"{"password":"unfinished"#, "\u{fffd}"] {
            assert_eq!(
                project(&chunks(&[invalid]), false, 1024)["status"],
                "omitted"
            );
        }
        assert_eq!(source, saved);
    }
    #[test]
    fn sse_cross_chunk_events_have_independent_limits_and_preserve_later_events() {
        let source = chunks(&[
            "data: {\"password\":\"cross",
            "-chunk\"}\r",
            "\n\r\n",
            "data: [DONE]\n\n",
        ]);
        let value = project(&source, true, 64);
        assert_eq!(value["status"], "complete");
        assert_eq!(value["events"][0]["data"]["password"], "[REDACTED]");
        assert_eq!(value["events"][1]["data"], "[DONE]");
        let long =
            "data: {\"text\":\"".to_string() + &"x".repeat(128) + "\"}\n\ndata: {\"ok\":true}\n\n";
        let value = project(&chunks(&[&long]), true, 64);
        assert_eq!(value["status"], "partial");
        assert_eq!(
            value["events"][0]["omissionReason"],
            "event_parse_limit_exceeded"
        );
        assert_eq!(value["events"][1]["data"]["ok"], true);
        let repeated = "data: {\"ok\":true}\n\n".repeat(100);
        assert_eq!(
            project(&chunks(&[&repeated]), true, 32)["completedEvents"],
            100
        );
    }
    #[test]
    fn compressed_truncated_and_cancelled_exports_never_fall_back_to_raw() {
        let mut source = chunks(&[
            "data: {\"secret\":\"hidden\"}\n\n",
            "data: {\"ok\":true}\n\n",
        ]);
        source[0].truncated = true;
        let value = project(&source, true, 1024);
        assert_eq!(value["status"], "partial");
        assert_eq!(value["completedEvents"], 1);
        assert!(!value.to_string().contains("hidden"));
        let mut output = Vec::new();
        write_projection(
            &mut output,
            &source,
            &[DiagnosticHeader {
                name: "Content-Encoding".into(),
                value: "gzip".into(),
            }],
            false,
            1024,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap()["omissionReason"],
            "compressed_capture_unrecoverable"
        );
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(write_projection(&mut Vec::new(), &source, &[], false, 1024, &cancel).is_err());
    }
    #[test]
    #[ignore = "large serialized capacity checks: run alone in authorized CI with --test-threads=1"]
    fn json_64_256_mib_and_sse_over_64_mib_boundaries() {
        for mib in [64, 256] {
            let max = mib * 1024 * 1024;
            let text = format!("\"{}\"", "x".repeat(max - 2));
            assert_eq!(project(&chunks(&[&text]), false, max)["status"], "complete");
            assert_eq!(
                project(&chunks(&[&text]), false, max - 1)["status"],
                "omitted"
            );
        }
        let event = format!("data: {{\"text\":\"{}\"}}\n\n", "x".repeat(1024 * 1024));
        let source: Vec<_> = (0..65).map(|_| chunks(&[&event]).remove(0)).collect();
        let mut writer = io::sink();
        write_projection(
            &mut writer,
            &source,
            &[DiagnosticHeader {
                name: "content-type".into(),
                value: "text/event-stream".into(),
            }],
            false,
            64 * 1024 * 1024,
            &CancellationToken::new(),
        )
        .unwrap();
    }
}
