use axum::{Json, Router, http::HeaderMap, routing::get};
use serde_json::json;
use sumpter_core::config::{AppConfig, EndpointProtocolMode};

#[tokio::test]
async fn cpa_catalog_preserves_raw_gemini_names_across_probe_identities() {
    let router = Router::new().route(
        "/v1/models",
        get(|headers: HeaderMap| async move {
            let claude = headers.contains_key("anthropic-version")
                || headers
                    .get("user-agent")
                    .and_then(|value| value.to_str().ok())
                    .is_some_and(|value| value.starts_with("claude-cli"));
            if claude {
                Json(json!({"data":[{"id":"claude-synthetic-gemini-alias"}]}))
            } else {
                Json(json!({"data":[{"id":"gemini-dynamic-review"},{"id":"gpt-synthetic"}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let config: AppConfig = serde_json::from_value(json!({
        "schemaVersion":7,
        "endpoints":[{"id":"cpa","name":"CPA","baseURL":format!("http://{address}"),
            "apiKey":"synthetic-catalog-key","protocol":"anthropic","enabled":true,
            "mappings":[{"clientPattern":"*","upstreamModel":""}]}]
    }))
    .unwrap();
    let mut endpoint = config.endpoints[0].clone();
    let result = super::fetch_provider_models_inner(&endpoint).await;
    let (models, _) = result.unwrap();
    assert!(models.contains(&"gemini-dynamic-review".to_string()));
    assert!(models.contains(&"claude-synthetic-gemini-alias".to_string()));
    endpoint.protocol = EndpointProtocolMode::OpenAI;
    let (models, _) = super::fetch_provider_models_inner(&endpoint).await.unwrap();
    task.abort();
    assert_eq!(models, ["gemini-dynamic-review", "gpt-synthetic"]);
}

#[test]
fn catalog_extracts_codex_slugs_and_gemini_resource_names() {
    assert_eq!(
        super::extract_models(&json!({"models":[{"slug":"gemini-dynamic-review"}]})),
        ["gemini-dynamic-review"]
    );
    assert_eq!(
        super::extract_models(&json!({"models":[{"name":"models/gemini-dynamic-review"}]})),
        ["models/gemini-dynamic-review"]
    );
}
