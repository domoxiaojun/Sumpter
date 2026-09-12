//! Linux Admin API 与代理 listener 生命周期。
//!
//! Admin 默认绑定 `127.0.0.1:57879`，可通过 daemon CLI / 环境变量覆盖。
//! Web 静态资源公开加载，管理 API 与 SSE 使用内置登录会话 Cookie + CSRF。
//! 所有配置写入、reload 与 SIGHUP 共用一个串行事务；
//! Proxy listener 可独立启停或原子重绑。

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::services::ServeDir;

pub use crate::admin_auth::{AdminAuth, DEFAULT_ADMIN_USERNAME};
use crate::engine::{Engine, EngineNotice};
use crate::server;
use axum::extract::State;
use axum::http::StatusCode;
use sumpter_core::config::AppConfig;
use sumpter_core::config_store::{ConfigDir, MigrationNotice};

#[path = "admin_auth_routes.rs"]
mod auth_routes;
#[path = "admin_autostart.rs"]
mod autostart_routes;
#[path = "admin_config.rs"]
mod config_routes;
#[path = "admin_diagnostics.rs"]
mod diagnostics_routes;
#[path = "admin_runtime.rs"]
mod runtime_routes;

pub(crate) use auth_routes::{
    admin_guard, admin_security_headers, auth_login, auth_logout, auth_session,
    change_admin_credentials,
};
pub(crate) use autostart_routes::{get_autostart, put_autostart};
pub use config_routes::validate_config;
pub(crate) use config_routes::{
    PutConfigBody, apply_secrets, config_view, endpoint_secret, get_config, provider_models,
    put_config, reload, validate_config_identity,
};
#[cfg(test)]
use config_routes::{
    SecretUpdates, extract_models, model_catalog_paths, provider_model_auth_modes,
    provider_model_auth_sets, proxy_hint_for, summarize_probe_failure, valid_cidr,
};
#[cfg(test)]
use diagnostics_routes::{
    DiagnosticCaptureFormat, parse_diagnostic_capture_format, redact_capture_value,
};
pub(crate) use diagnostics_routes::{
    clear_diagnostic_capture, diagnostic_capture, diagnostic_capture_detail,
    diagnostic_capture_export, diagnostics, set_diagnostic_capture,
};
#[cfg(test)]
use runtime_routes::RuntimeEventsQuery;

pub(crate) use runtime_routes::{
    JsonPayload, clear_project_sticky, clear_runtime_session_sticky, delete_runtime_session,
    export_runtime_session, proxy_start, proxy_stop, recreate_runtime, require_json, reset_runtime,
    runtime_analytics, runtime_cleanup, runtime_cleanup_preview, runtime_dimensions,
    runtime_errors, runtime_event_detail, runtime_events, runtime_export, runtime_export_estimate,
    runtime_facets, runtime_pricing, runtime_pricing_update, runtime_projects,
    runtime_request_chain, runtime_retention, runtime_retention_update, runtime_sessions,
    runtime_storage, runtime_summary, runtime_trends, status,
};

pub const DEFAULT_ADMIN_HOST: &str = "127.0.0.1";
pub const DEFAULT_ADMIN_PORT: u16 = 57_879;
pub const SYSTEMD_UNIT: &str = "sumpter.service";
pub(crate) const MAX_MODEL_CATALOG_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_MODEL_CATALOG_ITEMS: usize = 5_000;

/// Query parameters arrive from `serde_urlencoded` as strings even when the
/// target type is numeric. Keep accepting the decimal form emitted by older
/// macOS clients (for example `809539200.0`) instead of relying on the
/// extractor's direct `f64` deserializer, which rejects that representation.
pub(crate) fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<String>::deserialize(deserializer)?;
    raw.map(|value| {
        let parsed = value.parse::<f64>().map_err(de::Error::custom)?;
        if parsed.is_finite() {
            Ok(parsed)
        } else {
            Err(de::Error::custom("必须是有限数字"))
        }
    })
    .transpose()
}

/// Admin/Web listener 启动绑定（不进 config.json，避免 SIGHUP 改管理口）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminListen {
    pub host: String,
    pub port: u16,
}

impl Default for AdminListen {
    fn default() -> Self {
        Self {
            host: DEFAULT_ADMIN_HOST.into(),
            port: DEFAULT_ADMIN_PORT,
        }
    }
}

impl AdminListen {
    pub fn new(host: impl Into<String>, port: u16) -> Result<Self, String> {
        if port == 0 {
            return Err("Admin 端口必须在 1...65535".into());
        }
        let host = host.into();
        // 预校验可绑定语义；失败时给出清晰错误而不是静默落到 127.0.0.1。
        parse_admin_socket(&host, port)?;
        Ok(Self { host, port })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        parse_admin_socket(&self.host, self.port).expect("AdminListen 构造时已校验 host/port")
    }

    pub fn is_loopback_bind(&self) -> bool {
        is_loopback_admin_host(&self.host)
    }

    pub fn is_all_interfaces(&self) -> bool {
        is_all_interfaces_host(&self.host)
    }
}

/// 解析 Admin 绑定地址（与 proxy `listener_address` 语义对齐）。
pub fn parse_admin_socket(host: &str, port: u16) -> Result<SocketAddr, String> {
    if port == 0 {
        return Err("Admin 端口必须在 1...65535".into());
    }
    let host = host.trim();
    let ip = match host {
        "" | "0.0.0.0" => IpAddr::from([0, 0, 0, 0]),
        value if value.eq_ignore_ascii_case("localhost") => IpAddr::from([127, 0, 0, 1]),
        "::" => IpAddr::from([0u16; 8]),
        value => value.parse::<IpAddr>().map_err(|_| {
            format!("Admin 监听地址必须是 IP 地址、localhost 或 0.0.0.0/::: {value}")
        })?,
    };
    Ok(SocketAddr::new(ip, port))
}

pub fn is_loopback_admin_host(host: &str) -> bool {
    let trimmed = host.trim().to_ascii_lowercase();
    trimmed == "127.0.0.1" || trimmed == "localhost" || trimmed == "::1" || trimmed == "[::1]"
}

pub fn is_all_interfaces_host(host: &str) -> bool {
    let trimmed = host.trim();
    trimmed.is_empty() || trimmed == "0.0.0.0" || trimmed == "::"
}

/// The unit scope is an explicit daemon launch contract. It must never be
/// inferred from the effective UID because a system service intentionally runs
/// the daemon as an unprivileged account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemdScope {
    User,
    System,
}

impl SystemdScope {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "user" => Ok(Self::User),
            "system" => Ok(Self::System),
            _ => Err("--systemd-scope 只能是 user 或 system".to_string()),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::System => "system",
        }
    }

    fn systemctl_args(self, action: &str) -> Vec<&str> {
        match self {
            Self::User => vec!["--user", action, SYSTEMD_UNIT],
            Self::System => vec![action, SYSTEMD_UNIT],
        }
    }

    const fn controllable(self) -> bool {
        matches!(self, Self::User)
    }

    const fn journalctl_command(self) -> &'static str {
        match self {
            Self::User => "journalctl --user -u sumpter.service -n 200 --no-pager",
            Self::System => "journalctl -u sumpter.service -n 200 --no-pager",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    pub running: bool,
    pub state: &'static str,
    pub host: Option<String>,
    pub port: Option<u16>,
}

