//! 路由行为测试:对齐 Swift SumpterCoreTests 中路由/指纹/粘性相关用例的语义。
//! 请求 fixture 按 Claude Code 真实 wire 形状手造(指纹字符串必须逐字对齐)。

use serde_json::{Value, json};
use sumpter_core::capability::ModelCapability;
use sumpter_core::config::*;
use sumpter_core::routing::{
    RESOURCE_ROUTING_MODEL, RouteMode, RoutePlanError, RoutePlanner, RoutingRequest, inspector,
    sticky,
};
use sumpter_core::{ReasoningEffort, RequestPurpose};

fn request_from(value: Value) -> RoutingRequest {
    RoutingRequest::from_value(&value).expect("request object")
}

fn endpoint(id: &str, mappings: Vec<ModelMapping>) -> Endpoint {
    Endpoint {
        api_key: "sk-test".into(),
        base_url: format!("https://{id}.example.com"),
        catalog: None,
        enabled: true,
        id: id.into(),
        keep_alive: false,
        mappings,
        name: id.into(),
        priority: 0,
        protocol: EndpointProtocolMode::Anthropic,
        sticky_group: None,
    }
}

fn endpoint_with_priority(id: &str, priority: i64) -> Endpoint {
    // 新配置语义要求每个入口显式声明客户端模型；这些 fixture 都参与
    // claude-opus-5 路由，因此直接给出显式映射。
    let mut endpoint = endpoint(id, vec![mapping("claude-opus-*", "")]);
    endpoint.priority = priority;
    endpoint
}

fn mapping(pattern: &str, upstream: &str) -> ModelMapping {
    ModelMapping {
        client_pattern: pattern.into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Disabled,
        effort: None,
        upstream_model: upstream.into(),
        capabilities: Vec::new(),
    }
}

/// 扁平 Provider 候选序列（入口显式映射）的典型配置。
fn base_config() -> AppConfig {
    let mut qwen = endpoint(
        "qwen",
        vec![ModelMapping {
            failover_timeout_seconds: Some(15.0),
            ..mapping("claude-haiku-4-5-20251001", "qwen3.7-plus")
        }],
    );
    qwen.priority = 10;
    AppConfig {
        endpoints: vec![
            endpoint("main-a", vec![mapping("claude-opus-*", "")]),
            endpoint("main-b", vec![mapping("claude-opus-*", "")]),
            qwen,
        ],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized()
}

fn endpoint_mut<'a>(config: &'a mut AppConfig, id: &str) -> &'a mut Endpoint {
    config
        .endpoints
        .iter_mut()
        .find(|endpoint| endpoint.id == id)
        .unwrap_or_else(|| panic!("missing endpoint {id}"))
}

fn plain_request(model: &str) -> RoutingRequest {
    request_from(json!({
        "model": model,
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [{"role": "user", "content": "hello"}],
    }))
}

// ---------------------------------------------------------------------------
// 普通路由
// ---------------------------------------------------------------------------

#[test]
fn normalization_does_not_create_implicit_provider_rules() {
    let normalized = AppConfig::bootstrap().normalized();
    assert!(normalized.endpoints.is_empty());
    assert!(normalized.accepted_models().is_empty());
}

#[test]
fn exact_mapping_precedes_migrated_wildcard_mapping() {
    let endpoint = endpoint(
        "mixed",
        vec![
            mapping("claude-opus-*", "wildcard"),
            mapping("claude-opus-5", "exact"),
        ],
    );
    assert_eq!(
        endpoint
            .mapping_for("claude-opus-5[1m]")
            .unwrap()
            .upstream_model,
        "exact"
    );
    assert_eq!(
        endpoint
            .mapping_for("claude-opus-4-6")
            .unwrap()
            .upstream_model,
        "wildcard"
    );
}

#[test]
fn most_specific_wildcard_precedes_broad_mapping_even_when_declared_later() {
    let endpoint = endpoint(
        "mixed",
        vec![
            mapping("gpt-*", "broad"),
            mapping("gpt-image-*", "image"),
            mapping("gpt-image-2-*", "image-specific"),
        ],
    );
    assert_eq!(
        endpoint
            .mapping_for("gpt-image-2-preview")
            .unwrap()
            .upstream_model,
        "image-specific"
    );
    assert_eq!(
        endpoint.mapping_for("gpt-image-1").unwrap().upstream_model,
        "image"
    );
    assert_eq!(
        endpoint.mapping_for("gpt-5").unwrap().upstream_model,
        "broad"
    );
}

#[test]
fn capability_filtering_also_uses_wildcard_specificity_and_stable_ties() {
    let endpoint = endpoint(
        "mixed",
        vec![
            ModelMapping {
                capabilities: vec![ModelCapability::Text],
                ..mapping("gpt-*", "text")
            },
            ModelMapping {
                capabilities: vec![ModelCapability::Image],
                ..mapping("gpt-image-*", "image")
            },
            ModelMapping {
                capabilities: vec![ModelCapability::Image],
                ..mapping("gpt-image-*", "image-later")
            },
        ],
    );
    assert_eq!(
        endpoint
            .mapping_for_capability("gpt-image-2", ModelCapability::Image)
            .unwrap()
            .upstream_model,
        "image"
    );
    assert_eq!(
        endpoint.mapping_for_capability("gpt-5", ModelCapability::Image),
        None
    );
}

#[test]
fn primary_wildcard_routes_to_primary_with_global_rule_defaults() {
    let config = base_config();
    let plan = RoutePlanner::plan(&plain_request("claude-opus-5[1m]"), &config).unwrap();
    assert_eq!(plan.client_model, "claude-opus-5");
    assert_eq!(plan.effective_model, "claude-opus-5");
    assert_eq!(plan.feature_rule_id, None);
    assert_eq!(plan.endpoints.len(), 2);
    let ep = &plan.endpoints[0];
    assert_eq!(ep.upstream_model, "claude-opus-5"); // global 规则 = 同名
    assert_eq!(ep.thinking, ThinkingMode::Disabled);
    assert_eq!(ep.context, ContextMode::Standard);
    assert_eq!(ep.failover_timeout_seconds, None); // 主池恒 None
}

