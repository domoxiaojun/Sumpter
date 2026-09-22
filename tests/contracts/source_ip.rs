//! Both adapters must retain the network peer across request lifecycle boundaries
//! and resolve the event `sourceIP` with the shared rule: `X-Real-IP`, then
//! `X-Forwarded-For`, then the TCP peer, without a trusted-proxy precondition.
//! Access control keeps evaluating the TCP peer.
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

/// Headers a client or proxy may attach; `Forwarded` is never parsed.
fn declared_headers() -> Vec<(String, String)> {
    vec![
        ("x-forwarded-for".into(), "198.51.100.99".into()),
        ("forwarded".into(), "for=198.51.100.99".into()),
        ("x-real-ip".into(), "198.51.100.99".into()),
    ]
}

/// Without forwarded headers every event of the request (client, both upstream
/// attempts) records the TCP peer, IPv4 or IPv6.
#[tokio::test]
async fn source_ip_survives_retries_and_stream_completion() {
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
        let (status, _) = call(
            &engine,
            Some(ip.parse().unwrap()),
            "/v1/messages",
            vec![],
            body(),
        )
        .await;
        assert_eq!(status, 200);
        assert_source_ip(&engine, Some(ip), 3);
    }
}

/// A declared client address needs no trusted-proxy entry: loopback, IPv6 and
/// private peers all get their `X-Real-IP` recorded for the whole request.
#[tokio::test]
async fn declared_client_ip_replaces_source_ip_without_trust_list() {
    for peer in ["127.0.0.1", "::1", "10.0.0.5"] {
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
        let (status, _) = call(
            &engine,
            Some(peer.parse().unwrap()),
            "/v1/messages",
            declared_headers(),
            body(),
        )
        .await;
        assert_eq!(status, 200, "{peer}");
        assert_source_ip(&engine, Some("198.51.100.99"), 3);
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

/// Header precedence and fallbacks apply to every event of the request
/// (client, both upstream attempts, cancellation).
#[tokio::test]
async fn forwarded_header_precedence_applies_to_whole_request() {
    let mut config = two_endpoint_config();
    config.listener.trusted_proxy_cidrs = vec!["127.0.0.1".into(), "fd00::/8".into()];
    for (peer, headers, expected) in [
        // X-Real-IP wins over X-Forwarded-For, whatever the peer is.
        (
            "10.0.0.5",
            vec![
                ("x-forwarded-for".to_string(), "198.51.100.99".to_string()),
                ("x-real-ip".to_string(), "192.0.2.10".to_string()),
            ],
            "192.0.2.10",
        ),
        // Listed proxy hops are skipped from the right of the chain.
        (
            "127.0.0.1",
            vec![(
                "x-forwarded-for".to_string(),
                "198.51.100.99, 127.0.0.1".to_string(),
            )],
            "198.51.100.99",
        ),
        // IPv6 peer and client.
        (
            "fd00::1",
            vec![("x-real-ip".to_string(), "[2001:db8::7]".to_string())],
            "2001:db8::7",
        ),
        // A malformed X-Real-IP falls through to X-Forwarded-For.
        (
            "127.0.0.1",
            vec![
                ("x-real-ip".to_string(), "not-an-ip".to_string()),
                ("x-forwarded-for".to_string(), "203.0.113.5".to_string()),
            ],
            "203.0.113.5",
        ),
        // Both malformed: the TCP peer stays.
        (
            "127.0.0.1",
            vec![
                ("x-forwarded-for".to_string(), "not-an-ip".to_string()),
                ("x-real-ip".to_string(), "198.51.100.99:8080".to_string()),
            ],
            "127.0.0.1",
        ),
    ] {
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
        let engine = engine_with(config.clone(), fake);
        let (status, _) = call(
            &engine,
            Some(peer.parse().unwrap()),
            "/v1/messages",
            headers,
            body(),
        )
        .await;
        assert_eq!(status, 200, "{peer}");
        assert_source_ip(&engine, Some(expected), 3);
    }

    // Cancellation after the inbound scope ends keeps the declared client, with
    // the default (empty) trusted-proxy list.
    let fake = FakeTransport::new();
    fake.push("a.example.com", Outcome::Gated { status: 200 });
    let engine = engine_with(two_endpoint_config(), fake.clone());
    let response = engine
        .handle_request(
            loopback(),
            "POST",
            "/v1/messages",
            vec![("x-forwarded-for".into(), "203.0.113.9".into())],
            body(),
        )
        .await;
    let mut stream = response.into_body().into_data_stream();
    fake.gate_sender()
        .send(Ok(Bytes::from_static(b"data: chunk\n\n")))
        .unwrap();
    stream.next().await.unwrap().unwrap();
    drop(stream);
    let events = engine.runtime_snapshot().recent_events;
    assert!(
        events
            .iter()
            .any(|event| event.outcome == Some(RuntimeEventOutcome::Cancelled))
    );
    assert_source_ip(&engine, Some("203.0.113.9"), 2);
}

/// Forwarded headers never change authentication, the CIDR allow-list or the
/// loopback-only `/__status` policy: those keep evaluating the TCP peer, while
/// the rejection event still records the declared client.
#[tokio::test]
async fn forwarded_headers_do_not_affect_access_control() {
    // 1. allowedCIDRs rejects the peer even though an allow-listed client is
    //    declared, with or without a trusted-proxy list.
    for trusted in [vec![], vec!["10.0.0.0/8".to_string()]] {
        let mut config = two_endpoint_config();
        config.listener.allowed_cidrs = vec!["192.0.2.0/24".into()];
        config.listener.trusted_proxy_cidrs = trusted;
        let engine = engine_with(config, FakeTransport::new());
        let (status, _) = call(
            &engine,
            Some("10.0.0.1".parse().unwrap()),
            "/v1/messages",
            vec![("x-forwarded-for".into(), "192.0.2.10".into())],
            body(),
        )
        .await;
        assert_eq!(status, 403);
        assert_source_ip(&engine, Some("192.0.2.10"), 1);
    }

    // 2. A declared loopback address does not unlock the peer-only status policy.
    let engine = engine_with(two_endpoint_config(), FakeTransport::new());
    let response = engine
        .handle_request(
            Some("10.0.0.1".parse().unwrap()),
            "GET",
            "/__status",
            vec![
                ("x-forwarded-for".into(), "127.0.0.1".into()),
                ("x-real-ip".into(), "127.0.0.1".into()),
            ],
            Bytes::new(),
        )
        .await;
    assert_eq!(response.status(), 403);

    // 3. Inbound auth is unaffected: a request without the token is still 401,
    //    and the rejection is attributed to the declared client.
    let mut config = two_endpoint_config();
    config.listener.auth_token = "test-secret".into();
    let engine = engine_with(config, FakeTransport::new());
    let (status, _) = call(
        &engine,
        loopback(),
        "/v1/messages",
        vec![("x-forwarded-for".into(), "203.0.113.9".into())],
        body(),
    )
    .await;
    assert_eq!(status, 401);
    assert_source_ip(&engine, Some("203.0.113.9"), 1);
    assert!(
        !engine.authorize_websocket(
            loopback(),
            &[("x-forwarded-for".into(), "203.0.113.9".into())]
        ),
        "forwarded headers must not satisfy inbound auth"
    );
}
