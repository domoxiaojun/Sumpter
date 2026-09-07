//! catalog regression tests.

use axum::http::StatusCode;
use serde_json::{Value, json};
use sumpter_core::config::AppConfig;

use crate::engine::catalog::{local_models_json, supports_extended_reasoning_levels};

#[test]
fn codex_model_reasoning_levels_follow_client_version() {
    assert!(!supports_extended_reasoning_levels("0.143.9"));
    assert!(supports_extended_reasoning_levels("0.144.0"));
    assert!(supports_extended_reasoning_levels("v0.149.1"));
    assert!(supports_extended_reasoning_levels(""));
    assert!(supports_extended_reasoning_levels("latest"));
    assert!(supports_extended_reasoning_levels("0"));
}

fn catalog_config() -> AppConfig {
    AppConfig::from_json(
        r#"{
              "schemaVersion": 6,
              "listener": {"host":"127.0.0.1","port":0},
              "endpoints": [{
                "id": "cpa",
                "name": "CPA",
                "baseURL": "https://cpa.example.invalid",
                  "apiKey": "sk",
                  "protocol": "openai",
                  "enabled": true,
                  "catalog": {
                    "models": ["gpt-4o", "gpt-5.6-sol", "gpt-image-2", "gpt-ignored-other"]
                  },
                  "mappings": [
                  {"clientPattern": "gpt-5.6-sol", "upstreamModel": "gpt-5.6-sol"},
                  {"clientPattern": "gpt-4o", "upstreamModel": "gpt-4o"},
                  {"clientPattern": "*", "upstreamModel": "star"},
                  {"clientPattern": "gpt-image-2", "upstreamModel": "gpt-image-2"}
                ]
              }]
            }"#,
    )
    .expect("catalog fixture")
    .normalized()
}

#[test]
fn local_models_catalog_uses_openai_list_shape() {
    let json = local_models_json(&catalog_config(), "/v1/models", None, &[]).expect("catalog");
    assert_eq!(json["object"], "list");
    let ids: Vec<&str> = json["data"]
        .as_array()
        .expect("data")
        .iter()
        .filter_map(|model| model["id"].as_str())
        .collect();
    assert_eq!(
        ids,
        vec!["gpt-4o", "gpt-5.6-sol", "gpt-ignored-other", "gpt-image-2"]
    );
    assert!(json["data"][0].get("endpointIDs").is_none());
    assert_eq!(json["data"][0]["capabilities"], json!(["text"]));
    assert_eq!(json["data"][0]["owned_by"], "sumpter");
}

#[test]
fn local_models_catalog_uses_codex_client_version_shape() {
    let json = local_models_json(
        &catalog_config(),
        "/v1/models",
        Some("client_version=0.149.1"),
        &[],
    )
    .expect("catalog");
    assert!(json.get("data").is_none());
    let models = json["models"].as_array().expect("models");
    let chat = models
        .iter()
        .find(|model| model["slug"] == "gpt-5.6-sol")
        .expect("chat model");
    assert_eq!(chat["display_name"], "GPT-5.6-Sol");
    assert_eq!(chat["default_reasoning_level"], "low");
    assert_eq!(chat["shell_type"], "shell_command");
    assert!(
        chat["base_instructions"]
            .as_str()
            .is_some_and(|value| value.contains("Codex"))
    );
    assert_eq!(chat["truncation_policy"]["mode"], "tokens");
    let gpt4o = models
        .iter()
        .find(|model| model["slug"] == "gpt-4o")
        .expect("gpt-4o stays a chat model");
    assert_eq!(gpt4o["slug"], "gpt-4o");
    assert_eq!(gpt4o["display_name"], "gpt-4o");
    assert_ne!(
        gpt4o.get("visibility").and_then(Value::as_str),
        Some("hide")
    );
    assert_eq!(gpt4o["shell_type"], "shell_command");
    assert!(
        gpt4o
            .get("base_instructions")
            .and_then(Value::as_str)
            .is_some()
    );
    assert!(
        gpt4o["priority"].as_i64().unwrap_or_default()
            > chat["priority"].as_i64().unwrap_or_default()
    );
    let efforts: Vec<&str> = chat["supported_reasoning_levels"]
        .as_array()
        .expect("levels")
        .iter()
        .filter_map(|level| level["effort"].as_str())
        .collect();
    assert!(efforts.contains(&"xhigh"));
    assert!(efforts.contains(&"max"));
    let image = models
        .iter()
        .find(|model| model["slug"] == "gpt-image-2")
        .expect("image model");
    assert_eq!(image["visibility"], "hide");
    assert_eq!(image["slug"], "gpt-image-2");
    assert!(
        image
            .get("base_instructions")
            .and_then(Value::as_str)
            .is_some()
    );
    assert!(models.iter().all(|model| model["slug"] != "*"));
}

