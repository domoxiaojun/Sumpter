//! Generation speed: output tokens divided by the post-upstream-TTFB window.
//!
//! The value is derived when a response is built. It is not stored in
//! `payload_json`. Client rows use the final upstream attempt timing.
//! Aggregates sum eligible tokens and generation milliseconds
//! separately, then divide once. They are not an average of per-request rates.

use serde_json::{Value, json};

use sumpter_core::events::RuntimeEvent;
use sumpter_core::routing::RequestPurpose;

/// SQL predicate matching [`output_generation_sample`].
pub(crate) const OUTPUT_SPEED_PREDICATE: &str = "\
kind='client' AND is_in_flight=0 AND outcome='succeeded' \
AND (request_purpose IS NULL OR request_purpose!='token_count') \
AND output_tokens IS NOT NULL AND output_tokens>=0 \
AND upstream_duration_ms IS NOT NULL AND upstream_duration_ms>=0 \
AND upstream_ttfb_ms IS NOT NULL AND upstream_ttfb_ms>=0 \
AND upstream_duration_ms>upstream_ttfb_ms";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct OutputGenerationSample {
    pub(crate) output_tokens: i64,
    pub(crate) generation_ms: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct OutputSpeedTotals {
    pub(crate) tokens: i64,
    pub(crate) generation_ms: i64,
    pub(crate) requests: i64,
}

impl OutputSpeedTotals {
    pub(crate) fn observe(&mut self, sample: Option<OutputGenerationSample>) {
        let Some(sample) = sample else {
            return;
        };
        self.tokens = self.tokens.saturating_add(sample.output_tokens);
        self.generation_ms = self.generation_ms.saturating_add(sample.generation_ms);
        self.requests = self.requests.saturating_add(1);
    }

    pub(crate) fn rate(self) -> Option<f64> {
        output_speed_rate(self.tokens, self.generation_ms)
    }
}

pub(crate) fn output_generation_sample(
    kind: &str,
    in_flight: bool,
    request_purpose: Option<&str>,
    output_tokens: Option<i64>,
    duration_ms: Option<i64>,
    ttfb_ms: Option<i64>,
) -> Option<OutputGenerationSample> {
    if in_flight || kind != "client" || request_purpose == Some("token_count") {
        return None;
    }
    let output_tokens = output_tokens.filter(|value| *value >= 0)?;
    let duration_ms = duration_ms.filter(|value| *value >= 0)?;
    let ttfb_ms = ttfb_ms.filter(|value| *value >= 0)?;
    let generation_ms = duration_ms
        .checked_sub(ttfb_ms)
        .filter(|value| *value > 0)?;
    Some(OutputGenerationSample {
        output_tokens,
        generation_ms,
    })
}

pub(crate) fn output_speed_rate(tokens: i64, generation_ms: i64) -> Option<f64> {
    if tokens < 0 || generation_ms <= 0 {
        return None;
    }
    let rate = tokens as f64 / (generation_ms as f64 / 1000.0);
    rate.is_finite().then_some(rate)
}

pub(crate) fn output_tokens_per_second(
    kind: &str,
    in_flight: bool,
    request_purpose: Option<&str>,
    output_tokens: Option<i64>,
    duration_ms: Option<i64>,
    ttfb_ms: Option<i64>,
) -> Option<f64> {
    output_generation_sample(
        kind,
        in_flight,
        request_purpose,
        output_tokens,
        duration_ms,
        ttfb_ms,
    )
    .and_then(|sample| output_speed_rate(sample.output_tokens, sample.generation_ms))
}

pub(crate) fn output_tokens_per_second_for_event(event: &RuntimeEvent) -> Option<f64> {
    if !event.is_succeeded() {
        return None;
    }
    let purpose = match event.request_purpose {
        Some(RequestPurpose::TokenCount) => Some("token_count"),
        Some(_) | None => None,
    };
    let output_tokens = event
        .stream_trace
        .as_ref()
        .and_then(|trace| trace.usage.as_ref())
        .and_then(|usage| usage.output_tokens)
        .and_then(|value| i64::try_from(value).ok());
    output_tokens_per_second(
        event.kind.as_str(),
        event.is_in_flight(),
        purpose,
        output_tokens,
        event.upstream_duration_ms,
        event.upstream_ttfb_ms,
    )
}

/// Attach the computed field onto a runtime change or bare event object.
/// Missing speed stays absent so older clients and stored payloads are unchanged.
pub fn decorate_runtime_change_value(value: &mut Value) {
    if value.get("event").is_some() {
        let speed = value
            .get("event")
            .and_then(output_tokens_per_second_from_json);
        if let Some(speed) = speed {
            if let Some(event) = value.get_mut("event").and_then(Value::as_object_mut) {
                event.insert("outputTokensPerSecond".to_owned(), json!(speed));
            }
        }
        return;
    }
    if let Some(speed) = output_tokens_per_second_from_json(value) {
        if let Some(event) = value.as_object_mut() {
            event.insert("outputTokensPerSecond".to_owned(), json!(speed));
        }
    }
}

fn output_tokens_per_second_from_json(event: &Value) -> Option<f64> {
    if event.get("outcome").and_then(Value::as_str) != Some("succeeded") {
        return None;
    }
    output_tokens_per_second(
        event.get("kind").and_then(Value::as_str).unwrap_or(""),
        event.get("phase").and_then(Value::as_str) == Some("inFlight"),
        event.get("requestPurpose").and_then(Value::as_str),
        json_i64(
            event
                .get("streamTrace")
                .and_then(|trace| trace.get("usage"))
                .and_then(|usage| usage.get("outputTokens")),
        ),
        json_i64(event.get("upstreamDurationMS")),
        json_i64(event.get("upstreamTTFBMS")),
    )
}

fn json_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_speed_covers_the_eligibility_matrix() {
        assert_eq!(
            output_tokens_per_second("client", false, None, Some(100), Some(2_100), Some(100)),
            Some(50.0)
        );
        assert_eq!(
            output_tokens_per_second(
                "client",
                false,
                Some("standard"),
                Some(0),
                Some(2_100),
                Some(100)
            ),
            Some(0.0)
        );
        assert_eq!(
            output_tokens_per_second("client", false, None, None, Some(2_100), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second("client", false, None, Some(10), Some(2_100), None),
            None
        );
        assert_eq!(
            output_tokens_per_second("client", false, None, Some(10), Some(100), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second("client", false, None, Some(10), Some(50), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second("client", false, None, Some(-1), Some(2_100), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second("client", true, None, Some(10), Some(2_100), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second("upstream", false, None, Some(10), Some(2_100), Some(100)),
            None
        );
        assert_eq!(
            output_tokens_per_second(
                "client",
                false,
                Some("token_count"),
                Some(10),
                Some(2_100),
                Some(100)
            ),
            None
        );
    }

    #[test]
    fn aggregate_speed_is_token_weighted_not_the_mean_of_rates() {
        let mut totals = OutputSpeedTotals::default();
        totals.observe(output_generation_sample(
            "client",
            false,
            None,
            Some(100),
            Some(1_100),
            Some(100),
        ));
        totals.observe(output_generation_sample(
            "client",
            false,
            None,
            Some(100),
            Some(9_100),
            Some(100),
        ));
        totals.observe(output_generation_sample(
            "client",
            false,
            Some("token_count"),
            Some(9_000),
            Some(2_000),
            Some(100),
        ));
        totals.observe(output_generation_sample(
            "upstream",
            false,
            None,
            Some(9_000),
            Some(2_000),
            Some(100),
        ));
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.tokens, 200);
        assert_eq!(totals.generation_ms, 10_000);
        let weighted = totals.rate().unwrap();
        let arithmetic_mean = (100.0 + (100.0 / 9.0)) / 2.0;
        assert!((weighted - 20.0).abs() < 1e-9);
        assert!((weighted - arithmetic_mean).abs() > 1.0);
    }
}
