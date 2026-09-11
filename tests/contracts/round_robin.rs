//! Exercise the shared first-session scheduler through both platform adapters.
use super::*;

fn config() -> AppConfig {
    let mut config = two_endpoint_config();
    config.schema_version = 7;
    let source = config.endpoints[0].clone();
    config.endpoints = ["a", "b", "c", "d", "e"]
        .map(|id| {
            let mut entry = source.clone();
            entry.id = id.into();
            entry.name = id.into();
            entry.base_url = format!("https://{id}.invalid");
            entry.sticky_group = None;
            entry
        })
        .to_vec();
    let bindings = ["a", "b", "c", "d", "e"]
        .into_iter()
        .map(|id| json!({"endpointID": id}))
        .collect::<Vec<_>>();
    config.model_groups = serde_json::from_value(json!([{
        "id": "main", "models": ["claude-*"], "schedulingStrategy": "roundRobinSticky",
        "bindings": bindings
    }]))
    .unwrap();
    config
}

#[tokio::test(flavor = "multi_thread", worker_threads = 5)]
async fn round_robin_concurrent_conversations_keep_their_first_assignment() {
    let fake = FakeTransport::new();
    let engine = engine_with(config(), fake.clone());
    for id in ["a", "b", "c", "d", "e"] {
        for _ in 0..3 {
            fake.push(&format!("{id}.invalid"), sse_ok(&["data: {}\n\n"]));
        }
    }
    let barrier = Arc::new(tokio::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for index in 0..5 {
        let engine = engine.clone();
        let barrier = barrier.clone();
        workers.push(tokio::spawn(async move {
            barrier.wait().await;
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session(&format!("new-{index}"))),
                body(),
            )
            .await
            .0
        }));
    }
    for worker in workers {
        assert_eq!(worker.await.unwrap(), 200);
    }
    let mut hosts = fake
        .requests()
        .iter()
        .map(|r| r.host.clone())
        .collect::<Vec<_>>();
    hosts.sort();
    assert_eq!(
        hosts,
        [
            "a.invalid",
            "b.invalid",
            "c.invalid",
            "d.invalid",
            "e.invalid"
        ]
    );
    for index in 0..5 {
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session(&format!("new-{index}"))),
                body()
            )
            .await
            .0,
            200
        );
    }
    let mut repeated = fake.requests()[5..]
        .iter()
        .map(|r| r.host.clone())
        .collect::<Vec<_>>();
    repeated.sort();
    assert_eq!(repeated, hosts);
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&stable_session("sixth")),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(fake.requests().last().unwrap().host, "a.invalid");
}

#[tokio::test]
async fn round_robin_failover_keeps_configured_backup_order_and_rebinds() {
    let fake = FakeTransport::new();
    let engine = engine_with(config(), fake.clone());
    for (index, host) in ["a.invalid", "b.invalid"].into_iter().enumerate() {
        fake.push(host, sse_ok(&["data: {}\n\n"]));
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session(&format!("prime-{index}"))),
                body()
            )
            .await
            .0,
            200
        );
    }
    fake.push(
        "c.invalid",
        Outcome::Status {
            status: 503,
            headers: vec![],
            chunks: vec![],
        },
    );
    for _ in 0..2 {
        fake.push("a.invalid", sse_ok(&["data: {}\n\n"]));
    }
    for _ in 0..2 {
        assert_eq!(
            call(
                &engine,
                loopback(),
                "/v1/messages",
                session_header(&stable_session("third")),
                body()
            )
            .await
            .0,
            200
        );
    }
    assert_eq!(
        fake.requests()
            .iter()
            .map(|r| r.host.as_str())
            .collect::<Vec<_>>(),
        [
            "a.invalid",
            "b.invalid",
            "c.invalid",
            "a.invalid",
            "a.invalid"
        ]
    );
    fake.push("d.invalid", sse_ok(&["data: {}\n\n"]));
    assert_eq!(
        call(
            &engine,
            loopback(),
            "/v1/messages",
            session_header(&stable_session("fourth")),
            body()
        )
        .await
        .0,
        200
    );
    assert_eq!(fake.requests().last().unwrap().host, "d.invalid");
}
