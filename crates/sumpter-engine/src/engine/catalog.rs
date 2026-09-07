//! Catalog implementation for the shared engine.

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use sumpter_core::config::AppConfig;

use super::context::header_value;
use super::http_response::{error_response, json_response};
use super::protocol::decoded_query_value;

#[path = "codex_client_catalog.rs"]
mod codex_client_catalog;

pub(super) fn supports_extended_reasoning_levels(client_version: &str) -> bool {
    let trimmed = client_version.trim().trim_start_matches('v');
    if trimmed.is_empty() {
        return true;
    }
    let parts = trimmed.split('.').collect::<Vec<_>>();
    if !(2..=3).contains(&parts.len()) {
        return true;
    }
    let Some(major) = parts[0].parse::<u64>().ok() else {
        return true;
    };
    let Some(minor) = parts[1].parse::<u64>().ok() else {
        return true;
    };
    let patch = parts
        .get(2)
        .and_then(|part| part.parse::<u64>().ok())
        .unwrap_or(0);
    (major, minor, patch) >= (0, 144, 0)
}

pub(super) fn is_local_models_path(path: &str) -> bool {
    if [
        "/v1/models",
        "/models",
        "/openai/v1/models",
        "/backend-api/codex/models",
    ]
    .contains(&path)
    {
        return true;
    }
    [
        "/v1/models/",
        "/models/",
        "/openai/v1/models/",
        "/backend-api/codex/models/",
    ]
    .iter()
    .any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|id| !id.is_empty() && !id.contains('/'))
    })
}

pub(super) fn local_model_id_from_path(path: &str) -> Option<&str> {
    [
        "/v1/models/",
        "/models/",
        "/openai/v1/models/",
        "/backend-api/codex/models/",
    ]
    .iter()
    .find_map(|prefix| path.strip_prefix(prefix))
    .filter(|id| !id.is_empty() && !id.contains('/'))
}

#[derive(Clone)]
pub(super) struct LocalModelEntry {
    pub(super) id: String,
    pub(super) capabilities: Vec<sumpter_core::capability::ModelCapability>,
}

pub(super) fn collect_local_models(
    config: &AppConfig,
    requested_capability: Option<sumpter_core::capability::ModelCapability>,
    requested_id: Option<&str>,
) -> Vec<LocalModelEntry> {
    let mut models =
        std::collections::BTreeMap::<String, Vec<sumpter_core::capability::ModelCapability>>::new();
    for scoped in config
        .routing_endpoints()
        .iter()
        .filter(|e| e.endpoint.enabled)
    {
        let endpoint = &scoped.endpoint;
        let mut concrete_models = std::collections::BTreeSet::new();
        for mapping in &endpoint.mappings {
            let pattern = mapping.client_pattern.trim();
            let cleaned_pattern = sumpter_core::model_name::clean(pattern);
            if cleaned_pattern.contains('*') {
                for model in endpoint
                    .catalog
                    .as_ref()
                    .into_iter()
                    .flat_map(|catalog| catalog.models.iter())
                    .map(|model| sumpter_core::model_name::clean(model))
                    .filter(|model| {
                        !model.is_empty()
                            && !model.contains('*')
                            && sumpter_core::model_name::pattern_matches(&cleaned_pattern, model)
                    })
                {
                    concrete_models.insert(model);
                }
            } else {
                let model = sumpter_core::capability::canonical_model_from_pattern(pattern);
                if !model.is_empty() && !model.contains('*') {
                    concrete_models.insert(model);
                }
            }
        }

        for model in concrete_models {
            if requested_id.is_some_and(|id| id != model) {
                continue;
            }
            // Directory classification must use the same capability-aware
            // precedence as the planner.  A precise text mapping must not
            // hide a broader image/video/live mapping for the same logical
            // model, and a mapping can intentionally advertise more than one
            // capability.  Resolve each capability independently, then union
            // only the capabilities that are actually routable.
            let wanted_capabilities = requested_capability
                .map(|wanted| vec![wanted])
                .unwrap_or_else(|| {
                    vec![
                        sumpter_core::capability::ModelCapability::Text,
                        sumpter_core::capability::ModelCapability::Image,
                        sumpter_core::capability::ModelCapability::Video,
                        sumpter_core::capability::ModelCapability::Live,
                        sumpter_core::capability::ModelCapability::Files,
                    ]
                });
            let mut resolved_capabilities = Vec::new();
            for wanted in wanted_capabilities {
                let Some(mapping) = endpoint.mapping_for_capability(&model, wanted) else {
                    continue;
                };
                let capabilities = sumpter_core::capability::capabilities_for_model(
                    &mapping.capabilities,
                    &mapping.client_pattern,
                    &model,
                );
                for capability in capabilities {
                    if !resolved_capabilities.contains(&capability) {
                        resolved_capabilities.push(capability);
                    }
                }
            }
            if !resolved_capabilities.is_empty() {
                let entry = models.entry(model).or_default();
                for capability in resolved_capabilities {
                    if !entry.contains(&capability) {
                        entry.push(capability);
                    }
                }
            }
        }
    }
    models
        .into_iter()
        .map(|(id, mut capabilities)| {
            capabilities.sort_by_key(|capability| capability.as_str());
            LocalModelEntry { id, capabilities }
        })
        .collect()
}

