//! admin API(sumpterd 专属,SwiftUI 壳的控制通道)。
//! 鉴权:**loopback + `X-Control-Token`**(值同 `.control_token`);非环回一律 403。
//! 端点镜像 schema v5,设计参考 PLAN.md §5。

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{ConnectInfo, Json, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Request, StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use serde::de::{self, Deserializer};
use serde_json::{Value, json};

use crate::engine::{Engine, EngineNotice};
use crate::health;
use crate::runtime_query::{
    DimensionKind, DimensionPageQuery, DimensionSort, ErrorPageQuery, EventPageQuery, ExportFormat,
    ExportPrivacy, ExportQuery, ExportScope, RuntimeFilter, RuntimeQueryError, SortOrder,
    TrendGranularity, TrendQuery,
};
use crate::runtime_store::{
    AnalyticsFilter, RuntimeModelPriceInput, RuntimePricingUpdate, RuntimeRetentionUpdate,
};
use sumpter_engine::PlatformNotice;

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

pub fn admin_router(engine: Engine) -> Router {
    Router::new()
        .route("/admin/status", get(status))
        .route("/admin/runtime/summary", get(runtime_summary))
        .route("/admin/runtime/events", get(runtime_events))
        .route("/admin/runtime/events/{id}", get(runtime_event_detail))
        .route("/admin/runtime/request-chain", get(runtime_request_chain))
        .route("/admin/runtime/analytics", get(runtime_analytics))
        .route("/admin/runtime/facets", get(runtime_facets))
        .route("/admin/runtime/trends", get(runtime_trends))
        .route("/admin/runtime/errors", get(runtime_errors))
        .route("/admin/runtime/dimensions", get(runtime_dimensions))
        .route("/admin/runtime/projects", get(runtime_projects))
        .route("/admin/runtime/sessions", get(runtime_sessions))
        .route("/admin/runtime/storage", get(runtime_storage))
        .route(
            "/admin/runtime/retention",
            get(runtime_retention).put(runtime_retention_update),
        )
        .route(
            "/admin/runtime/pricing",
            get(runtime_pricing).put(runtime_pricing_update),
        )
        .route(
            "/admin/runtime/export/estimate",
            get(runtime_export_estimate),
        )
        .route("/admin/runtime/export", get(runtime_export))
        .route(
            "/admin/runtime/session",
            axum::routing::delete(delete_runtime_session),
        )
        .route("/admin/runtime/session/export", get(export_runtime_session))
        .route(
            "/admin/runtime/projects/sticky-clear",
            post(clear_project_sticky),
        )
        .route(
            "/admin/runtime/sessions/sticky-clear",
            post(clear_runtime_session_sticky),
        )
        .route(
            "/admin/runtime/cleanup/preview",
            post(runtime_cleanup_preview),
        )
        .route("/admin/runtime/cleanup", post(runtime_cleanup))
        .route("/admin/runtime/reset", post(reset_runtime))
        .route("/admin/runtime/recreate", post(recreate_runtime))
        .route("/admin/reload", post(reload))
        .route(
            "/admin/provider-models",
            get(provider_models_get).post(provider_models),
        )
        .route("/admin/events", get(events))
        .route("/admin/diagnostics", get(diagnostics))
        .route(
            "/admin/diagnostic-capture",
            get(diagnostic_capture)
                .put(set_diagnostic_capture)
                .delete(clear_diagnostic_capture),
        )
        .route(
            "/admin/diagnostic-capture/export",
            get(diagnostic_capture_export),
        )
        .route(
            "/admin/diagnostic-capture/{request_id}",
            get(diagnostic_capture_detail),
        )
        .fallback(not_found)
        .layer(axum::middleware::from_fn_with_state(engine.clone(), guard))
        .with_state(engine)
}

/// loopback + token 双门禁。
async fn guard(
    State(engine): State<Engine>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: axum::middleware::Next,
) -> Response {
    if let Some((code, message)) = admin_access_error(&engine, remote, request.headers()) {
        return error(StatusCode::FORBIDDEN, code, message);
    }
    next.run(request).await
}

fn admin_access_error(
    engine: &Engine,
    remote: SocketAddr,
    headers: &HeaderMap,
) -> Option<(&'static str, &'static str)> {
    if !remote.ip().is_loopback() {
        return Some(("loopback_only", "admin API 仅限本机访问"));
    }
    let token_ok = headers
        .get("x-control-token")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == engine.control_token());
    (!token_ok).then_some(("bad_token", "X-Control-Token 缺失或不匹配"))
}

async fn status(State(engine): State<Engine>) -> Response {
    let config = engine.config();
    let runtime = engine.runtime_snapshot();
    let health = health::evaluate(
        &runtime.recent_events,
        engine.runtime_database_issue().is_none(),
    );
    json_ok(&json!({
        "running": engine.runtime_database_issue().is_none(),
        "runtimeDatabaseIssue": engine.runtime_database_issue(),
        "runtimeApiVersion": 1,
        "generation": engine.generation(),
        "uptimeSeconds": engine.uptime_seconds(),
        "listener": {
            "host": config.listener.host,
            "port": config.listener.port,
            "allowedCIDRs": config.listener.allowed_cidrs,
            "hasAuthToken": !config.listener.auth_token.is_empty(),
        },
        "providers": config.endpoints.len(),
        "endpoints": config.endpoints.len(),
        "counters": {
            "clientRequests": runtime.client_requests,
            "clientSuccesses": runtime.client_successes,
            "clientFailures": runtime.client_failures,
            "upstreamAttempts": runtime.upstream_attempts,
            "failovers": runtime.failovers,
        },
        "health": health,
        "lastError": engine.last_error(),
        "statsWritable": engine.stats_writable(),
    }))
}

