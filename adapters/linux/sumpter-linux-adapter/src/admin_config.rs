use super::{
    AdminState, JsonPayload, MAX_MODEL_CATALOG_BYTES, MAX_MODEL_CATALOG_ITEMS, api_error, json_ok,
    listener_address, require_json,
};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{SystemTime, UNIX_EPOCH};
use sumpter_core::config::{AppConfig, Endpoint, SCHEMA_VERSION};
use sumpter_core::config_store::validate_config_wire;

pub(crate) async fn get_config(State(state): State<AdminState>) -> Response {
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

pub(crate) struct PutConfigBody {
    pub(crate) expected_generation: String,
    pub(crate) config: AppConfig,
    pub(crate) secret_updates: SecretUpdates,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PutConfigWireBody {
    expected_generation: String,
    config: Value,
    secret_updates: SecretUpdates,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretUpdates {
    pub(crate) inbound_auth_token: Option<String>,
    #[serde(default)]
    pub(crate) endpoints: HashMap<String, EndpointSecretUpdate>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EndpointSecretUpdate {
    api_key: Option<String>,
}

pub(crate) async fn put_config(
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

pub(crate) async fn reload(
    State(state): State<AdminState>,
    payload: JsonPayload<Value>,
) -> Response {
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

pub(crate) fn config_view(config: &AppConfig, generation: &str, extra: Option<Value>) -> Value {
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

pub(crate) fn secret_status(secret: &str) -> Value {
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

pub(crate) fn apply_secrets(
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

pub(crate) fn validate_config_identity(config: &AppConfig) -> Result<(), String> {
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

pub(crate) fn valid_cidr(raw: &str) -> bool {
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
pub(crate) struct ProviderModelsBody {
    #[serde(rename = "endpointID", alias = "endpointId")]
    endpoint_id: String,
}

pub(crate) async fn provider_models(
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

pub(crate) async fn fetch_provider_models(
    endpoint: &Endpoint,
) -> Result<(Vec<String>, String), String> {
    tokio::time::timeout(
        std::time::Duration::from_secs(12),
        fetch_provider_models_inner(endpoint),
    )
    .await
    .map_err(|_| "获取模型整体超时（12 秒）".to_string())?
}

pub(crate) async fn fetch_provider_models_inner(
    endpoint: &Endpoint,
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
    // 鉴权 -> 路径的顺序让同一鉴权有机会快速覆盖
    // `/v1/models`、`/models` 等候选；每次请求和整个探测均受 deadline 约束，
    // 避免一个失联入口把完整矩阵拖到分钟级。
    'probes: for auth in provider_model_auth_sets(key) {
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
            let mut request = client.get(url.clone());
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
                Ok(response) => errors.push(format!("{}: HTTP {}", url.path(), response.status())),
                Err(error) => errors.push(format!("{}: {error}", url.path())),
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

pub(crate) async fn read_model_catalog_body(response: reqwest::Response) -> Result<Bytes, String> {
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

/// 按真实转发的 base path 生成目录候选，避免 `/v1/v1/models`。
pub(crate) fn model_catalog_paths(base: &reqwest::Url) -> Vec<String> {
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
pub(crate) fn summarize_probe_failure(errors: Vec<String>, hint: Option<String>) -> String {
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
pub(crate) fn proxy_env_hint() -> Option<String> {
    proxy_hint_for(|name| {
        std::env::var_os(name)
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    })
}

/// 文案生成与环境读取分开，测试注入 `is_set` 即可，不改进程环境。
pub(crate) fn proxy_hint_for(is_set: impl Fn(&str) -> bool) -> Option<String> {
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
pub(crate) fn provider_model_auth_modes(key: &str) -> Vec<Option<(&'static str, String)>> {
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
pub(crate) fn provider_model_auth_sets(key: &str) -> Vec<Vec<(&'static str, String)>> {
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

pub(crate) fn extract_models(value: &Value) -> Vec<String> {
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
pub(crate) async fn endpoint_secret(
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
