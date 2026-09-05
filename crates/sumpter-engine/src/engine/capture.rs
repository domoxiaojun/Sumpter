//! Diagnostic capture state, bounded indexes, and engine capture operations.
//!
//! Full capture storage stays private to the engine. Public methods expose
//! snapshots and bounded metadata while request/attempt helpers remain
//! `pub(super)` for the relay and event modules.

pub use sumpter_core::events::{DiagnosticCaptureSnapshot, DiagnosticRequestCapture};

pub const DEFAULT_MAX_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_INDEX_RECORDS: usize = 200;
pub const DEFAULT_CAPTURE_MAX_BYTES: usize = DEFAULT_MAX_BYTES;

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Instant;

use serde_json::{Value, json};

use sumpter_core::events::{
    ClientDeclaredMetadata, DiagnosticAttemptCapture, DiagnosticChunk, DiagnosticHeader,
    RuntimeEvent,
};
use sumpter_core::routing::PlannedEndpoint;

use super::Engine;
use super::context::ClientMeta;
use super::events::{new_event_id, protocol_token};
use super::state::now_unix;
use crate::outbound::TransportError;

const MAX_CAPTURE_INDEX_RECORDS: usize = MAX_INDEX_RECORDS;
const CAPTURE_STOP_MANUAL: &str = "manual";
pub(super) const CAPTURE_STOP_CAPACITY: &str = "capacity_limit";

pub(super) struct DiagnosticCaptureState {
    pub(super) enabled: bool,
    pub(super) started_at: Option<f64>,
    pub(super) max_bytes: usize,
    pub(super) captured_bytes: usize,
    pub(super) limit_reached: bool,
    pub(super) stop_reason: Option<String>,
    pub(super) records: Vec<DiagnosticRequestCapture>,
    pub(super) attempt_started: HashMap<String, Instant>,
}

/// 诊断捕获的轻量索引缓存。
///
/// 完整记录仍然保存在 `DiagnosticCaptureState` 中并按原样持久化；索引只
/// 保存最近 200 条元数据。把它拆成独立锁后，后台落盘复制 512 MB 明文时，
/// Admin 刷新不必等待完整记录锁，也不会把正文带回前端。
pub(super) struct DiagnosticCaptureIndexCache {
    enabled: bool,
    started_at: Option<f64>,
    max_bytes: usize,
    captured_bytes: usize,
    limit_reached: bool,
    stop_reason: Option<String>,
    record_count: usize,
    records: Vec<Value>,
}

fn record_size(record: &DiagnosticRequestCapture) -> usize {
    record.request_id.len()
        + record.method.len()
        + record.path.len()
        + record.client_model.len()
        + record.effective_model.len()
        + record.feature_rule_id.as_ref().map_or(0, String::len)
        + record
            .client_declared
            .as_ref()
            .map_or(0, client_declared_size)
        + record.failure_detail.as_ref().map_or(0, String::len)
        + record.inbound_body.len()
        + record
            .inbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + record
            .attempts
            .iter()
            .map(diagnostic_attempt_size)
            .sum::<usize>()
        + record
            .client_chunks
            .iter()
            .map(|chunk| chunk.data.len())
            .sum::<usize>()
}

fn client_declared_size(declared: &ClientDeclaredMetadata) -> usize {
    declared.project.as_ref().map_or(0, String::len)
        + declared.workspace.as_ref().map_or(0, String::len)
        + declared.git_remote.as_ref().map_or(0, String::len)
        + declared.user.as_ref().map_or(0, String::len)
}

fn diagnostic_attempt_size(attempt: &DiagnosticAttemptCapture) -> usize {
    attempt.id.len()
        + attempt.endpoint_id.len()
        + attempt.endpoint_name.len()
        + attempt.protocol.len()
        + attempt.pinned_ip.as_ref().map_or(0, String::len)
        + attempt.outbound_method.len()
        + attempt.outbound_url.len()
        + attempt.outbound_body.len()
        + attempt.error.as_ref().map_or(0, String::len)
        + attempt
            .outbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + attempt
            .response_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>()
        + attempt
            .upstream_chunks
            .iter()
            .map(|chunk| chunk.data.len())
            .sum::<usize>()
}

