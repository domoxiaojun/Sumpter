//! Linux Admin API 与代理 listener 生命周期。
//!
//! Admin 默认绑定 `127.0.0.1:57879`，可通过 daemon CLI / 环境变量覆盖。
//! Web 静态资源公开加载，管理 API 与 SSE 使用内置登录会话 Cookie + CSRF。
//! 所有配置写入、reload 与 SIGHUP 共用一个串行事务；
//! Proxy listener 可独立启停或原子重绑。

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::services::ServeDir;

use sumpter_core::config::{AppConfig, Endpoint, SCHEMA_VERSION};
use sumpter_core::config_store::{ConfigDir, MigrationNotice, validate_config_wire};

pub use crate::admin_auth::{AdminAuth, DEFAULT_ADMIN_USERNAME};
use crate::engine::{Engine, EngineNotice};
use crate::runtime_query::{
    DimensionKind, DimensionPageQuery, DimensionSort, ErrorPageQuery, EventPageQuery, ExportFormat,
    ExportPrivacy, ExportQuery, ExportScope, RuntimeFilter, RuntimeQueryError, SortOrder,
    TrendGranularity, TrendQuery,
};
use crate::runtime_store::{
    AnalyticsFilter, RuntimeModelPriceInput, RuntimePricingUpdate, RuntimeRetentionUpdate,
};
use crate::{health, server};

pub const DEFAULT_ADMIN_HOST: &str = "127.0.0.1";
pub const DEFAULT_ADMIN_PORT: u16 = 57_879;
pub const SYSTEMD_UNIT: &str = "sumpter.service";
const MAX_MODEL_CATALOG_BYTES: usize = 2 * 1024 * 1024;
const MAX_MODEL_CATALOG_ITEMS: usize = 5_000;

/// Query parameters arrive from `serde_urlencoded` as strings even when the
/// target type is numeric. Keep accepting the decimal form emitted by older
/// macOS clients (for example `809539200.0`) instead of relying on the
/// extractor's direct `f64` deserializer, which rejects that representation.
fn deserialize_optional_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
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

struct ProxyRuntime {
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

fn listener_address(config: &AppConfig) -> Result<SocketAddr, String> {
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

struct AdminInner {
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

async fn admin_guard(
    State(state): State<AdminState>,
    mut request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    let secure_cookie = request_uses_https(request.headers());
    let Some(session) = state.inner.admin_auth.authenticate(request.headers()) else {
        return unauthorized_response(secure_cookie);
    };
    let bodyless_runtime_reset = request.method() == Method::POST
        && matches!(
            request.uri().path(),
            "/runtime/reset"
                | "/admin/api/runtime/reset"
                | "/runtime/recreate"
                | "/admin/api/runtime/recreate"
        );
    if is_write_method(request.method())
        && !bodyless_runtime_reset
        && !is_json_content_type(request.headers())
    {
        return api_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "json_required",
            "写请求必须使用 Content-Type: application/json",
        );
    }
    if is_write_method(request.method())
        && !state
            .inner
            .admin_auth
            .accepts_csrf(request.headers(), &session)
    {
        return api_error(
            StatusCode::FORBIDDEN,
            "csrf_required",
            "登录会话校验失败，请刷新页面后重试",
        );
    }
    request.extensions_mut().insert(session);
    next.run(request).await
}

fn unauthorized_response(secure_cookie: bool) -> Response {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "admin_auth_required",
        "登录会话不存在或已过期",
    );
    response.headers_mut().insert(
        header::SET_COOKIE,
        header::HeaderValue::from_str(&AdminAuth::clear_cookie(secure_cookie))
            .expect("固定 Cookie 属性有效"),
    );
    response
}

async fn admin_security_headers(request: Request<Body>, next: axum::middleware::Next) -> Response {
    let path = request.uri().path().to_string();
    let mut response = next.run(request).await;
    let is_api = path == "/admin/api" || path.starts_with("/admin/api/");
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        header::HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("content-security-policy"),
        header::HeaderValue::from_static(
            "default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'; form-action 'self'",
        ),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-frame-options"),
        header::HeaderValue::from_static("DENY"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("referrer-policy"),
        header::HeaderValue::from_static("no-referrer"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("permissions-policy"),
        header::HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    if is_api || path == "/admin" || path.starts_with("/admin/") {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-store"),
        );
    }
    response
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginBody {
    username: String,
    password: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChangeCredentialsBody {
    current_password: String,
    username: String,
    new_password: String,
}

async fn auth_session(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let secure_cookie = request_uses_https(&headers);
    match state.inner.admin_auth.authenticate(&headers) {
        Some(session) => {
            auth_session_response(&session.username, &session.csrf_token, session.expires_at)
        }
        None => {
            let mut response = json_ok(&json!({"authenticated": false}));
            set_cookie_header(&mut response, &AdminAuth::clear_cookie(secure_cookie));
            response
        }
    }
}

async fn auth_login(
    State(state): State<AdminState>,
    headers: HeaderMap,
    payload: JsonPayload<LoginBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.username.chars().count() > 128 || body.password.chars().count() > 1024 {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_credentials",
            "用户名或密码格式无效",
        );
    }
    let secure_cookie = request_uses_https(&headers);
    let Some(grant) = state
        .inner
        .admin_auth
        .login(&body.username, &body.password, secure_cookie)
    else {
        tokio::time::sleep(Duration::from_millis(250)).await;
        return api_error(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "用户名或密码不正确",
        );
    };
    let mut response = auth_session_response(
        &grant.session.username,
        &grant.session.csrf_token,
        grant.session.expires_at,
    );
    set_cookie_header(&mut response, &grant.set_cookie);
    response
}

async fn auth_logout(State(state): State<AdminState>, headers: HeaderMap) -> Response {
    let secure_cookie = request_uses_https(&headers);
    state.inner.admin_auth.logout(&headers);
    let mut response = json_ok(&json!({"authenticated": false}));
    set_cookie_header(&mut response, &AdminAuth::clear_cookie(secure_cookie));
    response
}

async fn change_admin_credentials(
    State(state): State<AdminState>,
    headers: HeaderMap,
    payload: JsonPayload<ChangeCredentialsBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let secure_cookie = request_uses_https(&headers);
    match state.inner.admin_auth.change_credentials(
        &body.current_password,
        &body.username,
        &body.new_password,
        secure_cookie,
    ) {
        Ok(update) => {
            let mut response = auth_session_response(
                &update.grant.session.username,
                &update.grant.session.csrf_token,
                update.grant.session.expires_at,
            );
            if let Some(warning) = update.durability_warning {
                let mut value = json!({
                    "authenticated": true,
                    "username": update.grant.session.username,
                    "csrfToken": update.grant.session.csrf_token,
                    "expiresAt": update.grant.session.expires_at,
                    "warning": warning,
                });
                response = json_ok(&value.take());
            }
            set_cookie_header(&mut response, &update.grant.set_cookie);
            response
        }
        Err(message) if message == "当前密码不正确" => api_error(
            StatusCode::BAD_REQUEST,
            "invalid_current_password",
            &message,
        ),
        Err(message) if message.starts_with("用户名") || message.starts_with("新密码") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_credentials", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "credentials_write_failed",
            &message,
        ),
    }
}

fn auth_session_response(username: &str, csrf_token: &str, expires_at: u64) -> Response {
    json_ok(&json!({
        "authenticated": true,
        "username": username,
        "csrfToken": csrf_token,
        "expiresAt": expires_at,
    }))
}

fn set_cookie_header(response: &mut Response, value: &str) {
    if let Ok(value) = header::HeaderValue::from_str(value) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
}

fn request_uses_https(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"))
        || headers
            .get("forwarded")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .any(|part| part.trim().eq_ignore_ascii_case("proto=https"))
            })
}

fn is_write_method(method: &Method) -> bool {
    method == Method::POST
        || method == Method::PUT
        || method == Method::PATCH
        || method == Method::DELETE
}

fn is_json_content_type(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
        })
}

