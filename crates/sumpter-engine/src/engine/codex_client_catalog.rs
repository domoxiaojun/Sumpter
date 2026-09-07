//! Codex `{models:[...]}` catalog built like CLIProxyAPI: clone official
//! templates by slug, otherwise clone `gpt-5.5` and only rewrite identity.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::{Value, json};

use super::LocalModelEntry;
use super::is_codex_chat_model;
use super::supports_extended_reasoning_levels;

const CODEX_CLIENT_MODELS_JSON: &str = include_str!("codex_client_models.json");
const DEFAULT_TEMPLATE_SLUG: &str = "gpt-5.5";
const LEGACY_REASONING_LEVELS: &[&str] = &["none", "minimal", "low", "medium", "high", "xhigh"];

struct CodexClientTemplates {
    by_slug: HashMap<String, Value>,
    default_template: Value,
    max_priority: i64,
}

fn templates() -> &'static CodexClientTemplates {
    static TEMPLATES: OnceLock<CodexClientTemplates> = OnceLock::new();
    TEMPLATES.get_or_init(load_templates)
}

fn load_templates() -> CodexClientTemplates {
    let payload: Value = serde_json::from_str(CODEX_CLIENT_MODELS_JSON)
        .expect("embedded Codex client catalog must be valid JSON");
    let models = payload
        .get("models")
        .and_then(Value::as_array)
        .expect("embedded Codex client catalog must have models[]");
    let mut by_slug = HashMap::new();
    let mut max_priority = 0_i64;
    for model in models {
        let Some(slug) = model.get("slug").and_then(Value::as_str) else {
            continue;
        };
        let slug = slug.trim();
        if slug.is_empty() {
            continue;
        }
        max_priority = max_priority.max(json_priority(model));
        by_slug.insert(slug.to_string(), model.clone());
    }
    let default_template = by_slug
        .get(DEFAULT_TEMPLATE_SLUG)
        .cloned()
        .expect("embedded Codex client catalog must include gpt-5.5");
    CodexClientTemplates {
        by_slug,
        default_template,
        max_priority,
    }
}

pub(super) fn codex_models_payload(models: &[LocalModelEntry], client_version: &str) -> Value {
    json!({ "models": build_codex_models(models, client_version) })
}

fn build_codex_models(models: &[LocalModelEntry], client_version: &str) -> Vec<Value> {
    let templates = templates();
    let mut result = Vec::with_capacity(models.len());
    let mut extra_indexes = Vec::new();
    for model in models {
        if model.id.is_empty() || model.id.contains('*') {
            continue;
        }
        let (mut entry, from_template) = if let Some(template) = templates.by_slug.get(&model.id) {
            (template.clone(), true)
        } else {
            (templates.default_template.clone(), false)
        };
        if !from_template && let Some(object) = entry.as_object_mut() {
            object.insert("slug".into(), json!(model.id));
            object.insert("display_name".into(), json!(model.id));
            object.insert("description".into(), json!(model.id));
            object.insert("prefer_websockets".into(), json!(false));
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
