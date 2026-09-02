//! Linux composition wrapper around the single shared Sumpter engine.
//!
//! This module intentionally contains no forwarding or routing logic. It
//! injects the Linux platform boundary, serves the Linux-only built-in helper,
//! and preserves the adapter crate's historical API while callers migrate to
//! `sumpter_engine::Engine` directly.

use std::ops::Deref;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::Response;
use sumpter_core::access;
use sumpter_core::config::AppConfig;
use sumpter_core::config_store::ConfigDir;
use sumpter_core::events::{RuntimeEvent, RuntimeSnapshot};
use sumpter_engine::boundary::{EngineCapabilities, EngineServices, PlatformNotice};
use sumpter_engine::outbound::UpstreamTransport;
use sumpter_engine::{ConfigReplacement, PlatformBoundary};

pub use sumpter_engine::engine::{error_response, json_bytes_response, json_response};
pub use sumpter_engine::{EngineNotice, MAX_BODY_BYTES};

/// Linux listener 上提供给远程 Claude Code 客户端的内置配置器路径。
pub const ATTRIBUTION_SCRIPT_PATH: &str = "/__sumpter/cc-project-attribution.sh";
const ATTRIBUTION_SCRIPT: &str =
    include_str!("../../../../platforms/linux/scripts/cc-project-attribution.sh");

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
        let body = body.into();
        let config = self.inner.config();
        if let Some(response) =
            attribution_script_response(remote, method, path_and_query, &headers, &config)
        {
            return response;
        }
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
        let path_and_query = request
            .uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or("/");
        let headers: Vec<(String, String)> = request
            .headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_string(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        let config = self.inner.config();
        if let Some(response) = attribution_script_response(
            request.remote_ip,
            request.method.as_str(),
            path_and_query,
            &headers,
            &config,
        ) {
            return response;
        }
        if request.uri.path() == "/__notify" {
            return error_response(axum::http::StatusCode::NOT_FOUND, &[("error", "not_found")]);
        }
        self.inner.handle_inbound_request(request).await
    }
}

fn attribution_script_response(
    remote: Option<std::net::IpAddr>,
    method: &str,
    path_and_query: &str,
    headers: &[(String, String)],
    config: &AppConfig,
) -> Option<Response> {
    let path = path_and_query
        .split_once('?')
        .map_or(path_and_query, |(path, _)| path);
    if path != ATTRIBUTION_SCRIPT_PATH {
        return None;
    }

    if !access::is_allowed(
        remote.map(|ip| ip.to_string()).as_deref(),
        &config.listener.allowed_cidrs,
    ) {
        return Some(error_response(
            StatusCode::FORBIDDEN,
            &[("error", "client_forbidden")],
        ));
    }

    if method != "GET" {
        return Some(
            Response::builder()
                .status(StatusCode::METHOD_NOT_ALLOWED)
                .header(header::ALLOW, "GET")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty())),
        );
    }

    if !config.listener.auth_token.is_empty()
        && !inbound_auth_ok(headers, &config.listener.auth_token)
    {
        return Some(
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(header::WWW_AUTHENTICATE, "Bearer")
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::empty())
                .unwrap_or_else(|_| Response::new(Body::empty())),
        );
    }

    Some(
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store")
            .header(
                header::CONTENT_DISPOSITION,
                "inline; filename=cc-project-attribution.sh",
            )
            .body(Body::from(ATTRIBUTION_SCRIPT))
            .unwrap_or_else(|_| Response::new(Body::from(ATTRIBUTION_SCRIPT))),
    )
}

fn inbound_auth_ok(headers: &[(String, String)], token: &str) -> bool {
    headers.iter().any(|(name, value)| {
        if name.eq_ignore_ascii_case("x-api-key") {
            return value == token;
        }
        if name.eq_ignore_ascii_case("authorization") {
            let value = value.trim();
            return value
                .get(..7)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("bearer "))
                && value[7..].trim() == token;
        }
        false
    })
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
