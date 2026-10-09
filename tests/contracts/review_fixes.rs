//! Data-plane and configuration regression contract, run by both adapters.
use super::*;
use sumpter_engine::model_catalog::{
    CatalogApplyOutcome, ProviderCatalogSnapshot, endpoint_fingerprint,
};

fn snapshot(endpoint: &Endpoint) -> ProviderCatalogSnapshot {
    ProviderCatalogSnapshot {
        endpoint_id: endpoint.id.clone(),
        endpoint_fingerprint: endpoint_fingerprint(endpoint),
        models: vec!["synthetic-model".into()],
        source: "synthetic-provider".into(),
        attempted_at: "1".into(),
        updated_at: "1".into(),
        error: None,
    }
}

#[tokio::test]
async fn stale_catalog_results_do_not_touch_memory_or_disk() {
    let dir = temp_config_dir("stale-catalog");
    let mut base = two_endpoint_config();
    base.schema_version = 7;
    let original = snapshot(&base.endpoints[0]);
    let engine = engine_with_dir(base.clone(), dir.clone(), FakeTransport::new());
    for field in 0..7 {
        let mut changed = base.clone();
        match field {
            0 => changed.endpoints[0].base_url = "https://edited.invalid".into(),
            1 => changed.endpoints[0].api_key = "synthetic-new-key".into(),
            2 => changed.endpoints[0].resolve_ip = "127.0.0.2".into(),
            3 => changed.endpoints[0].protocol = EndpointProtocolMode::Auto,
            4 => changed.endpoints[0].enabled = false,
            5 => {
                changed.endpoints.remove(0);
            }
            _ => changed.endpoints[0].id = "replacement".into(),
        }
        {
            let _guard = engine.config_transaction().await;
            let _ = dir.save_config(&changed).unwrap();
            engine.replace_config(changed.clone());
        }
        let bytes = std::fs::read(dir.config_path()).unwrap();
        assert_eq!(
            engine
                .apply_provider_catalog_snapshot(&original)
                .await
                .unwrap(),
            CatalogApplyOutcome::Stale
        );
        assert_eq!(*engine.config(), changed);
        assert_eq!(std::fs::read(dir.config_path()).unwrap(), bytes);
    }
    drop(engine);
    std::fs::remove_dir_all(dir.root).unwrap();
}

#[tokio::test]
async fn concurrent_catalog_results_merge_and_failures_keep_last_success() {
    let dir = temp_config_dir("catalog-merge");
    let mut config = two_endpoint_config();
    config.schema_version = 7;
    let first = snapshot(&config.endpoints[0]);
    let second = snapshot(&config.endpoints[1]);
    let engine = engine_with_dir(config, dir.clone(), FakeTransport::new());
    let (a, b) = tokio::join!(
        engine.apply_provider_catalog_snapshot(&first),
        engine.apply_provider_catalog_snapshot(&second)
    );
    assert_eq!(a.unwrap(), CatalogApplyOutcome::Applied);
    assert_eq!(b.unwrap(), CatalogApplyOutcome::Applied);
    assert!(
        engine
            .config()
            .endpoints
            .iter()
            .all(|e| e.catalog.as_ref().unwrap().models == first.models)
    );
    let mut failed = first.clone();
    failed.attempted_at = "2".into();
    failed.error = Some("synthetic timeout".into());
    failed.models.clear();
    assert_eq!(
        engine
            .apply_provider_catalog_snapshot(&failed)
            .await
            .unwrap(),
        CatalogApplyOutcome::Applied
    );
    let current = engine.config();
    let catalog = current.endpoints[0].catalog.as_ref().unwrap();
    assert_eq!(catalog.models, first.models);
    assert_eq!(catalog.updated_at, "1");
    assert_eq!(catalog.attempted_at, "2");
    assert_eq!(dir.load_config().unwrap(), *current);
    drop(engine);
    std::fs::remove_dir_all(dir.root).unwrap();
}

#[tokio::test]
async fn connection_nominated_headers_stay_on_their_hop() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("Connection".into(), " X-Hop-Secret, ,bad token".into()),
                ("X-Hop-Secret".into(), "synthetic-response-secret".into()),
                ("x-public".into(), "kept".into()),
            ],
            chunks: vec![br#"{"id":"resp_test","status":"completed","output":[]}"#.to_vec()],
        },
    );
    let engine = engine_with(config, fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/responses",
            vec![
                ("content-type".into(), "application/json".into()),
                ("Connection".into(), "x-hop-secret, ,bad token".into()),
                ("connection".into(), "X-Other".into()),
                ("X-Hop-Secret".into(), "synthetic-request-secret".into()),
                ("x-other".into(), "private".into()),
                ("x-public".into(), "one".into()),
                ("x-public".into(), "two".into()),
            ],
            Body::from(r#"{"model":"claude-opus-5","input":"hello","stream":false}"#),
        )
        .await;
    assert_eq!(response.status(), 200);
    assert!(!response.headers().contains_key("x-hop-secret"));
    assert!(!response.headers().contains_key("connection"));
    assert_eq!(response.headers()["x-public"], "kept");
    let _ = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let requests = fake.requests();
    assert_eq!(requests.len(), 1);
    assert!(
        !requests[0]
            .headers
            .iter()
            .any(|(name, _)| ["x-hop-secret", "x-other", "connection"].contains(&name.as_str()))
    );
    assert_eq!(
        requests[0]
            .headers
            .iter()
            .filter(|(name, _)| name == "x-public")
            .count(),
        1
    );
}
