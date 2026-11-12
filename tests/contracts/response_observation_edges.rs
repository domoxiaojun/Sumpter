//! Classification, client-close and bridge-error boundaries shared by both adapters.
use super::*;
use std::io::Write;

const GEMINI_SSE: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]},\"finishReason\":\"STOP\"}]}\n\n",
    "data: {\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":11}}\n\n"
);
const GEMINI_JSON: &str = "{\"candidates\":[{\"finishReason\":\"STOP\"}],\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":11}}";

fn config(protocol: EndpointProtocolMode) -> AppConfig {
    let mut c = two_endpoint_config();
    c.endpoints.truncate(1);
    c.endpoints[0].protocol = protocol;
    c.endpoints[0].mappings[0].client_pattern = "*".into();
    c.endpoints[0].mappings[0].upstream_model = "upstream-model".into();
    c
}

fn gemini_request() -> Bytes {
    Bytes::from_static(br#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#)
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

async fn next_chunk<S, E>(stream: &mut S) -> Bytes
where
    S: futures_util::Stream<Item = Result<Bytes, E>> + Unpin,
    E: std::fmt::Debug,
{
    tokio::time::timeout(Duration::from_secs(2), stream.next())
        .await
        .expect("response chunk must not wait for HTTP EOF")
        .expect("response must not end before its chunk")
        .expect("response chunk must be valid")
}

type StreamItem = Result<Bytes, TransportError>;
type StreamSender = tokio::sync::mpsc::UnboundedSender<StreamItem>;
type StreamReceiver = tokio::sync::mpsc::UnboundedReceiver<StreamItem>;

struct GatedResponse {
    receiver: Mutex<Option<StreamReceiver>>,
    mime: &'static str,
    calls: std::sync::atomic::AtomicUsize,
}

impl GatedResponse {
    fn new(mime: &'static str) -> (Arc<Self>, StreamSender) {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        (
            Arc::new(Self {
                receiver: Mutex::new(Some(receiver)),
                mime,
                calls: std::sync::atomic::AtomicUsize::new(0),
            }),
            sender,
        )
    }
}

#[async_trait::async_trait]
impl UpstreamTransport for GatedResponse {
    async fn send_streaming(
        &self,
        _request: OutboundRequest,
        _timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let receiver = self.receiver.lock().unwrap().take().expect("no replay");
        Ok(UpstreamResponse {
            status: 200,
            headers: vec![
                ("content-type".into(), self.mime.into()),
                ("content-encoding".into(), "gzip".into()),
            ],
            stream: futures_util::stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|chunk| (chunk, receiver))
            })
            .boxed(),
        })
    }
}