pub(crate) struct ProxyRuntime {
    task: Option<server::ServerHandle>,
    local: Option<SocketAddr>,
}

#[derive(Clone)]
pub struct ProxySupervisor {
    engine: Engine,
    runtime: Arc<tokio::sync::Mutex<ProxyRuntime>>,
}

impl ProxySupervisor {
    pub fn new(engine: Engine) -> Self {
        Self {
            engine,
            runtime: Arc::new(tokio::sync::Mutex::new(ProxyRuntime {
                task: None,
                local: None,
            })),
        }
    }

    pub async fn status(&self) -> ProxyStatus {
        let mut runtime = self.runtime.lock().await;
        Self::discard_finished(&mut runtime);
        Self::snapshot(&runtime)
    }

    pub async fn start(&self) -> Result<ProxyStatus, String> {
        self.engine.ensure_runtime_ready()?;
        let mut runtime = self.runtime.lock().await;
        Self::discard_finished(&mut runtime);
        if runtime.task.is_some() {
            return Ok(Self::snapshot(&runtime));
        }
        let config = self.engine.config();
        let address = listener_address(&config)?;
        let (local, listener) = server::bind_listener(address)
            .await
            .map_err(|error| format!("proxy 监听失败 {address}: {error}"))?;
        let task = server::serve_bound_router(server::router(self.engine.clone()), listener);
        runtime.local = Some(local);
        runtime.task = Some(task);
        let status = Self::snapshot(&runtime);
        self.publish(&status);
        Ok(status)
    }

    pub async fn stop(&self) -> ProxyStatus {
        let mut runtime = self.runtime.lock().await;
        if let Some(task) = runtime.task.take() {
            task.shutdown().await;
        }
        runtime.local = None;
        let status = Self::snapshot(&runtime);
        self.publish(&status);
        status
    }

    /// 在旧 listener 仍接入时先 bind 新地址；只有 bind 成功才替换 Engine 并切换 task。
    pub async fn apply_config(
        &self,
        config: AppConfig,
    ) -> Result<(String, Vec<String>, ProxyStatus), String> {
        let desired = listener_address(&config)?;
        let mut runtime = self.runtime.lock().await;
        Self::discard_finished(&mut runtime);
        let needs_rebind = runtime.task.is_some() && runtime.local != Some(desired);
        let current = runtime.local;
        let overlaps_current =
            needs_rebind && current.is_some_and(|address| address.port() == desired.port());

        // 同端口 host 变化（例如 127.0.0.1 → 0.0.0.0）通常无法预绑定。
        // 先停旧 task 再 bind；失败时立即按旧地址恢复，Engine 此时仍是旧配置。
        if overlaps_current {
            if let Some(old_task) = runtime.task.take() {
                old_task.shutdown().await;
            }
            runtime.local = None;
            match server::bind_listener(desired).await {
                Ok((local, listener)) => {
                    let (generation, warnings) = self.engine.replace_config(config);
                    runtime.task = Some(server::serve_bound_router(
                        server::router(self.engine.clone()),
                        listener,
                    ));
                    runtime.local = Some(local);
                    let status = Self::snapshot(&runtime);
                    self.publish(&status);
                    return Ok((generation, warnings, status));
                }
                Err(error) => {
                    let restore = match current {
                        Some(old_address) => match server::bind_listener(old_address).await {
                            Ok((local, listener)) => {
                                runtime.task = Some(server::serve_bound_router(
                                    server::router(self.engine.clone()),
                                    listener,
                                ));
                                runtime.local = Some(local);
                                "旧 listener 已恢复".to_string()
                            }
                            Err(restore_error) => {
                                format!("旧 listener 恢复失败: {restore_error}")
                            }
                        },
                        None => "旧 listener 地址缺失".to_string(),
                    };
                    self.publish(&Self::snapshot(&runtime));
                    return Err(format!("新 proxy 监听失败 {desired}: {error}；{restore}"));
                }
            }
        }

        let prepared = if needs_rebind {
            Some(
                server::bind_listener(desired)
                    .await
                    .map_err(|error| format!("新 proxy 监听失败 {desired}: {error}"))?,
            )
        } else {
            None
        };

        let (generation, warnings) = self.engine.replace_config(config);
        if let Some((local, listener)) = prepared {
            let new_task =
                server::serve_bound_router(server::router(self.engine.clone()), listener);
            if let Some(old_task) = runtime.task.replace(new_task) {
                old_task.shutdown().await;
            }
            runtime.local = Some(local);
        }
        let status = Self::snapshot(&runtime);
        if needs_rebind {
            self.publish(&status);
        }
        Ok((generation, warnings, status))
    }

    fn discard_finished(runtime: &mut ProxyRuntime) {
        if runtime.task.as_ref().is_some_and(|task| task.is_finished()) {
            runtime.task = None;
            runtime.local = None;
        }
    }

    fn snapshot(runtime: &ProxyRuntime) -> ProxyStatus {
        let running = runtime.task.is_some();
        ProxyStatus {
            running,
            state: if running { "running" } else { "stopped" },
            host: runtime.local.map(|address| address.ip().to_string()),
            port: runtime.local.map(|address| address.port()),
        }
    }

    fn publish(&self, status: &ProxyStatus) {
        self.engine
            .publish_proxy_state(status.running, status.host.clone(), status.port);
    }
}

pub(crate) fn listener_address(config: &AppConfig) -> Result<SocketAddr, String> {
    let port = u16::try_from(config.listener.port)
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| "代理端口必须在 1...65535".to_string())?;
    let host = config.listener.host.trim();
    let ip = match host {
        "" | "0.0.0.0" => IpAddr::from([0, 0, 0, 0]),
        value if value.eq_ignore_ascii_case("localhost") => IpAddr::from([127, 0, 0, 1]),
        "::" => IpAddr::from([0u16; 8]),
        value => value
            .parse::<IpAddr>()
            .map_err(|_| format!("监听地址必须是 IP 地址: {value}"))?,
    };
    Ok(SocketAddr::new(ip, port))
}

pub(crate) struct AdminInner {
    engine: Engine,
    proxy: ProxySupervisor,
    config_dir: ConfigDir,
    transaction: tokio::sync::Mutex<()>,
    web_root: Option<PathBuf>,
    systemd_scope: SystemdScope,
    admin_listen: AdminListen,
    admin_auth: AdminAuth,
    migration_notice: std::sync::RwLock<Option<MigrationNotice>>,
    migration_notices: tokio::sync::broadcast::Sender<MigrationNotice>,
}

#[derive(Clone)]
pub struct AdminState {
    inner: Arc<AdminInner>,
}

impl AdminState {
    pub fn new(
        engine: Engine,
        proxy: ProxySupervisor,
        config_dir: ConfigDir,
        web_root: Option<PathBuf>,
        admin_listen: AdminListen,
        admin_auth: AdminAuth,
    ) -> Self {
        Self::with_systemd_scope_and_auth(
            engine,
            proxy,
            config_dir,
            web_root,
            SystemdScope::User,
            admin_listen,
            admin_auth,
        )
    }

