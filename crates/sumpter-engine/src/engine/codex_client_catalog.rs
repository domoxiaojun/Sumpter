//! Codex `{models:[...]}` catalog built like CLIProxyAPI: clone official
//! templates by slug, otherwise clone `gpt-5.5` and only rewrite identity.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use serde_json::{Value, json};

use super::LocalModelEntry;
use super::is_codex_chat_model;
use super::supports_extended_reasoning_levels;
use crate::model_catalog::{MAX_CODEX_CATALOG_BYTES, ProviderModelMetadata};

const CODEX_CLIENT_MODELS_JSON: &str = include_str!("codex_client_models.json");
const DEFAULT_TEMPLATE_SLUG: &str = "gpt-5.5";
const LEGACY_REASONING_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];

#[derive(Clone)]
pub(crate) struct CodexClientTemplates {
    by_slug: HashMap<String, Value>,
    default_template: Value,
    max_priority: i64,
}

fn templates() -> &'static RwLock<CodexClientTemplates> {
    static TEMPLATES: OnceLock<RwLock<CodexClientTemplates>> = OnceLock::new();
    TEMPLATES.get_or_init(|| RwLock::new(load_templates()))
}

fn load_templates() -> CodexClientTemplates {
    let payload: Value = serde_json::from_str(CODEX_CLIENT_MODELS_JSON)
        .expect("embedded Codex client catalog must be valid JSON");
    parse_templates(&payload).expect("embedded Codex client catalog is invalid")
}

fn parse_templates(payload: &Value) -> Result<CodexClientTemplates, String> {
    let models = payload
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| "models 必须是非空数组".to_string())?;
    if models.is_empty() {
        return Err("models 必须是非空数组".into());
    }
    let mut by_slug = HashMap::new();
    let mut max_priority = 0_i64;
    for model in models {
        let slug = model
            .get("slug")
            .and_then(Value::as_str)
            .ok_or_else(|| "模型缺少 slug".to_string())?;
        let slug = slug.trim();
        if slug.is_empty() {
            return Err("模型 slug 不能为空".into());
        }
        if by_slug.contains_key(slug) {
            return Err(format!("模型 slug 重复: {slug}"));
        }
        let context = model
            .get("context_window")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{slug} 缺少 context_window"))?;
        let max_context = model
            .get("max_context_window")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("{slug} 缺少 max_context_window"))?;
        if context == 0 || max_context == 0 || context > max_context {
            return Err(format!("{slug} context_window 无效"));
        }
        let levels = model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{slug} 缺少 supported_reasoning_levels"))?;
        if levels.is_empty() {
            return Err(format!("{slug} reasoning levels 不能为空"));
        }
        let default = model
            .get("default_reasoning_level")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("{slug} 缺少 default_reasoning_level"))?;
        if !levels.iter().any(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|effort| effort == default)
        }) {
            return Err(format!("{slug} default_reasoning_level 不在支持列表中"));
        }
        max_priority = max_priority.max(json_priority(model));
        by_slug.insert(slug.to_string(), model.clone());
    }
    let default_template = by_slug
        .get(DEFAULT_TEMPLATE_SLUG)
        .cloned()
        .ok_or_else(|| "必须包含 gpt-5.5".to_string())?;
    Ok(CodexClientTemplates {
        by_slug,
        default_template,
        max_priority,
    })
}

pub(crate) fn replace_templates(payload: &Value) -> Result<u64, String> {
    let next = parse_templates(payload)?;
    let current = templates().read().map_err(|_| "模板锁已损坏".to_string())?;
    if current.by_slug == next.by_slug {
        return Ok(template_revision());
    }
    drop(current);
    *templates()
        .write()
        .map_err(|_| "模板锁已损坏".to_string())? = next;
    Ok(TEMPLATE_REVISION.fetch_add(1, Ordering::AcqRel) + 1)
}

static TEMPLATE_REVISION: AtomicU64 = AtomicU64::new(1);

pub(crate) fn template_revision() -> u64 {
    TEMPLATE_REVISION.load(Ordering::Acquire)
}

