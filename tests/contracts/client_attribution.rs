//! Same ingestion and lifecycle contract on Linux and macOS.
use super::*;
use sumpter_core::client_attribution::ObservedRequestKind;

fn config() -> AppConfig {
    let mut c = two_endpoint_config();
    c.endpoints.truncate(1);
    c.endpoints[0].protocol = EndpointProtocolMode::Auto;
    c.endpoints[0].mappings[0].thinking = ThinkingMode::Passthrough;
    c.retry.max_500_retries = 1;
    c
}

fn attribution_headers(client: &str) -> Vec<(String, String)> {
    vec![
        ("x-sumpter-client".into(), client.into()),
        ("x-sumpter-request-kind".into(), "memory".into()),
        ("x-sumpter-agent-role".into(), "memory".into()),
        ("x-sumpter-client-variant".into(), "sdk".into()),
        (
            "x-sumpter-workspace".into(),
            "/srv/customer-a/apps/api".into(),
        ),
        ("x-sumpter-source-id".into(), "synthetic-host-a".into()),
        ("x-sumpter-collector-version".into(), "2.0.0-test".into()),
    ]
}

#[tokio::test]
async fn runtime_dimensions_keep_stable_identity_after_endpoint_rename() {
    use sumpter_engine::runtime_query::{DimensionKind, DimensionPageQuery, RuntimeFilter};

    let dir = temp_config_dir("dimension-rename-contract");
    let fake = FakeTransport::new();
    let mut current = config();
    current.endpoints[0].name = "Z old".into();
    let engine = engine_with_dir(current.clone(), dir.clone(), fake.clone());
    for name in ["Z old", "A new"] {
        current.endpoints[0].name = name.into();
        let _ = engine.replace_config(current.clone().normalized());
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let mut headers = attribution_headers("codex");
        headers.extend(session_header("dimension-session"));
        let (status, _) = call(&engine, loopback(), "/v1/messages", headers, body()).await;
        assert_eq!(status, 200);
    }
    engine.flush_stats().unwrap();
    let query = DimensionPageQuery {
        filter: RuntimeFilter {
            request_kind: Some("memory".into()),
            client_variant: Some("sdk".into()),
            agent_role: Some("memory".into()),
            from: Some(0.0),
            ..Default::default()
        },
        ..Default::default()
    };
    for kind in [
        DimensionKind::Endpoint,
        DimensionKind::Project,
        DimensionKind::Session,
    ] {
        let page = engine.runtime_dimension_page(kind, &query).unwrap();
        assert_eq!(page["totalCount"], 1, "{kind:?}");
        assert_eq!(page["rows"][0]["requests"], 2);
        if kind == DimensionKind::Endpoint {
            assert_eq!(page["rows"][0]["name"], "A new");
        }
    }
    drop(engine);
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn runtime_cache_sort_keys_round_trip() {
    use sumpter_engine::runtime_query::{
        DimensionKind, DimensionPageQuery, DimensionSort, SortOrder,
    };

    let dir = temp_config_dir("cache-sort-contract");
    let engine = engine_with_dir(config(), dir.clone(), FakeTransport::new());
    for (sort, key) in [
        (DimensionSort::CacheReadTokens, "cache_read"),
        (DimensionSort::CacheWriteTokens, "cache_write"),
    ] {
        for (order, direction) in [(SortOrder::Asc, "asc"), (SortOrder::Desc, "desc")] {
            let query = DimensionPageQuery {
                sort,
                order,
                ..Default::default()
            };
            let page = engine
                .runtime_dimension_page(DimensionKind::Model, &query)
                .unwrap();
            assert_eq!(page["sort"], key);
            assert_eq!(page["order"], direction);
        }
    }
    drop(engine);
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn observational_identity_does_not_change_protocol_conversion() {
    let mut outputs = Vec::new();
    for declared in [false, true] {
        let fake = FakeTransport::new();
        fake.push("a.example.com", Outcome::Status {
            status: 200,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![serde_json::to_vec(&json!({"id":"synthetic","type":"message","role":"assistant","model":"claude-opus-5","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}})).unwrap()],
        });
        let mut c = config();
        c.endpoints[0].protocol = EndpointProtocolMode::Anthropic;
        let engine = engine_with(c, fake.clone());
        let request = Bytes::from(json!({"model":"claude-opus-5","stream":false,"input":[{"role":"user","content":"hello"}]}).to_string());
        let h = if declared {
            attribution_headers("cursor")
        } else {
            vec![]
        };
        let (status, _) = call(&engine, loopback(), "/v1/responses", h, request).await;
        assert_eq!(status, 200);
        outputs.push(fake.requests()[0].body.clone());
        if declared {
            let snapshot = engine.runtime_snapshot();
            for event in snapshot
                .recent_events
                .iter()
                .filter(|e| e.kind == "client" || e.kind == "upstream")
            {
                assert_eq!(event.request_kind, Some(ObservedRequestKind::Memory));
                assert_eq!(event.agent_role.as_deref(), Some("memory"));
            }
        }
    }
    assert_eq!(outputs[0], outputs[1]);
}

#[tokio::test]
async fn all_declared_clients_preserve_native_body_and_strip_private_attribution_headers() {
    for kind in sumpter_core::client_attribution::CLIENTS {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let engine = engine_with(config(), fake.clone());
        let body = body();
        let (status, _) = call(
            &engine,
            loopback(),
            "/v1/messages",
            attribution_headers(kind.as_str()),
            body.clone(),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(fake.requests().len(), 1);
        assert_eq!(fake.requests()[0].body, body.as_ref());
        assert!(
            !fake.requests()[0]
                .headers
                .iter()
                .any(|(n, _)| n.to_ascii_lowercase().starts_with("x-sumpter-"))
        );
        let snapshot = engine.runtime_snapshot();
        assert_eq!(snapshot.client_requests, 1);
        for event in snapshot
            .recent_events
            .iter()
            .filter(|e| e.kind == "client" || e.kind == "upstream")
        {
            assert_eq!(event.client_kind, Some(*kind));
            assert_eq!(event.request_kind, Some(ObservedRequestKind::Memory));
            assert_eq!(event.client_variant.as_deref(), Some("sdk"));
            assert_eq!(event.agent_role.as_deref(), Some("memory"));
            assert_eq!(event.client_attribution.as_ref().unwrap().version, 2);
            let declared = event.client_declared.as_ref().unwrap();
            assert_eq!(declared.source_id.as_deref(), Some("synthetic-host-a"));
            assert_eq!(declared.collector_version.as_deref(), Some("2.0.0-test"));
            let identity = declared.workspace_identity.as_ref().unwrap();
            assert_eq!(identity.version, 2);
            assert_eq!(identity.name, "api");
            assert!(identity.scoped);
        }
    }
}

#[tokio::test]
async fn stream_snapshot_completion_cancellation_and_retry_keep_attribution() {
    for cancel in [false, true] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 500,
                headers: vec![],
                chunks: vec![b"temporary".to_vec()],
            },
        );
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let engine = engine_with(config(), fake.clone());
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/messages",
                attribution_headers("cline"),
                Body::from(body()),
            )
            .await;
        let before = engine.runtime_snapshot();
        let pending = before
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap();
        assert_eq!(pending.request_kind, Some(ObservedRequestKind::Memory));
        let evidence = pending.client_attribution.clone();
        if cancel {
            drop(response);
        } else {
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
        }
        let after = engine.runtime_snapshot();
        assert_eq!(after.client_requests, 1);
        assert_eq!(fake.requests().len(), 2);
        for event in after
            .recent_events
            .iter()
            .filter(|e| e.kind == "client" || e.kind == "upstream")
        {
            assert_eq!(event.request_kind, Some(ObservedRequestKind::Memory));
            assert_eq!(event.client_attribution, evidence);
        }
    }
}

#[tokio::test]
async fn rejection_and_native_body_metadata_are_observed_before_normalization() {
    let fake = FakeTransport::new();
    let engine = engine_with(config(), fake.clone());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        attribution_headers("workbuddy"),
        Bytes::from_static(b"{invalid"),
    )
    .await;
    assert_eq!(status, 400);
    assert!(fake.requests().is_empty());
    let snapshot = engine.runtime_snapshot();
    assert_eq!(
        snapshot.recent_events[0].client_kind,
        Some(sumpter_core::events::ClientKind::WorkBuddy)
    );
    assert_eq!(
        snapshot.recent_events[0].request_kind,
        Some(ObservedRequestKind::Memory)
    );

    fake.push(
        "a.example.com",
        sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
    );
    let request = json!({"model":"claude-opus-5","stream":true,"messages":[{"role":"user","content":"hello"}],"dsh_session_log":{"version":1,"session":{"origin":"subagent"},"events":[]}}).to_string();
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![
            ("user-agent".into(), "deepseek-harness/1".into()),
            ("x-deepseek-harness-compact".into(), "1".into()),
        ],
        Bytes::from(request.clone()),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(fake.requests()[0].body, request.as_bytes());
    let snapshot = engine.runtime_snapshot();
    let event = snapshot
        .recent_events
        .iter()
        .find(|e| e.client_kind == Some(sumpter_core::events::ClientKind::DeepSeekHarness))
        .unwrap();
    assert_eq!(event.request_kind, Some(ObservedRequestKind::Compaction));
    assert_eq!(event.agent_role.as_deref(), Some("subagent"));
}

#[tokio::test]
async fn token_binding_and_relay_forwarding_preserve_body_and_native_session() {
    use sumpter_core::client_identity::ClientBinding;
    for relay in [false, true] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let mut c = config();
        c.listener.auth_token = "synthetic-bound-key".into();
        c.listener.auth_token_enabled = true;
        c.listener.client_identity.token_bindings.insert(
            "legacy".into(),
            ClientBinding {
                client: "codex".into(),
                variant: Some("sdk".into()),
                relay_source: Some("office".into()),
            },
        );
        if relay {
            c.listener
                .client_identity
                .relay_endpoints
                .insert(c.endpoints[0].id.clone(), "home".into());
        }
        let engine = engine_with(c, fake.clone());
        let original = body();
        let (status, _) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![
                ("authorization".into(), "Bearer synthetic-bound-key".into()),
                ("user-agent".into(), "openai/1".into()),
                ("session_id".into(), "native-session".into()),
                ("x-sumpter-relay-source".into(), "office".into()),
                ("x-sumpter-forwarded-client".into(), "codex".into()),
                (
                    "x-sumpter-forwarded-session".into(),
                    "remote-session".into(),
                ),
            ],
            original.clone(),
        )
        .await;
        assert_eq!(status, 200);
        let captured = fake.requests();
        assert_eq!(captured[0].body, original.as_ref());
        let private: Vec<_> = captured[0]
            .headers
            .iter()
            .filter(|(n, _)| n.to_ascii_lowercase().starts_with("x-sumpter-"))
            .collect();
        if relay {
            assert_eq!(private.len(), 3);
            for (name, value) in [
                ("x-sumpter-relay-source", "home"),
                ("x-sumpter-forwarded-client", "codex"),
                ("x-sumpter-forwarded-session", "native-session"),
            ] {
                assert!(private.iter().any(|(n, v)| n == name && v == value));
            }
        } else {
            assert!(private.is_empty());
        }
        for event in engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .filter(|e| e.kind == "client" || e.kind == "upstream")
        {
            assert_eq!(event.client_kind, Some(ClientKind::Codex));
            assert_eq!(event.session_id.as_deref(), Some("native-session"));
            assert_eq!(event.inbound_token.as_ref().unwrap().id, "legacy");
        }
    }
}

#[tokio::test]
async fn native_session_headers_work_without_a_collector() {
    for (ua, name) in [
        ("opencode/1", "x-opencode-session"),
        ("workbuddy/1", "x-conversation-id"),
        ("deepseek-harness/1", "x-deepseek-harness-session-id"),
    ] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let engine = engine_with(config(), fake);
        let (status, _) = call(
            &engine,
            loopback(),
            "/v1/messages",
            vec![
                ("user-agent".into(), ua.into()),
                (name.into(), "native-only".into()),
            ],
            body(),
        )
        .await;
        assert_eq!(status, 200);
        for event in engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .filter(|e| e.kind == "client" || e.kind == "upstream")
        {
            assert_eq!(event.session_id.as_deref(), Some("native-only"));
        }
    }
}
