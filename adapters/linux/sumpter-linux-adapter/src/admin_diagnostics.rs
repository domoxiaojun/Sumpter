use super::{AdminState, JsonPayload, api_error, json_ok, require_json};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;
use serde_json::json;

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
        "proxyListener": {"host": config.listener.host, "port": config.listener.port, "httpsPort": config.listener.https_port()},
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
use sumpter_engine::diagnostic_export::DiagnosticCaptureExportQuery;
#[cfg(test)]
pub(super) use sumpter_engine::diagnostic_export::{
    DiagnosticCaptureFormat, parse_diagnostic_capture_format,
};
#[cfg(test)]
pub(super) use sumpter_engine::diagnostic_redaction::{
    redact_body_text, redact_capture_value, redact_url_query,
};

#[cfg(test)]
#[path = "../../../../tests/contracts/diagnostic_redaction.rs"]
mod diagnostic_redaction_tests;

#[cfg(test)]
fn diagnostic_test_state(dir: sumpter_core::config_store::ConfigDir) -> AdminState {
    let engine = crate::engine::Engine::new(
        sumpter_core::config::AppConfig::bootstrap().normalized(),
        Some(dir.clone()),
        std::sync::Arc::new(crate::outbound::ReqwestTransport::new()),
    );
    AdminState::new(
        engine.clone(),
        super::ProxySupervisor::new(engine),
        dir,
        None,
        super::AdminListen::default(),
        super::AdminAuth::password(b"synthetic-test-password").unwrap(),
    )
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
    sumpter_engine::diagnostic_export::export(state.inner.engine.shared().clone(), query).await
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
