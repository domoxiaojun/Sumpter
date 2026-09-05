use super::{AdminState, JsonPayload, api_error, json_ok, require_json};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};

pub(crate) async fn diagnostics(State(state): State<AdminState>) -> Response {
    let config = state.inner.engine.config();
    let proxy = state.inner.proxy.status().await;
    let warnings = sumpter_core::warnings::evaluate(&config)
        .into_iter()
        .map(|message| json!({"level": "warning", "message": message}))
        .collect::<Vec<_>>();
    json_ok(&json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeSeconds": state.inner.engine.uptime_seconds(),
        "configPath": state.inner.config_dir.config_path().to_string_lossy(),
        "webRoot": state.inner.web_root.as_ref().map(|path| path.to_string_lossy().to_string()),
        "generation": state.inner.engine.generation(),
        "adminListener": {
            "host": state.inner.admin_listen.host,
            "port": state.inner.admin_listen.port,
        },
        "proxyListener": {"host": config.listener.host, "port": config.listener.port},
        "proxyRunning": proxy.running,
        "warnings": warnings,
        "lastError": state.inner.engine.last_error(),
        "statsWritable": state.inner.engine.stats_writable(),
        "journalctlCommand": state.inner.systemd_scope.journalctl_command(),
        "systemdScope": state.inner.systemd_scope.as_str(),
    }))
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiagnosticCaptureBody {
    enabled: bool,
    #[serde(rename = "maxBytes")]
    max_bytes: Option<usize>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiagnosticCaptureExportQuery {
    scope: Option<String>,
    format: Option<String>,
    privacy: Option<String>,
    #[serde(default)]
    confirm_raw: bool,
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DiagnosticCaptureFormat {
    Jsonl,
    Json,
}

impl DiagnosticCaptureFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Jsonl => "jsonl",
            Self::Json => "json",
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Self::Jsonl => "application/x-ndjson; charset=utf-8",
            Self::Json => "application/json; charset=utf-8",
        }
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn parse_diagnostic_capture_format(
    raw: Option<&str>,
) -> Result<DiagnosticCaptureFormat, Response> {
    match raw.unwrap_or("jsonl") {
        "jsonl" => Ok(DiagnosticCaptureFormat::Jsonl),
        "json" => Ok(DiagnosticCaptureFormat::Json),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_format",
            "format 只允许 jsonl 或 json",
        )),
    }
}

pub(crate) fn sensitive_capture_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_'], "");
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

pub(crate) fn sensitive_header_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    sensitive_capture_key(&name)
        || name.contains("token")
        || name.contains("auth")
        || name == "proxy-authenticate"
        || name == "www-authenticate"
}

pub(crate) fn redact_url_query(raw: &str) -> String {
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
            if sensitive_capture_key(key) || key.to_ascii_lowercase().contains("token") {
                format!("{key}=[REDACTED]")
            } else {
                format!("{key}{value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{prefix}{redacted}{fragment}")
}

pub(crate) fn redact_text_secrets(text: &str) -> String {
    let mut output = text.to_string();
    for marker in [
        "Bearer ",
        "Basic ",
        "token=",
        "access_token=",
        "api_key=",
        "apiKey=",
    ] {
        let mut search_from = 0;
        while let Some(relative) = output[search_from..].find(marker) {
            let start = search_from + relative + marker.len();
            let end = output[start..]
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '&' | ',' | '"' | '\''))
                .map(|offset| start + offset)
                .unwrap_or(output.len());
            output.replace_range(start..end, "[REDACTED]");
            search_from = start + "[REDACTED]".len();
            if search_from >= output.len() {
                break;
            }
        }
    }
    output
}

pub(crate) fn redact_json_value(value: &mut Value) {
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

pub(crate) fn redact_body_text(body: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json_value(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| "[REDACTED_BODY]".into())
    } else {
        redact_text_secrets(body)
    }
}

pub(crate) fn redact_capture_value(value: &mut Value) {
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
                                        *header_value = Value::String(redact_text_secrets(text));
                                    }
                                }
                            }
                        }
                    }
                    "inboundBody" | "outboundBody" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_body_text(text));
                        }
                    }
                    "outboundURL" => {
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
        _ => {}
    }
}

pub(crate) fn diagnostic_capture_wire(
    record: &sumpter_core::events::DiagnosticRequestCapture,
    redacted: bool,
) -> Result<Vec<u8>, String> {
    let mut value = serde_json::to_value(record).map_err(|error| error.to_string())?;
    if redacted {
        redact_capture_value(&mut value);
    }
    serde_json::to_vec(&value).map_err(|error| error.to_string())
}

pub(crate) fn diagnostic_capture_response(
    body: Body,
    format: DiagnosticCaptureFormat,
    scope: &str,
    privacy: &str,
) -> Response {
    let filename = format!(
        "attachment; filename=\"sumpter-diagnostic-{scope}-{privacy}.{}\"",
        format.extension()
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, format.content_type())
        .header(header::CONTENT_DISPOSITION, filename)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("x-sumpter-privacy", privacy)
        .body(body)
        .unwrap_or_default()
}
pub(crate) async fn diagnostic_capture(State(state): State<AdminState>) -> Response {
    json_ok(&state.inner.engine.diagnostic_capture_index())
}
pub(crate) async fn diagnostic_capture_detail(
    State(state): State<AdminState>,
    Path(request_id): Path<String>,
) -> Response {
    // A selected record can approach the configured capture limit. Keep the
    // lock/read and serde allocation off the async runtime worker so one large
    // detail request cannot stall index refreshes or other Admin endpoints.
    let engine = state.inner.engine.clone();
    match tokio::task::spawn_blocking(move || engine.diagnostic_capture_detail_json(&request_id))
        .await
    {
        Ok(Ok(Some(body))) => crate::engine::json_bytes_response(StatusCode::OK, body),
        Ok(Ok(None)) => api_error(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求"),
        Ok(Err(error)) => {
            tracing::error!(%error, "诊断捕获详情 JSON 编码失败");
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capture_detail_encode_failed",
                "诊断捕获详情编码失败",
            )
        }
        Err(error) => {
            tracing::error!(%error, "诊断捕获详情读取任务异常");
            api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "capture_detail_unavailable",
                "诊断捕获详情读取任务未能完成，请稍后重试",
            )
        }
    }
}