fn diagnostic_attempt_response_size(attempt: &DiagnosticAttemptCapture) -> usize {
    attempt
        .response_headers
        .iter()
        .map(|header| header.name.len() + header.value.len())
        .sum::<usize>()
        + attempt.error.as_ref().map_or(0, String::len)
}

pub(super) fn diagnostic_capture_size(records: &[DiagnosticRequestCapture]) -> usize {
    records.iter().map(record_size).sum()
}

fn diagnostic_capture_index_record(record: &DiagnosticRequestCapture) -> Value {
    json!({
        "requestID": record.request_id,
        "timestamp": record.timestamp,
        "method": record.method,
        "path": record.path,
        "clientKind": record.client_kind,
        "requestPurpose": record.request_purpose,
        "clientModel": record.client_model,
        "effectiveModel": record.effective_model,
        "featureRuleID": record.feature_rule_id,
        "sourceFormat": record.source_format,
        "targetFormat": record.target_format,
        "routeMode": record.route_mode,
        "completedAtMS": record.completed_at_ms,
        "statusCode": record.status_code,
        "outcome": record.outcome,
        "failureKind": record.failure_kind,
        "truncated": record.truncated,
        "attemptCount": record.attempts.len(),
        "clientChunkCount": record.client_chunks.len(),
    })
}

pub(super) fn capture_index_cache_from_capture(
    capture: &DiagnosticCaptureState,
) -> DiagnosticCaptureIndexCache {
    DiagnosticCaptureIndexCache {
        enabled: capture.enabled,
        started_at: capture.started_at,
        max_bytes: capture.max_bytes,
        captured_bytes: capture.captured_bytes,
        limit_reached: capture.limit_reached,
        stop_reason: capture.stop_reason.clone(),
        record_count: capture.records.len(),
        records: capture
            .records
            .iter()
            .take(MAX_CAPTURE_INDEX_RECORDS)
            .map(diagnostic_capture_index_record)
            .collect(),
    }
}

fn sync_capture_index_status(
    cache: &mut DiagnosticCaptureIndexCache,
    capture: &DiagnosticCaptureState,
) {
    cache.enabled = capture.enabled;
    cache.started_at = capture.started_at;
    cache.max_bytes = capture.max_bytes;
    cache.captured_bytes = capture.captured_bytes;
    cache.limit_reached = capture.limit_reached;
    cache.stop_reason = capture.stop_reason.clone();
    cache.record_count = capture.records.len();
}

fn capture_snapshot(capture: &DiagnosticCaptureState) -> DiagnosticCaptureSnapshot {
    DiagnosticCaptureSnapshot {
        enabled: capture.enabled,
        started_at: capture.started_at,
        max_bytes: capture.max_bytes,
        captured_bytes: capture.captured_bytes,
        limit_reached: capture.limit_reached,
        stop_reason: capture.stop_reason.clone(),
        records: capture.records.clone(),
    }
}

fn diagnostic_text(bytes: &[u8], remaining: usize) -> (String, bool) {
    let take = bytes.len().min(remaining);
    let mut text = String::from_utf8_lossy(&bytes[..take]).into_owned();
    if text.len() > remaining {
        let mut end = remaining.min(text.len());
        while !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        text.truncate(end);
    }
    (text, take < bytes.len())
}

fn diagnostic_headers(
    headers: &[(String, String)],
    remaining: usize,
) -> (Vec<DiagnosticHeader>, bool) {
    let mut out = Vec::new();
    let mut left = remaining;
    for (name, value) in headers {
        if left == 0 {
            return (out, true);
        }
        let (name, nt) = diagnostic_text(name.as_bytes(), left);
        left = left.saturating_sub(name.len());
        let (value, vt) = diagnostic_text(value.as_bytes(), left);
        left = left.saturating_sub(value.len());
        out.push(DiagnosticHeader { name, value });
        if nt || vt {
            return (out, true);
        }
    }
    (out, false)
}

fn refresh_capture_usage(capture: &mut DiagnosticCaptureState, truncated: bool) {
    if truncated || capture.captured_bytes >= capture.max_bytes {
        capture.enabled = false;
        capture.limit_reached = true;
        capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
        capture.attempt_started.clear();
    }
}