    pub fn with_systemd_scope_and_auth(
        engine: Engine,
        proxy: ProxySupervisor,
        config_dir: ConfigDir,
        web_root: Option<PathBuf>,
        systemd_scope: SystemdScope,
        admin_listen: AdminListen,
        admin_auth: AdminAuth,
    ) -> Self {
        let (migration_notices, _) = tokio::sync::broadcast::channel(16);
        Self {
            inner: Arc::new(AdminInner {
                engine,
                proxy,
                config_dir,
                transaction: tokio::sync::Mutex::new(()),
                web_root,
                systemd_scope,
                admin_listen,
                admin_auth,
                migration_notice: std::sync::RwLock::new(None),
                migration_notices,
            }),
        }
    }

    pub fn admin_listen(&self) -> &AdminListen {
        &self.inner.admin_listen
    }

    pub fn engine(&self) -> Engine {
        self.inner.engine.clone()
    }

    pub fn proxy(&self) -> ProxySupervisor {
        self.inner.proxy.clone()
    }

    pub fn publish_migration_notice(&self, notice: MigrationNotice) {
        *self.inner.migration_notice.write().unwrap() = Some(notice.clone());
        let _ = self.inner.migration_notices.send(notice);
    }

    fn migration_notice(&self) -> Option<MigrationNotice> {
        self.inner.migration_notice.read().unwrap().clone()
    }

    /// Admin reload 与 SIGHUP 共用此入口。失败时旧 Engine/listener 不变。
    pub async fn reload_from_disk(&self) -> Result<ConfigApplyResult, String> {
        let _transaction = self.inner.transaction.lock().await;
        let loaded = self
            .inner
            .config_dir
            .load_config_with_notice()
            .map_err(|error| error.to_string())?;
        let raw_config = loaded.config;
        if let Some(notice) = loaded.migration_notice {
            self.publish_migration_notice(notice);
        }
        validate_config(&raw_config)?;
        let config = raw_config.clone().normalized();
        validate_config(&config)?;
        if config != raw_config {
            let _ = self
                .inner
                .config_dir
                .save_config(&config)
                .map_err(|error| format!("统一 Provider 配置迁移写盘失败: {error}"))?;
        }
        let (generation, warnings, proxy) = self.inner.proxy.apply_config(config).await?;
        self.inner.engine.set_last_error(None);
        Ok(ConfigApplyResult {
            generation,
            warnings,
            proxy,
        })
    }

    async fn put_config(&self, body: PutConfigBody) -> Result<Value, ApiFailure> {
        let _transaction = self.inner.transaction.lock().await;
        let old_config = self.inner.engine.config().as_ref().clone();
        let current_generation = self.inner.engine.generation();
        if body.expected_generation != current_generation {
            return Err(ApiFailure::new(
                StatusCode::CONFLICT,
                "generation_conflict",
                format!(
                    "配置已变化，expectedGeneration={}，currentGeneration={current_generation}",
                    body.expected_generation
                ),
            ));
        }

        validate_config_identity(&body.config).map_err(ApiFailure::bad_config)?;
        let mut config = body.config;
        apply_secrets(&old_config, &mut config, body.secret_updates)
            .map_err(ApiFailure::bad_config)?;
        validate_config(&config).map_err(ApiFailure::bad_config)?;
        config = config.normalized();
        validate_config(&config).map_err(ApiFailure::bad_config)?;

        let write_outcome = self
            .inner
            .config_dir
            .save_config(&config)
            .map_err(|error| ApiFailure::internal("config_write_failed", error.to_string()))?;
        let durability_warning = write_outcome.durability_warning();

        let apply_result = self.inner.proxy.apply_config(config).await;
        let (generation, mut warnings, proxy) = match apply_result {
            Ok(result) => result,
            Err(error) => {
                let rollback = self.inner.config_dir.save_config(&old_config);
                let message = match rollback {
                    Ok(outcome) => match outcome.durability_warning() {
                        Some(warning) => {
                            format!("{error}；磁盘配置已回滚，但 {warning}")
                        }
                        None => format!("{error}；磁盘配置已回滚"),
                    },
                    Err(rollback_error) => {
                        format!("{error}；磁盘回滚失败: {rollback_error}")
                    }
                };
                self.inner.engine.set_last_error(Some(message.clone()));
                return Err(ApiFailure::internal("listener_rebind_failed", message));
            }
        };
        if let Some(warning) = &durability_warning {
            warnings.push(warning.clone());
        }
        self.inner.engine.set_last_error(durability_warning.clone());
        Ok(config_view(
            &self.inner.engine.config(),
            &generation,
            Some(json!({"warnings": warnings, "proxy": proxy})),
        ))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigApplyResult {
    pub generation: String,
    pub warnings: Vec<String>,
    pub proxy: ProxyStatus,
}

pub fn admin_router(state: AdminState) -> Router {
    let protected_api = Router::new()
        .route("/status", get(status))
        .route("/proxy/start", post(proxy_start))
        .route("/proxy/stop", post(proxy_stop))
        .route("/runtime/summary", get(runtime_summary))
        .route("/runtime/events", get(runtime_events))
        .route("/runtime/events/{id}", get(runtime_event_detail))
        .route("/runtime/request-chain", get(runtime_request_chain))
        .route("/runtime/analytics", get(runtime_analytics))
        .route("/runtime/facets", get(runtime_facets))
        .route("/runtime/trends", get(runtime_trends))
        .route("/runtime/errors", get(runtime_errors))
        .route("/runtime/dimensions", get(runtime_dimensions))
        .route("/runtime/projects", get(runtime_projects))
        .route("/runtime/sessions", get(runtime_sessions))
        .route("/runtime/storage", get(runtime_storage))
        .route(
            "/runtime/retention",
            get(runtime_retention).put(runtime_retention_update),
        )
        .route(
            "/runtime/pricing",
            get(runtime_pricing).put(runtime_pricing_update),
        )
        .route("/runtime/export/estimate", get(runtime_export_estimate))
        .route("/runtime/export", get(runtime_export))
        .route(
            "/runtime/session",
            axum::routing::delete(delete_runtime_session),
        )
        .route("/runtime/session/export", get(export_runtime_session))
        .route("/runtime/projects/sticky-clear", post(clear_project_sticky))
        .route(
            "/runtime/sessions/sticky-clear",
            post(clear_runtime_session_sticky),
        )
        .route("/runtime/cleanup/preview", post(runtime_cleanup_preview))
        .route("/runtime/cleanup", post(runtime_cleanup))
        .route("/runtime/reset", post(reset_runtime))
        .route("/runtime/recreate", post(recreate_runtime))
        .route("/events", get(events))
        .route("/config", get(get_config).put(put_config))
        .route("/reload", post(reload))
        .route("/provider-models", post(provider_models))
        .route("/endpoint-secret", get(endpoint_secret))
        .route("/diagnostics", get(diagnostics))
        .route(
            "/diagnostic-capture",
            get(diagnostic_capture)
                .put(set_diagnostic_capture)
                .delete(clear_diagnostic_capture),
        )
        .route("/diagnostic-capture/export", get(diagnostic_capture_export))
        .route(
            "/diagnostic-capture/{request_id}",
            get(diagnostic_capture_detail),
        )
        .route("/autostart", get(get_autostart).put(put_autostart))
        .route("/auth/logout", post(auth_logout))
        .route(
            "/auth/credentials",
            axum::routing::put(change_admin_credentials),
        )
        .fallback(api_not_found)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            admin_guard,
        ));

