//! Shared Provider model catalog discovery.

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;
use sumpter_core::config::Endpoint;
use sumpter_core::config_store::ConfigDir;

use crate::Engine;
use crate::engine::catalog::codex_client_catalog;

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalogStatus {
    pub enabled: bool,
    pub refresh_on_startup: bool,
    pub interval_minutes: u64,
    pub running: bool,
    pub last_run_at: String,
    pub next_run_at: String,
    pub last_error: String,
    pub remote_metadata: RemoteMetadataStatus,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteMetadataStatus {
    pub enabled: bool,
    pub models_revision: u64,
    pub codex_revision: u64,
    pub source: String,
    pub updated_at: String,
    pub error: String,
}

const MODELS_PRIMARY: &str =
    "https://raw.githubusercontent.com/router-for-me/models/refs/heads/main/models.json";
const MODELS_MIRROR: &str = "https://models.router-for.me/models.json";
const CODEX_PRIMARY: &str = "https://raw.githubusercontent.com/router-for-me/models/refs/heads/main/codex_client_models.json";
const CODEX_MIRROR: &str = "https://models.router-for.me/codex_client_models.json";
const MAX_METADATA_BYTES: usize = 4 * 1024 * 1024;
const MAX_METADATA_ITEMS: usize = 20_000;

pub const MAX_MODEL_CATALOG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_MODEL_CATALOG_ITEMS: usize = 5_000;
pub const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCatalogSnapshot {
    pub endpoint_id: String,
    pub models: Vec<String>,
    pub source: String,
    pub attempted_at: String,
    pub updated_at: String,
    pub error: Option<String>,
}

pub struct ProviderCatalogFetcher;

#[derive(Clone)]
pub struct ModelCatalogScheduler {
    engine: Engine,
    status: Arc<RwLock<ModelCatalogStatus>>,
}

impl ModelCatalogScheduler {
    pub fn new(engine: Engine) -> Self {
        let status = engine.model_catalog_status_store();
        if let Some(dir) = engine.config_dir()
            && let Ok(bytes) = dir.load_model_catalog("codex_client_models.json")
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && let Ok(revision) = codex_client_catalog::replace_templates(&value)
            && let Ok(mut current) = status.write()
        {
            current.remote_metadata.codex_revision = revision;
            current.remote_metadata.models_revision =
                revision_for(&dir, "models.json").unwrap_or(1);
        }
        Self { engine, status }
    }

    pub fn start(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut startup = true;
            loop {
                let settings = self.engine.config().model_catalog.clone();
                if startup
                    && settings.refresh_on_startup
                    && (settings.auto_refresh || settings.remote_metadata_enabled)
                {
                    self.refresh_once().await;
                }
                startup = false;
                tokio::time::sleep(Duration::from_secs(
                    settings.refresh_interval_minutes.saturating_mul(60),
                ))
                .await;
                let settings = self.engine.config().model_catalog.clone();
                if settings.auto_refresh || settings.remote_metadata_enabled {
                    self.refresh_once().await;
                }
            }
        })
    }

    pub async fn status(&self) -> ModelCatalogStatus {
        self.status
            .read()
            .expect("model catalog status lock poisoned")
            .clone()
    }

    pub async fn refresh_once(&self) {
        let config = self.engine.config();
        {
            let mut status = self
                .status
                .write()
                .expect("model catalog status lock poisoned");
            status.running = true;
            status.last_error.clear();
            status.last_run_at = unix_timestamp();
        }

        let metadata = if config.model_catalog.remote_metadata_enabled {
            Some(refresh_remote_metadata(self.engine.config_dir()).await)
        } else {
            None
        };
        let mut tasks = futures_util::stream::iter(if config.model_catalog.auto_refresh {
            config
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.enabled)
                .cloned()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        })
        .map(|endpoint| async move { ProviderCatalogFetcher::fetch(&endpoint).await })
        .buffer_unordered(4);

        let mut failures = Vec::new();
        while let Some(snapshot) = tasks.next().await {
            if let Some(error) = snapshot.error.as_deref() {
                failures.push(format!("{}: {error}", snapshot.endpoint_id));
            }
            if let Err(error) = self.engine.apply_provider_catalog_snapshot(&snapshot) {
                failures.push(format!("{}: 保存失败: {error}", snapshot.endpoint_id));
            }
        }
        let previous_metadata = self
            .status
            .read()
            .expect("model catalog status lock poisoned")
            .remote_metadata
            .clone();
        let mut status = self
            .status
            .write()
            .expect("model catalog status lock poisoned");
        status.running = false;
        status.last_error = failures.join("；");
        let minutes = self.engine.config().model_catalog.refresh_interval_minutes;
        status.next_run_at = unix_timestamp_after(minutes.saturating_mul(60));
        if let Some(metadata) = metadata {
            status.remote_metadata = metadata;
            if status.remote_metadata.models_revision != previous_metadata.models_revision {
                let _ =
                    self.engine
                        .notice_sender()
                        .send(crate::EngineNotice::ModelMetadataUpdated {
                            catalog: "models.json".into(),
                            revision: status.remote_metadata.models_revision,
                        });
            }
            if status.remote_metadata.codex_revision != previous_metadata.codex_revision {
                let _ =
                    self.engine
                        .notice_sender()
                        .send(crate::EngineNotice::ModelMetadataUpdated {
                            catalog: "codex_client_models.json".into(),
                            revision: status.remote_metadata.codex_revision,
                        });
            }
        }
    }
}

