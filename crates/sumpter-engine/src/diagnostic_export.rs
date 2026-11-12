//! Shared Admin export boundary: bounded work queue and streaming serialization.
use crate::diagnostic_redaction::{self, DEFAULT_MAX_PARSE_MIB};
use crate::engine::{Engine, error_response};
use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use serde::Deserialize;
use std::io::{self, Write};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCaptureExportQuery {
    pub scope: Option<String>,
    pub format: Option<String>,
    pub privacy: Option<String>,
    #[serde(default)]
    pub confirm_raw: bool,
    #[serde(rename = "requestID", alias = "requestId")]
    pub request_id: Option<String>,
    pub max_parse_mib: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticCaptureFormat {
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
pub fn parse_diagnostic_capture_format(
    raw: Option<&str>,
) -> Result<DiagnosticCaptureFormat, Response> {
    match raw.unwrap_or("jsonl") {
        "jsonl" => Ok(DiagnosticCaptureFormat::Jsonl),
        "json" => Ok(DiagnosticCaptureFormat::Json),
        _ => Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid_capture_format",
            "format 只允许 jsonl 或 json",
        )),
    }
}
fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    error_response(status, &[("error", code), ("message", message)])
}

struct RedactionPool {
    active: Arc<Semaphore>,
    waiting: Arc<Semaphore>,
}
impl RedactionPool {
    fn new() -> Self {
        Self {
            active: Arc::new(Semaphore::new(2)),
            waiting: Arc::new(Semaphore::new(4)),
        }
    }
    async fn acquire(&self) -> Result<OwnedSemaphorePermit, &'static str> {
        if let Ok(permit) = self.active.clone().try_acquire_owned() {
            return Ok(permit);
        }
        let _waiting = self
            .waiting
            .clone()
            .try_acquire_owned()
            .map_err(|_| "redaction_queue_full")?;
        tokio::time::timeout(Duration::from_secs(30), self.active.clone().acquire_owned())
            .await
            .map_err(|_| "redaction_queue_timeout")?
            .map_err(|_| "redaction_unavailable")
    }
}
static POOL: OnceLock<RedactionPool> = OnceLock::new();

struct ExportWriter {
    sender: tokio::sync::mpsc::Sender<Result<Bytes, io::Error>>,
    buffer: Vec<u8>,
    cancel: CancellationToken,
}
impl Write for ExportWriter {
    fn write(&mut self, mut input: &[u8]) -> io::Result<usize> {
        let length = input.len();
        while !input.is_empty() {
            if self.cancel.is_cancelled() || self.sender.is_closed() {
                return Err(io::Error::other("export cancelled"));
            }
            let count = input.len().min(64 * 1024 - self.buffer.len());
            self.buffer.extend_from_slice(&input[..count]);
            input = &input[count..];
            if self.buffer.len() == 64 * 1024 {
                self.flush()?;
            }
        }
        Ok(length)
    }
    fn flush(&mut self) -> io::Result<()> {
        if !self.buffer.is_empty() {
            self.sender
                .blocking_send(Ok(Bytes::from(std::mem::take(&mut self.buffer))))
                .map_err(|_| io::Error::other("export cancelled"))?;
        }
        Ok(())
    }
}