async fn runtime_summary(State(engine): State<Engine>) -> Response {
    json_ok(&engine.runtime_summary_value())
}

async fn reload(State(engine): State<Engine>) -> Response {
    match engine.reload_config() {
        Ok((generation, warnings)) => json_ok(&json!({
            "generation": generation,
            "warnings": warnings,
        })),
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, "reload_failed", &message),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderModelsBody {
    #[serde(rename = "endpointID", alias = "endpointId")]
    endpoint_id: String,
}

#[derive(Debug, Deserialize)]
struct ProviderModelsQuery {
    #[serde(rename = "endpoint", alias = "endpointID", alias = "endpointId")]
    endpoint_id: String,
}

async fn provider_models_get(
    State(engine): State<Engine>,
    query: Result<Query<ProviderModelsQuery>, axum::extract::rejection::QueryRejection>,
) -> Response {
    let Query(query) = match query {
        Ok(value) => value,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_query",
                &rejection.to_string(),
            );
        }
    };
    provider_models_for_endpoint(engine, query.endpoint_id).await
}

async fn provider_models(
    State(engine): State<Engine>,
    payload: Result<Json<ProviderModelsBody>, JsonRejection>,
) -> Response {
    let Json(body) = match payload {
        Ok(value) => value,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &rejection.to_string(),
            );
        }
    };
    provider_models_for_endpoint(engine, body.endpoint_id).await
}

async fn provider_models_for_endpoint(engine: Engine, endpoint_id: String) -> Response {
    let config = engine.config();
    let Some(endpoint) = config.endpoint(&endpoint_id).cloned() else {
        return error(StatusCode::NOT_FOUND, "endpoint_not_found", &endpoint_id);
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
        Err(message) => error(StatusCode::BAD_GATEWAY, "models_fetch_failed", &message),
    }
}

async fn fetch_provider_models(
    endpoint: &sumpter_core::config::Endpoint,
) -> Result<(Vec<String>, String), String> {
    tokio::time::timeout(
        Duration::from_secs(12),
        fetch_provider_models_inner(endpoint),
    )
    .await
    .map_err(|_| "获取模型整体超时（12 秒）".to_string())?
}