impl ModelCatalogStatus {
    pub fn for_config(config: &sumpter_core::config::AppConfig) -> Self {
        Self {
            enabled: config.model_catalog.auto_refresh,
            refresh_on_startup: config.model_catalog.refresh_on_startup,
            interval_minutes: config.model_catalog.refresh_interval_minutes,
            remote_metadata: RemoteMetadataStatus {
                enabled: config.model_catalog.remote_metadata_enabled,
                models_revision: 1,
                codex_revision: codex_client_catalog::template_revision(),
                ..RemoteMetadataStatus::default()
            },
            ..Self::default()
        }
    }
}

async fn refresh_remote_metadata(dir: Option<ConfigDir>) -> RemoteMetadataStatus {
    let mut status = RemoteMetadataStatus {
        enabled: true,
        models_revision: 1,
        codex_revision: codex_client_catalog::template_revision(),
        ..RemoteMetadataStatus::default()
    };
    let client = match reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(4))
        .timeout(Duration::from_secs(12))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            status.error = error.to_string();
            return status;
        }
    };
    let models = fetch_metadata(&client, &[MODELS_PRIMARY, MODELS_MIRROR]).await;
    let codex = fetch_metadata(&client, &[CODEX_PRIMARY, CODEX_MIRROR]).await;
    let mut errors = Vec::new();
    if let Ok((payload, source)) = models {
        if let Err(error) = validate_models_metadata(&payload) {
            errors.push(format!("models.json: {error}"));
        } else if let Some(dir) = &dir {
            if let Err(error) = persist_metadata(dir, "models.json", &payload).await {
                errors.push(format!("models.json 缓存失败: {error}"));
            } else {
                status.models_revision = revision_for(dir, "models.json").unwrap_or(1);
                status.source = source;
                status.updated_at = unix_timestamp();
            }
        }
    } else if let Err(error) = models {
        errors.push(format!("models.json: {error}"));
    }
    if let Ok((payload, _source)) = codex {
        if let Err(error) = validate_codex_metadata(&payload) {
            errors.push(format!("codex_client_models.json: {error}"));
        } else {
            match codex_client_catalog::replace_templates(&payload) {
                Ok(revision) => {
                    status.codex_revision = revision;
                    if let Some(dir) = &dir
                        && let Err(error) =
                            persist_metadata(dir, "codex_client_models.json", &payload).await
                    {
                        errors.push(format!("Codex 模板缓存失败: {error}"));
                    }
                }
                Err(error) => errors.push(format!("codex_client_models.json: {error}")),
            }
        }
    } else if let Err(error) = codex {
        errors.push(format!("codex_client_models.json: {error}"));
    }
    status.error = errors.join("；");
    status
}

async fn fetch_metadata(
    client: &reqwest::Client,
    urls: &[&str],
) -> Result<(Value, String), String> {
    let mut errors = Vec::new();
    for url in urls {
        match client.get(*url).send().await {
            Ok(response) if response.status() == StatusCode::OK => {
                match read_body(response).await {
                    Ok(body) => match serde_json::from_slice::<Value>(&body) {
                        Ok(value) => return Ok((value, (*url).to_string())),
                        Err(error) => errors.push(format!("{url}: JSON {error}")),
                    },
                    Err(error) => errors.push(format!("{url}: {error}")),
                }
            }
            Ok(response) => errors.push(format!("{url}: HTTP {}", response.status())),
            Err(error) => errors.push(format!("{url}: {error}")),
        }
    }
    Err(errors.join("；"))
}