pub async fn export(engine: Engine, query: DiagnosticCaptureExportQuery) -> Response {
    let Some(scope) = query.scope.as_deref() else {
        return failure(
            StatusCode::BAD_REQUEST,
            "capture_scope_required",
            "导出必须明确指定 scope=current、selected 或 all",
        );
    };
    if !matches!(scope, "current" | "selected" | "all") {
        return failure(
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
        return failure(
            StatusCode::BAD_REQUEST,
            "invalid_capture_privacy",
            "privacy 只允许 redacted 或 raw",
        );
    }
    if privacy == "raw" && !query.confirm_raw {
        return failure(
            StatusCode::BAD_REQUEST,
            "raw_capture_confirmation_required",
            "raw 导出必须显式确认 confirmRaw=true",
        );
    }
    if scope == "selected" && query.request_id.as_deref().is_none_or(str::is_empty) {
        return failure(
            StatusCode::BAD_REQUEST,
            "capture_request_required",
            "selected 导出必须提供 requestID",
        );
    }
    if scope != "selected" && query.request_id.is_some() {
        return failure(
            StatusCode::BAD_REQUEST,
            "capture_request_not_allowed",
            "只有 selected 导出允许 requestID",
        );
    }
    let max_parse = query.max_parse_mib.unwrap_or(DEFAULT_MAX_PARSE_MIB);
    if !(1..=256).contains(&max_parse) {
        return failure(
            StatusCode::BAD_REQUEST,
            "invalid_max_parse_mib",
            "maxParseMib 必须为 1–256 的整数",
        );
    }
    let redacted = privacy == "redacted";
    let permit = if redacted {
        match POOL.get_or_init(RedactionPool::new).acquire().await {
            Ok(permit) => Some(Arc::new(permit)),
            Err(code) => {
                return failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    code,
                    "脱敏处理繁忙（最多 2 个处理、4 个等待，等待不超过 30 秒），请稍后重试",
                );
            }
        }
    } else {
        None
    };
    let selected = if scope == "selected" {
        let engine = engine.clone();
        let id = query.request_id.unwrap_or_default();
        let lease = permit.clone();
        match tokio::task::spawn_blocking(move || {
            let _lease = lease;
            engine.diagnostic_capture_detail(&id)
        })
        .await
        {
            Ok(Some(record)) => Some(record),
            Ok(None) => {
                return failure(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求");
            }
            Err(_) => {
                return failure(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "capture_detail_unavailable",
                    "诊断捕获详情读取失败",
                );
            }
        }
    } else {
        None
    };
    let filename = format!(
        "attachment; filename=\"sumpter-diagnostic-{scope}-{privacy}.{}\"",
        format.extension()
    );
    let current = scope == "current";
    let all = scope == "all";
    let (sender, receiver) = tokio::sync::mpsc::channel(2);
    let cancel = CancellationToken::new();
    let cancel_on_drop = cancel.clone().drop_guard();
    tokio::task::spawn_blocking(move || {
        // The blocking worker owns this lease, including after HTTP cancellation.
        let _permit = permit;
        let mut writer = ExportWriter {
            sender: sender.clone(),
            buffer: Vec::new(),
            cancel: cancel.clone(),
        };
        let result = (|| -> io::Result<()> {
            if current {
                let mut value = engine.diagnostic_capture_index();
                if redacted {
                    diagnostic_redaction::redact_capture_value(&mut value);
                }
                serde_json::to_writer(&mut writer, &value)?;
                if format == DiagnosticCaptureFormat::Jsonl {
                    writer.write_all(b"\n")?;
                }
            } else {
                if all && format == DiagnosticCaptureFormat::Json {
                    writer.write_all(b"{\"records\":[")?;
                }
                let mut first = true;
                let mut visit = |record: &sumpter_core::events::DiagnosticRequestCapture| -> Result<(), String> {
                    let result = (|| -> io::Result<()> {
                        if cancel.is_cancelled() { return Err(io::Error::other("export cancelled")); }
                        if all && format == DiagnosticCaptureFormat::Json && !first { writer.write_all(b",")?; }
                        first = false;
                        if redacted { diagnostic_redaction::write_capture(&mut writer, record, usize::from(max_parse) * 1024 * 1024, &cancel)?; }
                        else { serde_json::to_writer(&mut writer, record)?; }
                        if format == DiagnosticCaptureFormat::Jsonl { writer.write_all(b"\n")?; }
                        Ok(())
                    })();
                    result.map_err(|error| error.to_string())
                };
                if let Some(record) = selected {
                    visit(&record).map_err(io::Error::other)?;
                } else {
                    engine
                        .with_diagnostic_capture_records(visit)
                        .map_err(io::Error::other)?;
                }
                if all && format == DiagnosticCaptureFormat::Json {
                    writer.write_all(b"]}")?;
                }
            }
            writer.flush()
        })();
        if let Err(error) = result {
            let _ = sender.blocking_send(Err(error));
        }
    });
    let stream = futures_util::stream::unfold(
        (receiver, cancel_on_drop),
        |(mut receiver, guard)| async move {
            receiver.recv().await.map(|item| (item, (receiver, guard)))
        },
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, format.content_type())
        .header(header::CONTENT_DISPOSITION, filename)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("x-sumpter-privacy", privacy)
        .body(Body::from_stream(stream))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn redaction_waits_for_actual_permits_and_times_out() {
        let pool = RedactionPool::new();
        let first = pool.acquire().await.unwrap();
        let second = pool.acquire().await.unwrap();
        assert_eq!(pool.acquire().await.unwrap_err(), "redaction_queue_timeout");
        let slots: Vec<_> = (0..4)
            .map(|_| pool.waiting.clone().try_acquire_owned().unwrap())
            .collect();
        assert_eq!(pool.acquire().await.unwrap_err(), "redaction_queue_full");
        drop(slots);
        drop(first);
        drop(second);
        assert!(pool.acquire().await.is_ok());
    }
}
