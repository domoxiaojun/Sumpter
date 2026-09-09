//! Failure implementation for the shared engine.

use std::time::Duration;

use sumpter_core::events::{
    RuntimeEvent, RuntimeEventOutcome, RuntimeFailureKind, RuntimeFailurePhase,
};
use sumpter_core::stream_terminal::SseTerminal;

use crate::outbound::TransportError;

#[derive(Debug, thiserror::Error)]
pub(super) enum StreamReadError {
    #[error("stream idle timeout")]
    IdleTimeout(Duration),
    #[error("{0}")]
    Upstream(TransportError),
    #[error("missing protocol terminal event")]
    MissingTerminal,
}

#[derive(Clone)]
pub(super) struct FailureInfo {
    pub(super) kind: RuntimeFailureKind,
    pub(super) phase: RuntimeFailurePhase,
    pub(super) detail: Option<String>,
    pub(super) timeout_ms: Option<i64>,
    pub(super) upstream_status_code: Option<i64>,
    pub(super) upstream_request_id: Option<String>,
    pub(super) retry_after_seconds: Option<f64>,
}

impl FailureInfo {
    pub(super) fn from_transport(error: &TransportError, response_timeout: Option<f64>) -> Self {
        let (kind, detail, timeout_ms) = match error {
            TransportError::Timeout => (
                RuntimeFailureKind::ResponseTimeout,
                Some(
                    "upstream response headers were not received before the effective deadline"
                        .into(),
                ),
                response_timeout.map(timeout_ms),
            ),
            TransportError::ConnectionFailed(detail) => (
                RuntimeFailureKind::ConnectionFailed,
                Some(detail.clone()),
                None,
            ),
            TransportError::InvalidResponse(detail) => (
                RuntimeFailureKind::InvalidResponse,
                Some(detail.clone()),
                None,
            ),
        };
        Self {
            kind,
            phase: RuntimeFailurePhase::BeforeResponse,
            detail,
            timeout_ms,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    pub(super) fn upstream_http(status: u16, request_id: Option<String>) -> Self {
        Self {
            kind: RuntimeFailureKind::UpstreamHttpStatus,
            phase: RuntimeFailurePhase::ResponseHeaders,
            detail: Some(format!("upstream returned HTTP {status}")),
            timeout_ms: None,
            upstream_status_code: Some(status as i64),
            upstream_request_id: request_id,
            retry_after_seconds: None,
        }
    }

    pub(super) fn endpoints_exhausted() -> Self {
        Self {
            kind: RuntimeFailureKind::EndpointsExhausted,
            phase: RuntimeFailurePhase::BeforeResponse,
            detail: Some("no eligible upstream endpoint produced a response".into()),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    pub(super) fn from_stream(error: &StreamReadError) -> Self {
        match error {
            StreamReadError::IdleTimeout(deadline) => Self {
                kind: RuntimeFailureKind::StreamIdleTimeout,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(
                    "upstream response stream exceeded the configured idle deadline".into(),
                ),
                timeout_ms: Some(deadline.as_millis().min(i64::MAX as u128) as i64),
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
            StreamReadError::Upstream(error) => Self {
                kind: RuntimeFailureKind::StreamInterrupted,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(error.to_string()),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
            StreamReadError::MissingTerminal => Self {
                kind: RuntimeFailureKind::StreamInterrupted,
                phase: RuntimeFailurePhase::ResponseStream,
                detail: Some(
                    "upstream response stream ended before a protocol terminal event".into(),
                ),
                timeout_ms: None,
                upstream_status_code: None,
                upstream_request_id: None,
                retry_after_seconds: None,
            },
        }
    }

    pub(super) fn from_protocol_terminal(terminal: SseTerminal) -> Option<Self> {
        let (kind, detail) = match terminal {
            SseTerminal::Completed => return None,
            SseTerminal::Incomplete { detail } => {
                (RuntimeFailureKind::UpstreamResponseIncomplete, detail)
            }
            SseTerminal::Failed { detail } => (RuntimeFailureKind::UpstreamResponseFailed, detail),
        };
        Some(Self {
            kind,
            phase: RuntimeFailurePhase::ResponseStream,
            detail: Some(detail),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        })
    }

    pub(super) fn client_cancelled(response_started: bool) -> Self {
        Self {
            kind: RuntimeFailureKind::ClientCancelled,
            phase: if response_started {
                RuntimeFailurePhase::ResponseStream
            } else {
                RuntimeFailurePhase::BeforeResponse
            },
            detail: Some("client disconnected or cancelled the request".into()),
            timeout_ms: None,
            upstream_status_code: None,
            upstream_request_id: None,
            retry_after_seconds: None,
        }
    }

    pub(super) fn apply_to(&self, event: &mut RuntimeEvent) {
        event.outcome = Some(RuntimeEventOutcome::Failed);
        event.failure_kind = Some(self.kind);
        event.failure_phase = Some(self.phase);
        event.failure_detail = self.detail.clone();
        event.timeout_ms = self.timeout_ms;
        event.upstream_status_code = self.upstream_status_code;
        event.upstream_request_id = self.upstream_request_id.clone();
    }
}

fn timeout_ms(seconds: f64) -> i64 {
    Duration::from_secs_f64(seconds)
        .as_millis()
        .min(i64::MAX as u128) as i64
}
