//! Output speed estimated from observed incremental output before buffering.
//! Unknown historical spans are not inferred from response-header latency.
//! Aggregates divide eligible token totals by eligible observed spans once.

use serde_json::{Value, json};
use sumpter_core::events::RuntimeEvent;
use sumpter_core::routing::RequestPurpose;

/// Matches `output_generation_sample`; 100ms is the observation resolution.
pub(crate) const OUTPUT_SPEED_PREDICATE: &str = "\
kind='client' AND is_in_flight=0 AND outcome='succeeded' \
AND (request_purpose IS NULL OR request_purpose!='token_count') \
AND output_tokens IS NOT NULL AND output_tokens>=0 \
AND output_span_ms IS NOT NULL AND output_span_ms>=100";

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
    outcome: Option<&str>,
    in_flight: bool,
    request_purpose: Option<&str>,
    output_tokens: Option<i64>,
    output_span_ms: Option<i64>,
) -> Option<OutputGenerationSample> {
    if in_flight
        || kind != "client"
        || outcome != Some("succeeded")
        || request_purpose == Some("token_count")
    {
        return None;
    }
    Some(OutputGenerationSample {
        output_tokens: output_tokens.filter(|value| *value >= 0)?,
        generation_ms: output_span_ms.filter(|value| *value >= 100)?,
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
    outcome: Option<&str>,
    in_flight: bool,
    request_purpose: Option<&str>,
    output_tokens: Option<i64>,
    output_span_ms: Option<i64>,
) -> Option<f64> {
    output_generation_sample(
        kind,
        outcome,
        in_flight,
        request_purpose,
        output_tokens,
        output_span_ms,
    )
    .and_then(|sample| output_speed_rate(sample.output_tokens, sample.generation_ms))
}

pub(crate) fn output_tokens_per_second_for_event(event: &RuntimeEvent) -> Option<f64> {
    let purpose = match event.request_purpose {
        Some(RequestPurpose::TokenCount) => Some("token_count"),
        Some(_) | None => None,
    };
    let trace = event.stream_trace.as_ref()?;
    let output_tokens = trace
        .usage
        .as_ref()?
        .output_tokens
        .and_then(|value| i64::try_from(value).ok());
    output_tokens_per_second(
        event.kind.as_str(),
        event.is_succeeded().then_some("succeeded"),
        event.is_in_flight(),
        purpose,
        output_tokens,
        trace.output_span_ms,
    )
}

/// Explicit null clears a formerly valid speed on an updated live event.
/// Stored event payloads remain free of the derived rate.
pub fn decorate_runtime_change_value(value: &mut Value) {
    let event = if value.get("event").is_some() {
        value.get_mut("event").expect("checked event")
    } else {
        value
    };
    let speed = output_tokens_per_second_from_json(event);
    if let Some(event) = event.as_object_mut() {
        event.insert("outputTokensPerSecond".to_owned(), json!(speed));
    }
}

fn output_tokens_per_second_from_json(event: &Value) -> Option<f64> {
    output_tokens_per_second(
        event.get("kind").and_then(Value::as_str).unwrap_or(""),
        event.get("outcome").and_then(Value::as_str),
        event.get("phase").and_then(Value::as_str) == Some("inFlight"),
        event.get("requestPurpose").and_then(Value::as_str),
        event
            .pointer("/streamTrace/usage/outputTokens")
            .and_then(Value::as_i64),
        event
            .pointer("/streamTrace/outputSpanMS")
            .and_then(Value::as_i64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_speed_requires_success_and_a_measurable_span() {
        for (span, expected) in [
            (None, None),
            (Some(-1), None),
            (Some(0), None),
            (Some(5), None),
            (Some(7), None),
            (Some(12), None),
            (Some(99), None),
            (Some(100), Some(1000.0)),
            (Some(2000), Some(50.0)),
        ] {
            assert_eq!(
                output_tokens_per_second("client", Some("succeeded"), false, None, Some(100), span),
                expected
            );
        }
        for outcome in [None, Some("failed"), Some("cancelled")] {
            assert_eq!(
                output_tokens_per_second("client", outcome, false, None, Some(26), Some(1000)),
                None
            );
        }
        for (kind, in_flight, purpose, tokens) in [
            ("upstream", false, None, Some(100)),
            ("client", true, None, Some(100)),
            ("client", false, Some("token_count"), Some(100)),
            ("client", false, None, None),
            ("client", false, None, Some(-1)),
        ] {
            assert_eq!(
                output_tokens_per_second(
                    kind,
                    Some("succeeded"),
                    in_flight,
                    purpose,
                    tokens,
                    Some(1000)
                ),
                None
            );
        }
        assert_eq!(
            output_tokens_per_second(
                "client",
                Some("succeeded"),
                false,
                None,
                Some(0),
                Some(1000)
            ),
            Some(0.0)
        );
    }

    #[test]
    fn aggregate_speed_excludes_unknown_and_burst_samples() {
        let mut totals = OutputSpeedTotals::default();
        for (tokens, span) in [
            (100, Some(1000)),
            (100, Some(9000)),
            (490, Some(12)),
            (229, None),
        ] {
            totals.observe(output_generation_sample(
                "client",
                Some("succeeded"),
                false,
                None,
                Some(tokens),
                span,
            ));
        }
        assert_eq!(totals.requests, 2);
        assert_eq!(totals.tokens, 200);
        assert_eq!(totals.generation_ms, 10000);
        assert_eq!(totals.rate(), Some(20.0));
    }

    #[test]
    fn missing_or_invalid_observation_clears_stale_live_speed() {
        let mut event = json!({"kind":"client", "phase":"completed", "outcome":"succeeded",
            "outputTokensPerSecond": 40833.3, "upstreamTTFBMS":8800, "upstreamDurationMS":8812,
            "streamTrace":{"usage":{"outputTokens":490}}});
        decorate_runtime_change_value(&mut event);
        assert!(event["outputTokensPerSecond"].is_null());
        event["streamTrace"]["outputSpanMS"] = json!(1000);
        decorate_runtime_change_value(&mut event);
        assert_eq!(event["outputTokensPerSecond"], 490.0);
        event["outcome"] = json!("failed");
        decorate_runtime_change_value(&mut event);
        assert!(event["outputTokensPerSecond"].is_null());
    }
}