#[test]
fn planned_endpoints_preserve_priority_and_configuration_order() {
    let mut config = base_config();
    config.endpoints = vec![
        endpoint_with_priority("main-high", 10),
        endpoint_with_priority("main-first", 0),
        endpoint_with_priority("main-second", 0),
    ];
    let plan = RoutePlanner::plan(&plain_request("claude-opus-5"), &config.normalized()).unwrap();
    let ids = plan
        .endpoints
        .iter()
        .map(|endpoint| (endpoint.endpoint_id.as_str(), endpoint.priority))
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![("main-first", 0), ("main-second", 0), ("main-high", 10)]
    );
}

#[test]
fn negative_priority_normalizes_to_zero_and_zero_is_omitted() {
    let mut config = base_config();
    config.endpoints[0].priority = -7;
    let normalized = config.normalized();
    assert_eq!(normalized.endpoints[0].priority, 0);
    let value: Value = serde_json::from_str(&normalized.to_json_pretty().unwrap()).unwrap();
    assert!(value["endpoints"][0].get("priority").is_none());
}

#[test]
fn mapped_model_routes_to_unified_provider_and_unmapped_model_is_rejected() {
    let config = base_config();
    let plan = RoutePlanner::plan(&plain_request("claude-haiku-4-5-20251001"), &config).unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].upstream_model, "qwen3.7-plus");
    assert_eq!(plan.endpoints[0].thinking, ThinkingMode::Disabled);
    assert_eq!(plan.endpoints[0].failover_timeout_seconds, Some(15.0));

    let err = RoutePlanner::plan(&plain_request("gpt-5.4"), &config).unwrap_err();
    assert_eq!(err, RoutePlanError::NoProviderForModel("gpt-5.4".into()));
}

#[test]
fn resource_plan_does_not_require_text_model_mapping_or_anthropic_endpoint() {
    let mut openai = endpoint("openai", vec![]);
    openai.protocol = EndpointProtocolMode::OpenAI;
    let mut anthropic = endpoint("anthropic", vec![]);
    anthropic.protocol = EndpointProtocolMode::Anthropic;
    let config = AppConfig {
        endpoints: vec![anthropic, openai],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let plan = RoutePlanner::plan_for_resource(&config, ProviderProtocol::OpenAI).unwrap();
    assert_eq!(plan.client_model, RESOURCE_ROUTING_MODEL);
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].endpoint_id, "openai");
    assert_eq!(plan.endpoints[0].upstream_model, RESOURCE_ROUTING_MODEL);
}

#[test]
fn files_resource_plan_requires_explicit_files_capability() {
    let mut xiao = endpoint("xiao", vec![mapping("gpt-5.6-sol", "")]);
    xiao.protocol = EndpointProtocolMode::OpenAI;
    xiao.priority = 0;
    let mut cpa = endpoint("cpa", vec![mapping("gpt-live-1-codex", "")]);
    cpa.protocol = EndpointProtocolMode::OpenAI;
    cpa.priority = 10;
    let config = AppConfig {
        endpoints: vec![xiao, cpa],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let error = RoutePlanner::plan_for_resource_capability(
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Files,
    )
    .unwrap_err();
    assert_eq!(
        error,
        RoutePlanError::NoProviderForCapability {
            capability: "files".into()
        }
    );
}

#[test]
fn files_resource_plan_honors_explicit_files_capability() {
    let mut files_mapping = mapping("gpt-5.6-sol", "");
    files_mapping.capabilities = vec![ModelCapability::Files];
    let mut xiao = endpoint("xiao", vec![files_mapping]);
    xiao.protocol = EndpointProtocolMode::OpenAI;
    xiao.priority = 0;
    let mut cpa = endpoint("cpa", vec![mapping("gpt-live-1-codex", "")]);
    cpa.protocol = EndpointProtocolMode::OpenAI;
    cpa.priority = 10;
    let config = AppConfig {
        endpoints: vec![xiao, cpa],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let plan = RoutePlanner::plan_for_resource_capability(
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Files,
    )
    .unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].endpoint_id, "xiao");
}

