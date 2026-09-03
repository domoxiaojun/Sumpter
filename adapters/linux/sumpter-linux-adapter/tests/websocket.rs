use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use sumpter_core::config::{AppConfig, ContextMode, ModelMapping, ThinkingMode};
use sumpter_engine::replay::{ReplayReply, ReplayTransport};
use sumpter_linux_adapter::Engine;
use sumpter_linux_adapter::server;
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

fn config() -> AppConfig {
    config_with_base("https://provider.invalid", "openai-responses")
}

fn config_with_base(base_url: &str, protocol: &str) -> AppConfig {
    let raw = r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [{"id":"openai", "name":"OpenAI", "baseURL":"__BASE_URL__", "apiKey":"provider-key", "protocol":"__PROTOCOL__", "enabled":true,
             "mappings":[{"clientPattern":"gpt-4o", "upstreamModel":"gpt-4o-mini"}, {"clientPattern":"gpt-live-1-codex", "upstreamModel":"gpt-live-1-codex"}, {"clientPattern":"gpt-realtime", "upstreamModel":"gpt-realtime"}, {"clientPattern":"grok-imagine-video", "upstreamModel":"grok-imagine-video"}, {"clientPattern":"gpt-image-2", "upstreamModel":"gpt-image-2"}] }]
        }"#
    .replace("__BASE_URL__", base_url)
    .replace("__PROTOCOL__", protocol);
    AppConfig::from_json(&raw)
        .expect("fixture config is valid")
        .normalized()
}

