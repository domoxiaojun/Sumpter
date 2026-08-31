//! Linux composition wrapper around the single shared Sumpter engine.
//!
//! This module intentionally contains no proxy logic. It only injects the
//! Linux platform boundary and preserves the adapter crate's historical API
//! while callers migrate to `sumpter_engine::Engine` directly.

use std::ops::Deref;
use std::sync::Arc;

use sumpter_core::config::AppConfig;
use sumpter_core::config_store::ConfigDir;
use sumpter_core::events::{RuntimeEvent, RuntimeSnapshot};
use sumpter_engine::boundary::{EngineCapabilities, EngineServices, PlatformNotice};
use sumpter_engine::outbound::UpstreamTransport;
use sumpter_engine::{ConfigReplacement, PlatformBoundary};

pub use sumpter_engine::engine::{error_response, json_bytes_response, json_response};
pub use sumpter_engine::{EngineNotice, MAX_BODY_BYTES};

#[derive(Clone)]
pub struct Engine {
    inner: sumpter_engine::Engine,
}

impl Engine {
    pub fn new(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
    ) -> Self {
        Self::with_platform(config, dir, transport, Arc::new(super::platform::Platform))
    }

    pub fn with_platform(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
        platform: Arc<dyn PlatformBoundary>,
    ) -> Self {
        Self {
            inner: sumpter_engine::Engine::new_with_services(
                config,
                dir,
                EngineServices {
                    transport,
                    platform,
                },
            ),
        }
    }

    pub fn shared(&self) -> &sumpter_engine::Engine {
        &self.inner
    }

    pub async fn handle_request(
        &self,
        remote: Option<std::net::IpAddr>,
        method: &str,
        path_and_query: &str,
        headers: Vec<(String, String)>,
        body: impl Into<axum::body::Body>,
    ) -> axum::response::Response {
        // Linux deliberately has no hook notification endpoint. Preserve the
        // adapter contract before the shared engine performs auth/body work.
        if path_and_query
            .split_once('?')
            .map_or(path_and_query, |(path, _)| path)
            == "/__notify"
        {
            return error_response(axum::http::StatusCode::NOT_FOUND, &[("error", "not_found")]);
        }
        self.inner
            .handle_request(remote, method, path_and_query, headers, body)
            .await
    }

    pub async fn handle_inbound_request(
        &self,
        request: sumpter_engine::InboundRequest,
    ) -> axum::response::Response {
        if request.uri.path() == "/__notify" {
            return error_response(axum::http::StatusCode::NOT_FOUND, &[("error", "not_found")]);
        }
        self.inner.handle_inbound_request(request).await
    }
}

impl Deref for Engine {
    type Target = sumpter_engine::Engine;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl EngineCapabilities for Engine {
    fn runtime_snapshot(&self) -> RuntimeSnapshot {
        self.inner.runtime_snapshot()
    }

    fn replace_config(&self, config: AppConfig) -> Result<ConfigReplacement, String> {
        EngineCapabilities::replace_config(&self.inner, config)
    }

    fn reload_config(&self) -> Result<ConfigReplacement, String> {
        EngineCapabilities::reload_config(&self.inner)
    }

    fn record_platform_event(&self, event: RuntimeEvent) {
        EngineCapabilities::record_platform_event(&self.inner, event)
    }

    fn publish_platform_notice(&self, notice: PlatformNotice) {
        EngineCapabilities::publish_platform_notice(&self.inner, notice)
    }
}
