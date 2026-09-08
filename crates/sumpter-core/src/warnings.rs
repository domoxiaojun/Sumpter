//! 配置校验:纯函数,输入 AppConfig(建议先 normalized),输出 reload wire 的提示列表。
//! 文案在 daemon reload 与 Admin API 之间逐字共用;macOS App UI 不再展示配置风险监测,
//! 但后端提示字段保留以兼容现有 wire。
//! 分组类提示(重复/共用地址)在 Swift 侧因字典无序而顺序不稳,这里按首次出现顺序输出;
//! 测试仍应断言「包含某条」而非整列顺序。

use crate::config::{AppConfig, Endpoint, FeatureRule, ProviderProtocol};
use crate::model_name;

pub fn evaluate(config: &AppConfig) -> Vec<String> {
    let mut risks: Vec<String> = Vec::new();
    if config.accepted_models().is_empty() {
        risks.push("没有任何 Provider 候选声明承接模型：请添加显式模型映射。".into());
    }
    if !config
        .endpoints
        .iter()
        .any(|endpoint| endpoint.enabled && !endpoint.api_key.is_empty())
    {
        risks.push("没有启用且配好 Key 的 Provider 候选。".into());
    }

    if !is_local_host(&config.listener.host)
        && config.listener.auth_token.trim().is_empty()
        && config.listener.allowed_cidrs.is_empty()
    {
        risks.push(format!(
            "监听地址 {} 对外开放，但未设置入站 Auth Token 或 CIDR 白名单。",
            config.listener.host
        ));
    }

    let all_ids: Vec<&str> = config.endpoints.iter().map(|e| e.id.as_str()).collect();
    append_duplicate_warnings(&all_ids, "入口 ID", &mut risks);

    for endpoint in &config.endpoints {
        append_endpoint_warnings(endpoint, &mut risks);
    }
    append_shared_resource_warnings(&config.endpoints, &mut risks);
    append_lossy_translation_warnings(config, &mut risks);

    for rule in config.feature_rules.iter().filter(|r| r.enabled) {
        append_feature_rule_warnings(rule, config, &mut risks);
    }
    risks
}

/// 有损翻译降级提示。
///
/// 出站桥能表达文本、图片和工具调用,但表达不了结构化输出(`output_config`),也不
/// 回放历史推理。一个模型若**只**能落到非 Anthropic 协议的入口,该模型的 Claude
/// 客户端流量就必然走翻译面 —— 这在运行时只表现为「模型行为不如预期」,配置期不
/// 提示的话没人会想到是协议转换造成的。
///
/// 只在「该模型没有任何 Anthropic 入口可落」时提示:混合配置下 native 入口优先,
/// 翻译面只在 native 全不可用时才会被用到,不值得为此报警。
fn append_lossy_translation_warnings(config: &AppConfig, risks: &mut Vec<String>) {
    for model in config.accepted_models() {
        let mut has_anthropic = false;
        let mut translated_targets: Vec<&str> = Vec::new();
        for endpoint in config.endpoints.iter().filter(|e| e.enabled) {
            if endpoint.mapping_for(&model).is_none() {
                continue;
            }
            match endpoint.protocol.fixed_protocol() {
                // Auto 入口跟随入站协议,Anthropic 客户端进来就是原生转发。
                None | Some(ProviderProtocol::Anthropic) => has_anthropic = true,
                Some(protocol) => {
                    let token = protocol.token();
                    if !translated_targets.contains(&token) {
                        translated_targets.push(token);
                    }
                }
            }
        }
        if has_anthropic || translated_targets.is_empty() {
            continue;
        }
        risks.push(format!(
            "{model} 只能落到 {} 协议入口：Claude 客户端的请求会经协议转换转发，\
             结构化输出(output_config)会被拒绝，历史推理内容不会回放。",
            translated_targets.join(" / ")
        ));
    }
}

