//! Deterministic old/new-engine conformance helpers.
//!
//! Replay intentionally stops at the `UpstreamTransport` boundary.  A test
//! can feed the same request fixture and scripted upstream replies to a
//! legacy facade and the shared engine without issuing duplicate real
//! provider calls.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::HeaderMap;
use axum::response::Response;
use bytes::Bytes;
use futures_util::stream;
use serde::Serialize;
use serde_json::Value;
use sumpter_core::events::RuntimeEvent;

use crate::boundary::InboundRequest;
use crate::engine::events::comparable_event;
use crate::outbound::{OutboundRequest, TransportError, UpstreamResponse, UpstreamTransport};

/// Request fixture used by both sides of a replay.  `body` is bytes in the
/// fixture, but each runner receives a fresh lazy `Body` instance.
#[derive(Debug, Clone)]
pub struct ReplayRequest {
    pub method: axum::http::Method,
    pub uri: axum::http::Uri,
    pub headers: HeaderMap,
    pub remote_ip: Option<std::net::IpAddr>,
    pub body: Bytes,
}

impl ReplayRequest {
    pub fn into_inbound(&self) -> InboundRequest {
        InboundRequest {
            method: self.method.clone(),
            uri: self.uri.clone(),
            headers: self.headers.clone(),
            remote_ip: self.remote_ip,
            body: Body::from(self.body.clone()),
        }
    }
}

/// One deterministic upstream response in a replay script.
#[derive(Debug, Clone)]
pub struct ReplayReply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub chunks: Vec<Result<Bytes, TransportError>>,
}

impl ReplayReply {
    pub fn ok(body: impl Into<Bytes>) -> Self {
        Self {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![Ok(body.into())],
        }
    }
}

/// Scripted transport; every call is recorded so retry order can be compared.
pub struct ReplayTransport {
    replies: Mutex<VecDeque<Result<ReplayReply, TransportError>>>,
    calls: Mutex<Vec<OutboundRequest>>,
}