async fn status(State(state): State<AdminState>) -> Response {
    let config = state.inner.engine.config();
    let snapshot = state.inner.engine.runtime_snapshot();
    let proxy = state.inner.proxy.status().await;
    let health = health::evaluate(&snapshot.recent_events, proxy.running);
    json_ok(&json!({
        "running": proxy.running,
        "daemonRunning": true,
        "runtimeApiVersion": 1,
        "state": proxy.state,
        "version": env!("CARGO_PKG_VERSION"),
        "generation": state.inner.engine.generation(),
        "uptimeSeconds": state.inner.engine.uptime_seconds(),
        "listener": {
            "host": config.listener.host,
            "port": config.listener.port,
            "allowedCIDRs": config.listener.allowed_cidrs,
            "hasAuthToken": !config.listener.auth_token.is_empty(),
        },
        "providers": config.endpoints.len(),
        "endpoints": config.endpoints.len(),
        "counters": {
            "clientRequests": snapshot.client_requests,
            "clientSuccesses": snapshot.client_successes,
            "clientFailures": snapshot.client_failures,
            "upstreamAttempts": snapshot.upstream_attempts,
            "failovers": snapshot.failovers,
        },
        "health": health,
        "lastError": state.inner.engine.last_error(),
    }))
}

type JsonPayload<T> = Result<Json<T>, JsonRejection>;

#[allow(clippy::result_large_err)]
fn require_json<T>(payload: JsonPayload<T>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        api_error(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &rejection.to_string(),
        )
    })
}

async fn proxy_start(State(state): State<AdminState>, payload: JsonPayload<Value>) -> Response {
    if let Err(response) = require_json(payload) {
        return response;
    }
    match state.inner.proxy.start().await {
        Ok(proxy) => {
            state.inner.engine.set_last_error(None);
            json_ok(&json!({"proxy": proxy, "running": true}))
        }
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(StatusCode::CONFLICT, "proxy_start_failed", &message)
        }
    }
}

async fn proxy_stop(State(state): State<AdminState>, payload: JsonPayload<Value>) -> Response {
    if let Err(response) = require_json(payload) {
        return response;
    }
    let proxy = state.inner.proxy.stop().await;
    json_ok(&json!({"proxy": proxy, "running": false}))
}

async fn runtime_summary(State(state): State<AdminState>) -> Response {
    json_ok(&state.inner.engine.runtime_summary_value())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeEventsQuery {
    view: Option<String>,
    page: Option<usize>,
    #[serde(rename = "pageSize", alias = "page_size")]
    page_size: Option<usize>,
    #[serde(rename = "snapshotSeq", alias = "snapshot_seq")]
    snapshot_seq: Option<i64>,
    #[serde(rename = "historyGeneration", alias = "history_generation")]
    history_generation: Option<i64>,
    before_seq: Option<i64>,
    after_change_seq: Option<i64>,
    limit: Option<usize>,
    kind: Option<String>,
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
    outcome: Option<String>,
    #[serde(rename = "clientKind", alias = "client_kind")]
    client_kind: Option<String>,
    #[serde(rename = "requestPurpose", alias = "request_purpose")]
    request_purpose: Option<String>,
    #[serde(rename = "endpointID", alias = "endpoint_id")]
    endpoint_id: Option<String>,
    model: Option<String>,
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: Option<String>,
    /// Human-readable project name filter. Keep projectID/project_id as the
    /// stable identity filter; `project` is an additive compatibility alias
    /// used by the WebUI selector.
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "failureKind", alias = "failure_kind")]
    failure_kind: Option<String>,
    #[serde(rename = "failurePhase", alias = "failure_phase")]
    failure_phase: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    to: Option<f64>,
}

async fn runtime_events(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeEventsQuery>,
) -> Response {
    if query.view.as_deref() == Some("page") {
        if query.before_seq.is_some() || query.after_change_seq.is_some() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_runtime_query",
                "view=page 不能与 beforeSeq/afterChangeSeq 同时使用",
            );
        }
        let request = EventPageQuery {
            page: query.page.unwrap_or(1),
            page_size: query.page_size.unwrap_or(10),
            snapshot_seq: query.snapshot_seq,
            history_generation: query.history_generation,
            filter: runtime_filter_from_events_query(&query),
        };
        return match state.inner.engine.runtime_events_page(&request) {
            Ok(value) => json_ok(&value),
            Err(error) => runtime_query_error_response(error),
        };
    }
    if query.view.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_runtime_query",
            "view 只支持 page",
        );
    }
    match state.inner.engine.runtime_events(
        query.before_seq,
        query.after_change_seq,
        query.limit.unwrap_or(10),
        query.kind.as_deref(),
        query.request_id.as_deref(),
        query.outcome.as_deref(),
        query.from,
        query.to,
    ) {
        Ok(value) => json_ok(&value),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

fn runtime_filter_from_events_query(query: &RuntimeEventsQuery) -> RuntimeFilter {
    RuntimeFilter {
        kind: query.kind.clone(),
        outcome: query.outcome.clone(),
        client_kind: query.client_kind.clone(),
        request_purpose: query.request_purpose.clone(),
        request_id: query.request_id.clone(),
        endpoint_id: query.endpoint_id.clone(),
        model: query.model.clone(),
        project_id: query.project_id.clone(),
        project_name: query.project.clone(),
        session_id: query.session_id.clone(),
        failure_kind: query.failure_kind.clone(),
        failure_phase: query.failure_phase.clone(),
        from: query.from,
        to: query.to,
    }
}

async fn runtime_event_detail(State(state): State<AdminState>, Path(id): Path<String>) -> Response {
    match state.inner.engine.runtime_event(&id) {
        Ok(Some(value)) => json_ok(&value),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            "runtime_event_not_found",
            "运行事件不存在",
        ),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeRequestChainQuery {
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
}

async fn runtime_request_chain(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeRequestChainQuery>,
) -> Response {
    let Some(request_id) = query
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "request_id_required",
            "必须提供 requestID",
        );
    };
    match state.inner.engine.runtime_request_chain(request_id) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFilterQuery {
    kind: Option<String>,
    outcome: Option<String>,
    client_kind: Option<String>,
    request_purpose: Option<String>,
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "endpointID", alias = "endpointId")]
    endpoint_id: Option<String>,
    model: Option<String>,
    #[serde(rename = "projectID", alias = "projectId")]
    project_id: Option<String>,
    /// Human-readable project name filter; projectID remains the stable ID.
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "sessionId")]
    session_id: Option<String>,
    failure_kind: Option<String>,
    failure_phase: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    to: Option<f64>,
}

