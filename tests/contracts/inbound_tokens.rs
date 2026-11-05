//! Shared HTTP/SSE lifecycle contract for both platform adapters.
use super::*;
use sumpter_core::config::InboundTokenConfig;

#[tokio::test]
async fn inbound_tokens_keep_identity_across_stream_completion_and_reject_disabled_tokens() {
    let fake = FakeTransport::new();
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    config.listener.auth_tokens = vec![InboundTokenConfig {
        id: "work".into(),
        name: "工作电脑".into(),
        enabled: true,
        token: "synthetic-work-token".into(),
    }];
    config.listener.auth_token.clear();
    let engine = engine_with(config.clone(), fake.clone());
    for cancel in [false, true] {
        fake.push(
            "a.example.com",
            sse_ok(&["data: {\"type\":\"message_stop\"}\n\n"]),
        );
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                "/v1/messages",
                vec![
                    ("authorization".into(), "Bearer synthetic-work-token".into()),
                    ("x-sumpter-token-id".into(), "spoofed".into()),
                ],
                Body::from(body()),
            )
            .await;
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        if cancel {
            drop(response);
        } else {
            axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
        }
    }
    let events = engine.runtime_snapshot().recent_events;
    for event in events
        .iter()
        .filter(|event| matches!(event.kind.as_str(), "client" | "upstream"))
    {
        assert_eq!(event.inbound_token.as_ref().unwrap().id, "work");
        assert!(
            !serde_json::to_string(event)
                .unwrap()
                .contains("synthetic-work-token")
        );
    }
    assert_eq!(fake.requests().len(), 2);
    for request in fake.requests() {
        assert!(
            !request
                .headers
                .iter()
                .any(|(_, value)| value.contains("synthetic-work-token"))
        );
    }
    config.listener.auth_tokens[0].enabled = false;
    let _ = engine.replace_config(config.normalized());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("x-api-key".into(), "synthetic-work-token".into())],
        body(),
    )
    .await;
    assert_eq!(status, 401);
    assert_eq!(fake.requests().len(), 2);
}
