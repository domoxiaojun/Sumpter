use serde_json::{Value, json};
use sumpter_core::config::{AppConfig, ProviderProtocol};
use sumpter_core::routing::{RoutePlanner, RoutingRequest};

fn config() -> AppConfig {
    serde_json::from_value(json!({
        "schemaVersion": 7,
        "endpoints": (["a", "b", "c"].map(|id| json!({
            "id": id, "name": id, "baseURL": format!("https://{id}.invalid"), "protocol": "auto",
            "mappings": [{"clientPattern": "*", "thinking":"passthrough", "effort":"high", "failoverTimeoutSeconds": 17}]
        }))),
        "modelGroups": [
          {"id":"main", "name":"主用", "priority":1, "models":["gpt-x","claude-x"], "bindings":[
            {"endpointID":"b", "priority":2, "models":["gpt-x"]},
            {"endpointID":"a", "priority":1, "models":null}]},
          {"id":"backup", "name":"备用", "priority":2, "models":["gpt-x","claude-x","gemini-x","grok-x"],
            "bindings":[{"endpointID":"c", "priority":0, "models":null}]}
        ]
    })).unwrap()
}

fn route(config: &AppConfig, model: &str) -> Vec<sumpter_core::routing::PlannedEndpoint> {
    let request = RoutingRequest::from_value(&json!({"model":model})).unwrap();
    RoutePlanner::plan_for_passthrough(&request, config, ProviderProtocol::OpenAIResponses)
        .unwrap()
        .endpoints
}