impl ReplayTransport {
    pub fn new(replies: impl IntoIterator<Item = Result<ReplayReply, TransportError>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> Vec<OutboundRequest> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl UpstreamTransport for ReplayTransport {
    async fn send_streaming(
        &self,
        request: OutboundRequest,
        _response_timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        self.calls.lock().unwrap().push(request);
        match self.replies.lock().unwrap().pop_front() {
            Some(Ok(reply)) => Ok(UpstreamResponse {
                status: reply.status,
                headers: reply.headers,
                stream: Box::pin(stream::iter(reply.chunks)),
            }),
            Some(Err(error)) => Err(error),
            None => Err(TransportError::ConnectionFailed(
                "replay script exhausted".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReplayObservation {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ReplayDifference {
    #[error("status differs: {left} != {right}")]
    Status { left: u16, right: u16 },
    #[error("stable response headers differ")]
    Headers {
        left: Vec<(String, String)>,
        right: Vec<(String, String)>,
    },
    #[error("normalized response body differs")]
    Body { left: Vec<u8>, right: Vec<u8> },
    #[error("runtime event projection differs")]
    Events {
        left: Vec<RuntimeEvent>,
        right: Vec<RuntimeEvent>,
    },
}

/// Consume a response for a replay comparison while retaining only the
/// deterministic wire projection and caller-supplied events.
pub async fn observe_response(
    response: Response,
    events: impl IntoIterator<Item = RuntimeEvent>,
) -> Result<ReplayObservation, String> {
    let status = response.status().as_u16();
    let headers = normalize_headers(response.headers());
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .map_err(|error| error.to_string())?;
    Ok(ReplayObservation {
        status,
        headers,
        body: normalize_body(&body),
        events: events
            .into_iter()
            .map(|event| comparable_event(&event))
            .collect(),
    })
}

pub fn compare(
    left: &ReplayObservation,
    right: &ReplayObservation,
) -> Result<(), ReplayDifference> {
    if left.status != right.status {
        return Err(ReplayDifference::Status {
            left: left.status,
            right: right.status,
        });
    }
    if left.headers != right.headers {
        return Err(ReplayDifference::Headers {
            left: left.headers.clone(),
            right: right.headers.clone(),
        });
    }
    if left.body != right.body {
        return Err(ReplayDifference::Body {
            left: left.body.clone(),
            right: right.body.clone(),
        });
    }
    if left.events != right.events {
        return Err(ReplayDifference::Events {
            left: left.events.clone(),
            right: right.events.clone(),
        });
    }
    Ok(())
}

/// Convert a runtime event from either the frozen legacy crate or the shared
/// crate into the same deterministic JSON projection.  The two crates have
/// intentionally distinct Cargo package identities during parallel
/// migration, so replay tests cannot compare their Rust types directly.
pub fn normalize_event_json<T: Serialize>(event: &T) -> Result<Value, String> {
    let mut value = serde_json::to_value(event).map_err(|error| error.to_string())?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "runtime event is not a JSON object".to_string())?;

    // These fields are generated or clock-derived independently by each run.
    // Remove only timing/identity projections; protocol, retry, outcome,
    // usage, tool and failure semantics remain strict.
    object.insert("id".into(), Value::String("<generated>".into()));
    object.remove("requestID");
    object.insert("timestamp".into(), Value::from(0));
    object.insert("durationMS".into(), Value::from(0));
    object.remove("ttfbMS");
    if let Some(trace) = object.get_mut("streamTrace").and_then(Value::as_object_mut) {
        trace.remove("maxChunkGapMS");
        trace.remove("lastChunkAtMS");
    }
    Ok(value)
}

pub fn normalize_event_json_list<'a, T: Serialize + 'a>(
    events: impl IntoIterator<Item = &'a T>,
) -> Result<Vec<Value>, String> {
    events.into_iter().map(normalize_event_json).collect()
}

fn normalize_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    let mut normalized = headers
        .iter()
        .filter_map(|(name, value)| {
            let key = name.as_str().to_ascii_lowercase();
            if matches!(key.as_str(), "date" | "server" | "via") {
                return None;
            }
            let value = if key == "x-kekulv-request-id" {
                "<request-id>".to_string()
            } else {
                value.to_str().ok()?.to_string()
            };
            Some((key, value))
        })
        .collect::<Vec<_>>();
    normalized.sort();
    normalized
}

/// Normalize known generated JSON fields without reordering SSE frames or
/// changing user-visible values.  Invalid/non-JSON bytes are compared as-is.
pub fn normalize_body(body: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(body) else {
        return body.to_vec();
    };
    if text.contains("data:") {
        return text
            .lines()
            .map(normalize_sse_line)
            .collect::<Vec<_>>()
            .join("\n")
            .into_bytes();
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    serde_json::to_vec(&normalize_json(value)).unwrap_or_else(|_| body.to_vec())
}

fn normalize_sse_line(line: &str) -> String {
    let Some(data) = line.strip_prefix("data:") else {
        return line.to_string();
    };
    let Ok(value) = serde_json::from_str::<Value>(data.trim()) else {
        return line.to_string();
    };
    format!(
        "data: {}",
        serde_json::to_string(&normalize_json(value)).unwrap_or_default()
    )
}

fn normalize_json(value: Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.into_iter().map(normalize_json).collect()),
        Value::Object(mut object) => {
            for key in [
                "id",
                "request_id",
                "requestID",
                "created",
                "created_at",
                "createdAt",
                "timestamp",
            ] {
                if object.contains_key(key) {
                    object.insert(key.to_string(), Value::String("<generated>".into()));
                }
            }
            for child in object.values_mut() {
                let current = std::mem::take(child);
                *child = normalize_json(current);
            }
            Value::Object(object)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_generated_json_fields_but_keeps_frame_order() {
        let body = br#"data: {"id":"a","created":1,"choices":[{"delta":{"text":"one"}}]}

data: {"id":"b","created":2,"choices":[{"delta":{"text":"two"}}]}"#;
        let normalized = String::from_utf8(normalize_body(body)).unwrap();
        assert!(normalized.contains("one"));
        assert!(normalized.contains("two"));
        assert_eq!(normalized.matches("<generated>").count(), 4);
        assert!(normalized.find("one").unwrap() < normalized.find("two").unwrap());
    }
}
