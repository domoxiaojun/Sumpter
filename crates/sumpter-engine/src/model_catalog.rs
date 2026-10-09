//! Shared Provider model catalog discovery.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
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
    /// Results discarded because the queried endpoint changed while fetching.
    pub stale_endpoints: Vec<String>,
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
pub(crate) const MAX_CODEX_CATALOG_BYTES: usize = 1 << 20;
const MAX_METADATA_ITEMS: usize = 20_000;
const MAX_METADATA_STRING_BYTES: usize = 128 * 1024;

pub const MAX_MODEL_CATALOG_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_MODEL_CATALOG_ITEMS: usize = 5_000;
pub const MODEL_CATALOG_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCatalogSnapshot {
    pub endpoint_id: String,
    pub endpoint_fingerprint: String,
    pub models: Vec<String>,
    pub source: String,
    pub attempted_at: String,
    pub updated_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogApplyOutcome {
    Applied,
    Unchanged,
    Stale,
}

/// A non-persisted digest of the exact discovery inputs, including credentials.
/// Debug/status output must never contain the original API key.
pub fn endpoint_fingerprint(endpoint: &Endpoint) -> String {
    use sha2::{Digest, Sha256};
    let input = serde_json::json!([
        endpoint.id,
        endpoint.base_url,
        endpoint.resolve_ip,
        endpoint.protocol,
        endpoint.enabled,
        endpoint.api_key,
        endpoint.user_agent
    ]);
    format!("{:x}", Sha256::digest(input.to_string().as_bytes()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum MetadataSource {
    CodexTemplate,
    #[default]
    ProviderRegistry,
    EndpointCatalog,
    Fallback,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ProviderModelMetadata {
    pub(crate) id: String,
    pub(crate) canonical_id: String,
    pub(crate) aliases: BTreeSet<String>,
    pub(crate) providers: BTreeSet<String>,
    pub(crate) source: MetadataSource,
    pub(crate) display_name: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) context_window: Option<u64>,
    pub(crate) max_context_window: Option<u64>,
    pub(crate) max_output_tokens: Option<u64>,
    pub(crate) reasoning_levels: Option<Vec<String>>,
    pub(crate) default_reasoning_level: Option<String>,
    pub(crate) input_modalities: Option<Vec<String>>,
    pub(crate) output_modalities: Option<Vec<String>>,
    pub(crate) supports_search_tool: Option<bool>,
    pub(crate) prefer_websockets: Option<bool>,
    pub(crate) service_tiers: Option<Vec<String>>,
    pub(crate) conflicts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModelMetadataIndex {
    pub(crate) models: BTreeMap<String, ProviderModelMetadata>,
    pub(crate) aliases: BTreeMap<String, String>,
}

impl ModelMetadataIndex {
    pub(crate) fn lookup(&self, id: &str) -> Option<&ProviderModelMetadata> {
        let key = normalize_metadata_id(id);
        let base = key.split_once('/').map(|(_, base)| base);
        if let Some(canonical) = self.aliases.get(&key) {
            return self.models.get(canonical);
        }
        if let Some(model) = self.models.get(&key) {
            return Some(model);
        }
        base.and_then(|base| self.aliases.get(base))
            .and_then(|canonical| self.models.get(canonical))
    }
}

fn metadata_index() -> &'static RwLock<ModelMetadataIndex> {
    static INDEX: OnceLock<RwLock<ModelMetadataIndex>> = OnceLock::new();
    INDEX.get_or_init(|| RwLock::new(ModelMetadataIndex::default()))
}

static METADATA_REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn provider_metadata_snapshot() -> ModelMetadataIndex {
    metadata_index()
        .read()
        .expect("model metadata lock poisoned")
        .clone()
}

pub(crate) fn provider_metadata_revision() -> u64 {
    METADATA_REVISION.load(Ordering::Acquire)
}

pub(crate) fn replace_provider_metadata(value: &Value) -> Result<u64, String> {
    let next = parse_provider_metadata(value)?;
    let mut current = metadata_index()
        .write()
        .map_err(|_| "模型元数据锁已损坏".to_string())?;
    if current.models == next.models {
        return Ok(provider_metadata_revision());
    }
    *current = next;
    Ok(METADATA_REVISION.fetch_add(1, Ordering::AcqRel) + 1)
}

fn normalize_metadata_id(id: &str) -> String {
    id.trim().to_ascii_lowercase()
}

fn metadata_aliases(id: &str) -> Vec<String> {
    let mut aliases = vec![normalize_metadata_id(id)];
    if let Some((_, base)) = id.split_once('/') {
        aliases.push(normalize_metadata_id(base));
    }
    aliases
}

fn json_string(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_positive_u64(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<u64> {
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_u64))
        .filter(|value| *value > 0)
}

fn json_string_list(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<Vec<String>> {
    keys.iter().find_map(|key| {
        let values = object.get(*key)?.as_array()?;
        let values = values
            .iter()
            .filter_map(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("id").and_then(Value::as_str))
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
            })
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        Some(values)
    })
}

fn json_reasoning_levels(object: &serde_json::Map<String, Value>) -> Option<Vec<String>> {
    let value = object
        .get("supported_reasoning_levels")
        .or_else(|| object.get("reasoning"))
        .or_else(|| object.get("thinking"))?;
    let value = value.get("levels").unwrap_or(value);
    let values = value.as_array()?;
    let levels = values
        .iter()
        .filter_map(|value| {
            value
                .as_str()
                .or_else(|| value.get("effort").and_then(Value::as_str))
                .map(str::trim)
                .filter(|level| !level.is_empty())
                .map(ToString::to_string)
        })
        .collect::<Vec<_>>();
    (!levels.is_empty()).then_some(levels)
}

pub(crate) fn parse_provider_model(provider: &str, value: &Value) -> Option<ProviderModelMetadata> {
    let object = value.as_object()?;
    let id = json_string(object, &["id", "slug", "name", "model", "model_id"])?;
    let metadata = ProviderModelMetadata {
        id: id.clone(),
        canonical_id: normalize_metadata_id(&id),
        aliases: metadata_aliases(&id).into_iter().collect(),
        providers: [normalize_metadata_id(provider)].into_iter().collect(),
        source: MetadataSource::ProviderRegistry,
        display_name: json_string(object, &["display_name", "displayName"]),
        description: json_string(object, &["description"]),
        context_window: json_positive_u64(object, &["context_length", "context_window"]),
        max_context_window: json_positive_u64(object, &["max_context_window"]),
        max_output_tokens: json_positive_u64(
            object,
            &["max_completion_tokens", "max_output_tokens", "max_tokens"],
        ),
        reasoning_levels: json_reasoning_levels(object),
        default_reasoning_level: json_string(object, &["default_reasoning_level"]),
        input_modalities: json_string_list(
            object,
            &["supportedInputModalities", "input_modalities"],
        ),
        output_modalities: json_string_list(
            object,
            &["supportedOutputModalities", "output_modalities"],
        ),
        supports_search_tool: object
            .get("supports_search_tool")
            .or_else(|| object.get("supports_web_search"))
            .and_then(Value::as_bool)
            .or_else(|| {
                object
                    .get("native_capabilities")
                    .and_then(|v| v.get("web_search"))
                    .and_then(Value::as_bool)
            }),
        prefer_websockets: object.get("prefer_websockets").and_then(Value::as_bool),
        service_tiers: json_string_list(object, &["service_tiers"]),
        conflicts: Vec::new(),
    };
    Some(metadata)
}

fn intersect_strings(left: &[String], right: &[String]) -> Vec<String> {
    left.iter()
        .filter(|value| {
            right
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(value))
        })
        .cloned()
        .collect()
}

pub(crate) fn merge_provider_metadata(
    current: &mut ProviderModelMetadata,
    next: ProviderModelMetadata,
) {
    current.aliases.extend(next.aliases);
    current.providers.extend(next.providers);
    if next.source == MetadataSource::Fallback {
        current.source = MetadataSource::Fallback;
    }
    if current.canonical_id != next.canonical_id {
        current.canonical_id.clear();
        current.conflicts.push("canonical_id".into());
    }
    if current.display_name.is_none() {
        current.display_name = next.display_name.clone();
    } else if next.display_name.is_some() && current.display_name != next.display_name {
        current.conflicts.push("display_name".into());
    }
    if current.description.is_none() {
        current.description = next.description.clone();
    } else if next.description.is_some() && current.description != next.description {
        current.conflicts.push("description".into());
    }
    current.context_window = match (current.context_window, next.context_window) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    current.max_context_window = match (current.max_context_window, next.max_context_window) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    current.max_output_tokens = match (current.max_output_tokens, next.max_output_tokens) {
        (Some(left), Some(right)) => Some(left.min(right)),
        (left, right) => left.or(right),
    };
    current.reasoning_levels = match (&current.reasoning_levels, next.reasoning_levels) {
        (Some(left), Some(right)) => Some(intersect_strings(left, &right)),
        (None, right) => right,
        (left, None) => left.clone(),
    };
    current.input_modalities = match (&current.input_modalities, next.input_modalities) {
        (Some(left), Some(right)) => Some(intersect_strings(left, &right)),
        (None, right) => right,
        (left, None) => left.clone(),
    };
    current.output_modalities = match (&current.output_modalities, next.output_modalities) {
        (Some(left), Some(right)) => Some(intersect_strings(left, &right)),
        (None, right) => right,
        (left, None) => left.clone(),
    };
    let (left, right) = (current.supports_search_tool, next.supports_search_tool);
    current.supports_search_tool = Some(left.unwrap_or(false) && right.unwrap_or(false));
    let (left, right) = (current.prefer_websockets, next.prefer_websockets);
    current.prefer_websockets = Some(left.unwrap_or(false) && right.unwrap_or(false));
    current.service_tiers = match (&current.service_tiers, next.service_tiers) {
        (Some(left), Some(right)) => Some(intersect_strings(left, &right)),
        (None, right) => right,
        (left, None) => left.clone(),
    };
    if let Some(levels) = &current.reasoning_levels {
        let default = current
            .default_reasoning_level
            .as_ref()
            .filter(|default| {
                levels
                    .iter()
                    .any(|level| level.eq_ignore_ascii_case(default))
            })
            .cloned()
            .or_else(|| {
                levels
                    .iter()
                    .find(|level| level.eq_ignore_ascii_case("low"))
                    .cloned()
            })
            .or_else(|| levels.first().cloned());
        current.default_reasoning_level = default;
    }
    current.conflicts.extend(next.conflicts);
}

pub(crate) fn fallback_metadata(id: &str) -> ProviderModelMetadata {
    ProviderModelMetadata {
        id: id.trim().to_string(),
        canonical_id: normalize_metadata_id(id),
        aliases: metadata_aliases(id).into_iter().collect(),
        providers: BTreeSet::new(),
        source: MetadataSource::Fallback,
        display_name: Some(id.trim().to_string()),
        description: Some(id.trim().to_string()),
        context_window: Some(128_000),
        max_context_window: Some(128_000),
        max_output_tokens: Some(16_384),
        reasoning_levels: Some(vec!["none".into()]),
        default_reasoning_level: Some("none".into()),
        input_modalities: Some(vec!["text".into()]),
        output_modalities: Some(vec!["text".into()]),
        supports_search_tool: Some(false),
        prefer_websockets: Some(false),
        service_tiers: Some(Vec::new()),
        conflicts: Vec::new(),
    }
}

pub(crate) fn aggregate_metadata(
    id: &str,
    candidates: impl IntoIterator<Item = ProviderModelMetadata>,
) -> ProviderModelMetadata {
    let mut candidates = candidates.into_iter();
    let Some(mut aggregate) = candidates.next() else {
        return fallback_metadata(id);
    };
    for candidate in candidates {
        merge_provider_metadata(&mut aggregate, candidate);
    }
    aggregate.id = id.trim().to_string();
    aggregate.aliases.extend(metadata_aliases(id));
    if aggregate
        .reasoning_levels
        .as_ref()
        .is_some_and(Vec::is_empty)
    {
        aggregate.reasoning_levels = Some(vec!["none".into()]);
        aggregate.default_reasoning_level = Some("none".into());
    }
    if let (Some(context), Some(max_context)) =
        (aggregate.context_window, aggregate.max_context_window)
        && context > max_context
    {
        aggregate.context_window = Some(max_context);
        aggregate.conflicts.push("context_window".into());
    }
    aggregate
}

pub(crate) fn complete_metadata(
    id: &str,
    mut metadata: ProviderModelMetadata,
) -> ProviderModelMetadata {
    let defaults = fallback_metadata(id);
    if metadata.display_name.is_none() {
        metadata.display_name = defaults.display_name;
    }
    if metadata.description.is_none() {
        metadata.description = defaults.description;
    }
    if metadata.context_window.is_none() {
        metadata.context_window = Some(
            defaults
                .context_window
                .unwrap_or(128_000)
                .min(metadata.max_context_window.unwrap_or(u64::MAX)),
        );
    }
    if metadata.max_context_window.is_none() {
        metadata.max_context_window = metadata.context_window;
    }
    if let (Some(context), Some(max_context)) =
        (metadata.context_window, metadata.max_context_window)
        && context > max_context
    {
        metadata.context_window = Some(max_context);
        metadata.conflicts.push("context_window".into());
    }
    if metadata.max_output_tokens.is_none() {
        metadata.max_output_tokens = defaults.max_output_tokens;
    }
    if metadata.reasoning_levels.is_none() {
        metadata.reasoning_levels = defaults.reasoning_levels;
    }
    if metadata.default_reasoning_level.is_none() {
        metadata.default_reasoning_level = defaults.default_reasoning_level;
    }
    if metadata.input_modalities.is_none() {
        metadata.input_modalities = defaults.input_modalities;
    }
    if metadata.output_modalities.is_none() {
        metadata.output_modalities = defaults.output_modalities;
    }
    if metadata.supports_search_tool.is_none() {
        metadata.supports_search_tool = defaults.supports_search_tool;
    }
    if metadata.prefer_websockets.is_none() {
        metadata.prefer_websockets = defaults.prefer_websockets;
    }
    if metadata.service_tiers.is_none() {
        metadata.service_tiers = defaults.service_tiers;
    }
    metadata
}

pub(crate) fn parse_provider_metadata(value: &Value) -> Result<ModelMetadataIndex, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "models.json 顶层必须是对象".to_string())?;
    let mut index = ModelMetadataIndex::default();
    for (provider, section) in object {
        let Some(models) = section.as_array() else {
            continue;
        };
        for model in models {
            let Some(metadata) = parse_provider_model(provider, model) else {
                continue;
            };
            let key = normalize_metadata_id(&metadata.id);
            if let Some(current) = index.models.get_mut(&key) {
                merge_provider_metadata(current, metadata);
            } else {
                index.models.insert(key.clone(), metadata);
            }
        }
    }
    for (key, model) in &index.models {
        for alias in &model.aliases {
            index
                .aliases
                .entry(alias.clone())
                .or_insert_with(|| key.clone());
        }
    }
    if index.models.is_empty() {
        return Err("models.json 没有可用模型".into());
    }
    Ok(index)
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
        if let Some(dir) = engine.config_dir()
            && let Ok(bytes) = dir.load_model_catalog("models.json")
            && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
            && replace_provider_metadata(&value).is_ok()
            && let Ok(mut current) = status.write()
        {
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
        let mut stale_endpoints = Vec::new();
        while let Some(snapshot) = tasks.next().await {
            match self.engine.apply_provider_catalog_snapshot(&snapshot).await {
                Ok(CatalogApplyOutcome::Stale) => stale_endpoints.push(snapshot.endpoint_id),
                Ok(_) => {
                    if let Some(error) = snapshot.error.as_deref() {
                        failures.push(format!("{}: {error}", snapshot.endpoint_id));
                    }
                }
                Err(error) => failures.push(format!("{}: 保存失败: {error}", snapshot.endpoint_id)),
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
        status.stale_endpoints = stale_endpoints;
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
        } else if let Err(error) = replace_provider_metadata(&payload) {
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
    parse_provider_metadata(value)?;
    validate_json_strings(value, 0)?;
    Ok(())
}

fn validate_json_strings(value: &Value, count: usize) -> Result<(), String> {
    if count > MAX_METADATA_ITEMS {
        return Err("模型数量超限".into());
    }
    match value {
        Value::String(text) => {
            if text
                .bytes()
                .any(|byte| byte == 0 || (byte < 0x20 && !matches!(byte, b'\n' | b'\r' | b'\t')))
                || text.len() > MAX_METADATA_STRING_BYTES
            {
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
    let mut slugs = BTreeSet::new();
    for model in models {
        let object = model
            .as_object()
            .ok_or_else(|| "Codex 模型必须是对象".to_string())?;
        let slug = object
            .get("slug")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|slug| !slug.is_empty())
            .ok_or_else(|| "Codex 模型缺少 slug".to_string())?;
        if !slugs.insert(slug.to_string()) {
            return Err(format!("Codex 模型 slug 重复: {slug}"));
        }
        let context = object
            .get("context_window")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{slug} 缺少 context_window"))?;
        let max_context = object
            .get("max_context_window")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{slug} 缺少 max_context_window"))?;
        if context == 0 || max_context == 0 || context > max_context {
            return Err(format!("{slug} context_window 无效"));
        }
        let levels = object
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .filter(|levels| !levels.is_empty())
            .ok_or_else(|| format!("{slug} reasoning levels 不能为空"))?;
        let default = object
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{slug} 缺少 default_reasoning_level"))?;
        if !levels.iter().any(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|effort| effort == default)
        }) {
            return Err(format!("{slug} default_reasoning_level 无效"));
        }
    }
    if !slugs.contains("gpt-5.5") {
        return Err("必须包含唯一的 gpt-5.5".into());
    }
    validate_json_strings(value, 0)?;
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
                endpoint_fingerprint: endpoint_fingerprint(endpoint),
                models,
                source,
                attempted_at: attempted_at.clone(),
                updated_at: attempted_at,
                error: None,
            },
            Ok(Err(error)) => ProviderCatalogSnapshot {
                endpoint_id: endpoint.id.clone(),
                endpoint_fingerprint: endpoint_fingerprint(endpoint),
                models: Vec::new(),
                source: String::new(),
                attempted_at,
                updated_at: String::new(),
                error: Some(error),
            },
            Err(_) => ProviderCatalogSnapshot {
                endpoint_id: endpoint.id.clone(),
                endpoint_fingerprint: endpoint_fingerprint(endpoint),
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

    #[test]
    fn provider_metadata_accepts_multiline_templates_and_normalizes_fields() {
        let value = json!({
            "codex": [{
                "id": "metadata-test-model",
                "display_name": "Metadata Test",
                "context_length": 272000,
                "max_completion_tokens": 64000,
                "thinking": {"levels": ["low", "high"]},
                "supportedInputModalities": ["text", "image"],
                "native_capabilities": {"web_search": true},
                "description": "line one\nline two"
            }]
        });
        validate_models_metadata(&value).expect("provider metadata");
        let index = parse_provider_metadata(&value).expect("index");
        let model = index
            .lookup("provider-a/metadata-test-model")
            .expect("provider alias");
        assert_eq!(model.context_window, Some(272000));
        assert_eq!(model.max_output_tokens, Some(64000));
        assert_eq!(
            model.reasoning_levels,
            Some(vec!["low".into(), "high".into()])
        );
        assert_eq!(
            model.input_modalities,
            Some(vec!["text".into(), "image".into()])
        );
        assert_eq!(model.supports_search_tool, Some(true));
    }

    #[test]
    fn sparse_metadata_fallback_keeps_context_windows_consistent() {
        for (fields, context, max_context) in [
            (
                json!({"id": "custom", "context_length": 272000}),
                272000,
                272000,
            ),
            (
                json!({"id": "custom", "max_context_window": 32000}),
                32000,
                32000,
            ),
            (
                json!({"id": "custom", "context_length": 272000, "max_context_window": 128000}),
                128000,
                128000,
            ),
            (json!({"id": "custom"}), 128000, 128000),
        ] {
            let metadata =
                complete_metadata("custom", parse_provider_model("custom", &fields).unwrap());
            assert_eq!(metadata.context_window, Some(context));
            assert_eq!(metadata.max_context_window, Some(max_context));
        }
    }

    #[test]
    fn aggregate_metadata_never_emits_an_invalid_context_window_pair() {
        let left = complete_metadata(
            "left",
            parse_provider_model(
                "provider-a",
                &json!({"id": "left", "context_length": 272000, "max_context_window": 872000}),
            )
            .unwrap(),
        );
        let right = complete_metadata(
            "right",
            parse_provider_model(
                "provider-b",
                &json!({"id": "right", "context_length": 128000, "max_context_window": 64000}),
            )
            .unwrap(),
        );
        let aggregate = aggregate_metadata("shared", [left, right]);
        assert_eq!(aggregate.context_window, Some(64000));
        assert_eq!(aggregate.max_context_window, Some(64000));
    }

    #[test]
    fn codex_metadata_validation_rejects_duplicate_and_invalid_models() {
        let duplicate = json!({
            "models": [
                {"slug":"gpt-5.5","context_window":1,"max_context_window":1,"supported_reasoning_levels":[{"effort":"none"}],"default_reasoning_level":"none"},
                {"slug":"gpt-5.5","context_window":1,"max_context_window":1,"supported_reasoning_levels":[{"effort":"none"}],"default_reasoning_level":"none"}
            ]
        });
        assert!(validate_codex_metadata(&duplicate).is_err());

        let invalid_context = json!({
            "models": [
                {"slug":"gpt-5.5","context_window":2,"max_context_window":1,"supported_reasoning_levels":[{"effort":"none"}],"default_reasoning_level":"none"}
            ]
        });
        assert!(validate_codex_metadata(&invalid_context).is_err());
    }
}