#[tokio::test]
async fn gemini_identity_is_independent_of_native_protocol_and_early_rejection() {
    for (path, mime, response) in [
        (
            "/v1beta/models/gemini-test:generateContent",
            "application/json",
            GEMINI_JSON,
        ),
        (
            "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
            "text/event-stream",
            GEMINI_SSE,
        ),
        (
            "/v1/models/gemini-test:countTokens",
            "application/json",
            "{\"totalTokens\":3}",
        ),
    ] {
        let mut forwarded = Vec::new();
        for (ua, expected) in [
            (None, ClientKind::Unknown),
            (Some("google-genai-sdk/1.0"), ClientKind::Unknown),
            (Some("GeminiCLI/0.1.0"), ClientKind::GeminiCli),
        ] {
            for rejection in [0, 401, 403] {
                let fake = FakeTransport::new();
                fake.push(
                    "a.example.com",
                    Outcome::Status {
                        status: 200,
                        headers: vec![("content-type".into(), mime.into())],
                        chunks: vec![response.as_bytes().to_vec()],
                    },
                );
                let mut cfg = config(EndpointProtocolMode::Gemini);
                if rejection == 401 {
                    cfg.listener.auth_token = "synthetic-inbound-token".into();
                } else if rejection == 403 {
                    cfg.listener.allowed_cidrs = vec!["192.0.2.0/24".into()];
                }
                let engine = engine_with(cfg, fake.clone());
                let mut headers: Vec<(String, String)> = ua
                    .map(|ua| vec![("user-agent".into(), ua.into())])
                    .unwrap_or_default();
                headers.push(("originator".into(), "generic-proxy".into()));
                let request = gemini_request();
                let remote = if rejection == 403 {
                    Some("203.0.113.10".parse().unwrap())
                } else {
                    loopback()
                };
                let (status, _) = call(&engine, remote, path, headers, request).await;
                assert_eq!(
                    status,
                    if rejection > 0 { rejection } else { 200 },
                    "{path}"
                );
                let runtime = engine.runtime_snapshot();
                let client = runtime
                    .recent_events
                    .iter()
                    .find(|event| event.kind == "client")
                    .unwrap();
                assert_eq!(
                    client.client_kind,
                    Some(expected),
                    "{path}/{ua:?}/{rejection}"
                );
                assert_eq!(client.source_format, Some(ProviderProtocol::Gemini));
                if rejection > 0 {
                    assert!(fake.requests().is_empty());
                } else {
                    assert!(client.is_succeeded(), "{client:?}");
                    assert_eq!(client.target_format, Some(ProviderProtocol::Gemini));
                    let requests = fake.requests();
                    let sent = &requests[0];
                    forwarded.push((sent.path.clone(), sent.body.clone()));
                }
            }
        }
        assert!(forwarded.windows(2).all(|pair| pair[0] == pair[1]));
    }
}