impl From<RuntimeFilterQuery> for RuntimeFilter {
    fn from(query: RuntimeFilterQuery) -> Self {
        Self {
            kind: query.kind,
            outcome: query.outcome,
            client_kind: query.client_kind,
            request_purpose: query.request_purpose,
            request_id: query.request_id,
            endpoint_id: query.endpoint_id,
            model: query.model,
            project_id: query.project_id,
            project_name: query.project,
            session_id: query.session_id,
            failure_kind: query.failure_kind,
            failure_phase: query.failure_phase,
            from: query.from,
            to: query.to,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeTrendQuery {
    range: Option<String>,
    granularity: Option<String>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeFacetsQuery {
    range: Option<String>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

async fn runtime_facets(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeFacetsQuery>,
) -> Response {
    let now = admin_apple_timestamp();
    let range = query.range.as_deref().unwrap_or("24h");
    let range_from = match range {
        "1h" => Some(now - 3_600.0),
        "24h" => Some(now - 86_400.0),
        // The UI supplies the user's local midnight as `from`. The UTC
        // fallback keeps API callers deterministic instead of silently
        // treating an omitted boundary as "all".
        "today" => Some(utc_today_start(now)),
        "7d" => Some(now - 7.0 * 86_400.0),
        "30d" => Some(now - 30.0 * 86_400.0),
        "all" => None,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "range 必须是 today、1h、24h、7d、30d 或 all",
            );
        }
    };
    let mut filter: RuntimeFilter = query.filters.into();
    filter.from = crate::runtime_query::merge_range_lower_bound(range, filter.from, range_from);
    filter.to = Some(filter.to.map_or(now, |value| value.min(now)));
    match state.inner.engine.runtime_facets(&filter) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_trends(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeTrendQuery>,
) -> Response {
    let now = admin_apple_timestamp();
    let (default_from, default_to) = match query.range.as_deref().unwrap_or("24h") {
        "1h" => (now - 3_600.0, now),
        "24h" => (now - 86_400.0, now),
        // The client-provided local midnight wins when present; otherwise use
        // a deterministic UTC calendar-day fallback.
        "today" => (utc_today_start(now), now),
        "7d" => (now - 7.0 * 86_400.0, now),
        "30d" => (now - 30.0 * 86_400.0, now),
        "all" => (0.0, now),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "range 必须是 today、1h、24h、7d、30d 或 all",
            );
        }
    };
    let granularity = match query.granularity.as_deref().unwrap_or("auto") {
        "auto" => TrendGranularity::Auto,
        "hour" => TrendGranularity::Hour,
        "day" => TrendGranularity::Day,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_granularity",
                "granularity 必须是 auto、hour 或 day",
            );
        }
    };
    let filters = RuntimeFilter::from(query.filters);
    let request = TrendQuery {
        from: filters.from.unwrap_or(default_from),
        to: filters.to.unwrap_or(default_to),
        granularity,
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: filters,
    };
    match state.inner.engine.runtime_trends(&request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimePagedQuery {
    page: Option<usize>,
    page_size: Option<usize>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    search: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

async fn runtime_errors(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let request = ErrorPageQuery {
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(10),
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    };
    match state.inner.engine.runtime_error_groups(&request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_projects(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(state, DimensionKind::Project, query)
}

async fn runtime_sessions(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(state, DimensionKind::Session, query)
}

async fn runtime_dimensions(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let Some(kind) = parse_dimension_kind(query.filters.kind.as_deref()) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_dimension_kind",
            "kind 必须是 endpoint、model、clientKind、purpose、failureKind、failurePhase、protocol、streamTerminal、project 或 session",
        );
    };
    let mut query = query;
    query.filters.kind = None;
    runtime_dimension_response(state, kind, query)
}

fn parse_dimension_kind(value: Option<&str>) -> Option<DimensionKind> {
    match value? {
        "endpoint" => Some(DimensionKind::Endpoint),
        "model" => Some(DimensionKind::Model),
        "clientKind" | "client_kind" => Some(DimensionKind::ClientKind),
        "purpose" | "requestPurpose" | "request_purpose" => Some(DimensionKind::Purpose),
        "failureKind" | "failure_kind" => Some(DimensionKind::FailureKind),
        "failurePhase" | "failure_phase" => Some(DimensionKind::FailurePhase),
        "protocol" => Some(DimensionKind::Protocol),
        "streamTerminal" | "stream_terminal" => Some(DimensionKind::StreamTerminal),
        "project" => Some(DimensionKind::Project),
        "session" => Some(DimensionKind::Session),
        _ => None,
    }
}

fn runtime_dimension_response(
    state: AdminState,
    kind: DimensionKind,
    query: RuntimePagedQuery,
) -> Response {
    let sort = match query.sort.as_deref().unwrap_or("last_seen") {
        "name" => DimensionSort::Name,
        "requests" => DimensionSort::Requests,
        "success_rate" | "successRate" => DimensionSort::SuccessRate,
        "failures" => DimensionSort::Failures,
        "input_tokens" | "inputTokens" => DimensionSort::InputTokens,
        "output_tokens" | "outputTokens" => DimensionSort::OutputTokens,
        "cache_read" | "cacheRead" => DimensionSort::CacheReadTokens,
        "cache_write" | "cacheWrite" => DimensionSort::CacheWriteTokens,
        "tokens" => DimensionSort::Tokens,
        "average_duration" | "averageDuration" => DimensionSort::AverageDuration,
        "last_seen" | "lastSeen" => DimensionSort::LastSeen,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_sort",
                "sort 必须是 name、requests、success_rate、failures、input_tokens、output_tokens、cache_read、cache_write、tokens、average_duration 或 last_seen",
            );
        }
    };
    let order = match query.order.as_deref().unwrap_or("desc") {
        "asc" => SortOrder::Asc,
        "desc" => SortOrder::Desc,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_order",
                "order 必须是 asc 或 desc",
            );
        }
    };
    let request = DimensionPageQuery {
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(10),
        search: query.search,
        sort,
        order,
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    };
    match state.inner.engine.runtime_dimension_page(kind, &request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_storage(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_storage_details() {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_retention(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_storage_details() {
        Ok(value) => json_ok(&value["retention"]),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeRetentionPayload {
    expected_revision: i64,
    #[serde(default)]
    max_age_days: Option<i64>,
    #[serde(default)]
    storage_limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeCleanupPayload {
    /// Apple reference-date seconds; rows strictly older than this cutoff
    /// are eligible when their complete request group is also old.
    older_than: f64,
}

async fn runtime_cleanup_preview(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state
        .inner
        .engine
        .runtime_cleanup_preview(payload.older_than)
    {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => api_error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

async fn runtime_cleanup(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state.inner.engine.runtime_cleanup(payload.older_than) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => api_error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

async fn runtime_retention_update(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeRetentionPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state
        .inner
        .engine
        .runtime_set_retention(RuntimeRetentionUpdate {
            expected_revision: payload.expected_revision,
            max_age_days: payload.max_age_days,
            storage_limit_bytes: payload.storage_limit_bytes,
        }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            api_error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_retention", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_retention_failed",
            &message,
        ),
    }
}

async fn runtime_pricing(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_pricing() {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeModelPricePayload {
    endpoint_id: Option<String>,
    model_key: String,
    effective_from: f64,
    effective_to: Option<f64>,
    input_per_million_micros: Option<i64>,
    output_per_million_micros: Option<i64>,
    cache_read_per_million_micros: Option<i64>,
    cache_creation_per_million_micros: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimePricingPayload {
    expected_revision: i64,
    currency: String,
    prices: Vec<RuntimeModelPricePayload>,
}

async fn runtime_pricing_update(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimePricingPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let prices = payload
        .prices
        .into_iter()
        .map(|price| RuntimeModelPriceInput {
            endpoint_id: price.endpoint_id,
            model_key: price.model_key,
            effective_from: price.effective_from,
            effective_to: price.effective_to,
            input_per_million_micros: price.input_per_million_micros,
            output_per_million_micros: price.output_per_million_micros,
            cache_read_per_million_micros: price.cache_read_per_million_micros,
            cache_creation_per_million_micros: price.cache_creation_per_million_micros,
        })
        .collect();
    match state
        .inner
        .engine
        .runtime_replace_pricing(RuntimePricingUpdate {
            expected_revision: payload.expected_revision,
            currency: payload.currency,
            prices,
        }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            api_error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_pricing", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_pricing_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeExportQuery {
    scope: Option<String>,
    format: Option<String>,
    privacy: Option<String>,
    confirm_stored: Option<bool>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

// Err 侧是已构造好的 axum Response,直接返回给调用方;装箱只会多一次堆分配。
#[allow(clippy::result_large_err)]
fn parse_runtime_export_query(query: RuntimeExportQuery) -> Result<ExportQuery, Response> {
    let scope = match query.scope.as_deref().unwrap_or("events") {
        "events" => ExportScope::Events,
        "projects" => ExportScope::Projects,
        "sessions" => ExportScope::Sessions,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_scope",
                "scope 必须是 events、projects 或 sessions",
            ));
        }
    };
    let format = match query.format.as_deref().unwrap_or("jsonl") {
        "csv" => ExportFormat::Csv,
        "jsonl" => ExportFormat::Jsonl,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_format",
                "format 必须是 csv 或 jsonl",
            ));
        }
    };
    let privacy = match query.privacy.as_deref().unwrap_or("stored") {
        "redacted" => ExportPrivacy::Redacted,
        "stored" => ExportPrivacy::Stored,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_privacy",
                "privacy 必须是 redacted 或 stored",
            ));
        }
    };
    Ok(ExportQuery {
        scope,
        format,
        privacy,
        confirm_stored: query.confirm_stored == Some(true),
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    })
}

async fn runtime_export_estimate(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    match state.inner.engine.runtime_export_estimate(&query) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_export(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if query.privacy == ExportPrivacy::Stored && !query.confirm_stored {
        return api_error(
            StatusCode::BAD_REQUEST,
            "stored_export_confirmation_required",
            "privacy=stored 必须同时提供 confirmStored=true",
        );
    }
    // Resolve and validate the exact snapshot before response headers are
    // committed. The streaming worker reuses these values, so estimate and
    // export cannot drift to a newer history window.
    let estimate = match state.inner.engine.runtime_export_estimate(&query) {
        Ok(value) => value,
        Err(error) => return runtime_query_error_response(error),
    };
    let snapshot_seq = estimate["snapshotSeq"].as_i64().unwrap_or(0);
    let history_generation = estimate["historyGeneration"].as_i64().unwrap_or(0);
    let row_count = estimate["rowCount"].as_i64().unwrap_or(0);
    let mut stream_query = query.clone();
    stream_query.snapshot_seq = Some(snapshot_seq);
    stream_query.history_generation = Some(history_generation);

    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let engine = state.inner.engine.clone();
    tokio::task::spawn_blocking(move || {
        let worker_sender = sender.clone();
        let result = engine.runtime_stream_export(&stream_query, |chunk| {
            worker_sender
                .blocking_send(Ok(Bytes::from(chunk)))
                .map_err(|_| "export client disconnected".to_string())
        });
        if let Err(error) = result {
            let _ = sender.blocking_send(Err(std::io::Error::other(error.to_string())));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    let filename = format!(
        "sumpter-runtime-{}-{}.{}",
        match query.scope {
            ExportScope::Events => "events",
            ExportScope::Projects => "projects",
            ExportScope::Sessions => "sessions",
        },
        match query.privacy {
            ExportPrivacy::Redacted => "redacted",
            ExportPrivacy::Stored => "stored",
        },
        query.format.extension(),
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(query.format.content_type()),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    for (name, value) in [
        ("x-sumpter-snapshot-seq", snapshot_seq.to_string()),
        (
            "x-sumpter-history-generation",
            history_generation.to_string(),
        ),
        ("x-sumpter-row-count", row_count.to_string()),
        (
            "x-sumpter-privacy",
            match query.privacy {
                ExportPrivacy::Redacted => "redacted".into(),
                ExportPrivacy::Stored => "stored".into(),
            },
        ),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    response
}

fn admin_apple_timestamp() -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() - APPLE_EPOCH_OFFSET_SECS)
        .unwrap_or(0.0)
}

fn utc_today_start(now: f64) -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    const DAY_SECS: f64 = 86_400.0;
    let unix = now + APPLE_EPOCH_OFFSET_SECS;
    unix - unix.rem_euclid(DAY_SECS) - APPLE_EPOCH_OFFSET_SECS
}

fn runtime_query_error_response(error: RuntimeQueryError) -> Response {
    match error {
        RuntimeQueryError::InvalidInput(message) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_runtime_query", &message)
        }
        RuntimeQueryError::SnapshotExpired { requested, current } => crate::engine::json_response(
            StatusCode::CONFLICT,
            &json!({
                "error": "runtime_snapshot_expired",
                "message": format!("历史快照 {requested} 已失效"),
                "currentHistoryGeneration": current,
            }),
        ),
        RuntimeQueryError::SnapshotTrimmed {
            snapshot_seq,
            retained_from_seq,
        } => crate::engine::json_response(
            StatusCode::CONFLICT,
            &json!({
                "error": "runtime_snapshot_trimmed",
                "message": format!("快照 {snapshot_seq} 涉及已被手动删除的历史记录"),
                "retainedFromSeq": retained_from_seq,
            }),
        ),
        RuntimeQueryError::NotFound(message) => {
            api_error(StatusCode::NOT_FOUND, "runtime_not_found", &message)
        }
        RuntimeQueryError::ProjectionNotReady { backfill_cursor } => crate::engine::json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "error": "runtime_projection_not_ready",
                "message": "历史数据索引正在后台补齐，请稍后重试",
                "backfillCursor": backfill_cursor,
            }),
        ),
        RuntimeQueryError::CorruptPayload { event_id, detail } => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_payload_corrupt",
            &format!("事件 {event_id} 无法读取: {detail}"),
        ),
        RuntimeQueryError::Output(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_export_failed",
            &message,
        ),
        RuntimeQueryError::Sql(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &error.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeAnalyticsQuery {
    range: Option<String>,
    #[serde(rename = "clientKind", alias = "client_kind")]
    client_kind: Option<String>,
    #[serde(rename = "endpointID", alias = "endpoint_id")]
    endpoint_id: Option<String>,
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: Option<String>,
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    to: Option<f64>,
}

fn analytics_filter_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn runtime_analytics(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeAnalyticsQuery>,
) -> Response {
    let range = query.range.as_deref().unwrap_or("24h");
    if !matches!(range, "today" | "1h" | "24h" | "7d" | "30d" | "all") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "range 必须是 today、1h、24h、7d、30d 或 all",
        );
    }
    let filter = AnalyticsFilter {
        client_kind: analytics_filter_value(query.client_kind),
        endpoint_id: analytics_filter_value(query.endpoint_id),
        project_id: analytics_filter_value(query.project_id),
        project: analytics_filter_value(query.project),
        session_id: analytics_filter_value(query.session_id),
        from: query
            .from
            .or_else(|| (range == "today").then(|| utc_today_start(admin_apple_timestamp()))),
        to: query.to,
    };
    match state
        .inner
        .engine
        .runtime_analytics_filtered(range, &filter)
    {
        Ok(value) => json_ok(&value),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct RuntimeSessionQuery {
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "confirmUnidentified", alias = "confirm_unidentified")]
    confirm_unidentified: Option<bool>,
}

async fn delete_runtime_session(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = analytics_filter_value(query.session_id) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match state
        .inner
        .engine
        .delete_runtime_session_confirmed(&session_id, query.confirm_unidentified == Some(true))
    {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("不能删除未识别会话") => {
            api_error(StatusCode::BAD_REQUEST, "session_not_deletable", &message)
        }
        Err(message) if message.contains("Query returned no rows") => {
            api_error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_delete_failed",
            &message,
        ),
    }
}

async fn export_runtime_session(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = analytics_filter_value(query.session_id) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match state.inner.engine.export_runtime_session(&session_id) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("会话不存在") => {
            api_error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_export_failed",
            &message,
        ),
    }
}

async fn reset_runtime(State(state): State<AdminState>) -> Response {
    match state.inner.engine.reset_runtime() {
        Ok(_) => json_ok(
            &json!({"reset": true, "resetGeneration": state.inner.engine.runtime_summary_value()["resetGeneration"]}),
        ),
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_reset_failed",
                &message,
            )
        }
    }
}

