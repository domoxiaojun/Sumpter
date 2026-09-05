use super::{AdminState, JsonPayload, api_error, autostart_status, json_ok, require_json};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Response;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct AutostartBody {
    pub(crate) enabled: bool,
}

pub(crate) async fn get_autostart(State(state): State<AdminState>) -> Response {
    json_ok(&autostart_status(state.inner.systemd_scope).await)
}

pub(crate) async fn put_autostart(
    State(state): State<AdminState>,
    payload: JsonPayload<AutostartBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !state.inner.systemd_scope.controllable() {
        return api_error(
            StatusCode::FORBIDDEN,
            "systemd_system_root_required",
            "系统服务由 root 管理，请使用 sudo systemctl enable 或 disable sumpter.service",
        );
    }
    let action = if body.enabled { "enable" } else { "disable" };
    let output = tokio::process::Command::new("systemctl")
        .args(state.inner.systemd_scope.systemctl_args(action))
        .output()
        .await;
    match output {
        Ok(output) if output.status.success() => {
            json_ok(&autostart_status(state.inner.systemd_scope).await)
        }
        Ok(output) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "systemd_failed",
            &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "systemd_unavailable",
            &error.to_string(),
        ),
    }
}
