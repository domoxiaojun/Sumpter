//! Cache-read facts derived from upstream evidence, independently of HTTP/outcome.
use serde::{Deserialize, Serialize};

use crate::events::{KIND_NOTIFY, RuntimeEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheReadState {
    Hit,
    Miss,
    Pending,
    Unknown,
    NotApplicable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheReadFinality {
    Confirmed,
    Provisional,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheReadReason {
    Unreported,
    NotObserved,
    UnsupportedTransport,
    UnknownApplicability,
    ObservationTruncated,
    InvalidValue,
    ConflictingEvidence,
    InsufficientEvidence,
}

/// Bounded field-quality evidence; the number itself remains in ResponseUsage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReadEvidence {
    pub finality: CacheReadFinality,
    pub observed: bool,
    pub complete: bool,
    pub truncated: bool,
    pub issue: Option<CacheReadReason>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReadSummary {
    pub state: CacheReadState,
    pub read_tokens: Option<u64>,
    pub finality: CacheReadFinality,
    pub reason: Option<CacheReadReason>,
}

/// Confirmed request-level facts; tokens and HTTP success use other denominators.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheReadStatistics {
    pub hit_requests: i64,
    pub miss_requests: i64,
    pub unknown_requests: i64,
    pub pending_requests: i64,
    pub not_applicable_requests: i64,
    pub applicability_unknown_requests: i64,
    pub confirmed_hit_rate: Option<f64>,
    pub confirmation_coverage: Option<f64>,
}

impl CacheReadStatistics {
    pub fn add(&mut self, state: CacheReadState, reason: Option<CacheReadReason>, count: i64) {
        let counter = match state {
            CacheReadState::Hit => &mut self.hit_requests,
            CacheReadState::Miss => &mut self.miss_requests,
            CacheReadState::Pending => &mut self.pending_requests,
            CacheReadState::NotApplicable => &mut self.not_applicable_requests,
            CacheReadState::Unknown
                if reason == Some(CacheReadReason::UnknownApplicability) || reason.is_none() =>
            {
                &mut self.applicability_unknown_requests
            }
            CacheReadState::Unknown => &mut self.unknown_requests,
        };
        *counter = counter.saturating_add(count.max(0));
        let confirmed = self.hit_requests.saturating_add(self.miss_requests);
        let eligible = confirmed.saturating_add(self.unknown_requests);
        self.confirmed_hit_rate =
            (confirmed > 0).then(|| self.hit_requests as f64 / confirmed as f64);
        self.confirmation_coverage = (eligible > 0).then(|| confirmed as f64 / eligible as f64);
    }
}

impl RuntimeEvent {
    /// Refresh the API projection at the event boundary, never trust a stored
    /// boolean or infer a cache miss from HTTP success / a missing usage field.
    pub fn refresh_cache_read(&mut self) {
        if self.client_variant.is_none() {
            self.client_variant = Some(self.derived_client_variant().to_owned());
        }
        // Preserve raw metadata separately; these existing top-level fields are
        // the common projection used by pagination, detail and SSE consumers.
        self.agent_role = Some(self.derived_agent_role().to_owned());
        if let Some(metadata) = &self.codex_metadata {
            if self.session_id.is_none() && metadata.session_id.is_some() {
                self.session_id = metadata.session_id.clone();
                self.session_source = Some("codex_session".into());
            }
            self.agent_name = self
                .agent_name
                .clone()
                .or_else(|| metadata.agent_name.clone());
            self.parent_thread_id = self
                .parent_thread_id
                .clone()
                .or_else(|| metadata.parent_thread_id.clone());
            self.parent_turn_id = self
                .parent_turn_id
                .clone()
                .or_else(|| metadata.parent_turn_id.clone());
            self.root_turn_id = self
                .root_turn_id
                .clone()
                .or_else(|| metadata.root_turn_id.clone());
        }
        self.cache_read = Some(self.observed_cache_read());
    }

    pub fn observed_cache_read(&self) -> CacheReadSummary {
        use CacheReadFinality::{Confirmed, Provisional, Unknown as Uncertain};
        use CacheReadReason::*;
        use CacheReadState::*;
        let unknown = |reason| CacheReadSummary {
            state: Unknown,
            read_tokens: None,
            finality: Uncertain,
            reason: Some(reason),
        };
        let intent = self.route_intent.as_deref().unwrap_or_default();
        if self.kind == KIND_NOTIFY
            || matches!(
                intent,
                "files" | "models" | "models-detail" | "claude-count-tokens"
            )
        {
            return CacheReadSummary {
                state: NotApplicable,
                read_tokens: None,
                finality: Uncertain,
                reason: None,
            };
        }
        let trace = self.stream_trace.as_ref();
        let evidence = trace.and_then(|trace| trace.cache_read_evidence.as_ref());
        if let Some(issue) = evidence.and_then(|e| e.issue) {
            return unknown(issue);
        }
        let read_tokens = trace
            .and_then(|t| t.usage.as_ref())
            .and_then(|u| u.cache_read_input_tokens);
        if read_tokens.is_some_and(|tokens| tokens > i64::MAX as u64) {
            return unknown(InvalidValue);
        }
        let finality = evidence.map(|e| e.finality).unwrap_or(Uncertain);
        if let Some(tokens) = read_tokens.filter(|&tokens| tokens > 0) {
            return CacheReadSummary {
                state: Hit,
                read_tokens: Some(tokens),
                finality: if finality == Confirmed {
                    Confirmed
                } else {
                    Provisional
                },
                reason: None,
            };
        }
        if read_tokens == Some(0) && finality == Confirmed {
            return CacheReadSummary {
                state: Miss,
                read_tokens: Some(0),
                finality: Confirmed,
                reason: None,
            };
        }
        if trace.is_some_and(|t| t.websocket_trace.is_some())
            || matches!(intent, "live" | "realtime")
        {
            return unknown(UnsupportedTransport);
        }
        if evidence.is_some_and(|e| e.truncated) {
            return unknown(ObservationTruncated);
        }
        // A recognized observer is also evidence of applicability when older
        // producers have no routeIntent. Unrecognized routes stay unknown.
        let supported = matches!(
            intent,
            "messages"
                | "anthropic"
                | "chat"
                | "responses"
                | "responses-compact"
                | "gemini-generate"
        ) || self
            .request_path
            .as_deref()
            .is_some_and(|path| path.ends_with("/messages"))
            || evidence.is_some_and(|e| e.observed);
        if self.is_in_flight() && supported {
            return CacheReadSummary {
                state: Pending,
                read_tokens,
                finality,
                reason: None,
            };
        }
        let reason = if evidence.is_some_and(|e| e.complete) && read_tokens.is_none() {
            Unreported
        } else if read_tokens.is_some() || evidence.is_some_and(|e| e.observed) {
            InsufficientEvidence
        } else if supported {
            NotObserved
        } else {
            UnknownApplicability
        };
        unknown(reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn event(tokens: Option<u64>, finality: &str) -> RuntimeEvent {
        serde_json::from_value(json!({
            "id":"cache-test", "kind":"client", "phase":"completed", "statusCode":200,
            "routeIntent":"responses", "streamTrace":{
                "usage":{"cacheReadInputTokens":tokens},
                "cacheReadEvidence":{"finality":finality,"observed":true,"complete":true,"truncated":false,"issue":null}
            }
        })).unwrap()
    }

    #[test]
    fn cache_fact_is_independent_of_http_and_final_result() {
        for outcome in ["failed", "cancelled", "succeeded"] {
            let mut value = event(Some(1280), "confirmed");
            value.outcome = serde_json::from_value(json!(outcome)).unwrap();
            assert_eq!(value.observed_cache_read().state, CacheReadState::Hit);
        }
        assert_eq!(
            event(None, "unknown").observed_cache_read().state,
            CacheReadState::Unknown
        );
    }

    #[test]
    fn deserialized_cache_count_must_fit_the_database_projection() {
        let value = event(Some(i64::MAX as u64 + 1), "confirmed");
        assert_eq!(value.observed_cache_read().state, CacheReadState::Unknown);
        assert_eq!(
            value.observed_cache_read().reason,
            Some(CacheReadReason::InvalidValue)
        );
        assert_eq!(value.observed_cache_read().read_tokens, None);
    }

    #[test]
    fn zero_requires_field_authority_not_request_completion() {
        let mut value = event(Some(0), "provisional");
        assert_eq!(value.observed_cache_read().state, CacheReadState::Unknown);
        value.phase = Some(crate::events::RuntimeEventPhase::InFlight);
        assert_eq!(value.observed_cache_read().state, CacheReadState::Pending);
        value
            .stream_trace
            .as_mut()
            .unwrap()
            .cache_read_evidence
            .as_mut()
            .unwrap()
            .finality = CacheReadFinality::Confirmed;
        assert_eq!(value.observed_cache_read().state, CacheReadState::Miss);
    }

    #[test]
    fn missing_write_only_truncated_and_conflicting_evidence_are_distinct() {
        let mut value = event(None, "unknown");
        value
            .stream_trace
            .as_mut()
            .unwrap()
            .usage
            .as_mut()
            .unwrap()
            .cache_creation_input_tokens = Some(100);
        assert_eq!(
            value.observed_cache_read().reason,
            Some(CacheReadReason::Unreported)
        );
        value
            .stream_trace
            .as_mut()
            .unwrap()
            .cache_read_evidence
            .as_mut()
            .unwrap()
            .truncated = true;
        assert_eq!(
            value.observed_cache_read().reason,
            Some(CacheReadReason::ObservationTruncated)
        );
        value
            .stream_trace
            .as_mut()
            .unwrap()
            .usage
            .as_mut()
            .unwrap()
            .cache_read_input_tokens = Some(10);
        assert_eq!(value.observed_cache_read().state, CacheReadState::Hit);
        value
            .stream_trace
            .as_mut()
            .unwrap()
            .cache_read_evidence
            .as_mut()
            .unwrap()
            .issue = Some(CacheReadReason::ConflictingEvidence);
        assert_eq!(value.observed_cache_read().state, CacheReadState::Unknown);
    }

    #[test]
    fn unsupported_and_non_inference_are_not_misses() {
        let mut value = event(None, "unknown");
        value.route_intent = Some("realtime".into());
        assert_eq!(
            value.observed_cache_read().reason,
            Some(CacheReadReason::UnsupportedTransport)
        );
        value.route_intent = Some("files".into());
        assert_eq!(
            value.observed_cache_read().state,
            CacheReadState::NotApplicable
        );
        value.kind = KIND_NOTIFY.into();
        assert_eq!(
            value.observed_cache_read().state,
            CacheReadState::NotApplicable
        );
    }
}