#[test]
fn files_resource_plan_does_not_treat_live_or_chat_mapping_as_files() {
    let mut xiao = endpoint("xiao", vec![mapping("gpt-4o", "")]);
    xiao.protocol = EndpointProtocolMode::OpenAI;
    xiao.priority = 0;
    let mut cpa = endpoint("cpa", vec![mapping("gpt-live-1-codex", "")]);
    cpa.protocol = EndpointProtocolMode::OpenAI;
    cpa.priority = 10;
    let config = AppConfig {
        endpoints: vec![xiao, cpa],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let error = RoutePlanner::plan_for_resource_capability(
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Files,
    )
    .unwrap_err();
    assert_eq!(
        error,
        RoutePlanError::NoProviderForCapability {
            capability: "files".into()
        }
    );
}

#[test]
fn provider_mapping_allows_wildcard_for_every_endpoint() {
    let mut config = base_config();
    endpoint_mut(&mut config, "qwen")
        .mappings
        .push(mapping("claude-sonnet-*", "x"));
    let config = config.normalized();
    let plan = RoutePlanner::plan(&plain_request("claude-sonnet-5"), &config).unwrap();
    assert_eq!(plan.endpoints[0].endpoint_id, "qwen");
}

#[test]
fn unified_provider_uses_other_endpoint_when_primary_group_is_disabled() {
    let mut config = base_config();
    endpoint_mut(&mut config, "main-a").enabled = false;
    endpoint_mut(&mut config, "main-b").enabled = false;
    endpoint_mut(&mut config, "qwen")
        .mappings
        .push(mapping("claude-opus-5", "opus-alias"));
    let config = config.normalized();
    let plan = RoutePlanner::plan(&plain_request("claude-opus-5"), &config).unwrap();
    assert_eq!(plan.endpoints[0].upstream_model, "opus-alias");
}

#[test]
fn endpoint_with_own_mappings_only_serves_declared_models() {
    let mut config = base_config();
    // main-b 声明自带映射(gpt-5.4)后,不再承接池级 claude-opus-*。
    config.endpoints[1].mappings = vec![mapping("gpt-5.4", "")];
    let config = config.normalized();
    let plan = RoutePlanner::plan(&plain_request("claude-opus-5"), &config).unwrap();
    let ids: Vec<&str> = plan
        .endpoints
        .iter()
        .map(|e| e.endpoint_id.as_str())
        .collect();
    assert_eq!(ids, vec!["main-a"]);

    // gpt-5.4 在主池被 main-b 承接(入口自带映射,主池允许通配/精确)。
    let plan = RoutePlanner::plan(&plain_request("gpt-5.4"), &config).unwrap();
    let ids: Vec<&str> = plan
        .endpoints
        .iter()
        .map(|e| e.endpoint_id.as_str())
        .collect();
    assert_eq!(ids, vec!["main-b"]);
    assert_eq!(plan.endpoints[0].upstream_model, "gpt-5.4"); // 空 upstream = 同名
}

#[test]
fn no_providers_at_all_reports_no_provider_for_model() {
    let config = AppConfig {
        endpoints: vec![],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();
    let err = RoutePlanner::plan(&plain_request("m"), &config).unwrap_err();
    assert_eq!(err, RoutePlanError::NoProviderForModel("m".into()));
}

#[test]
fn auto_endpoint_resolves_each_source_format_as_native() {
    let mut config = base_config();
    config.endpoints.truncate(1);
    config.endpoints[0].protocol = EndpointProtocolMode::Auto;

    for source_format in [
        ProviderProtocol::Anthropic,
        ProviderProtocol::OpenAI,
        ProviderProtocol::OpenAIResponses,
    ] {
        let plan =
            RoutePlanner::plan_for_source(&plain_request("claude-opus-5"), &config, source_format)
                .unwrap();
        let endpoint = &plan.endpoints[0];
        assert_eq!(endpoint.configured_protocol, EndpointProtocolMode::Auto);
        assert_eq!(endpoint.source_format, source_format);
        assert_eq!(endpoint.protocol, source_format);
        assert_eq!(endpoint.route_mode, RouteMode::Native);
    }
}

#[test]
fn native_candidate_wins_over_higher_priority_translated_candidate() {
    let mut config = base_config();
    let mut translated = endpoint_with_priority("anthropic-priority-zero", 0);
    translated.protocol = EndpointProtocolMode::Anthropic;
    let mut native = endpoint_with_priority("auto-priority-ten", 10);
    native.protocol = EndpointProtocolMode::Auto;
    config.endpoints = vec![translated, native];

    let plan = RoutePlanner::plan_for_source(
        &plain_request("claude-opus-5"),
        &config,
        ProviderProtocol::OpenAI,
    )
    .unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].endpoint_id, "auto-priority-ten");
    assert_eq!(plan.endpoints[0].protocol, ProviderProtocol::OpenAI);
    assert_eq!(plan.endpoints[0].route_mode, RouteMode::Native);
}

#[test]
fn translated_candidates_are_used_only_when_no_native_candidate_exists() {
    let mut config = base_config();
    config.endpoints.truncate(1);
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAI;

    let plan = RoutePlanner::plan_for_source(
        &plain_request("claude-opus-5"),
        &config,
        ProviderProtocol::OpenAIResponses,
    )
    .unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].protocol, ProviderProtocol::OpenAI);
    assert_eq!(plan.endpoints[0].route_mode, RouteMode::Translated);
}

#[test]
fn feature_rule_target_protocol_accepts_auto_and_matching_fixed_only() {
    let mut config = base_config();
    let mut auto = endpoint("auto", vec![]);
    auto.mappings = vec![mapping("claude-opus-*", "")];
    auto.protocol = EndpointProtocolMode::Auto;
    let mut chat = endpoint("chat", vec![]);
    chat.mappings = vec![mapping("claude-opus-*", "")];
    chat.protocol = EndpointProtocolMode::OpenAI;
    let mut responses = endpoint("responses", vec![]);
    responses.mappings = vec![mapping("claude-opus-*", "")];
    responses.protocol = EndpointProtocolMode::OpenAIResponses;
    config.endpoints = vec![auto, chat, responses];
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "responses-target".into(),
        match_: FeatureRuleMatch {
            model_equals: Some("claude-opus-5".into()),
            ..FeatureRuleMatch::default()
        },
        name: "Responses target".into(),
        target: FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "claude-opus-5".into(),
            protocol_override: Some(ProviderProtocol::OpenAIResponses),
        },
    }];

    let plan = RoutePlanner::plan_for_source(
        &plain_request("claude-opus-5"),
        &config,
        ProviderProtocol::OpenAIResponses,
    )
    .unwrap();
    let ids = plan
        .endpoints
        .iter()
        .map(|endpoint| endpoint.endpoint_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["auto", "responses"]);
    assert!(
        plan.endpoints
            .iter()
            .all(|endpoint| endpoint.protocol == ProviderProtocol::OpenAIResponses)
    );
    assert!(
        plan.endpoints
            .iter()
            .all(|endpoint| endpoint.route_mode == RouteMode::Native)
    );
}

#[test]
fn feature_rule_target_protocol_can_explicitly_select_translation() {
    let mut config = base_config();
    let mut auto = endpoint("auto", vec![]);
    auto.mappings = vec![mapping("claude-opus-*", "")];
    auto.protocol = EndpointProtocolMode::Auto;
    let mut responses = endpoint("responses", vec![]);
    responses.mappings = vec![mapping("claude-opus-*", "")];
    responses.protocol = EndpointProtocolMode::OpenAIResponses;
    let mut chat = endpoint("chat", vec![]);
    chat.mappings = vec![mapping("claude-opus-*", "")];
    chat.protocol = EndpointProtocolMode::OpenAI;
    config.endpoints = vec![auto, responses, chat];
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "responses-target".into(),
        match_: FeatureRuleMatch {
            model_equals: Some("claude-opus-5".into()),
            ..FeatureRuleMatch::default()
        },
        name: "Responses target".into(),
        target: FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "claude-opus-5".into(),
            protocol_override: Some(ProviderProtocol::OpenAIResponses),
        },
    }];

    let plan = RoutePlanner::plan_for_source(
        &plain_request("claude-opus-5"),
        &config,
        ProviderProtocol::Anthropic,
    )
    .unwrap();
    let ids = plan
        .endpoints
        .iter()
        .map(|endpoint| endpoint.endpoint_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["auto", "responses"]);
    assert!(
        plan.endpoints
            .iter()
            .all(|endpoint| endpoint.protocol == ProviderProtocol::OpenAIResponses)
    );
    assert!(
        plan.endpoints
            .iter()
            .all(|endpoint| endpoint.route_mode == RouteMode::Translated)
    );
}

