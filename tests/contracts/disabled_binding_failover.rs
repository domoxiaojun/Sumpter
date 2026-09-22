//! 组内停用的绑定不能进入故障转移，包括与启用入口共用 stickyGroup 的情况。
use super::*;

fn grouped(mut config: AppConfig, groups: Value) -> AppConfig {
    config.schema_version = 7;
    config.retry.max_500_retries = 0;
    config.retry.max_deferred_rounds = 1;
    config.retry.session_sticky_retries = 0;
    config.retry.failover_on_500 = true;
    config.model_groups = serde_json::from_value(groups).unwrap();
    config
}

fn one_group(b_enabled: bool) -> Value {
    json!([{
        "id": "main",
        "name": "主用",
        "priority": 0,
        "models": ["claude-*"],
        "bindings": [
            {"endpointID": "a", "enabled": true, "priority": 0, "models": null},
            {"endpointID": "b", "enabled": b_enabled, "priority": 1, "models": null}
        ]
    }])
}

fn status_only(status: u16) -> Outcome {
    Outcome::Status {
        status,
        headers: vec![],
        chunks: vec![],
    }
}

async fn hosts_after(config: AppConfig, status: u16) -> Vec<String> {
    let fake = FakeTransport::new();
    fake.push("a.example.com", status_only(status));
    let engine = engine_with(config, fake.clone());
    let (response_status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(response_status, status);
    fake.requests()
        .into_iter()
        .map(|request| request.host)
        .collect()
}

#[tokio::test]
async fn disabled_binding_is_skipped_on_503_and_500_failover() {
    for sticky in [false, true] {
        let mut config = grouped(two_endpoint_config(), one_group(false));
        if sticky {
            config.endpoints[0].sticky_group = Some("team".into());
            config.endpoints[1].sticky_group = Some("team".into());
        } else {
            config.endpoints[0].sticky_group = Some("a-only".into());
            config.endpoints[1].sticky_group = Some("b-only".into());
        }
        assert_eq!(
            hosts_after(config.clone(), 503).await,
            ["a.example.com"],
            "sticky={sticky}"
        );
        assert_eq!(
            hosts_after(config, 500).await,
            ["a.example.com"],
            "sticky={sticky}"
        );
    }
}

#[tokio::test]
async fn library_disabled_endpoint_is_skipped_on_failover() {
    let mut config = grouped(two_endpoint_config(), one_group(true));
    config.endpoints[1].enabled = false;
    assert_eq!(hosts_after(config, 503).await, ["a.example.com"]);
}

#[tokio::test]
async fn pinned_rule_does_not_call_group_disabled_binding() {
    let mut config = grouped(two_endpoint_config(), one_group(false));
    config.feature_rules = vec![
        serde_json::from_value(json!({
            "id": "pin-b",
            "enabled": true,
            "name": "pin-b",
            "match": {"modelEquals": "claude-opus-5"},
            "target": {"endpointID": "b", "model": "claude-opus-5"}
        }))
        .unwrap(),
    ];
    assert_eq!(hosts_after(config, 503).await, ["a.example.com"]);

    let mut sealed = grouped(
        two_endpoint_config(),
        json!([{
            "id": "main",
            "models": ["claude-*"],
            "bindings": [
                {"endpointID": "a", "enabled": false},
                {"endpointID": "b", "enabled": false}
            ]
        }]),
    );
    sealed.feature_rules = vec![
        serde_json::from_value(json!({
            "id": "pin-b",
            "enabled": true,
            "name": "pin-b",
            "match": {"modelEquals": "claude-opus-5"},
            "target": {"endpointID": "b", "model": "claude-opus-5"}
        }))
        .unwrap(),
    ];
    let fake = FakeTransport::new();
    let engine = engine_with(sealed, fake.clone());
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 400);
    assert!(fake.requests().is_empty());
}

#[tokio::test]
async fn failover_uses_endpoint_only_through_its_enabled_group() {
    let config = grouped(
        two_endpoint_config(),
        json!([
            {
                "id": "main",
                "name": "主用",
                "priority": 0,
                "models": ["claude-*"],
                "bindings": [
                    {"endpointID": "a", "enabled": true, "models": null},
                    {"endpointID": "b", "enabled": false, "models": null}
                ]
            },
            {
                "id": "backup",
                "name": "备用",
                "priority": 1,
                "models": ["claude-*"],
                "bindings": [
                    {"endpointID": "b", "enabled": true, "models": null}
                ]
            }
        ]),
    );
    let fake = FakeTransport::new();
    fake.push("a.example.com", status_only(503));
    fake.push("b.example.com", sse_ok(&["data: {}\n\n"]));
    let engine = engine_with(config, fake.clone());
    let (status, _) = call(&engine, loopback(), "/v1/messages", vec![], body()).await;
    assert_eq!(status, 200);
    assert_eq!(
        fake.requests()
            .iter()
            .map(|request| request.host.as_str())
            .collect::<Vec<_>>(),
        ["a.example.com", "b.example.com"]
    );
    let backup = engine
        .runtime_snapshot()
        .recent_events
        .into_iter()
        .find(|event| event.kind == "upstream" && event.endpoint_id.as_deref() == Some("b"))
        .expect("备用组应留下上游事件");
    assert_eq!(backup.model_group_id.as_deref(), Some("backup"));
    assert!(backup.failover);
}
