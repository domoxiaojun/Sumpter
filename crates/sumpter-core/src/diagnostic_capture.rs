//! Binary capture redaction shared by both Admin adapters. Source stays unchanged.
use serde_json::{Map, Value};

fn compressed(headers: Option<&Value>) -> bool {
    headers.and_then(Value::as_array).is_some_and(|headers| {
        headers.iter().any(|header| {
            header
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case("content-encoding"))
                && header
                    .get("value")
                    .and_then(Value::as_str)
                    .is_some_and(|value| {
                        value
                            .split(',')
                            .any(|encoding| !encoding.trim().eq_ignore_ascii_case("identity"))
                    })
        })
    })
}

fn remove(object: &mut Map<String, Value>, field: &str, marker: &str) {
    if object.contains_key(field) {
        object.insert(field.into(), Value::String(String::new()));
        object.insert(marker.into(), Value::Bool(true));
    }
}

fn remove_chunks(object: &mut Map<String, Value>, field: &str) {
    if let Some(chunks) = object.get_mut(field).and_then(Value::as_array_mut) {
        for chunk in chunks {
            if let Some(chunk) = chunk.as_object_mut() {
                remove(chunk, "data", "redacted");
            }
        }
    }
}

/// Apply before the platform's ordinary textual/header redaction. A compressed
/// legacy capture must also be removed: its text may be damaged, not sanitized.
pub fn redact_binary_payloads(object: &mut Map<String, Value>) {
    for (field, encoding, headers, marker) in [
        (
            "inboundBody",
            "inboundBodyEncoding",
            "inboundHeaders",
            "inboundBodyRedacted",
        ),
        (
            "outboundBody",
            "outboundBodyEncoding",
            "outboundHeaders",
            "outboundBodyRedacted",
        ),
        ("data", "dataEncoding", "", "redacted"),
    ] {
        let encoded = object
            .get(encoding)
            .and_then(Value::as_str)
            .is_some_and(|value| value != "utf8");
        if encoded || compressed(object.get(headers)) {
            remove(object, field, marker);
        }
    }
    if compressed(object.get("responseHeaders")) {
        remove_chunks(object, "upstreamChunks");
    }
    let native = object.get("routeMode").and_then(Value::as_str) != Some("translated");
    let compressed_client = native
        && object
            .get("attempts")
            .and_then(Value::as_array)
            .is_some_and(|attempts| {
                attempts
                    .iter()
                    .any(|attempt| compressed(attempt.get("responseHeaders")))
            });
    if compressed_client {
        remove_chunks(object, "clientChunks");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{DiagnosticChunk, DiagnosticDataEncoding};
    use serde_json::json;

    #[test]
    fn legacy_chunk_encoding_defaults_to_utf8_and_new_binary_round_trips() {
        let legacy = json!({"atMS":3,"bytes":2,"data":"hi","truncated":false});
        let chunk: DiagnosticChunk = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(chunk.data_encoding, DiagnosticDataEncoding::Utf8);
        assert_eq!(serde_json::to_value(&chunk).unwrap(), legacy);
        let binary =
            json!({"atMS":4,"bytes":2,"data":"AP8=","dataEncoding":"base64","truncated":false});
        let chunk: DiagnosticChunk = serde_json::from_value(binary.clone()).unwrap();
        assert_eq!(serde_json::to_value(&chunk).unwrap(), binary);
    }
}
