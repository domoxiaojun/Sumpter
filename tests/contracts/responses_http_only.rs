//! Responses transport negotiation never opens a socket or counts as inference.
use super::{http_only_test_engine, pi_test_shutdown, server};
use axum::body::Body;
use bytes::Bytes;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use sumpter_core::events::RuntimeEventOutcome;
use sumpter_engine::engine::EngineNotice;
use sumpter_engine::replay::{ReplayReply, ReplayTransport};

const ROOTS: [&str; 3] = [
    "/v1/responses",
    "/responses",
    "/backend-api/codex/responses",
];

#[tokio::test]
async fn responses_upgrade_aliases_and_partial_handshakes_are_local_negotiation() {
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut config = super::config();
    config.endpoints[0].base_url = format!("http://{}", upstream.local_addr().unwrap());
    config.migrate_model_groups();
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = http_only_test_engine(config, transport.clone());
    engine.set_diagnostic_capture(true, Some(64 * 1024));
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    for root in ROOTS {
        for suffix in ["", "?model=gpt-4o", "/compact", "/resp-test?model=gpt-4o"] {
            for upgrade in ["websocket", "WebSocket", "h2c, WebSocket"] {
                for complete in [false, true] {
                    let mut request = client
                        .get(format!("http://{address}{root}{suffix}"))
                        .bearer_auth("listener-secret")
                        .header("Upgrade", upgrade);
                    if complete {
                        request = request
                            .header("Connection", "Upgrade")
                            .header("Sec-WebSocket-Version", "13")
                            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==");
                    }
                    let response = request.send().await.unwrap();
                    assert_eq!(response.status(), 426, "{root}{suffix} {upgrade}");
                    assert!(!response.headers().contains_key("sec-websocket-accept"));
                    let body: serde_json::Value =
                        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
                    assert_eq!(body["error"]["code"], "websocket_not_supported");
                }
            }
        }
        let response = client
            .get(format!("http://{address}{root}?model=gpt-4o"))
            .bearer_auth("listener-secret")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 405);
        assert_eq!(response.headers()["allow"], "POST");
    }
    for path in ["/unknown", "/v1/responses-other", "/v1/chat/completions"] {
        let response = client
            .get(format!("http://{address}{path}?model=gpt-4o"))
            .bearer_auth("listener-secret")
            .header("Upgrade", "websocket")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
    }
    assert_eq!(engine.runtime_snapshot().client_requests, 0);
    assert_eq!(engine.runtime_snapshot().upstream_attempts, 0);
    assert!(engine.runtime_snapshot().recent_events.is_empty());
    assert!(engine.diagnostic_capture_snapshot().records.is_empty());
    assert!(notices.try_recv().is_err());
    assert!(transport.calls().is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(30), upstream.accept())
            .await
            .is_err()
    );
    let summary = engine.runtime_summary_value();
    assert_eq!(summary["apiVersion"], 1);
    assert!(summary.get("responsesWebSocketConnections").is_none());

    // Rejection still authenticates even when no complete handshake was supplied.
    let denied = client
        .get(format!("http://{address}/v1/responses"))
        .header("Upgrade", "websocket")
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), 401);
    assert_eq!(engine.runtime_snapshot().client_failures, 1);
    pi_test_shutdown(handle).await;
}