impl Default for DiagnosticCaptureState {
    fn default() -> Self {
        Self {
            enabled: false,
            started_at: None,
            max_bytes: DEFAULT_CAPTURE_MAX_BYTES,
            captured_bytes: 0,
            limit_reached: false,
            stop_reason: None,
            records: Vec::new(),
            attempt_started: HashMap::new(),
        }
    }
}
impl Engine {
    pub fn diagnostic_capture_snapshot(&self) -> DiagnosticCaptureSnapshot {
        capture_snapshot(&self.inner.capture.lock().unwrap())
    }

    pub fn diagnostic_capture_index(&self) -> Value {
        let cache = self.inner.capture_index.lock().unwrap();
        json!({"enabled": cache.enabled, "startedAt": cache.started_at, "maxBytes": cache.max_bytes,
            "capturedBytes": cache.captured_bytes, "limitReached": cache.limit_reached,
            "stopReason": cache.stop_reason, "recordCount": cache.record_count,
            "indexTruncated": cache.record_count > MAX_CAPTURE_INDEX_RECORDS,
            "records": cache.records.clone()})
    }

    /// 同步只读索引的状态字段，而不复制任何正文。
    fn sync_capture_index_status(&self, capture: &DiagnosticCaptureState) {
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
    }

    /// 新记录插入/删除会改变窗口，重建最多 200 条轻量元数据即可。
    fn sync_capture_index_window(&self, capture: &DiagnosticCaptureState) {
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
        cache.records = capture
            .records
            .iter()
            .take(MAX_CAPTURE_INDEX_RECORDS)
            .map(diagnostic_capture_index_record)
            .collect();
    }

