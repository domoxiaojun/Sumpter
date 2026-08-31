use std::sync::Arc;

use std::os::unix::fs::MetadataExt;

use async_trait::async_trait;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;
use sumpter_core::access;

use sumpter_engine::boundary::{
    AccessDecision, ControlRequestMeta, EngineCapabilities, PlatformAction, PlatformBoundary,
    PlatformRequest,
};

#[derive(Debug, Default)]
pub struct Platform;

pub fn validate_opened_capture(
    path_metadata: &std::fs::Metadata,
    opened_metadata: &std::fs::Metadata,
) -> Result<(), String> {
    if path_metadata.dev() != opened_metadata.dev() || path_metadata.ino() != opened_metadata.ino()
    {
        return Err("诊断捕获快照在打开前已被替换，拒绝导出".into());
    }
    Ok(())
}

#[async_trait]
impl PlatformBoundary for Platform {
    fn authorize_status(&self, request: &ControlRequestMeta<'_>) -> AccessDecision {
        if access::is_loopback(request.remote_ip.map(|ip| ip.to_string()).as_deref()) {
            AccessDecision::Allow
        } else {
            AccessDecision::Deny
        }
    }

    fn status_denied_error(&self) -> &'static str {
        "loopback_only"
    }

    fn platform_action(&self, _method: &str, _path: &str) -> Option<PlatformAction> {
        None
    }

    fn validate_opened_capture(
        &self,
        path_metadata: &std::fs::Metadata,
        opened_metadata: &std::fs::Metadata,
    ) -> Result<(), String> {
        validate_opened_capture(path_metadata, opened_metadata)
    }

    async fn handle_platform_action(
        &self,
        _action: PlatformAction,
        _request: PlatformRequest,
        _engine: Arc<dyn EngineCapabilities>,
    ) -> Response {
        sumpter_engine::engine::error_response(StatusCode::NOT_FOUND, &[("error", "not_found")])
    }
}

#[allow(dead_code)]
fn _contract_shape() -> serde_json::Value {
    json!({"platform": "linux", "control": false})
}