async fn recreate_runtime(State(state): State<AdminState>) -> Response {
    match state.inner.engine.recreate_runtime() {
        Ok(generation) => json_ok(&json!({
            "reset": true,
            "recreated": true,
            "resetGeneration": generation,
        })),
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_recreate_failed",
                &message,
            )
        }
    }
}

async fn events(
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

async fn get_config(State(state): State<AdminState>) -> Response {
    let generation = state.inner.engine.generation();
    let migration_notice = state
        .migration_notice()
        .map(|notice| json!({"migrationNotice": notice}));
    json_ok(&config_view(
        &state.inner.engine.config(),
        &generation,
        migration_notice,
    ))
}

struct PutConfigBody {
    expected_generation: String,
    config: AppConfig,
    secret_updates: SecretUpdates,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PutConfigWireBody {
    expected_generation: String,
    config: Value,
    secret_updates: SecretUpdates,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SecretUpdates {
    inbound_auth_token: Option<String>,
    #[serde(default)]
    endpoints: HashMap<String, EndpointSecretUpdate>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndpointSecretUpdate {
    api_key: Option<String>,
}

async fn put_config(
    State(state): State<AdminState>,
    payload: JsonPayload<PutConfigWireBody>,
) -> Response {
    let wire = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    if wire.config.get("schemaVersion").and_then(Value::as_u64) != Some(u64::from(SCHEMA_VERSION)) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_config",
            &format!("config.schemaVersion 必须显式为 {SCHEMA_VERSION}"),
        );
    }
    if let Err(message) = validate_config_wire(&wire.config) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_config", &message);
    }
    let config = match serde_json::from_value::<AppConfig>(wire.config) {
        Ok(config) => config,
        Err(error) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_config",
                &format!("config 解码失败: {error}"),
            );
        }
    };
    let body = PutConfigBody {
        expected_generation: wire.expected_generation,
        config,
        secret_updates: wire.secret_updates,
    };
    match state.put_config(body).await {
        Ok(value) => json_ok(&value),
        Err(failure) => failure.into_response(),
    }
}

