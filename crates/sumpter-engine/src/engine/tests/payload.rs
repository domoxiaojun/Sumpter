//! payload regression tests.

use crate::engine::payload::{
    RAW_MODEL_SNIFF_BYTES, native_multipart_fields_with_default, raw_body_model_hint,
    raw_request_body_hint, realtime_body_model, rewrite_realtime_multipart_model,
};
use crate::request_build;

#[test]
fn videos_multipart_model_is_used_for_routing_without_rebuilding_body() {
    let boundary = "video-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\ncat\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video-1.5\r\n--{boundary}--\r\n"
    );
    let fields = native_multipart_fields_with_default(
        body.as_bytes(),
        &format!("multipart/form-data; boundary=\"{boundary}\""),
        "",
    )
    .expect("valid video multipart");
    assert_eq!(fields.model, "grok-imagine-video-1.5");
    assert!(!fields.stream);
}

#[test]
fn realtime_multipart_model_normalizes_session_without_touching_sdp_or_parts() {
    let boundary = "live-model-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\nContent-Type: application/sdp\r\n\r\nv=0\\r\\no=offer\\r\\n\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\nContent-Type: application/json\r\n\r\n{{\"model\":\"claude-fable-5\",\"instructions\":\"keep me\"}}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"opaque\"\r\n\r\nbytes--{boundary}\r\n\r\n--{boundary}--\r\n"
    );
    let rewritten = rewrite_realtime_multipart_model(
        body.as_bytes(),
        &format!("multipart/form-data; boundary=\"{boundary}\""),
        request_build::DEFAULT_CODEX_LIVE_MODEL,
    );
    let rewritten = std::str::from_utf8(&rewritten).expect("multipart remains UTF-8");
    assert!(rewritten.contains("v=0\\r\\no=offer\\r\\n"));
    assert!(rewritten.contains("\"model\":\"gpt-live-1-codex\""));
    assert!(rewritten.contains("\"instructions\":\"keep me\""));
    assert!(rewritten.contains(&format!("bytes--{boundary}")));
}

#[test]
fn multipart_parser_requires_real_line_delimiters_and_closing_boundary() {
    let boundary = "safe-boundary";
    // The uploaded bytes contain boundary-looking text, but neither
    // occurrence is a valid delimiter (bad line position/suffix). The
    // parser must retain the complete binary field and still find the
    // actual closing delimiter.
    let mut body = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\n")
        .into_bytes();
    body.extend_from_slice(b"binary--safe-boundaryX\r\n");
    body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video\r\n"
            )
            .as_bytes(),
        );
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    let fields = native_multipart_fields_with_default(
        &body,
        &format!("multipart/form-data; boundary={boundary}"),
        "",
    )
    .expect("valid multipart");
    assert_eq!(fields.model, "grok-imagine-video");

    let truncated = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\ngrok-imagine-video\r\n"
    );
    assert_eq!(
        native_multipart_fields_with_default(
            truncated.as_bytes(),
            &format!("multipart/form-data; boundary={boundary}"),
            "",
        )
        .unwrap_err(),
        "multipart closing boundary is missing"
    );
}

#[test]
fn multipart_field_names_are_case_insensitive() {
    let boundary = "case-boundary";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"MODEL\"\r\n\r\ngrok-imagine-video\r\n--{boundary}--\r\n"
    );
    let fields = native_multipart_fields_with_default(
        body.as_bytes(),
        &format!("multipart/form-data; boundary={boundary}"),
        "",
    )
    .expect("valid multipart");
    assert_eq!(fields.model, "grok-imagine-video");
}

#[test]
fn raw_dispatch_hints_require_framing_and_bound_model_sniffing() {
    assert!(raw_request_body_hint(
        "POST",
        &[("content-length".into(), "12".into())]
    ));
    assert!(!raw_request_body_hint("POST", &[]));
    assert!(!raw_request_body_hint(
        "GET",
        &[("content-length".into(), "0".into())]
    ));
    assert!(raw_request_body_hint(
        "POST",
        &[("transfer-encoding".into(), "chunked".into())]
    ));

    assert_eq!(
        raw_body_model_hint(br#" {"model":"vendor-model","x":1} "#, None).as_deref(),
        Some("vendor-model")
    );
    assert_eq!(raw_body_model_hint(b"\0{\"model\":\"nope\"}", None), None);
    assert_eq!(
        raw_body_model_hint(&vec![b' '; RAW_MODEL_SNIFF_BYTES + 1], None),
        None
    );
}

#[test]
fn realtime_body_model_reads_session_model_without_guessing_text_model() {
    assert_eq!(
        realtime_body_model(
            br#"{"session":{"model":"gpt-realtime"}}"#,
            Some("application/json")
        )
        .as_deref(),
        Some("gpt-realtime")
    );
    assert_eq!(realtime_body_model(b"{}", Some("application/sdp")), None);
    let boundary = "model-extraction-boundary";
    let multipart = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"sdp\"\r\n\r\nv=0\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"session\"\r\n\r\n{{\"model\":\"claude-fable-5\",\"voice\":\"marin\"}}\r\n--{boundary}--\r\n"
    );
    assert_eq!(
        realtime_body_model(
            multipart.as_bytes(),
            Some(&format!("multipart/form-data; boundary={boundary}")),
        )
        .as_deref(),
        Some("claude-fable-5")
    );
}
