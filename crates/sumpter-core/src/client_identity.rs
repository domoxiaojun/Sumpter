//! Server-owned identity configuration; no script installation or credential parsing.
use crate::client_registry as registry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientBinding {
    pub client: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// Only this authenticated token may supply forwarded identity labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_source: Option<String>,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClientIdentitySettings {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub token_bindings: BTreeMap<String, ClientBinding>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub reporting_aliases: BTreeMap<String, String>,
    /// Exact UA product aliases, never substring or regex matches.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ua_aliases: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub relay_endpoints: BTreeMap<String, String>,
}
impl ClientIdentitySettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.token_bindings.len() > 129
            || self.reporting_aliases.len() > 128
            || self.ua_aliases.len() > 128
        {
            return Err("客户端身份规则超过上限".into());
        }
        if self.relay_endpoints.len() > 128
            || self.relay_endpoints.iter().any(|(id, source)| {
                id.is_empty() || id.len() > 128 || !registry::valid_product(source)
            })
        {
            return Err("中继入口配置无效".into());
        }
        for (id, binding) in &self.token_bindings {
            if (id.is_empty()
                || id.len() > 128
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')))
                || !registry::valid_product(&binding.client)
            {
                return Err("Token ID 或客户端产品标识无效".into());
            }
            if binding
                .variant
                .as_deref()
                .is_some_and(|v| !registry::VARIANTS.contains(&v))
            {
                return Err("未知客户端入口形态".into());
            }
            if binding
                .relay_source
                .as_deref()
                .is_some_and(|v| !registry::valid_product(v))
            {
                return Err("中继来源必须是有效产品标识".into());
            }
        }
        for (product, target) in self.reporting_aliases.iter().chain(self.ua_aliases.iter()) {
            if !registry::valid_product(product)
                || !registry::CATALOG.iter().any(|d| d.id == target)
            {
                return Err("客户端映射必须从有效产品标识指向已知客户端".into());
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedIdentity {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}