pub(super) fn codex_models_payload(models: &[LocalModelEntry], client_version: &str) -> Value {
    let mut entries = build_codex_models(models, client_version);
    let payload = json!({ "models": entries });
    if serde_json::to_vec(&payload)
        .map(|bytes| bytes.len() <= MAX_CODEX_CATALOG_BYTES)
        .unwrap_or(false)
    {
        return payload;
    }
    entries = compact_catalog_entries(entries);
    json!({ "models": entries })
}

const FALLBACK_INSTRUCTIONS: &str =
    "You are Codex, a coding agent. You and the user share one workspace.";

fn compact_catalog_entries(mut entries: Vec<Value>) -> Vec<Value> {
    for entry in &mut entries {
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        object.insert("base_instructions".into(), json!(FALLBACK_INSTRUCTIONS));
        object.insert(
            "model_messages".into(),
            json!({
                "instructions_template": FALLBACK_INSTRUCTIONS,
                "instructions_variables": null,
                "approvals": null,
                "collaboration_modes": null,
                "auto_review": null,
                "permissions": null,
                "multi_agent": null
            }),
        );
    }
    entries
}

fn build_codex_models(models: &[LocalModelEntry], client_version: &str) -> Vec<Value> {
    let templates = templates()
        .read()
        .expect("Codex client catalog lock poisoned");
    let mut result = Vec::with_capacity(models.len());
    let mut extra_indexes = Vec::new();
    for model in models {
        if model.id.is_empty() || model.id.contains('*') {
            continue;
        }
        let template = templates
            .by_slug
            .get(&model.id)
            .or_else(|| templates.by_slug.get(&model.metadata.id));
        let (mut entry, from_template) = if let Some(template) = template {
            (template.clone(), true)
        } else {
            (
                compact_fallback_template(&templates.default_template),
                false,
            )
        };
        apply_metadata_overrides(
            &mut entry,
            &model.metadata,
            from_template,
            model.metadata_route_count,
        );
        if let Some(object) = entry.as_object_mut() {
            object.insert("slug".into(), json!(model.id));
        }
        if !is_codex_chat_model(&model.capabilities)
            && let Some(object) = entry.as_object_mut()
        {
            object.insert("visibility".into(), json!("hide"));
        }
        sanitize_reasoning_levels(&mut entry, client_version);
        if !from_template {
            extra_indexes.push(result.len());
        }
        result.push(entry);
    }

    extra_indexes.sort_by_key(|&index| extra_display_name(&result[index]));
    for (rank, index) in extra_indexes.into_iter().enumerate() {
        if let Some(object) = result[index].as_object_mut() {
            object.insert(
                "priority".into(),
                json!(templates.max_priority + 100 * (rank as i64 + 1)),
            );
        }
    }

    result.sort_by_key(json_priority);
    result
}

fn compact_fallback_template(template: &Value) -> Value {
    let mut entry = template.clone();
    let Some(object) = entry.as_object_mut() else {
        return entry;
    };
    object.insert("base_instructions".into(), json!(FALLBACK_INSTRUCTIONS));
    object.insert(
        "model_messages".into(),
        json!({
            "instructions_template": FALLBACK_INSTRUCTIONS,
            "instructions_variables": null,
            "approvals": null,
            "collaboration_modes": null,
            "auto_review": null,
            "permissions": null,
            "multi_agent": null
        }),
    );
    object.insert("supports_search_tool".into(), json!(false));
    object.insert("prefer_websockets".into(), json!(false));
    object.insert("service_tiers".into(), json!([]));
    object.insert("apply_patch_tool_type".into(), Value::Null);
    object.insert("upgrade".into(), Value::Null);
    object.insert("availability_nux".into(), Value::Null);
    entry
}

fn is_pure_codex_provider(metadata: &ProviderModelMetadata) -> bool {
    !metadata.providers.is_empty()
        && metadata
            .providers
            .iter()
            .all(|provider| provider.contains("codex") || provider == "openai")
}

