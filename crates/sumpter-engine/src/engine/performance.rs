//! Opt-in stage timings, with no request content or identity fields.
use std::time::Instant;

pub(super) struct StageTimer {
    stage: &'static str,
    bytes: usize,
    started: Option<Instant>,
}

impl StageTimer {
    pub(super) fn new(stage: &'static str, bytes: usize) -> Self {
        Self {
            stage,
            bytes,
            started:
                tracing::enabled!(target: "sumpter_engine::request_perf", tracing::Level::DEBUG)
                    .then(Instant::now),
        }
    }

    pub(super) fn set_bytes(&mut self, bytes: usize) {
        self.bytes = bytes;
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            tracing::debug!(target: "sumpter_engine::request_perf",
                stage = self.stage, bytes = self.bytes,
                elapsed_us = started.elapsed().as_micros() as u64,
                "request stage");
        }
    }
}

pub(super) fn measure<T>(stage: &'static str, bytes: usize, work: impl FnOnce() -> T) -> T {
    let _timer = StageTimer::new(stage, bytes);
    work()
}