async fn reload(State(state): State<AdminState>, payload: JsonPayload<Value>) -> Response {
    if let Err(response) = require_json(payload) {
        return response;
    }
    match state.reload_from_disk().await {
        Ok(result) => json_ok(&serde_json::to_value(result).unwrap_or(Value::Null)),
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(StatusCode::CONFLICT, "reload_failed", &message)
        }
    }
}

fn config_view(config: &AppConfig, generation: &str, extra: Option<Value>) -> Value {
    let mut redacted = config.clone();
    redacted.listener.auth_token.clear();
    for endpoint in &mut redacted.endpoints {
        endpoint.api_key.clear();
    }
    let endpoints = config
        .endpoints
        .iter()
        .map(|endpoint| {
            (
                endpoint.id.clone(),
                json!({
                    "apiKey": secret_status(&endpoint.api_key),
                }),
            )
        })
        .collect::<serde_json::Map<String, Value>>();
    let mut value = json!({
        "generation": generation,
        "config": redacted,
        "warnings": sumpter_core::warnings::evaluate(config),
        "secretStatus": {
            "inboundAuthToken": secret_status(&config.listener.auth_token),
            "endpoints": endpoints,
        },
    });
    if let Some(Value::Object(extra)) = extra
        && let Some(object) = value.as_object_mut()
    {
        object.extend(extra);
    }
    value
}

fn secret_status(secret: &str) -> Value {
    let last4: String = secret
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    json!({
        "configured": !secret.is_empty(),
        "last4": if secret.is_empty() { "" } else { &last4 },
    })
}

fn apply_secrets(
    old: &AppConfig,
    draft: &mut AppConfig,
    updates: SecretUpdates,
) -> Result<(), String> {
    let old_endpoints: HashMap<&str, &str> = old
        .endpoints
        .iter()
        .map(|endpoint| (endpoint.id.as_str(), endpoint.api_key.as_str()))
        .collect();
    let draft_ids: HashSet<&str> = draft
        .endpoints
        .iter()
        .map(|endpoint| endpoint.id.as_str())
        .collect();
    if let Some(unknown) = updates
        .endpoints
        .keys()
        .find(|id| !draft_ids.contains(id.as_str()))
    {
        return Err(format!("secretUpdates 引用了不存在的 endpoint: {unknown}"));
    }

    draft.listener.auth_token = updates
        .inbound_auth_token
        .unwrap_or_else(|| old.listener.auth_token.clone());
    for endpoint in &mut draft.endpoints {
        endpoint.api_key = updates
            .endpoints
            .get(&endpoint.id)
            .and_then(|update| update.api_key.clone())
            .or_else(|| {
                old_endpoints
                    .get(endpoint.id.as_str())
                    .map(|key| (*key).to_string())
            })
            .unwrap_or_default();
    }
    Ok(())
}

fn validate_config_identity(config: &AppConfig) -> Result<(), String> {
    if config.schema_version != SCHEMA_VERSION {
        return Err(format!("schemaVersion 必须为 {SCHEMA_VERSION}"));
    }
    let mut endpoint_ids = HashSet::new();
    for endpoint in &config.endpoints {
        if endpoint.id.trim().is_empty() || !endpoint_ids.insert(endpoint.id.as_str()) {
            return Err(format!("Endpoint ID 为空或重复: {}", endpoint.id));
        }
    }
    let mut rule_ids = HashSet::new();
    for rule in &config.feature_rules {
        if rule.id.trim().is_empty() || !rule_ids.insert(rule.id.as_str()) {
            return Err(format!("分流规则 ID 为空或重复: {}", rule.id));
        }
    }
    Ok(())
}

pub fn validate_config(config: &AppConfig) -> Result<(), String> {
    validate_config_identity(config)?;
    let _ = listener_address(config)?;
    for cidr in &config.listener.allowed_cidrs {
        if !valid_cidr(cidr) {
            return Err(format!("无效 allowedCIDR: {cidr}"));
        }
    }
    if config.retry.max_deferred_rounds < 0
        || config.retry.max_retry_duration_seconds < 0.0
        || config.retry.pinned_ip_concurrency < 1
        || config.retry.max_500_retries < 0
        || config.retry.session_sticky_retries < 0
        || config
            .retry
            .response_timeout_seconds
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        || config
            .retry
            .stream_idle_timeout_seconds
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
        || config
            .retry
            .retry_delay_seconds
            .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err("全局重试/超时参数无效".into());
    }
    for endpoint in &config.endpoints {
        let url = reqwest::Url::parse(&endpoint.base_url)
            .map_err(|_| format!("Endpoint {} 的 baseURL 无效", endpoint.id))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(format!(
                "Endpoint {} 的 baseURL 必须是 HTTP(S)",
                endpoint.id
            ));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(format!(
                "Endpoint {} 的 baseURL 不得内嵌用户名或密码",
                endpoint.id
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(format!(
                "Endpoint {} 的 baseURL 不得包含 query 或 fragment",
                endpoint.id
            ));
        }
        for ip in &endpoint.pinned_ips {
            ip.parse::<IpAddr>()
                .map_err(|_| format!("Endpoint {} 的 pinned IP 无效: {ip}", endpoint.id))?;
        }
        let mut mapping_patterns = HashSet::new();
        for mapping in &endpoint.mappings {
            let pattern = mapping.client_pattern.trim();
            if pattern.is_empty() {
                return Err(format!("Endpoint {} 存在空模型映射", endpoint.id));
            }
            if !mapping_patterns.insert(pattern) {
                return Err(format!(
                    "Endpoint {} 存在重复模型映射: {pattern}",
                    endpoint.id
                ));
            }
            if mapping
                .failover_timeout_seconds
                .is_some_and(|value| !value.is_finite() || value <= 0.0)
            {
                return Err(format!(
                    "Endpoint {} 的 failoverTimeoutSeconds 无效",
                    endpoint.id
                ));
            }
        }
    }
    for rule in &config.feature_rules {
        if let Some(endpoint_id) = rule.target.endpoint_id.as_deref()
            && !config
                .endpoints
                .iter()
                .any(|endpoint| endpoint.id == endpoint_id)
        {
            return Err(format!(
                "分流规则 {} 指向不存在的 Endpoint {endpoint_id}",
                rule.id
            ));
        }
    }
    Ok(())
}

fn valid_cidr(raw: &str) -> bool {
    let Some((ip, prefix)) = raw.trim().split_once('/') else {
        return raw.trim().parse::<IpAddr>().is_ok();
    };
    let Ok(ip) = ip.parse::<IpAddr>() else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u8>() else {
        return false;
    };
    prefix <= if ip.is_ipv4() { 32 } else { 128 }
}

#[derive(Debug, Deserialize)]
struct ProviderModelsBody {
    #[serde(rename = "endpointID", alias = "endpointId")]
    endpoint_id: String,
}

