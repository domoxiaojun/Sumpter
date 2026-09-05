//! protocol regression tests.

use std::cell::RefCell;

use sumpter_core::events::{ClientKind, CodexMetadata};

use crate::engine::context::{INBOUND_REQUEST_CONTEXT, InboundRequestContext};
use crate::engine::payload::metadata_json_body_hint;
use crate::engine::protocol::{
    RealtimeCallPathError, RealtimeRouteIntent, classify_realtime_intent, is_codex_live_path,
    is_codex_live_sideband_target, is_live_bootstrap_request, live_call_id_from_headers,
    live_call_id_from_json, live_call_id_from_target, realtime_voice_route_model,
    unsupported_realtime_call_action, validate_realtime_call_target,
};
use crate::request_build;

#[tokio::test]
async fn realtime_root_live_bootstrap_is_the_only_root_that_registers_a_call() {
    INBOUND_REQUEST_CONTEXT
        .scope(
            RefCell::new(InboundRequestContext {
                method: "POST".into(),
                path: "/v1/realtime".into(),
                route_intent: "live".into(),
                session_id: None,
                grok_metadata: None,
            }),
            async {
                assert!(is_live_bootstrap_request(
                    "POST",
                    "/v1/realtime?intent=quicksilver"
                ));
                assert!(!is_live_bootstrap_request("GET", "/v1/realtime"));
                assert!(!is_live_bootstrap_request(
                    "POST",
                    "/v1/realtime?intent=quicksilver&call_id=rtc_1"
                ));
            },
        )
        .await;
}

#[test]
fn body_only_codex_originator_is_used_without_content_type() {
    let body = br#"{"clientMetadata":{"Originator":"Codex Desktop"}}"#;
    let value = metadata_json_body_hint(body, None).expect("JSON body hint");
    let metadata = CodexMetadata::from_request(&[], Some(&value)).expect("metadata");
    assert_eq!(metadata.originator.as_deref(), Some("Codex Desktop"));
    assert_eq!(
        ClientKind::detect_with_originator(
            Some("Mozilla/5.0"),
            metadata.originator.as_deref(),
            true,
        ),
        ClientKind::Codex
    );
    // An explicit non-JSON media type remains opaque even if its bytes
    // happen to start with a JSON object.
    assert!(metadata_json_body_hint(body, Some("application/sdp")).is_none());
}

#[test]
fn live_path_uses_the_same_strict_call_target_validator() {
    let target = validate_realtime_call_target("/v1/live/rtc_1").expect("valid target");
    assert_eq!(target.unwrap().call_id, "rtc_1");
    let target = validate_realtime_call_target("/openai/v1/live/rtc-1/hangup")
        .expect("valid action target")
        .expect("target");
    assert_eq!(target.action.as_deref(), Some("hangup"));
    assert_eq!(
        validate_realtime_call_target("/v1/live/rtc%2F1"),
        Err(RealtimeCallPathError::InvalidId)
    );
    assert_eq!(
        validate_realtime_call_target("/v1/live/rtc_1/content"),
        Err(RealtimeCallPathError::UnsupportedAction)
    );
    assert_eq!(
        validate_realtime_call_target("/v1/live/"),
        Err(RealtimeCallPathError::InvalidId)
    );
}

#[test]
fn public_realtime_path_is_not_treated_as_codex_live() {
    assert!(is_codex_live_path("/v1/live"));
    assert!(!is_codex_live_path("/v1/realtime/calls"));
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/live",
            None,
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime",
            Some(request_build::DEFAULT_REALTIME_MODEL),
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "GET",
            "/v1/realtime?model=gpt-realtime",
            Some(request_build::DEFAULT_REALTIME_MODEL),
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::StandardRealtime
    );
    assert_eq!(
        classify_realtime_intent(
            "GET",
            "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver",
            Some(request_build::DEFAULT_CODEX_LIVE_MODEL),
            None,
            None,
            ClientKind::Codex,
            true,
        ),
        RealtimeRouteIntent::StandardRealtime
    );
    assert_eq!(
        classify_realtime_intent(
            "GET",
            "/v1/realtime?architecture=avas",
            None,
            None,
            None,
            ClientKind::Codex,
            false,
        ),
        RealtimeRouteIntent::StandardRealtime
    );
    assert_eq!(
        classify_realtime_intent(
            "GET",
            "/v1/realtime?call_id=call-1",
            None,
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime/calls",
            None,
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime?model=claude-fable-5",
            Some("claude-fable-5"),
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime?model=claude-fable-5",
            Some("claude-fable-5"),
            None,
            None,
            ClientKind::Codex,
            false,
        ),
        RealtimeRouteIntent::CodexLive
    );
    assert_eq!(
        classify_realtime_intent(
            "GET",
            "/v1/realtime?model=claude-fable-5",
            Some("claude-fable-5"),
            None,
            None,
            ClientKind::Codex,
            false,
        ),
        RealtimeRouteIntent::StandardRealtime
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime/client_secrets",
            Some("gpt-4o"),
            None,
            None,
            ClientKind::OpenaiCompat,
            false,
        ),
        RealtimeRouteIntent::StandardRealtime
    );
    assert_eq!(
        classify_realtime_intent(
            "POST",
            "/v1/realtime/calls/call-1/hangup",
            Some(request_build::DEFAULT_CODEX_LIVE_MODEL),
            None,
            None,
            ClientKind::Codex,
            true,
        ),
        RealtimeRouteIntent::CodexLive
    );
}