    let api = Router::new()
        .route("/auth/session", get(auth_session))
        .route("/auth/login", post(auth_login))
        .merge(protected_api);

    let mut router = Router::new().nest("/admin/api", api);
    if let Some(web_root) = state.inner.web_root.clone() {
        router = router
            .route("/", get(|| async { Redirect::temporary("/admin/") }))
            .route("/admin", get(|| async { Redirect::temporary("/admin/") }))
            .nest_service(
                "/admin/",
                ServeDir::new(web_root).append_index_html_on_directories(true),
            );
    }
    router
        .layer(axum::extract::DefaultBodyLimit::max(4 * 1024 * 1024))
        .layer(axum::middleware::from_fn(admin_security_headers))
        .route("/healthz", get(|| async { StatusCode::NO_CONTENT }))
        .with_state(state)
}

pub(crate) async fn events(
    State(state): State<AdminState>,
) -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    struct Receivers {
        engine: tokio::sync::broadcast::Receiver<EngineNotice>,
        migrations: tokio::sync::broadcast::Receiver<MigrationNotice>,
    }
    let receiver = Receivers {
        engine: state.inner.engine.subscribe(),
        migrations: state.inner.migration_notices.subscribe(),
    };
    let inner = state.inner.clone();
    let stream = futures_util::stream::unfold(receiver, move |mut receivers| {
        let inner = inner.clone();
        async move {
            loop {
                tokio::select! {
                    result = receivers.engine.recv() => match result {
                        Ok(notice) => {
                            let (name, data) = match notice {
                                EngineNotice::Event(_) => continue,
                                EngineNotice::RuntimeChange { seq, change_seq, event } => (
                                    "runtime-change",
                                    json!({"seq": seq, "changeSeq": change_seq, "event": event}),
                                ),
                                EngineNotice::ConfigReloaded { generation } => {
                                    ("config-reloaded", json!({"generation": generation}))
                                }
                                EngineNotice::StatsReset => (
                                    "stats-reset",
                                    json!({"resetGeneration": inner.engine.runtime_summary_value()["resetGeneration"]}),
                                ),
                                EngineNotice::ProxyState {
                                    running,
                                    host,
                                    port,
                                } => (
                                    "proxy-state",
                                    json!({"running": running, "host": host, "port": port}),
                                ),
                                EngineNotice::PlatformNotice(_) => continue,
                            };
                            let event = SseEvent::default().event(name).data(data.to_string());
                            let event = if name == "runtime-change" {
                                event.id(data["changeSeq"].as_i64().unwrap_or_default().to_string())
                            } else {
                                event
                            };
                            return Some((Ok::<_, Infallible>(event), receivers));
                        }
                        // A lagged receiver has an unknown gap. Close the SSE
                        // stream so the client reconnect path can use
                        // afterChangeSeq and perform an explicit cursor check.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return None,
                        Err(_) => return None,
                    },
                    result = receivers.migrations.recv() => match result {
                        Ok(notice) => {
                            let data = serde_json::to_value(notice).unwrap_or(Value::Null);
                            let event = SseEvent::default()
                                .event("config-migrated")
                                .data(data.to_string());
                            return Some((Ok::<_, Infallible>(event), receivers));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return None,
                        Err(_) => return None,
                    },
                }
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

pub(crate) async fn autostart_status(scope: SystemdScope) -> Value {
    match tokio::process::Command::new("systemctl")
        .args(scope.systemctl_args("is-enabled"))
        .output()
        .await
    {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let enabled = matches!(text.as_str(), "enabled" | "enabled-runtime" | "linked");
            let available = output.status.success()
                || matches!(
                    text.as_str(),
                    "disabled" | "static" | "masked" | "indirect" | "generated" | "transient"
                );
            json!({
                "available": available,
                "enabled": enabled,
                "unit": SYSTEMD_UNIT,
                "scope": scope.as_str(),
                "controllable": scope.controllable(),
                "reason": if available && scope.controllable() {
                    Value::Null
                } else if scope.controllable() {
                    Value::String(String::from_utf8_lossy(&output.stderr).trim().to_string())
                } else {
                    Value::String("系统服务由 root 管理；请使用 sudo systemctl enable|disable sumpter.service".to_string())
                },
            })
        }
        Err(error) => json!({
            "available": false,
            "enabled": false,
            "unit": SYSTEMD_UNIT,
            "scope": scope.as_str(),
            "controllable": scope.controllable(),
            "reason": error.to_string(),
        }),
    }
}

pub(crate) async fn api_not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not_found", "unknown admin endpoint")
}

pub(crate) fn json_ok(body: &Value) -> Response {
    crate::engine::json_response(StatusCode::OK, body)
}

pub(crate) fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    crate::engine::json_response(status, &json!({"error": code, "message": message}))
}

pub(crate) struct ApiFailure {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiFailure {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn bad_config(message: String) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_config", message)
    }

    fn internal(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, code, message)
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        api_error(self.status, self.code, &self.message)
    }
}

#[cfg(test)]
use axum::extract::Query;
#[cfg(test)]
use axum::http::header;
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use sumpter_core::config::{Endpoint, SCHEMA_VERSION};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_view_never_contains_full_secrets() {
        let mut config = AppConfig::bootstrap();
        config.listener.auth_token = "testtest0000".into();
        let view = config_view(&config, "generation", None);
        let text = view.to_string();
        assert!(!text.contains("testtest0000"));
        assert!(text.contains("0000"));
    }

    #[test]
    fn config_view_exposes_only_nonsecret_migration_notice() {
        let config = AppConfig::bootstrap();
        let notice = MigrationNotice {
            id: "schema-v3-to-v5-1".into(),
            from_schema: 3,
            to_schema: SCHEMA_VERSION,
            backup_file: "config.before-schema-v5-1.json".into(),
            endpoint_count: 1,
            expanded_legacy_passthrough_endpoints: 1,
            converted_to_auto_endpoint_ids: vec!["provider-a".into()],
            removed_fields: vec!["listener.inboundDialectPassthrough".into()],
        };
        let view = config_view(
            &config,
            "generation",
            Some(json!({"migrationNotice": notice})),
        );

        assert_eq!(
            view["migrationNotice"]["convertedToAutoEndpointIds"],
            json!(["provider-a"])
        );
        assert!(view["migrationNotice"].get("apiKey").is_none());
    }

    #[test]
    fn cidr_validation_accepts_hosts_and_networks() {
        assert!(valid_cidr("127.0.0.1"));
        assert!(valid_cidr("10.0.0.0/8"));
        assert!(valid_cidr("::1/128"));
        assert!(!valid_cidr("10.0.0.0/99"));
        assert!(!valid_cidr("not-an-ip"));
    }

    #[test]
    fn model_catalog_shapes_are_supported() {
        let value = json!({"data": [{"id": "m1"}, {"model": "m2"}, "m1"]});
        assert_eq!(extract_models(&value), vec!["m1", "m2"]);

        let nested = json!({"result": {"data": {"models": {"m3": {}, "m4": {}}}}});
        assert_eq!(extract_models(&nested), vec!["m3", "m4"]);
    }

    #[test]
    fn model_catalog_paths_preserve_custom_base_and_deduplicate_v1() {
        let root = reqwest::Url::parse("https://example.com").unwrap();
        assert_eq!(
            model_catalog_paths(&root),
            vec!["/v1/models", "/models", "/v1/model/list", "/api/v1/models"]
        );
        let v1 = reqwest::Url::parse("https://example.com/v1/").unwrap();
        assert_eq!(
            model_catalog_paths(&v1),
            vec!["/v1/models", "/v1/model/list"]
        );
        let custom = reqwest::Url::parse("https://example.com/apps/anthropic").unwrap();
        assert_eq!(
            model_catalog_paths(&custom),
            vec![
                "/apps/anthropic/v1/models",
                "/apps/anthropic/models",
                "/apps/anthropic/v1/model/list"
            ]
        );
    }

    #[test]
    fn provider_model_auth_modes_allow_anonymous_endpoints() {
        assert_eq!(provider_model_auth_modes(" "), vec![None]);
        assert_eq!(
            provider_model_auth_modes("sk-test"),
            vec![
                Some(("x-api-key", "sk-test".to_string())),
                Some(("authorization", "Bearer sk-test".to_string())),
                Some(("authorization", "x-api-key sk-test".to_string())),
            ]
        );
        let sets = provider_model_auth_sets("sk-test");
        assert_eq!(
            sets[0],
            vec![
                ("authorization", "Bearer sk-test".to_string()),
                ("x-api-key", "sk-test".to_string())
            ]
        );
        assert_eq!(provider_model_auth_sets(" "), vec![Vec::new()]);
    }

    /// 代理提示:注入 `is_set` 而不是改进程环境 —— 改了会让并行跑的 admin 测试里
    /// `reqwest::Client::new()` 读到 `*_PROXY` 并试图通过假代理连本机 server。
    #[test]
    fn proxy_env_hint_names_variables_without_leaking_their_values() {
        assert!(
            proxy_hint_for(|_| false).is_none(),
            "没有任何代理变量时不应给提示"
        );

        let hint = proxy_hint_for(|name| name == "HTTPS_PROXY").expect("设置后应给出提示");
        assert!(hint.contains("HTTPS_PROXY"), "提示应点出变量名: {hint}");
        assert!(
            !hint.contains("proxy.internal.invalid") && !hint.contains("8080"),
            "不得回显代理地址(可能带内网信息或凭据): {hint}"
        );
        assert!(
            hint.contains("不走代理"),
            "提示要说清探测与转发口径一致: {hint}"
        );

        // 小写变量同样识别；多个同时设置时都要列出。
        let both = proxy_hint_for(|name| name == "all_proxy" || name == "http_proxy")
            .expect("小写变量也要识别");
        assert!(
            both.contains("http_proxy") && both.contains("all_proxy"),
            "{both}"
        );
    }

    /// 提示必须真的被拼进调用方看到的失败摘要 —— 只测 `proxy_hint_for` 本身不够。
    /// `summarize_probe_failure` 是 `fetch_provider_models_inner` 唯一的错误摘要构造点。
    #[test]
    fn probe_failure_summary_appends_proxy_hint_and_keeps_first_four_reasons() {
        let reasons: Vec<String> = (1..=6).map(|n| format!("/p{n} HTTP 500")).collect();

        let plain = summarize_probe_failure(reasons.clone(), None);
        assert!(plain.starts_with("/p1 HTTP 500"), "原因排在前面: {plain}");
        assert!(!plain.contains("/p5"), "仍应只保留前 4 条: {plain}");

        let hinted =
            summarize_probe_failure(reasons, Some("（检测到 HTTPS_PROXY：……不走代理……）".into()));
        assert!(
            hinted.starts_with("/p1 HTTP 500"),
            "原因仍排在前面: {hinted}"
        );
        assert!(hinted.contains("不走代理"), "提示要被拼上: {hinted}");
        assert!(!hinted.contains("/p5"), "提示不应打乱截断: {hinted}");
    }

    #[tokio::test]
    async fn provider_models_endpoint_id_reaches_handler_through_admin_router() {
        let config = AppConfig::bootstrap().normalized();
        let engine = Engine::new(
            config,
            None,
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            ConfigDir::new(std::env::temp_dir().join(format!(
                "sumpter-admin-provider-models-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ))),
            None,
            AdminListen::default(),
            AdminAuth::password(b"synthetic-admin-password").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();

        let client = reqwest::Client::new();
        let (cookie, csrf) = login_test_session(&client, address, "synthetic-admin-password").await;
        let response = client
            .post(format!("http://{address}/admin/api/provider-models"))
            .header(header::HOST, "127.0.0.1:57879")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(r#"{"endpointID":"missing-endpoint"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"], "endpoint_not_found");
        assert_eq!(body["message"], "missing-endpoint");

        server.shutdown().await;
    }

    #[tokio::test]
    async fn endpoint_secret_returns_plaintext_only_to_authenticated_callers() {
        use sumpter_core::config::EndpointProtocolMode;

        const SYNTHETIC_KEY: &str = "sk-synthetic-plaintext-4242";
        let mut config = AppConfig::bootstrap();
        config.endpoints.push(Endpoint {
            api_key: SYNTHETIC_KEY.into(),
            base_url: "https://api.example.com".into(),
            catalog: None,
            enabled: true,
            id: "probe".into(),
            keep_alive: false,
            mappings: vec![],
            name: "探针入口".into(),
            priority: 0,
            protocol: EndpointProtocolMode::Anthropic,
            user_agent: Default::default(),
            sticky_group: None,
        });
        let engine = Engine::new(
            config.normalized(),
            None,
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            ConfigDir::new(std::env::temp_dir().join(format!(
                "sumpter-admin-endpoint-secret-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ))),
            None,
            AdminListen::default(),
            AdminAuth::password(b"synthetic-admin-password").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/api/endpoint-secret?endpointID=probe");

        let unauthorized = client.get(&url).send().await.unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let (cookie, _csrf) =
            login_test_session(&client, address, "synthetic-admin-password").await;
        let authorized = client
            .get(&url)
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
        assert_eq!(
            authorized
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let body: Value = serde_json::from_slice(&authorized.bytes().await.unwrap()).unwrap();
        assert_eq!(body["endpointID"], "probe");
        assert_eq!(body["apiKey"], SYNTHETIC_KEY);
        assert_eq!(body["configured"], true);

        let missing = client
            .get(format!(
                "http://{address}/admin/api/endpoint-secret?endpointID=nope"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);

        let no_query = client
            .get(format!("http://{address}/admin/api/endpoint-secret"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(no_query.status(), StatusCode::BAD_REQUEST);

        // 明文只走这一个显式端点;配置视图的脱敏红线保持不变。
        let view = client
            .get(format!("http://{address}/admin/api/config"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        let text = view.text().await.unwrap();
        assert!(!text.contains(SYNTHETIC_KEY));

        server.shutdown().await;
    }

    /// sticky-clear 是运维面动作:鉴权 + 参数校验 + runtime store 就绪时返回
    /// 清除计数;本测试走空库路径,引擎层测试再验证真实清除链路。
    #[tokio::test]
    async fn project_sticky_clear_requires_auth_body_and_project_id() {
        let config = AppConfig::bootstrap().normalized();
        let dir = ConfigDir::new(std::env::temp_dir().join(format!(
            "sumpter-admin-sticky-clear-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        )));
        let engine = Engine::new(
            config,
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            dir,
            None,
            AdminListen::default(),
            AdminAuth::password(b"synthetic-admin-password").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/api/runtime/projects/sticky-clear");

        let unauthorized = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"projectID":"sha256:abc"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let (cookie, csrf) = login_test_session(&client, address, "synthetic-admin-password").await;
        let empty_id = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(r#"{"projectID":"   "}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(empty_id.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&empty_id.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"], "project_id_required");

        // 空库:项目无事件 → 匹配 0、清除 0,仍是成功路径。
        let ok = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(r#"{"projectID":"sha256:synthetic"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&ok.bytes().await.unwrap()).unwrap();
        assert_eq!(body["cleared"], 0);
        assert_eq!(body["matched"], 0);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn session_sticky_clear_requires_auth_body_and_session_id() {
        let config = AppConfig::bootstrap().normalized();
        let dir = ConfigDir::new(std::env::temp_dir().join(format!(
            "sumpter-admin-sticky-clear-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        )));
        let engine = Engine::new(
            config,
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            dir,
            None,
            AdminListen::default(),
            AdminAuth::password(b"synthetic-admin-password").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/api/runtime/sessions/sticky-clear");

        let unauthorized = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"sessionID":"sha256:abc"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let (cookie, csrf) = login_test_session(&client, address, "synthetic-admin-password").await;
        let empty_id = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(r#"{"sessionID":"   "}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(empty_id.status(), StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&empty_id.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"], "session_id_required");

        for body in [
            r#"{}"#,
            r#"{"sessionID":"unidentified_session"}"#,
            "invalid-json",
        ] {
            let response = client
                .post(&url)
                .header("content-type", "application/json")
                .header(header::COOKIE, &cookie)
                .header("x-sumpter-csrf", &csrf)
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        // 空库:项目无事件 → 匹配 0、清除 0,仍是成功路径。
        let ok = client
            .post(&url)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(r#"{"sessionID":"sha256:synthetic"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: Value = serde_json::from_slice(&ok.bytes().await.unwrap()).unwrap();
        assert_eq!(body["cleared"], 0);
        assert_eq!(body["matched"], 0);

        server.shutdown().await;
    }

    #[tokio::test]
    async fn session_auth_protects_api_and_leaves_static_login_public() {
        let config = AppConfig::bootstrap().normalized();
        let engine = Engine::new(
            config,
            None,
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::with_systemd_scope_and_auth(
            engine.clone(),
            ProxySupervisor::new(engine),
            ConfigDir::new(std::env::temp_dir().join(format!(
                "sumpter-admin-auth-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ))),
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../platforms/linux/web")),
            SystemdScope::User,
            AdminListen::default(),
            AdminAuth::password(b"correct horse battery staple").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/api/status");

        let health = client
            .get(format!("http://{address}/healthz"))
            .send()
            .await
            .unwrap();
        assert_eq!(health.status(), StatusCode::NO_CONTENT);

        let page = client
            .get(format!("http://{address}/admin/"))
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page_html = page.text().await.unwrap();
        assert!(page_html.contains("管理台"));

        let missing = client.get(&url).send().await.unwrap();
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            missing
                .headers()
                .get(axum::http::HeaderName::from_static("www-authenticate")),
            None
        );
        assert_eq!(
            missing
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        let basic_is_not_supported = client
            .get(&url)
            .header(header::AUTHORIZATION, "Basic a2tsOnN5bnRoZXRpYw==")
            .send()
            .await
            .unwrap();
        assert_eq!(basic_is_not_supported.status(), StatusCode::UNAUTHORIZED);

        let rejected = client
            .post(format!("http://{address}/admin/api/auth/login"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(r#"{"username":"kkl","password":"wrong password"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);

        let (cookie, csrf) =
            login_test_session(&client, address, "correct horse battery staple").await;
        let accepted = client
            .get(&url)
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::OK);

        let write_without_csrf = client
            .post(format!("http://{address}/admin/api/proxy/start"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(write_without_csrf.status(), StatusCode::FORBIDDEN);

        let logout = client
            .post(format!("http://{address}/admin/api/auth/logout"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(logout.status(), StatusCode::OK);
        assert_eq!(
            client
                .get(&url)
                .header(header::COOKIE, &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );

        let script_path = page_html
            .split("<script type=\"module\" crossorigin src=\"")
            .nth(1)
            .and_then(|value| value.split('\"').next())
            .expect("built WebUI module script");
        let script_url = format!(
            "http://{address}/admin/{}",
            script_path.trim_start_matches("./")
        );
        let script_missing = client.get(&script_url).send().await.unwrap();
        assert_eq!(script_missing.status(), StatusCode::OK);
        assert_eq!(
            script_missing
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );

        server.shutdown().await;
    }

    #[tokio::test]
    async fn runtime_api_v1_replaces_legacy_contract_and_protects_reset() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-admin-runtime-v1-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let dir = ConfigDir::new(root.clone());
        let config = AppConfig::bootstrap().normalized();
        let engine = Engine::new(
            config,
            Some(dir.clone()),
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        // 全量导出读取最近一次原子快照；先写一个空快照钉住路由、鉴权和响应头契约。
        engine.flush_diagnostic_capture().unwrap();
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            dir,
            None,
            AdminListen::default(),
            AdminAuth::password(b"runtime-api-password").unwrap(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let (cookie, csrf) = login_test_session(&client, address, "runtime-api-password").await;

        let export_without_cookie = client
            .get(format!(
                "http://{address}/admin/api/diagnostic-capture/export?scope=all&format=json&privacy=redacted"
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(export_without_cookie.status(), StatusCode::UNAUTHORIZED);
        let export = client
            .get(format!(
                "http://{address}/admin/api/diagnostic-capture/export?scope=all&format=json&privacy=redacted"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(export.status(), StatusCode::OK);
        assert_eq!(
            export
                .headers()
                .get(header::CONTENT_DISPOSITION)
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"sumpter-diagnostic-all-redacted.json\"")
        );
        assert_eq!(
            export
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            export
                .headers()
                .get("x-content-type-options")
                .and_then(|value| value.to_str().ok()),
            Some("nosniff")
        );
        let exported: Value = serde_json::from_slice(&export.bytes().await.unwrap()).unwrap();
        assert_eq!(exported["records"], json!([]));

        let summary = client
            .get(format!("http://{address}/admin/api/runtime/summary"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(summary.status(), StatusCode::OK);
        let summary: Value = serde_json::from_slice(&summary.bytes().await.unwrap()).unwrap();
        assert_eq!(summary["apiVersion"], 1);
        assert_eq!(summary["storage"]["backend"], "sqlite");

        let events = client
            .get(format!(
                "http://{address}/admin/api/runtime/events?limit=50"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);
        let events: Value = serde_json::from_slice(&events.bytes().await.unwrap()).unwrap();
        assert_eq!(events["cursorValid"], true);
        assert_eq!(events["events"], json!([]));

        let page = client
            .get(format!(
                "http://{address}/admin/api/runtime/events?view=page&page=1&pageSize=25"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page: Value = serde_json::from_slice(&page.bytes().await.unwrap()).unwrap();
        assert_eq!(page["apiVersion"], 3);
        assert_eq!(page["totalCount"], 0);
        assert_eq!(page["pageSize"], 25);

        let mixed_cursor = client
            .get(format!(
                "http://{address}/admin/api/runtime/events?view=page&beforeSeq=10"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(mixed_cursor.status(), StatusCode::BAD_REQUEST);

        let storage = client
            .get(format!("http://{address}/admin/api/runtime/storage"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(storage.status(), StatusCode::OK);
        let storage: Value = serde_json::from_slice(&storage.bytes().await.unwrap()).unwrap();
        assert_eq!(storage["schemaVersion"], 5);
        assert_eq!(storage["retainedEvents"], 0);

        let retention_update = client
            .put(format!("http://{address}/admin/api/runtime/retention"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(
                serde_json::to_vec(&json!({
                    "expectedRevision": 1,
                    "maxEvents": 10000
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(retention_update.status(), StatusCode::BAD_REQUEST);
        let retention_error: Value =
            serde_json::from_slice(&retention_update.bytes().await.unwrap()).unwrap();
        assert_eq!(retention_error["error"], "invalid_json");

        let retention_update = client
            .put(format!("http://{address}/admin/api/runtime/retention"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body(
                serde_json::to_vec(&json!({
                    "expectedRevision": 1,
                    "maxAgeDays": 30,
                    "storageLimitBytes": 8388608
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(retention_update.status(), StatusCode::OK);
        let retention_update: Value =
            serde_json::from_slice(&retention_update.bytes().await.unwrap()).unwrap();
        assert_eq!(retention_update["revision"], 2);
        assert_eq!(retention_update["maxAgeDays"], 30);
        assert_eq!(retention_update["storageLimitBytes"], 8_388_608);
        assert!(retention_update.get("maxEvents").is_none());

        let estimate = client
            .get(format!(
                "http://{address}/admin/api/runtime/export/estimate?scope=events&format=jsonl&privacy=redacted"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(estimate.status(), StatusCode::OK);
        let estimate: Value = serde_json::from_slice(&estimate.bytes().await.unwrap()).unwrap();
        assert_eq!(estimate["rowCount"], 0);

        let stored_without_confirmation = client
            .get(format!(
                "http://{address}/admin/api/runtime/export?scope=events&format=csv&privacy=stored"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(
            stored_without_confirmation.status(),
            StatusCode::BAD_REQUEST
        );

        let export = client
            .get(format!(
                "http://{address}/admin/api/runtime/export?scope=events&format=jsonl&privacy=redacted"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(export.status(), StatusCode::OK);
        assert_eq!(
            export
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            export
                .headers()
                .get("x-sumpter-row-count")
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );

        let Query(request_query) = Query::<RuntimeEventsQuery>::try_from_uri(
            &"/runtime/events?requestID=request-wire&beforeSeq=9&afterChangeSeq=3&limit=50"
                .parse()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(request_query.request_id.as_deref(), Some("request-wire"));
        assert_eq!(request_query.before_seq, Some(9));
        assert_eq!(request_query.after_change_seq, Some(3));

        let Query(decimal_query) = Query::<RuntimeEventsQuery>::try_from_uri(
            &"/runtime/events?from=809539200.0&to=809539260.0"
                .parse()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(decimal_query.from, Some(809539200.0));
        assert_eq!(decimal_query.to, Some(809539260.0));

        let missing_detail = client
            .get(format!("http://{address}/admin/api/runtime/events/missing"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(missing_detail.status(), StatusCode::NOT_FOUND);

        let one_hour_range = client
            .get(format!(
                "http://{address}/admin/api/runtime/analytics?range=1h"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(one_hour_range.status(), StatusCode::OK);

        let missing_session_delete = client
            .delete(format!("http://{address}/admin/api/runtime/session"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_session_delete.status(), StatusCode::BAD_REQUEST);
        let missing_session_export = client
            .get(format!("http://{address}/admin/api/runtime/session/export"))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(missing_session_export.status(), StatusCode::BAD_REQUEST);

        let unknown_session_delete = client
            .delete(format!(
                "http://{address}/admin/api/runtime/session?sessionID=missing-session"
            ))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_session_delete.status(), StatusCode::NOT_FOUND);
        let unknown_session_export = client
            .get(format!(
                "http://{address}/admin/api/runtime/session/export?sessionID=missing-session"
            ))
            .header(header::COOKIE, &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_session_export.status(), StatusCode::NOT_FOUND);

        for (method, path) in [
            (reqwest::Method::GET, "/admin/api/runtime"),
            (reqwest::Method::POST, "/admin/api/reset-stats"),
        ] {
            let response = client
                .request(method, format!("http://{address}{path}"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &cookie)
                .header("x-sumpter-csrf", &csrf)
                .body("{}")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let reset_without_csrf = client
            .post(format!("http://{address}/admin/api/runtime/reset"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(reset_without_csrf.status(), StatusCode::FORBIDDEN);

        let reset = client
            .post(format!("http://{address}/admin/api/runtime/reset"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .send()
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        let reset: Value = serde_json::from_slice(&reset.bytes().await.unwrap()).unwrap();
        assert_eq!(reset["reset"], true);
        assert_eq!(reset["resetGeneration"], 1);

        let empty_reset = client
            .post(format!("http://{address}/admin/api/runtime/reset"))
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .send()
            .await
            .unwrap();
        assert_eq!(empty_reset.status(), StatusCode::OK);
        let empty_reset: Value =
            serde_json::from_slice(&empty_reset.bytes().await.unwrap()).unwrap();
        assert_eq!(empty_reset["reset"], true);
        assert_eq!(empty_reset["resetGeneration"], 2);

        let recreate = client
            .post(format!("http://{address}/admin/api/runtime/recreate"))
            .header(header::COOKIE, &cookie)
            .header("x-sumpter-csrf", &csrf)
            .send()
            .await
            .unwrap();
        assert_eq!(recreate.status(), StatusCode::OK);
        let recreate: Value = serde_json::from_slice(&recreate.bytes().await.unwrap()).unwrap();
        assert_eq!(recreate["reset"], true);
        assert_eq!(recreate["recreated"], true);
        assert_eq!(recreate["resetGeneration"], 3);

        server.shutdown().await;
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn credentials_route_rotates_session_and_persists_new_username() {
        let credential_path = std::env::temp_dir().join(format!(
            "sumpter-admin-credentials-route-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(&credential_path, b"old-admin-password\n").unwrap();
        let auth = AdminAuth::from_password_file(&credential_path).unwrap();
        let config = AppConfig::bootstrap().normalized();
        let engine = Engine::new(
            config,
            None,
            Arc::new(crate::outbound::ReqwestTransport::new()),
        );
        let state = AdminState::new(
            engine.clone(),
            ProxySupervisor::new(engine),
            ConfigDir::new(std::env::temp_dir().join(format!(
                "sumpter-admin-credentials-config-{}-{}",
                std::process::id(),
                rand::random::<u64>()
            ))),
            None,
            AdminListen::default(),
            auth,
        );
        let (address, server) = crate::server::serve_router(
            admin_router(state),
            SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let (old_cookie, old_csrf) =
            login_test_session(&client, address, "old-admin-password").await;
        let response = client
            .put(format!("http://{address}/admin/api/auth/credentials"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, &old_cookie)
            .header("x-sumpter-csrf", old_csrf)
            .body(
                serde_json::to_string(&json!({
                    "currentPassword": "old-admin-password",
                    "username": "new-admin",
                    "newPassword": "new-admin-password-1234"
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let replacement_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        let replacement_csrf = body["csrfToken"].as_str().unwrap();
        assert_eq!(body["username"], "new-admin");
        assert_eq!(
            client
                .get(format!("http://{address}/admin/api/status"))
                .header(header::COOKIE, &old_cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            client
                .get(format!("http://{address}/admin/api/status"))
                .header(header::COOKIE, &replacement_cookie)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert!(
            std::fs::read_to_string(&credential_path)
                .unwrap()
                .contains("new-admin")
        );
        assert!(
            !std::fs::read_to_string(&credential_path)
                .unwrap()
                .contains("new-admin-password-1234")
        );
        assert!(
            client
                .post(format!("http://{address}/admin/api/auth/logout"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::COOKIE, &replacement_cookie)
                .header("x-sumpter-csrf", replacement_csrf)
                .body("{}")
                .send()
                .await
                .unwrap()
                .status()
                == StatusCode::OK
        );
        server.shutdown().await;
        let _ = std::fs::remove_file(credential_path);
    }

    #[test]
    fn session_auth_requires_csrf_for_writes() {
        let auth = AdminAuth::password(b"part:two").unwrap();
        let grant = auth.login("kkl", "part:two", false).unwrap();
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(
            header::COOKIE,
            grant.set_cookie.split(';').next().unwrap().parse().unwrap(),
        );
        assert!(auth.authenticate(&headers).is_some());
        assert!(!auth.accepts_csrf(&headers, &grant.session));
        headers.insert("x-sumpter-csrf", grant.session.csrf_token.parse().unwrap());
        assert!(auth.accepts_csrf(&headers, &grant.session));
    }

    async fn login_test_session(
        client: &reqwest::Client,
        address: SocketAddr,
        password: &str,
    ) -> (String, String) {
        let response = client
            .post(format!("http://{address}/admin/api/auth/login"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_string(&json!({"username": "kkl", "password": password})).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .unwrap()
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_string();
        let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
        (cookie, body["csrfToken"].as_str().unwrap().to_string())
    }

    #[test]
    fn omitted_secret_update_preserves_and_empty_clears() {
        let mut old = AppConfig::bootstrap();
        old.listener.auth_token = "inbound-secret".into();
        let mut preserved = old.clone();
        preserved.listener.auth_token.clear();
        apply_secrets(&old, &mut preserved, SecretUpdates::default()).unwrap();
        assert_eq!(preserved.listener.auth_token, "inbound-secret");

        let mut cleared = old.clone();
        cleared.listener.auth_token.clear();
        apply_secrets(
            &old,
            &mut cleared,
            SecretUpdates {
                inbound_auth_token: Some(String::new()),
                endpoints: HashMap::new(),
            },
        )
        .unwrap();
        assert!(cleared.listener.auth_token.is_empty());
    }

    #[test]
    fn bootstrap_passes_linux_validation() {
        validate_config(&AppConfig::bootstrap()).unwrap();
    }

    #[test]
    fn listener_address_maps_localhost_without_accepting_other_hostnames() {
        let mut config = AppConfig::bootstrap();
        config.listener.port = 57_878;
        for host in ["localhost", "LOCALHOST", "LocalHost"] {
            config.listener.host = host.into();
            assert_eq!(
                listener_address(&config).unwrap(),
                SocketAddr::new(IpAddr::from([127, 0, 0, 1]), 57_878),
                "{host}"
            );
        }

        config.listener.host = "example.com".into();
        assert!(listener_address(&config).is_err());
    }

    #[test]
    fn systemd_scope_never_infers_or_grants_system_control() {
        assert_eq!(SystemdScope::parse("user").unwrap(), SystemdScope::User);
        assert_eq!(SystemdScope::parse("system").unwrap(), SystemdScope::System);
        assert!(SystemdScope::parse("root").is_err());

        assert_eq!(
            SystemdScope::User.systemctl_args("is-enabled"),
            vec!["--user", "is-enabled", SYSTEMD_UNIT]
        );
        assert_eq!(
            SystemdScope::System.systemctl_args("is-enabled"),
            vec!["is-enabled", SYSTEMD_UNIT]
        );
        assert!(SystemdScope::User.controllable());
        assert!(!SystemdScope::System.controllable());
        assert_eq!(
            SystemdScope::System.journalctl_command(),
            "journalctl -u sumpter.service -n 200 --no-pager"
        );
    }

    #[test]
    fn admin_listen_defaults_and_rejects_bad_host() {
        let default = AdminListen::default();
        assert_eq!(default.host, DEFAULT_ADMIN_HOST);
        assert_eq!(default.port, DEFAULT_ADMIN_PORT);
        assert!(default.is_loopback_bind());
        assert!(!default.is_all_interfaces());

        let all = AdminListen::new("0.0.0.0", 9_001).unwrap();
        assert!(all.is_all_interfaces());
        assert!(!all.is_loopback_bind());
        assert_eq!(
            all.socket_addr(),
            SocketAddr::new(IpAddr::from([0, 0, 0, 0]), 9_001)
        );

        assert!(AdminListen::new("example.com", 9_001).is_err());
        assert!(AdminListen::new("127.0.0.1", 0).is_err());
    }

    #[test]
    fn diagnostic_capture_redaction_masks_headers_urls_and_json_body_fields() {
        let mut value = json!({
            "inboundHeaders": [
                {"name": "Authorization", "value": "Bearer super-secret"},
                {"name": "X-Request-ID", "value": "req-1"}
            ],
            "outboundURL": "https://provider.invalid/v1/messages?api_key=secret&trace=ok",
            "outboundBody": r#"{"model":"demo","api_key":"secret","messages":[{"text":"hello"}]}"#,
            "attempts": [{
                "responseHeaders": [{"name":"Set-Cookie", "value":"sid=secret"}],
                "error": "upstream failed token=secret"
            }]
        });
        redact_capture_value(&mut value);
        assert_eq!(value["inboundHeaders"][0]["value"], "[REDACTED]");
        assert_eq!(value["inboundHeaders"][1]["value"], "req-1");
        assert_eq!(
            value["outboundURL"],
            "https://provider.invalid/v1/messages?api_key=[REDACTED]&trace=ok"
        );
        let body: Value = serde_json::from_str(value["outboundBody"].as_str().unwrap()).unwrap();
        assert_eq!(body["api_key"], "[REDACTED]");
        assert_eq!(
            value["attempts"][0]["responseHeaders"][0]["value"],
            "[REDACTED]"
        );
        assert_eq!(
            value["attempts"][0]["error"],
            "upstream failed token=[REDACTED]"
        );
    }

    #[test]
    fn diagnostic_capture_format_defaults_to_jsonl_and_rejects_unknown_values() {
        assert_eq!(
            parse_diagnostic_capture_format(None).unwrap(),
            DiagnosticCaptureFormat::Jsonl
        );
        assert_eq!(
            parse_diagnostic_capture_format(Some("json")).unwrap(),
            DiagnosticCaptureFormat::Json
        );
        assert!(parse_diagnostic_capture_format(Some("xml")).is_err());
    }
}
