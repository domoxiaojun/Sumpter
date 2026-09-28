//! Offline, release-mode large-body benchmark. No provider or database access.
//! Arguments: source target text_units iterations candidates capture [images].
//! A text unit is the synthetic string `word `, NOT a measured model token.
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::stream;
use serde_json::json;
use sumpter_core::config::AppConfig;
use sumpter_engine::Engine;
use sumpter_engine::outbound::{
    OutboundRequest, TransportError, UpstreamResponse, UpstreamTransport,
};

struct Sink;

#[async_trait::async_trait]
impl UpstreamTransport for Sink {
    async fn send_streaming(
        &self,
        request: OutboundRequest,
        _: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        std::hint::black_box(&request.body);
        let body = if request.path_and_query.contains("messages") {
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"bench\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"bench\",\"content\":[]}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"
        } else if request.path_and_query.contains("chat") {
            "data: {\"id\":\"bench\",\"object\":\"chat.completion.chunk\",\"model\":\"bench\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"
        } else {
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"bench\",\"status\":\"completed\",\"output\":[]}}\n\n"
        };
        Ok(UpstreamResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/event-stream".into())],
            stream: Box::pin(stream::once(async move {
                Ok(Bytes::from_static(body.as_bytes()))
            })),
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if std::env::var_os("SUMPTER_BENCH_STAGES").is_some() {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::stderr)
            .without_time()
            .with_ansi(false)
            .init();
    }
    let args: Vec<String> = std::env::args().collect();
    let source = args.get(1).map(String::as_str).unwrap_or("responses");
    let target = args.get(2).map(String::as_str).unwrap_or(source);
    let units: usize = args.get(3).map(|s| s.parse().unwrap()).unwrap_or(700_000);
    let iterations: usize = args.get(4).map(|s| s.parse().unwrap()).unwrap_or(10);
    let candidates: usize = args.get(5).map(|s| s.parse().unwrap()).unwrap_or(1);
    let capture = args.get(6).is_some_and(|s| s == "true");
    let images = args.get(7).is_some_and(|s| s == "images");
    let protocol = match target {
        "chat" => "openai",
        "responses" => "openai-responses",
        "anthropic" => "anthropic",
        _ => panic!("unsupported target"),
    };
    let config = json!({
        "schemaVersion": 7,
        "retry": {"maxDeferredRounds":1, "sessionStickyRetries":0},
        "endpoints": (0..candidates).map(|i| json!({
            "id": format!("bench-{i}"), "name":"Benchmark", "baseURL":"https://bench.invalid",
            "protocol":protocol, "enabled":true,
            "mappings":[{"clientPattern":"bench", "upstreamModel":"bench"}]
        })).collect::<Vec<_>>()
    });
    let config = AppConfig::from_json(&config.to_string())
        .unwrap()
        .normalized();
    let text = "word ".repeat(units / 32);
    let messages: Vec<_> = (0..32)
        .map(|i| {
            json!({
                "role": if i % 2 == 0 {"user"} else {"assistant"}, "content": text
            })
        })
        .collect();
    let (path, mut value) = match source {
        "responses" => (
            "/v1/responses",
            json!({"model":"bench", "stream":true, "input":messages}),
        ),
        "chat" => (
            "/v1/chat/completions",
            json!({"model":"bench", "stream":true, "messages":messages}),
        ),
        "anthropic" => (
            "/v1/messages",
            json!({"model":"bench", "stream":true, "max_tokens":32, "messages":messages}),
        ),
        _ => panic!("unsupported source"),
    };
    if images {
        let data = "AAAA".repeat(2 * 1024 * 1024);
        let block = if source == "anthropic" {
            json!({"type":"image", "source":{"type":"base64", "media_type":"image/png", "data":data}})
        } else if source == "responses" {
            json!({"type":"input_image", "image_url":format!("data:image/png;base64,{data}")})
        } else {
            json!({"type":"image_url", "image_url":{"url":format!("data:image/png;base64,{data}")}})
        };
        let key = if source == "responses" {
            "input"
        } else {
            "messages"
        };
        value[key][0]["content"] = json!([{"type":"text", "text":text}, block]);
    }
    let body = Bytes::from(serde_json::to_vec(&value).unwrap());
    drop(value);
    let engine = Engine::new(config, None, Arc::new(Sink));
    engine.set_diagnostic_capture(capture, None);
    let started = Instant::now();
    for _ in 0..iterations {
        if capture {
            engine.clear_diagnostic_capture().unwrap();
        }
        let response = engine
            .handle_request(
                None,
                "POST",
                path,
                vec![("content-type".into(), "application/json".into())],
                body.clone(),
            )
            .await;
        assert_eq!(response.status(), 200);
        let output = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        std::hint::black_box(output);
    }
    println!(
        "source={source} target={target} text_units={units} body_bytes={} iterations={iterations} candidates={candidates} capture={capture} images={images} elapsed_ms={:.3}",
        body.len(),
        started.elapsed().as_secs_f64() * 1000.0
    );
}