#[test]
fn local_models_catalog_hides_extended_reasoning_for_old_codex() {
    let json = local_models_json(
        &catalog_config(),
        "/v1/models",
        Some("client_version=0.143.9"),
        &[],
    )
    .expect("catalog");
    let chat = json["models"]
        .as_array()
        .expect("models")
        .iter()
        .find(|model| model["slug"] == "gpt-5.6-sol")
        .expect("chat model");
    let efforts: Vec<&str> = chat["supported_reasoning_levels"]
        .as_array()
        .expect("levels")
        .iter()
        .filter_map(|level| level["effort"].as_str())
        .collect();
    assert!(efforts.contains(&"xhigh"));
    assert!(!efforts.contains(&"max"));
    assert!(!efforts.contains(&"ultra"));
}

#[test]
fn local_models_catalog_lookup_and_unknown_id() {
    let found = local_models_json(&catalog_config(), "/v1/models/gpt-5.6-sol", None, &[])
        .expect("known model");
    assert_eq!(found["id"], "gpt-5.6-sol");
    assert_eq!(
        local_models_json(&catalog_config(), "/v1/models/not-a-model", None, &[]).unwrap_err(),
        StatusCode::NOT_FOUND
    );
    let cursor = local_models_json(&catalog_config(), "/v1/models", Some("cursor=next"), &[])
        .expect("cursor stays local");
    assert_eq!(cursor["object"], "list");
    let grok = local_models_json(
        &catalog_config(),
        "/v1/models",
        None,
        &[("user-agent".into(), "grok-shell/1.0".into())],
    )
    .expect("grok catalog");
    assert_eq!(grok["data"][0]["api_backend"], "responses");
    let claude = local_models_json(
        &catalog_config(),
        "/v1/models",
        None,
        &[("anthropic-version".into(), "2023-06-01".into())],
    )
    .expect("anthropic catalog");
    assert_eq!(claude["has_more"], false);
    assert!(claude["data"].as_array().is_some());
    let decoded = local_models_json(
        &catalog_config(),
        "/v1/models",
        Some("client_version=0.149.1"),
        &[],
    )
    .expect("plain version");
    let encoded = local_models_json(
        &catalog_config(),
        "/v1/models",
        Some("client_version=0%2E149%2E1"),
        &[],
    )
    .expect("encoded version");
    let decoded_sol = decoded["models"]
        .as_array()
        .expect("decoded models")
        .iter()
        .find(|model| model["slug"] == "gpt-5.6-sol")
        .expect("sol");
    let encoded_sol = encoded["models"]
        .as_array()
        .expect("encoded models")
        .iter()
        .find(|model| model["slug"] == "gpt-5.6-sol")
        .expect("sol");
    assert_eq!(decoded_sol["default_reasoning_level"], "low");
    assert_eq!(encoded_sol["default_reasoning_level"], "low");
}

#[test]
fn local_models_catalog_resolves_each_capability_independently() {
    let config: AppConfig = serde_json::from_value::<AppConfig>(json!({
        "schemaVersion": 6,
        "listener": {"host": "127.0.0.1", "port": 0},
        "endpoints": [{
            "id": "mixed",
            "name": "mixed",
            "baseURL": "https://provider.invalid",
            "protocol": "openai",
            "enabled": true,
            "apiKey": "",
            "catalog": {"models": ["gpt-image-2"]},
            "mappings": [
                {"clientPattern": "gpt-image-2", "capabilities": ["text"]},
                {"clientPattern": "gpt-image-*", "capabilities": ["image"]}
            ]
        }]
    }))
    .expect("catalog config")
    .normalized();
    let all = local_models_json(&config, "/v1/models", None, &[]).expect("catalog");
    let entry = all["data"]
        .as_array()
        .and_then(|models| models.iter().find(|model| model["id"] == "gpt-image-2"))
        .expect("image model");
    let capabilities = entry["capabilities"].as_array().expect("capabilities");
    assert!(capabilities.iter().any(|value| value == "text"));
    assert!(capabilities.iter().any(|value| value == "image"));

    let image_only = local_models_json(&config, "/v1/models", Some("capability=image"), &[])
        .expect("image catalog");
    assert_eq!(image_only["data"][0]["id"], "gpt-image-2");
}
