//! websocket regression tests.

use std::time::Instant;

use axum::extract::ws::Message as WebSocketMessage;

use crate::engine::websocket::{websocket_upstream_headers, websocket_url};
use crate::engine::websocket_relay::websocket_trace;
use crate::engine::websocket_relay::{
    WebSocketRelayCounters, tungstenite_message_stats, websocket_connect_error,
    websocket_message_size,
};
use crate::request_build;

#[test]
fn websocket_cookie_cleanup_preserves_other_business_cookies() {
    let headers = websocket_upstream_headers(&[
        (
            "Cookie".into(),
            "first=one; sumpter_admin_session=synthetic-admin".into(),
        ),
        ("cookie".into(), "second=two".into()),
        ("Cookie".into(), "broken".into()),
    ]);
    let values: Vec<_> = headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(values, ["first=one", "second=two"]);
}

#[test]
fn websocket_close_metrics_have_one_byte_contract_and_eof_is_failure() {
    let axum_close = WebSocketMessage::Close(Some(axum::extract::ws::CloseFrame {
        code: 1000,
        reason: "bye".into(),
    }));
    assert_eq!(websocket_message_size(&axum_close), 5);
    assert_eq!(websocket_message_size(&WebSocketMessage::Close(None)), 0);

    let tungstenite_close = tokio_tungstenite::tungstenite::Message::Close(Some(
        tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal,
            reason: "bye".into(),
        },
    ));
    assert_eq!(
        tungstenite_message_stats(&tungstenite_close),
        (5, Some(1000))
    );

    let counters = WebSocketRelayCounters::default();
    counters.record_error("upstream_eof_without_close", "upstream");
    let metrics = counters.snapshot(Instant::now());
    assert!(metrics.failed);
    assert!(metrics.abnormal_close);
    assert_eq!(metrics.closed_by.as_deref(), Some("upstream"));
    assert_eq!(
        metrics.relay_error.as_deref(),
        Some("upstream_eof_without_close")
    );
}

#[test]
fn websocket_non_http_connect_error_has_no_upstream_status() {
    let (status, _) = websocket_connect_error(&tokio_tungstenite::tungstenite::Error::Io(
        std::io::Error::other("dial failed"),
    ));
    assert_eq!(status, 0);
}

#[test]
fn websocket_trace_keeps_client_upgrade_and_first_message_wait_separate() {
    let trace = websocket_trace(
        0,
        Some(101),
        "awaiting_first_message",
        Some(37),
        0,
        0,
        0,
        0,
        None,
        None,
        Some("client".into()),
        Some("client_eof_without_close".into()),
        true,
        0,
    );
    let value = serde_json::to_value(trace.websocket_trace.unwrap()).unwrap();
    assert_eq!(value["clientHandshakeStatus"], 101);
    assert_eq!(value["stage"], "awaiting_first_message");
    assert_eq!(value["firstMessageWaitMS"], 37);
    assert!(value.get("handshakeStatus").is_none());
}

#[test]
fn realtime_websocket_does_not_forward_listener_or_project_headers() {
    let forwarded = websocket_upstream_headers(&[
        ("authorization".into(), "Bearer listener-secret".into()),
        ("X-Goog-Api-Key".into(), "synthetic-listener-key".into()),
        ("x-api-key".into(), "synthetic-listener-key".into()),
        ("X-Sumpter-Agent-Name".into(), "synthetic-agent".into()),
        (
            "x-sumpter-future-private-field".into(),
            "synthetic-private".into(),
        ),
        ("x-sumpter-project".into(), "private-workspace".into()),
        ("x-sumpter-user".into(), "kkl".into()),
        ("openai-beta".into(), "realtime=v1".into()),
        ("sec-websocket-protocol".into(), "realtime".into()),
    ]);
    assert!(
        !forwarded
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("authorization"))
    );
    assert!(!forwarded.iter().any(
        |(name, _)| name.to_ascii_lowercase().starts_with("x-sumpter-")
            || name.eq_ignore_ascii_case("x-goog-api-key")
            || name.eq_ignore_ascii_case("x-api-key")
    ));
    assert!(forwarded.iter().any(
            |(name, value)| name.eq_ignore_ascii_case("openai-beta") && value == "realtime=v1"
        ));
    assert!(forwarded.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("sec-websocket-protocol") && value == "realtime"
    }));
}

#[test]
fn realtime_websocket_url_preserves_base_path_without_duplicate_v1() {
    assert_eq!(
        websocket_url(
            "https://provider.invalid/v1",
            "/v1/realtime?model=gpt-realtime"
        ),
        Some("wss://provider.invalid/v1/realtime?model=gpt-realtime".into())
    );
    assert_eq!(
        websocket_url("http://127.0.0.1:9000", "/realtime"),
        Some("ws://127.0.0.1:9000/realtime".into())
    );
    assert_eq!(
        websocket_url(
            "https://provider.invalid/v1",
            "/backend-api/codex/responses?model=gpt-4o"
        ),
        Some("wss://provider.invalid/v1/backend-api/codex/responses?model=gpt-4o".into())
    );
    assert_eq!(
        websocket_url(
            "https://ccc.domob.org",
            &request_build::strip_codex_live_query(
                "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver&architecture=avas",
            ),
        ),
        Some("wss://ccc.domob.org/v1/realtime?model=gpt-live-1-codex".into())
    );
}