fn append_endpoint_warnings(endpoint: &Endpoint, risks: &mut Vec<String>) {
    let id = if endpoint.id.is_empty() {
        if endpoint.name.is_empty() {
            "入口"
        } else {
            &endpoint.name
        }
    } else {
        &endpoint.id
    };
    if endpoint.api_key.is_empty() {
        // 空 key 是合法用法(本地 LLM / 内网无鉴权上游),但多数情况是漏填,故仍提示。
        risks.push(format!("{id} 未配置 API Key(将以无鉴权方式转发)。"));
    }
    let mut seen = std::collections::HashSet::new();
    for mapping in &endpoint.mappings {
        let client = model_name::clean(&mapping.client_pattern);
        if client.is_empty() {
            risks.push(format!("{id} 存在空的客户端模型映射。"));
            continue;
        }
        if seen.contains(&client) {
            risks.push(format!("{id} 内重复映射 {client}；只会保留一条生效路径。"));
        }
        seen.insert(client.clone());
        if let Some(timeout) = mapping.failover_timeout_seconds
            && timeout < 1.0
        {
            risks.push(format!("{client} / {id} 首个超时秒数必须大于 0。"));
        }
    }
}

/// Provider 候选间共用 API 地址 / key 尾号相同 —— 多半是复制粘贴时忘了改。
/// 同一中转站的多账号本来就共用地址,但应显式填写相同粘性组；空值会按入口 ID
/// 分成独立组，因此仍提示用户确认是否漏填共享组。
fn append_shared_resource_warnings(endpoints: &[Endpoint], risks: &mut Vec<String>) {
    let mut base_owners: Vec<(String, Vec<String>)> = Vec::new();
    let mut key_tail_owners: Vec<(String, Vec<String>)> = Vec::new();
    let mut sticky_priorities: Vec<(String, i64, String)> = Vec::new();
    for endpoint in endpoints {
        let id = if endpoint.id.is_empty() {
            endpoint.name.clone()
        } else {
            endpoint.id.clone()
        };
        if endpoint.sticky_group.is_none() {
            push_group(&mut base_owners, endpoint.base_url.clone(), id.clone());
        }
        if let Some(group) = endpoint.sticky_group.as_deref() {
            if let Some((_, minimum, _)) = sticky_priorities
                .iter_mut()
                .find(|(name, _, _)| name == group)
            {
                *minimum = (*minimum).min(endpoint.priority);
            } else {
                sticky_priorities.push((group.to_string(), endpoint.priority, id.clone()));
            }
        }
        if endpoint.api_key.chars().count() >= 4 {
            let tail: String = {
                let chars: Vec<char> = endpoint.api_key.chars().collect();
                chars[chars.len() - 4..].iter().collect()
            };
            push_group(&mut key_tail_owners, tail, id);
        }
    }
    for (base, owners) in base_owners.iter().filter(|(_, o)| o.len() > 1) {
        risks.push(format!(
            "多个 Provider 入口共用 API 地址 {}：{}。",
            base,
            owners.join(", ")
        ));
    }
    for (tail, owners) in key_tail_owners.iter().filter(|(_, o)| o.len() > 1) {
        risks.push(format!(
            "多个 Provider 入口 key 尾号相同 ...{}：{}。",
            tail,
            owners.join(", ")
        ));
    }
    for (group, minimum, _) in sticky_priorities.iter() {
        let members = endpoints
            .iter()
            .filter(|endpoint| endpoint.sticky_group.as_deref() == Some(group.as_str()))
            .collect::<Vec<_>>();
        if members.iter().any(|endpoint| endpoint.priority != *minimum) {
            let details = members
                .iter()
                .map(|endpoint| format!("{}={}", endpoint.id, endpoint.priority))
                .collect::<Vec<_>>()
                .join(", ");
            risks.push(format!(
                "粘性组 {group} 内入口优先级不一致（{details}）；调度按组内最低值 {minimum}，组内仍按配置顺序。"
            ));
        }
    }
}

fn append_feature_rule_warnings(rule: &FeatureRule, config: &AppConfig, risks: &mut Vec<String>) {
    let id = if rule.id.is_empty() {
        "规则"
    } else {
        &rule.id
    };
    let target = model_name::clean(&rule.target.model);
    if target.is_empty() {
        risks.push(format!("分流规则 {id} 已启用但未选 model。"));
        return;
    }
    if let Some(endpoint_id) = &rule.target.endpoint_id {
        if config.endpoint(endpoint_id).is_none() {
            risks.push(format!(
                "分流规则 {id} 固定的 Provider {endpoint_id} 已不存在；会退回候选序列 failover。"
            ));
        }
    } else if !config.matches_model(&target) {
        risks.push(format!(
            "分流规则 {id} 使用候选序列，但 model {target} 没有任何 Provider 承接来源。"
        ));
    }
}