pub(super) fn openai_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "object": "model",
        "created": 0,
        "owned_by": "sumpter",
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
    })
}

pub(super) fn is_media_only_conversation_model(model: &str) -> bool {
    let capabilities = sumpter_core::capability::inferred_capabilities(model);
    let media = capabilities.iter().any(|capability| {
        matches!(
            capability,
            sumpter_core::capability::ModelCapability::Image
                | sumpter_core::capability::ModelCapability::Video
        )
    });
    media && !capabilities.contains(&sumpter_core::capability::ModelCapability::Text)
}

pub(super) fn is_codex_chat_model(
    capabilities: &[sumpter_core::capability::ModelCapability],
) -> bool {
    capabilities.contains(&sumpter_core::capability::ModelCapability::Text)
        && !capabilities.iter().any(|capability| {
            matches!(
                capability,
                sumpter_core::capability::ModelCapability::Image
                    | sumpter_core::capability::ModelCapability::Video
                    | sumpter_core::capability::ModelCapability::Live
            )
        })
}

pub(super) fn grok_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "model": model.id,
        "name": model.id,
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
        "api_backend": if is_codex_chat_model(&model.capabilities) {
            "responses"
        } else {
            "chat"
        },
    })
}

pub(super) fn anthropic_model_object(model: &LocalModelEntry) -> Value {
    json!({
        "id": model.id,
        "type": "model",
        "display_name": model.id,
        "capabilities": model.capabilities.iter().map(|capability| capability.as_str()).collect::<Vec<_>>(),
    })
}

pub(super) fn local_models_json(
    config: &AppConfig,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
) -> Result<Value, StatusCode> {
    let requested_capability = query
        .and_then(|query| decoded_query_value(query, "capability"))
        .and_then(|value| match value.to_ascii_lowercase().as_str() {
            "text" => Some(sumpter_core::capability::ModelCapability::Text),
            "image" => Some(sumpter_core::capability::ModelCapability::Image),
            "video" => Some(sumpter_core::capability::ModelCapability::Video),
            "live" | "realtime" => Some(sumpter_core::capability::ModelCapability::Live),
            "files" => Some(sumpter_core::capability::ModelCapability::Files),
            _ => None,
        });
    let requested_id = local_model_id_from_path(path);
    let models = collect_local_models(config, requested_capability, requested_id);
    if let Some(id) = requested_id {
        let Some(model) = models.iter().find(|model| model.id == id) else {
            return Err(StatusCode::NOT_FOUND);
        };
        return Ok(openai_model_object(model));
    }
    if let Some(client_version) =
        query.and_then(|query| decoded_query_value(query, "client_version"))
    {
        return Ok(codex_client_catalog::codex_models_payload(
            &models,
            &client_version,
        ));
    }
    let user_agent = header_value(headers, "user-agent").unwrap_or("");
    let grok_shell = user_agent.to_ascii_lowercase().contains("grok-shell");
    let claude_cli = user_agent.starts_with("claude-cli/")
        || header_value(headers, "anthropic-version").is_some();
    if grok_shell {
        return Ok(json!({
            "object": "list",
            "data": models.iter().map(grok_model_object).collect::<Vec<_>>(),
        }));
    }
    if claude_cli {
        let data = models
            .iter()
            .map(anthropic_model_object)
            .collect::<Vec<_>>();
        let first_id = data.first().and_then(|value| value["id"].as_str());
        let last_id = data.last().and_then(|value| value["id"].as_str());
        return Ok(json!({
            "data": data,
            "has_more": false,
            "first_id": first_id,
            "last_id": last_id,
        }));
    }
    Ok(json!({
        "object": "list",
        "data": models.iter().map(openai_model_object).collect::<Vec<_>>(),
    }))
}

pub(super) fn local_models_response(
    config: &AppConfig,
    path: &str,
    query: Option<&str>,
    headers: &[(String, String)],
) -> Response {
    match local_models_json(config, path, query, headers) {
        Ok(body) => json_response(StatusCode::OK, &body),
        Err(StatusCode::NOT_FOUND) => {
            error_response(StatusCode::NOT_FOUND, &[("error", "model_not_found")])
        }
        Err(status) => error_response(status, &[("error", "model_not_found")]),
    }
}
