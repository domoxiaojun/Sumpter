use super::{AdminState, JsonPayload, api_error, json_ok, listener_address, require_json};
use axum::extract::rejection::QueryRejection;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};
use sumpter_core::config::{AppConfig, SCHEMA_VERSION};
use sumpter_core::config_store::validate_config_wire;

#[cfg(test)]
#[path = "../../../../tests/contracts/provider_catalog.rs"]
mod provider_catalog_contract;

pub(crate) async fn get_config(State(state): State<AdminState>) -> Response {
    let _transaction = state.inner.engine.config_transaction().await;
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
    pub(crate) inbound_tokens: HashMap<String, String>,
    #[serde(default)]
    pub(crate) endpoints: HashMap<String, EndpointSecretUpdate>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EndpointSecretUpdate {
    api_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InboundTokenSecretBody {
    #[serde(rename = "tokenID")]
    token_id: String,
    expected_generation: String,
}

/// Called only on explicit reveal/copy. The ordinary config view stays redacted.
pub(crate) async fn inbound_token_secret(
    State(state): State<AdminState>,
    payload: JsonPayload<InboundTokenSecretBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let _transaction = state.inner.engine.config_transaction().await;
    let generation = state.inner.engine.generation();
    if body.expected_generation != generation {
        return api_error(
            StatusCode::CONFLICT,
            "config_conflict",
            "配置已变化，请重新载入 Token 列表",
        );
    }
    let config = state.inner.engine.config();
    let token = if body.token_id == "legacy" {
        Some(config.listener.auth_token.as_str()).filter(|value| !value.is_empty())
    } else {
        config
            .listener
            .auth_tokens
            .iter()
            .find(|entry| entry.id == body.token_id)
            .map(|entry| entry.token.as_str())
    };
    let Some(token) = token else {
        return api_error(
            StatusCode::NOT_FOUND,
            "inbound_token_not_found",
            "Token 已删除或不存在",
        );
    };
    let mut response =
        json_ok(&json!({"tokenID": body.token_id, "token": token, "generation": generation}));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    response
}

#[cfg(test)]
mod inbound_token_tests {
    use super::*;
    use sumpter_core::config::InboundTokenConfig;

    #[test]
    fn inbound_tokens_redact_preserve_rotate_and_validate() {
        let mut old = AppConfig::bootstrap();
        old.listener.auth_tokens = vec![InboundTokenConfig {
            id: "work".into(),
            name: "工作".into(),
            enabled: true,
            token: "synthetic-work-token".into(),
        }];
        let view = config_view(&old, "test", None);
        assert!(!view.to_string().contains("synthetic-work-token"));
        assert_eq!(
            view["secretStatus"]["inboundTokens"]["work"]["last4"],
            "oken"
        );
        let mut draft: AppConfig = serde_json::from_value(view["config"].clone()).unwrap();
        draft.listener.auth_tokens[0].name = "新名称".into();
        apply_secrets(&old, &mut draft, SecretUpdates::default()).unwrap();
        assert_eq!(
            draft.listener.auth_tokens[0].token,
            old.listener.auth_tokens[0].token
        );
        apply_secrets(
            &old,
            &mut draft,
            SecretUpdates {
                inbound_tokens: HashMap::from([("work".into(), "synthetic-rotated-token".into())]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            draft.listener.auth_tokens[0].token,
            "synthetic-rotated-token"
        );
        assert!(
            apply_secrets(
                &old,
                &mut draft,
                SecretUpdates {
                    inbound_tokens: HashMap::from([(
                        "missing".into(),
                        "synthetic-invalid-token".into()
                    )]),
                    ..Default::default()
                }
            )
            .is_err()
        );
        draft.listener.auth_tokens[0].id = "new".into();
        assert!(apply_secrets(&old, &mut draft, SecretUpdates::default()).is_err());
        draft.listener.auth_tokens.clear();
        apply_secrets(&old, &mut draft, SecretUpdates::default()).unwrap();
        assert!(draft.listener.auth_tokens.is_empty());
    }
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
    for entry in &mut redacted.listener.auth_tokens {
        entry.token.clear();
    }
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
            "inboundTokens": config.listener.auth_tokens.iter().map(|entry| (entry.id.clone(), secret_status(&entry.token))).collect::<serde_json::Map<String, Value>>(),
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

    if updates.inbound_tokens.keys().any(|id| {
        !draft
            .listener
            .auth_tokens
            .iter()
            .any(|entry| &entry.id == id)
    }) {
        return Err("secretUpdates 引用了不存在的入站 Token".into());
    }
    for entry in &mut draft.listener.auth_tokens {
        entry.token = updates
            .inbound_tokens
            .get(&entry.id)
            .cloned()
            .or_else(|| {
                old.listener
                    .auth_tokens
                    .iter()
                    .find(|old| old.id == entry.id)
                    .map(|old| old.token.clone())
            })
            .unwrap_or_default();
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
    draft.listener.validate_inbound_tokens(true)?;
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
    config.request_policy.validate_security_limits()?;
    config.listener.validate_tls()?;
    config.validate_user_agents()?;
    config.validate_resolve_ips()?;
    validate_config_identity(config)?;
    config.validate_model_groups()?;
    let _ = listener_address(config)?;
    config.listener.validate_cidr_lists()?;
    config.listener.validate_inbound_tokens(true)?;
    if config.retry.max_deferred_rounds < 0
        || config.retry.max_retry_duration_seconds < 0.0
        || config.retry.max_500_retries < 0
        || config.retry.max_stream_error_retries < 0
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
    let snapshot = sumpter_engine::model_catalog::ProviderCatalogFetcher::fetch(&endpoint).await;
    state.inner.engine.publish_endpoint_capabilities(&snapshot);
    match snapshot
        .error
        .map_or_else(|| Ok((snapshot.models, snapshot.source)), Err)
    {
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
