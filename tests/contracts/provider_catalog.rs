use axum::{Json, Router, http::HeaderMap, routing::get};
use serde_json::json;
use sumpter_core::config::{AppConfig, EndpointProtocolMode};
use sumpter_engine::model_catalog::{ProviderCatalogFetcher, extract_models};

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
    let result = ProviderCatalogFetcher::fetch(&endpoint).await;
    assert_eq!(result.error, None);
    let models = result.models;
    assert!(models.contains(&"gemini-dynamic-review".to_string()));
    assert!(models.contains(&"claude-synthetic-gemini-alias".to_string()));
    endpoint.protocol = EndpointProtocolMode::OpenAI;
    let result = ProviderCatalogFetcher::fetch(&endpoint).await;
    assert_eq!(result.error, None);
    let models = result.models;
    task.abort();
    assert_eq!(models, ["gemini-dynamic-review", "gpt-synthetic"]);
}

#[test]
fn catalog_extracts_codex_slugs_and_gemini_resource_names() {
    assert_eq!(
        extract_models(&json!({"models":[{"slug":"gemini-dynamic-review"}]})),
        ["gemini-dynamic-review"]
    );
    assert_eq!(
        extract_models(&json!({"models":[{"name":"models/gemini-dynamic-review"}]})),
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

#[tokio::test]
async fn gemini_catalog_uses_native_auth_versions_and_all_pages() {
    use axum::extract::Query;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let router = Router::new().fallback(get(move |uri: axum::http::Uri, headers: HeaderMap, Query(query): Query<HashMap<String, String>>| {
        let recorded = recorded.clone();
        async move {
            assert!(uri.path().ends_with("/models"));
            assert_eq!(headers.get("x-goog-api-key").unwrap(), "synthetic-gemini-key");
            assert!(!headers.contains_key("authorization"));
            assert!(!headers.contains_key("x-api-key"));
            assert!(!headers.contains_key("anthropic-version"));
            assert_eq!(query.get("pageSize").map(String::as_str), Some("1000"));
            recorded.lock().unwrap().push(uri.path().to_string());
            if let Some(token) = query.get("pageToken") {
                assert_eq!(token, "opaque+/=& token");
                Json(json!({"models":[{"name":"models/gemini-embedding", "supportedGenerationMethods":["embedContent"]}]}))
            } else {
                Json(json!({"models":[{"name":"models/gemini-test"}],"nextPageToken":"opaque+/=& token"}))
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for (suffix, expected_path) in [
        ("", "/v1beta/models"),
        ("/v1beta", "/v1beta/models"),
        ("/v1", "/v1/models"),
        ("/proxy/v1alpha/", "/proxy/v1alpha/models"),
    ] {
        let config: AppConfig = serde_json::from_value(json!({"endpoints":[{"id":"gemini", "protocol":"gemini", "baseURL":format!("http://{address}{suffix}"), "apiKey":"synthetic-gemini-key"}]})).unwrap();
        let result = ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
        assert_eq!(result.error, None);
        assert_eq!(result.models, ["gemini-embedding", "gemini-test"]);
        assert!(result.route_capabilities.contains_key("gemini-test"));
        assert!(!result.route_capabilities.contains_key("models/gemini-test"));
        let paths = seen.lock().unwrap();
        assert!(paths.iter().rev().take(2).all(|path| path == expected_path));
    }
    assert_eq!(seen.lock().unwrap().len(), 8);
    task.abort();
}

#[tokio::test]
async fn gemini_catalog_rejects_failed_or_cyclic_pagination_without_partial_models() {
    use axum::{extract::Query, response::IntoResponse};
    use std::collections::HashMap;
    for cyclic in [false, true] {
        let router = Router::new().route("/v1beta/models", get(move |Query(query): Query<HashMap<String, String>>| async move {
            if !cyclic && query.contains_key("pageToken") {
                return axum::http::StatusCode::FORBIDDEN.into_response();
            }
            Json(json!({"models":[{"name":"models/gemini-partial"}],"nextPageToken":"same-token"})).into_response()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let config: AppConfig = serde_json::from_value(json!({"endpoints":[{"id":"gemini", "protocol":"gemini", "baseURL":format!("http://{address}"), "apiKey":"synthetic"}]})).unwrap();
        let result = ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
        assert!(result.models.is_empty());
        assert!(
            result
                .error
                .unwrap()
                .contains(if cyclic { "分页重复" } else { "403" })
        );
        task.abort();
    }
}

#[tokio::test]
async fn gemini_catalog_auto_mode_and_oauth_bearer() {
    let router = Router::new().route(
        "/v1beta/models",
        get(|headers: HeaderMap| async move {
            assert_eq!(
                headers.get("authorization").unwrap(),
                "Bearer synthetic-oauth"
            );
            assert!(!headers.contains_key("x-goog-api-key"));
            Json(json!({"models":[{"name":"models/gemini-oauth"}]}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let config: AppConfig = serde_json::from_value(json!({"endpoints":[{"id":"auto", "protocol":"auto", "baseURL":format!("http://{address}"), "apiKey":"Bearer synthetic-oauth"}]})).unwrap();
    let result = ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
    assert_eq!(result.error, None);
    assert_eq!(result.models, ["gemini-oauth"]);
    task.abort();
}