pub fn is_local_host(host: &str) -> bool {
    let trimmed = host.trim().to_lowercase();
    trimmed.is_empty() || trimmed == "127.0.0.1" || trimmed == "localhost" || trimmed == "::1"
}

fn append_duplicate_warnings(values: &[&str], name: &str, risks: &mut Vec<String>) {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for value in values {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        push_group(&mut groups, trimmed.to_lowercase(), trimmed.to_string());
    }
    for (_, duplicates) in groups.iter().filter(|(_, d)| d.len() > 1) {
        risks.push(format!("{name} 重复：{}。", duplicates[0]));
    }
}

fn push_group(groups: &mut Vec<(String, Vec<String>)>, key: String, value: String) {
    match groups.iter_mut().find(|(k, _)| *k == key) {
        Some((_, list)) => list.push(value),
        None => groups.push((key, vec![value])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::*;

    fn endpoint(id: &str, key: &str) -> Endpoint {
        Endpoint {
            api_key: key.into(),
            base_url: format!("https://{id}.example.com"),
            catalog: None,
            enabled: true,
            id: id.into(),
            keep_alive: false,
            mappings: vec![],
            name: id.into(),
            priority: 0,
            protocol: EndpointProtocolMode::Anthropic,
            sticky_group: None,
        }
    }

    fn config() -> AppConfig {
        AppConfig {
            endpoints: vec![Endpoint {
                mappings: vec![ModelMapping {
                    client_pattern: "claude-opus-*".into(),
                    context: ContextMode::OneMillion,
                    failover_timeout_seconds: None,
                    thinking: ThinkingMode::Adaptive,
                    effort: None,
                    upstream_model: String::new(),
                    capabilities: Vec::new(),
                }],
                ..endpoint("a", "sk-11112222")
            }],
            feature_rules: vec![],
            listener: ListenerConfig::default(),
            retry: RetryPolicy::default(),
            session_sticky_ttl_hours: crate::config::DEFAULT_SESSION_STICKY_TTL_HOURS,
            schema_version: SCHEMA_VERSION,
            model_groups: None,
        }
    }

    #[test]
    fn healthy_config_has_no_warnings() {
        assert!(evaluate(&config()).is_empty());
    }

    #[test]
    fn empty_primary_and_missing_key_warn() {
        let mut c = config();
        c.endpoints[0].mappings.clear();
        c.endpoints[0].api_key = String::new();
        let risks = evaluate(&c);
        assert!(
            risks.contains(&"没有任何 Provider 候选声明承接模型：请添加显式模型映射。".to_string())
        );
        assert!(risks.contains(&"没有启用且配好 Key 的 Provider 候选。".to_string()));
        assert!(risks.contains(&"a 未配置 API Key(将以无鉴权方式转发)。".to_string()));
    }

    #[test]
    fn model_reachable_only_through_translation_warns() {
        let mut c = config();
        // 唯一承接该模型的入口是 OpenAI 协议 → Claude 客户端必然走翻译面。
        c.endpoints[0].protocol = crate::config::EndpointProtocolMode::OpenAI;
        let risks = evaluate(&c);
        assert!(
            risks
                .iter()
                .any(|risk| risk.contains("只能落到 openai 协议入口")),
            "{risks:?}"
        );

        // 只要还有一个 Anthropic(或 Auto)入口能承接,native 优先,不提示。
        let mut native = c.endpoints[0].clone();
        native.id = "b".into();
        native.protocol = crate::config::EndpointProtocolMode::Anthropic;
        c.endpoints.push(native);
        let risks = evaluate(&c);
        assert!(
            !risks.iter().any(|risk| risk.contains("只能落到")),
            "{risks:?}"
        );
    }

    #[test]
    fn exposed_listener_without_auth_warns() {
        let mut c = config();
        c.listener.host = "0.0.0.0".into();
        let risks = evaluate(&c);
        assert!(risks.iter().any(|r| r.contains("对外开放")));
        // 设了 token 后不再警告。
        c.listener.auth_token = "t".into();
        assert!(!evaluate(&c).iter().any(|r| r.contains("对外开放")));
    }

    #[test]
    fn provider_endpoint_without_mappings_is_not_a_model_candidate() {
        let mut c = config();
        c.endpoints.push(endpoint("b", "sk-33334444"));
        assert!(c.endpoints[1].mapping_for("claude-opus-5").is_none());
    }

    #[test]
    fn shared_base_warns_without_sticky_group_and_key_tail_still_warns() {
        let mut c = config();
        let mut e1 = endpoint("x1", "sk-9999");
        let mut e2 = endpoint("x2", "sk-0000");
        e1.base_url = "https://same.example.com".into();
        e2.base_url = "https://same.example.com".into();
        c.endpoints = vec![e1.clone(), e2.clone()];
        let risks = evaluate(&c);
        assert!(risks.iter().any(|r| r.contains("共用 API 地址")));

        // 显式分组表示这是同一中转站的多账号,不再提示共用地址。
        e1.sticky_group = Some("g1".into());
        e2.sticky_group = Some("g2".into());
        c.endpoints = vec![e1.clone(), e2.clone()];
        assert!(!evaluate(&c).iter().any(|r| r.contains("共用 API 地址")));

        // key 尾号相同仍要警告(与粘性分组无关)。
        let mut k1 = endpoint("k1", "sk-abcd1234");
        let mut k2 = endpoint("k2", "sk-zzzz1234");
        k1.sticky_group = Some("g1".into());
        k2.sticky_group = Some("g2".into());
        c.endpoints = vec![k1, k2];
        assert!(
            evaluate(&c)
                .iter()
                .any(|r| r.contains("key 尾号相同 ...1234"))
        );
    }

    #[test]
    fn mixed_priority_sticky_group_warns_with_minimum_rule() {
        let mut c = config();
        let mut second = endpoint("b", "sk-33334444");
        second.sticky_group = Some("shared".into());
        second.priority = 10;
        c.endpoints[0].sticky_group = Some("shared".into());
        c.endpoints[0].priority = 1;
        c.endpoints.push(second);
        let risks = evaluate(&c);
        assert!(
            risks
                .iter()
                .any(|risk| risk.contains("优先级不一致") && risk.contains("最低值 1"))
        );
    }

    #[test]
    fn overlapping_provider_mapping_is_allowed() {
        let mut c = config();
        let mut b = endpoint("b", "sk-33334444");
        b.mappings.push(ModelMapping {
            client_pattern: "claude-opus-5".into(),
            context: ContextMode::Standard,
            failover_timeout_seconds: None,
            thinking: ThinkingMode::Disabled,
            effort: None,
            upstream_model: "x".into(),
            capabilities: Vec::new(),
        });
        c.endpoints.push(b);
        let risks = evaluate(&c);
        assert!(
            !risks
                .iter()
                .any(|risk| risk.contains("备用映射不会被默认路由使用"))
        );
    }

    #[test]
    fn feature_rule_warnings() {
        let mut c = config();
        c.feature_rules = builtin_rules::canonical();
        c.feature_rules[0].enabled = true;
        c.feature_rules[0].target.model = String::new();
        c.feature_rules[1].enabled = true;
        c.feature_rules[1].target.model = "missing-model".into();
        c.feature_rules[2].enabled = true;
        c.feature_rules[2].target.endpoint_id = Some("ghost".into());
        let risks = evaluate(&c);
        assert!(risks.contains(&"分流规则 websearch 已启用但未选 model。".to_string()));
        assert!(risks.contains(&"分流规则 webfetch 使用候选序列，但 model missing-model 没有任何 Provider 承接来源。".to_string()));
        assert!(
            risks.contains(
                &"分流规则 classifier 固定的 Provider ghost 已不存在；会退回候选序列 failover。"
                    .to_string()
            )
        );

        // 候选序列无承接来源。
        let mut c2 = config();
        c2.feature_rules = builtin_rules::canonical();
        c2.feature_rules[2].enabled = true;
        let risks = evaluate(&c2);
        assert!(
            risks
                .iter()
                .any(|r| r.contains("没有任何 Provider 承接来源"))
        );
    }

    #[test]
    fn duplicate_endpoint_ids_warn_case_insensitive() {
        let mut c = config();
        c.endpoints.push(endpoint("A", "sk-55556666"));
        // "a" 与 "A" 跨迁移来源大小写不敏感重复。
        let risks = evaluate(&c);
        assert!(risks.contains(&"入口 ID 重复：a。".to_string()));
    }
}