/// 全量捕获导出必须走最近一次原子落盘文件的流式响应，不能经过 `json_ok` 或
/// WebUI 的 `response.text()`，否则会把接近 512 MiB 的正文再次聚合到内存。
pub(crate) async fn diagnostic_capture_export(
    State(state): State<AdminState>,
    Query(query): Query<DiagnosticCaptureExportQuery>,
) -> Response {
    let Some(scope) = query.scope.as_deref() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_scope_required",
            "导出必须明确指定 scope=current、selected 或 all",
        );
    };
    if !matches!(scope, "current" | "selected" | "all") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_scope",
            "不支持的诊断捕获导出范围",
        );
    }
    let format = match parse_diagnostic_capture_format(query.format.as_deref()) {
        Ok(format) => format,
        Err(response) => return response,
    };
    let privacy = query.privacy.as_deref().unwrap_or("raw");
    if !matches!(privacy, "redacted" | "raw") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_privacy",
            "privacy 只允许 redacted 或 raw",
        );
    }
    if privacy == "raw" && !query.confirm_raw {
        return api_error(
            StatusCode::BAD_REQUEST,
            "raw_capture_confirmation_required",
            "raw 导出必须显式确认 confirmRaw=true",
        );
    }
    if scope == "selected" && query.request_id.as_deref().is_none_or(str::is_empty) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_request_required",
            "selected 导出必须提供 requestID",
        );
    }
    if scope != "selected" && query.request_id.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_request_not_allowed",
            "只有 selected 导出允许 requestID",
        );
    }
    let redacted = privacy == "redacted";
    let engine = state.inner.engine.clone();
    if scope == "current" {
        let bytes = serde_json::to_vec(&engine.diagnostic_capture_index())
            .unwrap_or_else(|_| b"{}".to_vec());
        let body = if format == DiagnosticCaptureFormat::Jsonl {
            let mut line = bytes;
            line.push(b'\n');
            Body::from(line)
        } else {
            Body::from(bytes)
        };
        return diagnostic_capture_response(body, format, scope, privacy);
    }
    if scope == "selected" {
        let request_id = query.request_id.unwrap_or_default();
        return match tokio::task::spawn_blocking(move || {
            engine.diagnostic_capture_detail(&request_id)
        })
        .await
        {
            Ok(Some(record)) => match diagnostic_capture_wire(&record, redacted) {
                Ok(mut bytes) => {
                    if format == DiagnosticCaptureFormat::Jsonl {
                        bytes.push(b'\n');
                    }
                    diagnostic_capture_response(Body::from(bytes), format, scope, privacy)
                }
                Err(error) => {
                    tracing::error!(%error, "诊断捕获详情脱敏编码失败");
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "capture_detail_encode_failed",
                        "诊断捕获详情编码失败",
                    )
                }
            },
            Ok(None) => api_error(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求"),
            Err(_) => api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "capture_detail_unavailable",
                "诊断捕获详情读取任务未能完成，请稍后重试",
            ),
        };
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let worker_engine = engine.clone();
    tokio::task::spawn_blocking(move || {
        let worker_sender = sender.clone();
        let mut first = true;
        let send = |bytes: Vec<u8>| {
            worker_sender
                .blocking_send(Ok(Bytes::from(bytes)))
                .map_err(|_| "export client disconnected".to_string())
        };
        let result = (|| {
            if format == DiagnosticCaptureFormat::Json {
                send(br#"{"records":["#.to_vec())?;
            }
            worker_engine.with_diagnostic_capture_records(|record| {
                let mut bytes = diagnostic_capture_wire(record, redacted)?;
                if format == DiagnosticCaptureFormat::Json {
                    if !first {
                        let mut comma = vec![b','];
                        comma.append(&mut bytes);
                        bytes = comma;
                    }
                    first = false;
                } else {
                    bytes.push(b'\n');
                }
                send(bytes)
            })?;
            if format == DiagnosticCaptureFormat::Json {
                send(b"]}".to_vec())?;
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            let _ = worker_sender.blocking_send(Err(std::io::Error::other(error)));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    diagnostic_capture_response(Body::from_stream(stream), format, scope, privacy)
}
pub(crate) async fn set_diagnostic_capture(
    State(state): State<AdminState>,
    payload: JsonPayload<DiagnosticCaptureBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(v) => v,
        Err(r) => return r,
    };
    if body.enabled && body.max_bytes == Some(0) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_capacity",
            "maxBytes 必须大于 0",
        );
    };
    state
        .inner
        .engine
        .set_diagnostic_capture(body.enabled, body.max_bytes);
    json_ok(&state.inner.engine.diagnostic_capture_index())
}
pub(crate) async fn clear_diagnostic_capture(State(state): State<AdminState>) -> Response {
    match state.inner.engine.clear_diagnostic_capture() {
        Ok(()) => json_ok(&json!({"cleared":true})),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostic_capture_clear_failed",
            &message,
        ),
    }
}