#[test]
fn feature_rule_target_protocol_rejects_nonmatching_fixed_endpoint() {
    let mut config = base_config();
    config.endpoints.truncate(1);
    config.endpoints[0].protocol = EndpointProtocolMode::OpenAI;
    config.feature_rules = vec![FeatureRule {
        enabled: true,
        id: "responses-target".into(),
        match_: FeatureRuleMatch {
            model_equals: Some("claude-opus-5".into()),
            ..FeatureRuleMatch::default()
        },
        name: "Responses target".into(),
        target: FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "claude-opus-5".into(),
            protocol_override: Some(ProviderProtocol::OpenAIResponses),
        },
    }];

    let err = RoutePlanner::plan_for_source(
        &plain_request("claude-opus-5"),
        &config,
        ProviderProtocol::OpenAIResponses,
    )
    .unwrap_err();
    assert_eq!(
        err,
        RoutePlanError::NoCompatibleProvider {
            source_format: "openai-responses".into(),
        }
    );
}

// ---------------------------------------------------------------------------
// 分流规则
// ---------------------------------------------------------------------------

fn classifier_request() -> RoutingRequest {
    request_from(json!({
        "model": "gpt-5.6-luna[1m]",
        "system": [{"type": "text", "text": "You are a security monitor for autonomous AI coding agents. Review actions."}],
        "messages": [{"role": "user", "content": "<transcript>\nBash: rm -rf /tmp/x\n</transcript>"}],
        "stop_sequences": ["</block>"],
    }))
}

fn websearch_request() -> RoutingRequest {
    request_from(json!({
        "model": "claude-opus-5",
        "system": "You are an assistant for performing a web search tool use for the user.",
        "messages": [{"role": "user", "content": "Perform a web search for the query: rust axum sse"}],
        "tools": [{"name": "web_search", "type": "web_search_20250305"}],
        "tool_choice": {"type": "tool", "name": "web_search"},
    }))
}

fn webfetch_request() -> RoutingRequest {
    request_from(json!({
        "model": "claude-opus-5",
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [{"role": "user", "content": "Web page content:\n---\nSome docs here\n---\n\nProvide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed."}],
    }))
}

fn config_with_enabled_rules() -> AppConfig {
    let mut config = base_config();
    endpoint_mut(&mut config, "qwen")
        .mappings
        .push(mapping("gpt-5.6-luna", "qwen3.7-plus"));
    config.feature_rules = builtin_rules::canonical()
        .into_iter()
        .map(|mut r| {
            r.enabled = true;
            r.target = FeatureRuleTarget {
                endpoint_id: None,
                effort: None,
                model: "gpt-5.6-luna".into(),
                protocol_override: None,
            };
            r
        })
        .collect();
    config.normalized()
}

#[test]
fn classifier_fingerprint_detected_and_routed() {
    let config = config_with_enabled_rules();
    let request = classifier_request();
    assert_eq!(
        inspector::detected_request_kind(&request),
        Some(RequestKind::Classifier)
    );
    assert_eq!(
        inspector::request_purpose(&request),
        RequestPurpose::Classifier
    );

    let plan = RoutePlanner::plan(&request, &config).unwrap();
    assert_eq!(plan.feature_rule_id, Some("classifier".into()));
    assert_eq!(plan.client_model, "gpt-5.6-luna");
    assert_eq!(plan.effective_model, "gpt-5.6-luna");
    assert_eq!(plan.endpoints[0].upstream_model, "qwen3.7-plus");
}

#[test]
fn feature_route_carries_effort_override_to_planned_endpoints() {
    let mut config = config_with_enabled_rules();
    let classifier = config
        .feature_rules
        .iter_mut()
        .find(|rule| rule.id == "classifier")
        .unwrap();
    classifier.target.effort = Some(ReasoningEffort::High);

    let plan = RoutePlanner::plan(&classifier_request(), &config).unwrap();
    assert_eq!(
        plan.endpoints[0].effort_override,
        Some(ReasoningEffort::High)
    );
}

#[test]
fn classifier_stage2_without_stop_sequences_still_matches() {
    let mut value = json!({
        "model": "gpt-5.6-luna",
        "system": "You are a security monitor for autonomous AI coding agents.",
        "messages": [{"role": "user", "content": "<transcript>\nx\n</transcript>"}],
    });
    let request = request_from(value.clone());
    assert_eq!(
        inspector::detected_request_kind(&request),
        Some(RequestKind::Classifier)
    );

    // stop_sequences 存在但不含分类器专用序列 → 不命中。
    value["stop_sequences"] = json!(["\n\nHuman:"]);
    let request = request_from(value);
    assert_eq!(inspector::detected_request_kind(&request), None);
}

#[test]
fn websearch_and_webfetch_fingerprints() {
    assert_eq!(
        inspector::detected_request_kind(&websearch_request()),
        Some(RequestKind::WebSearch)
    );
    assert_eq!(
        inspector::detected_request_kind(&webfetch_request()),
        Some(RequestKind::WebFetch)
    );

    let config = config_with_enabled_rules();
    let plan = RoutePlanner::plan(&websearch_request(), &config).unwrap();
    assert_eq!(plan.feature_rule_id, Some("websearch".into()));
}

