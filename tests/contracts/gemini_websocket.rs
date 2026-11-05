//! Native Gemini Live uses Google's setup frames and credential surface.
use super::{pi_test_engine, pi_test_shutdown, server};
use futures_util::{SinkExt, StreamExt};
use sumpter_core::config::AppConfig;
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Message, client::IntoClientRequest};

#[tokio::test]
async fn gemini_live_handshake_and_frames_remain_native() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_address = listener.local_addr().unwrap();
    let path = "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
    let setup = r#"{"setup":{"model":"models/gemini-live-native","generationConfig":{"responseModalities":["AUDIO"]}}}"#;
    let upstream = tokio::spawn(async move {
        #[allow(clippy::result_large_err)]
        let mut socket = accept_hdr_async(
            listener.accept().await.unwrap().0,
            move |request: &Request, response: Response| {
                assert_eq!(request.uri().path(), path);
                assert_eq!(request.headers()["x-goog-api-key"], "provider-secret");
                assert!(!request.headers().contains_key("authorization"));
                assert!(!request.headers().contains_key("x-api-key"));
                assert!(!request.headers().contains_key("x-sumpter-endpoint-id"));
                Ok(response)
            },
        )
        .await
        .unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Text(setup.into())
        );
        socket
            .send(Message::Text(r#"{"setupComplete":{}}"#.into()))
            .await
            .unwrap();
        socket
            .send(Message::Binary(vec![0, 1, 2, 255].into()))
            .await
            .unwrap();
        let _ = socket.close(None).await;
    });
    let mut config = AppConfig::from_json(&serde_json::json!({
        "listener":{"host":"127.0.0.1","port":0,"authToken":"listener-secret"},
        "endpoints":[{"id":"gemini","baseURL":format!("http://{upstream_address}/v1beta"),"protocol":"gemini","apiKey":"provider-secret","enabled":true,"capabilities":["live"],"mappings":[]}]
    }).to_string()).unwrap().normalized();
    let mut text_only = config.endpoints[0].clone();
    text_only.id = "text-only".into();
    text_only.capabilities.clear();
    config.endpoints.push(text_only);
    let engine = pi_test_engine(config);
    let (address, handle) = server::serve(engine, "127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let mut request = format!("ws://{address}{path}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("x-goog-api-key", "listener-secret".parse().unwrap());
    let (mut socket, _) = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .unwrap()
    .unwrap();
    socket.send(Message::Text(setup.into())).await.unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Text(r#"{"setupComplete":{}}"#.into())
    );
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Binary(vec![0, 1, 2, 255].into())
    );
    let _ = socket.next().await;
    upstream.await.unwrap();
    pi_test_shutdown(handle).await;
}