async fn connect(
    url: String,
    token: Option<&str>,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let mut request = url.into_client_request().expect("websocket request");
    if let Some(token) = token {
        request.headers_mut().insert(
            "Authorization",
            format!("Bearer {token}")
                .parse()
                .expect("authorization header"),
        );
    }
    let (socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("websocket upgrade");
    socket
}

#[tokio::test]
async fn responses_websocket_relays_frames_without_http_or_sse_reconstruction() {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream_listener.accept().await.expect("accept upstream");
        let mut socket = accept_async(stream).await.expect("upstream websocket");
        let received = socket.next().await.expect("upstream frame").expect("frame");
        let _ = received_tx.send(received);
        socket
            .send(Message::Text(r#"{"type":"response.created"}"#.into()))
            .await
            .expect("send upstream event");
        socket
            .send(Message::Binary(b"responses-binary".as_slice().into()))
            .await
            .expect("send upstream binary");
        let _ = socket.close(None).await;
    });

    let engine = Engine::new(
        config_with_base(&format!("http://{upstream_address}"), "openai-responses"),
        None,
        Arc::new(ReplayTransport::new([])),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");

    let mut socket = connect(
        format!("ws://{address}/v1/responses?model=gpt-4o"),
        Some("listener-secret"),
    )
    .await;
    socket
        .send(Message::Text(
            r#"{"type":"response.create","model":"gpt-4o","input":"hello"}"#.into(),
        ))
        .await
        .expect("send response frame");
    assert_eq!(
        received_rx.await.expect("upstream receive"),
        Message::Text(r#"{"type":"response.create","model":"gpt-4o","input":"hello"}"#.into())
    );
    assert_eq!(
        socket.next().await.expect("response event").expect("frame"),
        Message::Text(r#"{"type":"response.created"}"#.into())
    );
    assert_eq!(
        socket
            .next()
            .await
            .expect("response binary")
            .expect("frame"),
        Message::Binary(b"responses-binary".as_slice().into())
    );

    let _ = socket.close(None).await;
    handle.shutdown().await;
    let _ = upstream_task.await;
}

#[tokio::test]
async fn realtime_websocket_relays_text_and_binary_frames_to_native_provider() {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream_listener.accept().await.expect("accept upstream");
        let mut socket = accept_async(stream).await.expect("upstream websocket");
        let received = socket
            .next()
            .await
            .expect("upstream client frame")
            .expect("frame");
        let _ = received_tx.send(received);
        socket
            .send(Message::Text(r#"{"type":"session.created"}"#.into()))
            .await
            .expect("send text frame");
        socket
            .send(Message::Binary(b"provider-binary".as_slice().into()))
            .await
            .expect("send binary frame");
        let _ = socket.close(None).await;
    });

    let engine = Engine::new(
        config_with_base(&format!("http://{upstream_address}"), "openai"),
        None,
        Arc::new(ReplayTransport::new([])),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let mut downstream = connect(
        format!("ws://{address}/v1/realtime?model=gpt-4o"),
        Some("listener-secret"),
    )
    .await;
    downstream
        .send(Message::Text(r#"{"type":"session.update"}"#.into()))
        .await
        .expect("send client frame");

    let received = tokio::time::timeout(std::time::Duration::from_secs(2), received_rx)
        .await
        .expect("upstream receive timeout")
        .expect("upstream receive");
    assert_eq!(
        received,
        Message::Text(r#"{"type":"session.update"}"#.into())
    );
    assert_eq!(
        downstream
            .next()
            .await
            .expect("downstream text")
            .expect("frame"),
        Message::Text(r#"{"type":"session.created"}"#.into())
    );
    assert_eq!(
        downstream
            .next()
            .await
            .expect("downstream binary")
            .expect("frame"),
        Message::Binary(b"provider-binary".as_slice().into())
    );

    let _ = downstream.close(None).await;
    handle.shutdown().await;
    let _ = upstream_task.await;
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

    let engine = Engine::new(
        config_with_base(&format!("http://{upstream_address}"), "openai"),
        None,
        Arc::new(ReplayTransport::new([])),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let request = format!("ws://{address}/v1/realtime?model=gpt-realtime")
        .into_client_request()
        .expect("websocket request");
    let mut request = request;
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
    handle.shutdown().await;
    upstream_task.await.expect("upstream task");
}

#[tokio::test]
async fn realtime_client_secret_authorizes_follow_up_websocket() {
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_address = upstream_listener.local_addr().expect("upstream address");
    let (received_tx, received_rx) = tokio::sync::oneshot::channel();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream_listener.accept().await.expect("accept upstream");
        let mut socket = accept_async(stream).await.expect("upstream websocket");
        let received = socket.next().await.expect("session update").expect("frame");
        let _ = received_tx.send(received);
        socket
            .send(Message::Text(r#"{"type":"session.created"}"#.into()))
            .await
            .expect("send session.created");
        let _ = socket.close(None).await;
    });

    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_secs()
        + 60;
    let secret_body = format!(
        r#"{{"value":"ek_test_secret","expires_at":{expires_at},"session":{{"type":"realtime","model":"gpt-4o","instructions":"speak","audio":{{"output":{{"voice":"marin"}}}}}}}}"#
    );
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok(
        secret_body.into_bytes(),
    ))]));
    let engine = Engine::new(
        config_with_base(&format!("http://{upstream_address}"), "openai"),
        None,
        transport,
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let secret_response = client
        .post(format!("http://{address}/v1/realtime/client_secrets"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/json")
        .body(r#"{"session":{"type":"realtime","model":"gpt-4o"}}"#)
        .send()
        .await
        .expect("client secret response");
    assert_eq!(secret_response.status(), 200);
    let secret_value: serde_json::Value =
        serde_json::from_slice(&secret_response.bytes().await.unwrap()).unwrap();
    assert_eq!(secret_value["value"], "ek_test_secret");

    let mut websocket_request = format!("ws://{address}/v1/realtime?model=gpt-4o")
        .into_client_request()
        .expect("websocket request");
    websocket_request.headers_mut().insert(
        "Authorization",
        "Bearer ek_test_secret"
            .parse()
            .expect("authorization header"),
    );
    let (mut downstream, _) = tokio_tungstenite::connect_async(websocket_request)
        .await
        .expect("ephemeral websocket upgrade");
    let session_update = received_rx
        .await
        .expect("session update received by upstream");
    let session_update: serde_json::Value = match session_update {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("session update JSON"),
        other => panic!("unexpected session update frame: {other:?}"),
    };
    assert_eq!(session_update["type"], "session.update");
    assert_eq!(session_update["session"]["model"], "gpt-4o-mini");
    assert_eq!(session_update["session"]["instructions"], "speak");
    assert_eq!(
        session_update["session"]["audio"]["output"]["voice"],
        "marin"
    );
    assert_eq!(
        downstream
            .next()
            .await
            .expect("downstream frame")
            .expect("downstream message"),
        Message::Text(r#"{"type":"session.created"}"#.into())
    );

    let _ = downstream.close(None).await;
    handle.shutdown().await;
    let _ = upstream_task.await;
}

#[tokio::test]
async fn realtime_client_secret_model_is_used_for_http_follow_up() {
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_secs()
        + 60;
    let transport = Arc::new(ReplayTransport::new([
        Ok(ReplayReply::ok(
            format!(r#"{{"value":"ek_http_secret","expires_at":{expires_at},"session":{{"type":"realtime","model":"gpt-4o","id":"sess_public","object":"realtime.session","instructions":"http session","audio":{{"output":{{"voice":"marin"}}}}}}}}"#).into_bytes(),
        )),
        Ok(ReplayReply::ok(br#"{"id":"realtime-call"}"#.to_vec())),
    ]));
    let engine = Engine::new(config(), None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let secret_response = client
        .post(format!("http://{address}/v1/realtime/client_secrets"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/json")
        .body(r#"{"session":{"type":"realtime","model":"gpt-4o"}}"#)
        .send()
        .await
        .expect("client secret response");
    assert_eq!(secret_response.status().as_u16(), 200);
    let _ = secret_response.bytes().await.expect("secret body");

    let follow_up = client
        .post(format!("http://{address}/v1/realtime/calls"))
        .header("Authorization", "Bearer ek_http_secret")
        .header("Content-Type", "application/json")
        .body(r#"{"session":{"type":"realtime"}}"#)
        .send()
        .await
        .expect("realtime follow-up");
    assert_eq!(follow_up.status().as_u16(), 200);
    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].method, "POST");
    assert_eq!(calls[1].path_and_query, "/v1/realtime/calls");
    assert!(calls[1].headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("authorization") && value == "Bearer ek_http_secret"
    }));
    let call_body: serde_json::Value = serde_json::from_slice(&calls[1].body).expect("call JSON");
    assert_eq!(call_body["session"]["instructions"], "http session");
    assert_eq!(call_body["session"]["audio"]["output"]["voice"], "marin");
    assert!(call_body["session"].get("id").is_none());
    assert!(call_body["session"].get("object").is_none());
    handle.shutdown().await;
}

#[tokio::test]
async fn websocket_upgrade_rejects_missing_listener_auth_before_101() {
    let engine = Engine::new(
        config(),
        None,
        Arc::new(ReplayTransport::new([Ok(ReplayReply::ok("{}"))])),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
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

    handle.shutdown().await;
}

#[tokio::test]
async fn websocket_paths_keep_http_methods_on_the_engine_fallback() {
    let transport = Arc::new(ReplayTransport::new([
        Ok(ReplayReply {
            status: 200,
            headers: vec![("content-type".into(), "application/sdp".into())],
            chunks: vec![Ok(bytes::Bytes::from_static(b"v=0\r\n"))],
        }),
        Ok(ReplayReply {
            status: 200,
            headers: vec![("content-type".into(), "application/sdp".into())],
            chunks: vec![Ok(bytes::Bytes::from_static(b"v=0\r\n"))],
        }),
    ]));
    let engine = Engine::new(
        config_with_base("https://provider.invalid", "openai"),
        None,
        transport.clone(),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");

    let live = client
        .post(format!("http://{address}/v1/live"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .header("Accept", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_ne!(live.status().as_u16(), 405);
    assert_eq!(live.status().as_u16(), 200);

    let realtime = client
        .post(format!("http://{address}/v1/realtime/calls"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .header("Accept", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("realtime bootstrap response");
    assert_ne!(realtime.status().as_u16(), 405);
    assert_eq!(realtime.status().as_u16(), 200);

    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].method, "POST");
    assert_eq!(
        calls[0].path_and_query,
        "/v1/live?intent=quicksilver&architecture=avas"
    );
    let live_body: serde_json::Value =
        serde_json::from_slice(&calls[0].body).expect("Codex Live JSON envelope");
    assert_eq!(live_body["session"]["type"], "quicksilver");
    assert_eq!(live_body["session"]["model"], "gpt-live-1-codex");
    assert_eq!(
        calls[0]
            .headers
            .iter()
            .find(|(name, _)| name == "content-type")
            .map(|(_, value)| value.as_str()),
        Some("application/json")
    );
    assert_eq!(calls[1].method, "POST");
    assert_eq!(
        calls[1].path_and_query,
        "/v1/realtime/calls?intent=quicksilver&architecture=avas"
    );
    assert_eq!(calls[1].body, b"v=0\r\n");
    assert!(
        calls[1]
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "application/sdp")
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn codex_live_without_a_live_mapping_returns_no_live_provider() {
    let mut config = config();
    config.endpoints[0]
        .mappings
        .retain(|mapping| mapping.client_pattern == "gpt-4o");
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = Engine::new(config, None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
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
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_eq!(response.status().as_u16(), 503);
    let body: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.expect("error body bytes"))
            .expect("error body JSON");
    assert_eq!(body["error"], "no_live_provider");
    assert!(transport.calls().is_empty());
    handle.shutdown().await;
}

#[tokio::test]
async fn codex_live_never_falls_back_to_anthropic_text_provider() {
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = Engine::new(
        config_with_base("https://provider.invalid", "anthropic"),
        None,
        transport.clone(),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
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
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_eq!(response.status().as_u16(), 503);
    let body: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.expect("error body bytes"))
            .expect("error body JSON");
    assert_eq!(body["error"], "no_live_provider");
    assert!(transport.calls().is_empty());
    handle.shutdown().await;
}

#[tokio::test]
async fn codex_live_rejects_a_wildcard_mapping_to_a_text_model() {
    let mut config = config();
    config.endpoints[0]
        .mappings
        .retain(|mapping| mapping.client_pattern == "gpt-4o");
    config.endpoints[0].mappings.push(ModelMapping {
        client_pattern: "*".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Passthrough,
        upstream_model: "claude-fable-5".into(),
        capabilities: Vec::new(),
    });
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = Engine::new(config, None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
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
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_eq!(response.status().as_u16(), 503);
    let body: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.expect("error body bytes"))
            .expect("error body JSON");
    assert_eq!(body["error"], "no_live_provider");
    assert!(transport.calls().is_empty());
    handle.shutdown().await;
}

#[tokio::test]
async fn realtime_rejects_a_wildcard_mapping_to_a_text_model() {
    let mut config = config();
    config.endpoints[0]
        .mappings
        .retain(|mapping| mapping.client_pattern == "gpt-4o");
    config.endpoints[0].mappings.push(ModelMapping {
        client_pattern: "*".into(),
        context: ContextMode::Standard,
        failover_timeout_seconds: None,
        thinking: ThinkingMode::Passthrough,
        upstream_model: "claude-fable-5".into(),
        capabilities: Vec::new(),
    });
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = Engine::new(config, None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let response = client
        .post(format!("http://{address}/v1/realtime/calls"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("realtime bootstrap response");
    assert_eq!(response.status().as_u16(), 503);
    let body: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.expect("error body bytes"))
            .expect("error body JSON");
    assert_eq!(body["error"], "no_live_provider");
    assert!(transport.calls().is_empty());
    handle.shutdown().await;
}

#[tokio::test]
async fn realtime_bootstrap_does_not_follow_a_leaked_chat_model() {
    let raw = r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [
            {"id":"xiao", "name":"xiao", "baseURL":"https://anyrouter.invalid", "apiKey":"xiao-key", "protocol":"openai", "enabled":true,
             "mappings":[{"clientPattern":"claude-fable-5", "upstreamModel":"claude-fable-5"}]},
            {"id":"cpa", "name":"CPA", "baseURL":"https://provider.invalid", "apiKey":"provider-key", "protocol":"openai", "enabled":true,
             "mappings":[{"clientPattern":"gpt-live-1-codex", "upstreamModel":"gpt-live-1-codex"}, {"clientPattern":"gpt-realtime", "upstreamModel":"gpt-realtime"}]}
          ]
        }"#;
    let config = AppConfig::from_json(raw)
        .expect("fixture config is valid")
        .normalized();
    let transport = Arc::new(ReplayTransport::new([
        Ok(ReplayReply {
            status: 200,
            headers: vec![("content-type".into(), "application/sdp".into())],
            chunks: vec![Ok(bytes::Bytes::from_static(b"v=0\r\n"))],
        }),
        Ok(ReplayReply {
            status: 200,
            headers: vec![("content-type".into(), "application/sdp".into())],
            chunks: vec![Ok(bytes::Bytes::from_static(b"v=0\r\n"))],
        }),
    ]));
    let engine = Engine::new(config, None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");

    let live = client
        .post(format!("http://{address}/v1/live?model=claude-fable-5"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("live bootstrap response");
    assert_eq!(live.status().as_u16(), 200);

    let realtime = client
        .post(format!("http://{address}/v1/realtime?model=claude-fable-5"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/sdp")
        .body("v=0\r\n")
        .send()
        .await
        .expect("realtime bootstrap response");
    assert_eq!(realtime.status().as_u16(), 200);

    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| call.base_url == "https://provider.invalid")
    );
    assert!(
        calls
            .iter()
            .all(|call| call.base_url != "https://anyrouter.invalid")
    );
    assert_eq!(
        calls[0].path_and_query,
        "/v1/live?model=gpt-live-1-codex&intent=quicksilver&architecture=avas"
    );
    assert_eq!(
        calls[1].path_and_query,
        "/v1/realtime?model=gpt-live-1-codex&intent=quicksilver&architecture=avas"
    );
    let live_body: serde_json::Value =
        serde_json::from_slice(&calls[0].body).expect("Codex Live JSON envelope");
    assert_eq!(live_body["session"]["model"], "gpt-live-1-codex");
    let realtime_body: serde_json::Value =
        serde_json::from_slice(&calls[1].body).expect("Codex Realtime JSON envelope");
    assert_eq!(realtime_body["session"]["model"], "gpt-live-1-codex");
    handle.shutdown().await;
}

#[tokio::test]
async fn unknown_codex_live_sideband_call_returns_session_expired() {
    let transport = Arc::new(ReplayTransport::new([]));
    let engine = Engine::new(config(), None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");
    let response = client
        .get(format!("http://{address}/v1/live/call-missing"))
        .header("Authorization", "Bearer listener-secret")
        .send()
        .await
        .expect("sideband response");
    assert_eq!(response.status().as_u16(), 410);
    let body: serde_json::Value =
        serde_json::from_slice(&response.bytes().await.expect("error body bytes"))
            .expect("error body JSON");
    assert_eq!(body["error"], "live_session_expired");
    assert!(transport.calls().is_empty());
    handle.shutdown().await;
}

#[tokio::test]
async fn cpa_resource_paths_preserve_dynamic_suffix_and_http_method() {
    let transport = Arc::new(ReplayTransport::new([Ok(ReplayReply::ok(
        br#"{"id":"file_123"}"#.to_vec(),
    ))]));
    let engine = Engine::new(
        config_with_base("https://provider.invalid", "openai"),
        None,
        transport.clone(),
    );
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
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

    handle.shutdown().await;
}

#[tokio::test]
async fn videos_create_does_not_follow_the_first_text_provider() {
    let raw = r#"{
          "schemaVersion": 6,
          "listener": {"host":"127.0.0.1", "port":0, "authToken":"listener-secret"},
          "retry": {"maxDeferredRounds":0, "sessionStickyRetries":0, "pinnedIPConcurrency":1},
          "endpoints": [
            {"id":"xiao", "name":"xiao", "baseURL":"https://anyrouter.invalid", "apiKey":"xiao-key", "protocol":"openai", "enabled":true,
             "mappings":[{"clientPattern":"claude-fable-5", "upstreamModel":"claude-fable-5"}]},
            {"id":"cpa", "name":"CPA", "baseURL":"https://provider.invalid", "apiKey":"provider-key", "protocol":"openai", "enabled":true,
             "mappings":[{"clientPattern":"grok-imagine-video", "upstreamModel":"grok-imagine-video"}, {"clientPattern":"grok-imagine-image", "upstreamModel":"grok-imagine-image"}]}
          ]
        }"#;
    let config = AppConfig::from_json(raw)
        .expect("fixture config is valid")
        .normalized();
    let transport = Arc::new(ReplayTransport::new([
        Ok(ReplayReply::ok(br#"{"id":"video_abc"}"#.to_vec())),
        Ok(ReplayReply::ok(b"video-bytes".to_vec())),
    ]));
    let engine = Engine::new(config, None, transport.clone());
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .expect("bind server");
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("http client");

    let created = client
        .post(format!("http://{address}/v1/videos"))
        .header("Authorization", "Bearer listener-secret")
        .header("Content-Type", "application/json")
        .body(r#"{"prompt":"a cat","model":"grok-imagine-video"}"#)
        .send()
        .await
        .expect("video create");
    assert_eq!(created.status().as_u16(), 200);

    let content = client
        .get(format!("http://{address}/v1/videos/video_abc/content"))
        .header("Authorization", "Bearer listener-secret")
        .send()
        .await
        .expect("video content");
    assert_eq!(content.status().as_u16(), 200);

    let calls = transport.calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|call| call.base_url == "https://provider.invalid")
    );
    assert_eq!(calls[0].path_and_query, "/v1/videos");
    assert_eq!(calls[1].path_and_query, "/v1/videos/video_abc/content");
    handle.shutdown().await;
}
