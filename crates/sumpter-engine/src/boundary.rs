//! Platform boundary contracts.  The common engine depends on these traits,
//! never on a concrete Linux or macOS control implementation.

use std::net::IpAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use sumpter_core::config::AppConfig;
use sumpter_core::config_store::MigrationNotice;
use sumpter_core::events::{RuntimeEvent, RuntimeSnapshot};

use crate::outbound::UpstreamTransport;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessDecision {
    Allow,
    Deny,
}

/// Platform-neutral fallback used by library callers that do not expose
/// platform control endpoints. Product binaries should inject their concrete
/// adapter with `Engine::new_with_services`.
#[derive(Debug, Default)]
pub struct NoopPlatform;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformAction {
    Notify,
    Reload,
}

pub struct ControlRequestMeta<'a> {
    pub remote_ip: Option<IpAddr>,
    pub query: Option<&'a str>,
    pub headers: &'a [(String, String)],
}

pub struct PlatformRequest {
    pub method: String,
    pub path_and_query: String,
    pub headers: Vec<(String, String)>,
    pub remote_ip: Option<IpAddr>,
    pub body: Body,
}

/// Owned request envelope crossing the HTTP/server boundary.
///
/// The body is intentionally kept as an opaque stream.  The shared engine
/// performs CIDR and endpoint authentication checks before any handler turns
/// it into bytes, so rejected requests cannot force an allocation of their
/// payload.
pub struct InboundRequest {
    pub method: Method,
    pub uri: Uri,
    pub headers: HeaderMap,
    pub remote_ip: Option<IpAddr>,
    pub body: Body,
}

#[derive(Debug, Clone)]
pub enum PlatformNotice {
    Notify {
        client_kind: sumpter_core::events::ClientKind,
        kind: String,
        title: String,
        message: String,
        sound: Option<String>,
        category: Option<String>,
        priority: Option<String>,
        action_id: Option<String>,
        session_id: Option<String>,
        cwd: Option<String>,
    },
    Migration(MigrationNotice),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigReplacement {
    pub generation: String,
    pub warnings: Vec<String>,
}

/// Narrow capability view handed to a platform control handler.
pub trait EngineCapabilities: Send + Sync {
    fn runtime_snapshot(&self) -> RuntimeSnapshot;
    fn replace_config(&self, config: AppConfig) -> Result<ConfigReplacement, String>;
    /// Reloading is optional for embedders that do not own a configuration
    /// directory.  The daemon implementation overrides it; a platform
    /// action can therefore return a precise `reload_not_supported` error
    /// without gaining access to filesystem details.
    fn reload_config(&self) -> Result<ConfigReplacement, String> {
        Err("reload_not_supported".into())
    }
    fn record_platform_event(&self, event: RuntimeEvent);
    fn publish_platform_notice(&self, notice: PlatformNotice);
}

pub struct EngineServices {
    pub transport: Arc<dyn UpstreamTransport>,
    pub platform: Arc<dyn PlatformBoundary>,
}

#[async_trait]
pub trait PlatformBoundary: Send + Sync {
    fn authorize_status(&self, request: &ControlRequestMeta<'_>) -> AccessDecision;

    /// Stable machine-readable error token for a denied `/__status` request.
    /// Linux keeps its historical `loopback_only` response while macOS uses
    /// the control-token wording.  Embedders can retain the generic default.
    fn status_denied_error(&self) -> &'static str {
        "forbidden: bad/missing token"
    }

    fn platform_action(&self, method: &str, path: &str) -> Option<PlatformAction>;

    /// Validate that an opened diagnostic snapshot still refers to the path
    /// selected by the caller. Platform adapters may provide OS-specific
    /// identity checks; embedders without file exports can keep the default.
    fn validate_opened_capture(
        &self,
        _path_metadata: &std::fs::Metadata,
        _opened_metadata: &std::fs::Metadata,
    ) -> Result<(), String> {
        Ok(())
    }

    async fn handle_platform_action(
        &self,
        action: PlatformAction,
        request: PlatformRequest,
        engine: Arc<dyn EngineCapabilities>,
    ) -> Response;
}

#[async_trait]
impl PlatformBoundary for NoopPlatform {
    fn authorize_status(&self, _request: &ControlRequestMeta<'_>) -> AccessDecision {
        AccessDecision::Deny
    }

    fn platform_action(&self, _method: &str, _path: &str) -> Option<PlatformAction> {
        None
    }

    async fn handle_platform_action(
        &self,
        _action: PlatformAction,
        _request: PlatformRequest,
        _engine: Arc<dyn EngineCapabilities>,
    ) -> Response {
        Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap_or_else(|_| Response::new(Body::empty()))
    }
}