async fn fetch_provider_models_inner(
    endpoint: &sumpter_core::config::Endpoint,
) -> Result<(Vec<String>, String), String> {
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
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .no_proxy()
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| error.to_string())?;
    let mut errors = Vec::new();
    let paths = model_catalog_paths(&base);
    let deadline = std::time::Instant::now() + Duration::from_secs(12);
    'probes: for auth in provider_model_auth_sets(key) {
        for path in &paths {
            for user_agent in
                crate::request_build::probe_user_agents(endpoint.protocol, &endpoint.user_agent)
            {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    errors.push("整体超时，已停止尝试".into());
                    break 'probes;
                }
                let mut url = base.clone();
                url.set_path(path);
                url.set_query(None);
                url.set_fragment(None);
                let mut request = client.get(url.clone());
                for (header, value) in
                    crate::request_build::provider_probe_headers_with_user_agent("", &user_agent)
                {
                    request = request.header(header, value);
                }
                for (header, value) in &auth {
                    request = request.header(*header, value.as_str());
                }
                request = request.timeout(remaining.min(Duration::from_secs(3)));
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
    errors.dedup();
    Err(errors.into_iter().take(4).collect::<Vec<_>>().join("；"))
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

fn model_catalog_paths(base: &reqwest::Url) -> Vec<String> {
    let prefix = base.path().trim_end_matches('/');
    let mut paths = Vec::new();
    let mut append = |path: &str| {
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
    append("/v1/models");
    append("/models");
    append("/v1/model/list");
    if prefix.is_empty() {
        append("/api/v1/models");
    }
    paths
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
    models.retain(|model| !model.is_empty());
    models.truncate(MAX_MODEL_CATALOG_ITEMS);
    models
}

#[derive(Debug, serde::Deserialize)]
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
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
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
    State(engine): State<Engine>,
    Query(query): Query<RuntimeEventsQuery>,
) -> Response {
    if query.view.as_deref() == Some("page") {
        if query.before_seq.is_some() || query.after_change_seq.is_some() {
            return error(
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
        return match engine.runtime_events_page(&request) {
            Ok(value) => json_ok(&value),
            Err(error) => runtime_query_error_response(error),
        };
    }
    if query.view.is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_runtime_query",
            "view 只支持 page",
        );
    }
    match engine.runtime_events(
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
        Err(message) => error(
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
        client_variant: query.client_variant.clone(),
        agent_role: query.agent_role.clone(),
        agent_name: query.agent_name.clone(),
        parent_thread_id: query.parent_thread_id.clone(),
        parent_turn_id: query.parent_turn_id.clone(),
        root_turn_id: query.root_turn_id.clone(),
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

async fn runtime_event_detail(State(engine): State<Engine>, Path(id): Path<String>) -> Response {
    match engine.runtime_event(&id) {
        Ok(Some(value)) => json_ok(&value),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "runtime_event_not_found",
            "运行事件不存在",
        ),
        Err(message) => error(
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
    State(engine): State<Engine>,
    Query(query): Query<RuntimeRequestChainQuery>,
) -> Response {
    let Some(request_id) = query
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return error(
            StatusCode::BAD_REQUEST,
            "request_id_required",
            "必须提供 requestID",
        );
    };
    match engine.runtime_request_chain(request_id) {
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
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
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
            client_variant: query.client_variant,
            agent_role: query.agent_role,
            agent_name: query.agent_name,
            parent_thread_id: query.parent_thread_id,
            parent_turn_id: query.parent_turn_id,
            root_turn_id: query.root_turn_id,
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
    State(engine): State<Engine>,
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
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "range 必须是 today、1h、24h、7d、30d 或 all",
            );
        }
    };
    let mut filter: RuntimeFilter = query.filters.into();
    filter.from = crate::runtime_query::merge_range_lower_bound(range, filter.from, range_from);
    filter.to = Some(filter.to.map_or(now, |value| value.min(now)));
    match engine.runtime_facets(&filter) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_trends(
    State(engine): State<Engine>,
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
            return error(
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
            return error(
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
    match engine.runtime_trends(&request) {
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
    State(engine): State<Engine>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let request = ErrorPageQuery {
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(10),
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    };
    match engine.runtime_error_groups(&request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_projects(
    State(engine): State<Engine>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(engine, DimensionKind::Project, query)
}

async fn runtime_sessions(
    State(engine): State<Engine>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(engine, DimensionKind::Session, query)
}

async fn runtime_dimensions(
    State(engine): State<Engine>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let Some(kind) = parse_dimension_kind(query.filters.kind.as_deref()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_dimension_kind",
            "kind 必须是 endpoint、model、clientKind、purpose、failureKind、failurePhase、protocol、streamTerminal、project 或 session",
        );
    };
    let mut query = query;
    query.filters.kind = None;
    runtime_dimension_response(engine, kind, query)
}

fn parse_dimension_kind(value: Option<&str>) -> Option<DimensionKind> {
    match value? {
        "endpoint" => Some(DimensionKind::Endpoint),
        "model" => Some(DimensionKind::Model),
        "clientKind" | "client_kind" => Some(DimensionKind::ClientKind),
        "clientVariant" | "client_variant" => Some(DimensionKind::ClientVariant),
        "agentRole" | "agent_role" => Some(DimensionKind::AgentRole),
        "agentName" | "agent_name" => Some(DimensionKind::AgentName),
        "parentThread" | "parent_thread_id" => Some(DimensionKind::ParentThread),
        "parentTurn" | "parent_turn_id" => Some(DimensionKind::ParentTurn),
        "rootTurn" | "root_turn_id" => Some(DimensionKind::RootTurn),
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
    engine: Engine,
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
            return error(
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
            return error(
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
    match engine.runtime_dimension_page(kind, &request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_storage(State(engine): State<Engine>) -> Response {
    match engine.runtime_storage_details() {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_retention(State(engine): State<Engine>) -> Response {
    match engine.runtime_storage_details() {
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
    older_than: f64,
}

async fn runtime_cleanup_preview(
    State(engine): State<Engine>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match engine.runtime_cleanup_preview(payload.older_than) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

async fn runtime_cleanup(
    State(engine): State<Engine>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match engine.runtime_cleanup(payload.older_than) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

async fn runtime_retention_update(
    State(engine): State<Engine>,
    payload: JsonPayload<RuntimeRetentionPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match engine.runtime_set_retention(RuntimeRetentionUpdate {
        expected_revision: payload.expected_revision,
        max_age_days: payload.max_age_days,
        storage_limit_bytes: payload.storage_limit_bytes,
    }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            error(StatusCode::BAD_REQUEST, "invalid_retention", &message)
        }
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_retention_failed",
            &message,
        ),
    }
}

async fn runtime_pricing(State(engine): State<Engine>) -> Response {
    match engine.runtime_pricing() {
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
    State(engine): State<Engine>,
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
    match engine.runtime_replace_pricing(RuntimePricingUpdate {
        expected_revision: payload.expected_revision,
        currency: payload.currency,
        prices,
    }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            error(StatusCode::BAD_REQUEST, "invalid_pricing", &message)
        }
        Err(message) => error(
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
            return Err(error(
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
            return Err(error(
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
            return Err(error(
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
    State(engine): State<Engine>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    match engine.runtime_export_estimate(&query) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

async fn runtime_export(
    State(engine): State<Engine>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if query.privacy == ExportPrivacy::Stored && !query.confirm_stored {
        return error(
            StatusCode::BAD_REQUEST,
            "stored_export_confirmation_required",
            "privacy=stored 必须同时提供 confirmStored=true",
        );
    }
    // Resolve and validate the exact snapshot before response headers are
    // committed. The streaming worker reuses these values, so estimate and
    // export cannot drift to a newer history window.
    let estimate = match engine.runtime_export_estimate(&query) {
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
    let engine = engine.clone();
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

fn runtime_query_error_response(query_error: RuntimeQueryError) -> Response {
    match query_error {
        RuntimeQueryError::InvalidInput(message) => {
            error(StatusCode::BAD_REQUEST, "invalid_runtime_query", &message)
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
            error(StatusCode::NOT_FOUND, "runtime_not_found", &message)
        }
        RuntimeQueryError::ProjectionNotReady { backfill_cursor } => crate::engine::json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "error": "runtime_projection_not_ready",
                "message": "历史数据索引正在后台补齐，请稍后重试",
                "backfillCursor": backfill_cursor,
            }),
        ),
        RuntimeQueryError::CorruptPayload { event_id, detail } => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_payload_corrupt",
            &format!("事件 {event_id} 无法读取: {detail}"),
        ),
        RuntimeQueryError::Output(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_export_failed",
            &message,
        ),
        RuntimeQueryError::Sql(sql_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &sql_error.to_string(),
        ),
    }
}

#[derive(Debug, serde::Deserialize)]
struct RuntimeAnalyticsQuery {
    range: Option<String>,
    #[serde(rename = "clientKind", alias = "client_kind")]
    client_kind: Option<String>,
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
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
    State(engine): State<Engine>,
    Query(query): Query<RuntimeAnalyticsQuery>,
) -> Response {
    let range = query.range.as_deref().unwrap_or("24h");
    if !matches!(range, "today" | "1h" | "24h" | "7d" | "30d" | "all") {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "range 必须是 today、1h、24h、7d、30d 或 all",
        );
    }
    let filter = AnalyticsFilter {
        client_kind: analytics_filter_value(query.client_kind),
        client_variant: analytics_filter_value(query.client_variant),
        agent_role: analytics_filter_value(query.agent_role),
        agent_name: analytics_filter_value(query.agent_name),
        parent_thread_id: analytics_filter_value(query.parent_thread_id),
        parent_turn_id: analytics_filter_value(query.parent_turn_id),
        root_turn_id: analytics_filter_value(query.root_turn_id),
        endpoint_id: analytics_filter_value(query.endpoint_id),
        project_id: analytics_filter_value(query.project_id),
        project: analytics_filter_value(query.project),
        session_id: analytics_filter_value(query.session_id),
        from: query
            .from
            .or_else(|| (range == "today").then(|| utc_today_start(admin_apple_timestamp()))),
        to: query.to,
    };
    match engine.runtime_analytics_filtered(range, &filter) {
        Ok(value) => json_ok(&value),
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

#[derive(Debug, serde::Deserialize)]
struct RuntimeSessionQuery {
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "confirmUnidentified", alias = "confirm_unidentified")]
    confirm_unidentified: Option<bool>,
}

fn session_query_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

async fn delete_runtime_session(
    State(engine): State<Engine>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = session_query_value(query.session_id) else {
        return error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match engine
        .delete_runtime_session_confirmed(&session_id, query.confirm_unidentified == Some(true))
    {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("不能删除未识别会话") => {
            error(StatusCode::BAD_REQUEST, "session_not_deletable", &message)
        }
        Err(message) if message.contains("Query returned no rows") => {
            error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_delete_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct ProjectStickyBody {
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: String,
}

async fn clear_project_sticky(
    State(engine): State<Engine>,
    payload: Result<Json<ProjectStickyBody>, JsonRejection>,
) -> Response {
    let Json(body) = match payload {
        Ok(value) => value,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &rejection.to_string(),
            );
        }
    };
    let project_id = body.project_id.trim().to_owned();
    if project_id.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "project_id_required",
            "必须提供 projectID",
        );
    }
    match engine.clear_project_sticky(&project_id) {
        Ok(value) => json_ok(&value),
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "sticky_clear_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
struct SessionStickyBody {
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: String,
}

async fn clear_runtime_session_sticky(
    State(engine): State<Engine>,
    payload: Result<Json<SessionStickyBody>, JsonRejection>,
) -> Response {
    let Json(body) = match payload {
        Ok(value) => value,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                &rejection.to_string(),
            );
        }
    };
    let session_id = body.session_id.trim().to_owned();
    if session_id.is_empty() || session_id == "unidentified_session" {
        return error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供已识别会话的完整 sessionID/threadID",
        );
    }
    match engine.clear_runtime_session_sticky(&session_id) {
        Ok(value) => json_ok(&value),
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "sticky_clear_failed",
            &message,
        ),
    }
}

async fn export_runtime_session(
    State(engine): State<Engine>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = session_query_value(query.session_id) else {
        return error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match engine.export_runtime_session(&session_id) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("会话不存在") => {
            error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_export_failed",
            &message,
        ),
    }
}

async fn reset_runtime(State(engine): State<Engine>) -> Response {
    match engine.reset_runtime() {
        Ok(_) => json_ok(
            &json!({"reset": true, "resetGeneration": engine.runtime_summary_value()["resetGeneration"]}),
        ),
        Err(message) => error(StatusCode::CONFLICT, "stats_protected", &message),
    }
}

async fn recreate_runtime(State(engine): State<Engine>) -> Response {
    match engine.recreate_runtime() {
        Ok(generation) => json_ok(&json!({
            "reset": true,
            "recreated": true,
            "resetGeneration": generation,
        })),
        Err(message) => error(StatusCode::CONFLICT, "stats_recreate_failed", &message),
    }
}

/// SSE 事件流:runtime_event / notify / config_reloaded / stats_reset。
async fn events(
    State(engine): State<Engine>,
) -> Sse<impl futures_util::Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = engine.subscribe();
    let initial_migration_notice = engine.migration_notice();
    let stream = futures_util::stream::unfold(
        (initial_migration_notice, rx),
        move |(mut initial_migration_notice, mut rx)| {
            let engine = engine.clone();
            async move {
                if let Some(notice) = initial_migration_notice.take() {
                    let event = SseEvent::default()
                        .event("migration_notice")
                        .data(serde_json::to_string(&notice).unwrap_or_else(|_| "null".into()));
                    return Some((Ok::<_, Infallible>(event), (initial_migration_notice, rx)));
                }
                loop {
                    match rx.recv().await {
                        Ok(notice) => {
                            let (name, data) = match notice {
                                EngineNotice::Event(_) => continue,
                                EngineNotice::RuntimeChange {
                                    seq,
                                    change_seq,
                                    event,
                                } => (
                                    "runtime-change",
                                    json!({"seq": seq, "changeSeq": change_seq, "event": event}),
                                ),
                                EngineNotice::PlatformNotice(PlatformNotice::Notify {
                                    hook_event,
                                    client_kind,
                                    title,
                                    message,
                                    sound,
                                    category,
                                    priority,
                                    action_id,
                                    session_id,
                                    cwd,
                                    ..
                                }) => (
                                    "notify",
                                    json!({
                                        "clientKind": client_kind.as_str(),
                                        "title": title,
                                        "message": message,
                                        "sound": sound,
                                        "type": "notification",
                                        "hookEvent": hook_event,
                                        "category": category,
                                        "priority": priority,
                                        "actionID": action_id,
                                        "sessionId": session_id,
                                        "cwd": cwd,
                                    }),
                                ),
                                EngineNotice::PlatformNotice(PlatformNotice::Migration(_)) => {
                                    continue;
                                }
                                EngineNotice::ConfigReloaded { generation } => {
                                    ("config_reloaded", json!({"generation": generation}))
                                }
                                EngineNotice::StatsReset => (
                                    "stats-reset",
                                    json!({"resetGeneration": engine.runtime_summary_value()["resetGeneration"]}),
                                ),
                                EngineNotice::ProxyState { .. } => continue,
                            };
                            let event = SseEvent::default().event(name).data(data.to_string());
                            let event = if name == "runtime-change" {
                                event.id(data["changeSeq"].as_i64().unwrap_or_default().to_string())
                            } else {
                                event
                            };
                            return Some((
                                Ok::<_, Infallible>(event),
                                (initial_migration_notice, rx),
                            ));
                        }
                        // A lagged receiver has an unknown gap. Close the SSE
                        // stream so the client reconnect path can use
                        // afterChangeSeq and perform an explicit cursor check.
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => return None,
                        Err(_) => return None,
                    }
                }
            }
        },
    );
    Sse::new(stream).keep_alive(KeepAlive::default())
}

async fn diagnostics(State(engine): State<Engine>) -> Response {
    let config = engine.config();
    let warnings = sumpter_core::warnings::evaluate(&config);
    json_ok(&json!({
        "version": env!("CARGO_PKG_VERSION"),
        "uptimeSeconds": engine.uptime_seconds(),
        "configPath": engine
            .config_dir()
            .map(|d| d.config_path().to_string_lossy().to_string()),
        "generation": engine.generation(),
        "warnings": warnings,
    }))
}

#[derive(Debug, serde::Deserialize)]
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
        _ => Err(error(
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
    let mut value =
        serde_json::to_value(record).map_err(|encode_error| encode_error.to_string())?;
    if redacted {
        redact_capture_value(&mut value);
    }
    serde_json::to_vec(&value).map_err(|encode_error| encode_error.to_string())
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

async fn diagnostic_capture(State(engine): State<Engine>) -> Response {
    json_ok(&engine.diagnostic_capture_index())
}
async fn diagnostic_capture_detail(
    State(engine): State<Engine>,
    Path(request_id): Path<String>,
) -> Response {
    // A selected record can approach the configured capture limit. Keep the
    // lock/read and serde allocation off the async runtime worker so one large
    // detail request cannot stall index refreshes or other Admin endpoints.
    match tokio::task::spawn_blocking(move || engine.diagnostic_capture_detail_json(&request_id))
        .await
    {
        Ok(Ok(Some(body))) => crate::engine::json_bytes_response(StatusCode::OK, body),
        Ok(Ok(None)) => error(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求"),
        Ok(Err(encode_error)) => {
            tracing::error!(%encode_error, "诊断捕获详情 JSON 编码失败");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "capture_detail_encode_failed",
                "诊断捕获详情编码失败",
            )
        }
        Err(join_error) => {
            tracing::error!(%join_error, "诊断捕获详情读取任务异常");
            error(
                StatusCode::SERVICE_UNAVAILABLE,
                "capture_detail_unavailable",
                "诊断捕获详情读取任务未能完成，请稍后重试",
            )
        }
    }
}

/// 按记录流式编码并使用有界 channel 背压。默认是源数据；只有调用方主动
/// 选择 `privacy=redacted` 才生成脱敏副本。raw 仍要求显式风险确认。
async fn diagnostic_capture_export(
    State(engine): State<Engine>,
    Query(query): Query<DiagnosticCaptureExportQuery>,
) -> Response {
    let Some(scope) = query.scope.as_deref() else {
        return error(
            StatusCode::BAD_REQUEST,
            "capture_scope_required",
            "导出必须明确指定 scope=current、selected 或 all",
        );
    };
    if !matches!(scope, "current" | "selected" | "all") {
        return error(
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
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_privacy",
            "privacy 只允许 redacted 或 raw",
        );
    }
    if privacy == "raw" && !query.confirm_raw {
        return error(
            StatusCode::BAD_REQUEST,
            "raw_capture_confirmation_required",
            "raw 导出必须显式确认 confirmRaw=true",
        );
    }
    if scope == "selected" && query.request_id.as_deref().is_none_or(str::is_empty) {
        return error(
            StatusCode::BAD_REQUEST,
            "capture_request_required",
            "selected 导出必须提供 requestID",
        );
    }
    if scope != "selected" && query.request_id.is_some() {
        return error(
            StatusCode::BAD_REQUEST,
            "capture_request_not_allowed",
            "只有 selected 导出允许 requestID",
        );
    }

    let redacted = privacy == "redacted";
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
        return match tokio::task::spawn_blocking({
            let engine = engine.clone();
            move || engine.diagnostic_capture_detail(&request_id)
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
                Err(encode_error) => {
                    tracing::error!(%encode_error, "诊断捕获详情编码失败");
                    error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "capture_detail_encode_failed",
                        "诊断捕获详情编码失败",
                    )
                }
            },
            Ok(None) => error(StatusCode::NOT_FOUND, "capture_not_found", "未找到抓包请求"),
            Err(join_error) => {
                tracing::error!(%join_error, "诊断捕获详情读取任务异常");
                error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "capture_detail_unavailable",
                    "诊断捕获详情读取任务未能完成，请稍后重试",
                )
            }
        };
    }

    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
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
            engine.with_diagnostic_capture_records(|record| {
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
        if let Err(export_error) = result {
            let _ = worker_sender.blocking_send(Err(std::io::Error::other(export_error)));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    diagnostic_capture_response(Body::from_stream(stream), format, scope, privacy)
}
async fn set_diagnostic_capture(
    State(engine): State<Engine>,
    axum::Json(body): axum::Json<DiagnosticCaptureBody>,
) -> Response {
    if body.enabled && body.max_bytes == Some(0) {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_capture_capacity",
            "maxBytes 必须大于 0",
        );
    };
    engine.set_diagnostic_capture(body.enabled, body.max_bytes);
    json_ok(&engine.diagnostic_capture_index())
}
async fn clear_diagnostic_capture(State(engine): State<Engine>) -> Response {
    match engine.clear_diagnostic_capture() {
        Ok(()) => json_ok(&json!({"cleared":true})),
        Err(message) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "diagnostic_capture_clear_failed",
            &message,
        ),
    }
}

async fn not_found() -> Response {
    error(StatusCode::NOT_FOUND, "not_found", "unknown admin endpoint")
}

fn json_ok(body: &Value) -> Response {
    crate::engine::json_response(StatusCode::OK, body)
}

type JsonPayload<T> = Result<Json<T>, JsonRejection>;

#[allow(clippy::result_large_err)]
fn require_json<T>(payload: JsonPayload<T>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        error(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &rejection.to_string(),
        )
    })
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    crate::engine::error_response(status, &[("error", code), ("message", message)]).into_response()
}

#[allow(dead_code)]
fn _headers(_: HeaderMap) {}

#[cfg(test)]
mod tests {
    use futures_util::StreamExt;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    use super::*;
    use sumpter_core::config::AppConfig;
    use sumpter_core::config_store::ConfigDir;
    use sumpter_core::events::ClientKind;
    use sumpter_engine::boundary::EngineCapabilities;

    fn test_engine() -> Engine {
        Engine::new(
            AppConfig::bootstrap().normalized(),
            None,
            Arc::new(crate::outbound::ReqwestTransport::new()),
            "test-control-token".into(),
        )
    }

    /// sticky-clear:loopback + token 门禁之下的运维动作;空库走 200 空计数,
    /// 真实清除链路由 engine 集成测试覆盖。
    #[tokio::test]
    async fn project_sticky_clear_rejects_bad_token_and_empty_project_id() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-macos-admin-sticky-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let engine = Engine::new(
            AppConfig::bootstrap().normalized(),
            Some(ConfigDir::new(root.clone())),
            Arc::new(crate::outbound::ReqwestTransport::new()),
            "sticky-control-token".into(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(engine),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/runtime/projects/sticky-clear");

        let bad_token = client
            .post(&url)
            .header("x-control-token", "wrong-token")
            .header("content-type", "application/json")
            .body(r#"{"projectID":"sha256:abc"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(bad_token.status(), StatusCode::FORBIDDEN);

        let empty_id = client
            .post(&url)
            .header("x-control-token", "sticky-control-token")
            .header("content-type", "application/json")
            .body(r#"{"projectID":"  "}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(empty_id.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value =
            serde_json::from_slice(&empty_id.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"], "project_id_required");

        let ok = client
            .post(&url)
            .header("x-control-token", "sticky-control-token")
            .header("content-type", "application/json")
            .body(r#"{"projectID":"sha256:synthetic"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&ok.bytes().await.unwrap()).unwrap();
        assert_eq!(body["cleared"], 0);
        assert_eq!(body["matched"], 0);

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn session_sticky_clear_rejects_bad_token_and_empty_session_id() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-macos-admin-sticky-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let engine = Engine::new(
            AppConfig::bootstrap().normalized(),
            Some(ConfigDir::new(root.clone())),
            Arc::new(crate::outbound::ReqwestTransport::new()),
            "sticky-control-token".into(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(engine),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let url = format!("http://{address}/admin/runtime/sessions/sticky-clear");

        let bad_token = client
            .post(&url)
            .header("x-control-token", "wrong-token")
            .header("content-type", "application/json")
            .body(r#"{"sessionID":"sha256:abc"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(bad_token.status(), StatusCode::FORBIDDEN);

        let empty_id = client
            .post(&url)
            .header("x-control-token", "sticky-control-token")
            .header("content-type", "application/json")
            .body(r#"{"sessionID":"  "}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(empty_id.status(), StatusCode::BAD_REQUEST);
        let body: serde_json::Value =
            serde_json::from_slice(&empty_id.bytes().await.unwrap()).unwrap();
        assert_eq!(body["error"], "session_id_required");

        for body in [
            r#"{}"#,
            r#"{"sessionID":"unidentified_session"}"#,
            "invalid-json",
        ] {
            let response = client
                .post(&url)
                .header("content-type", "application/json")
                .header("x-control-token", "sticky-control-token")
                .body(body)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }

        let ok = client
            .post(&url)
            .header("x-control-token", "sticky-control-token")
            .header("content-type", "application/json")
            .body(r#"{"sessionID":"sha256:synthetic"}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(ok.status(), StatusCode::OK);
        let body: serde_json::Value = serde_json::from_slice(&ok.bytes().await.unwrap()).unwrap();
        assert_eq!(body["cleared"], 0);
        assert_eq!(body["matched"], 0);

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn admin_access_requires_loopback_and_the_exact_control_token() {
        let engine = test_engine();
        let mut headers = HeaderMap::new();
        headers.insert("x-control-token", "test-control-token".parse().unwrap());
        assert!(
            admin_access_error(
                &engine,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345),
                &headers,
            )
            .is_none()
        );
        assert_eq!(
            admin_access_error(
                &engine,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10)), 12345),
                &headers,
            )
            .map(|value| value.0),
            Some("loopback_only")
        );
        headers.insert("x-control-token", "wrong-token".parse().unwrap());
        assert_eq!(
            admin_access_error(
                &engine,
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 12345),
                &headers,
            )
            .map(|value| value.0),
            Some("bad_token")
        );
    }

    #[tokio::test]
    async fn admin_events_include_notify_client_kind() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-macos-admin-events-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let engine = Engine::new(
            AppConfig::bootstrap().normalized(),
            Some(ConfigDir::new(root.clone())),
            Arc::new(crate::outbound::ReqwestTransport::new()),
            "events-control-token".into(),
        );
        let (address, server) = crate::server::serve_router(
            admin_router(engine.clone()),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let response = client
            .get(format!("http://{address}/admin/events"))
            .header("x-control-token", "events-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut stream = response.bytes_stream();

        EngineCapabilities::publish_platform_notice(
            &engine,
            PlatformNotice::Notify {
                client_kind: ClientKind::Codex,
                kind: "stop".into(),
                hook_event: Some("Stop".into()),
                title: "Codex CLI · 回合完成".into(),
                message: "Codex CLI 主回合已完成".into(),
                sound: None,
                category: Some("turn_completed".into()),
                priority: Some("normal".into()),
                action_id: None,
                session_id: Some("session-1".into()),
                cwd: None,
            },
        );
        let chunk = tokio::time::timeout(Duration::from_secs(1), stream.next())
            .await
            .expect("SSE notify should arrive")
            .expect("SSE stream should stay open")
            .expect("SSE chunk should be readable");
        let text = String::from_utf8_lossy(&chunk);
        assert!(text.contains("event: notify"), "{text}");
        assert!(text.contains("\"clientKind\":\"codex\""), "{text}");
        assert!(text.contains("\"hookEvent\":\"Stop\""), "{text}");
        assert!(text.contains("Codex CLI 主回合已完成"), "{text}");

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn model_catalog_shapes_collect_every_model() {
        assert_eq!(
            extract_models(&json!({
                "data": [{"id": "gpt-a"}, {"id": "gpt-b"}, {"id": "gpt-c"}]
            })),
            vec!["gpt-a", "gpt-b", "gpt-c"]
        );
        assert_eq!(
            extract_models(&json!({
                "data": [{"type": "model", "id": "claude-opus"}, {"type": "model", "id": "claude-sonnet"}]
            })),
            vec!["claude-opus", "claude-sonnet"]
        );
        assert_eq!(extract_models(&json!(["m1", "m2", "m1"])), vec!["m1", "m2"]);
        assert_eq!(
            extract_models(&json!({"models": {"gpt-4o": {}, "gpt-4o-mini": {}}})),
            vec!["gpt-4o", "gpt-4o-mini"]
        );
        assert_eq!(
            extract_models(&json!({
                "result": {"data": [{"name": "nested-a"}, {"model": "nested-b"}]}
            })),
            vec!["nested-a", "nested-b"]
        );
    }

    #[test]
    fn model_catalog_limits_depth_and_item_count() {
        let mut nested = json!("too-deep");
        for _ in 0..10 {
            nested = json!({"data": nested});
        }
        assert!(extract_models(&nested).is_empty());

        let many = (0..=MAX_MODEL_CATALOG_ITEMS)
            .map(|index| json!({"id": format!("model-{index:05}")}))
            .collect::<Vec<_>>();
        assert_eq!(
            extract_models(&json!({"data": many})).len(),
            MAX_MODEL_CATALOG_ITEMS
        );
    }

    #[test]
    fn model_catalog_paths_and_authentication_are_compatible() {
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

    #[tokio::test]
    async fn runtime_api_v1_replaces_legacy_contract() {
        let root = std::env::temp_dir().join(format!(
            "sumpter-macos-admin-runtime-v1-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let engine = Engine::new(
            AppConfig::bootstrap().normalized(),
            Some(ConfigDir::new(root.clone())),
            Arc::new(crate::outbound::ReqwestTransport::new()),
            "runtime-control-token".into(),
        );
        engine.flush_diagnostic_capture().unwrap();
        let (address, server) = crate::server::serve_router(
            admin_router(engine),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        )
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let base = format!("http://{address}");

        let export_without_token = client
            .get(format!("{base}/admin/diagnostic-capture/export"))
            .send()
            .await
            .unwrap();
        assert_eq!(export_without_token.status(), StatusCode::FORBIDDEN);
        let export = client
            .get(format!(
                "{base}/admin/diagnostic-capture/export?scope=all&format=json&privacy=redacted"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(export.status(), StatusCode::OK);
        assert_eq!(
            export
                .headers()
                .get("content-disposition")
                .and_then(|value| value.to_str().ok()),
            Some("attachment; filename=\"sumpter-diagnostic-all-redacted.json\"")
        );
        assert_eq!(
            export
                .headers()
                .get("cache-control")
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

        let missing_token = client
            .get(format!("{base}/admin/runtime/summary"))
            .send()
            .await
            .unwrap();
        assert_eq!(missing_token.status(), StatusCode::FORBIDDEN);

        let summary = client
            .get(format!("{base}/admin/runtime/summary"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(summary.status(), StatusCode::OK);
        let summary: Value = serde_json::from_slice(&summary.bytes().await.unwrap()).unwrap();
        assert_eq!(summary["apiVersion"], 1);
        assert_eq!(summary["storage"]["backend"], "sqlite");

        let events = client
            .get(format!("{base}/admin/runtime/events?limit=50"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(events.status(), StatusCode::OK);
        let events: Value = serde_json::from_slice(&events.bytes().await.unwrap()).unwrap();
        assert_eq!(events["cursorValid"], true);

        let page = client
            .get(format!(
                "{base}/admin/runtime/events?view=page&page=1&pageSize=25"
            ))
            .header("x-control-token", "runtime-control-token")
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
                "{base}/admin/runtime/events?view=page&beforeSeq=10"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(mixed_cursor.status(), StatusCode::BAD_REQUEST);

        for path in [
            "/admin/runtime/trends?range=24h",
            "/admin/runtime/errors?page=1&pageSize=25",
            "/admin/runtime/projects?page=1&pageSize=25",
            "/admin/runtime/sessions?page=1&pageSize=25",
            "/admin/runtime/storage",
            "/admin/runtime/retention",
            "/admin/runtime/pricing",
            "/admin/runtime/export/estimate?scope=events&format=jsonl&privacy=redacted",
        ] {
            let response = client
                .get(format!("{base}{path}"))
                .header("x-control-token", "runtime-control-token")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{path}");
        }

        let retention_update = client
            .put(format!("{base}/admin/runtime/retention"))
            .header("x-control-token", "runtime-control-token")
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&json!({
                    "expectedRevision": 1,
                    "maxEvents": 100000
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
            .put(format!("{base}/admin/runtime/retention"))
            .header("x-control-token", "runtime-control-token")
            .header("content-type", "application/json")
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

        let pricing_update = client
            .put(format!("{base}/admin/runtime/pricing"))
            .header("x-control-token", "runtime-control-token")
            .header("content-type", "application/json")
            .body(
                serde_json::to_vec(&json!({
                    "expectedRevision": 1,
                    "currency": "USD",
                    "prices": []
                }))
                .unwrap(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(pricing_update.status(), StatusCode::OK);
        let pricing_update: Value =
            serde_json::from_slice(&pricing_update.bytes().await.unwrap()).unwrap();
        assert_eq!(pricing_update["revision"], 2);

        let missing_chain = client
            .get(format!("{base}/admin/runtime/request-chain"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_chain.status(), StatusCode::BAD_REQUEST);

        let stored_without_confirmation = client
            .get(format!(
                "{base}/admin/runtime/export?scope=events&format=csv&privacy=stored"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(
            stored_without_confirmation.status(),
            StatusCode::BAD_REQUEST
        );

        let runtime_export = client
            .get(format!(
                "{base}/admin/runtime/export?scope=events&format=jsonl&privacy=redacted"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(runtime_export.status(), StatusCode::OK);
        assert_eq!(
            runtime_export
                .headers()
                .get("cache-control")
                .and_then(|value| value.to_str().ok()),
            Some("no-store")
        );
        assert_eq!(
            runtime_export
                .headers()
                .get("x-sumpter-row-count")
                .and_then(|value| value.to_str().ok()),
            Some("0")
        );
        assert!(runtime_export.bytes().await.unwrap().is_empty());

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
            .get(format!("{base}/admin/runtime/events/missing"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_detail.status(), StatusCode::NOT_FOUND);

        let one_hour_range = client
            .get(format!("{base}/admin/runtime/analytics?range=1h"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(one_hour_range.status(), StatusCode::OK);

        let missing_session_delete = client
            .delete(format!("{base}/admin/runtime/session"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_session_delete.status(), StatusCode::BAD_REQUEST);
        let missing_session_export = client
            .get(format!("{base}/admin/runtime/session/export"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(missing_session_export.status(), StatusCode::BAD_REQUEST);

        let unknown_session_delete = client
            .delete(format!(
                "{base}/admin/runtime/session?sessionID=missing-session"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_session_delete.status(), StatusCode::NOT_FOUND);
        let unknown_session_export = client
            .get(format!(
                "{base}/admin/runtime/session/export?sessionID=missing-session"
            ))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(unknown_session_export.status(), StatusCode::NOT_FOUND);

        for (method, path) in [
            (reqwest::Method::GET, "/admin/runtime"),
            (reqwest::Method::POST, "/admin/reset-stats"),
        ] {
            let response = client
                .request(method, format!("{base}{path}"))
                .header("x-control-token", "runtime-control-token")
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        }

        let reset = client
            .post(format!("{base}/admin/runtime/reset"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(reset.status(), StatusCode::OK);
        let reset: Value = serde_json::from_slice(&reset.bytes().await.unwrap()).unwrap();
        assert_eq!(reset["resetGeneration"], 1);

        let recreate = client
            .post(format!("{base}/admin/runtime/recreate"))
            .header("x-control-token", "runtime-control-token")
            .send()
            .await
            .unwrap();
        assert_eq!(recreate.status(), StatusCode::OK);
        let recreate: Value = serde_json::from_slice(&recreate.bytes().await.unwrap()).unwrap();
        assert_eq!(recreate["reset"], true);
        assert_eq!(recreate["recreated"], true);
        assert_eq!(recreate["resetGeneration"], 2);

        server.abort();
        let _ = std::fs::remove_dir_all(root);
    }
}
