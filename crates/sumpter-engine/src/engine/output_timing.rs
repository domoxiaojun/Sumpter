//! Observe incremental output before any retry buffer or protocol bridge.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::{StreamExt, stream};
use sumpter_core::config::ProviderProtocol;
use sumpter_core::stream_terminal::{SseDialect, SseTerminalTracker};

use super::response_decode::ResponseDecoder;
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
    let tracker = SseTerminalTracker::new(dialect);
    let upstream = std::mem::replace(&mut response.stream, stream::empty().boxed());
    let decoder = ResponseDecoder::new(&response.headers);
    response.stream = stream::unfold(
        (upstream, decoder, tracker),
        move |(mut upstream, mut decoder, mut tracker)| {
            let shared = shared.clone();
            async move {
                let item = upstream.next().await;
                let at_ms = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
                match &item {
                    Some(Ok(chunk)) => decoder.feed(chunk, |data| {
                        let _ = tracker.push(data);
                    }),
                    None => decoder.finish(|data| {
                        let _ = tracker.push(data);
                    }),
                    Some(Err(_)) => {}
                }
                if let Ok(mut window) = shared.lock() {
                    window.observe(
                        tracker.output_events(),
                        at_ms,
                        tracker.observation_truncated()
                            || decoder.issue().is_some()
                            || matches!(item, Some(Err(_))),
                    );
                }
                item.map(|item| (item, (upstream, decoder, tracker)))
            }
        },
    )
    .boxed();
    Some(timing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::time::Duration;

    #[tokio::test]
    async fn gzip_timing_uses_output_arrival_not_delayed_usage_or_trailer() {
        use std::io::Write;
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut wire_offset = 0;
        let mut chunks = Vec::new();
        for (delay, frame) in [
            (
                0,
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"one\"}]}}]}\n\n",
            ),
            (
                150,
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"two\"}]},\"finishReason\":\"STOP\"}]}\n\n",
            ),
            (
                200,
                "data: {\"usageMetadata\":{\"candidatesTokenCount\":2}}\n\n",
            ),
        ] {
            encoder.write_all(frame.as_bytes()).unwrap();
            encoder.flush().unwrap();
            chunks.push((
                delay,
                Bytes::copy_from_slice(&encoder.get_ref()[wire_offset..]),
            ));
            wire_offset = encoder.get_ref().len();
        }
        let wire = encoder.finish().unwrap();
        chunks.push((200, Bytes::copy_from_slice(&wire[wire_offset..])));
        for corrupt in [false, true] {
            let mut case_chunks = chunks.clone();
            let mut expected_wire = wire.clone();
            if corrupt {
                let crc = expected_wire.len() - 8;
                expected_wire[crc] ^= 1;
                let last = &mut case_chunks.last_mut().unwrap().1;
                let mut tail = last.to_vec();
                let crc = tail.len() - 8;
                tail[crc] ^= 1;
                *last = Bytes::from(tail);
            }
            let mut response = UpstreamResponse {
                status: 200,
                headers: vec![
                    ("content-type".into(), "text/event-stream".into()),
                    ("content-encoding".into(), "gzip".into()),
                ],
                stream: stream::iter(case_chunks)
                    .then(|(delay, bytes)| async move {
                        tokio::time::sleep(Duration::from_millis(delay)).await;
                        Ok(bytes)
                    })
                    .boxed(),
            };
            let timing = observe(&mut response, ProviderProtocol::Gemini).unwrap();
            let mut returned = Vec::new();
            for _ in 0..2 {
                returned.extend_from_slice(&response.stream.next().await.unwrap().unwrap());
            }
            let before_usage = span_ms(&timing).expect("two separated output batches");
            while let Some(item) = response.stream.next().await {
                returned.extend_from_slice(&item.unwrap());
            }
            assert_eq!(returned, expected_wire);
            assert_eq!(
                span_ms(&timing),
                if corrupt { None } else { Some(before_usage) }
            );
        }
    }

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
