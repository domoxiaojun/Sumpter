use std::sync::Arc;

use sumpter_core::config::AppConfig;
use sumpter_engine::replay::{ReplayReply, ReplayTransport};
use sumpter_macos_adapter::Engine;
use sumpter_macos_adapter::server;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

fn config() -> AppConfig {
    AppConfig::from_json(
        r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [{"id":"openai", "name":"OpenAI", "baseURL":"https://provider.invalid", "apiKey":"provider-key", "protocol":"openai-responses", "enabled":true,
             "mappings":[{"clientPattern":"gpt-4o", "upstreamModel":"gpt-4o-mini"}, {"clientPattern":"gpt-live-1-codex", "upstreamModel":"gpt-live-1-codex"}, {"clientPattern":"gpt-realtime", "upstreamModel":"gpt-realtime"}, {"clientPattern":"grok-imagine-video", "upstreamModel":"grok-imagine-video"}, {"clientPattern":"gpt-image-2", "upstreamModel":"gpt-image-2"}] }]
        }"#,
    )
    .expect("fixture config is valid")
    .normalized()
}

#[tokio::test]
async fn responses_websocket_rejects_missing_listener_auth_before_101() {
    let engine = Engine::new(
        config(),
        None,
        Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{}"))])),
        String::new(),
    );
    let (address, server) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");

    let request = format!("ws://{address}/v1/responses")
        .into_client_request()
        .expect("websocket request");
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("unauthorized upgrade must fail");
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 401);
        }
        other => panic!("unexpected websocket error: {other}"),
    }

    server.abort();
}

#[tokio::test]
async fn realtime_upstream_handshake_error_is_returned_before_101() {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    let upstream_task = tokio::spawn(async move {
        let (mut stream, _) = upstream_listener.accept().await.expect("accept upstream");
        stream
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: 24\r\nConnection: close\r\n\r\n{\"error\":\"unauthorized\"}",
            )
            .await
            .expect("write handshake rejection");
    });
    let mut config = config();
    config.endpoints[0].base_url = format!("http://{upstream_address}");
    let engine = Engine::new(
        config,
        None,
        Arc::new(ReplayTransport::new([])),
        String::new(),
    );
    let (address, server) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let mut request = format!("ws://{address}/v1/realtime?model=gpt-realtime")
        .into_client_request()
        .expect("websocket request");
    request.headers_mut().insert(
        "Authorization",
        "Bearer listener-secret"
            .parse()
            .expect("authorization header"),
    );
    let error = tokio_tungstenite::connect_async(request)
        .await
        .expect_err("upstream handshake rejection must fail downstream upgrade");
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status().as_u16(), 401);
        }
        other => panic!("unexpected websocket error: {other}"),
    }
    server.abort();
    upstream_task.await.expect("upstream task");
}

#[tokio::test]
async fn websocket_paths_keep_http_methods_on_the_engine_fallback() {
    let engine = Engine::new(
        config(),
        None,
        Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{}"))])),
        String::new(),
    );
    let (address, server) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let response = client
        .post(format!("http://{address}/v1/live"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .header("Accept", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_ne!(response.status().as_u16(), 405);
    assert_eq!(response.status().as_u16(), 200);
    server.abort();
}

#[tokio::test]
async fn cpa_resource_paths_preserve_dynamic_suffix_and_http_method() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok(
        br#"{"id":"file_123"}"#.to_vec(),
    ))]));
    let engine = Engine::new(config(), None, transport.clone(), String::new());
    let (address, server) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let auth = "Bearer listener-secret";

    let file = client
        .delete(format!("http://{address}/v1/files/file_123"))
        .header("Authorization", auth)
        .send()
        .await
        .expect("file delete");
    assert_eq!(file.status().as_u16(), 200);

    let video = client
        .get(format!(
            "http://{address}/v1/videos/video_123/content?variant=video"
        ))
        .header("Authorization", auth)
        .header("Accept", "video/mp4")
        .send()
        .await
        .expect("video content");
    assert_eq!(video.status().as_u16(), 404);

    let models = client
        .get(format!("http://{address}/v1/models?cursor=next"))
        .header("Authorization", auth)
        .send()
        .await
        .expect("model list");
    assert_eq!(models.status().as_u16(), 200);
    let models_json: serde_json::Value =
        serde_json::from_slice(&models.bytes().await.expect("model list body"))
            .expect("model list JSON");
    assert_eq!(models_json["object"], "list");
    assert!(
        models_json["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|model| model["id"] == "gpt-4o")
    );

    let calls = transport.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].method, "DELETE");
    assert_eq!(calls[0].path_and_query, "/v1/files/file_123");

    server.abort();
}
