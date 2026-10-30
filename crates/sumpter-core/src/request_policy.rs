//! Optional request governance. Defaults preserve routing and retry selection.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProtection {
    #[default]
    Complete,
    Prelude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestLimits {
    pub max_concurrent: u32,
    pub max_queued: u32,
    pub queue_timeout_seconds: u32,
    pub requests_per_minute: u32,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_concurrent: 0,
            max_queued: 128,
            queue_timeout_seconds: 30,
            requests_per_minute: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompatibilityProfile {
    pub strict_tools: Option<bool>,
    pub grammar_tools: Option<bool>,
    pub developer_role: Option<bool>,
    pub structured_output: Option<bool>,
}

impl CompatibilityProfile {
    pub fn intersect(&mut self, other: &Self) {
        fn claim(a: Option<bool>, b: Option<bool>) -> Option<bool> {
            match (a, b) {
                (Some(false), _) | (_, Some(false)) => Some(false),
                (Some(true), Some(true)) => Some(true),
                _ => None,
            }
        }
        self.strict_tools = claim(self.strict_tools, other.strict_tools);
        self.grammar_tools = claim(self.grammar_tools, other.grammar_tools);
        self.developer_role = claim(self.developer_role, other.developer_role);
        self.structured_output = claim(self.structured_output, other.structured_output);
    }
    /// Explicit denials reject before sending; never silently rewrite native content.
    pub fn unsupported_field(&self, body: &Value) -> Option<&'static str> {
        let tools = body.get("tools").and_then(Value::as_array);
        if self.strict_tools == Some(false)
            && tools.is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("strict")
                        .or_else(|| tool.pointer("/function/strict"))
                        == Some(&Value::Bool(true))
                })
            })
        {
            return Some("tools.strict");
        }
        if self.grammar_tools == Some(false)
            && tools.is_some_and(|tools| {
                tools.iter().any(|tool| {
                    tool.get("type").and_then(Value::as_str) == Some("custom")
                        && (tool
                            .pointer("/format/type")
                            .or_else(|| tool.pointer("/custom/format/type")))
                        .and_then(Value::as_str)
                            == Some("grammar")
                })
            })
        {
            return Some("tools.grammar");
        }
        if self.developer_role == Some(false)
            && ["messages", "input"].iter().any(|key| {
                body.get(key)
                    .and_then(Value::as_array)
                    .is_some_and(|items| {
                        items.iter().any(|item| {
                            item.get("role").and_then(Value::as_str) == Some("developer")
                        })
                    })
            })
        {
            return Some("developer");
        }
        if self.structured_output == Some(false)
            && ([
                body.pointer("/response_format/type"),
                body.pointer("/text/format/type"),
            ]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|kind| matches!(kind, "json_object" | "json_schema"))
                || body
                    .pointer("/generationConfig/responseSchema")
                    .is_some_and(|v| !v.is_null())
                || body
                    .pointer("/generationConfig/responseJsonSchema")
                    .is_some_and(|v| !v.is_null())
                || body
                    .pointer("/generationConfig/responseMimeType")
                    .and_then(Value::as_str)
                    == Some("application/json"))
        {
            return Some("structured_output");
        }
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EndpointPolicy {
    pub limits: RequestLimits,
    /// Exact upstream model ids, or an explicitly configured '*' fallback.
    pub models: BTreeMap<String, CompatibilityProfile>,
    pub protocol_models: BTreeMap<String, BTreeMap<String, CompatibilityProfile>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestPolicy {
    pub max_total_attempts: u32,
    pub stream_protection: StreamProtection,
    pub stream_buffer_mib: u32,
    pub stream_buffer_chunks: u32,
    pub stream_buffer_seconds: u32,
    pub max_buffered_mib: u32,
    pub global: RequestLimits,
    pub endpoints: BTreeMap<String, EndpointPolicy>,
}

impl Default for RequestPolicy {
    fn default() -> Self {
        Self {
            max_total_attempts: 0,
            stream_protection: StreamProtection::Complete,
            stream_buffer_mib: 16,
            stream_buffer_chunks: 8192,
            stream_buffer_seconds: 0,
            max_buffered_mib: 128,
            global: RequestLimits::default(),
            endpoints: BTreeMap::new(),
        }
    }
}

impl RequestPolicy {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    pub fn normalize(&mut self) {
        self.max_total_attempts = self.max_total_attempts.min(10_000);
        self.stream_buffer_mib = self.stream_buffer_mib.clamp(1, 256);
        self.stream_buffer_chunks = self.stream_buffer_chunks.clamp(16, 65_536);
        self.stream_buffer_seconds = self.stream_buffer_seconds.min(3600);
        self.max_buffered_mib = self.max_buffered_mib.clamp(1, 4096);
        for limits in std::iter::once(&mut self.global)
            .chain(self.endpoints.values_mut().map(|p| &mut p.limits))
        {
            limits.max_concurrent = limits.max_concurrent.min(10_000);
            limits.max_queued = limits.max_queued.min(10_000);
            limits.queue_timeout_seconds = limits.queue_timeout_seconds.clamp(1, 300);
            limits.requests_per_minute = limits.requests_per_minute.min(1_000_000);
        }
    }

    pub fn profile(
        &self,
        endpoint: &str,
        model: &str,
        protocol: crate::config::ProviderProtocol,
    ) -> Option<&CompatibilityProfile> {
        let policy = self.endpoints.get(endpoint)?;
        let scoped = policy.protocol_models.get(protocol.token());
        scoped
            .and_then(|models| models.get(model).or_else(|| models.get("*")))
            .or_else(|| policy.models.get(model))
            .or_else(|| policy.models.get("*"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    RateLimit,
    QuotaExhausted,
    Credential,
    ModelUnsupported,
    Upstream,
    Request,
    Unknown,
}

#[derive(Debug, Clone, Copy)]
pub struct FailureDecision {
    pub class: FailureClass,
    pub retry_same: bool,
    pub retry_after_seconds: Option<f64>,
}

/// Bounded JSON supplied by the transport inspector. Unknown shapes keep status policy.
pub fn classify_failure(status: u16, body: &[u8]) -> FailureDecision {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = value.get("error").unwrap_or(&value);
    let codes = [error.get("code"), error.get("type"), error.get("status")];
    let terminal_credentials = codes
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|code| {
            matches!(
                code.to_ascii_lowercase().as_str(),
                "invalid_api_key" | "authentication_error" | "invalid_grant"
            )
        });
    let mut hard_quota = codes
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|code| {
            matches!(
                code.to_ascii_lowercase().as_str(),
                "insufficient_quota" | "quota_exhausted" | "billing_hard_limit_reached"
            )
        });
    let mut retry_after_seconds: Option<f64> = None;
    if let Some(details) = error.get("details").and_then(Value::as_array) {
        for detail in details.iter().take(32) {
            if let Some(reason) = detail.get("reason").and_then(Value::as_str) {
                hard_quota |= matches!(
                    reason,
                    "QUOTA_EXHAUSTED" | "INSUFFICIENT_G1_CREDITS_BALANCE"
                );
            }
            if let Some(violations) = detail.get("violations").and_then(Value::as_array) {
                hard_quota |= violations
                    .iter()
                    .take(32)
                    .filter_map(|v| v.get("quotaId").and_then(Value::as_str))
                    .any(|id| id.contains("PerDay") || id.contains("Daily"));
            }
            if let Some(delay) = detail.get("retryDelay").and_then(Value::as_str) {
                let parsed = delay
                    .strip_suffix("ms")
                    .and_then(|s| s.parse::<f64>().ok())
                    .map(|n| n / 1000.0)
                    .or_else(|| delay.strip_suffix('s').and_then(|s| s.parse::<f64>().ok()));
                if let Some(seconds) = parsed.filter(|s| s.is_finite() && *s >= 0.0) {
                    retry_after_seconds =
                        Some(retry_after_seconds.unwrap_or(0.0).max(seconds.min(300.0)));
                }
            }
        }
    }
    let class = if hard_quota && matches!(status, 402 | 403 | 429 | 499 | 503) {
        FailureClass::QuotaExhausted
    } else {
        match status {
            401 | 403 => FailureClass::Credential,
            402 => FailureClass::QuotaExhausted,
            404 => FailureClass::ModelUnsupported,
            429 => FailureClass::RateLimit,
            500..=599 => FailureClass::Upstream,
            400..=499 => FailureClass::Request,
            _ => FailureClass::Unknown,
        }
    };
    FailureDecision {
        class,
        retry_same: !(matches!(
            class,
            FailureClass::QuotaExhausted | FailureClass::ModelUnsupported
        ) || (terminal_credentials && class == FailureClass::Credential)),
        retry_after_seconds,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestTrace {
    pub steps: Vec<RequestTraceStep>,
    pub omitted: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RequestTraceStep {
    pub at_ms: u64,
    pub action: String,
    pub endpoint_id: Option<String>,
    pub detail: String,
    pub duration_ms: Option<u64>,
}

impl RequestTrace {
    pub fn push(&mut self, mut step: RequestTraceStep) {
        if self.steps.len() < 64 {
            fn bounded(value: &mut String, max: usize) {
                if let Some((end, _)) = value.char_indices().nth(max) {
                    value.truncate(end);
                }
            }
            bounded(&mut step.action, 48);
            bounded(&mut step.detail, 256);
            if let Some(endpoint) = &mut step.endpoint_id {
                bounded(endpoint, 160);
            }
            self.steps.push(step);
        } else {
            self.omitted = self.omitted.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distinguishes_daily_quota_from_temporary_throttle() {
        let hard = br#"{"error":{"details":[{"violations":[{"quotaId":"RequestsPerDay"}]}]}}"#;
        assert!(!classify_failure(429, hard).retry_same);
        let temporary =
            br#"{"error":{"details":[{"retryDelay":"250ms","reason":"RATE_LIMIT_EXCEEDED"}]}}"#;
        let decision = classify_failure(429, temporary);
        assert!(decision.retry_same);
        assert_eq!(decision.retry_after_seconds, Some(0.25));
        assert_eq!(
            classify_failure(429, b"unknown").class,
            FailureClass::RateLimit
        );
        assert_eq!(classify_failure(400, hard).class, FailureClass::Request);
    }
    #[test]
    fn explicit_denials_preserve_unknown_and_supported_native_requests() {
        let body = serde_json::json!({"tools":[{"type":"function","function":{"strict":true}}]});
        assert_eq!(
            CompatibilityProfile::default().unsupported_field(&body),
            None
        );
        let profile = CompatibilityProfile {
            strict_tools: Some(false),
            ..Default::default()
        };
        assert_eq!(profile.unsupported_field(&body), Some("tools.strict"));
    }

    #[test]
    fn capability_denials_only_match_the_actual_feature() {
        let profile = CompatibilityProfile {
            grammar_tools: Some(false),
            structured_output: Some(false),
            ..Default::default()
        };
        assert_eq!(profile.unsupported_field(&serde_json::json!({"tools":[{"type":"custom","format":{"type":"text"}}],"response_format":{"type":"text"}})), None);
        for body in [
            serde_json::json!({"tools":[{"type":"custom","format":{"type":"grammar"}}]}),
            serde_json::json!({"tools":[{"type":"custom","custom":{"format":{"type":"grammar"}}}]}),
        ] {
            assert_eq!(profile.unsupported_field(&body), Some("tools.grammar"));
        }
        for body in [
            serde_json::json!({"text":{"format":{"type":"json_schema"}}}),
            serde_json::json!({"generationConfig":{"responseMimeType":"application/json"}}),
        ] {
            assert_eq!(profile.unsupported_field(&body), Some("structured_output"));
        }
    }

    #[test]
    fn profiles_are_scoped_and_intersections_never_invent_support() {
        let mut policy = RequestPolicy::default();
        let endpoint = policy.endpoints.entry("a".into()).or_default();
        let yes = CompatibilityProfile {
            strict_tools: Some(true),
            ..Default::default()
        };
        let no = CompatibilityProfile {
            strict_tools: Some(false),
            ..Default::default()
        };
        endpoint.models.insert("*".into(), yes.clone());
        endpoint
            .protocol_models
            .entry("openai-responses".into())
            .or_default()
            .insert("m".into(), no.clone());
        assert_eq!(
            policy.profile("a", "m", crate::config::ProviderProtocol::OpenAIResponses),
            Some(&no)
        );
        assert_eq!(
            policy.profile("a", "m", crate::config::ProviderProtocol::OpenAI),
            Some(&yes)
        );
        assert!(
            policy
                .profile("b", "m", crate::config::ProviderProtocol::OpenAI)
                .is_none()
        );
        let mut combined = yes;
        combined.intersect(&CompatibilityProfile::default());
        assert_eq!(combined.strict_tools, None);
        combined.intersect(&no);
        assert_eq!(combined.strict_tools, Some(false));
    }

    #[test]
    fn unknown_credentials_keep_status_policy_and_trace_is_bounded() {
        assert!(classify_failure(401, b"unknown").retry_same);
        assert!(!classify_failure(401, br#"{"error":{"code":"invalid_api_key"}}"#).retry_same);
        let mut trace = RequestTrace::default();
        for _ in 0..100 {
            trace.push(RequestTraceStep::default());
        }
        assert_eq!(trace.steps.len(), 64);
        assert_eq!(trace.omitted, 36);
    }
}
