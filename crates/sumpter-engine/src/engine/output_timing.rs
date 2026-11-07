//! Observe incremental output before any retry buffer or protocol bridge.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::{StreamExt, stream};
use sumpter_core::config::ProviderProtocol;
use sumpter_core::stream_terminal::{SseDialect, SseTerminalTracker};

use crate::outbound::UpstreamResponse;

pub(super) type ObservedOutputTiming = Arc<Mutex<OutputWindow>>;

#[derive(Default)]
pub(super) struct OutputWindow {
    first_ms: Option<i64>,
    last_ms: Option<i64>,
    output_events: u64,
    incomplete: bool,
}

impl OutputWindow {
    fn observe(&mut self, events: u64, at_ms: i64, incomplete: bool) {
        self.incomplete |= incomplete;
        if events > self.output_events {
            self.first_ms.get_or_insert(at_ms);
            self.last_ms = Some(at_ms);
            self.output_events = events;
        }
    }

    fn span_ms(&self) -> Option<i64> {
        if self.incomplete {
            return None;
        }
        // Conservative measurement resolution, not a maximum TPS cap. Many
        // output events received in one batch still have a zero-length span.
        self.last_ms?
            .checked_sub(self.first_ms?)
            .filter(|ms| *ms >= 100)
    }
}

pub(super) fn span_ms(timing: &ObservedOutputTiming) -> Option<i64> {
    timing.lock().ok()?.span_ms()
}

pub(super) fn observe(
    response: &mut UpstreamResponse,
    protocol: ProviderProtocol,
) -> Option<ObservedOutputTiming> {
    if response.status != 200
        || !response.headers.iter().any(|(name, value)| {
            name.eq_ignore_ascii_case("content-type")
                && value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        })
    {
        return None;
    }
    let dialect = match protocol {
        ProviderProtocol::Anthropic => SseDialect::Anthropic,
        ProviderProtocol::OpenAI => SseDialect::OpenAiChat,
        ProviderProtocol::OpenAIResponses => SseDialect::OpenAiResponses,
        ProviderProtocol::Gemini => SseDialect::Gemini,
    };
    let timing = Arc::new(Mutex::new(OutputWindow::default()));
    let shared = timing.clone();
    let started = Instant::now();
    let mut tracker = SseTerminalTracker::new(dialect);
    let upstream = std::mem::replace(&mut response.stream, stream::empty().boxed());
    response.stream = upstream
        .map(move |item| {
            if let Ok(chunk) = &item {
                let _ = tracker.push(chunk);
                if let Ok(mut window) = shared.lock() {
                    window.observe(
                        tracker.output_events(),
                        started.elapsed().as_millis().min(i64::MAX as u128) as i64,
                        tracker.observation_truncated(),
                    );
                }
            }
            item
        })
        .boxed();
    Some(timing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;

    #[tokio::test]
    async fn timing_survives_buffering_and_ignores_delayed_usage() {
        let delta = Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}\n\n",
        );
        let terminal = Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"output_tokens\":2}}}\n\n");
        let chunks = vec![
            (0, delta.clone()),
            (150, delta.clone()),
            (150, terminal.clone()),
        ];
        let mut response = UpstreamResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            stream: stream::iter(chunks)
                .then(|(delay, bytes)| async move {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                    Ok(bytes)
                })
                .boxed(),
        };
        let timing = observe(&mut response, ProviderProtocol::OpenAIResponses).unwrap();
        // Collect first, then replay: the downstream drain is instantaneous.
        let buffered = response.stream.collect::<Vec<_>>().await;
        let measured = span_ms(&timing).unwrap();
        assert!(measured >= 100);
        assert_eq!(buffered.len(), 3);
        assert_eq!(buffered[0].as_ref().unwrap(), &delta);
        assert_eq!(buffered[2].as_ref().unwrap(), &terminal);
        let _ = stream::iter(buffered).collect::<Vec<_>>().await;
        assert_eq!(span_ms(&timing), Some(measured));
        let window = timing.lock().unwrap();
        assert_eq!(window.output_events, 2);
    }

    #[test]
    fn only_separated_output_batches_form_a_measurable_window() {
        let mut window = OutputWindow::default();
        window.observe(0, 8000, false); // headers/heartbeat do not start it
        window.observe(12, 8800, false); // twelve events delivered together
        window.observe(12, 9000, false); // usage or EOF must not extend it
        assert_eq!(window.span_ms(), None);
        window.observe(13, 8812, false);
        assert_eq!(window.span_ms(), None);
        window.observe(14, 8899, false);
        assert_eq!(window.span_ms(), None);
        window.observe(15, 8900, false);
        assert_eq!(window.span_ms(), Some(100));
        window.observe(16, 9800, false);
        assert_eq!(window.span_ms(), Some(1000));
        window.observe(16, 12000, true);
        assert_eq!(window.span_ms(), None);
    }
}