fn validate_models_metadata(value: &Value) -> Result<(), String> {
    let object = value
        .as_object()
        .ok_or_else(|| "顶层必须是对象".to_string())?;
    if !object.keys().any(|key| {
        matches!(
            key.as_str(),
            "openai" | "anthropic" | "claude" | "gemini" | "google" | "codex" | "providers"
        )
    }) {
        return Err("缺少已知 provider section".into());
    }
    for section in object.values().filter_map(Value::as_array) {
        let mut ids = BTreeSet::new();
        for model in section {
            let object = model
                .as_object()
                .ok_or_else(|| "provider section 必须包含模型对象".to_string())?;
            let id = object
                .get("id")
                .or_else(|| object.get("slug"))
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| "模型 ID 不能为空".to_string())?;
            if !ids.insert(id.to_string()) {
                return Err(format!("模型 ID 重复: {id}"));
            }
            for field in [
                "context_length",
                "context_window",
                "max_completion_tokens",
                "max_output_tokens",
            ] {
                if let Some(value) = object.get(field) {
                    let number = value
                        .as_u64()
                        .ok_or_else(|| format!("{id}.{field} 必须是正整数"))?;
                    if number == 0 {
                        return Err(format!("{id}.{field} 必须是正整数"));
                    }
                }
            }
        }
    }
    validate_json_strings(value, 0)?;
    Ok(())
}

