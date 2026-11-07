use sumpter_core::stream_terminal::{SseDialect, SseTerminalTracker};

#[test]
fn counts_complete_nonempty_output_deltas_in_each_supported_dialect() {
    for (dialect, payload) in [
        (
            SseDialect::Anthropic,
            r#"{"type":"content_block_delta","delta":{"text":"hi"}}"#,
        ),
        (
            SseDialect::OpenAiChat,
            r#"{"choices":[{"delta":{"reasoning_content":"thinking"}}]}"#,
        ),
        (
            SseDialect::OpenAiResponses,
            r#"{"type":"response.function_call_arguments.delta","delta":"{}"}"#,
        ),
        (
            SseDialect::Gemini,
            r#"{"candidates":[{"content":{"parts":[{"text":"hi"}]}}]}"#,
        ),
    ] {
        let mut tracker = SseTerminalTracker::new(dialect);
        let frame = format!("data: {payload}\n\n");
        for byte in frame.as_bytes() {
            let _ = tracker.push(&[*byte]);
        }
        assert_eq!(tracker.output_events(), 1, "{dialect:?}");
        let _ = tracker.push(frame.as_bytes());
        assert_eq!(tracker.output_events(), 2, "{dialect:?}");
    }
}

#[test]
fn lifecycle_usage_and_empty_deltas_are_not_output_samples() {
    for dialect in [
        SseDialect::Anthropic,
        SseDialect::OpenAiChat,
        SseDialect::OpenAiResponses,
        SseDialect::Gemini,
    ] {
        let mut tracker = SseTerminalTracker::new(dialect);
        for payload in [
            r#"{"type":"response.created","response":{"output":[]}}"#,
            r#"{"type":"response.output_text.delta","delta":""}"#,
            r#"{"choices":[{"delta":{"role":"assistant","content":""}}]}"#,
            r#"{"usage":{"output_tokens":100}}"#,
            r#"{"usageMetadata":{"candidatesTokenCount":100}}"#,
            r#"{"type":"content_block_start","content_block":{"type":"tool_use","name":"read"}}"#,
        ] {
            let _ = tracker.push(format!("data: {payload}\n\n").as_bytes());
        }
        let _ = tracker.push(b": keepalive\n\n");
        assert_eq!(tracker.output_events(), 0, "{dialect:?}");
    }
}
