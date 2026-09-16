//! 代理审查回归：方法、副作用重放与有界响应观察。

use std::sync::Arc;

use sumpter_core::config::AppConfig;
use sumpter_engine::Engine;
use sumpter_engine::replay::{ReplayReply, ReplayTransport, observe_response};

fn config(protocol: &str, model: &str) -> AppConfig {
    let mut config = AppConfig::from_json(&serde_json::json!({
        "schemaVersion": 7,
        "listener": {"host": "127.0.0.1", "port": 0, "authToken": ""},
        "retry": {"maxDeferredRounds": 1, "sessionStickyRetries": 0},
        "endpoints": [{"id": "synthetic", "baseURL": "https://upstream.invalid", "protocol": protocol,
            "mappings": [{"clientPattern": model, "upstreamModel": model}]}],
    }).to_string()).unwrap();
    config = config.normalized();
    config
}

#[tokio::test]
async fn conversation_methods_are_rejected_instead_of_rewritten() {
    for (protocol, path, body) in [
        (
            "openai",
            "/chat/completions?keep=1",
            r#"{"model":"synthetic","messages":[{"role":"user","content":"hi"}]}"#,
        ),
        (
            "openai-responses",
            "/responses?keep=1",
            r#"{"model":"synthetic","input":"hi"}"#,
        ),
        (
            "openai-responses",
            "/responses/compact?keep=1",
            r#"{"model":"synthetic","input":"hi"}"#,
        ),
    ] {
        for method in ["PUT", "DELETE", "PATCH"] {
            let transport = Arc::new(ReplayTransport::new([]));
            let engine = Engine::new(config(protocol, "synthetic"), None, transport.clone());
            let response = engine
                .handle_request(None, method, path, vec![], body)
                .await;
            assert_eq!(response.status(), 405, "{method} {path}");
            assert_eq!(response.headers()["allow"], "POST");
            assert!(transport.calls().is_empty());
        }
    }
}

#[tokio::test]
async fn large_json_observation_limit_is_not_a_transport_failure() {
    let response_body = serde_json::json!({"choices": [{"message": {"role": "assistant", "content": "x".repeat(1024 * 1024 + 64)}, "finish_reason": "stop"}]}).to_string();
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok(
        response_body.clone(),
    ))]));
    let engine = Engine::new(config("openai", "synthetic"), None, transport);
    let response = engine
        .handle_request(
            None,
            "POST",
            "/v1/chat/completions",
            vec![],
            r#"{"model":"synthetic","messages":[{"role":"user","content":"hi"}]}"#,
        )
        .await;
    let observed = observe_response(response, []).await.unwrap();
    assert_eq!(observed.body.as_slice(), response_body.as_bytes());
    let snapshot = engine.runtime_snapshot();
    let client = snapshot
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(
        client.outcome,
        Some(sumpter_core::events::RuntimeEventOutcome::Succeeded)
    );
    assert!(client.failure_kind.is_none());
    let trace = client.stream_trace.as_ref().unwrap();
    assert!(trace.cache_read_evidence.as_ref().unwrap().truncated);
    assert!(trace.terminal_event.is_none());
}

#[tokio::test]
async fn small_malformed_json_still_fails_terminal_validation() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{\"choices\":"))]));
    let engine = Engine::new(config("openai", "synthetic"), None, transport);
    let response = engine
        .handle_request(
            None,
            "POST",
            "/v1/chat/completions",
            vec![],
            r#"{"model":"synthetic","messages":[{"role":"user","content":"hi"}]}"#,
        )
        .await;
    observe_response(response, []).await.unwrap();
    assert!(
        engine
            .runtime_snapshot()
            .recent_events
            .iter()
            .any(|event| event.kind == "client" && event.failure_kind.is_some())
    );
}

#[tokio::test]
async fn standard_realtime_never_replays_failures_or_fails_over() {
    for status in [429, 500, 503] {
        let mut config = config("openai", "gpt-realtime");
        config.retry.max_500_retries = 3;
        config.retry.session_sticky_retries = 2;
        config.retry.max_deferred_rounds = 2;
        let mut other = config.endpoints[0].clone();
        other.id = "synthetic-backup".into();
        config.endpoints.push(other);
        let mut failed = ReplayReply::ok(r#"{"error":"synthetic failure"}"#);
        failed.status = status;
        let transport = Arc::new(ReplayTransport::new([
            Ok(failed),
            Ok(ReplayReply::ok(r#"{"id":"must-not-be-created"}"#)),
        ]));
        let engine = Engine::new(config, None, transport.clone());
        let response = engine
            .handle_request(
                None,
                "POST",
                "/v1/realtime/client_secrets",
                vec![],
                r#"{"session":{"type":"realtime","model":"gpt-realtime"}}"#,
            )
            .await;
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(transport.calls().len(), 1, "status {status}");
        observe_response(response, []).await.unwrap();
    }
}