fn validate_json_strings(value: &Value, count: usize) -> Result<(), String> {
    if count > MAX_METADATA_ITEMS {
        return Err("模型数量超限".into());
    }
    match value {
        Value::String(text) => {
            if text.chars().any(char::is_control) || text.len() > 16 * 1024 {
                return Err("字符串包含控制字符或长度超限".into());
            }
        }
        Value::Array(values) => {
            for value in values {
                validate_json_strings(value, count + 1)?;
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                validate_json_strings(value, count + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_codex_metadata(value: &Value) -> Result<(), String> {
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "models 必须是非空数组".to_string())?;
    if models.is_empty() || models.len() > MAX_METADATA_ITEMS {
        return Err("models 数量无效".into());
    }
    validate_json_strings(value, 0)?;
    if models
        .iter()
        .filter_map(|model| model.get("slug").and_then(Value::as_str))
        .filter(|slug| *slug == "gpt-5.5")
        .count()
        != 1
    {
        return Err("必须包含唯一的 gpt-5.5".into());
    }
    Ok(())
}

async fn persist_metadata(dir: &ConfigDir, name: &str, value: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_METADATA_BYTES {
        return Err("响应过大".into());
    }
    dir.save_model_catalog(name, &bytes)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn revision_for(dir: &ConfigDir, name: &str) -> Option<u64> {
    use md5::Digest;
    let bytes = std::fs::read(dir.model_catalog_path(name)).ok()?;
    let digest = md5::Md5::digest(bytes);
    Some(u64::from_be_bytes([
        digest[0], digest[1], digest[2], digest[3], digest[4], digest[5], digest[6], digest[7],
    ]))
}

impl ProviderCatalogFetcher {
    pub async fn fetch(endpoint: &Endpoint) -> ProviderCatalogSnapshot {
        let attempted_at = unix_timestamp();
        match tokio::time::timeout(MODEL_CATALOG_TIMEOUT, fetch_inner(endpoint)).await {
            Ok(Ok((models, source))) => ProviderCatalogSnapshot {
                endpoint_id: endpoint.id.clone(),
                models,
                source,
                attempted_at: attempted_at.clone(),
                updated_at: attempted_at,
                error: None,
            },
            Ok(Err(error)) => ProviderCatalogSnapshot {
                endpoint_id: endpoint.id.clone(),
                models: Vec::new(),
                source: String::new(),
                attempted_at,
                updated_at: String::new(),
                error: Some(error),
            },
            Err(_) => ProviderCatalogSnapshot {
                endpoint_id: endpoint.id.clone(),
                models: Vec::new(),
                source: String::new(),
                attempted_at,
                updated_at: String::new(),
                error: Some("获取模型整体超时（12 秒）".into()),
            },
        }
    }
}

async fn fetch_inner(endpoint: &Endpoint) -> Result<(Vec<String>, String), String> {
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

    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .http1_only()
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .no_proxy()
        .pool_max_idle_per_host(0)
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3));
    if !endpoint.resolve_ip.trim().is_empty() {
        let ip = endpoint
            .resolve_ip
            .trim()
            .parse()
            .map_err(|_| format!("resolveIP 无效: {}", endpoint.resolve_ip))?;
        let host = base
            .host_str()
            .ok_or_else(|| "baseURL 缺少主机名".to_string())?;
        let port = base
            .port_or_known_default()
            .ok_or_else(|| "baseURL 缺少有效端口".to_string())?;
        builder = builder.resolve(host, std::net::SocketAddr::new(ip, port));
    }
    let client = builder.build().map_err(|error| error.to_string())?;
    let probes =
        crate::request_build::model_catalog_probes(endpoint.protocol, &endpoint.user_agent);
    let deadline = Instant::now() + Duration::from_millis(11_500);
    let mut errors = Vec::new();
    let mut discovered = BTreeSet::new();
    let mut source = None;

    'probes: for (index, probe) in probes.iter().enumerate() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let probe_deadline = Instant::now() + remaining / (probes.len() - index) as u32;
        for auth in auth_sets(key) {
            for path in model_catalog_paths(&base) {
                let remaining = probe_deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    errors.push("目录身份探测超时，继续其它身份".into());
                    continue 'probes;
                }
                let mut url = base.clone();
                url.set_path(&path);
                url.set_query(None);
                url.set_fragment(None);
                let mut request = client.get(url.clone());
                for (header, value) in crate::request_build::model_catalog_probe_headers(probe) {
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
                        match read_body(response).await {
                            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                                Ok(value) => {
                                    let models = extract_models(&value);
                                    if !models.is_empty() {
                                        source.get_or_insert_with(|| url.to_string());
                                        discovered.extend(models);
                                        continue 'probes;
                                    }
                                    errors.push(format!("{}: 响应中没有模型", url.path()));
                                }
                                Err(error) => errors.push(format!("{}: JSON {error}", url.path())),
                            },
                            Err(error) => errors.push(format!("{}: {error}", url.path())),
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

    if let Some(source) = source {
        return Ok((
            discovered
                .into_iter()
                .take(MAX_MODEL_CATALOG_ITEMS)
                .collect(),
            source,
        ));
    }
    errors.dedup();
    Err(errors.into_iter().take(4).collect::<Vec<_>>().join("；"))
}

async fn read_body(response: reqwest::Response) -> Result<Bytes, String> {
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

pub fn model_catalog_paths(base: &reqwest::Url) -> Vec<String> {
    let prefix = base.path().trim_end_matches('/');
    let mut paths = Vec::new();
    for candidate in ["/v1/models", "/models", "/v1/model/list"] {
        let path = if prefix.is_empty() {
            candidate.to_string()
        } else if prefix.ends_with("/v1") && candidate.starts_with("/v1/") {
            format!("{}{}", prefix, &candidate[3..])
        } else {
            format!("{prefix}{candidate}")
        };
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if prefix.is_empty() {
        paths.push("/api/v1/models".into());
    }
    paths
}

fn auth_sets(key: &str) -> Vec<Vec<(&'static str, String)>> {
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

pub fn extract_models(value: &Value) -> Vec<String> {
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
            for key in ["id", "slug", "name", "model", "model_id"] {
                if let Some(model) = object.get(key).and_then(Value::as_str) {
                    visit(&Value::String(model.to_string()), models, depth + 1);
                    break;
                }
            }
        }
        if models.len() == before
            && let Some(Value::Object(entries)) = object.get("models")
        {
            for key in entries.keys() {
                models.push(key.clone());
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
            models.extend(object.keys().cloned());
        }
    }

    let mut models = Vec::new();
    visit(value, &mut models, 0);
    models.sort();
    models.dedup();
    models.truncate(MAX_MODEL_CATALOG_ITEMS);
    models
}

fn unix_timestamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_default()
}

fn unix_timestamp_after(seconds: u64) -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs().saturating_add(seconds).to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_common_catalog_shapes() {
        assert_eq!(
            extract_models(&json!({"data": [{"id": "m1"}, {"model": "m2"}]})),
            vec!["m1", "m2"]
        );
        assert_eq!(
            extract_models(&json!({"models": {"m3": {}, "m4": {}}})),
            vec!["m3", "m4"]
        );
    }

    #[test]
    fn preserves_custom_base_path_without_duplicate_v1() {
        let base = reqwest::Url::parse("https://example.invalid/apps/v1").unwrap();
        assert_eq!(
            model_catalog_paths(&base),
            vec!["/apps/v1/models", "/apps/v1/model/list"]
        );
    }
}
