//! Model groups organize candidates; retries and session affinity remain owned
//! by the existing scheduler. Endpoint credentials are never copied into groups.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::{AppConfig, ContextMode, Endpoint, ModelMapping, ThinkingMode};
use crate::model_name;

fn enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelGroup {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i64,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub bindings: Vec<ModelGroupBinding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelGroupBinding {
    #[serde(rename = "endpointID")]
    pub endpoint_id: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub priority: i64,
    /// null = all group models, [] = no models. Discovery never expands this list.
    #[serde(default)]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<ModelGroupModelOverride>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelGroupModelOverride {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct RoutingEndpoint {
    pub endpoint: Endpoint,
    pub group_id: Option<String>,
    pub group_name: Option<String>,
    /// Rank after stable sorting by group priority and configuration position.
    pub group_rank: usize,
    pub model_priorities: BTreeMap<String, i64>,
}

impl RoutingEndpoint {
    pub fn legacy(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            group_id: None,
            group_name: None,
            group_rank: 0,
            model_priorities: BTreeMap::new(),
        }
    }
}

/// Existing model matching supports exact names and a single trailing '*'.
fn intersection(a: &str, b: &str) -> Option<String> {
    let a = model_name::clean(a);
    let b = model_name::clean(b);
    if model_name::pattern_matches(&a, &b) {
        Some(b)
    } else if model_name::pattern_matches(&b, &a) {
        Some(a)
    } else {
        None
    }
}

fn mapping(pattern: String) -> ModelMapping {
    ModelMapping {
        client_pattern: pattern,
        upstream_model: String::new(),
        thinking: ThinkingMode::Adaptive,
        context: ContextMode::Standard,
        effort: None,
        failover_timeout_seconds: None,
        capabilities: vec![],
    }
}

/// Rank an original mapping before it is projected into a group's model
/// namespace.  This mirrors `Endpoint::mapping_for`: exact names win, then
/// longer trailing-wildcard prefixes, with a bare `*` last.
fn mapping_specificity(pattern: &str) -> (u8, usize) {
    let cleaned = model_name::clean(pattern);
    if !cleaned.is_empty() && !cleaned.ends_with('*') {
        return (2, cleaned.len());
    }
    (1, cleaned.strip_suffix('*').unwrap_or(&cleaned).len())
}

impl AppConfig {
    /// A missing collection is the legacy flat configuration; an explicit empty
    /// collection disables automatic routing. This distinction prevents a delete
    /// operation from accidentally opening access to the entire endpoint library.
    pub fn routing_endpoints(&self) -> Vec<RoutingEndpoint> {
        let Some(groups) = &self.model_groups else {
            return self
                .endpoints
                .iter()
                .cloned()
                .map(RoutingEndpoint::legacy)
                .collect();
        };
        let mut ordered: Vec<_> = groups.iter().filter(|g| g.enabled).collect();
        ordered.sort_by_key(|g| g.priority);
        let mut output = Vec::new();
        for (group_rank, group) in ordered.into_iter().enumerate() {
            for binding in group.bindings.iter().filter(|b| b.enabled) {
                let Some(source) = self.endpoint(&binding.endpoint_id).filter(|e| e.enabled) else {
                    continue;
                };
                let patterns: Vec<String> = match &binding.models {
                    None => group.models.clone(),
                    Some(models) => group
                        .models
                        .iter()
                        .flat_map(|g| models.iter().filter_map(move |m| intersection(g, m)))
                        .collect(),
                };
                let mut endpoint = source.clone();
                endpoint.priority = binding.priority;
                endpoint.mappings.clear();
                // Preserve every existing capability-specific mapping and all
                // of its thinking/context/effort/timeout settings inside the scope.
                // Projection can make a broad source mapping and a precise
                // source mapping share the same client pattern (for example
                // `*` and `gpt-x` both become `gpt-x`). Preserve capability
                // variants and order collisions by their source specificity.
                let mut inherited: Vec<(ModelMapping, (u8, usize))> = Vec::new();
                for original in &source.mappings {
                    for pattern in &patterns {
                        if let Some(pattern) = intersection(&original.client_pattern, pattern) {
                            let mut scoped = original.clone();
                            scoped.client_pattern = pattern;
                            let rank = mapping_specificity(&original.client_pattern);
                            if let Some(index) = inherited.iter().position(|(m, _)| m == &scoped) {
                                if rank <= inherited[index].1 {
                                    continue;
                                }
                                inherited.remove(index);
                            }
                            let index = inherited
                                .iter()
                                .position(|(m, previous_rank)| {
                                    m.client_pattern == scoped.client_pattern
                                        && rank > *previous_rank
                                })
                                .unwrap_or(inherited.len());
                            inherited.insert(index, (scoped, rank));
                        }
                    }
                }
                endpoint.mappings = inherited.into_iter().map(|(mapping, _)| mapping).collect();
                for pattern in patterns {
                    if !endpoint
                        .mappings
                        .iter()
                        .any(|m| m.client_pattern == pattern)
                    {
                        endpoint.mappings.push(mapping(pattern));
                    }
                }
                let mut model_priorities = BTreeMap::new();
                for override_ in &binding.overrides {
                    if let Some(priority) = override_.priority {
                        model_priorities.insert(model_name::clean(&override_.model), priority);
                    }
                    if let Some(upstream) = &override_.upstream_model {
                        let mut exact: Vec<_> = endpoint
                            .mappings
                            .iter()
                            .filter(|m| {
                                model_name::pattern_matches(&m.client_pattern, &override_.model)
                            })
                            .cloned()
                            .collect();
                        // Exact mappings must precede inherited wildcard mappings.
                        exact.sort_by_key(|m| {
                            std::cmp::Reverse(mapping_specificity(&m.client_pattern))
                        });
                        for m in &mut exact {
                            m.client_pattern = model_name::clean(&override_.model);
                            m.upstream_model = model_name::clean(upstream);
                        }
                        exact.extend(endpoint.mappings);
                        endpoint.mappings = exact;
                    }
                }
                output.push(RoutingEndpoint {
                    endpoint,
                    group_id: Some(group.id.clone()),
                    group_name: Some(group.name.clone()),
                    group_rank,
                    model_priorities,
                });
            }
        }
        output
    }