async fn provider_models(
    State(state): State<AdminState>,
    payload: JsonPayload<ProviderModelsBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let config = state.inner.engine.config();
    let Some(endpoint) = config.endpoint(&body.endpoint_id).cloned() else {
        return api_error(
            StatusCode::NOT_FOUND,
            "endpoint_not_found",
            &body.endpoint_id,
        );
    };
    match fetch_provider_models(&endpoint).await {
        Ok((models, source)) => json_ok(&json!({
            "endpointID": endpoint.id,
            "models": models,
            "source": source,
            "updatedAt": SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs().to_string())
                .unwrap_or_default(),
        })),
        Err(message) => api_error(StatusCode::BAD_GATEWAY, "models_fetch_failed", &message),
    }
}

async fn fetch_provider_models(endpoint: &Endpoint) -> Result<(Vec<String>, String), String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        fetch_provider_models_inner(endpoint),
    )
    .await
    .map_err(|_| "获取模型整体超时（12 秒）".to_string())?
}

async fn fetch_provider_models_inner(endpoint: &Endpoint) -> Result<(Vec<String>, String), String> {
    let key = endpoint.api_key.trim();
    let base = reqwest::Url::parse(endpoint.base_url.trim_end_matches('/'))
        .map_err(|_| "baseURL 无效".to_string())?;
    if !matches!(base.scheme(), "http" | "https")
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err("baseURL 仅支持不带凭据、query 或 fragment 的 HTTP(S) 地址".into());
    }
    let host = base
        .host_str()
        .ok_or_else(|| "baseURL 缺少 host".to_string())?;
    let port = base
        .port()
        .unwrap_or(if base.scheme() == "http" { 80 } else { 443 });
    let pinned = probe_pinned_ips(endpoint)?;
    // 出站行为面必须与 `outbound.rs` 的转发路径一致，否则「获取模型」探测的结论
    // 不代表真实转发。默认 reqwest 会读 HTTPS_PROXY/ALL_PROXY 环境代理、做 h2 ALPN
    // 协商、跟随重定向——设了系统代理的机器上探测与转发会走两条不同链路，只认 h1
    // CC 指纹的上游也会给出与转发不同的结果。
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .no_proxy()
        .pool_max_idle_per_host(0)
        .connect_timeout(std::time::Duration::from_secs(2))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|error| error.to_string())?;
    let mut errors = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(12);
    // 鉴权 -> pinned -> 路径的顺序让同一鉴权/地址组合有机会快速覆盖
    // `/v1/models`、`/models` 等候选；每次请求和整个探测均受 deadline 约束，
    // 避免一个失联入口把完整矩阵拖到分钟级。
    'probes: for auth in provider_model_auth_sets(key) {
        for pinned_ip in &pinned {
            for path in model_catalog_paths(&base) {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    errors.push("整体超时，已停止尝试".into());
                    break 'probes;
                }
                let mut url = base.clone();
                url.set_path(&path);
                url.set_query(None);
                url.set_fragment(None);
                let probe_client = if let Some(ip) = pinned_ip {
                    let addr: SocketAddr = format!("{ip}:{port}")
                        .parse()
                        .or_else(|_| format!("[{ip}]:{port}").parse())
                        .map_err(|_| format!("pinned IP 无效: {ip}"))?;
                    reqwest::Client::builder()
                        .use_rustls_tls()
                        .http1_only()
                        .redirect(reqwest::redirect::Policy::none())
                        .tcp_nodelay(true)
                        .no_proxy()
                        .pool_max_idle_per_host(0)
                        .connect_timeout(std::time::Duration::from_secs(2))
                        .timeout(std::time::Duration::from_secs(3))
                        .resolve(host, addr)
                        .build()
                        .map_err(|error| error.to_string())?
                } else {
                    client.clone()
                };
                let mut request = probe_client.get(url.clone());
                for (header, value) in crate::request_build::provider_probe_headers("") {
                    request = request.header(header, value);
                }
                for (header, value) in &auth {
                    request = request.header(*header, value.as_str());
                }
                request = request.timeout(remaining.min(std::time::Duration::from_secs(3)));
                match request.send().await {
                    Ok(response) if response.status() == StatusCode::OK => {
                        if response
                            .content_length()
                            .is_some_and(|length| length > MAX_MODEL_CATALOG_BYTES as u64)
                        {
                            errors.push(format!(
                                "{}: 响应过大(>{} KiB)",
                                url.path(),
                                MAX_MODEL_CATALOG_BYTES / 1024
                            ));
                            continue;
                        }
                        match read_model_catalog_body(response).await {
                            Err(message) => errors.push(format!("{}: {message}", url.path())),
                            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                                Ok(value) => {
                                    let models = extract_models(&value);
                                    if !models.is_empty() {
                                        return Ok((models, url.to_string()));
                                    }
                                    errors.push(format!("{}: 响应中没有模型", url.path()));
                                }
                                Err(error) => errors.push(format!("{}: JSON {error}", url.path())),
                            },
                        }
                    }
                    Ok(response) => {
                        errors.push(format!("{}: HTTP {}", url.path(), response.status()))
                    }
                    Err(error) => errors.push(format!("{}: {error}", url.path())),
                }
            }
        }
    }
    let mut unique = Vec::new();
    for error in errors {
        if !unique.contains(&error) {
            unique.push(error);
        }
    }
    Err(summarize_probe_failure(unique, proxy_env_hint()))
}

