//! Native compressed responses must retain wire bytes while recovering semantics.
use super::*;
use std::io::Write;

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(body).unwrap();
    encoder.finish().unwrap()
}

fn config(protocol: EndpointProtocolMode) -> AppConfig {
    let mut config = two_endpoint_config();
    config.endpoints.truncate(1);
    config.endpoints[0].protocol = protocol;
    config.endpoints[0].mappings[0].client_pattern = "*".into();
    config.endpoints[0].mappings[0].upstream_model = "upstream-model".into();
    config
}

const GEMINI: &str = concat!(
    "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"},{\"functionCall\":{\"name\":\"read_file\",\"args\":{}}}]},\"finishReason\":\"STOP\"}]}\n\n",
    "data: {\"usageMetadata\":{\"promptTokenCount\":20,\"candidatesTokenCount\":8,\"thoughtsTokenCount\":3,\"cachedContentTokenCount\":0}}\n\n"
);

fn gemini_body() -> Bytes {
    Bytes::from_static(br#"{"contents":[{"role":"user","parts":[{"text":"hi"}]}]}"#)
}

struct InterruptedCompressed {
    prefix: Vec<u8>,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl UpstreamTransport for InterruptedCompressed {
    async fn send_streaming(
        &self,
        _request: OutboundRequest,
        _timeout: Option<Duration>,
    ) -> Result<UpstreamResponse, TransportError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(UpstreamResponse {
            status: 200,
            headers: vec![
                ("content-type".into(), "text/event-stream".into()),
                ("content-encoding".into(), "gzip".into()),
            ],
            stream: futures_util::stream::iter(vec![
                Ok(Bytes::from(self.prefix.clone())),
                Err(TransportError::ConnectionFailed("synthetic reset".into())),
            ])
            .boxed(),
        })
    }
}

#[tokio::test]
async fn actual_transport_interruption_takes_precedence_over_decoder_eof() {
    let wire = gzip(GEMINI.as_bytes());
    let prefix = wire[..wire.len() - 8].to_vec();
    let transport = Arc::new(InterruptedCompressed {
        prefix: prefix.clone(),
        calls: std::sync::atomic::AtomicUsize::new(0),
    });
    let engine = engine_with_transport(
        config(EndpointProtocolMode::Gemini).normalized(),
        transport.clone(),
    );
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
            vec![],
            gemini_body(),
        )
        .await;
    let mut stream = response.into_body().into_data_stream();
    let mut returned = Vec::new();
    let mut failed = false;
    while let Some(item) = stream.next().await {
        match item {
            Ok(bytes) => returned.extend_from_slice(&bytes),
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    assert!(failed);
    assert_eq!(returned, prefix);
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert_eq!(
        event.failure_kind,
        Some(RuntimeFailureKind::StreamInterrupted)
    );
    assert!(
        event
            .failure_detail
            .as_deref()
            .unwrap()
            .contains("synthetic reset")
    );
    assert_eq!(transport.calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn native_gzip_sse_retains_trailer_and_observes_all_dialects() {
    for (protocol, path, request, response) in [
        (
            EndpointProtocolMode::Gemini,
            "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
            gemini_body(),
            GEMINI,
        ),
        (
            EndpointProtocolMode::Anthropic,
            "/v1/messages",
            body(),
            "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":20}}}\n\nevent: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":11}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
        ),
        (
            EndpointProtocolMode::OpenAI,
            "/v1/chat/completions",
            Bytes::from_static(
                br#"{"model":"demo","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
            ),
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":20,\"completion_tokens\":11}}\n\ndata: [DONE]\n\n",
        ),
        (
            EndpointProtocolMode::OpenAIResponses,
            "/v1/responses",
            Bytes::from_static(br#"{"model":"demo","stream":true,"input":"hi"}"#),
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":20,\"output_tokens\":11}}}\n\n",
        ),
    ] {
        for split in [1, 7, 65536] {
            let fake = FakeTransport::new();
            let wire = gzip(response.as_bytes());
            fake.push(
                "a.example.com",
                Outcome::Status {
                    status: 200,
                    headers: vec![
                        ("content-type".into(), "text/event-stream".into()),
                        ("content-encoding".into(), "gzip".into()),
                    ],
                    chunks: wire.chunks(split).map(<[u8]>::to_vec).collect(),
                },
            );
            let engine = engine_with(config(protocol), fake.clone());
            let response = engine
                .handle_request(
                    loopback(),
                    "POST",
                    path,
                    vec![("accept-encoding".into(), "br, gzip, deflate".into())],
                    request.clone(),
                )
                .await;
            assert_eq!(response.status(), 200);
            assert_eq!(response.headers()["content-encoding"], "gzip");
            assert_eq!(
                axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap()
                    .as_ref(),
                wire
            );
            let runtime = engine.runtime_snapshot();
            let event = runtime
                .recent_events
                .iter()
                .find(|e| e.kind == "client")
                .unwrap();
            assert!(event.is_succeeded(), "{protocol:?}/{split}: {event:?}");
            let trace = event.stream_trace.as_ref().unwrap();
            let usage = trace.usage.as_ref().unwrap();
            assert_eq!(usage.input_tokens, Some(20));
            assert_eq!(usage.output_tokens, Some(11));
            assert_eq!(trace.terminal_event.as_deref(), Some("completed"));
            assert_eq!(fake.requests().len(), 1);
            if protocol == EndpointProtocolMode::Gemini {
                assert_eq!(usage.reasoning_tokens, Some(3));
                assert_eq!(usage.cache_read_input_tokens, Some(0));
                assert!(
                    event
                        .tool_calls
                        .as_ref()
                        .unwrap()
                        .iter()
                        .any(|name| name == "read_file")
                );
                assert!(trace.cache_read_evidence.as_ref().unwrap().complete);
            }
        }
    }
}

#[tokio::test]
async fn gzip_json_completion_and_binary_capture_round_trip() {
    use base64::Engine as _;
    use sumpter_core::events::DiagnosticDataEncoding;
    let fake = FakeTransport::new();
    let wire = gzip(br#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":20,"candidatesTokenCount":11}}"#);
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("content-encoding".into(), "gzip".into()),
            ],
            chunks: wire.chunks(3).map(<[u8]>::to_vec).collect(),
        },
    );
    let dir = temp_config_dir("gzip-capture");
    let engine = engine_with_dir(config(EndpointProtocolMode::Gemini), dir.clone(), fake);
    engine.set_diagnostic_capture(true, None);
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1beta/models/gemini-test:generateContent",
            vec![],
            gemini_body()
        )
        .await
        .1,
        wire
    );
    engine.set_diagnostic_capture(false, None);
    engine.flush_diagnostic_capture_if_dirty();
    let snapshot = engine.diagnostic_capture_snapshot();
    let record = &snapshot.records[0];
    assert_eq!(record.outcome, Some(RuntimeEventOutcome::Succeeded));
    for chunks in [&record.attempts[0].upstream_chunks, &record.client_chunks] {
        let mut restored = Vec::new();
        for chunk in chunks {
            assert_eq!(chunk.data_encoding, DiagnosticDataEncoding::Base64);
            restored.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk.data.as_bytes())
                    .unwrap(),
            );
        }
        assert_eq!(restored, wire);
    }
    let loaded = dir.load_diagnostic_capture().unwrap();
    let mut persisted = loaded.records[0].clone();
    // JSON's decimal float parser may round fractional wall-clock seconds by
    // one ULP. Payloads, integer timings and all remaining metadata stay exact.
    assert!(
        (persisted.timestamp - record.timestamp).abs() <= record.timestamp.abs() * f64::EPSILON
    );
    persisted.timestamp = record.timestamp;
    assert_eq!(persisted, *record);
    drop(engine);
    let _ = std::fs::remove_dir_all(dir.root);
}

