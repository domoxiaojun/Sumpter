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
