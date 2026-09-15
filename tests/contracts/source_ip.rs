//! Both adapters must retain the network peer across request lifecycle boundaries.
use super::*;

fn assert_source_ip(engine: &Engine, expected: Option<&str>, count: usize) {
    let events = engine.runtime_snapshot().recent_events;
    assert_eq!(events.len(), count);
    for event in events {
        assert_eq!(event.source_ip.as_deref(), expected, "{}", event.kind);
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire.get("sourceIP").and_then(Value::as_str), expected);
    }
}

#[tokio::test]
async fn source_ip_survives_retries_stream_completion_and_spoofed_headers() {
    for ip in ["127.0.0.1", "::1"] {
        let fake = FakeTransport::new();
        fake.push(
            "a.example.com",
            Outcome::Status {
                status: 503,
                headers: vec![],
                chunks: vec![],
            },
        );
        fake.push("b.example.com", sse_ok(&["data: {\"type\":\"ping\"}\n\n"]));
        let engine = engine_with(two_endpoint_config(), fake);
        let headers = vec![
            ("x-forwarded-for".into(), "198.51.100.99".into()),
            ("forwarded".into(), "for=198.51.100.99".into()),
            ("x-real-ip".into(), "198.51.100.99".into()),
        ];
        let (status, _) = call(
            &engine,
            Some(ip.parse().unwrap()),
            "/v1/messages",
            headers,
            body(),
        )
        .await;
        assert_eq!(status, 200);
        assert_source_ip(&engine, Some(ip), 3);
    }
}

#[tokio::test]
async fn source_ip_survives_cancellation_outside_inbound_scope() {
    let fake = FakeTransport::new();
    fake.push("a.example.com", Outcome::Gated { status: 200 });
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let response = engine
        .handle_request(loopback(), "POST", "/v1/messages", vec![], body())
        .await;
    let mut stream = response.into_body().into_data_stream();
    fake.gate_sender()
        .send(Ok(Bytes::from_static(b"data: chunk\n\n")))
        .unwrap();
    stream.next().await.unwrap().unwrap();
    assert_source_ip(&engine, Some("127.0.0.1"), 2);
    drop(stream);
    let events = engine.runtime_snapshot().recent_events;
    assert!(
        events
            .iter()
            .any(|event| event.outcome == Some(RuntimeEventOutcome::Cancelled))
    );
    assert_source_ip(&engine, Some("127.0.0.1"), 2);
}

#[tokio::test]
async fn source_ip_records_rejection_and_leaves_unknown_peer_absent() {
    for ip in [Some("127.0.0.1"), Some("::1"), None] {
        let mut config = two_endpoint_config();
        config.listener.auth_token = "test-secret".into();
        // Unknown peer is allowed through the CIDR boundary and rejected by auth.
        config.listener.allowed_cidrs = vec![];
        let engine = engine_with(config, FakeTransport::new());
        let response = engine
            .handle_request(
                ip.map(|ip| ip.parse().unwrap()),
                "POST",
                "/v1/messages",
                vec![],
                body(),
            )
            .await;
        assert!(response.status().is_client_error());
        assert_source_ip(&engine, ip, 1);
        engine.record_rejected_websocket(
            ip.map(|ip| ip.parse().unwrap()),
            "/v1/responses",
            &[],
            401,
            "inbound_auth_required",
        );
        assert_source_ip(&engine, ip, 2);
    }
}