    /// Build a lossless initial group, including legacy wildcard declarations.
    /// Existing feature rules and retry settings are deliberately left untouched.
    pub fn migrate_model_groups(&mut self) {
        if self.model_groups.is_some() {
            return;
        }
        let mut models = Vec::new();
        let mut bindings = Vec::new();
        for endpoint in &self.endpoints {
            let mut supported = Vec::new();
            for m in &endpoint.mappings {
                let name = model_name::clean(&m.client_pattern);
                if name.is_empty() {
                    continue;
                }
                if !models.contains(&name) {
                    models.push(name.clone());
                }
                if !supported.contains(&name) {
                    supported.push(name);
                }
            }
            bindings.push(ModelGroupBinding {
                endpoint_id: endpoint.id.clone(),
                enabled: true,
                priority: endpoint.priority,
                models: Some(supported),
                overrides: vec![],
            });
        }
        self.model_groups = Some(if self.endpoints.is_empty() {
            vec![]
        } else {
            vec![ModelGroup {
                id: "default".into(),
                name: "默认模型组".into(),
                enabled: true,
                priority: 0,
                models,
                bindings,
            }]
        });
    }

    /// 默认组(id="default")是迁移时对入口库的一次性快照,不应该成为第二份
    /// 需要单独维护的顺序表:每次归一化时把它的绑定重排为 endpoints 数组序,
    /// 绑定优先级改写为对应入口的 `priority`,并剔除引用已删除入口的悬空绑定。
    /// 刻意排除在默认组之外的入口(没有绑定)不会被补回——排除是用户的显式选择;
    /// 新入口的默认组绑定由添加入口的 UI 流程负责(见两端 addProviderAccount)。
    /// 非默认组的绑定顺序与优先级保持独立,由模型组编辑器维护。
    pub fn sync_default_group_bindings(&mut self) {
        let Some(groups) = &mut self.model_groups else {
            return;
        };
        for group in groups.iter_mut() {
            if group.id != "default" {
                continue;
            }
            let mut by_id: std::collections::HashMap<String, ModelGroupBinding> =
                std::mem::take(&mut group.bindings)
                    .into_iter()
                    .map(|binding| (binding.endpoint_id.clone(), binding))
                    .collect();
            group.bindings = self
                .endpoints
                .iter()
                .filter_map(|endpoint| {
                    by_id.remove(&endpoint.id).map(|mut binding| {
                        binding.priority = endpoint.priority;
                        binding
                    })
                })
                .collect();
        }
    }

    pub fn validate_model_groups(&self) -> Result<(), String> {
        let mut ids = HashSet::new();
        for group in self.model_groups.iter().flatten() {
            if group.id.trim().is_empty() || !ids.insert(&group.id) {
                return Err("模型组 ID 不能为空或重复".into());
            }
            if group.priority < 0 {
                return Err("模型组优先级不能为负数".into());
            }
            let mut models = HashSet::new();
            for model in &group.models {
                let clean = model_name::clean(model);
                if clean.is_empty()
                    || clean.contains('*')
                        && (!clean.ends_with('*') || clean.matches('*').count() != 1)
                    || !models.insert(clean)
                {
                    return Err(format!("模型组 {} 含空、重复或无效模型模式", group.name));
                }
            }
            let mut endpoints = HashSet::new();
            for binding in &group.bindings {
                if self.endpoint(&binding.endpoint_id).is_none() {
                    return Err(format!("模型组 {} 引用了不存在的入口", group.name));
                }
                if !endpoints.insert(&binding.endpoint_id) {
                    return Err(format!("模型组 {} 重复引用了同一入口", group.name));
                }
                if binding.priority < 0 {
                    return Err("入口优先级不能为负数".into());
                }
                if let Some(selected) = &binding.models {
                    let mut seen = HashSet::new();
                    for model in selected {
                        let clean = model_name::clean(model);
                        if clean.is_empty()
                            || (clean.contains('*')
                                && (!clean.ends_with('*') || clean.matches('*').count() != 1))
                            || !seen.insert(clean)
                            || !group
                                .models
                                .iter()
                                .any(|g| model_name::pattern_matches(g, model))
                        {
                            return Err(format!(
                                "模型组 {} 的入口选择了组外或重复模型",
                                group.name
                            ));
                        }
                    }
                }
                let mut overrides = HashSet::new();
                for item in &binding.overrides {
                    let clean = model_name::clean(&item.model);
                    if clean.is_empty()
                        || item.model.contains('*')
                        || !overrides.insert(clean)
                        || !group
                            .models
                            .iter()
                            .any(|g| model_name::pattern_matches(g, &item.model))
                        || binding.models.as_ref().is_some_and(|models| {
                            !models
                                .iter()
                                .any(|m| model_name::pattern_matches(m, &item.model))
                        })
                        || item.priority.is_some_and(|p| p < 0)
                    {
                        return Err(format!("模型组 {} 的模型覆盖无效", group.name));
                    }
                }
            }
        }
        Ok(())
    }
}
