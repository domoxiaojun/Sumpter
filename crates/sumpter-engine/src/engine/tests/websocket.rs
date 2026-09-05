//! websocket regression tests.

use std::time::Instant;

use axum::extract::ws::Message as WebSocketMessage;
use sumpter_core::events::ClientKind;

use crate::engine::websocket::{websocket_upstream_headers, websocket_url};
use crate::engine::websocket_relay::websocket_context_with_first_frame;
use crate::engine::websocket_relay::{
    WebSocketEventContext, WebSocketRelayCounters, tungstenite_message_stats,
    websocket_connect_error, websocket_message_codex_metadata, websocket_message_size,
};
use crate::request_build;

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
fn websocket_first_frame_originator_upgrades_generic_attribution() {
    let frame = WebSocketMessage::Text(
        r#"{"type":"response.create","model":"gpt-4o","originator":"Codex Desktop"}"#.into(),
    );
    let metadata = websocket_message_codex_metadata(&frame).expect("frame metadata");
    let context = WebSocketEventContext {
        request_id: "request".into(),
        request_path: "/v1/responses".into(),
        route_intent: "responses_websocket".into(),
        client_kind: ClientKind::OpenaiCompat,
        model: "gpt-4o".into(),
        codex_metadata: None,
        client_declared: None,
        grok_metadata: None,
        session_id: None,
        started: Instant::now(),
    };
    let merged = websocket_context_with_first_frame(&context, Some(&metadata));
    assert_eq!(merged.client_kind, ClientKind::Codex);
    assert_eq!(
        merged
            .codex_metadata
            .as_ref()
            .and_then(|metadata| metadata.originator.as_deref()),
        Some("Codex Desktop")
    );
}

#[test]
fn realtime_websocket_does_not_forward_listener_or_project_headers() {
    let forwarded = websocket_upstream_headers(&[
        ("authorization".into(), "Bearer listener-secret".into()),
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
    assert!(
        !forwarded
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("x-sumpter-project")
                || name.eq_ignore_ascii_case("x-sumpter-user"))
    );
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
