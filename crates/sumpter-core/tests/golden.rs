//! v6 配置 golden：Linux 与 macOS 共用扁平 Provider 候选序列。

use sumpter_core::config::*;

const EXAMPLE: &str = include_str!("../../../config.example.json");
const FULL: &str = include_str!("fixtures/full.json");

fn assert_value_roundtrip(source: &str) {
    let config = AppConfig::from_json(source).expect("decode");
    let reencoded = config.to_json_pretty().expect("encode");
    let original: serde_json::Value = serde_json::from_str(source).unwrap();
    let ours: serde_json::Value = serde_json::from_str(&reencoded).unwrap();
    assert_eq!(original, ours, "v6 配置 round-trip 发生字段漂移");
}

#[test]
fn example_config_decodes_as_flat_provider_candidates() {
    let config = AppConfig::from_json(EXAMPLE).expect("decode config.example.json");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert!(!config.endpoints.is_empty());
    assert!(config.endpoint("fallback").is_none());
    let first = &config.endpoints[0];
    assert!(!first.id.is_empty());
    assert!(!first.mappings.is_empty());
    assert_eq!(first.mappings[0].thinking, ThinkingMode::Adaptive);
    assert_eq!(first.mappings[0].context, ContextMode::OneMillion);
    assert_eq!(first.protocol, EndpointProtocolMode::Auto);
    let classifier = config
        .feature_rules
        .iter()
        .find(|rule| rule.id == "classifier")
        .unwrap();
    assert!(
        classifier
            .target
            .endpoint_id
            .as_deref()
            .is_none_or(|id| config.endpoint(id).is_some())
    );
    let normalized = config.clone().normalized();
    assert_eq!(normalized.endpoints.len(), config.endpoints.len());
}

#[test]
fn full_fixture_decodes_optional_fields_without_pool_layer() {
    let config = AppConfig::from_json(FULL).expect("decode full.json");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
    assert_eq!(config.endpoints.len(), 4);
    let first = &config.endpoints[0];
    assert!(first.pinned_ip_exclusive);
    assert_eq!(first.pinned_ips, vec!["1.2.3.4", "5.6.7.8"]);
    assert!(first.keep_alive);
    let qwen = config.endpoint("qwen").expect("qwen endpoint");
    assert!(!qwen.keep_alive);
    assert_eq!(qwen.catalog.as_ref().expect("catalog").models.len(), 2);
    assert_eq!(qwen.mappings[0].failover_timeout_seconds, Some(7.5));
    assert!(config.endpoint("oai").is_some());
    assert_eq!(
        config.endpoint("resp").unwrap().protocol,
        EndpointProtocolMode::OpenAIResponses
    );
    let classifier = &config.feature_rules[0];
    assert!(
        classifier
            .target
            .endpoint_id
            .as_deref()
            .is_some_and(|id| config.endpoint(id).is_some())
    );
    let normalized = config.normalized();
    assert_eq!(normalized.endpoints.len(), 4);
    assert_eq!(normalized.endpoints[1].id, "qwen");
    assert!(normalized.feature_rules.iter().all(|rule| {
        rule.target
            .endpoint_id
            .as_deref()
            .is_none_or(|id| normalized.endpoint(id).is_some())
    }));
}

#[test]
fn example_config_value_roundtrip() {
    assert_value_roundtrip(EXAMPLE);
}

#[test]
fn full_fixture_value_roundtrip() {
    assert_value_roundtrip(FULL);
}

#[test]
fn unknown_fields_are_tolerated() {
    let mut value: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    value["futureField"] = serde_json::json!({"nested": true});
    value["endpoints"][0]["futureFlag"] = serde_json::json!(1);
    let config = AppConfig::from_json(&value.to_string()).expect("宽容读未知字段");
    assert_eq!(config.schema_version, SCHEMA_VERSION);
}

#[test]
fn endpoint_protocol_mode_roundtrips_all_four_wire_values() {
    for (wire, expected) in [
        ("auto", EndpointProtocolMode::Auto),
        ("anthropic", EndpointProtocolMode::Anthropic),
        ("openai", EndpointProtocolMode::OpenAI),
        ("openai-responses", EndpointProtocolMode::OpenAIResponses),
    ] {
        let decoded: EndpointProtocolMode =
            serde_json::from_value(serde_json::json!(wire)).unwrap();
        assert_eq!(decoded, expected);
        assert_eq!(
            serde_json::to_value(decoded).unwrap(),
            serde_json::json!(wire)
        );
    }
}

#[test]
fn missing_endpoint_protocol_defaults_to_auto_and_is_saved_explicitly() {
    let mut value: serde_json::Value = serde_json::from_str(EXAMPLE).unwrap();
    value["endpoints"][0]
        .as_object_mut()
        .unwrap()
        .remove("protocol");
    let config = AppConfig::from_json(&value.to_string()).expect("decode missing protocol");
    assert_eq!(config.endpoints[0].protocol, EndpointProtocolMode::Auto);
    let saved: serde_json::Value = serde_json::from_str(&config.to_json_pretty().unwrap()).unwrap();
    assert_eq!(saved["endpoints"][0]["protocol"], "auto");
}

#[test]
fn invalid_endpoint_protocol_mode_is_rejected() {
    let error = serde_json::from_value::<EndpointProtocolMode>(serde_json::json!("codex"))
        .expect_err("invalid protocol must fail");
    assert!(error.to_string().contains("unknown variant"));
}
