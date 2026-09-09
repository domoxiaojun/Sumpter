use serde_json::json;
use sumpter_core::cache_read::{
    CacheReadFinality, CacheReadReason, CacheReadState, CacheReadStatistics,
};
use sumpter_core::events::RuntimeEvent;
use sumpter_core::stream_terminal::{SseDialect, SseTerminalTracker};

fn summary(
    tracker: &SseTerminalTracker,
    phase: &str,
) -> sumpter_core::cache_read::CacheReadSummary {
    let event: RuntimeEvent = serde_json::from_value(json!({
        "id":"r", "kind":"client", "routeIntent":"responses", "phase":phase,
        "statusCode":200, "outcome":"failed", "streamTrace":{
            "usage":tracker.usage(), "cacheReadEvidence":tracker.cache_read_evidence()
        }
    }))
    .unwrap();
    event.observed_cache_read()
}

#[test]
fn anthropic_input_cache_is_known_before_output_completes() {
    for (tokens, expected) in [(0, CacheReadState::Miss), (1280, CacheReadState::Hit)] {
        let mut tracker = SseTerminalTracker::new(SseDialect::Anthropic);
        tracker.push(format!("data: {{\"type\":\"message_start\",\"message\":{{\"usage\":{{\"cache_read_input_tokens\":{tokens}}}}}}}\n\n").as_bytes());
        assert_eq!(summary(&tracker, "inFlight").state, expected);
        assert_eq!(summary(&tracker, "completed").state, expected);
    }
}

#[test]
fn cumulative_responses_counts_are_not_summed_and_failure_retains_hit() {
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    for tokens in [10, 10, 12] {
        tracker.push(format!("data: {{\"type\":\"response.in_progress\",\"response\":{{\"usage\":{{\"input_tokens_details\":{{\"cached_tokens\":{tokens}}}}}}}}}\n\n").as_bytes());
    }
    assert_eq!(summary(&tracker, "inFlight").read_tokens, Some(12));
    assert_eq!(
        summary(&tracker, "inFlight").finality,
        CacheReadFinality::Provisional
    );
    tracker.push(b"data: {\"type\":\"response.failed\",\"response\":{\"usage\":{\"input_tokens_details\":{\"cached_tokens\":12}}}}\n\n");
    assert_eq!(summary(&tracker, "completed").state, CacheReadState::Hit);
    assert_eq!(
        summary(&tracker, "completed").finality,
        CacheReadFinality::Confirmed
    );
}

#[test]
fn provisional_zero_and_missing_cache_do_not_become_misses() {
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    tracker.push(b"data: {\"type\":\"response.in_progress\",\"response\":{\"usage\":{\"input_tokens_details\":{\"cached_tokens\":0}}}}\n\n");
    assert_eq!(summary(&tracker, "inFlight").state, CacheReadState::Pending);
    assert_eq!(
        summary(&tracker, "completed").state,
        CacheReadState::Unknown
    );
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiChat);
    tracker.observe_json(br#"{"choices":[{"finish_reason":"stop"}],"usage":{"prompt_tokens":100,"completion_tokens":10,"cache_write_tokens":25}}"#);
    assert_eq!(
        summary(&tracker, "completed").reason,
        Some(CacheReadReason::Unreported)
    );
}

#[test]
fn chat_and_gemini_explicit_final_zero_confirm_a_miss() {
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiChat);
    tracker.push(
        b"data: {\"choices\":[],\"usage\":{\"prompt_tokens_details\":{\"cached_tokens\":0}}}\n\n",
    );
    assert_eq!(summary(&tracker, "inFlight").state, CacheReadState::Miss);
    let mut tracker = SseTerminalTracker::new(SseDialect::Gemini);
    tracker.push(b"data: {\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"cachedContentTokenCount\":0}}\n\n");
    assert_eq!(summary(&tracker, "inFlight").state, CacheReadState::Pending);
    tracker.finish();
    assert_eq!(summary(&tracker, "completed").state, CacheReadState::Miss);
}

#[test]
fn invalid_and_conflicting_values_are_not_silently_normalized() {
    for value in [json!(-1), json!(1.5), json!("20")] {
        let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
        tracker.observe_json(
            serde_json::to_string(&json!({"status":"completed","usage":{"cached_tokens":value}}))
                .unwrap()
                .as_bytes(),
        );
        assert_eq!(
            summary(&tracker, "completed").reason,
            Some(CacheReadReason::InvalidValue)
        );
    }
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    tracker.observe_json(br#"{"status":"completed","usage":{"cached_tokens":20,"input_tokens_details":{"cached_tokens":0}}}"#);
    assert_eq!(
        summary(&tracker, "completed").reason,
        Some(CacheReadReason::ConflictingEvidence)
    );
}

#[test]
fn oversized_observation_stays_unknown_and_tool_names_use_utf8_bytes() {
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    tracker.observe_json(&vec![b' '; 1024 * 1024 + 1]);
    assert_eq!(
        summary(&tracker, "completed").reason,
        Some(CacheReadReason::ObservationTruncated)
    );
    let mut tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    let short = "工".repeat(42); // 126 UTF-8 bytes
    let long = "工".repeat(43); // 129 UTF-8 bytes
    tracker.observe_json(
        serde_json::to_string(&json!({"status":"completed","output":[
            {"type":"function_call","name":short}, {"type":"function_call","name":long}
        ]}))
        .unwrap()
        .as_bytes(),
    );
    assert_eq!(tracker.tool_calls(), &[short]);
    assert!(tracker.tool_calls_truncated());
}

#[test]
fn confirmed_rate_reports_coverage_and_keeps_unknown_applicability_separate() {
    let mut stats = CacheReadStatistics::default();
    stats.add(CacheReadState::Hit, None, 1);
    stats.add(CacheReadState::Miss, None, 1);
    stats.add(
        CacheReadState::Unknown,
        Some(CacheReadReason::Unreported),
        2,
    );
    stats.add(
        CacheReadState::Unknown,
        Some(CacheReadReason::UnknownApplicability),
        5,
    );
    stats.add(CacheReadState::NotApplicable, None, 3);
    assert_eq!(stats.confirmed_hit_rate, Some(0.5));
    assert_eq!(stats.confirmation_coverage, Some(0.5));
    assert_eq!(stats.applicability_unknown_requests, 5);
}
