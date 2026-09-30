use sumpter_core::stream_terminal::{ResponsesRetryProbe, StreamRetryDecision};

#[test]
fn stream_retry_recognizes_structured_errors_across_all_chunk_boundaries() {
    for terminal in [
        r#"{"type":"response.failed","response":{"output":[],"error":{"code":"rate_limit_exceeded"}}}"#,
        r#"{"type":"error","code":"rate_limit_exceeded"}"#,
        r#"{"type":"error","error":{"type":"rate_limit_exceeded"}}"#,
        r#"{"type":"response.completed","response":{"status":"failed","error":{"code":"rate_limit_exceeded"}}}"#,
    ] {
        let input = format!(
            ": heartbeat\r\n\r\ndata: {{\"type\":\"response.in_progress\",\"response\":{{\"output\":[]}}}}\r\n\r\ndata: {terminal}\n\n"
        );
        for split in 0..input.len() {
            let mut probe = ResponsesRetryProbe::default();
            assert_eq!(
                probe.push(&input.as_bytes()[..split]),
                StreamRetryDecision::Pending
            );
            assert_eq!(
                probe.push(&input.as_bytes()[split..]),
                StreamRetryDecision::Retry("rate_limit_exceeded".into())
            );
        }
    }
}

#[test]
fn stream_retry_does_not_infer_retryability_from_error_messages() {
    for payload in [
        r#"{"type":"response.failed","response":{"error":{"code":"invalid_request_error","message":"server_error rate_limit_exceeded"}}}"#,
        r#"{"type":"response.failed","response":{"error":{"message":"server_error"}}}"#,
        r#"{"type":"response.created","response":{"output":[{"type":"function_call"}]}}"#,
        r#"{"type":"response.created","response":{"output":null}}"#,
        r#"{"type":"response.incomplete","response":{"error":{"code":"server_error"}}}"#,
    ] {
        let mut probe = ResponsesRetryProbe::default();
        assert_eq!(
            probe.push(format!("data: {payload}\n\n").as_bytes()),
            StreamRetryDecision::Forward
        );
    }
}