async fn read_model_catalog_body(response: reqwest::Response) -> Result<Bytes, String> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("读取响应失败 {error}"))?;
        if body.len().saturating_add(chunk.len()) > MAX_MODEL_CATALOG_BYTES {
            return Err(format!("响应过大(>{} KiB)", MAX_MODEL_CATALOG_BYTES / 1024));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn probe_pinned_ips(endpoint: &Endpoint) -> Result<Vec<Option<String>>, String> {
    let mut values = Vec::new();
    for raw in &endpoint.pinned_ips {
        let ip = raw.trim();
        if ip.is_empty() {
            continue;
        }
        ip.parse::<IpAddr>()
            .map_err(|_| format!("pinned IP 无效: {ip}"))?;
        if !values
            .iter()
            .any(|item: &Option<String>| item.as_deref() == Some(ip))
        {
            values.push(Some(ip.to_string()));
        }
    }
    if endpoint.pinned_ip_exclusive && values.is_empty() {
        return Err("该入口启用了 pinned IP 独占，但没有可用 pinned IP".into());
    }
    if !endpoint.pinned_ip_exclusive || values.is_empty() {
        values.push(None);
    }
    Ok(values)
}

/// 按真实转发的 base path 生成目录候选，避免 `/v1/v1/models`。
fn model_catalog_paths(base: &reqwest::Url) -> Vec<String> {
    let prefix = base.path().trim_end_matches('/');
    let mut paths = Vec::new();
    let append = |paths: &mut Vec<String>, path: &str| {
        let value = if prefix.is_empty() {
            path.to_string()
        } else if prefix.ends_with("/v1") && path.starts_with("/v1/") {
            format!("{}{}", prefix, &path[3..])
        } else {
            format!("{prefix}{path}")
        };
        if !paths.contains(&value) {
            paths.push(value);
        }
    };
    append(&mut paths, "/v1/models");
    append(&mut paths, "/models");
    append(&mut paths, "/v1/model/list");
    // `/api/v1/models` is a root-level compatibility endpoint. Once a custom
    // base path is configured, prefixing it again would target a different API.
    if prefix.is_empty() {
        append(&mut paths, "/api/v1/models");
    }
    paths
}

/// 汇总探测失败原因，并在配了代理时补一句口径说明。
/// `hint` 由调用方传入，测试因此不必改进程环境（改了会污染并行测试里 reqwest 的
/// `Client::new()`，它会读 `*_PROXY` 环境变量）。
fn summarize_probe_failure(errors: Vec<String>, hint: Option<String>) -> String {
    let mut summary = errors.into_iter().take(4).collect::<Vec<_>>().join("；");
    if let Some(hint) = hint {
        summary.push_str(&hint);
    }
    summary
}

/// 环境里配了代理时，在探测失败信息后追加一句说明。
///
/// 探测和转发一样禁用代理（见 `fetch_provider_models_inner`），所以在「只有走代理才能
/// 出网」的机器上这里必然失败。不说明的话很容易被误读成上游挂了，而实际上真正的转发
/// 也走不通。只报告检测到哪个变量名，不回显它的值（可能带内网地址或凭据）。
fn proxy_env_hint() -> Option<String> {
    proxy_hint_for(|name| {
        std::env::var_os(name)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    })
}

/// 文案生成与环境读取分开，测试注入 `is_set` 即可，不改进程环境。
fn proxy_hint_for(is_set: impl Fn(&str) -> bool) -> Option<String> {
    const VARS: [&str; 6] = [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ];
    let mut found: Vec<&str> = VARS.into_iter().filter(|name| is_set(name)).collect();
    if found.is_empty() {
        return None;
    }
    found.dedup();
    Some(format!(
        "（检测到 {}：模型探测与实际转发一样不走代理，所以这里失败并不代表上游可用\
——需要代理才能出网的话，转发同样不通）",
        found.join("、")
    ))
}

/// 目录探测的互斥鉴权组合；空 Key 使用一个 None，表示不发送任何鉴权头。
#[cfg(test)]
fn provider_model_auth_modes(key: &str) -> Vec<Option<(&'static str, String)>> {
    let key = key.trim();
    if key.is_empty() {
        return vec![None];
    }
    vec![
        Some(("x-api-key", key.to_string())),
        Some(("authorization", format!("Bearer {key}"))),
        Some(("authorization", format!("x-api-key {key}"))),
    ]
}

/// 探测优先复用数据面同时发送的两种鉴权头；随后保留单头兼容重试。
fn provider_model_auth_sets(key: &str) -> Vec<Vec<(&'static str, String)>> {
    let key = key.trim();
    if key.is_empty() {
        return vec![Vec::new()];
    }
    vec![
        crate::request_build::provider_auth_headers(key),
        vec![("x-api-key", key.to_string())],
        vec![("authorization", format!("Bearer {key}"))],
        vec![("authorization", format!("x-api-key {key}"))],
    ]
}

fn extract_models(value: &Value) -> Vec<String> {
    fn visit(value: &Value, models: &mut Vec<String>, depth: usize) {
        if depth > 8 || models.len() >= MAX_MODEL_CATALOG_ITEMS {
            return;
        }
        if let Some(model) = value.as_str() {
            let model = model.trim();
            if !model.is_empty() {
                models.push(model.to_string());
            }
            return;
        }
        if let Some(items) = value.as_array() {
            for item in items {
                visit(item, models, depth + 1);
            }
            return;
        }
        let Some(object) = value.as_object() else {
            return;
        };
        let before = models.len();
        for key in ["data", "models", "result", "items", "results"] {
            if let Some(child) = object.get(key) {
                visit(child, models, depth + 1);
            }
        }
        if models.len() == before {
            for key in ["id", "name", "model", "model_id"] {
                if let Some(model) = object.get(key).and_then(Value::as_str) {
                    visit(&Value::String(model.to_string()), models, depth + 1);
                    break;
                }
            }
        }
        // Some gateways return a map keyed by model id: {"models":{"gpt-4o":{...}}}.
        if models.len() == before
            && let Some(Value::Object(entries)) = object.get("models")
        {
            for key in entries.keys() {
                visit(&Value::String(key.clone()), models, depth + 1);
            }
        }
        if models.len() == before
            && !object.is_empty()
            && ["data", "models", "result", "items", "results"]
                .iter()
                .all(|key| !object.contains_key(*key))
            && object
                .values()
                .all(|value| value.is_object() || value.is_null())
        {
            for key in object.keys() {
                models.push(key.clone());
            }
        }
    }
    let mut models = Vec::new();
    visit(value, &mut models, 0);
    models.sort();
    models.dedup();
    models.truncate(MAX_MODEL_CATALOG_ITEMS);
    models
}

/// 编辑已有入口时按需读取 provider apiKey 明文。
///
/// 与 `GET /config` 的分工:配置视图永远脱敏(那条红线不动),这里是单条、按需、
/// 需要显式指定 `endpointID` 的读取，复用同一套会话门禁并禁止缓存。
/// 只开放 provider apiKey;admin 密码与入站 authToken 仍然绝不回显。
async fn endpoint_secret(
    State(state): State<AdminState>,
    query: Result<Query<HashMap<String, String>>, QueryRejection>,
) -> Response {
    let Query(params) = match query {
        Ok(params) => params,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_query", &error.to_string());
        }
    };
    let Some(endpoint_id) = params
        .get("endpointID")
        .or_else(|| params.get("endpointId"))
        .filter(|id| !id.is_empty())
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "missing_endpoint_id",
            "需要 endpointID 参数",
        );
    };
    let config = state.inner.engine.config();
    let Some(endpoint) = config.endpoint(endpoint_id) else {
        return api_error(StatusCode::NOT_FOUND, "endpoint_not_found", endpoint_id);
    };
    let mut response = json_ok(&json!({
        "endpointID": endpoint.id,
        "apiKey": endpoint.api_key,
        "configured": !endpoint.api_key.is_empty(),
    }));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

async fn diagnostics(State(state): State<AdminState>) -> Response {
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
        "proxyListener": {"host": config.listener.host, "port": config.listener.port},
        "proxyRunning": proxy.running,
        "warnings": warnings,
        "lastError": state.inner.engine.last_error(),
        "statsWritable": state.inner.engine.stats_writable(),
        "journalctlCommand": state.inner.systemd_scope.journalctl_command(),
        "systemdScope": state.inner.systemd_scope.as_str(),
    }))
}

#[derive(Debug, Deserialize)]
struct DiagnosticCaptureBody {
    enabled: bool,
    #[serde(rename = "maxBytes")]
    max_bytes: Option<usize>,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticCaptureExportQuery {
    scope: Option<String>,
    format: Option<String>,
    privacy: Option<String>,
    #[serde(default)]
    confirm_raw: bool,
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticCaptureFormat {
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
fn parse_diagnostic_capture_format(raw: Option<&str>) -> Result<DiagnosticCaptureFormat, Response> {
    match raw.unwrap_or("jsonl") {
        "jsonl" => Ok(DiagnosticCaptureFormat::Jsonl),
        "json" => Ok(DiagnosticCaptureFormat::Json),
        _ => Err(api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_format",
            "format 只允许 jsonl 或 json",
        )),
    }
}

fn sensitive_capture_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase().replace(['-', '_'], "");
    [
        "authorization",
        "proxyauthorization",
        "cookie",
        "setcookie",
        "apikey",
        "accesstoken",
        "refreshtoken",
        "clientsecret",
        "password",
        "passwd",
        "secret",
        "privatekey",
        "signature",
        "webhooksecret",
    ]
    .iter()
    .any(|needle| key == *needle || key.contains(needle))
}

fn sensitive_header_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    sensitive_capture_key(&name)
        || name.contains("token")
        || name.contains("auth")
        || name == "proxy-authenticate"
        || name == "www-authenticate"
}

fn redact_url_query(raw: &str) -> String {
    let Some(question) = raw.find('?') else {
        return raw.to_string();
    };
    let (prefix, query_and_fragment) = raw.split_at(question + 1);
    let (query, fragment) = query_and_fragment
        .find('#')
        .map(|offset| query_and_fragment.split_at(offset))
        .unwrap_or((query_and_fragment, ""));
    let redacted = query
        .split('&')
        .map(|pair| {
            let Some(equal) = pair.find('=') else {
                return pair.to_string();
            };
            let (key, value) = pair.split_at(equal);
            if sensitive_capture_key(key) || key.to_ascii_lowercase().contains("token") {
                format!("{key}=[REDACTED]")
            } else {
                format!("{key}{value}")
            }
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("{prefix}{redacted}{fragment}")
}

fn redact_text_secrets(text: &str) -> String {
    let mut output = text.to_string();
    for marker in [
        "Bearer ",
        "Basic ",
        "token=",
        "access_token=",
        "api_key=",
        "apiKey=",
    ] {
        let mut search_from = 0;
        while let Some(relative) = output[search_from..].find(marker) {
            let start = search_from + relative + marker.len();
            let end = output[start..]
                .find(|ch: char| ch.is_whitespace() || matches!(ch, '&' | ',' | '"' | '\''))
                .map(|offset| start + offset)
                .unwrap_or(output.len());
            output.replace_range(start..end, "[REDACTED]");
            search_from = start + "[REDACTED]".len();
            if search_from >= output.len() {
                break;
            }
        }
    }
    output
}

fn redact_json_value(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if sensitive_capture_key(key) {
                    *child = Value::String("[REDACTED]".into());
                } else {
                    redact_json_value(child);
                }
            }
        }
        Value::Array(array) => array.iter_mut().for_each(redact_json_value),
        Value::String(text) => *text = redact_text_secrets(text),
        _ => {}
    }
}