#[test]
fn group_priority_then_binding_priority_and_model_scope() {
    let c = config();
    c.validate_model_groups().unwrap();
    let ids = |model| {
        route(&c, model)
            .into_iter()
            .map(|e| e.endpoint_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("gpt-x"), ["a", "b", "c"]);
    assert_eq!(ids("claude-x"), ["a", "c"]);
    assert_eq!(ids("gemini-x"), ["c"]);
    assert_eq!(ids("grok-x"), ["c"]);
    let first = &route(&c, "gpt-x")[0];
    assert_eq!(first.model_group_id.as_deref(), Some("main"));
    assert_eq!(first.failover_timeout_seconds, Some(17.0));
    assert_eq!(
        first.thinking,
        sumpter_core::config::ThinkingMode::Passthrough
    );
    assert_eq!(
        first.effort_override,
        Some(sumpter_core::ReasoningEffort::High)
    );
}

#[test]
fn ties_preserve_group_order_before_binding_priority() {
    let mut c = config();
    c.model_groups.as_mut().unwrap()[1].priority = 1;
    assert_eq!(
        route(&c, "gpt-x")
            .iter()
            .map(|e| e.endpoint_id.as_str())
            .collect::<Vec<_>>(),
        ["a", "b", "c"]
    );
    c.model_groups.as_mut().unwrap().swap(0, 1);
    assert_eq!(route(&c, "gpt-x")[0].endpoint_id, "c");
}

#[test]
fn explicit_empty_or_disabled_groups_never_reopen_library() {
    let mut c = config();
    for group in c.model_groups.as_mut().unwrap() {
        group.enabled = false;
    }
    assert!(!c.matches_model("gpt-x"));
    c.model_groups = Some(vec![]);
    assert!(!c.matches_model("gpt-x"));
    assert!(c.routing_endpoints().is_empty());
}

#[test]
fn all_tracks_added_models_but_selected_does_not() {
    let mut c = config();
    c.model_groups.as_mut().unwrap()[0]
        .models
        .push("new-model".into());
    assert_eq!(
        route(&c, "new-model")
            .iter()
            .map(|e| e.endpoint_id.as_str())
            .collect::<Vec<_>>(),
        ["a"]
    );
    c.endpoints[0].catalog = serde_json::from_value(json!({"models":["catalog-only"]})).ok();
    assert!(!c.matches_model("catalog-only"));
}

#[test]
fn per_model_overrides_are_local_to_binding_and_keep_parameters() {
    let mut c = config();
    c.model_groups.as_mut().unwrap()[0].bindings[0].overrides = serde_json::from_value(json!([
        {"model":"gpt-x","upstreamModel":"private-gpt","priority":0}]))
    .unwrap();
    let endpoints = route(&c, "gpt-x");
    assert_eq!(endpoints[0].endpoint_id, "b");
    assert_eq!(endpoints[0].upstream_model, "private-gpt");
    assert_eq!(endpoints[0].failover_timeout_seconds, Some(17.0));
    assert_eq!(c.endpoints[1].mappings[0].upstream_model, "");
}

#[test]
fn duplicate_calls_merge_candidates_but_different_mappings_do_not() {
    let mut c = config();
    let group = &mut c.model_groups.as_mut().unwrap()[1];
    group.bindings[0].endpoint_id = "a".into();
    assert_eq!(route(&c, "gpt-x").len(), 2);
    group_rewrite(
        &mut c,
        json!([{"model":"gpt-x","upstreamModel":"other-gpt"}]),
    );
    assert_eq!(route(&c, "gpt-x").len(), 3);
}

fn group_rewrite(c: &mut AppConfig, overrides: Value) {
    c.model_groups.as_mut().unwrap()[1].bindings[0].overrides =
        serde_json::from_value(overrides).unwrap();
}

#[test]
fn missing_references_and_out_of_group_models_are_rejected() {
    let mut c = config();
    c.model_groups.as_mut().unwrap()[0].bindings[0].endpoint_id = "missing".into();
    assert!(c.validate_model_groups().is_err());
    let mut c = config();
    c.model_groups.as_mut().unwrap()[0].bindings[0].models = Some(vec!["outside".into()]);
    assert!(c.validate_model_groups().is_err());
}

#[test]
fn explicit_null_groups_do_not_reopen_legacy_endpoint_library() {
    let value = json!({
        "schemaVersion": 7,
        "endpoints": [],
        "modelGroups": null,
    });
    let error = sumpter_core::config_store::validate_config_wire(&value).unwrap_err();
    assert!(error.contains("modelGroups 必须是数组"));
}

#[test]
fn projected_exact_mapping_keeps_precedence_over_source_wildcard() {
    let mut c = config();
    c.endpoints[0].mappings = vec![
        serde_json::from_value(json!({
            "clientPattern": "*",
            "thinking": "disabled",
            "effort": "low",
            "failoverTimeoutSeconds": 3,
            "upstreamModel": "broad-model"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "clientPattern": "gpt-*", "effort": "medium", "upstreamModel": "prefix-model"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "clientPattern": "gpt-*", "capabilities": ["video"], "upstreamModel": "video-model"
        }))
        .unwrap(),
        serde_json::from_value(json!({
            "clientPattern": "gpt-x",
            "thinking": "passthrough",
            "effort": "high",
            "failoverTimeoutSeconds": 19,
            "upstreamModel": "exact-model",
            "capabilities": ["text"]
        }))
        .unwrap(),
    ];
    let projected = c
        .routing_endpoints()
        .into_iter()
        .find(|entry| entry.endpoint.id == "a")
        .unwrap();
    let mapping = projected.endpoint.mapping_for("gpt-x").unwrap();
    assert_eq!(mapping.upstream_model, "exact-model");
    assert_eq!(mapping.failover_timeout_seconds, Some(19.0));
    assert_eq!(mapping.effort, Some(sumpter_core::ReasoningEffort::High));
    assert_eq!(
        mapping.capabilities,
        vec![sumpter_core::ModelCapability::Text]
    );
    assert_eq!(
        projected
            .endpoint
            .mapping_for_capability("gpt-x", sumpter_core::ModelCapability::Video)
            .unwrap()
            .upstream_model,
        "video-model"
    );
    c.endpoints[0].mappings.pop();
    assert_eq!(route(&c, "gpt-x")[0].upstream_model, "prefix-model");
    c.model_groups.as_mut().unwrap()[0].bindings[1].overrides = serde_json::from_value(json!([
        {"model":"gpt-x", "upstreamModel":"overridden"}
    ]))
    .unwrap();
    assert_eq!(
        route(&c, "gpt-x")[0].effort_override,
        Some(sumpter_core::ReasoningEffort::Medium)
    );
}

#[test]
fn group_scope_validation_rejects_empty_and_malformed_selections() {
    let mut c = config();
    c.model_groups.as_mut().unwrap()[0].models = vec!["*".into()];
    for invalid in [" ", "(high)", "gpt*x", "gpt**"] {
        c.model_groups.as_mut().unwrap()[0].bindings[0].models = Some(vec![invalid.into()]);
        assert!(c.validate_model_groups().is_err(), "{invalid}");
    }
    c.model_groups.as_mut().unwrap()[0].bindings[0].models = None;
    c.model_groups.as_mut().unwrap()[0].bindings[0].overrides = serde_json::from_value(json!([
        {"model":" ", "priority":0}
    ]))
    .unwrap();
    assert!(c.validate_model_groups().is_err());
}

#[test]
fn migration_preserves_wildcards_parameters_retry_and_sticky_ids() {
    let mut c = config();
    c.model_groups = None;
    c.endpoints[0].sticky_group = Some("existing-session".into());
    let retry = c.retry.clone();
    let rules = c.feature_rules.clone();
    let before = route(&c, "not-in-catalog");
    c.migrate_model_groups();
    c.validate_model_groups().unwrap();
    let after = route(&c, "not-in-catalog");
    for (left, right) in before.iter().zip(&after) {
        assert_eq!(left.endpoint_id, right.endpoint_id);
        assert_eq!(left.sticky_group, right.sticky_group);
        assert_eq!(left.upstream_model, right.upstream_model);
        assert_eq!(left.effort_override, right.effort_override);
    }
    assert_eq!(before.len(), after.len());
    assert_eq!(retry, c.retry);
    assert_eq!(rules, c.feature_rules);
}

fn default_group_config() -> AppConfig {
    let mut c = config();
    c.endpoints[0].priority = 5;
    c.endpoints[1].priority = 3;
    c.endpoints[2].priority = 0;
    c.model_groups = Some(vec![
        serde_json::from_value(json!({
            "id":"default", "name":"默认模型组", "models":["gpt-x","claude-x"],
            "bindings":[
                {"endpointID":"a", "priority":5},
                {"endpointID":"b", "priority":3},
                {"endpointID":"c", "priority":0}]}
        ))
        .unwrap(),
    ]);
    c
}

fn default_bindings(c: &AppConfig) -> Vec<(String, i64)> {
    c.model_groups
        .as_ref()
        .unwrap()
        .iter()
        .find(|group| group.id == "default")
        .unwrap()
        .bindings
        .iter()
        .map(|binding| (binding.endpoint_id.clone(), binding.priority))
        .collect()
}

#[test]
fn default_group_bindings_follow_endpoint_library_order_and_priority() {
    let mut c = default_group_config().normalized();
    // 入口库把 b 移到顺序 1 并同步改优先级:默认组必须跟随,
    // 否则入口库的排序编辑对路由无效(外部反馈的根因)。
    let b = c.endpoints.remove(1);
    c.endpoints.insert(0, b);
    c.endpoints[0].priority = 1;
    c.endpoints[1].priority = 5;
    c.endpoints[2].priority = 9;
    let c = c.normalized();
    assert_eq!(
        default_bindings(&c),
        vec![
            ("b".to_string(), 1),
            ("a".to_string(), 5),
            ("c".to_string(), 9),
        ]
    );
    assert_eq!(
        route(&c, "gpt-x")
            .iter()
            .map(|e| e.endpoint_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "a", "c"]
    );
}

#[test]
fn default_group_sync_keeps_exclusions_and_drops_dangling_bindings() {
    let mut c = default_group_config();
    let group = &mut c.model_groups.as_mut().unwrap()[0];
    // 刻意把 b 排除出默认组;同时留下引用已删除入口的悬空绑定。
    group.bindings.retain(|binding| binding.endpoint_id != "b");
    group
        .bindings
        .push(serde_json::from_value(json!({"endpointID": "deleted", "priority": 1})).unwrap());
    let c = c.normalized();
    let bindings = default_bindings(&c);
    // 排除的入口不会被补回,悬空引用被剔除。
    assert_eq!(bindings, vec![("a".to_string(), 5), ("c".to_string(), 0)]);
}

#[test]
fn non_default_groups_keep_their_own_binding_order() {
    let mut c = default_group_config();
    c.model_groups.as_mut().unwrap().push(
        serde_json::from_value(json!({
            "id":"custom", "name":"自定义组", "models":["gpt-x"],
            "bindings":[
                {"endpointID":"c", "priority":7},
                {"endpointID":"a", "priority":9}]}
        ))
        .unwrap(),
    );
    let a = c.endpoints.remove(0);
    c.endpoints.insert(2, a);
    let c = c.normalized();
    let custom = &c.model_groups.as_ref().unwrap()[1];
    assert_eq!(
        custom
            .bindings
            .iter()
            .map(|b| (b.endpoint_id.as_str(), b.priority))
            .collect::<Vec<_>>(),
        [("c", 7), ("a", 9)]
    );
}

#[test]
fn session_sticky_ttl_hours_default_clamp_and_conversion() {
    // 缺省键 → 72;负值与非有限值收敛为 0(永不过期)。
    let absent: AppConfig = serde_json::from_value(json!({
        "schemaVersion": 7,
        "endpoints": []
    }))
    .unwrap();
    assert_eq!(absent.session_sticky_ttl_hours, 72.0);
    assert_eq!(absent.session_sticky_ttl_secs(), 72.0 * 3600.0);
    let wire = serde_json::to_value(&absent).unwrap();
    assert_eq!(wire["sessionStickyTtlHours"], json!(72));

    let clamped: AppConfig = serde_json::from_value(json!({
        "schemaVersion": 7,
        "endpoints": [],
        "sessionStickyTtlHours": -5
    }))
    .unwrap();
    let clamped = clamped.normalized();
    assert_eq!(clamped.session_sticky_ttl_hours, 0.0);
    assert_eq!(clamped.session_sticky_ttl_secs(), 0.0);

    let fractional: AppConfig = serde_json::from_value(json!({
        "schemaVersion": 7,
        "endpoints": [],
        "sessionStickyTtlHours": 1.5
    }))
    .unwrap();
    assert_eq!(fractional.session_sticky_ttl_secs(), 5400.0);
}
