use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use sumpter_core::config::AppConfig;
use tokio_tungstenite::tungstenite::{Message, protocol::Role};

#[path = "../support/tls.rs"]
mod tls;

#[tokio::test]
async fn untrusted_upstream_certificate_is_rejected_by_https_wss_and_model_probe() {
    use sumpter_engine::outbound::{OutboundRequest, ReqwestTransport, UpstreamTransport};
    let certificates = tls::CertificateFiles::new();
    let port = tls::free_port();
    let mut upstream_config = AppConfig::bootstrap();
    upstream_config.listener.auth_token = "listener-secret".into();
    upstream_config.listener.tls = Some(certificates.settings(i64::from(port)));
    let (_, upstream) = server::serve(
        pi_test_engine(upstream_config),
        "127.0.0.1:0".parse().unwrap(),
    )
    .await
    .unwrap();
    let base = format!("https://127.0.0.1:{port}");
    // The fixture works when explicitly trusted by the test client.
    assert_eq!(
        certificates
            .client()
            .get(format!("{base}/v1/models"))
            .bearer_auth("listener-secret")
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let transport = ReqwestTransport::new();
    assert!(
        transport
            .send_streaming(
                OutboundRequest {
                    method: "GET".into(),
                    base_url: base.clone(),
                    resolve_ip: String::new(),
                    path_and_query: "/v1/models".into(),
                    headers: vec![],
                    body: bytes::Bytes::new(),
                    keep_alive: false,
                },
                Some(std::time::Duration::from_secs(3))
            )
            .await
            .is_err()
    );
    let config = AppConfig::from_json(&serde_json::json!({
        "schemaVersion":7,
        "listener":{"host":"127.0.0.1","authToken":"listener-secret"},
        "retry":{"maxDeferredRounds":0,"sessionStickyRetries":0},
        "endpoints":[{"id":"tls","name":"TLS","baseURL":base,"apiKey":"synthetic-key","protocol":"openai-responses","enabled":true,"capabilities":["live"],
            "mappings":[{"clientPattern":"gpt-4o","upstreamModel":"gpt-4o"}]}]
    }).to_string()).unwrap().normalized();
    let probe =
        sumpter_engine::model_catalog::ProviderCatalogFetcher::fetch(&config.endpoints[0]).await;
    assert!(probe.error.is_some());
    assert!(probe.models.is_empty());
    let engine = pi_test_engine(config);
    assert!(
        engine
            .prepare_realtime_websocket(None, "/v1/realtime?model=gpt-4o", &[])
            .await
            .is_err()
    );
    pi_test_shutdown(upstream).await;
}

#[tokio::test]
async fn http_and_https_share_auth_and_tls_does_not_block_http_accepts() {
    let certificates = tls::CertificateFiles::new();
    let mut config = AppConfig::bootstrap();
    config.listener.auth_token = "listener-secret".into();
    let https_port = tls::free_port();
    config.listener.tls = Some(certificates.settings(i64::from(https_port)));
    let (http, handle) = server::serve(pi_test_engine(config), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    // This unfinished TLS handshake must not serialize other accepts.
    let _slow_peer = tokio::net::TcpStream::connect(("127.0.0.1", https_port))
        .await
        .unwrap();
    let client = certificates.client();
    for base in [
        format!("http://{http}"),
        format!("https://127.0.0.1:{https_port}"),
    ] {
        assert_eq!(
            client
                .get(format!("{base}/v1/models"))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(
            client
                .get(format!("{base}/v1/models"))
                .bearer_auth("listener-secret")
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
    }
    assert!(
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(format!("https://127.0.0.1:{https_port}/v1/models"))
            .send()
            .await
            .is_err()
    );
    pi_test_shutdown(handle).await;
}

#[tokio::test]
async fn wss_uses_the_existing_realtime_relay() {
    let certificates = tls::CertificateFiles::new();
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = upstream.local_addr().unwrap();
    let upstream_task = tokio::spawn(async move {
        let (stream, _) = upstream.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        while let Some(Ok(message)) = ws.next().await {
            if message.is_close() {
                break;
            }
            ws.send(message).await.unwrap();
        }
    });
    let mut config = AppConfig::from_json(&serde_json::json!({
        "schemaVersion": 7,
        "listener": {"host": "127.0.0.1", "authToken": "listener-secret"},
        "endpoints": [{"id":"cpa", "name":"CPA", "baseURL":format!("http://{upstream_address}"),
            "apiKey":"provider-secret", "protocol":"openai", "enabled":true, "capabilities":["live"], "mappings":[]}],
        "modelGroups":[{"id":"default", "name":"default", "enabled":true, "priority":0, "models":[],
            "bindings":[{"endpointID":"cpa", "enabled":true, "priority":0, "models":[]}]}]
    }).to_string()).unwrap();
    let port = tls::free_port();
    config.listener.tls = Some(certificates.settings(i64::from(port)));
    let (_, handle) = server::serve(pi_test_engine(config), "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let response = certificates
        .client()
        .get(format!(
            "https://127.0.0.1:{port}/v1/realtime?model=gpt-realtime-2.1"
        ))
        .bearer_auth("listener-secret")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 101);
    let stream = response.upgrade().await.unwrap();
    let mut ws =
        tokio_tungstenite::WebSocketStream::from_raw_socket(stream, Role::Client, None).await;
    for message in [
        Message::Text("tls relay".into()),
        Message::Binary(vec![1, 2, 3].into()),
    ] {
        ws.send(message.clone()).await.unwrap();
        assert_eq!(ws.next().await.unwrap().unwrap(), message);
    }
    let _ = ws.close(None).await;
    pi_test_shutdown(handle).await;
    upstream_task.await.unwrap();
}

#[test]
fn self_signed_certificate_loads_but_malformed_and_mismatched_keys_do_not() {
    let certificates = tls::CertificateFiles::new();
    let other = tls::CertificateFiles::new();
    let mut config = AppConfig::bootstrap();
    config.listener.tls = Some(certificates.settings(57880));
    assert!(
        sumpter_engine::inbound_tls::load_listener_tls(&config.listener)
            .unwrap()
            .is_some()
    );
    config.listener.tls.as_mut().unwrap().private_key_path =
        other.key.to_string_lossy().into_owned();
    assert!(sumpter_engine::inbound_tls::load_listener_tls(&config.listener).is_err());
    std::fs::write(&other.key, "synthetic-invalid-private-key").unwrap();
    let error = sumpter_engine::inbound_tls::load_listener_tls(&config.listener)
        .err()
        .unwrap()
        .to_string();
    assert!(!error.contains("synthetic-invalid-private-key"));
    config.listener.tls.as_mut().unwrap().enabled = false;
    assert!(
        sumpter_engine::inbound_tls::load_listener_tls(&config.listener)
            .unwrap()
            .is_none()
    );
}