#[test]
fn voice_bootstrap_ignores_leaked_chat_models() {
    assert_eq!(
        realtime_voice_route_model(
            "POST",
            "/v1/realtime?model=claude-fable-5",
            Some("claude-fable-5".into()),
            None,
            None,
        ),
        request_build::DEFAULT_CODEX_LIVE_MODEL
    );
    assert_eq!(
        realtime_voice_route_model(
            "GET",
            "/v1/realtime?model=claude-fable-5",
            Some("claude-fable-5".into()),
            None,
            None,
        ),
        "claude-fable-5"
    );
    assert_eq!(
        realtime_voice_route_model(
            "POST",
            "/v1/realtime/client_secrets",
            Some("gpt-4o".into()),
            None,
            None,
        ),
        "gpt-4o"
    );
    assert_eq!(
        realtime_voice_route_model(
            "GET",
            "/v1/realtime?model=gpt-4o",
            Some("gpt-4o".into()),
            None,
            Some("gpt-4o".into()),
        ),
        "gpt-4o"
    );
}

#[test]
fn cpa_live_targets_accept_header_and_quicksilver_call_ids() {
    assert_eq!(
        live_call_id_from_headers(&[("X-Live-Session".into(), "call-header".into())]).as_deref(),
        Some("call-header")
    );
    assert_eq!(
        live_call_id_from_headers(&[
            ("X-Call-ID".into(), "fallback-call".into()),
            ("X-Live-Session".into(), "live-session".into()),
            ("X-Live-Call-ID".into(), "live-call".into()),
        ])
        .as_deref(),
        Some("live-call")
    );
    assert_eq!(
        live_call_id_from_target("/v1/live/call-path").as_deref(),
        Some("call-path")
    );
    assert_eq!(
        live_call_id_from_target("/v1/realtime?intent=quicksilver&call_id=call-query").as_deref(),
        Some("call-query")
    );
    assert_eq!(
        live_call_id_from_target("/v1/realtime?intent=quicksilver&call_id=rtc_1").as_deref(),
        Some("rtc_1")
    );
    assert_eq!(
        live_call_id_from_headers(&[(
            "Location".into(),
            "https://provider.invalid/v1/realtime?intent=quicksilver&call_id=call-location".into(),
        )])
        .as_deref(),
        Some("call-location")
    );
    assert!(is_codex_live_sideband_target(
        "/v1/realtime?intent=quicksilver&call_id=call-query"
    ));
    assert!(is_codex_live_sideband_target(
        "/v1/realtime?call_id=ordinary-realtime"
    ));
    assert!(!unsupported_realtime_call_action(
        "/v1/realtime/calls/rtc_1/hangup"
    ));
    assert!(!unsupported_realtime_call_action(
        "/v1/realtime/calls/rtc_1"
    ));
    assert!(unsupported_realtime_call_action(
        "/v1/realtime/calls/rtc_1/unknown"
    ));
    assert_eq!(
        validate_realtime_call_target("/v1/realtime/calls/%%%").unwrap_err(),
        RealtimeCallPathError::InvalidId
    );
    assert_eq!(
        validate_realtime_call_target("/v1/realtime/calls/").unwrap_err(),
        RealtimeCallPathError::InvalidId
    );
    assert_eq!(
        validate_realtime_call_target("/v1/realtime/calls/call-1/hangup/extra").unwrap_err(),
        RealtimeCallPathError::UnsupportedAction
    );
    assert_eq!(
        validate_realtime_call_target("/v1/realtime?call_id=call%2F1").unwrap_err(),
        RealtimeCallPathError::InvalidId
    );
    let target = validate_realtime_call_target("/v1/realtime/calls/call%2D1/hangup")
        .expect("decoded call target")
        .expect("call target");
    assert_eq!(target.call_id, "call-1");
    assert_eq!(target.action.as_deref(), Some("hangup"));
    assert_eq!(
        live_call_id_from_json(br#"{"object":"realtime.call","id":"rtc_1"}"#).as_deref(),
        Some("rtc_1")
    );
}

#[test]
fn resource_paths_preserve_inbound_aliases_and_avoid_duplicate_base_v1() {
    assert_eq!(
        request_build::openai_resource_path("", "/files/file_123/content?x=1"),
        "/files/file_123/content?x=1"
    );
    assert_eq!(
        request_build::openai_resource_path("/v1", "/v1/videos/video_123"),
        "/v1/videos/video_123"
    );
    assert_eq!(
        request_build::openai_resource_path("/gateway", "/openai/v1/files"),
        "/gateway/openai/v1/files"
    );
    assert_eq!(
        request_build::openai_resource_path("/gateway/v1", "/v1foo"),
        "/gateway/v1/v1foo"
    );
}
