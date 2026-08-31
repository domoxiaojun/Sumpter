//! Runtime-event seam shared by the data plane and replay tooling.

pub use sumpter_core::events::{
    ClientDeclaredMetadata, ClientKind, CodexMetadata, RuntimeEvent, RuntimeEventOutcome,
    RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase, RuntimeSnapshot, StreamTrace,
};

/// Fields generated per request are intentionally excluded from conformance
/// comparisons.  Callers can use this projection before comparing old/new
/// observations while retaining the full event for normal runtime storage.
pub fn comparable_event(event: &RuntimeEvent) -> RuntimeEvent {
    let mut normalized = event.clone();
    normalized.id.clear();
    normalized.request_id = None;
    normalized.timestamp = 0.0;
    normalized.duration_ms = 0;
    normalized.ttfb_ms = None;
    // Stream timing is measured from the scheduler/clock and can differ by a
    // millisecond (or more) between two otherwise identical replays.  Keep
    // protocol facts such as frame count, byte count, terminal event, and
    // usage, but remove only those wall-clock projections from the comparison.
    if let Some(trace) = normalized.stream_trace.as_mut() {
        trace.max_chunk_gap_ms = None;
        trace.last_chunk_at_ms = None;
    }
    normalized
}
