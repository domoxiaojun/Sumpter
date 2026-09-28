use std::sync::Arc;

use bytes::Bytes;
use serde_json::{Value, json};
use sumpter_core::config::AppConfig;
use sumpter_engine::replay::{ReplayReply, ReplayTransport};
use sumpter_engine::{Engine, MAX_BODY_BYTES};

fn config(protocol: &str, model: &str, count: usize) -> AppConfig {
    AppConfig::from_json(
        &json!({
            "schemaVersion":7,
            "retry":{"maxDeferredRounds":1,"sessionStickyRetries":0},
            "endpoints":(0..count).map(|i| json!({
                "id":format!("provider-{i}"),"name":"Test","protocol":protocol,
                "enabled":true,"baseURL":format!("https://provider-{i}.invalid"),
                "mappings":[{"clientPattern":"test", "upstreamModel":model}]
            })).collect::<Vec<_>>()
        })
        .to_string(),
    )
    .unwrap()
    .normalized()
}

fn reply() -> ReplayReply {
    ReplayReply::ok(
        r#"{"id":"r","status":"completed","output":[],"usage":{"input_tokens":7,"output_tokens":1}}"#,
    )
}

async fn run(engine: &Engine, path: &str, body: Bytes) -> (u16, Bytes) {
    let response = engine
        .handle_request(
            None,
            "POST",
            path,
            vec![("content-type".into(), "application/json".into())],
            body,
        )
        .await;
    let status = response.status().as_u16();
    (
        status,
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn native_responses_share_large_unchanged_body_across_failover_and_capture() {
    assert_eq!(MAX_BODY_BYTES, usize::MAX);
    let payload = json!({"model":"test","input":[
        {"role":"user","content":[{"type":"input_text","text":"word ".repeat(700_000)},
            {"type":"input_image","image_url":format!("data:image/png;base64,{}","AAAA".repeat(2*1024*1024))}]},
        {"type":"reasoning","encrypted_content":"opaque-native-reasoning"},
        {"type":"function_call","call_id":"a","name":"draw","arguments":"{\"size\":2}"},
        {"type":"function_call_output","call_id":"a","output":"finished"}
    ],"tools":[{"type":"function","name":"draw","parameters":{"type":"object"}}],
    "include":["reasoning.encrypted_content"],"store":false,"vendor":{"unknown":true}});
    let body = Bytes::from(serde_json::to_vec_pretty(&payload).unwrap());
    for capture in [false, true] {
        let transport = Arc::new(ReplayTransport::new([
            Ok(ReplayReply {
                status: 503,
                ..ReplayReply::ok("busy")
            }),
            Ok(reply()),
        ]));
        let engine = Engine::new(
            config("openai-responses", "test", 2),
            None,
            transport.clone(),
        );
        engine.set_diagnostic_capture(capture, None);
        let (status, _) = run(&engine, "/v1/responses", body.clone()).await;
        assert_eq!(status, 200);
        let calls = transport.calls();
        assert_eq!(calls.len(), 2);
        for call in calls {
            assert_eq!(call.body, body);
            assert_eq!(
                call.body.as_ptr(),
                body.as_ptr(),
                "unchanged retries must share body storage"
            );
        }
        if capture {
            let snapshot = engine.diagnostic_capture_index();
            assert_eq!(snapshot["recordCount"], 1);
        }
    }
}

#[tokio::test]
async fn mapped_native_json_changes_only_model_and_preserves_client_effort() {
    let payload = json!({"model":"test", "reasoning":{"effort":"high"},
        "input":[{"role":"user","content":"word ".repeat(700_000)}],
        "store":false,"vendor":{"unknown":true}});
    for model in ["test", "mapped"] {
        let transport = Arc::new(ReplayTransport::new([Ok(reply())]));
        let engine = Engine::new(
            config("openai-responses", model, 1),
            None,
            transport.clone(),
        );
        let body = Bytes::from(serde_json::to_vec(&payload).unwrap());
        assert_eq!(run(&engine, "/v1/responses", body.clone()).await.0, 200);
        let outbound = &transport.calls()[0].body;
        let mut expected = payload.clone();
        expected["model"] = json!(model);
        assert_eq!(serde_json::from_slice::<Value>(outbound).unwrap(), expected);
        if model == "test" {
            assert_eq!(outbound.as_ptr(), body.as_ptr());
        }
    }
}

#[tokio::test]
async fn image_generation_reuses_large_json_without_changing_image_fields() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{\"data\":[]}"))]));
    let mut config = config("openai", "test", 1);
    config.endpoints[0].mappings[0].capabilities =
        vec![sumpter_core::capability::ModelCapability::Image];
    let engine = Engine::new(config, None, transport.clone());
    let body = Bytes::from(
        serde_json::to_vec(&json!({"model":"test", "prompt":"draw ".repeat(700_000),
        "image":format!("data:image/png;base64,{}", "AAAA".repeat(2*1024*1024)), "size":"auto"}))
        .unwrap(),
    );
    assert_eq!(
        run(&engine, "/v1/images/generations", body.clone()).await.0,
        200
    );
    assert_eq!(transport.calls()[0].body, body);
    assert_eq!(transport.calls()[0].body.as_ptr(), body.as_ptr());
}

#[tokio::test]
async fn translated_candidates_keep_tools_images_and_reject_opaque_reasoning() {
    let image = format!("data:image/png;base64,{}", "AAAA".repeat(1024));
    let payload = json!({"model":"test", "input":[
        {"role":"user","content":[{"type":"input_text","text":"word ".repeat(700_000)}, {"type":"input_image","image_url":image}]},
        {"type":"function_call","call_id":"call","name":"draw","arguments":"{\"size\":2}"},
        {"type":"function_call_output","call_id":"call","output":"ok"}],
        "tools":[{"type":"function","name":"draw","parameters":{"type":"object"}}]});
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok(
        r#"{"type":"message","role":"assistant","content":[{"type":"text","text":"ok"}],"stop_reason":"end_turn"}"#,
    ))]));
    let engine = Engine::new(config("anthropic", "mapped", 8), None, transport.clone());
    assert_eq!(
        run(
            &engine,
            "/v1/responses",
            Bytes::from(serde_json::to_vec(&payload).unwrap())
        )
        .await
        .0,
        200
    );
    let output: Value = serde_json::from_slice(&transport.calls()[0].body).unwrap();
    assert_eq!(output["model"], "mapped");
    assert_eq!(
        output["messages"][0]["content"][0]["text"],
        payload["input"][0]["content"][0]["text"]
    );
    assert_eq!(
        output["messages"][0]["content"][1]["source"]["data"],
        "AAAA".repeat(1024)
    );
    assert_eq!(output["messages"][1]["content"][0]["input"]["size"], 2);
    assert_eq!(output["messages"][2]["content"][0]["tool_use_id"], "call");
    assert_eq!(output["tools"][0]["name"], "draw");
    let mut invalid = payload;
    invalid["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"type":"reasoning","encrypted_content":"opaque"}));
    assert_eq!(
        run(
            &engine,
            "/v1/responses",
            Bytes::from(serde_json::to_vec(&invalid).unwrap())
        )
        .await
        .0,
        400
    );
    assert_eq!(transport.calls().len(), 1);
}
