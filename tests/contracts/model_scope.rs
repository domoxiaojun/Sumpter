//! Endpoint model declarations bound routing on both platform adapters.
use super::*;

#[tokio::test]
async fn model_scope_reload_revokes_sticky_and_failover_candidates() {
    for selection in [json!(null), json!(["gpt-6-astra"])] {
        let dir = temp_config_dir("model-scope");
        let fake = FakeTransport::new();
        let mut config = two_endpoint_config();
        config.schema_version = 7;
        config.retry.max_500_retries = 0;
        config.retry.max_deferred_rounds = 1;
        for endpoint in &mut config.endpoints {
            endpoint.protocol = EndpointProtocolMode::Auto;
            endpoint.sticky_group = Some(endpoint.id.clone());
            endpoint.mappings[0].client_pattern = "gpt-6-astra".into();
        }
        config.model_groups = serde_json::from_value(json!([{
            "id":"default", "models":["gpt-6-astra","claude-fable-5-1"], "bindings":[
                {"endpointID":"a","models":selection},
                {"endpointID":"b","models":null}
            ]
        }]))
        .unwrap();
        let engine = engine_with_dir(config.clone(), dir.clone(), fake.clone());
        let session = stable_session("model-scope");
        fake.push("a.example.com", sse_ok(&["data: {}\n\n"]));
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/responses",
                session_header(&session),
                Bytes::from(
                    serde_json::to_vec(
                        &json!({"model":"gpt-6-astra","input":"hello","stream":true})
                    )
                    .unwrap()
                )
            )
            .await
            .0,
            200
        );
        assert_eq!(fake.requests()[0].host, "a.example.com");

        // Retain both the binding and old session affinity while removing
        // astra from a. New requests must use only b after config reload.
        config.endpoints[0].mappings[0].client_pattern = "claude-fable-5-1".into();
        let _ = dir.save_config(&config).unwrap();
        engine.reload_config().unwrap();
        fake.push(
            "b.example.com",
            Outcome::Status {
                status: 503,
                headers: vec![],
                chunks: vec![],
            },
        );
        let status = call(
            &engine,
            loopback(),
            "/v1/responses",
            session_header(&session),
            Bytes::from(
                serde_json::to_vec(&json!({"model":"gpt-6-astra","input":"hello","stream":true}))
                    .unwrap(),
            ),
        )
        .await
        .0;
        assert_eq!(status, 503);
        assert_eq!(
            fake.requests()
                .iter()
                .map(|r| r.host.as_str())
                .collect::<Vec<_>>(),
            ["a.example.com", "b.example.com"]
        );
        drop(engine);
        let _ = std::fs::remove_dir_all(dir.root);
    }
}