#[tokio::test]
async fn validated_terminal_then_client_close_is_not_a_499() {
    for (protocol, path, request, mime, payload) in [
        (
            EndpointProtocolMode::Gemini,
            "/v1beta/models/gemini-test:generateContent",
            gemini_request(),
            "application/json",
            GEMINI_JSON,
        ),
        (
            EndpointProtocolMode::Gemini,
            "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
            gemini_request(),
            "text/event-stream",
            GEMINI_SSE,
        ),
        (
            EndpointProtocolMode::Anthropic,
            "/v1/messages",
            body(),
            "text/event-stream",
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
        (
            EndpointProtocolMode::OpenAI,
            "/v1/chat/completions",
            Bytes::from_static(
                br#"{"model":"demo","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
            ),
            "text/event-stream",
            "data: [DONE]\n\n",
        ),
        (
            EndpointProtocolMode::OpenAIResponses,
            "/v1/responses",
            Bytes::from_static(br#"{"model":"demo","stream":true,"input":"hi"}"#),
            "text/event-stream",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
        ),
    ] {
        for split_trailer in [false, true] {
            let (transport, sender) = GatedResponse::new(mime);
            let engine = engine_with_transport(config(protocol).normalized(), transport.clone());
            let response = engine
                .handle_request(loopback(), "POST", path, vec![], request.clone())
                .await;
            assert_eq!(response.status(), 200, "{protocol:?}/{split_trailer}");
            let mut stream = response.into_body().into_data_stream();
            let wire = gzip(payload.as_bytes());
            let chunks = if split_trailer {
                vec![&wire[..wire.len() - 8], &wire[wire.len() - 8..]]
            } else {
                vec![wire.as_slice()]
            };
            for chunk in chunks {
                sender.send(Ok(Bytes::copy_from_slice(chunk))).unwrap();
                assert_eq!(next_chunk(&mut stream).await.as_ref(), chunk);
            }
            drop(stream); // sender remains alive: HTTP EOF has not arrived.
            let runtime = engine.runtime_snapshot();
            let event = runtime
                .recent_events
                .iter()
                .find(|event| event.kind == "client")
                .unwrap();
            assert!(
                event.is_succeeded(),
                "{protocol:?}/{split_trailer}: {event:?}"
            );
            assert_eq!(event.status_code, 200);
            let trace = event.stream_trace.as_ref().unwrap();
            assert_eq!(trace.terminal_event.as_deref(), Some("completed"));
            assert!(!trace.cache_read_evidence.as_ref().unwrap().complete);
            assert_eq!(trace.output_span_ms, None);
            assert_eq!(runtime.client_requests, 1);
            assert_eq!(runtime.client_successes, 1);
            assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
        }
    }
}

#[tokio::test]
async fn client_close_distinguishes_unverified_tail_corruption_and_mid_stream_cancel() {
    for (payload, corrupt, terminal) in [
        (GEMINI_SSE, false, true),
        (GEMINI_SSE, true, true),
        (
            "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
            false,
            false,
        ),
    ] {
        let (transport, sender) = GatedResponse::new("text/event-stream");
        let engine =
            engine_with_transport(config(EndpointProtocolMode::Gemini).normalized(), transport);
        let response = engine
            .handle_request(
                loopback(),
                "POST",
                "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
                vec![],
                gemini_request(),
            )
            .await;
        let mut stream = response.into_body().into_data_stream();
        let mut wire = gzip(payload.as_bytes());
        let crc = wire.len() - 8;
        sender
            .send(Ok(Bytes::copy_from_slice(&wire[..crc])))
            .unwrap();
        next_chunk(&mut stream).await;
        if corrupt {
            wire[crc] ^= 1;
            sender
                .send(Ok(Bytes::copy_from_slice(&wire[crc..])))
                .unwrap();
            next_chunk(&mut stream).await;
        }
        drop(stream);
        let runtime = engine.runtime_snapshot();
        let event = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .unwrap();
        if terminal {
            assert_eq!(event.status_code, 200);
            assert_eq!(
                event.failure_kind,
                Some(RuntimeFailureKind::ResponseObservationFailed)
            );
            assert_eq!(
                event
                    .stream_trace
                    .as_ref()
                    .unwrap()
                    .terminal_event
                    .as_deref(),
                Some("completed")
            );
            assert!(
                event
                    .failure_detail
                    .as_deref()
                    .unwrap()
                    .contains(if corrupt {
                        "invalid_compressed_data"
                    } else {
                        "compressed_tail_unverified_after_terminal"
                    }),
                "{event:?}"
            );
        } else {
            assert_eq!(event.status_code, 499);
            assert_eq!(event.outcome, Some(RuntimeEventOutcome::Cancelled));
        }
    }
}

#[tokio::test]
async fn validated_incomplete_terminal_stays_failed_on_client_close() {
    let (transport, sender) = GatedResponse::new("text/event-stream");
    let engine =
        engine_with_transport(config(EndpointProtocolMode::Gemini).normalized(), transport);
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
            vec![],
            gemini_request(),
        )
        .await;
    let mut stream = response.into_body().into_data_stream();
    let wire = gzip(b"data: {\"candidates\":[{\"finishReason\":\"MAX_TOKENS\"}]}\n\n");
    sender.send(Ok(Bytes::from(wire))).unwrap();
    next_chunk(&mut stream).await;
    drop(stream);
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert!(event.is_failed());
    assert_eq!(event.status_code, 200);
    assert_eq!(
        event
            .stream_trace
            .as_ref()
            .unwrap()
            .terminal_event
            .as_deref(),
        Some("incomplete")
    );
}

#[tokio::test]
async fn bridge_decode_errors_reach_both_json_and_sse_consumers() {
    let valid = gzip(GEMINI_SSE.as_bytes());
    let mut corrupt = valid.clone();
    let crc = corrupt.len() - 8;
    corrupt[crc] ^= 1;
    for streaming in [false, true] {
        for (encoding, wire) in [
            ("unknown", valid.clone()),
            ("gzip", b"not gzip".to_vec()),
            ("gzip", corrupt.clone()),
            ("gzip", valid[..valid.len() - 4].to_vec()),
        ] {
            let fake = FakeTransport::new();
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status: 200,
                    headers: vec![
                        ("content-type".into(), "text/event-stream".into()),
                        ("content-encoding".into(), encoding.into()),
                    ],
                    chunks: wire.chunks(7).map(<[u8]>::to_vec).collect(),
                },
            );
            let engine = engine_with(config(EndpointProtocolMode::Gemini), fake.clone());
            let mut request: Value = serde_json::from_slice(&body()).unwrap();
            request["stream"] = json!(streaming);
            let response = engine
                .handle_request(
                    loopback(),
                    "POST",
                    "/v1/messages",
                    vec![],
                    Bytes::from(request.to_string()),
                )
                .await;
            let mut stream = response.into_body().into_data_stream();
            let mut delivered = Vec::new();
            let mut failed = false;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => delivered.extend_from_slice(&chunk),
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            assert!(failed, "{encoding}/{streaming}");
            assert!(!String::from_utf8_lossy(&delivered).contains("message_stop"));
            if !streaming {
                assert!(delivered.is_empty());
            }
            let runtime = engine.runtime_snapshot();
            let event = runtime
                .recent_events
                .iter()
                .find(|event| event.kind == "client")
                .unwrap();
            assert_eq!(
                event.failure_kind,
                Some(RuntimeFailureKind::ResponseObservationFailed)
            );
            assert_eq!(fake.requests().len(), 1);
        }
    }
}