#[test]
fn grok_websearch_requires_responses_without_fixed_protocol_upgrade() {
    let mut config = base_config();
    endpoint_mut(&mut config, "qwen")
        .mappings
        .push(mapping("grok-4.5", "opaque-anthropic-grok-alias"));
    endpoint_mut(&mut config, "qwen").protocol = EndpointProtocolMode::Auto;
    let mut rule = builtin_rules::canonical()
        .into_iter()
        .find(|rule| rule.id == "websearch")
        .unwrap();
    rule.enabled = true;
    rule.target = FeatureRuleTarget {
        endpoint_id: Some("qwen".into()),
        effort: None,
        model: "grok-4.5".into(),
        protocol_override: None,
    };
    config.feature_rules = vec![rule.clone()];

    let plan = RoutePlanner::plan(&websearch_request(), &config.clone().normalized()).unwrap();
    assert_eq!(
        plan.endpoints[0].protocol,
        ProviderProtocol::OpenAIResponses
    );
    assert_eq!(plan.endpoints[0].routed_model, "grok-4.5");
    assert_eq!(
        plan.endpoints[0].upstream_model,
        "opaque-anthropic-grok-alias"
    );

    for protocol in [ProviderProtocol::Anthropic, ProviderProtocol::OpenAI] {
        rule.target.protocol_override = Some(protocol);
        config.feature_rules = vec![rule.clone()];
        let err = RoutePlanner::plan(&websearch_request(), &config.clone().normalized())
            .expect_err("Grok WebSearch must not upgrade a fixed non-Responses target");
        assert_eq!(
            err,
            RoutePlanError::NoCompatibleProvider {
                source_format: "anthropic".into(),
            }
        );
    }
}

#[test]
fn contaminated_main_conversation_stays_on_primary() {
    // 主对话夹带 webfetch 风格文案:多条消息/带工具 → 指纹不命中,留在主池。
    let request = request_from(json!({
        "model": "claude-opus-5",
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [
            {"role": "user", "content": "Web page content:\n---\nfoo\n---\n please summarize"},
            {"role": "assistant", "content": "ok"},
            {"role": "user", "content": "continue"}
        ],
        "tools": [{"name": "Bash", "input_schema": {}}],
    }));
    assert_eq!(inspector::detected_request_kind(&request), None);
    assert_eq!(
        inspector::request_purpose(&request),
        RequestPurpose::Standard
    );

    let config = config_with_enabled_rules();
    let plan = RoutePlanner::plan(&request, &config).unwrap();
    assert_eq!(plan.feature_rule_id, None);
}

#[test]
fn session_title_purpose_without_feature_routing() {
    let request = request_from(json!({
        "model": "claude-haiku-4-5-20251001(high)",
        "system": "Analyze this conversation. Write the title in English. Keep technical terms and code identifiers in their original form.",
        "messages": [{"role": "user", "content": "<session>user asked about rust</session>"}],
    }));
    assert_eq!(
        inspector::request_purpose(&request),
        RequestPurpose::SessionTitle
    );
    assert_eq!(inspector::detected_request_kind(&request), None);

    let config = config_with_enabled_rules();
    let plan = RoutePlanner::plan(&request, &config).unwrap();
    assert_eq!(plan.feature_rule_id, None);
    assert_eq!(plan.client_model, "claude-haiku-4-5-20251001"); // (high) 已剥
}

/// `unmatched_no_tools` 的判据面:只有「带专用 system 的单轮无工具请求 + 未命中任何指纹」
/// 才命中。这是指纹随 CC 升级失配时唯一的可见信号,误报会变成常驻噪音,所以逐条钉死。
#[test]
fn unmatched_no_tools_only_flags_unrecognized_internal_shape() {
    // 失配的内部请求:CC 改了标题 system 的措辞 → session_title 指纹落空。
    let drifted = request_from(json!({
        "model": "claude-haiku-4-5-20251001",
        "system": "Generate a short title for this conversation.",
        "messages": [{"role": "user", "content": "<session>user asked about rust</session>"}],
    }));
    assert_eq!(
        inspector::request_purpose(&drifted),
        RequestPurpose::Standard
    );
    assert!(inspector::is_unmatched_no_tools(&drifted));

    // 主对话:system 含 CC 身份标识 → 不是内部请求,不提示。
    let main_chat = request_from(json!({
        "model": "claude-opus-5",
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [{"role": "user", "content": "hello"}],
    }));
    assert!(!inspector::is_unmatched_no_tools(&main_chat));

    // 带 tools:CC 主对话的常态形状。
    let with_tools = request_from(json!({
        "model": "claude-opus-5",
        "system": "Some other system prompt.",
        "messages": [{"role": "user", "content": "hello"}],
        "tools": [{"name": "Read", "description": "read a file"}],
    }));
    assert!(!inspector::is_unmatched_no_tools(&with_tools));

    // 无 system:裸 curl / 简易客户端的探测,不该常年挂提示。
    let bare = request_from(json!({
        "model": "claude-opus-5",
        "messages": [{"role": "user", "content": "hello"}],
    }));
    assert!(!inspector::is_unmatched_no_tools(&bare));

    // 已识别的用途在「用途」列已经标好,不重复提示。
    let titled = request_from(json!({
        "model": "claude-haiku-4-5-20251001",
        "system": "Analyze this conversation. Write the title in English. Keep technical terms and code identifiers in their original form.",
        "messages": [{"role": "user", "content": "<session>user asked about rust</session>"}],
    }));
    assert_eq!(
        inspector::request_purpose(&titled),
        RequestPurpose::SessionTitle
    );
    assert!(!inspector::is_unmatched_no_tools(&titled));

    // CC 2.1.220+ 的辅助请求 system 带 CC 身份 + 专用指令;措辞失配时必须告警,
    // 不能因为出现身份标识就被屏蔽 —— 这正是该信号存在的场景。
    let drifted_with_identity = request_from(json!({
        "model": "claude-haiku-4-5-20251001",
        "system": [
            {"type": "text", "text": "x-anthropic-billing-header: cch=abcde;"},
            {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
            {"type": "text", "text": "Produce a one-line summary of the transcript."}
        ],
        "messages": [{"role": "user", "content": "<transcript>x</transcript>"}],
    }));
    assert_eq!(
        inspector::request_purpose(&drifted_with_identity),
        RequestPurpose::Standard
    );
    assert!(inspector::is_unmatched_no_tools(&drifted_with_identity));

    // 多轮对话:内部辅助请求都是单轮。
    let multi_turn = request_from(json!({
        "model": "claude-opus-5",
        "system": "Some other system prompt.",
        "messages": [
            {"role": "user", "content": "hello"},
            {"role": "assistant", "content": "hi"},
            {"role": "user", "content": "again"},
        ],
    }));
    assert!(!inspector::is_unmatched_no_tools(&multi_turn));
}

#[test]
fn pinned_endpoint_bypasses_mapping_filter_and_degrades_when_disabled() {
    let mut config = config_with_enabled_rules();
    // 钉住 qwen 入口 + 一个映射之外的模型:绕过映射筛选仍强制走它。
    config.feature_rules[2].target.endpoint_id = Some("qwen".into());
    config.feature_rules[2].target.model = "some-unmapped-model".into();
    let config2 = config.clone().normalized();
    let plan = RoutePlanner::plan(&classifier_request(), &config2).unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].endpoint_id, "qwen");
    // 无映射时 upstream = effective 原名。
    assert_eq!(plan.endpoints[0].upstream_model, "some-unmapped-model");

    // 钉住的入口被停用后降级为统一 Provider 整池；目标模型没有其它可用入口时直接报错。
    endpoint_mut(&mut config, "qwen").enabled = false;
    let config3 = config.normalized();
    let err = RoutePlanner::plan(&classifier_request(), &config3).unwrap_err();
    assert_eq!(
        err,
        RoutePlanError::NoCompatibleProvider {
            source_format: "anthropic".into(),
        }
    );
}