    /// 更新窗口内的一条记录；窗口外的明文记录仍完整保留，但不需要为
    /// 不可见的索引分配 JSON。
    fn sync_capture_index_record(&self, capture: &DiagnosticCaptureState, request_id: &str) {
        let position = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id);
        let mut cache = self.inner.capture_index.lock().unwrap();
        sync_capture_index_status(&mut cache, capture);
        match position {
            Some(position) if position < MAX_CAPTURE_INDEX_RECORDS => {
                let value = diagnostic_capture_index_record(&capture.records[position]);
                if let Some(existing) = cache.records.iter_mut().find(|entry| {
                    entry.get("requestID").and_then(Value::as_str) == Some(request_id)
                }) {
                    *existing = value;
                } else {
                    // Defensive recovery if a prior update was interrupted.
                    cache.records = capture
                        .records
                        .iter()
                        .take(MAX_CAPTURE_INDEX_RECORDS)
                        .map(diagnostic_capture_index_record)
                        .collect();
                }
            }
            _ => {
                cache.records.retain(|entry| {
                    entry.get("requestID").and_then(Value::as_str) != Some(request_id)
                });
            }
        }
    }

    pub fn diagnostic_capture_detail(&self, request_id: &str) -> Option<DiagnosticRequestCapture> {
        self.inner
            .capture
            .lock()
            .unwrap()
            .records
            .iter()
            .find(|r| r.request_id == request_id)
            .cloned()
    }

    /// 为 Admin 详情端点提供已经序列化的 JSON，避免先构造 serde_json::Value 再二次编码。
    /// 记录仍在锁外序列化，避免大正文占用捕获写入锁；调用方只在用户明确选中请求时使用。
    /// 序列化失败必须与“未找到”区分，避免把损坏详情静默伪装成 404。
    pub fn diagnostic_capture_detail_json(
        &self,
        request_id: &str,
    ) -> Result<Option<Vec<u8>>, serde_json::Error> {
        let Some(record) = self.diagnostic_capture_detail(request_id) else {
            return Ok(None);
        };
        serde_json::to_vec(&record).map(Some)
    }

    /// 以记录为单位遍历当前捕获快照。调用方可以在每条记录后把字节写入
    /// 背压流，因此不会把整个 512 MiB 捕获再次聚合到一个 `Vec`。锁会在
    /// 导出期间保持，以确保新增/淘汰不会让同一次导出的记录顺序漂移；
    /// 导出是显式的低频操作，捕获写入只会短暂等待客户端背压。
    pub fn with_diagnostic_capture_records<F>(&self, mut visit: F) -> Result<(), String>
    where
        F: FnMut(&DiagnosticRequestCapture) -> Result<(), String>,
    {
        let capture = self
            .inner
            .capture
            .lock()
            .map_err(|_| "诊断捕获锁不可用".to_string())?;
        for record in &capture.records {
            visit(record)?;
        }
        Ok(())
    }

    /// 打开最近一次原子落盘的固定快照文件供 Admin 流式下载。
    /// 导出不主动触发一次近容量上限的 `serde_json::to_vec_pretty`，避免用户点击
    /// 下载时再次制造大内存峰值；后台持久化会持续更新这个文件。不接受外部路径，
    /// 避免把诊断导出变成任意文件读取端点。
    pub fn diagnostic_capture_export_file(&self) -> Result<std::fs::File, String> {
        if !self.inner.capture_writable.load(Ordering::Acquire) {
            return Err("诊断捕获快照加载失败，拒绝导出不可验证的原文件".into());
        }
        let path = self
            .inner
            .dir
            .as_ref()
            .map(|dir| dir.root.join("diagnostic_capture.json"))
            .ok_or_else(|| String::from("当前运行没有可导出的持久化配置目录"))?;
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| format!("诊断捕获快照暂时不可读取: {error}"))?;
        if !metadata.file_type().is_file() {
            return Err("诊断捕获快照不是普通文件，拒绝导出".into());
        }
        let file =
            std::fs::File::open(&path).map_err(|error| format!("诊断捕获快照无法打开: {error}"))?;
        let opened_metadata = file
            .metadata()
            .map_err(|error| format!("诊断捕获快照无法检查: {error}"))?;
        if !opened_metadata.is_file() {
            return Err("诊断捕获快照不是普通文件，拒绝导出".into());
        }
        self.inner
            .platform
            .validate_opened_capture(&metadata, &opened_metadata)?;
        Ok(file)
    }

    pub fn set_diagnostic_capture(&self, enabled: bool, max_bytes: Option<usize>) {
        let mut capture = self.inner.capture.lock().unwrap();
        if enabled {
            if let Some(max_bytes) = max_bytes {
                capture.max_bytes = max_bytes.max(1);
            }
            capture.limit_reached = false;
            capture.stop_reason = None;
            capture.started_at = Some(now_unix());
            capture.enabled = capture.captured_bytes < capture.max_bytes;
            if !capture.enabled {
                capture.limit_reached = true;
                capture.stop_reason = Some(CAPTURE_STOP_CAPACITY.into());
            }
        } else {
            capture.enabled = false;
            capture.stop_reason = Some(CAPTURE_STOP_MANUAL.into());
            capture.attempt_started.clear();
        }
        self.sync_capture_index_status(&capture);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub fn clear_diagnostic_capture(&self) -> Result<(), String> {
        // Serialize explicit deletion with the background writer. A malformed
        // snapshot is protected from implicit overwrite at startup, but an
        // explicit DELETE is allowed to discard it and establish a valid empty
        // snapshot for the next daemon start.
        let _flush = self.inner.capture_flush.lock().unwrap();
        if !self.inner.capture_writable.load(Ordering::Acquire)
            && let Some(dir) = &self.inner.dir
        {
            dir.remove_diagnostic_capture()
                .map_err(|error| format!("清理损坏的 diagnostic_capture.json 失败: {error}"))?;
            self.inner.capture_writable.store(true, Ordering::Release);
        }
        let snapshot = {
            let mut capture = self.inner.capture.lock().unwrap();
            capture.records.clear();
            capture.attempt_started.clear();
            capture.captured_bytes = 0;
            capture.limit_reached = false;
            capture.stop_reason = None;
            if !capture.enabled {
                capture.started_at = None;
            }
            // Clear before cloning. Requests arriving after this lock is
            // released set dirty again, so their updates are not lost.
            self.inner.capture_dirty.store(false, Ordering::Release);
            capture_snapshot(&capture)
        };
        // The full snapshot above is intentionally retained for persistence;
        // the UI index can be updated independently without another body copy.
        let capture = self.inner.capture.lock().unwrap();
        self.sync_capture_index_window(&capture);
        drop(capture);
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        match dir.save_diagnostic_capture(&snapshot) {
            Ok(outcome) => match outcome.durability_warning() {
                Some(warning) => {
                    self.inner.capture_dirty.store(true, Ordering::Release);
                    Err(warning)
                }
                None => Ok(()),
            },
            Err(error) => {
                self.inner.capture_dirty.store(true, Ordering::Release);
                Err(format!("diagnostic_capture.json 落盘失败: {error}"))
            }
        }
    }

    pub(super) fn capture_start(
        &self,
        request_id: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: &[u8],
        meta: &ClientMeta,
    ) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        if let Some(index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        {
            let removed = capture.records.remove(index);
            capture.captured_bytes = capture.captured_bytes.saturating_sub(record_size(&removed));
        }
        let base_bytes = request_id.len()
            + method.len()
            + path.len()
            + meta.client_model.len()
            + meta.effective_model.len()
            + meta.feature_rule_id.as_ref().map_or(0, String::len);
        let available = capture.max_bytes.saturating_sub(capture.captured_bytes);
        if base_bytes > available {
            refresh_capture_usage(&mut capture, true);
            self.sync_capture_index_window(&capture);
            self.inner.capture_dirty.store(true, Ordering::Release);
            return;
        }
        let remaining = available - base_bytes;
        let (inbound_headers, headers_truncated) = diagnostic_headers(headers, remaining);
        let header_bytes = inbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>();
        let (inbound_body, body_truncated) =
            diagnostic_text(body, remaining.saturating_sub(header_bytes));
        let truncated = headers_truncated || body_truncated;
        let record = DiagnosticRequestCapture {
            request_id: request_id.into(),
            timestamp: now_unix(),
            method: method.into(),
            path: path.into(),
            inbound_headers,
            inbound_body,
            inbound_body_bytes: body.len() as u64,
            inbound_body_truncated: body_truncated,
            client_kind: meta.client_kind,
            request_purpose: meta.purpose,
            client_model: meta.client_model.clone(),
            effective_model: meta.effective_model.clone(),
            feature_rule_id: meta.feature_rule_id.clone(),
            client_declared: meta.client_declared.clone(),
            source_format: Some(meta.source_format),
            target_format: meta.target_format,
            route_mode: meta.route_mode,
            attempts: Vec::new(),
            client_chunks: Vec::new(),
            completed_at_ms: None,
            status_code: None,
            outcome: None,
            failure_kind: None,
            failure_detail: None,
            truncated,
        };
        capture.captured_bytes = capture.captured_bytes.saturating_add(record_size(&record));
        capture.records.insert(0, record);
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_window(&capture);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub(super) fn capture_attempt_started(
        &self,
        request_id: &str,
        endpoint: &PlannedEndpoint,
        request: &crate::outbound::OutboundRequest,
        started: Instant,
    ) -> String {
        let id = new_event_id();
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return id;
        }
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return id;
        };
        let started_at_ms =
            ((now_unix() - capture.records[record_index].timestamp) * 1000.0).max(0.0) as i64;
        let protocol = protocol_token(endpoint.protocol).to_string();
        let base_bytes = id.len()
            + endpoint.endpoint_id.len()
            + endpoint.endpoint_name.len()
            + protocol.len()
            + request.method.len();
        let available = capture.max_bytes.saturating_sub(capture.captured_bytes);
        if base_bytes > available {
            capture.records[record_index].truncated = true;
            refresh_capture_usage(&mut capture, true);
            self.sync_capture_index_record(&capture, request_id);
            self.inner.capture_dirty.store(true, Ordering::Release);
            return id;
        }
        let remaining = available - base_bytes;
        let outbound_url_raw = format!("{}{}", request.base_url, request.path_and_query);
        let (outbound_url, url_truncated) = diagnostic_text(outbound_url_raw.as_bytes(), remaining);
        let remaining = remaining.saturating_sub(outbound_url.len());
        let (outbound_headers, headers_truncated) = diagnostic_headers(&request.headers, remaining);
        let header_bytes = outbound_headers
            .iter()
            .map(|header| header.name.len() + header.value.len())
            .sum::<usize>();
        let (outbound_body, body_truncated) =
            diagnostic_text(&request.body, remaining.saturating_sub(header_bytes));
        let truncated = url_truncated || headers_truncated || body_truncated;
        let attempt = DiagnosticAttemptCapture {
            id: id.clone(),
            endpoint_id: endpoint.endpoint_id.clone(),
            endpoint_name: endpoint.endpoint_name.clone(),
            protocol,
            source_format: Some(endpoint.source_format),
            target_format: Some(endpoint.protocol),
            route_mode: Some(endpoint.route_mode),
            pinned_ip: None,
            started_at_ms,
            outbound_method: request.method.clone(),
            outbound_url,
            outbound_headers,
            outbound_body,
            outbound_body_bytes: request.body.len() as u64,
            outbound_body_truncated: body_truncated,
            response_status: None,
            response_headers: Vec::new(),
            upstream_chunks: Vec::new(),
            error: None,
            completed_at_ms: None,
        };
        capture.captured_bytes = capture
            .captured_bytes
            .saturating_add(diagnostic_attempt_size(&attempt));
        let record = &mut capture.records[record_index];
        record.truncated |= truncated;
        record.attempts.push(attempt);
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
        if capture.enabled {
            capture.attempt_started.insert(id.clone(), started);
        }
        id
    }

    pub(super) fn capture_attempt_result(
        &self,
        request_id: &str,
        attempt_id: &str,
        result: &Result<crate::outbound::UpstreamResponse, TransportError>,
    ) {
        let mut capture = self.inner.capture.lock().unwrap();
        let elapsed_ms = capture
            .attempt_started
            .remove(attempt_id)
            .map(|started| started.elapsed().as_millis().min(i64::MAX as u128) as i64);
        let (response_status, response_headers, error_text, truncated) = if capture.enabled {
            let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
            match result {
                Ok(response) => {
                    let (headers, truncated) = diagnostic_headers(&response.headers, remaining);
                    (Some(response.status), Some(headers), None, truncated)
                }
                Err(error) => {
                    let (text, truncated) =
                        diagnostic_text(error.to_string().as_bytes(), remaining);
                    (None, None, Some(text), truncated)
                }
            }
        } else {
            (
                result.as_ref().ok().map(|response| response.status),
                None,
                None,
                false,
            )
        };
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let Some(attempt_index) = capture.records[record_index]
            .attempts
            .iter()
            .position(|attempt| attempt.id == attempt_id)
        else {
            return;
        };
        capture.records[record_index].truncated |= truncated;
        let (old_response_size, new_response_size) = {
            let attempt = &mut capture.records[record_index].attempts[attempt_index];
            let old_response_size = diagnostic_attempt_response_size(attempt);
            attempt.completed_at_ms = elapsed_ms.map(|elapsed| attempt.started_at_ms + elapsed);
            if let Some(status) = response_status {
                attempt.response_status = Some(status);
                if let Some(headers) = response_headers {
                    attempt.response_headers = headers;
                }
            } else {
                attempt.error = error_text;
            }
            (old_response_size, diagnostic_attempt_response_size(attempt))
        };
        if new_response_size >= old_response_size {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_add(new_response_size - old_response_size);
        } else {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_sub(old_response_size - new_response_size);
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub(super) fn capture_upstream_chunk(
        &self,
        request_id: &str,
        attempt_id: &str,
        bytes: &[u8],
        at_ms: i64,
    ) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let Some(attempt_index) = capture.records[record_index]
            .attempts
            .iter()
            .position(|attempt| attempt.id == attempt_id)
        else {
            return;
        };
        let (data, truncated) = diagnostic_text(bytes, remaining);
        capture.records[record_index].truncated |= truncated;
        if !data.is_empty() {
            capture.captured_bytes = capture.captured_bytes.saturating_add(data.len());
            capture.records[record_index].attempts[attempt_index]
                .upstream_chunks
                .push(DiagnosticChunk {
                    at_ms,
                    bytes: bytes.len() as u64,
                    data,
                    truncated,
                });
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub(super) fn capture_client_chunk(&self, request_id: &str, bytes: &[u8], at_ms: i64) {
        let mut capture = self.inner.capture.lock().unwrap();
        if !capture.enabled {
            return;
        }
        let remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let (data, truncated) = diagnostic_text(bytes, remaining);
        capture.records[record_index].truncated |= truncated;
        if !data.is_empty() {
            capture.captured_bytes = capture.captured_bytes.saturating_add(data.len());
            capture.records[record_index]
                .client_chunks
                .push(DiagnosticChunk {
                    at_ms,
                    bytes: bytes.len() as u64,
                    data,
                    truncated,
                });
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub(super) fn capture_finish(&self, event: &RuntimeEvent) {
        let mut capture = self.inner.capture.lock().unwrap();
        let request_id = event.request_id.as_deref().unwrap_or_default();
        let capture_enabled = capture.enabled;
        let mut remaining = capture.max_bytes.saturating_sub(capture.captured_bytes);
        let (failure_detail, mut truncated) = match event.failure_detail.as_deref() {
            _ if !capture_enabled => (None, false),
            Some(detail) => {
                let (captured, truncated) = diagnostic_text(detail.as_bytes(), remaining);
                remaining = remaining.saturating_sub(captured.len());
                (Some(captured), truncated)
            }
            None => (None, false),
        };
        let Some(record_index) = capture
            .records
            .iter()
            .position(|record| record.request_id == request_id)
        else {
            return;
        };
        let mut size_delta: isize = 0;
        let attempt_ids = {
            let record = &mut capture.records[record_index];
            let old_failure_detail_size = record.failure_detail.as_ref().map_or(0, String::len);
            let new_failure_detail_size = failure_detail.as_ref().map_or(0, String::len);
            size_delta += new_failure_detail_size as isize - old_failure_detail_size as isize;
            record.completed_at_ms = Some(event.duration_ms);
            record.status_code = Some(event.status_code);
            record.outcome = event.outcome;
            record.failure_kind = event.failure_kind;
            record.failure_detail = failure_detail;
            for attempt in &mut record.attempts {
                if attempt.completed_at_ms.is_none() {
                    let old_response_size = diagnostic_attempt_response_size(attempt);
                    attempt.completed_at_ms = Some(event.duration_ms);
                    if attempt.error.is_none() {
                        let (error, error_truncated) = if capture_enabled {
                            diagnostic_text(b"attempt cancelled before response headers", remaining)
                        } else {
                            (String::new(), false)
                        };
                        remaining = remaining.saturating_sub(error.len());
                        if !error.is_empty() {
                            attempt.error = Some(error);
                        }
                        truncated |= error_truncated;
                    }
                    let new_response_size = diagnostic_attempt_response_size(attempt);
                    size_delta += new_response_size as isize - old_response_size as isize;
                }
            }
            record.truncated |= truncated;
            record
                .attempts
                .iter()
                .map(|attempt| attempt.id.clone())
                .collect::<Vec<_>>()
        };
        if size_delta >= 0 {
            capture.captured_bytes = capture.captured_bytes.saturating_add(size_delta as usize);
        } else {
            capture.captured_bytes = capture
                .captured_bytes
                .saturating_sub((-size_delta) as usize);
        }
        for attempt_id in attempt_ids {
            capture.attempt_started.remove(&attempt_id);
        }
        refresh_capture_usage(&mut capture, truncated);
        self.sync_capture_index_record(&capture, request_id);
        self.inner.capture_dirty.store(true, Ordering::Release);
    }

    pub fn flush_diagnostic_capture_if_dirty(&self) {
        if !self.inner.capture_writable.load(Ordering::Acquire)
            || !self.inner.capture_dirty.swap(false, Ordering::AcqRel)
        {
            return;
        }
        if let Err(error) = self.flush_diagnostic_capture() {
            self.inner.capture_dirty.store(true, Ordering::Release);
            tracing::warn!("诊断捕获落盘失败: {error}");
        }
    }

    pub fn flush_diagnostic_capture(&self) -> Result<(), String> {
        if !self.inner.capture_writable.load(Ordering::Acquire) {
            return Err("diagnostic_capture.json 加载失败，本次运行拒绝覆盖原文件".into());
        }
        let Some(dir) = &self.inner.dir else {
            return Ok(());
        };
        let _flush = self.inner.capture_flush.lock().unwrap();
        let snapshot = {
            let capture = self.inner.capture.lock().unwrap();
            capture_snapshot(&capture)
        };
        match dir.save_diagnostic_capture(&snapshot) {
            Ok(outcome) => match outcome.durability_warning() {
                Some(warning) => Err(warning),
                None => Ok(()),
            },
            Err(error) => Err(format!("diagnostic_capture.json 落盘失败: {error}")),
        }
    }
}
