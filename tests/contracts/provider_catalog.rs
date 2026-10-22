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

#[tokio::test]
async fn cpa_reversed_aliases_do_not_double_model_count() {
    let router = Router::new().route(
        "/v1/models",
        get(|headers: HeaderMap| async move {
            let claude = headers.contains_key("anthropic-version");
            if claude {
                Json(json!({"data":[{"id":"claude-fable-5-dd-los-1.6-tpg"}]}))
            } else {
                Json(json!({"data":[{"id":"gpt-6.1-sol"}]}))
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
            "apiKey":"synthetic-catalog-key","protocol":"auto","enabled":true}]
    }))
    .unwrap();
    let snapshot =
        sumpter_engine::model_catalog::ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
    task.abort();
    assert_eq!(snapshot.error, None);
    assert_eq!(snapshot.models, ["gpt-6.1-sol"]);
}

#[tokio::test]
async fn cpa_capability_discovery_does_not_expand_model_membership() {
    let router = Router::new().route("/v1/models", get(|uri: axum::http::Uri| async move {
        if uri.query() == Some("client_version=cpa") {
            Json(json!({"models":[
                {"slug":"synthetic", "supports_search_tool":true,"prefer_websockets":true,"service_tiers":[{"id":"priority"}],"cpa_capabilities":{"web_search":false}},
                {"slug":"hidden-extra", "supports_search_tool":true},
                {"slug":"denied", "supports_search_tool":true,"prefer_websockets":true,"service_tiers":[{"id":"priority"}],"cpa_capabilities":{"web_search":true}}
            ]}))
        } else { Json(json!({"data":[{"id":"synthetic"}, {"id":"denied","supports_search_tool":false,"prefer_websockets":false,"service_tiers":[],"supports_web_search":false}]})) }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let config: AppConfig = serde_json::from_value(json!({"schemaVersion":7,"endpoints":[{
        "id":"cpa","name":"CPA","baseURL":format!("http://{address}"),"protocol":"openai-responses","enabled":true,
        "mappings":[{"clientPattern":"synthetic"}]
    }]})).unwrap();
    let snapshot =
        sumpter_engine::model_catalog::ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
    task.abort();
    assert_eq!(snapshot.error, None);
    assert_eq!(snapshot.models, ["denied", "synthetic"]);
    assert_eq!(snapshot.route_capabilities.len(), 2);
    let caps = &snapshot.route_capabilities["synthetic"];
    assert_eq!(caps.supports_search_tool, Some(true));
    assert_eq!(caps.supports_web_search, Some(false));
    assert_eq!(caps.prefer_websockets, Some(true));
    assert_eq!(caps.service_tiers, Some(vec!["priority".into()]));
    let denied = &snapshot.route_capabilities["denied"];
    assert_eq!(denied.supports_search_tool, Some(false));
    assert_eq!(denied.supports_web_search, Some(false));
    assert_eq!(denied.prefer_websockets, Some(false));
    assert_eq!(denied.service_tiers, Some(vec![]));
}

#[tokio::test]
async fn optional_capability_timeout_keeps_successful_model_discovery() {
    let router = Router::new().route(
        "/v1/models",
        get(|uri: axum::http::Uri| async move {
            if uri.query().is_some() {
                std::future::pending::<()>().await;
            }
            Json(json!({"data":[{"id":"synthetic"}]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let config: AppConfig = serde_json::from_value(json!({"schemaVersion":7,"endpoints":[{
        "id":"cpa","baseURL":format!("http://{address}"),"protocol":"openai-responses","enabled":true
    }]})).unwrap();
    let snapshot =
        sumpter_engine::model_catalog::ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
    task.abort();
    assert_eq!(snapshot.error, None);
    assert_eq!(snapshot.models, ["synthetic"]);
    assert_eq!(
        snapshot.route_capabilities["synthetic"].supports_search_tool,
        None
    );
}