#[test]
fn feature_rule_protocol_override_applies() {
    let mut config = config_with_enabled_rules();
    endpoint_mut(&mut config, "qwen").protocol = EndpointProtocolMode::Auto;
    config.feature_rules[2].target.protocol_override = Some(ProviderProtocol::OpenAI);
    let config = config.normalized();
    let plan = RoutePlanner::plan(&classifier_request(), &config).unwrap();
    assert_eq!(plan.endpoints[0].protocol, ProviderProtocol::OpenAI);
}

#[test]
fn feature_rule_without_mapping_reports_unified_provider_error() {
    let mut config = base_config();
    config.feature_rules = builtin_rules::canonical();
    config.feature_rules[2].enabled = true;
    config.feature_rules[2].target.model = "totally-unmapped".into();
    let config = config.normalized();
    // classifier 命中但统一 Provider 无 totally-unmapped 映射 → 目标池直接报错。
    let err = RoutePlanner::plan(&classifier_request(), &config).unwrap_err();
    assert_eq!(
        err,
        RoutePlanError::NoCompatibleProvider {
            source_format: "anthropic".into(),
        }
    );
}

#[test]
fn custom_rule_conditions_are_anded_and_require_one() {
    let request = request_from(json!({
        "model": "claude-opus-5",
        "system": "You are a batch runner v2",
        "messages": [{"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "t1", "content": [{"type": "text", "text": "BATCH JOB done"}]}
        ]}],
        "tools": [{"name": "x", "type": "mcp__jira__search"}],
    }));
    let mut rule = FeatureRule {
        enabled: true,
        id: "custom".into(),
        match_: FeatureRuleMatch {
            messages_contain: Some("batch job".into()),
            system_contains: Some("batch runner".into()),
            tool_type_prefix: Some("mcp__".into()),
            model_equals: Some("claude-opus-5[1m]".into()),
            request_kind: None,
        },
        name: "custom".into(),
        target: FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "m".into(),
            protocol_override: None,
        },
    };
    assert!(inspector::feature_rule_matches(&rule, &request));

    // 任一条件不满足即失败(AND)。
    rule.match_.system_contains = Some("nonexistent".into());
    assert!(!inspector::feature_rule_matches(&rule, &request));
    rule.match_.system_contains = None;
    assert!(inspector::feature_rule_matches(&rule, &request)); // 剩余条件仍成立

    // 全部条件为空 → 不命中。
    let empty = FeatureRule {
        match_: FeatureRuleMatch::default(),
        ..rule.clone()
    };
    assert!(!inspector::feature_rule_matches(&empty, &request));

    // disabled → 不命中。
    rule.enabled = false;
    assert!(!inspector::feature_rule_matches(&rule, &request));
}

/// 2.1.220+ 的标题请求靠 output_config 的 json_schema 识别:system 措辞改了也不能
/// 退化成 Standard。
#[test]
fn session_title_recognized_by_structured_output_schema() {
    let structured = request_from(json!({
        "model": "claude-haiku-4-5-20251001",
        "system": [
            {"type": "text", "text": "x-anthropic-billing-header: cch=abcde;"},
            {"type": "text", "text": "You are Claude Code, Anthropic's official CLI for Claude."},
            {"type": "text", "text": "Some future wording nobody has seen yet."}
        ],
        "messages": [{"role": "user", "content": "<session>user asked about rust</session>"}],
        "thinking": {"type": "disabled"},
        "output_config": {"format": {"type": "json_schema", "schema": {
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
            "additionalProperties": false
        }}},
    }));
    assert_eq!(
        inspector::request_purpose(&structured),
        RequestPurpose::SessionTitle
    );
    // 已识别 → 不再算失配。
    assert!(!inspector::is_unmatched_no_tools(&structured));

    // schema 约束的不是 title 时不认:那是别的结构化输出请求。
    let other_schema = request_from(json!({
        "model": "claude-haiku-4-5-20251001",
        "system": "Some future wording nobody has seen yet.",
        "messages": [{"role": "user", "content": "<session>x</session>"}],
        "output_config": {"format": {"type": "json_schema", "schema": {
            "type": "object",
            "properties": {"summary": {"type": "string"}}
        }}},
    }));
    assert_eq!(
        inspector::request_purpose(&other_schema),
        RequestPurpose::Standard
    );
}