fn redact_body_text(body: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(body) {
        redact_json_value(&mut value);
        serde_json::to_string(&value).unwrap_or_else(|_| "[REDACTED_BODY]".into())
    } else {
        redact_text_secrets(body)
    }
}

fn redact_capture_value(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                redact_capture_value(item);
            }
        }
        Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                match key.as_str() {
                    "inboundHeaders" | "outboundHeaders" | "responseHeaders" => {
                        if let Some(headers) = child.as_array_mut() {
                            for header in headers {
                                if let Some(header) = header.as_object_mut() {
                                    let name =
                                        header.get("name").and_then(Value::as_str).unwrap_or("");
                                    if sensitive_header_name(name) {
                                        header.insert(
                                            "value".into(),
                                            Value::String("[REDACTED]".into()),
                                        );
                                    } else if let Some(header_value) = header.get_mut("value")
                                        && let Some(text) = header_value.as_str()
                                    {
                                        *header_value = Value::String(redact_text_secrets(text));
                                    }
                                }
                            }
                        }
                    }
                    "inboundBody" | "outboundBody" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_body_text(text));
                        }
                    }
                    "outboundURL" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_url_query(text));
                        }
                    }
                    "error" | "failureDetail" => {
                        if let Some(text) = child.as_str() {
                            *child = Value::String(redact_text_secrets(text));
                        }
                    }
                    _ => redact_capture_value(child),
                }
            }
        }
        _ => {}
    }
}

fn diagnostic_capture_wire(
    record: &sumpter_core::events::DiagnosticRequestCapture,
    redacted: bool,
) -> Result<Vec<u8>, String> {
    let mut value = serde_json::to_value(record).map_err(|error| error.to_string())?;
    if redacted {
        redact_capture_value(&mut value);
    }
    serde_json::to_vec(&value).map_err(|error| error.to_string())
}

fn diagnostic_capture_response(
    body: Body,
    format: DiagnosticCaptureFormat,
    scope: &str,
    privacy: &str,
) -> Response {
    let filename = format!(
        "attachment; filename=\"sumpter-diagnostic-{scope}-{privacy}.{}\"",
        format.extension()
    );
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, format.content_type())
        .header(header::CONTENT_DISPOSITION, filename)
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("x-sumpter-privacy", privacy)
        .body(body)
        .unwrap_or_default()
}
async fn diagnostic_capture(State(state): State<AdminState>) -> Response {
    json_ok(&state.inner.engine.diagnostic_capture_index())
}
async fn diagnostic_capture_detail(
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
async fn diagnostic_capture_export(
    State(state): State<AdminState>,
    Query(query): Query<DiagnosticCaptureExportQuery>,
) -> Response {
    let Some(scope) = query.scope.as_deref() else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_scope_required",
            "导出必须明确指定 scope=current、selected 或 all",
        );
    };
    if !matches!(scope, "current" | "selected" | "all") {
        return api_error(
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
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_privacy",
            "privacy 只允许 redacted 或 raw",
        );
    }
    if privacy == "raw" && !query.confirm_raw {
        return api_error(
            StatusCode::BAD_REQUEST,
            "raw_capture_confirmation_required",
            "raw 导出必须显式确认 confirmRaw=true",
        );
    }
    if scope == "selected" && query.request_id.as_deref().is_none_or(str::is_empty) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_request_required",
            "selected 导出必须提供 requestID",
        );
    }
    if scope != "selected" && query.request_id.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "capture_request_not_allowed",
            "只有 selected 导出允许 requestID",
        );
    }
    let redacted = privacy == "redacted";
    let engine = state.inner.engine.clone();
    if scope == "current" {
        let bytes = serde_json::to_vec(&engine.diagnostic_capture_index())
            .unwrap_or_else(|_| b"{}".to_vec());
        let body = if format == DiagnosticCaptureFormat::Jsonl {
            let mut line = bytes;
            line.push(b'\n');
            Body::from(line)
        } else {
            Body::from(bytes)
        };
        return diagnostic_capture_response(body, format, scope, privacy);
    }
    if scope == "selected" {
        let request_id = query.request_id.unwrap_or_default();
        return match tokio::task::spawn_blocking(move || {
            engine.diagnostic_capture_detail(&request_id)
        })
        .await
        {
            Ok(Some(record)) => match diagnostic_capture_wire(&record, redacted) {
                Ok(mut bytes) => {
                    if format == DiagnosticCaptureFormat::Jsonl {
                        bytes.push(b'\n');
                    }
                    diagnostic_capture_response(Body::from(bytes), format, scope, privacy)
                }
                Err(error) => {
                    tracing::error!(%error, "诊断捕获详情脱敏编码失败");
                    api_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "capture_detail_encode_failed",
                        "诊断捕获详情编码失败",
                    )
                }
            },
            Ok(None) => api_error(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求"),
            Err(_) => api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "capture_detail_unavailable",
                "诊断捕获详情读取任务未能完成，请稍后重试",
            ),
        };
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let worker_engine = engine.clone();
    tokio::task::spawn_blocking(move || {
        let worker_sender = sender.clone();
        let mut first = true;
        let send = |bytes: Vec<u8>| {
            worker_sender
                .blocking_send(Ok(Bytes::from(bytes)))
                .map_err(|_| "export client disconnected".to_string())
        };
        let result = (|| {
            if format == DiagnosticCaptureFormat::Json {
                send(br#"{"records":["#.to_vec())?;
            }
            worker_engine.with_diagnostic_capture_records(|record| {
                let mut bytes = diagnostic_capture_wire(record, redacted)?;
                if format == DiagnosticCaptureFormat::Json {
                    if !first {
                        let mut comma = vec![b','];
                        comma.append(&mut bytes);
                        bytes = comma;
                    }
                    first = false;
                } else {
                    bytes.push(b'\n');
                }
                send(bytes)
            })?;
            if format == DiagnosticCaptureFormat::Json {
                send(b"]}".to_vec())?;
            }
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            let _ = worker_sender.blocking_send(Err(std::io::Error::other(error)));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    diagnostic_capture_response(Body::from_stream(stream), format, scope, privacy)
}
async fn set_diagnostic_capture(
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
async fn clear_diagnostic_capture(State(state): State<AdminState>) -> Response {
    match state.inner.engine.clear_diagnostic_capture() {
        Ok(()) => json_ok(&json!({"cleared":true})),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostic_capture_clear_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct AutostartBody {
    enabled: bool,
}

async fn get_autostart(State(state): State<AdminState>) -> Response {
    json_ok(&autostart_status(state.inner.systemd_scope).await)
}

async fn put_autostart(
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

async fn autostart_status(scope: SystemdScope) -> Value {
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

async fn api_not_found() -> Response {
    api_error(StatusCode::NOT_FOUND, "not_found", "unknown admin endpoint")
}

fn json_ok(body: &Value) -> Response {
    crate::engine::json_response(StatusCode::OK, body)
}

fn api_error(status: StatusCode, code: &str, message: &str) -> Response {
    crate::engine::json_response(status, &json!({"error": code, "message": message}))
}

struct ApiFailure {
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
            pinned_ip_exclusive: false,
            pinned_ips: vec![],
            priority: 0,
            protocol: EndpointProtocolMode::Anthropic,
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
        assert_eq!(storage["schemaVersion"], 3);
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
