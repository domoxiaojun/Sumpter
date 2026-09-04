use std::sync::Arc;

use axum::http::{HeaderMap, HeaderValue, Method, Uri};
use bytes::Bytes;
use sumpter_core::config::AppConfig;
use sumpter_engine::replay::{
    ReplayReply, ReplayRequest, ReplayTransport, compare, observe_response,
};
use sumpter_engine::{Engine, EngineServices, NoopPlatform};

fn config() -> AppConfig {
    AppConfig::from_json(
        r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":57878, "authToken":""},
          "retry": {"maxDeferredRounds":1, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [
            {"id":"primary", "name":"Primary", "baseURL":"https://primary.invalid", "apiKey":"test-primary", "protocol":"anthropic", "enabled":true,
             "mappings":[{"clientPattern":"claude-test", "upstreamModel":"claude-upstream"}]},
            {"id":"secondary", "name":"Secondary", "baseURL":"https://secondary.invalid", "apiKey":"test-secondary", "protocol":"anthropic", "enabled":true,
             "mappings":[{"clientPattern":"claude-test", "upstreamModel":"claude-upstream"}]}
          ]
        }"#,
    )
    .expect("fixture config is valid")
    .normalized()
}

fn request(stream: bool) -> ReplayRequest {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("user-agent", HeaderValue::from_static("replay-client"));
    let body = format!(
        "{{\"model\":\"claude-test\",\"max_tokens\":32,\"stream\":{stream},\"messages\":[{{\"role\":\"user\",\"content\":\"hello\"}}]}}"
    );
    ReplayRequest {
        method: Method::POST,
        uri: Uri::from_static("http://127.0.0.1:57878/v1/messages"),
        headers,
        remote_ip: Some("127.0.0.1".parse().unwrap()),
        body: Bytes::from(body),
    }
}

fn live_config() -> AppConfig {
    live_config_with_sticky_retries(0)
}

fn live_config_with_sticky_retries(session_sticky_retries: i64) -> AppConfig {
    let mut config = AppConfig::from_json(
        r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":57878, "authToken":""},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [
            {"id":"cpa", "name":"CPA", "baseURL":"https://cpa.invalid", "apiKey":"test-cpa", "protocol":"openai", "enabled":true,
             "mappings":[{"clientPattern":"gpt-live-1-codex", "upstreamModel":"gpt-live-1-codex", "capabilities":["live"]}]}
          ]
        }"#,
    )
    .expect("Live fixture config is valid");
    config.retry.session_sticky_retries = session_sticky_retries;
    config.normalized()
}

fn live_request() -> ReplayRequest {
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("application/json"));
    headers.insert("originator", HeaderValue::from_static("Codex Desktop"));
    ReplayRequest {
        method: Method::POST,
        uri: Uri::from_static("http://127.0.0.1:57878/v1/live"),
        headers,
        remote_ip: Some("127.0.0.1".parse().unwrap()),
        body: Bytes::from_static(br#"{"model":"gpt-live-1-codex"}"#),
    }
}

fn success_json() -> Bytes {
    Bytes::from(
        r#"{"id":"msg-generated","type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"model":"claude-upstream","stop_reason":"end_turn","usage":{"input_tokens":2,"output_tokens":1}}"#,
    )
}