#[test]
fn tool_type_prefix_requires_absence_of_client_tools() {
    // 存在无 type 的客户端工具 → 不算命中(主会话带 MCP 工具不误触发)。
    let request = request_from(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "x"}],
        "tools": [
            {"name": "mcp_tool", "type": "mcp__jira__search"},
            {"name": "Bash", "input_schema": {}}
        ],
    }));
    assert!(!inspector::has_tool_type(&request, "mcp__"));

    // 显式 `type: "custom"` 是同一件事的另一种 wire 形状,同样不算命中。
    let explicit_custom = request_from(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "x"}],
        "tools": [
            {"name": "mcp_tool", "type": "mcp__jira__search"},
            {"name": "Bash", "type": "custom", "input_schema": {}}
        ],
    }));
    assert!(!inspector::has_tool_type(&explicit_custom, "mcp__"));

    // 只有目标前缀工具 → 命中。
    let only_target = request_from(json!({
        "model": "m",
        "messages": [{"role": "user", "content": "x"}],
        "tools": [{"name": "mcp_tool", "type": "mcp__jira__search"}],
    }));
    assert!(inspector::has_tool_type(&only_target, "mcp__"));
}

// ---------------------------------------------------------------------------
// 粘性哈希
// ---------------------------------------------------------------------------

#[test]
fn session_key_stable_and_fingerprint_is_distinct() {
    let request = plain_request("claude-opus-5");
    let key1 = sticky::session_key(&request);
    let key2 = sticky::session_key(&request);
    assert_eq!(key1, key2);
    assert_eq!(key1.len(), 32); // md5 hex

    // 不同会话(不同首条 user)大概率不同 key;至少不 panic 且长度一致。
    let other = request_from(json!({
        "model": "claude-opus-5",
        "system": "You are Claude Code, Anthropic's official CLI for Claude.",
        "messages": [{"role": "user", "content": "another session"}],
    }));
    assert_ne!(sticky::session_key(&other), key1);
}

#[test]
fn session_key_uses_python_style_for_structured_content() {
    // 结构化 content 走 python 风格序列化,不 panic 且确定性。
    let request = request_from(json!({
        "model": "m",
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": "hi"},
            {"type": "image", "source": {"type": "base64", "data": "xx"}}
        ]}],
    }));
    assert_eq!(sticky::session_key(&request), sticky::session_key(&request));
}

#[test]
fn stable_claude_session_key_ignores_context_changes_and_hashes_the_identifier() {
    let first = request_from(json!({
        "model": "claude-opus-5",
        "system": "system before compact",
        "messages": [{"role": "user", "content": "first message"}],
        "metadata": {"user_id": "same-device-not-session"}
    }));
    let compacted = request_from(json!({
        "model": "claude-opus-5",
        "system": "system after compact",
        "messages": [{"role": "user", "content": "summary replacement"}],
        "metadata": {"user_id": "same-device-not-session"}
    }));

    let first_key = sticky::resolved_sticky_key(
        sticky::resolve_session_identity(&first, Some(" session-secret-a ")),
        "claude-opus-5",
        None,
    );
    let compacted_key = sticky::resolved_sticky_key(
        sticky::resolve_session_identity(&compacted, Some("session-secret-a")),
        "claude-opus-5",
        None,
    );
    assert!(first_key.persistent);
    assert_eq!(first_key, compacted_key);
    assert_eq!(first_key.value.len(), 64);
    assert!(!first_key.value.contains("session-secret-a"));

    let other = sticky::resolved_sticky_key(
        sticky::resolve_session_identity(&compacted, Some("session-secret-b")),
        "claude-opus-5",
        None,
    );
    assert_ne!(first_key.value, other.value);

    let legacy_first = sticky::resolved_sticky_key(
        sticky::resolve_session_identity(&first, None),
        "claude-opus-5",
        None,
    );
    let legacy_compacted = sticky::resolved_sticky_key(
        sticky::resolve_session_identity(&compacted, Some("   ")),
        "claude-opus-5",
        None,
    );
    assert!(!legacy_first.persistent);
    assert_ne!(legacy_first.value, legacy_compacted.value);
}

#[test]
fn sticky_key_v3_matches_canonical_sha256_vectors_and_redacts_debug() {
    let stable = sticky::StickyKey::new(
        sticky::SessionIdentity::StableSession("session-1".into()),
        "claude-opus-5",
        None,
    );
    assert_eq!(
        stable.affinity_id(),
        "f5ef272c795ec9ae38fef42d40a07edcbf7a813324a222f055743e47d84fac2c"
    );
    assert!(stable.persistent());
    assert!(!format!("{stable:?}").contains("session-1"));

    let content = sticky::StickyKey::new(
        sticky::SessionIdentity::ContentFingerprint("same".into()),
        "m",
        Some(String::new()),
    );
    assert_eq!(
        content.affinity_id(),
        "5e007e24c9977613afc4f4844c643b2fbca93666d035f7e1a11e2ad3f18563b2"
    );
    assert!(!content.persistent());

    let unicode = sticky::StickyKey::new(
        sticky::SessionIdentity::StableSession("会话🧪".into()),
        "模型",
        Some("规则".into()),
    );
    assert_eq!(
        unicode.affinity_id(),
        "b376a00daaa61da23110a3cd7f2e9614a66a6829d2c44c9c56fce9b01b763db5"
    );
}

