//! Bounded observations only: never rewrite, replay, or retain business payloads.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::{CodexMetadata, codex_request_kind};

pub const MAX_OBSERVED_REQUESTS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesRequestSummary {
    pub counts: BTreeMap<String, u64>,
    pub controls: BTreeMap<String, u64>,
    pub recent: Vec<ResponsesRequestObservation>,
    pub dropped: u64,
    pub unobserved_frames: u64,
    pub unassociated_events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsesRequestObservation {
    pub sequence: u64,
    pub kind: String,
    pub turn_id: Option<String>,
    pub response_id: Option<String>,
    pub generate: Option<bool>,
    pub terminal: Option<String>,
}

#[derive(Default)]
pub struct ResponsesRequestObserver {
    summary: ResponsesRequestSummary,
    sequence: u64,
    correlation_lost: bool,
    // Metadata of the most recent create, not of append/steer/interrupt.
    pub latest_metadata: Option<CodexMetadata>,
}

impl ResponsesRequestObserver {
    pub fn unobserved(&mut self) {
        self.summary.unobserved_frames += 1;
        self.latest_metadata = None;
        self.correlation_lost = true;
    }

    pub fn client(&mut self, value: &Value, metadata: Option<CodexMetadata>) {
        match value.get("type").and_then(Value::as_str) {
            Some("response.create") => {}
            Some(kind @ ("response.append" | "response.steer" | "response.interrupt")) => {
                *self.summary.controls.entry(kind.into()).or_default() += 1;
                return;
            }
            _ => return,
        }
        self.latest_metadata = metadata;
        let generate = value.get("generate").and_then(Value::as_bool);
        let declared = codex_request_kind(self.latest_metadata.as_ref());
        // generate=false explicitly denotes setup. Do not label a generating
        // request prewarm just because stale handshake metadata says so.
        let kind = if generate == Some(false) {
            "prewarm"
        } else if declared == "prewarm" && generate == Some(true) {
            "unknown"
        } else {
            declared
        };
        *self.summary.counts.entry(kind.into()).or_default() += 1;
        self.sequence += 1;
        if self.summary.recent.len() == MAX_OBSERVED_REQUESTS {
            if self.summary.recent[0].terminal.is_none() {
                self.correlation_lost = true;
            }
            self.summary.recent.remove(0);
            self.summary.dropped += 1;
        }
        self.summary.recent.push(ResponsesRequestObservation {
            sequence: self.sequence,
            kind: kind.into(),
            turn_id: self
                .latest_metadata
                .as_ref()
                .and_then(|m| m.turn_id.clone()),
            response_id: None,
            generate,
            terminal: None,
        });
    }

    pub fn upstream(&mut self, value: &Value) {
        let Some(kind) = value.get("type").and_then(Value::as_str) else {
            return;
        };
        if !matches!(
            kind,
            "response.created"
                | "response.completed"
                | "response.done"
                | "response.failed"
                | "response.incomplete"
                | "response.interrupted"
                | "error"
        ) {
            return;
        }
        let id = value
            .pointer("/response/id")
            .or_else(|| value.get("response_id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control));
        let matching = id.and_then(|id| {
            self.summary
                .recent
                .iter()
                .position(|r| r.response_id.as_deref() == Some(id))
        });
        let unbound = self
            .summary
            .recent
            .iter()
            .enumerate()
            .filter(|(_, r)| r.response_id.is_none() && r.terminal.is_none())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        // No FIFO guess for concurrent creates. IDs must match once assigned.
        let active = self
            .summary
            .recent
            .iter()
            .enumerate()
            .filter(|(_, r)| r.terminal.is_none())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let index = matching.or_else(|| {
            if self.correlation_lost {
                return None;
            }
            if kind == "response.created" {
                (unbound.len() == 1).then(|| unbound[0])
            } else {
                (id.is_none() && active.len() == 1).then(|| active[0])
            }
        });
        let Some(index) = index else {
            self.summary.unassociated_events += 1;
            return;
        };
        let record = &mut self.summary.recent[index];
        if record.terminal.is_some() {
            return;
        }
        if let Some(id) = id {
            record.response_id = Some(id.into());
        }
        if kind != "response.created" {
            record.terminal = Some(kind.into());
        }
    }

    pub fn summary(&self) -> ResponsesRequestSummary {
        self.summary.clone()
    }

    pub fn has_requests(&self) -> bool {
        self.sequence > 0
    }
}
