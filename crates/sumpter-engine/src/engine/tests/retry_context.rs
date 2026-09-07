//! retry_context regression tests.

use std::time::Duration;

use crate::engine::context::observed_session_id;
use crate::engine::context::{detect_client_kind, retain_codex_metadata_for_client};
use sumpter_core::events::{ClientKind, CodexMetadata};

#[test]
fn pi_explicit_identity_and_session_headers_have_stable_precedence() {
    let mut headers = vec![
        ("User-Agent".into(), "claude-cli/1.0".into()),
        ("X-Sumpter-Client".into(), "pi".into()),
        ("session_id".into(), "native".into()),
        ("X-Sumpter-Session-ID".into(), "explicit".into()),
    ];
    assert_eq!(detect_client_kind(&headers, false), ClientKind::Pi);
    assert_eq!(observed_session_id(&headers).as_deref(), Some("explicit"));
    assert!(
        retain_codex_metadata_for_client(
            ClientKind::Pi,
            CodexMetadata::from_request(&headers, None)
        )
        .is_none()
    );
    headers.clear();
    for name in [
        "session_id",
        "session-id",
        "x-session-id",
        "x-session-affinity",
    ] {
        assert_eq!(
            observed_session_id(&[(name.into(), "session".into())]).as_deref(),
            Some("session")
        );
    }
    assert!(observed_session_id(&[("x-client-request-id".into(), "request".into())]).is_none());
}
use crate::engine::dispatch::{retry_after_seconds, retry_backoff_delay};

fn assert_seconds(actual: Duration, expected: f64) {
    assert!(
        (actual.as_secs_f64() - expected).abs() < 0.001,
        "expected {expected}s, got {}s",
        actual.as_secs_f64()
    );
}

#[test]
fn retry_after_uses_largest_valid_numeric_header() {
    let headers = vec![
        ("Retry-After".into(), "2.5".into()),
        ("retry-after".into(), "7".into()),
        ("Retry-After".into(), "Wed, 21 Oct 2015 07:28:00 GMT".into()),
        ("Retry-After".into(), "-3".into()),
        ("Retry-After".into(), "NaN".into()),
    ];
    assert_eq!(retry_after_seconds(&headers), Some(7.0));
}

#[test]
fn retry_backoff_is_exponential_and_capped_with_retry_after() {
    assert_seconds(retry_backoff_delay(1, None), 0.5);
    assert_seconds(retry_backoff_delay(2, None), 0.85);
    assert_seconds(retry_backoff_delay(3, None), 1.445);
    assert_seconds(retry_backoff_delay(1, Some(7.0)), 7.0);
    assert_seconds(retry_backoff_delay(1, Some(45.0)), 30.0);
    assert_seconds(retry_backoff_delay(100, None), 30.0);
    assert_seconds(retry_backoff_delay(1, Some(-1.0)), 0.5);
    assert_seconds(retry_backoff_delay(1, Some(f64::NAN)), 0.5);
}

#[test]
fn observed_session_id_is_trimmed_bounded_and_rejects_controls() {
    let valid = vec![(
        "X-Claude-Code-Session-Id".into(),
        "  claude-session  ".into(),
    )];
    assert_eq!(
        observed_session_id(&valid).as_deref(),
        Some("claude-session")
    );

    let grok = vec![("x-grok-session-id".into(), "  grok-sess  ".into())];
    assert_eq!(observed_session_id(&grok).as_deref(), Some("grok-sess"));
    let grok_conv = vec![("x-grok-conv-id".into(), "conv-only".into())];
    assert_eq!(
        observed_session_id(&grok_conv).as_deref(),
        Some("conv-only")
    );

    let control = vec![("session_id".into(), "bad\nsession".into())];
    assert_eq!(observed_session_id(&control), None);

    let long = vec![("session-id".into(), "会".repeat(100))];
    let value = observed_session_id(&long).unwrap();
    assert!(value.len() <= 256);
    assert!(value.is_char_boundary(value.len()));
}