#[test]
fn sticky_key_v3_namespaces_source_and_remaining_route_dimensions() {
    fn affinity(identity: sticky::SessionIdentity, model: &str, rule: Option<&str>) -> String {
        sticky::resolved_sticky_key(identity, model, rule).value
    }

    let baseline = affinity(
        sticky::SessionIdentity::StableSession("session-a".into()),
        "claude-opus-5",
        None,
    );
    assert_eq!(
        baseline,
        affinity(
            sticky::SessionIdentity::StableSession("session-a".into()),
            "claude-opus-5",
            None,
        )
    );
    for changed in [
        affinity(
            sticky::SessionIdentity::StableSession("session-b".into()),
            "claude-opus-5",
            None,
        ),
        affinity(
            sticky::SessionIdentity::StableSession("session-a".into()),
            "claude-sonnet-5",
            None,
        ),
        affinity(
            sticky::SessionIdentity::StableSession("session-a".into()),
            "claude-opus-5",
            Some("rule-a"),
        ),
    ] {
        assert_ne!(baseline, changed);
    }

    assert_ne!(
        baseline,
        affinity(
            sticky::SessionIdentity::StableSession("session-a".into()),
            "claude-opus-5",
            Some(""),
        )
    );
    assert_ne!(
        affinity(
            sticky::SessionIdentity::StableSession("same".into()),
            "m",
            None,
        ),
        affinity(
            sticky::SessionIdentity::ContentFingerprint("same".into()),
            "m",
            None,
        )
    );

    // 长度前缀使相邻字段无法通过移动边界产生相同规范字节串。
    assert_ne!(
        affinity(
            sticky::SessionIdentity::StableSession("s".into()),
            "ab",
            Some("c"),
        ),
        affinity(
            sticky::SessionIdentity::StableSession("s".into()),
            "a",
            Some("bc"),
        )
    );
}

#[test]
fn session_identity_preserves_case_trims_and_redacts_stable_ids() {
    let request = plain_request("m");
    let stable = sticky::resolve_session_identity(&request, Some("  Session-A  "));
    assert_eq!(
        stable,
        sticky::SessionIdentity::StableSession("Session-A".into())
    );
    assert!(stable.persistent());
    assert!(!format!("{stable:?}").contains("Session-A"));

    let missing = sticky::resolve_session_identity(&request, None);
    let blank = sticky::resolved_session_identity(&request, Some(" \t\n "));
    assert_eq!(missing, blank);
    assert!(!missing.persistent());
}

#[test]
fn video_intent_does_not_use_the_first_text_provider() {
    let mut xiao = endpoint("xiao", vec![mapping("claude-fable-5", "")]);
    xiao.protocol = EndpointProtocolMode::OpenAI;
    let mut cpa = endpoint(
        "cpa",
        vec![
            mapping("gpt-5.6-sol", ""),
            mapping("grok-imagine-video", ""),
            mapping("grok-imagine-image", ""),
        ],
    );
    cpa.protocol = EndpointProtocolMode::OpenAI;
    let config = AppConfig {
        endpoints: vec![xiao, cpa],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    assert_eq!(
        RoutePlanner::default_model_for_capability(
            &config,
            sumpter_core::capability::ModelCapability::Video
        )
        .as_deref(),
        Some("grok-imagine-video")
    );

    let plan = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "grok-imagine-video"})),
        &config,
        ProviderProtocol::OpenAI,
        sumpter_core::capability::ModelCapability::Video,
    )
    .unwrap();
    assert_eq!(plan.endpoints[0].endpoint_id, "cpa");

    let rejected = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "claude-fable-5"})),
        &config,
        ProviderProtocol::OpenAI,
        sumpter_core::capability::ModelCapability::Video,
    )
    .unwrap_err();
    assert!(matches!(
        rejected,
        RoutePlanError::NoProviderForCapability { .. }
    ));
}

#[test]
fn text_wildcard_cannot_steal_video_capability_traffic() {
    let mut xiao = endpoint("xiao", vec![mapping("*", "stolen")]);
    xiao.protocol = EndpointProtocolMode::OpenAI;
    xiao.priority = 0;
    let mut cpa = endpoint("cpa", vec![mapping("grok-imagine-video", "")]);
    cpa.protocol = EndpointProtocolMode::OpenAI;
    cpa.priority = 10;
    let config = AppConfig {
        endpoints: vec![xiao, cpa],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let plan = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "grok-imagine-video"})),
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Video,
    )
    .unwrap();
    assert_eq!(plan.endpoints.len(), 1);
    assert_eq!(plan.endpoints[0].endpoint_id, "cpa");
}

#[test]
fn explicit_mapping_capabilities_override_name_inference_for_routing() {
    let mut video = endpoint(
        "video",
        vec![ModelMapping {
            client_pattern: "grok-imagine-video".into(),
            context: ContextMode::Standard,
            failover_timeout_seconds: None,
            thinking: ThinkingMode::Disabled,
            effort: None,
            upstream_model: "grok-imagine-video".into(),
            capabilities: vec![sumpter_core::capability::ModelCapability::Text],
        }],
    );
    video.protocol = EndpointProtocolMode::OpenAI;
    let config = AppConfig {
        endpoints: vec![video],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let error = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "grok-imagine-video"})),
        &config,
        ProviderProtocol::OpenAI,
        sumpter_core::capability::ModelCapability::Video,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        RoutePlanError::NoProviderForCapability { .. }
    ));
}

#[test]
fn realtime_public_model_aliases_use_private_codex_live_mapping() {
    let mut endpoint = endpoint(
        "cpa",
        vec![ModelMapping {
            client_pattern: "gpt-live-1-codex".into(),
            upstream_model: String::new(),
            capabilities: vec![ModelCapability::Live],
            ..mapping("gpt-live-1-codex", "")
        }],
    );
    endpoint.protocol = EndpointProtocolMode::OpenAI;
    let config = AppConfig {
        endpoints: vec![endpoint],
        feature_rules: vec![],
        listener: ListenerConfig::default(),
        retry: RetryPolicy::default(),
        schema_version: SCHEMA_VERSION,
    }
    .normalized();

    let plan = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "gpt-realtime"})),
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Live,
    )
    .expect("private Live mapping should serve public Realtime alias");
    assert_eq!(plan.client_model, "gpt-realtime");
    assert_eq!(plan.endpoints[0].routed_model, "gpt-realtime");
    assert_eq!(plan.endpoints[0].upstream_model, "gpt-live-1-codex");

    let preview = RoutePlanner::plan_for_capability(
        &request_from(json!({"model": "realtime-preview-2025"})),
        &config,
        ProviderProtocol::OpenAI,
        ModelCapability::Live,
    )
    .expect("realtime-preview should share the alias scope");
    assert_eq!(preview.endpoints[0].upstream_model, "gpt-live-1-codex");
}
