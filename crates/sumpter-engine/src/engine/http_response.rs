//! Client-facing JSON responses and shared proxy failure rendering.

use super::failure::FailureInfo;
use axum::body::Body;
use axum::http::HeaderValue;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::Value;
use sumpter_core::events::RuntimeFailureKind;

pub fn error_response(status: StatusCode, pairs: &[(&str, &str)]) -> Response {
    let mut map = serde_json::Map::new();
    for (k, v) in pairs {
        map.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    json_response(status, &Value::Object(map))
}

pub(super) fn method_not_allowed_response(allow: &str) -> Response {
    let mut response = error_response(
        StatusCode::METHOD_NOT_ALLOWED,
        &[("error", "method_not_allowed")],
    );
    if let Ok(value) = HeaderValue::from_str(allow) {
        response.headers_mut().insert("allow", value);
    }
    response
}

pub(super) fn proxy_failure_response(
    status: StatusCode,
    error: &str,
    request_id: &str,
    failure: &FailureInfo,
    retry_delay_seconds: Option<f64>,
    pass_through_retry_delay: bool,
) -> Response {
    let mut body = serde_json::Map::new();
    body.insert("error".into(), Value::String(error.into()));
    body.insert("requestID".into(), Value::String(request_id.into()));
    body.insert(
        "failureKind".into(),
        Value::String(failure.kind.as_str().into()),
    );
    body.insert(
        "failurePhase".into(),
        Value::String(failure.phase.as_str().into()),
    );
    if let Some(detail) = &failure.detail {
        body.insert("message".into(), Value::String(detail.clone()));
    }
    if let Some(timeout_ms) = failure.timeout_ms {
        body.insert("timeoutMS".into(), Value::Number(timeout_ms.into()));
    }
    if let Some(upstream_status) = failure.upstream_status_code {
        body.insert(
            "upstreamStatusCode".into(),
            Value::Number(upstream_status.into()),
        );
    }
    if let Some(upstream_request_id) = &failure.upstream_request_id {
        body.insert(
            "upstreamRequestID".into(),
            Value::String(upstream_request_id.clone()),
        );
    }
    let configured_retry_delay =
        retry_delay_seconds.filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    let provider_retry_delay = failure
        .retry_after_seconds
        .filter(|seconds| seconds.is_finite() && *seconds > 0.0);
    let retry_delay_seconds = pass_through_retry_delay
        .then(|| {
            matches!(
                failure.kind,
                RuntimeFailureKind::ResponseTimeout
                    | RuntimeFailureKind::ConnectionFailed
                    | RuntimeFailureKind::UpstreamHttpStatus
                    | RuntimeFailureKind::EndpointsExhausted
            )
            .then(|| match (configured_retry_delay, provider_retry_delay) {
                (Some(configured), Some(provider)) => Some(configured.max(provider)),
                (Some(configured), None) => Some(configured),
                (None, Some(provider)) => Some(provider),
                (None, None) => None,
            })
            .flatten()
        })
        .flatten();
    if let Some(seconds) = retry_delay_seconds {
        body.insert(
            "retry_delay".into(),
            serde_json::Number::from_f64(seconds)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    let mut response = json_response(status, &Value::Object(body));
    if let Ok(value) = HeaderValue::from_str(request_id) {
        response.headers_mut().insert("x-sumpter-request-id", value);
    }
    if let Some(seconds) = retry_delay_seconds {
        // 标准 Retry-After 只接受整数 delay-seconds；向上取整避免客户端过早重试。
        let delay = seconds.ceil().min(u64::MAX as f64) as u64;
        if let Ok(value) = HeaderValue::from_str(&delay.to_string()) {
            response.headers_mut().insert("retry-after", value);
        }
    }
    response
}

pub fn json_response(status: StatusCode, body: &Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .body(Body::from(serde_json::to_vec(body).unwrap_or_default()))
        .unwrap_or_default()
}

pub fn json_bytes_response(status: StatusCode, body: Vec<u8>) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .body(Body::from(body))
        .unwrap_or_default()
}