#[tokio::test]
async fn replay_compares_status_headers_body_events_and_retry_order() {
    let replies = vec![
        Ok(ReplayReply {
            status: 503,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![Ok(Bytes::from_static(br#"{"error":"busy"}"#))],
        }),
        Ok(ReplayReply::ok(success_json())),
    ];
    let left_transport = Arc::new(ReplayTransport::new(replies.clone()));
    let right_transport = Arc::new(ReplayTransport::new(replies));
    let left = Engine::new_with_services(
        config(),
        None,
        EngineServices {
            transport: left_transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );
    let right = Engine::new_with_services(
        config(),
        None,
        EngineServices {
            transport: right_transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );

    let left_response = left
        .handle_inbound_request(request(false).into_inbound())
        .await;
    let right_response = right
        .handle_inbound_request(request(false).into_inbound())
        .await;
    let mut left_observation = observe_response(left_response, []).await.unwrap();
    let mut right_observation = observe_response(right_response, []).await.unwrap();
    left_observation.events = left
        .runtime_snapshot()
        .recent_events
        .iter()
        .map(sumpter_engine::engine::events::comparable_event)
        .collect();
    right_observation.events = right
        .runtime_snapshot()
        .recent_events
        .iter()
        .map(sumpter_engine::engine::events::comparable_event)
        .collect();

    compare(&left_observation, &right_observation).expect("identical replay must compare equal");
    assert_eq!(left_observation.status, 200);
    assert_eq!(left_transport.calls().len(), 2);
    assert_eq!(right_transport.calls().len(), 2);
    assert_eq!(
        left_transport
            .calls()
            .iter()
            .map(|call| call.base_url.as_str())
            .collect::<Vec<_>>(),
        vec!["https://primary.invalid", "https://secondary.invalid"]
    );
}

#[tokio::test]
async fn replay_preserves_sse_frame_order_while_normalizing_generated_ids() {
    let sse = Bytes::from_static(
        b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"a\",\"role\":\"assistant\"}}\n\nevent: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"one\"}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
        chunks: vec![Ok(sse)],
    })]));
    let engine = Engine::new_with_services(
        config(),
        None,
        EngineServices {
            transport,
            platform: Arc::new(NoopPlatform),
        },
    );
    let response = engine
        .handle_inbound_request(request(true).into_inbound())
        .await;
    let mut observed = observe_response(response, []).await.unwrap();
    observed.events = engine
        .runtime_snapshot()
        .recent_events
        .iter()
        .map(sumpter_engine::engine::events::comparable_event)
        .collect();
    let body = String::from_utf8(observed.body).unwrap();
    assert!(body.find("one").unwrap() < body.find("end_turn").unwrap());
    assert_eq!(body.matches("<generated>").count(), 1);
    assert_eq!(observed.status, 200);
}

#[tokio::test]
async fn realtime_401_does_not_cool_the_cpa_endpoint_for_the_next_request() {
    let unauthorized = || ReplayReply {
        status: 401,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![Ok(Bytes::from_static(
            br#"{"error":"revoked OAuth account"}"#,
        ))],
    };
    let transport = Arc::new(ReplayTransport::new([
        Ok(unauthorized()),
        Ok(unauthorized()),
    ]));
    let engine = Engine::new_with_services(
        live_config(),
        None,
        EngineServices {
            transport: transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );

    let first = engine
        .handle_inbound_request(live_request().into_inbound())
        .await;
    let second = engine
        .handle_inbound_request(live_request().into_inbound())
        .await;

    assert_eq!(first.status(), 401);
    assert_eq!(second.status(), 401);
    assert_eq!(transport.calls().len(), 2);
}

#[tokio::test]
async fn codex_live_bootstrap_retries_401_within_the_same_request() {
    let transport = Arc::new(ReplayTransport::new([
        Ok(ReplayReply {
            status: 401,
            headers: vec![("content-type".into(), "application/json".into())],
            chunks: vec![Ok(Bytes::from_static(
                br#"{"error":{"code":"token_revoked"}}"#,
            ))],
        }),
        Ok(ReplayReply {
            status: 201,
            headers: vec![
                ("content-type".into(), "application/sdp".into()),
                ("location".into(), "/v1/live/call-working".into()),
            ],
            chunks: vec![Ok(Bytes::from_static(b"v=0\r\na=answer\r\n"))],
        }),
    ]));
    let engine = Engine::new_with_services(
        live_config_with_sticky_retries(1),
        None,
        EngineServices {
            transport: transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );

    let response = engine
        .handle_inbound_request(live_request().into_inbound())
        .await;

    assert_eq!(response.status(), 201);
    let calls = transport.calls();
    assert_eq!(calls.len(), 2, "Live bootstrap should retry once after 401");
    assert!(calls.iter().all(|call| {
        call.base_url.as_str() == "https://cpa.invalid"
            && call.headers.iter().any(|(name, value)| {
                name.eq_ignore_ascii_case("authorization") && value == "Bearer test-cpa"
            })
    }));
}

#[tokio::test]
async fn standard_realtime_does_not_replay_401_even_when_sticky_retries_are_enabled() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply {
        status: 401,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![Ok(Bytes::from_static(br#"{"error":"unauthorized"}"#))],
    })]));
    let engine = Engine::new_with_services(
        live_config_with_sticky_retries(2),
        None,
        EngineServices {
            transport: transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );
    let mut request = live_request();
    request.uri = Uri::from_static("http://127.0.0.1:57878/v1/realtime/client_secrets");

    let response = engine.handle_inbound_request(request.into_inbound()).await;

    assert_eq!(response.status(), 401);
    assert_eq!(transport.calls().len(), 1);
}

#[tokio::test]
async fn codex_live_uses_endpoint_key_instead_of_an_ephemeral_realtime_token() {
    // `ek_…` is a credential for the public Realtime client-secret flow.  A
    // Codex `/v1/live` request must authenticate CPA with the endpoint key;
    // CPA then selects the Codex OAuth account itself.  This guards against
    // accidentally forwarding a downstream Realtime token to CPA and getting
    // a misleading 401 before the account pool is consulted.
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply {
        status: 401,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![Ok(Bytes::from_static(br#"{"error":"token_revoked"}"#))],
    })]));
    let engine = Engine::new_with_services(
        live_config(),
        None,
        EngineServices {
            transport: transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );
    let mut request = live_request();
    request.headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer ek_downstream"),
    );

    let response = engine.handle_inbound_request(request.into_inbound()).await;
    assert_eq!(response.status(), 401);
    let calls = transport.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("authorization") && value == "Bearer test-cpa"
    }));
    assert!(
        !calls[0]
            .headers
            .iter()
            .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                && value.contains("ek_downstream"))
    );
    assert!(
        !calls[0]
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("x-api-key"))
    );
}

#[tokio::test]
async fn unregistered_realtime_token_does_not_override_endpoint_key() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply {
        status: 401,
        headers: vec![("content-type".into(), "application/json".into())],
        chunks: vec![Ok(Bytes::from_static(br#"{"error":"invalid_api_key"}"#))],
    })]));
    let engine = Engine::new_with_services(
        live_config(),
        None,
        EngineServices {
            transport: transport.clone(),
            platform: Arc::new(NoopPlatform),
        },
    );
    let mut request = live_request();
    request.uri = Uri::from_static("http://127.0.0.1:57878/v1/realtime/calls");
    request.headers.insert(
        "authorization",
        HeaderValue::from_static("Bearer ek_not_registered"),
    );

    let response = engine.handle_inbound_request(request.into_inbound()).await;
    assert_eq!(response.status(), 401);
    let calls = transport.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("authorization") && value == "Bearer test-cpa"
    }));
    assert!(!calls[0].headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("authorization") && value.contains("ek_not_registered")
    }));
}