#[tokio::test]
async fn shared_entry_rejects_before_body_reads_and_preserves_http_resource_methods() {
    let mut config = super::config();
    config.listener.allowed_cidrs = vec!["192.0.2.0/24".into()];
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{}"))]));
    let engine = http_only_test_engine(config, transport.clone());
    let headers = vec![
        ("authorization".into(), "Bearer listener-secret".into()),
        ("upgrade".into(), "websocket".into()),
    ];
    let unread = Body::from_stream(futures_util::stream::once(async {
        panic!("transport negotiation must not poll the body");
        #[allow(unreachable_code)]
        Ok::<Bytes, std::io::Error>(Bytes::new())
    }));
    let result = engine
        .handle_request(
            Some("127.0.0.1".parse().unwrap()),
            "GET",
            "/v1/responses",
            headers.clone(),
            unread,
        )
        .await;
    assert_eq!(result.status(), 426);
    assert_eq!(engine.runtime_snapshot().client_requests, 0);
    let denied = engine
        .handle_request(
            Some("203.0.113.1".parse().unwrap()),
            "GET",
            "/responses",
            headers.clone(),
            Body::empty(),
        )
        .await;
    assert_eq!(denied.status(), 403);
    assert!(transport.calls().is_empty());
    let resource_headers = vec![("authorization".into(), "Bearer listener-secret".into())];
    let response = engine
        .handle_request(
            Some("127.0.0.1".parse().unwrap()),
            "GET",
            "/v1/responses/resp-test",
            resource_headers,
            Body::empty(),
        )
        .await;
    // This unbound resource already returns 404; it must not become 405/426.
    assert_eq!(response.status(), 404);
    let _ = axum::body::to_bytes(response.into_body(), 1024)
        .await
        .unwrap();
    assert!(transport.calls().is_empty());
}

#[tokio::test]
async fn http_fallback_keeps_each_turn_outcome_and_usage_independent() {
    let replies = (0..3).map(|index| {
        let terminal = if index == 1 { "response.failed" } else { "response.completed" };
        let response = json!({"id":format!("resp-{index}"),"model":"gpt-4o-mini",
            "status":if index == 1 {"failed"} else {"completed"}, "output":[],
            "usage":{"input_tokens":100+index,"output_tokens":20+index,"total_tokens":120+2*index}});
        let stream = format!("data: {}\n\ndata: {}\n\n", json!({"type":"response.created","response":{"id":format!("resp-{index}")}}), json!({"type":terminal,"response":response}));
        Ok(ReplayReply {status:200,headers:vec![("content-type".into(),"text/event-stream".into())],chunks:vec![Ok(Bytes::from(stream))]})
    });
    let transport = Arc::new(ReplayTransport::new(replies));
    let engine = http_only_test_engine(super::config(), transport.clone());
    let mut notices = engine.subscribe();
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let mut ids = std::collections::HashSet::new();
    for (index, root) in ROOTS.iter().enumerate() {
        let probe = client
            .get(format!("http://{address}{root}"))
            .bearer_auth("listener-secret")
            .header("Upgrade", "websocket")
            .send()
            .await
            .unwrap();
        assert_eq!(probe.status(), 426);
        let input = if index == 0 {
            json!([{"role":"user","content":"hello"}])
        } else {
            json!([{"role":"user","content":"hello"},{"type":"function_call","call_id":"tool-1","name":"test","arguments":"{}"},{"type":"function_call_output","call_id":"tool-1","output":"done"}])
        };
        let reply = client
            .post(format!("http://{address}{root}"))
            .bearer_auth("listener-secret")
            // A stray Upgrade header must not turn a POST into a WS probe.
            .header("Upgrade", "websocket")
            .header("Content-Type", "application/json")
            .body(json!({"model":"gpt-4o","stream":true,"input":input}).to_string())
            .send()
            .await
            .unwrap();
        let _ = reply.bytes().await.unwrap();
        let event = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let EngineNotice::Event(event) = notices.recv().await.unwrap()
                    && event.kind == "client"
                    && !event.is_in_flight()
                {
                    break event;
                }
            }
        })
        .await
        .unwrap();
        assert!(ids.insert(event.request_id.clone()));
        assert_eq!(
            event.outcome,
            Some(if index == 1 {
                RuntimeEventOutcome::Failed
            } else {
                RuntimeEventOutcome::Succeeded
            })
        );
        let usage = event.stream_trace.as_ref().unwrap().usage.as_ref().unwrap();
        assert_eq!(usage.input_tokens, Some(100 + index as u64));
        assert_eq!(usage.output_tokens, Some(20 + index as u64));
        assert_eq!(
            engine.runtime_snapshot().client_requests,
            (index + 1) as i64
        );
    }
    let snapshot = engine.runtime_snapshot();
    assert_eq!(snapshot.client_successes, 2);
    assert_eq!(snapshot.client_failures, 1);
    assert_eq!(transport.calls().len(), 3);
    assert_eq!(snapshot.upstream_attempts, 3);
    pi_test_shutdown(handle).await;
}
