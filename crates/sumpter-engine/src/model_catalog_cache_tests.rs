use super::*;
use axum::{Router, body::Body, http::HeaderMap, response::Response};
use std::sync::atomic::AtomicUsize;

async fn server() -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let all = Arc::new(AtomicUsize::new(0));
    let conditional = Arc::new(AtomicUsize::new(0));
    let a = all.clone();
    let c = conditional.clone();
    let app = Router::new().fallback(move |headers: HeaderMap| {
        let a = a.clone();
        let c = c.clone();
        async move {
            a.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(30)).await;
            if headers
                .get("if-none-match")
                .is_some_and(|v| v == "\"revision-one\"")
            {
                c.fetch_add(1, Ordering::SeqCst);
                Response::builder().status(304).body(Body::empty()).unwrap()
            } else {
                Response::builder()
                    .header("etag", "\"revision-one\"")
                    .body(Body::from(r#"{"data":[{"id":"fixture-model"}]}"#))
                    .unwrap()
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (base, all, conditional, task)
}

#[tokio::test]
async fn conditional_catalog_reuses_304_body_but_never_crosses_credentials() {
    let (base, all, conditional, task) = server().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let scope = "endpoint-a:resolve-ip-a";
    let first = send_catalog_request(
        &client,
        client
            .get(&base)
            .header("authorization", "Bearer synthetic-a"),
        scope,
    )
    .await
    .unwrap();
    let body = read_body(first).await.unwrap();
    let second = send_catalog_request(
        &client,
        client
            .get(&base)
            .header("authorization", "Bearer synthetic-a"),
        scope,
    )
    .await
    .unwrap();
    assert_eq!(read_body(second).await.unwrap(), body);
    let mut secret = reqwest::header::HeaderValue::from_static("Bearer synthetic-b");
    secret.set_sensitive(true);
    let third = send_catalog_request(
        &client,
        client.get(&base).header("authorization", secret),
        scope,
    )
    .await
    .unwrap();
    assert_eq!(read_body(third).await.unwrap(), body);
    let different_ip = send_catalog_request(
        &client,
        client
            .get(&base)
            .header("authorization", "Bearer synthetic-a"),
        "endpoint-a:resolve-ip-b",
    )
    .await
    .unwrap();
    assert_eq!(read_body(different_ip).await.unwrap(), body);
    let mut different_secret = reqwest::header::HeaderValue::from_static("Bearer synthetic-c");
    different_secret.set_sensitive(true);
    let _ = send_catalog_request(
        &client,
        client.get(&base).header("authorization", different_secret),
        scope,
    )
    .await
    .unwrap();
    assert_eq!(all.load(Ordering::SeqCst), 5);
    assert_eq!(conditional.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn overlapping_catalog_refreshes_share_one_probe() {
    let (base, all, _, task) = server().await;
    let endpoint: Endpoint = serde_json::from_value(serde_json::json!({"id":"fixture", "baseURL":base,"protocol":"gemini","apiKey":"synthetic"})).unwrap();
    let results =
        futures_util::future::join_all((0..12).map(|_| ProviderCatalogFetcher::fetch(&endpoint)))
            .await;
    assert!(
        results
            .iter()
            .all(|r| r.error.is_none() && r.models == ["fixture-model"])
    );
    assert_eq!(all.load(Ordering::SeqCst), 1);
    task.abort();
}