#[tokio::test]
async fn compressed_upstream_is_decoded_before_protocol_conversion() {
    let fake = FakeTransport::new();
    let wire = gzip(GEMINI.as_bytes());
    fake.push(
        "a.example.com",
        Outcome::Status {
            status: 200,
            headers: vec![
                ("content-type".into(), "text/event-stream".into()),
                ("content-encoding".into(), "gzip".into()),
            ],
            chunks: wire.chunks(3).map(<[u8]>::to_vec).collect(),
        },
    );
    let engine = engine_with(config(EndpointProtocolMode::Gemini), fake.clone());
    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    assert!(!response.headers().contains_key("content-encoding"));
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("message_stop"));
    assert!(text.contains("read_file"));
    let runtime = engine.runtime_snapshot();
    let event = runtime
        .recent_events
        .iter()
        .find(|e| e.kind == "client")
        .unwrap();
    assert!(event.is_succeeded(), "{event:?}");
    assert_eq!(
        event
            .stream_trace
            .as_ref()
            .unwrap()
            .usage
            .as_ref()
            .unwrap()
            .output_tokens,
        Some(11)
    );
    assert_eq!(fake.requests().len(), 1);
}

#[tokio::test]
async fn gzip_observation_failures_keep_wire_and_do_not_retry() {
    let valid = gzip(GEMINI.as_bytes());
    let mut corrupt = valid.clone();
    let crc = corrupt.len() - 8;
    corrupt[crc] ^= 0x80;
    for (encoding, wire, detail, kind) in [
        (
            "gzip",
            corrupt,
            "invalid_compressed_data",
            RuntimeFailureKind::ResponseObservationFailed,
        ),
        (
            "gzip",
            valid[..valid.len() - 4].to_vec(),
            "truncated_compressed_data",
            RuntimeFailureKind::ResponseObservationFailed,
        ),
        (
            "unknown",
            valid,
            "unsupported_content_encoding",
            RuntimeFailureKind::ResponseObservationFailed,
        ),
        (
            "gzip",
            gzip(
                b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n",
            ),
            "before a protocol terminal event",
            RuntimeFailureKind::StreamInterrupted,
        ),
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
                chunks: wire.chunks(5).map(<[u8]>::to_vec).collect(),
            },
        );
        let engine = engine_with(config(EndpointProtocolMode::Gemini), fake.clone());
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1beta/models/gemini-test:streamGenerateContent?alt=sse",
                vec![],
                gemini_body()
            )
            .await
            .1,
            wire
        );
        let runtime = engine.runtime_snapshot();
        let event = runtime
            .recent_events
            .iter()
            .find(|e| e.kind == "client")
            .unwrap();
        assert_eq!(event.failure_kind, Some(kind));
        assert!(
            event.failure_detail.as_deref().unwrap().contains(detail),
            "{event:?}"
        );
        assert_eq!(event.stream_trace.as_ref().unwrap().output_span_ms, None);
        assert_eq!(fake.requests().len(), 1);
        if kind == RuntimeFailureKind::ResponseObservationFailed {
            let evidence = event
                .stream_trace
                .as_ref()
                .unwrap()
                .cache_read_evidence
                .as_ref()
                .unwrap();
            assert!(!evidence.complete);
            assert_ne!(
                evidence.finality,
                sumpter_core::cache_read::CacheReadFinality::Confirmed
            );
        }
    }
}
