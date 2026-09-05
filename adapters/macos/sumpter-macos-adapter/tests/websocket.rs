use std::sync::Arc;

use sumpter_core::config::AppConfig;
use sumpter_engine::replay::{ReplayReply, ReplayTransport};
use sumpter_macos_adapter::Engine;
use sumpter_macos_adapter::server;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

#[tokio::test]
async fn legacy_fixed_ips_do_not_duplicate_live_posts_and_new_captures_omit_ip() {
    use axum::http::{HeaderMap, Method, StatusCode, Uri};
    use bytes::Bytes;
    use std::time::Duration;
    use sumpter_engine::outbound::ReqwestTransport;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let authority = format!("localhost:{port}");
    let base_url = format!("http://{authority}");
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let sent = sent.clone();
            async move {
                sent.send((method, uri, headers, body)).unwrap();
                // Leave headers pending so any competing POST reaches the server.
                tokio::time::sleep(Duration::from_millis(50)).await;
                (
                    StatusCode::CREATED,
                    [
                        ("content-type", "application/sdp"),
                        ("location", "/v1/live/call-once"),
                    ],
                    "v=0\r\na=answer\r\n",
                )
            }
        },
    );
    let upstream = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut legacy = serde_json::to_value(config()).unwrap();
    legacy["endpoints"][0]["baseURL"] = serde_json::json!(base_url);
    legacy["endpoints"][0]["pinnedIPs"] = serde_json::json!(["127.0.0.1", "127.0.0.2"]);
    legacy["endpoints"][0]["pinnedIPExclusive"] = serde_json::json!(false);
    legacy["retry"]["pinnedIPConcurrency"] = serde_json::json!(3);
    legacy["retry"]["sessionStickyRetries"] = serde_json::json!(2);
    let config = AppConfig::from_json(&legacy.to_string())
        .unwrap()
        .normalized();
    let engine = Engine::new(
        config,
        None,
        Arc::new(ReqwestTransport::new()),
        String::new(),
    );
    engine.set_diagnostic_capture(true, Some(1024 * 1024));
    let (address, handle) = server::serve(engine.clone(), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let response = client
        .post(format!("http://{address}/v1/live"))
        .bearer_auth("listener-secret")
        .header("Content-Type", "application/sdp")
        .body("v=0\r\na=offer\r\n")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(response.text().await.unwrap(), "v=0\r\na=answer\r\n");

    let (method, uri, headers, body) = received.try_recv().unwrap();
    assert_eq!(method, Method::POST);
    assert_eq!(uri.path(), "/v1/live");
    assert_eq!(headers["host"], authority);
    assert_eq!(body.as_ref(), b"v=0\r\na=offer\r\n");
    assert!(matches!(
        received.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
    let runtime = engine.runtime_snapshot();
    assert_eq!(runtime.upstream_attempts, 1);
    assert_eq!(runtime.upstream_successes, 1);
    assert_eq!(runtime.upstream_failures, 0);
    assert_eq!(runtime.failovers, 0);
    assert_eq!(
        runtime
            .recent_events
            .iter()
            .filter(|event| event.kind == "upstream")
            .count(),
        1
    );
    let capture = engine.diagnostic_capture_snapshot();
    assert_eq!(capture.records.len(), 1);
    assert_eq!(capture.records[0].attempts.len(), 1);
    let attempt = serde_json::to_value(&capture.records[0].attempts[0]).unwrap();
    assert!(attempt.get("pinnedIP").is_none());
    assert!(attempt.get("pinnedIp").is_none());

    handle.abort();
    upstream.abort();
}

fn config() -> AppConfig {
    AppConfig::from_json(
        r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0},
          "endpoints": [{"id":"openai", "name":"OpenAI", "baseURL":"https://provider.invalid", "apiKey":"provider-key", "protocol":"openai-responses", "enabled":true,
             "mappings":[{"clientPattern":"gpt-4o", "upstreamModel":"gpt-4o-mini", "capabilities":["live"]}, {"clientPattern":"gpt-live-1-codex", "upstreamModel":"gpt-live-1-codex"}, {"clientPattern":"gpt-realtime", "upstreamModel":"gpt-realtime"}, {"clientPattern":"grok-imagine-video", "upstreamModel":"grok-imagine-video"}, {"clientPattern":"gpt-image-2", "upstreamModel":"gpt-image-2"}, {"clientPattern":"file-*", "upstreamModel":"file-*", "capabilities":["files"]}] }]
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
