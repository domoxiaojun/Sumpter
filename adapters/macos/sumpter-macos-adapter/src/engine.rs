//! macOS composition wrapper around the single shared Sumpter engine.
//!
//! The wrapper injects the macOS control boundary; all request routing,
//! protocol translation, retries, and runtime persistence live in
//! `sumpter-engine`.

use std::ops::Deref;
use std::sync::Arc;
use std::sync::RwLock;

use sumpter_core::config::AppConfig;
use sumpter_core::config_store::{ConfigDir, MigrationNotice};
use sumpter_core::events::{RuntimeEvent, RuntimeSnapshot};
use sumpter_engine::boundary::{EngineCapabilities, EngineServices, PlatformNotice};
use sumpter_engine::outbound::UpstreamTransport;
use sumpter_engine::{ConfigReplacement, PlatformBoundary};

pub use sumpter_engine::engine::{error_response, json_bytes_response, json_response};
pub use sumpter_engine::{EngineNotice, MAX_BODY_BYTES};

#[derive(Clone)]
pub struct Engine {
    inner: sumpter_engine::Engine,
    control_token: String,
    migration_notice: Arc<RwLock<Option<MigrationNotice>>>,
}

impl Engine {
    pub fn new(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
        control_token: String,
    ) -> Self {
        Self::with_control_token(config, dir, transport, control_token)
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
            control_token: String::new(),
            migration_notice: Arc::new(RwLock::new(None)),
        }
    }

    /// Compatibility constructor used by the macOS control facade while the
    /// platform token remains an adapter concern.
    pub fn with_control_token(
        config: AppConfig,
        dir: Option<ConfigDir>,
        transport: Arc<dyn UpstreamTransport>,
        control_token: String,
    ) -> Self {
        Self {
            inner: sumpter_engine::Engine::new_with_services(
                config,
                dir,
                EngineServices {
                    transport,
                    platform: Arc::new(super::platform::Platform::new(control_token.clone())),
                },
            ),
            control_token,
            migration_notice: Arc::new(RwLock::new(None)),
        }
    }

    pub fn control_token(&self) -> &str {
        &self.control_token
    }

    pub fn migration_notice(&self) -> Option<MigrationNotice> {
        self.migration_notice.read().unwrap().clone()
    }

    pub fn publish_migration_notice(&self, notice: MigrationNotice) {
        *self.migration_notice.write().unwrap() = Some(notice);
    }

    pub fn reload_config(&self) -> Result<(String, Vec<String>), String> {
        self.inner
            .reload_config()
            .map(|replacement| (replacement.generation, replacement.warnings))
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
        self.inner
            .handle_request(remote, method, path_and_query, headers, body)
            .await
    }

    pub async fn handle_inbound_request(
        &self,
        request: sumpter_engine::InboundRequest,
    ) -> axum::response::Response {
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