fn apply_metadata_overrides(
    entry: &mut Value,
    metadata: &ProviderModelMetadata,
    from_template: bool,
    metadata_route_count: usize,
) {
    let Some(object) = entry.as_object_mut() else {
        return;
    };
    if !from_template {
        object.insert(
            "display_name".into(),
            json!(metadata.display_name.as_deref().unwrap_or(&metadata.id)),
        );
        object.insert(
            "description".into(),
            json!(metadata.description.as_deref().unwrap_or(&metadata.id)),
        );
        if let Some(context) = metadata.context_window {
            object.insert("context_window".into(), json!(context));
        }
        if let Some(max_context) = metadata.max_context_window {
            object.insert("max_context_window".into(), json!(max_context));
        }
    }
    if let Some(context) = metadata.context_window {
        object.insert("context_window".into(), json!(context));
    }
    if let Some(max_context) = metadata.max_context_window {
        object.insert("max_context_window".into(), json!(max_context));
    }
    if let Some(max_output) = metadata.max_output_tokens {
        object.insert("max_tokens".into(), json!(max_output));
    }
    let apply_route_metadata = !from_template || metadata_route_count > 1;
    if apply_route_metadata && let Some(levels) = &metadata.reasoning_levels {
        let levels = levels
            .iter()
            .map(|effort| json!({ "effort": effort, "description": reasoning_description(effort) }))
            .collect::<Vec<_>>();
        if !levels.is_empty() {
            object.insert("supported_reasoning_levels".into(), json!(levels));
            let default = metadata
                .default_reasoning_level
                .as_deref()
                .filter(|level| levels.iter().any(|entry| entry["effort"] == *level))
                .unwrap_or_else(|| levels[0]["effort"].as_str().unwrap_or("none"));
            object.insert("default_reasoning_level".into(), json!(default));
        }
    }
    if apply_route_metadata && let Some(modalities) = &metadata.input_modalities {
        let modalities = modalities
            .iter()
            .map(|modality| modality.to_ascii_lowercase())
            .filter(|modality| modality == "text" || modality == "image")
            .collect::<Vec<_>>();
        let has_image = modalities.iter().any(|modality| modality == "image");
        object.insert("input_modalities".into(), json!(modalities));
        if has_image {
            object.insert("supports_image_detail_original".into(), json!(true));
        } else {
            object.remove("supports_image_detail_original");
        }
    }
    let should_clear_capabilities = !from_template
        || metadata.supports_search_tool != Some(true)
        || (metadata_route_count > 0 && !is_pure_codex_provider(metadata));
    if should_clear_capabilities {
        object.insert("supports_search_tool".into(), json!(false));
        object.insert("prefer_websockets".into(), json!(false));
        object.insert("service_tiers".into(), json!([]));
        object.insert("apply_patch_tool_type".into(), Value::Null);
        object.insert("upgrade".into(), Value::Null);
        object.insert("availability_nux".into(), Value::Null);
    }
}

fn reasoning_description(level: &str) -> &'static str {
    match level {
        "none" => "No reasoning",
        "minimal" => "Fastest responses with minimal reasoning",
        "low" => "Fast responses with lighter reasoning",
        "medium" => "Balances speed and reasoning depth for everyday tasks",
        "high" => "Greater reasoning depth for complex problems",
        "xhigh" => "Extra high reasoning depth for complex problems",
        "max" => "Maximum available reasoning depth for complex problems",
        _ => "Reasoning effort supported by this model",
    }
}

fn extra_display_name(entry: &Value) -> String {
    entry
        .get("display_name")
        .and_then(Value::as_str)
        .or_else(|| entry.get("slug").and_then(Value::as_str))
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn json_priority(entry: &Value) -> i64 {
    entry
        .get("priority")
        .and_then(Value::as_i64)
        .or_else(|| {
            entry
                .get("priority")
                .and_then(Value::as_u64)
                .map(|value| value as i64)
        })
        .unwrap_or(100)
}

fn sanitize_reasoning_levels(entry: &mut Value, client_version: &str) {
    if supports_extended_reasoning_levels(client_version) {
        return;
    }
    let Some(levels) = entry
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    levels.retain(|level| {
        level
            .get("effort")
            .and_then(Value::as_str)
            .is_some_and(|effort| LEGACY_REASONING_LEVELS.contains(&effort))
    });
    let allowed: Vec<String> = levels
        .iter()
        .filter_map(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    if allowed.is_empty() {
        if let Some(object) = entry.as_object_mut() {
            object.remove("supported_reasoning_levels");
            object.remove("default_reasoning_level");
        }
        return;
    }
    let default = entry
        .get("default_reasoning_level")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !allowed.iter().any(|effort| effort == default)
        && let Some(object) = entry.as_object_mut()
    {
        object.insert("default_reasoning_level".into(), json!(allowed[0]));
    }
}