#[tokio::test]
async fn converted_prefix_does_not_count_as_a_delivered_terminal() {
    let (transport, sender) = GatedResponse::new("text/event-stream");
    let engine =
        engine_with_transport(config(EndpointProtocolMode::OpenAI).normalized(), transport);
    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    let frame = format!(
        "data: {}\n\n",
        json!({"choices":[{"delta":{"content":"x".repeat(1024)},"finish_reason":null}]})
    );
    let payload = format!("{}data: [DONE]\n\n", frame.repeat(100));
    sender
        .send(Ok(Bytes::from(gzip(payload.as_bytes()))))
        .unwrap();
    let mut stream = response.into_body().into_data_stream();
    let prefix = next_chunk(&mut stream).await;
    assert!(!String::from_utf8_lossy(&prefix).contains("message_stop"));
    drop(stream);
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|event| event.kind == "client")
        .unwrap();
    assert_eq!(event.status_code, 499);
    assert_eq!(event.outcome, Some(RuntimeEventOutcome::Cancelled));
}

#[tokio::test]
async fn converted_success_waits_for_compression_validation() {
    for corrupt in [false, true] {
        let (transport, sender) = GatedResponse::new("text/event-stream");
        let engine =
            engine_with_transport(config(EndpointProtocolMode::OpenAI).normalized(), transport);
        let response = engine
            .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
            .await;
        let mut stream = response.into_body().into_data_stream();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n").unwrap();
        encoder.flush().unwrap();
        let prefix = encoder.get_ref().clone();
        sender.send(Ok(Bytes::from(prefix.clone()))).unwrap();
        let mut delivered = next_chunk(&mut stream).await.to_vec();
        assert!(String::from_utf8_lossy(&delivered).contains("hello"));
        encoder.write_all(b"data: [DONE]\n\n").unwrap();
        encoder.flush().unwrap();
        let terminal_end = encoder.get_ref().len();
        sender
            .send(Ok(Bytes::copy_from_slice(
                &encoder.get_ref()[prefix.len()..],
            )))
            .unwrap();
        let mut wire = encoder.finish().unwrap();
        if corrupt {
            let crc = wire.len() - 8;
            wire[crc] ^= 1;
        }
        sender
            .send(Ok(Bytes::copy_from_slice(&wire[terminal_end..])))
            .unwrap();
        drop(sender);
        let mut failed = false;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => delivered.extend_from_slice(&chunk),
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        assert_eq!(failed, corrupt);
        assert_eq!(
            String::from_utf8_lossy(&delivered).contains("message_stop"),
            !corrupt
        );
        let runtime = engine.runtime_snapshot();
        let event = runtime
            .recent_events
            .iter()
            .find(|event| event.kind == "client")
            .unwrap();
        assert_eq!(event.is_succeeded(), !corrupt);
        if corrupt {
            assert_eq!(
                event.failure_kind,
                Some(RuntimeFailureKind::ResponseObservationFailed)
            );
        }
    }
}
