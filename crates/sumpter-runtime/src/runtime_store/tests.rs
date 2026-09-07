use super::*;

#[test]
fn pi_persistence_filters_and_session_export_preserve_attribution() {
    let dir = test_dir("pi-attribution");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut value = event(
        "pi-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.client_kind = Some(ClientKind::Pi);
    value.session_id = Some("pi-session".into());
    value.request_id = Some("pi-request".into());
    value.client_declared = ClientDeclaredMetadata::from_headers(&[
        ("x-sumpter-project".into(), "pi-project".into()),
        ("x-sumpter-workspace".into(), "/work/pi-project".into()),
        ("x-sumpter-user".into(), "local-user".into()),
    ]);
    store.enqueue(value, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();
    drop(store);
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let analytics = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                client_kind: Some("pi".into()),
                session_id: Some("pi-session".into()),
                project: Some("pi-project".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(analytics["clientRequests"], 1);
    assert_eq!(analytics["projects"][0]["clientKinds"], json!(["pi"]));
    assert_eq!(analytics["projects"][0]["projectSource"], "workspace_local");
    assert_eq!(analytics["sessions"][0]["name"], "pi-session");
    let export = store.export_session("pi-session").unwrap();
    assert_eq!(export["clientKinds"], json!(["pi"]));
    let exported = &export["events"][0]["event"];
    assert_eq!(exported["clientKind"], "pi");
    assert!(exported["codexMetadata"].is_null());
    let connection = Connection::open(&path).unwrap();
    let projection: (String, String, Option<String>, Option<String>) = connection.query_row(
        "SELECT client_kind,session_key,codex_thread_class,attribution_scope FROM runtime_events WHERE event_id='pi-client'",
        [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    ).unwrap();
    assert_eq!(projection, ("pi".into(), "pi-session".into(), None, None));
    drop(connection);
    drop(store);
    remove_test_dir(&dir);
}

fn test_dir(label: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "sumpter-runtime-store-{label}-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn event(
    id: &str,
    kind: &str,
    status_code: i64,
    phase: RuntimeEventPhase,
    outcome: Option<RuntimeEventOutcome>,
    timestamp: f64,
) -> RuntimeEvent {
    serde_json::from_value(json!({
        "id": id,
        "kind": kind,
        "durationMS": 25,
        "failover": false,
        "statusCode": status_code,
        "timestamp": timestamp,
        "phase": phase,
        "outcome": outcome,
    }))
    .unwrap()
}

fn remove_test_dir(path: &Path) {
    for _ in 0..20 {
        if std::fs::remove_dir_all(path).is_ok() || !path.exists() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn unknown_token_accounting_is_not_reported_as_mixed() {
    assert_eq!(
        merge_semantics(
            TokenAccountingSemantics::Subset,
            TokenAccountingSemantics::Unknown
        )
        .as_str(),
        "unknown"
    );
    assert_eq!(
        merge_semantics(
            TokenAccountingSemantics::Unknown,
            TokenAccountingSemantics::Independent
        )
        .as_str(),
        "unknown"
    );
    assert_eq!(
        merge_semantics(
            TokenAccountingSemantics::Subset,
            TokenAccountingSemantics::Independent
        )
        .as_str(),
        "mixed"
    );
}

#[test]
fn cleanup_before_deletes_only_complete_old_request_groups() {
    let dir = test_dir("cleanup-before");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let now = event_now();
    for (id, request_id, kind, timestamp, phase) in [
        (
            "old-client",
            Some("old-request"),
            KIND_CLIENT,
            now - 200.0,
            RuntimeEventPhase::Completed,
        ),
        (
            "old-upstream",
            Some("old-request"),
            KIND_UPSTREAM,
            now - 190.0,
            RuntimeEventPhase::Completed,
        ),
        (
            "mixed-client",
            Some("mixed-request"),
            KIND_CLIENT,
            now - 200.0,
            RuntimeEventPhase::Completed,
        ),
        (
            "mixed-upstream",
            Some("mixed-request"),
            KIND_UPSTREAM,
            now - 10.0,
            RuntimeEventPhase::Completed,
        ),
        (
            "active-client",
            Some("active-request"),
            KIND_CLIENT,
            now - 200.0,
            RuntimeEventPhase::Completed,
        ),
        (
            "active-upstream",
            Some("active-request"),
            KIND_UPSTREAM,
            now - 190.0,
            RuntimeEventPhase::InFlight,
        ),
    ] {
        let mut value = event(
            id,
            kind,
            200,
            phase,
            if phase == RuntimeEventPhase::Completed {
                Some(RuntimeEventOutcome::Succeeded)
            } else {
                None
            },
            timestamp,
        );
        value.request_id = request_id.map(str::to_owned);
        store.enqueue(value, RuntimeCounters::default()).unwrap();
    }
    store.flush().unwrap();

    let cutoff = now - 100.0;
    let preview = store.cleanup_before_preview(cutoff).unwrap();
    assert_eq!(preview.deletable_events, 2);
    assert_eq!(preview.deletable_requests, 1);
    let mutation = store.cleanup_before(cutoff).unwrap();
    assert_eq!(mutation.deleted_events, 2);
    assert_eq!(mutation.deleted_requests, 1);
    assert!(store.event("old-client").unwrap().is_none());
    assert!(store.event("old-upstream").unwrap().is_none());
    assert!(store.event("mixed-client").unwrap().is_some());
    assert!(store.event("active-upstream").unwrap().is_some());
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn schema_bootstrap_is_private_and_integral() {
    let dir = test_dir("schema");
    let path = dir.join("runtime.sqlite3");
    let (store, snapshot) = RuntimeStore::new(&path).unwrap();
    assert_eq!(snapshot, RuntimeSnapshot::default());
    store.flush().unwrap();
    let summary = store.summary();
    assert_eq!(summary.storage.schema_version, 3);
    assert!(summary.storage.backfill_complete);
    assert!(summary.storage.indexes_ready);
    assert!(summary.storage.rollup_complete);
    assert_eq!(summary.storage.rollup_dirty_buckets, 0);

    let connection = Connection::open(&path).unwrap();
    assert_eq!(meta_i64(&connection, "schema_version").unwrap(), Some(3));
    assert_eq!(
        connection
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    for table in [
        "runtime_hourly_rollups",
        "runtime_hourly_rollup_dirty",
        "runtime_retention",
        "runtime_pricing_meta",
        "runtime_model_prices",
    ] {
        assert!(
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master
                         WHERE type='table' AND name=?1)",
                    [table],
                    |row| row.get::<_, bool>(0),
                )
                .unwrap(),
            "missing table {table}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(connection);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn schema_v2_migrates_in_batches_and_quarantines_bad_payloads() {
    let dir = test_dir("schema-v2-migration");
    let path = dir.join("runtime.sqlite3");
    let mut connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','1');
                 CREATE TABLE runtime_counters(
                    id INTEGER PRIMARY KEY,client_requests INTEGER NOT NULL,
                    client_successes INTEGER NOT NULL,client_failures INTEGER NOT NULL,
                    upstream_attempts INTEGER NOT NULL,upstream_successes INTEGER NOT NULL,
                    upstream_failures INTEGER NOT NULL,failovers INTEGER NOT NULL);
                 INSERT INTO runtime_counters VALUES(1,0,0,0,0,0,0,0);
                 CREATE TABLE runtime_events(
                    seq INTEGER PRIMARY KEY,change_seq INTEGER NOT NULL UNIQUE,
                    event_id TEXT NOT NULL UNIQUE,request_id TEXT,timestamp REAL NOT NULL,
                    kind TEXT NOT NULL,phase TEXT,outcome TEXT,status_code INTEGER NOT NULL,
                    client_kind TEXT,request_purpose TEXT,endpoint_id TEXT,failure_kind TEXT,
                    is_in_flight INTEGER NOT NULL,payload_json TEXT NOT NULL,
                    created_at REAL NOT NULL,updated_at REAL NOT NULL);",
        )
        .unwrap();
    let mut valid = event(
        "legacy-valid",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    valid.session_id = Some("legacy-session".into());
    let valid_payload = serde_json::to_string(&valid).unwrap();
    connection
        .execute(
            "INSERT INTO runtime_events VALUES(
                    1,1,'legacy-valid',NULL,?1,'client','completed','succeeded',200,
                    NULL,NULL,NULL,NULL,0,?2,0,0)",
            params![valid.timestamp, valid_payload],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runtime_events VALUES(
                    2,2,'legacy-bad',NULL,?1,'client','completed','failed',500,
                    NULL,NULL,NULL,NULL,0,'{bad-json',0,0)",
            params![valid.timestamp],
        )
        .unwrap();

    setup_connection(&mut connection).unwrap();
    assert_eq!(meta_i64(&connection, "schema_version").unwrap(), Some(3));
    assert_eq!(
        connection
            .query_row(
                "SELECT projection_version FROM runtime_events WHERE seq=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        0
    );
    while run_projection_maintenance(&mut connection).unwrap() {}
    assert_eq!(
        connection
            .query_row(
                "SELECT projection_version FROM runtime_events WHERE seq=1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        PROJECTION_VERSION
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT projection_version FROM runtime_events WHERE seq=2",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        -1
    );
    assert_eq!(
        meta_i64(&connection, "projection_backfill_failed").unwrap(),
        Some(1)
    );
    assert_eq!(
        meta_i64(&connection, "projection_indexes_ready").unwrap(),
        Some(1)
    );
    assert_eq!(
        meta_i64(&connection, "hourly_rollup_complete").unwrap(),
        Some(1)
    );
    assert!(!run_projection_maintenance(&mut connection).unwrap());
    drop(connection);
    remove_test_dir(&dir);
}

#[test]
fn projection_preserves_null_zero_and_protocol_cache_semantics() {
    use sumpter_core::config::ProviderProtocol;

    let dir = test_dir("projection-token-semantics");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut openai = event(
        "projection-openai",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    openai.target_format = Some(ProviderProtocol::OpenAIResponses);
    openai.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 100, "outputTokens": 0, "cacheReadInputTokens": 60}
    }))
    .ok();
    store.enqueue(openai, RuntimeCounters::default()).unwrap();

    let mut anthropic = event(
        "projection-anthropic",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    anthropic.target_format = Some(ProviderProtocol::Anthropic);
    anthropic.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 10, "cacheReadInputTokens": 5, "cacheCreationInputTokens": 2}
    }))
    .ok();
    store
        .enqueue(anthropic, RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();
    let connection = Connection::open(&path).unwrap();
    let openai_row = connection
        .query_row(
            "SELECT output_tokens,cache_creation_input_tokens,processed_input_tokens,
                        uncached_input_tokens,token_accounting_semantics,
                        token_accounting_quality FROM runtime_events WHERE event_id=?1",
            ["projection-openai"],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(openai_row.0, Some(0), "显式 0 不能退化成 NULL");
    assert_eq!(openai_row.1, None, "上游缺失字段必须保留 NULL");
    assert_eq!(openai_row.2, Some(100));
    assert_eq!(openai_row.3, Some(40));
    assert_eq!(openai_row.4, "subset");
    assert_eq!(openai_row.5, "complete");
    let anthropic_row = connection
        .query_row(
            "SELECT output_tokens,processed_input_tokens,uncached_input_tokens,
                        token_accounting_semantics,token_accounting_quality
                 FROM runtime_events WHERE event_id=?1",
            ["projection-anthropic"],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(anthropic_row.0, None);
    assert_eq!(anthropic_row.1, Some(17));
    assert_eq!(anthropic_row.2, Some(10));
    assert_eq!(anthropic_row.3, "independent");
    assert_eq!(anthropic_row.4, "partial");
    drop(connection);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn sqlite_roundtrip_preserves_source_session_attribution() {
    let dir = test_dir("source-session-roundtrip");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut value = event(
        "source-session-event",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.session_id = Some("session-header-source".into());
    value.codex_metadata = serde_json::from_value(json!({
        "installationID": "sha256:display-only",
        "sourceInstallationID": "install-source-value",
        "sessionID": "codex-session-source",
        "threadID": "thread-source",
        "turnID": "turn-source",
        "windowNumber": 0, "contextWindowID": "context-source",
        "forkedFromOrdinalExclusive": 42, "turnTrigger": "user_input",
        "historyIngestRequested": false,
        "sourceWorkspacePaths": ["/Users/kkl/Documents/automode-proxy"],
    }))
    .ok();
    value.client_declared = serde_json::from_value(json!({
        "project": ".../automode-proxy",
        "workspace": ".../.claude/automode-proxy",
        "sourceProject": "automode-proxy",
        "sourceWorkspace": "/Users/kkl/.claude/automode-proxy",
    }))
    .ok();
    store.enqueue(value, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();
    drop(store);

    let (reopened, _) = RuntimeStore::new(&path).unwrap();
    let stored = reopened
        .event("source-session-event")
        .unwrap()
        .expect("source event survives reopen")
        .event;
    let codex = stored.codex_metadata.expect("codex source metadata");
    assert_eq!(
        codex.source_installation_id.as_deref(),
        Some("install-source-value")
    );
    assert_eq!(codex.session_id.as_deref(), Some("codex-session-source"));
    assert_eq!(codex.thread_id.as_deref(), Some("thread-source"));
    assert_eq!(codex.turn_id.as_deref(), Some("turn-source"));
    assert_eq!(codex.window_number, Some(0));
    assert_eq!(codex.context_window_id.as_deref(), Some("context-source"));
    assert_eq!(codex.forked_from_ordinal_exclusive, Some(42));
    assert_eq!(codex.turn_trigger.as_deref(), Some("user_input"));
    assert_eq!(codex.history_ingest_requested, Some(false));
    assert_eq!(
        codex.source_workspace_paths,
        vec!["/Users/kkl/Documents/automode-proxy"]
    );
    let declared = stored.client_declared.expect("client source metadata");
    assert_eq!(declared.source_project.as_deref(), Some("automode-proxy"));
    assert_eq!(
        declared.source_workspace.as_deref(),
        Some("/Users/kkl/.claude/automode-proxy")
    );

    let connection = Connection::open(&path).unwrap();
    let payload: String = connection
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE event_id=?1",
            ["source-session-event"],
            |row| row.get(0),
        )
        .unwrap();
    assert!(payload.contains("install-source-value"));
    assert!(payload.contains("/Users/kkl/.claude/automode-proxy"));
    drop(connection);
    drop(reopened);
    remove_test_dir(&dir);
}

#[test]
fn model_group_attribution_survives_update_reopen_and_list_projection() {
    let dir = test_dir("model-group-roundtrip");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut value = event(
        "group-event",
        KIND_CLIENT,
        0,
        RuntimeEventPhase::InFlight,
        None,
        event_now(),
    );
    value.model_group_id = Some("main".into());
    value.model_group_name = Some("主用".into());
    store
        .enqueue(value.clone(), RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();
    value.model_group_id = Some("backup".into());
    value.model_group_name = Some("备用".into());
    value.phase = Some(RuntimeEventPhase::Completed);
    value.outcome = Some(RuntimeEventOutcome::Succeeded);
    value.status_code = 200;
    store.enqueue(value, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();
    drop(store);
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let detail = store.event("group-event").unwrap().unwrap();
    let page = store
        .events(None, None, 10, None, None, None, None, None)
        .unwrap();
    assert_eq!(detail.event.model_group_id.as_deref(), Some("backup"));
    assert_eq!(page[0].model_group_name.as_deref(), Some("备用"));
    let row: (String, String) = Connection::open(&path).unwrap().query_row(
        "SELECT model_group_id,model_group_name FROM runtime_events WHERE event_id='group-event'",
        [], |row| Ok((row.get(0)?, row.get(1)?)),
    ).unwrap();
    assert_eq!(row, ("backup".into(), "备用".into()));
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn sticky_keys_for_project_aggregates_affinity_hashes_per_project() {
    let dir = test_dir("sticky-keys");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let push = |id: &str, workspace: &str, sticky: Option<&str>| {
        let mut value = event(
            id,
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.codex_metadata = Some(
            serde_json::from_value(json!({
                "workspaces": { workspace: {} }
            }))
            .unwrap(),
        );
        value.sticky_key = sticky.map(str::to_owned);
        store.enqueue(value, RuntimeCounters::default()).unwrap();
    };
    push("alpha-1", ".../demo/alpha", Some("affinity-alpha"));
    push("alpha-2", ".../demo/alpha", Some("affinity-alpha"));
    push("beta-1", ".../demo/beta", Some("affinity-beta"));
    push("alpha-rejected", ".../demo/alpha", None);
    store.flush().unwrap();
    let project_of = |event_id: &str| -> String {
        Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT project_id FROM runtime_events WHERE event_id=?1",
                params![event_id],
                |row| row.get(0),
            )
            .unwrap()
    };
    let alpha = project_of("alpha-1");
    let beta = project_of("beta-1");
    assert_ne!(alpha, beta);
    // 同键去重、None 不入结果、按项目隔离。
    assert_eq!(
        store.sticky_keys_for_project(&alpha).unwrap(),
        vec!["affinity-alpha".to_owned()]
    );
    assert_eq!(
        store.sticky_keys_for_project(&beta).unwrap(),
        vec!["affinity-beta".to_owned()]
    );
    // 事件 payload 里的 stickyKey 保持亲和哈希原文。
    let detail = store.event("alpha-1").unwrap().unwrap();
    assert_eq!(detail.event.sticky_key.as_deref(), Some("affinity-alpha"));
    assert!(
        store
            .event("alpha-rejected")
            .unwrap()
            .unwrap()
            .event
            .sticky_key
            .is_none()
    );
    // 空项目号在 store 边界直接拒绝。
    assert!(store.sticky_keys_for_project("  ").is_err());
    // 同一 affinity 出现在两个项目时整次拒绝，不能把共享键交给全局归属表删除。
    push("beta-shared", ".../demo/beta", Some("affinity-alpha"));
    assert!(
        store
            .sticky_keys_for_project(&alpha)
            .unwrap_err()
            .contains("共享")
    );
    assert!(
        store
            .sticky_keys_for_project(&beta)
            .unwrap_err()
            .contains("共享")
    );
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn pending_upsert_keeps_seq_and_advances_change_seq() {
    let dir = test_dir("upsert");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let first = store
        .enqueue(
            event(
                "event-1",
                KIND_CLIENT,
                0,
                RuntimeEventPhase::InFlight,
                None,
                event_now(),
            ),
            RuntimeCounters::default(),
        )
        .unwrap();
    let second = store
        .enqueue(
            event(
                "event-1",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    assert_eq!(first.seq, second.seq);
    assert!(second.change_seq > first.change_seq);
    store.flush().unwrap();
    let stored = store.event("event-1").unwrap().unwrap();
    assert_eq!(stored.seq, first.seq);
    assert_eq!(stored.change_seq, second.change_seq);
    assert_eq!(stored.event.phase, Some(RuntimeEventPhase::Completed));
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn hourly_rollup_waits_for_terminal_event_and_never_double_counts_upsert() {
    use sumpter_core::config::ProviderProtocol;

    let dir = test_dir("hourly-rollup-upsert");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let timestamp = event_now();
    let mut in_flight = event(
        "rollup-upsert",
        KIND_CLIENT,
        0,
        RuntimeEventPhase::InFlight,
        None,
        timestamp,
    );
    in_flight.target_format = Some(ProviderProtocol::OpenAIResponses);
    store
        .enqueue(in_flight.clone(), RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();

    in_flight.phase = Some(RuntimeEventPhase::Completed);
    in_flight.outcome = Some(RuntimeEventOutcome::Succeeded);
    in_flight.status_code = 200;
    in_flight.duration_ms = 120;
    in_flight.ttfb_ms = Some(30);
    in_flight.stream_trace = serde_json::from_value(json!({
        "usage": {
            "inputTokens": 100,
            "outputTokens": 20,
            "cacheReadInputTokens": 60,
            "cacheCreationInputTokens": 0
        }
    }))
    .ok();
    store
        .enqueue(
            in_flight,
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    store.flush().unwrap();
    for _ in 0..200 {
        let connection = Connection::open(&path).unwrap();
        if meta_i64(&connection, "hourly_rollup_complete").unwrap() == Some(1) {
            break;
        }
        drop(connection);
        thread::sleep(Duration::from_millis(10));
    }
    let connection = Connection::open(&path).unwrap();
    let row = connection
        .query_row(
            "SELECT client_requests,client_successes,duration_count,duration_ms_sum,
                        input_tokens,output_tokens,cache_read_input_tokens,
                        processed_input_tokens,cache_read_token_numerator,
                        cache_read_token_denominator
                 FROM runtime_hourly_rollups WHERE bucket_start=?1",
            params![hourly_bucket_start(timestamp)],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(row, (1, 1, 1, 120, 100, 20, 60, 100, 60, 100));
    assert_eq!(
        meta_i64(&connection, "hourly_rollup_max_seq").unwrap(),
        Some(1)
    );
    drop(connection);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn hourly_rollup_price_prefers_endpoint_rule_and_falls_back_to_global() {
    let make_price = |endpoint_id: Option<&str>, input_rate| RollupPrice {
        endpoint_id: endpoint_id.map(ToOwned::to_owned),
        model_key: "rollup-model".into(),
        effective_from: 0.0,
        effective_to: None,
        input_per_million_micros: Some(input_rate),
        output_per_million_micros: None,
        cache_read_per_million_micros: None,
        cache_creation_per_million_micros: None,
    };
    let prices = vec![make_price(None, 1), make_price(Some("endpoint-a"), 9)];
    assert_eq!(
        resolve_rollup_price(&prices, Some("endpoint-a"), "rollup-model", 10.0)
            .and_then(|price| price.input_per_million_micros),
        Some(9)
    );
    assert_eq!(
        resolve_rollup_price(&prices, Some("endpoint-b"), "rollup-model", 10.0)
            .and_then(|price| price.input_per_million_micros),
        Some(1)
    );
    assert_eq!(
        split_rollup_price_key("endpoint-a\u{1f}rollup-model".into()),
        (Some("endpoint-a".into()), "rollup-model".into())
    );
}

#[test]
fn retention_and_pricing_mutations_are_serialized_by_the_worker() {
    let dir = test_dir("retention-pricing-worker");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    for index in 0..3 {
        let mut value = event(
            &format!("retained-{index}"),
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now() + index as f64,
        );
        value.request_id = Some(format!("request-{index}"));
        store.enqueue(value, RuntimeCounters::default()).unwrap();
    }
    store.flush().unwrap();
    let retention = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: None,
            storage_limit_bytes: None,
        })
        .unwrap();
    assert_eq!(retention.revision, 2);
    let with_storage_limit = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 2,
            max_age_days: None,
            storage_limit_bytes: Some(8 * 1_048_576),
        })
        .unwrap();
    assert_eq!(with_storage_limit.revision, 3);
    assert_eq!(with_storage_limit.storage_limit_bytes, Some(8 * 1_048_576));
    assert!(
        store
            .set_retention(RuntimeRetentionUpdate {
                expected_revision: 1,
                max_age_days: None,
                storage_limit_bytes: None,
            })
            .unwrap_err()
            .contains("retention_revision_conflict")
    );

    let pricing = store
        .replace_pricing(RuntimePricingUpdate {
            expected_revision: 1,
            currency: "USD".into(),
            prices: vec![RuntimeModelPriceInput {
                endpoint_id: None,
                model_key: "gpt-test".into(),
                effective_from: 0.0,
                effective_to: None,
                input_per_million_micros: Some(1_000_000),
                output_per_million_micros: Some(2_000_000),
                cache_read_per_million_micros: Some(100_000),
                cache_creation_per_million_micros: None,
            }],
        })
        .unwrap();
    assert_eq!(pricing.revision, 2);
    assert_eq!(pricing.price_count, 1);
    assert!(
        store
            .replace_pricing(RuntimePricingUpdate {
                expected_revision: 1,
                currency: "USD".into(),
                prices: Vec::new(),
            })
            .unwrap_err()
            .contains("pricing_revision_conflict")
    );
    let connection = Connection::open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row("SELECT model_key FROM runtime_model_prices", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "gpt-test"
    );
    drop(connection);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn storage_limit_rotates_oldest_completed_request_groups_and_preserves_in_flight() {
    let dir = test_dir("storage-limit-rotation");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let old_timestamp = event_now() - 10_000.0;
    let mut old_client = event(
        "rotation-old-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        old_timestamp,
    );
    old_client.request_id = Some("rotation-old-request".into());
    old_client.message = Some("x".repeat(1_200_000));
    let mut old_upstream = old_client.clone();
    old_upstream.id = "rotation-old-upstream".into();
    old_upstream.kind = KIND_UPSTREAM.into();
    store
        .enqueue(old_client, RuntimeCounters::default())
        .unwrap();
    store
        .enqueue(old_upstream, RuntimeCounters::default())
        .unwrap();

    let mut retained = event(
        "rotation-retained",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    retained.request_id = Some("rotation-retained-request".into());
    store
        .enqueue(
            retained,
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();

    let mut in_flight = event(
        "rotation-in-flight",
        KIND_CLIENT,
        0,
        RuntimeEventPhase::InFlight,
        None,
        event_now() + 1.0,
    );
    in_flight.request_id = Some("rotation-live-request".into());
    store
        .enqueue(in_flight, RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();

    let mutation = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: None,
            storage_limit_bytes: Some(1_048_576),
        })
        .unwrap();
    assert_eq!(mutation.storage_limit_bytes, Some(1_048_576));
    assert!(store.event("rotation-old-client").unwrap().is_none());
    assert!(store.event("rotation-old-upstream").unwrap().is_none());
    assert!(store.event("rotation-retained").unwrap().is_some());
    assert!(store.event("rotation-in-flight").unwrap().is_some());
    let summary = store.summary();
    assert_eq!(summary.storage.in_flight_event_count, 1);
    assert_eq!(summary.counters.client_requests, 1);
    assert_eq!(summary.counters.client_successes, 1);

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn max_age_rotates_expired_completed_request_groups_and_keeps_fresh_rows() {
    let dir = test_dir("max-age-rotation");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let old_timestamp = event_now() - (3.0 * 86_400.0);
    let mut old_client = event(
        "age-old-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        old_timestamp,
    );
    old_client.request_id = Some("age-old-request".into());
    let mut old_upstream = old_client.clone();
    old_upstream.id = "age-old-upstream".into();
    old_upstream.kind = KIND_UPSTREAM.into();
    store
        .enqueue(old_client, RuntimeCounters::default())
        .unwrap();
    store
        .enqueue(old_upstream, RuntimeCounters::default())
        .unwrap();

    let mut fresh = event(
        "age-fresh-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    fresh.request_id = Some("age-fresh-request".into());
    store.enqueue(fresh, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let mutation = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: Some(1),
            storage_limit_bytes: None,
        })
        .unwrap();
    assert_eq!(mutation.max_age_days, Some(1));
    assert_eq!(mutation.storage_limit_bytes, None);
    assert!(store.event("age-old-client").unwrap().is_none());
    assert!(store.event("age-old-upstream").unwrap().is_none());
    assert!(store.event("age-fresh-client").unwrap().is_some());
    assert_eq!(store.summary().storage.event_count, 1);

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn max_age_protects_an_entire_request_group_while_any_event_is_in_flight() {
    let dir = test_dir("max-age-in-flight-group");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let old_timestamp = event_now() - (3.0 * 86_400.0);
    let request_id = "age-live-request";

    let mut completed = event(
        "age-live-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        old_timestamp,
    );
    completed.request_id = Some(request_id.into());
    store
        .enqueue(completed, RuntimeCounters::default())
        .unwrap();

    let mut active = event(
        "age-live-upstream",
        KIND_UPSTREAM,
        0,
        RuntimeEventPhase::InFlight,
        None,
        old_timestamp,
    );
    active.request_id = Some(request_id.into());
    store.enqueue(active, RuntimeCounters::default()).unwrap();

    let mut unrelated = event(
        "age-unrelated-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        old_timestamp,
    );
    unrelated.request_id = Some("age-unrelated-request".into());
    store
        .enqueue(unrelated, RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();

    store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: Some(1),
            storage_limit_bytes: None,
        })
        .unwrap();

    assert!(store.event("age-live-client").unwrap().is_some());
    assert!(store.event("age-live-upstream").unwrap().is_some());
    assert!(store.event("age-unrelated-client").unwrap().is_none());

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn max_age_can_be_disabled_without_deleting_existing_events() {
    let dir = test_dir("max-age-disable");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut old = event(
        "age-disabled-client",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now() - (3.0 * 86_400.0),
    );
    old.request_id = Some("age-disabled-request".into());
    store.enqueue(old, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let enabled = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: Some(1),
            storage_limit_bytes: None,
        })
        .unwrap();
    assert_eq!(enabled.max_age_days, Some(1));
    assert!(store.event("age-disabled-client").unwrap().is_none());

    let disabled = store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 2,
            max_age_days: None,
            storage_limit_bytes: None,
        })
        .unwrap();
    assert_eq!(disabled.max_age_days, None);

    let mut second_old = event(
        "age-disabled-client-2",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now() - (3.0 * 86_400.0),
    );
    second_old.request_id = Some("age-disabled-request-2".into());
    store
        .enqueue(second_old, RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();
    assert!(store.event("age-disabled-client-2").unwrap().is_some());

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn startup_detects_legacy_retention_without_migration() {
    let dir = test_dir("retention-startup-migration");
    let path = dir.join("runtime.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','3');
                 CREATE TABLE runtime_retention(
                    id INTEGER PRIMARY KEY CHECK(id=1),
                    revision INTEGER NOT NULL DEFAULT 1,
                    max_events INTEGER,
                    max_age_days INTEGER,
                    max_live_bytes INTEGER,
                    evicted_events INTEGER NOT NULL DEFAULT 0,
                    evicted_requests INTEGER NOT NULL DEFAULT 0,
                    last_pruned_at REAL,
                    over_limit INTEGER NOT NULL DEFAULT 0,
                    updated_at REAL NOT NULL);
                 INSERT INTO runtime_retention VALUES(1,7,10000,30,1048576,4,2,1234.0,1,1234.0);",
        )
        .unwrap();
    drop(connection);
    let (store, _) = RuntimeStore::new(&path).unwrap();

    let mut connection = Connection::open(&path).unwrap();
    connection.busy_timeout(Duration::from_secs(5)).unwrap();
    setup_connection(&mut connection).unwrap();

    let retention = connection
        .query_row(
            "SELECT revision
                 FROM runtime_retention WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(
        retention, 7,
        "旧 revision 可保留，但旧自动清理值不得转成新容量提醒"
    );
    let legacy = crate::runtime_query::storage_details(&path).unwrap();
    assert!(legacy.legacy_retention_detected);
    assert_eq!(legacy.retention.storage_limit_bytes, None);

    drop(connection);
    store.reset().unwrap();
    let after_reset = crate::runtime_query::storage_details(&path).unwrap();
    assert!(after_reset.legacy_retention_detected);

    store.recreate().unwrap();
    let after_recreate = crate::runtime_query::storage_details(&path).unwrap();
    assert!(!after_recreate.legacy_retention_detected);
    let connection = Connection::open(&path).unwrap();
    let columns = connection
        .prepare("PRAGMA table_info(runtime_retention)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(columns, vec!["id", "revision", "updated_at"]);

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn reset_clears_rows_without_reusing_sequences_or_touching_stats_json() {
    let dir = test_dir("reset");
    let path = dir.join("runtime.sqlite3");
    let stats_path = dir.join("stats.json");
    std::fs::write(&stats_path, b"{\"archived\":true}\n").unwrap();
    let before = std::fs::metadata(&stats_path).unwrap().modified().unwrap();
    let before_bytes = std::fs::read(&stats_path).unwrap();
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let first = store
        .enqueue(
            event(
                "event-before-reset",
                KIND_CLIENT,
                500,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Failed),
                event_now(),
            ),
            RuntimeCounters {
                client_requests: 1,
                client_failures: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    store.flush().unwrap();
    assert_eq!(store.reset().unwrap(), 1);
    assert!(store.event("event-before-reset").unwrap().is_none());
    let second = store
        .enqueue(
            event(
                "event-after-reset",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    assert!(second.seq > first.seq);
    assert!(second.change_seq > first.change_seq);
    store.flush().unwrap();
    assert_eq!(std::fs::read(&stats_path).unwrap(), before_bytes);
    assert_eq!(
        std::fs::metadata(&stats_path).unwrap().modified().unwrap(),
        before
    );
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn startup_normalizes_crash_leftovers_conservatively() {
    let dir = test_dir("startup");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    store
        .enqueue(
            event(
                "no-headers",
                KIND_CLIENT,
                0,
                RuntimeEventPhase::InFlight,
                None,
                event_now(),
            ),
            RuntimeCounters::default(),
        )
        .unwrap();
    store
        .enqueue(
            event(
                "headers-seen",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::InFlight,
                None,
                event_now(),
            ),
            RuntimeCounters::default(),
        )
        .unwrap();
    store.flush().unwrap();
    drop(store);
    thread::sleep(Duration::from_millis(20));

    let (reopened, snapshot) = RuntimeStore::new(&path).unwrap();
    assert!(reopened.event("no-headers").unwrap().is_none());
    let normalized = reopened.event("headers-seen").unwrap().unwrap();
    assert_eq!(normalized.event.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(normalized.event.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(
        normalized.event.failure_kind,
        Some(RuntimeFailureKind::StreamInterrupted)
    );
    assert_eq!(snapshot.client_requests, 1);
    assert_eq!(snapshot.client_failures, 1);
    drop(reopened);
    remove_test_dir(&dir);
}

#[test]
fn analytics_uses_apple_epoch_and_streams_aggregation() {
    let dir = test_dir("analytics");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut value = event(
        "recent-event",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.endpoint_id = Some("primary-id".into());
    value.endpoint_name = Some("primary".into());
    value.target_format = Some(sumpter_core::config::ProviderProtocol::Anthropic);
    value.tool_calls = Some(vec!["shell".into()]);
    value.stream_trace = Some(
        serde_json::from_value(json!({
            "usage": {
                "inputTokens": 12,
                "outputTokens": 7,
                "cacheReadInputTokens": 4,
                "cacheCreationInputTokens": 2
            }
        }))
        .unwrap(),
    );
    value.codex_metadata = Some(
        serde_json::from_value(json!({
            "workspaces": {
                ".../demo/automode-proxy": {
                    "associatedRemoteURLs": {
                        "origin": "https://github.com/example/sumpter.git"
                    }
                }
            }
        }))
        .unwrap(),
    );
    store
        .enqueue(
            value,
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    store.flush().unwrap();
    let analytics = store.analytics("24h").unwrap();
    assert_eq!(analytics["clientRequests"], 1);
    assert_eq!(analytics["clientSuccesses"], 1);
    assert_eq!(analytics["endpoints"][0]["name"], "primary");
    assert_eq!(analytics["toolCalls"][0]["name"], "shell");
    assert_eq!(analytics["tokenUsage"]["inputTokens"], 12);
    assert_eq!(analytics["tokenUsage"]["outputTokens"], 7);
    assert_eq!(analytics["tokenUsage"]["totalTokens"], 19);
    assert_eq!(analytics["tokenUsage"]["processedInputTokens"], 18);
    assert_eq!(analytics["tokenUsage"]["processedTotalTokens"], 25);
    assert!(
        (analytics["tokenUsage"]["cacheReadTokenRate"]
            .as_f64()
            .unwrap()
            - (4.0 / 18.0))
            .abs()
            < 0.000_001
    );
    assert_eq!(analytics["tokenUsage"]["cacheReadRequestRate"], 1.0);
    assert_eq!(analytics["tokenUsage"]["cacheReadTokenEligibleRequests"], 1);
    assert_eq!(
        analytics["tokenUsage"]["tokenAccountingSemantics"],
        "independent"
    );
    assert_eq!(analytics["tokenUsage"]["observedRequests"], 1);
    assert_eq!(analytics["projects"][0]["name"], "automode-proxy");
    assert_eq!(analytics["projects"][0]["totalTokens"], 19);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_filters_client_project_session_and_uses_protocol_token_semantics() {
    use sumpter_core::config::ProviderProtocol;

    let dir = test_dir("analytics-filters");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();

    let mut claude = event(
        "client-claude",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    claude.request_id = Some("request-claude".into());
    claude.session_id = Some("session-claude-header".into());
    claude.client_kind = Some(ClientKind::ClaudeCode);
    claude.target_format = Some(ProviderProtocol::Anthropic);
    claude.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 14, "outputTokens": 4, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 13503}
        })).ok();
    claude.codex_metadata = serde_json::from_value(json!({
        "sessionID": "session-claude-metadata",
        "workspaces": {".../projects/project-a": {}}
    }))
    .ok();
    store.enqueue(claude, RuntimeCounters::default()).unwrap();

    let mut upstream = event(
        "upstream-claude",
        KIND_UPSTREAM,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    upstream.request_id = Some("request-claude".into());
    upstream.endpoint_name = Some("agent-domo".into());
    store.enqueue(upstream, RuntimeCounters::default()).unwrap();

    let mut codex = event(
        "client-codex",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    codex.request_id = Some("request-codex".into());
    codex.client_kind = Some(ClientKind::Codex);
    codex.target_format = Some(ProviderProtocol::OpenAIResponses);
    codex.stream_trace = serde_json::from_value(json!({
            "usage": {"inputTokens": 100, "outputTokens": 20, "cacheReadInputTokens": 60, "cacheCreationInputTokens": 0, "reasoningTokens": 5}
        })).ok();
    codex.codex_metadata = serde_json::from_value(json!({
        "sessionID": "session-codex",
        "workspaces": {".../projects/project-b": {}}
    }))
    .ok();
    store.enqueue(codex, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let all = store.analytics("24h").unwrap();
    assert_eq!(all["tokenUsage"]["processedInputTokens"], 13_617);
    assert_eq!(all["tokenUsage"]["processedTotalTokens"], 13_641);
    assert_eq!(all["tokenUsage"]["uncachedInputTokens"], 54);
    assert_eq!(all["tokenUsage"]["reasoningTokens"], 5);
    assert_eq!(all["tokenUsage"]["tokenAccountingSemantics"], "mixed");
    assert_eq!(all["facets"]["clientKinds"].as_array().unwrap().len(), 2);

    let claude_only = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                client_kind: Some("claude_code".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(claude_only["clientRequests"], 1);
    assert_eq!(claude_only["upstreamAttempts"], 1);
    assert_eq!(claude_only["tokenUsage"]["processedInputTokens"], 13_517);
    assert_eq!(claude_only["projects"][0]["name"], "project-a");

    let claude_session = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                session_id: Some("session-claude-header".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(claude_session["clientRequests"], 1);
    assert_eq!(claude_session["upstreamAttempts"], 1);
    assert_eq!(
        claude_session["sessions"][0]["name"],
        "session-claude-header"
    );
    assert_eq!(
        claude_session["sessions"][0]["projects"],
        json!(["project-a"])
    );
    assert_eq!(
        claude_session["sessions"][0]["clientKinds"],
        json!(["claude_code"])
    );

    let codex_session = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                project: Some("project-b".into()),
                session_id: Some("session-codex".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(codex_session["clientRequests"], 1);
    assert_eq!(codex_session["upstreamAttempts"], 0);
    assert_eq!(codex_session["tokenUsage"]["processedInputTokens"], 100);
    assert_eq!(codex_session["sessions"][0]["name"], "session-codex");
    assert_eq!(
        codex_session["sessions"][0]["projects"],
        json!(["project-b"])
    );

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_attributes_client_declared_projects_below_codex_workspaces() {
    let dir = test_dir("analytics-client-declared");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();

    // wrapper 带了工作区路径时升格为本地项目，不再标 client_declared。
    let mut declared = event(
        "declared-only",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    declared.client_kind = Some(ClientKind::ClaudeCode);
    declared.client_declared = serde_json::from_value(json!({
        "project": "automode-proxy",
        "workspace": ".../.claude/automode-proxy",
        "gitRemote": "https://github.com/domoxiaojun/sumpter.git"
    }))
    .ok();
    store.enqueue(declared, RuntimeCounters::default()).unwrap();

    // 两者都在时 Codex 结构化 workspace 必须胜出,声明值不得覆盖它。
    let mut both = event(
        "codex-wins",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    both.client_kind = Some(ClientKind::Codex);
    both.codex_metadata = serde_json::from_value(json!({
        "workspaces": {"/Users/kkl/Documents/projects/codex-demo": {}}
    }))
    .ok();
    both.client_declared = serde_json::from_value(json!({"project": "declared-loser"})).ok();
    store.enqueue(both, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let analytics = store.analytics("24h").unwrap();
    let projects = analytics["projects"].as_array().expect("projects");
    let row = |name: &str| {
        projects
            .iter()
            .find(|row| row["name"] == name)
            .unwrap_or_else(|| panic!("缺少项目行 {name}: {projects:?}"))
            .clone()
    };

    let declared_row = row("automode-proxy");
    assert_eq!(declared_row["projectSource"], "workspace_local");
    assert_eq!(
        declared_row["workspacePaths"],
        json!([".../.claude/automode-proxy"]),
        "声明的工作区只以脱敏尾段暴露"
    );

    let codex_row = row("codex-demo");
    assert_eq!(
        codex_row["projectSource"], "workspace_local",
        "Codex 结构化 workspace 优先,可信度词不被声明值降级"
    );
    assert!(
        !projects.iter().any(|row| row["name"] == "declared-loser"),
        "声明值不得在 Codex workspace 存在时另立项目行"
    );
}

#[test]
fn analytics_reports_filter_state_usage_presence_and_project_source() {
    let dir = test_dir("analytics-contract-fields");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();

    let mut local = event(
        "contract-local",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    local.client_kind = Some(ClientKind::Codex);
    local.endpoint_id = Some("endpoint-local".into());
    local.stream_trace = serde_json::from_value(json!({
        "usage": {
            "inputTokens": 0,
            "outputTokens": 0,
            "cacheReadInputTokens": 0
        }
    }))
    .ok();
    local.codex_metadata = serde_json::from_value(json!({
        "workspaces": {"/Users/kkl/Documents/projects/local-demo": {}}
    }))
    .ok();
    store.enqueue(local, RuntimeCounters::default()).unwrap();

    let mut missing = event(
        "contract-missing",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    missing.client_kind = Some(ClientKind::Codex);
    missing.endpoint_id = Some("endpoint-other".into());
    missing.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 3}
    }))
    .ok();
    store.enqueue(missing, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let analytics = store.analytics("24h").unwrap();
    assert_eq!(analytics["filtersApplied"], true);
    assert_eq!(analytics["filterWarning"], Value::Null);
    assert_eq!(
        analytics["appliedFilters"],
        json!({
            "clientKind": null, "endpointID": null, "projectID": null, "project": null, "sessionID": null
        })
    );
    assert_eq!(analytics["tokenUsage"]["observedRequests"], 2);
    assert_eq!(
        analytics["tokenUsage"]["usageFieldPresence"],
        json!({
            "inputTokens": 2,
            "outputTokens": 1,
            "cacheReadInputTokens": 1,
            "cacheCreationInputTokens": 0,
            "reasoningTokens": 0
        })
    );
    let local_row = analytics["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "local-demo")
        .unwrap();
    assert_eq!(local_row["projectSource"], "workspace_local");
    assert_eq!(
        local_row["workspacePaths"],
        json!([".../projects/local-demo"])
    );
    let missing_row = analytics["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "unidentified_project")
        .unwrap();
    assert_eq!(missing_row["projectSource"], "missing_workspace_metadata");

    let filtered = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                client_kind: Some(" codex ".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(filtered["filtersApplied"], true);
    assert_eq!(filtered["appliedFilters"]["clientKind"], "codex");
    let endpoint_filtered = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                endpoint_id: Some("endpoint-local".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(endpoint_filtered["clientRequests"], 1);
    assert_eq!(
        endpoint_filtered["appliedFilters"]["endpointID"],
        "endpoint-local"
    );
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn unidentified_session_requires_explicit_confirmation_to_delete() {
    let dir = test_dir("delete-unidentified");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut unknown = event(
        "unknown-session-event",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    unknown.request_id = Some("unknown-session-request".into());
    store.enqueue(unknown, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();
    assert!(store.delete_session("unidentified_session").is_err());
    let mutation = store
        .delete_session_confirmed("unidentified_session", true)
        .unwrap();
    assert_eq!(mutation.deleted_requests, 1);
    assert_eq!(mutation.deleted_events, 1);
    assert_eq!(store.analytics("all").unwrap()["clientRequests"], 0);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_keeps_multiple_sessions_in_one_local_project_separate() {
    let dir = test_dir("analytics-session-project");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    for (id, session) in [
        ("client-session-a", "session-a"),
        ("client-session-b", "session-b"),
    ] {
        let mut value = event(
            id,
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        );
        value.session_id = Some(session.into());
        value.codex_metadata = serde_json::from_value(json!({
                "workspaces": {".../automode-proxy": {"associatedRemoteURLs": {"origin": "https://github.com/example/sumpter.git"}}}
            })).ok();
        store.enqueue(value, RuntimeCounters::default()).unwrap();
    }
    store.flush().unwrap();
    let analytics = store.analytics("24h").unwrap();
    let sessions = analytics["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(
        sessions
            .iter()
            .all(|row| row["projects"] == json!(["automode-proxy"]))
    );
    assert!(
        sessions
            .iter()
            .all(|row| row["clientKinds"] == json!(["unrecorded_client"]))
    );

    let selected_session = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                project: Some("automode-proxy".into()),
                session_id: Some("session-a".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(selected_session["clientRequests"], 1);
    assert_eq!(
        selected_session["facets"]["sessions"],
        json!([
            {"value": "session-a", "count": 1},
            {"value": "session-b", "count": 1}
        ]),
        "session facet must ignore the selected session while respecting the project"
    );
    assert_eq!(
        selected_session["facets"]["projects"],
        json!([{"value": "automode-proxy", "count": 1}]),
        "project facet must respect the selected session"
    );

    let incompatible_selection = store
        .analytics_filtered(
            "24h",
            &AnalyticsFilter {
                client_kind: Some("claude_code".into()),
                project: Some("automode-proxy".into()),
                session_id: Some("session-a".into()),
                ..AnalyticsFilter::default()
            },
        )
        .unwrap();
    assert_eq!(incompatible_selection["clientRequests"], 0);
    assert_eq!(
        incompatible_selection["facets"]["clientKinds"],
        json!([
            {"value": "claude_code", "count": 0},
            {"value": "unrecorded_client", "count": 1}
        ])
    );
    assert_eq!(
        incompatible_selection["facets"]["projects"],
        json!([{"value": "automode-proxy", "count": 0}])
    );
    assert_eq!(
        incompatible_selection["facets"]["sessions"],
        json!([{"value": "session-a", "count": 0}])
    );
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_attributes_client_usage_to_final_endpoint_without_duplicate_attempts() {
    let dir = test_dir("analytics-endpoint-usage");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut client = event(
        "client-endpoint-usage",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    client.request_id = Some("request-endpoint-usage".into());
    client.session_id = Some("session-endpoint-usage".into());
    client.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 10, "outputTokens": 2, "cacheReadInputTokens": 4}
    }))
    .ok();
    store.enqueue(client, RuntimeCounters::default()).unwrap();

    for (id, endpoint, outcome, status) in [
        (
            "upstream-endpoint-a",
            "entry-a",
            RuntimeEventOutcome::Failed,
            502,
        ),
        (
            "upstream-endpoint-b",
            "entry-b",
            RuntimeEventOutcome::Succeeded,
            200,
        ),
    ] {
        let mut upstream = event(
            id,
            KIND_UPSTREAM,
            status,
            RuntimeEventPhase::Completed,
            Some(outcome),
            event_now(),
        );
        upstream.request_id = Some("request-endpoint-usage".into());
        upstream.endpoint_id = Some(endpoint.into());
        upstream.endpoint_name = Some(endpoint.into());
        store.enqueue(upstream, RuntimeCounters::default()).unwrap();
    }
    store.flush().unwrap();
    let analytics = store.analytics("24h").unwrap();
    assert_eq!(analytics["clientSuccessRate"], 100.0);
    assert_eq!(analytics["clientPending"], 0);
    assert_eq!(analytics["endpoints"].as_array().unwrap().len(), 2);
    assert_eq!(analytics["endpoints"][0]["name"], "entry-a");
    assert_eq!(analytics["endpoints"][0]["inputTokens"], 0);
    assert_eq!(analytics["endpoints"][1]["name"], "entry-b");
    assert_eq!(analytics["endpoints"][1]["attempts"], 1);
    assert_eq!(analytics["endpoints"][1]["inputTokens"], 10);
    assert_eq!(analytics["endpoints"][1]["cacheReadInputTokens"], 4);
    let mutation = store.delete_session("session-endpoint-usage").unwrap();
    assert_eq!(mutation.deleted_requests, 1);
    assert_eq!(mutation.deleted_events, 3);
    assert_eq!(store.analytics("24h").unwrap()["clientRequests"], 0);
    assert!(store.export_session("session-endpoint-usage").is_err());
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_attributes_usage_to_last_endpoint_when_all_attempts_fail() {
    let dir = test_dir("analytics-last-failed-endpoint-usage");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut client = event(
        "client-last-failed-usage",
        KIND_CLIENT,
        502,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Failed),
        event_now(),
    );
    client.request_id = Some("request-last-failed-usage".into());
    client.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 7, "outputTokens": 1}
    }))
    .ok();
    store.enqueue(client, RuntimeCounters::default()).unwrap();
    for (id, endpoint) in [
        ("upstream-failed-a", "entry-a"),
        ("upstream-failed-b", "entry-b"),
    ] {
        let mut upstream = event(
            id,
            KIND_UPSTREAM,
            502,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Failed),
            event_now(),
        );
        upstream.request_id = Some("request-last-failed-usage".into());
        upstream.endpoint_id = Some(endpoint.into());
        upstream.endpoint_name = Some(endpoint.into());
        store.enqueue(upstream, RuntimeCounters::default()).unwrap();
    }
    store.flush().unwrap();
    let analytics = store.analytics("24h").unwrap();
    assert_eq!(analytics["endpoints"][0]["name"], "entry-a");
    assert_eq!(analytics["endpoints"][1]["name"], "entry-b");
    assert_eq!(analytics["endpoints"][0]["inputTokens"], 0);
    assert_eq!(analytics["endpoints"][1]["inputTokens"], 7);
    assert_eq!(analytics["endpoints"][1]["outputTokens"], 1);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_keeps_unknown_protocol_unverifiable() {
    use sumpter_core::config::ProviderProtocol;

    let dir = test_dir("analytics-mixed-unknown");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();

    let mut unknown = event(
        "unknown-protocol",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    unknown.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 3, "outputTokens": 1}
    }))
    .ok();
    store.enqueue(unknown, RuntimeCounters::default()).unwrap();

    let mut anthropic = event(
        "known-protocol",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    anthropic.target_format = Some(ProviderProtocol::Anthropic);
    anthropic.stream_trace = serde_json::from_value(json!({
        "usage": {"inputTokens": 5, "outputTokens": 2}
    }))
    .ok();
    store
        .enqueue(anthropic, RuntimeCounters::default())
        .unwrap();
    store.flush().unwrap();

    let analytics = store.analytics("24h").unwrap();
    assert_eq!(
        analytics["tokenUsage"]["tokenAccountingSemantics"],
        "unknown"
    );

    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn event_pages_support_keyset_cursors_filters_and_change_replay() {
    let dir = test_dir("pages");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let base = event_now();

    let mut first = event(
        "client-a",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        base - 2.0,
    );
    first.request_id = Some("request-a".into());
    let first_change = store.enqueue(first, RuntimeCounters::default()).unwrap();

    let mut second = event(
        "client-b",
        KIND_CLIENT,
        0,
        RuntimeEventPhase::InFlight,
        None,
        base - 1.0,
    );
    second.request_id = Some("request-b".into());
    let second_in_flight = store
        .enqueue(second.clone(), RuntimeCounters::default())
        .unwrap();
    second.status_code = 500;
    second.phase = Some(RuntimeEventPhase::Completed);
    second.outcome = Some(RuntimeEventOutcome::Failed);
    second.failure_kind = Some(RuntimeFailureKind::UpstreamHttpStatus);
    let second_completed = store.enqueue(second, RuntimeCounters::default()).unwrap();

    let mut third = event(
        "upstream-a",
        KIND_UPSTREAM,
        503,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Failed),
        base,
    );
    third.request_id = Some("request-a".into());
    let third_change = store.enqueue(third, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();

    let newest = store
        .events(None, None, 2, None, None, None, None, None)
        .unwrap();
    assert_eq!(newest.len(), 3, "store returns limit + 1 for hasMore");
    assert_eq!(newest[0].id, "upstream-a");
    assert_eq!(newest[1].id, "client-b");

    let zero_cursor = store
        .events(Some(0), Some(0), 2, None, None, None, None, None)
        .unwrap();
    assert_eq!(zero_cursor[0].id, "upstream-a");

    let older = store
        .events(Some(newest[1].seq), None, 50, None, None, None, None, None)
        .unwrap();
    assert_eq!(
        older
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["client-a"]
    );

    let filtered = store
        .events(
            None,
            None,
            50,
            Some(KIND_CLIENT),
            Some("request-b"),
            Some("failed"),
            Some(base - 1.5),
            Some(base - 0.5),
        )
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].id, "client-b");

    let changes = store
        .events(
            None,
            Some(first_change.change_seq),
            50,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        changes
            .iter()
            .map(|item| item.change_seq)
            .collect::<Vec<_>>(),
        vec![
            second_in_flight.change_seq,
            second_completed.change_seq,
            third_change.change_seq,
        ]
    );
    assert_eq!(changes[0].seq, second_in_flight.seq);
    assert_eq!(changes[1].seq, second_in_flight.seq);
    assert!(
        store
            .change_cursor_valid(Some(first_change.change_seq))
            .unwrap()
    );
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn change_window_keeps_the_newest_changes_and_rejects_expired_cursors() {
    let dir = test_dir("change-window");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut first_change_seq = 0;
    let mut latest_change_seq = 0;
    for index in 0..(RECENT_CHANGES_LIMIT + 5) {
        let change = store
            .enqueue(
                event(
                    &format!("window-{index}"),
                    KIND_CLIENT,
                    200,
                    RuntimeEventPhase::Completed,
                    Some(RuntimeEventOutcome::Succeeded),
                    event_now(),
                ),
                RuntimeCounters::default(),
            )
            .unwrap();
        if index == 0 {
            first_change_seq = change.change_seq;
        }
        latest_change_seq = change.change_seq;
    }
    assert!(!store.change_cursor_valid(Some(first_change_seq)).unwrap());
    assert!(
        store
            .change_cursor_valid(Some(latest_change_seq - 1))
            .unwrap()
    );
    let latest = store
        .events(
            None,
            Some(latest_change_seq - 1),
            50,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(latest.len(), 1);
    assert_eq!(latest[0].change_seq, latest_change_seq);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn analytics_excludes_in_flight_rows() {
    let dir = test_dir("analytics-in-flight");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    store
        .enqueue(
            event(
                "pending-client",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::InFlight,
                None,
                event_now(),
            ),
            RuntimeCounters::default(),
        )
        .unwrap();
    store.flush().unwrap();
    let analytics = store.analytics("all").unwrap();
    assert_eq!(analytics["clientRequests"], 0);
    assert_eq!(analytics["latencyBuckets"]["under1s"], 0);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn newer_schema_is_rejected_before_writes() {
    let dir = test_dir("future-schema");
    let path = dir.join("runtime.sqlite3");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES('schema_version','4');",
        )
        .unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    let error = match RuntimeStore::new(&path) {
        Ok(_) => panic!("future schema must be rejected"),
        Err(error) => error,
    };
    assert!(error.contains("newer than supported"));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    remove_test_dir(&dir);
}

#[test]
fn successful_commit_clears_degraded_and_backpressure_state() {
    let dir = test_dir("recovery");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    store.inner.backpressure.store(true, Ordering::Release);
    store.inner.hard_backpressure.store(true, Ordering::Release);
    store.inner.state.lock().unwrap().last_error = Some("temporary failure".into());
    store
        .enqueue(
            event(
                "recovery-event",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    store.flush().unwrap();
    let summary = store.summary();
    assert_eq!(summary.storage.state, "ready");
    assert!(!store.is_backpressured());
    assert!(summary.storage.last_error.is_none());
    assert_eq!(summary.storage.pending_events, 0);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn failed_flush_keeps_pending_and_recovers_without_losing_the_event() {
    let dir = test_dir("write-failure");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    store.set_test_write_failure(true);
    store
        .enqueue(
            event(
                "write-failure-event",
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            RuntimeCounters {
                client_requests: 1,
                client_successes: 1,
                ..RuntimeCounters::default()
            },
        )
        .unwrap();
    assert!(store.flush().is_err());
    let degraded = store.summary();
    assert_eq!(degraded.storage.state, "degraded");
    assert!(degraded.storage.pending_events > 0);

    store.set_test_write_failure(false);
    store.flush().unwrap();
    let recovered = store.summary();
    assert_eq!(recovered.storage.state, "ready");
    assert_eq!(recovered.storage.pending_events, 0);
    assert!(store.event("write-failure-event").unwrap().is_some());
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn hard_backpressure_is_bounded_and_clears_after_recovery() {
    let dir = test_dir("backpressure");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    store.set_test_write_failure(true);
    let mut rejected = false;
    for index in 0..(PENDING_EVENTS_LIMIT + 128) {
        let result = store.enqueue(
            event(
                &format!("backpressure-{index}"),
                KIND_CLIENT,
                200,
                RuntimeEventPhase::Completed,
                Some(RuntimeEventOutcome::Succeeded),
                event_now(),
            ),
            RuntimeCounters::default(),
        );
        if result.is_err() {
            rejected = true;
            break;
        }
    }
    assert!(rejected, "pending hard limit must reject new events");
    assert!(store.is_backpressured());
    let summary = store.summary();
    assert_eq!(summary.storage.state, "backpressure");
    assert!(summary.storage.pending_events <= PENDING_EVENTS_LIMIT);

    store.set_test_write_failure(false);
    store.flush().unwrap();
    assert!(!store.is_backpressured());
    assert_eq!(store.summary().storage.pending_events, 0);
    drop(store);
    remove_test_dir(&dir);
}

#[test]
fn pending_batch_resets_local_byte_accounting_after_take() {
    let mut pending = PendingBatch::default();
    let message = WriteMessage {
        seq: 1,
        change_seq: 1,
        event: event(
            "pending-bytes",
            KIND_CLIENT,
            200,
            RuntimeEventPhase::Completed,
            Some(RuntimeEventOutcome::Succeeded),
            event_now(),
        ),
        counters: RuntimeCounters::default(),
        bytes: 17,
    };
    let inner = Arc::new(Inner {
        path: PathBuf::from("pending-bytes.sqlite3"),
        sender: mpsc::sync_channel(1).0,
        state: Mutex::new(StoreState {
            next_seq: 1,
            next_change_seq: 1,
            reset_generation: 0,
            history_generation: 0,
            counters: RuntimeCounters::default(),
            active_sequences: HashMap::new(),
            recent_changes: VecDeque::new(),
            latest_event: None,
            last_commit_at: None,
            last_error: None,
            storage: CachedStorageMetrics::default(),
            storage_refreshed_at: std::time::Instant::now(),
        }),
        pending_events: AtomicUsize::new(0),
        pending_bytes: AtomicUsize::new(0),
        backpressure: AtomicBool::new(false),
        hard_backpressure: AtomicBool::new(false),
        #[cfg(test)]
        fail_writes: AtomicBool::new(false),
    });
    pending.upsert(message, &inner);
    assert_eq!(pending.bytes, 17);
    assert_eq!(pending.take().len(), 1);
    assert_eq!(pending.bytes, 0);
}

/// 列表投影是 macOS App 的主加载路径(单事件详情只在选中时才拉),漏掉客户端声明
/// 那边就恒显示「未识别项目」。同时钉住 codexMetadata 仍在:两个归因来源并存,
/// 不是互斥的。
#[test]
fn list_item_wire_carries_client_declared_project_attribution() {
    let mut value = event(
        "wire-declared",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.client_declared = serde_json::from_value(json!({
        "project": "automode-proxy",
        "workspace": ".../.claude/automode-proxy",
        "gitRemote": "https://github.com/domoxiaojun/sumpter.git",
        "user": "kkl",
    }))
    .ok();
    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();

    assert_eq!(wire["projectName"], "automode-proxy");
    assert_eq!(wire["projectSource"], "workspace_local");
    assert_eq!(wire["localUser"], "kkl");
    assert_eq!(wire["clientDeclared"]["project"], "automode-proxy");
    assert_eq!(wire["clientDeclared"]["user"], "kkl");
    assert_eq!(
        wire["clientDeclared"]["workspace"],
        ".../.claude/automode-proxy"
    );
    assert_eq!(
        wire["clientDeclared"]["gitRemote"],
        "https://github.com/domoxiaojun/sumpter.git"
    );
    assert!(
        wire.get("client_declared").is_none(),
        "列表投影只能是 camelCase 形状"
    );
}

#[test]
fn list_item_wire_carries_grok_client_metadata() {
    let mut value = event(
        "wire-grok",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.grok_metadata = serde_json::from_value(json!({
        "sessionID": "sess-1",
        "convID": "conv-1",
        "clientIdentifier": "grok-shell",
        "clientVersion": "0.2.119",
    }))
    .ok();
    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
    assert_eq!(wire["grokMetadata"]["sessionID"], "sess-1");
    assert_eq!(wire["grokMetadata"]["convID"], "conv-1");
    assert_eq!(wire["grokMetadata"]["clientIdentifier"], "grok-shell");
    assert!(wire.get("grok_metadata").is_none());
}

#[test]
fn list_item_wire_carries_codex_local_user_from_source_workspace_path() {
    let mut value = event(
        "wire-codex-local",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.client_kind = Some(ClientKind::Codex);
    value.codex_metadata = serde_json::from_value(json!({
        "workspaces": {".../claude/sumpter": {}},
        "sourceWorkspacePaths": ["/Users/kkl/Documents/claude/sumpter"],
    }))
    .ok();
    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
    assert_eq!(wire["projectName"], "sumpter");
    assert_eq!(wire["projectSource"], "workspace_local");
    assert_eq!(wire["localUser"], "kkl");
}

#[test]
fn list_item_wire_carries_codex_thread_scope() {
    let mut value = event(
        "wire-ambient",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.client_kind = Some(ClientKind::Codex);
    value.codex_metadata = serde_json::from_value(json!({
        "threadSource": "ambient_suggestion_safety"
    }))
    .ok();
    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
    assert_eq!(wire["codexThreadClass"], "ambient");
    assert_eq!(wire["attributionScope"], "internal_feature");
}

#[test]
fn codex_guardian_projection_is_rebuilt_for_existing_events() {
    let dir = test_dir("guardian-projection");
    let path = dir.join("runtime.sqlite3");
    let (store, _) = RuntimeStore::new(&path).unwrap();
    let mut value = event(
        "guardian",
        KIND_CLIENT,
        400,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Failed),
        event_now(),
    );
    value.client_kind = Some(ClientKind::Codex);
    value.codex_metadata =
        CodexMetadata::from_request(&[("x-openai-subagent".into(), "guardian".into())], None);
    store.enqueue(value, RuntimeCounters::default()).unwrap();
    store.flush().unwrap();
    drop(store);
    let connection = Connection::open(&path).unwrap();
    connection.execute("UPDATE runtime_events SET projection_version=7,codex_thread_class='unknown',attribution_scope='unknown'", []).unwrap();
    drop(connection);
    let mut connection = Connection::open(&path).unwrap();
    setup_connection(&mut connection).unwrap();
    // flush only drains queued writes; startup backfill runs asynchronously.
    // Drive the same maintenance batches deterministically for this fixture.
    while run_projection_maintenance(&mut connection).unwrap() {}
    let result: (String, String, i64) = connection.query_row(
        "SELECT codex_thread_class,attribution_scope,projection_version FROM runtime_events WHERE event_id='guardian'", [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?))).unwrap();
    assert_eq!(
        result,
        (
            "guardian_review".into(),
            "internal_feature".into(),
            PROJECTION_VERSION
        )
    );
    drop(connection);
    remove_test_dir(&dir);
}

#[test]
fn resource_requests_are_not_attributed_to_unidentified_project() {
    let mut value = event(
        "wire-resource",
        KIND_CLIENT,
        499,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Cancelled),
        event_now(),
    );
    value.client_kind = Some(ClientKind::Codex);
    value.client_model = Some(RESOURCE_ROUTING_MODEL.into());
    value.codex_metadata = serde_json::from_value(json!({
        "originator": "Codex Desktop"
    }))
    .ok();

    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();
    assert_eq!(wire["projectName"], "internal_feature");
    assert_eq!(wire["projectSource"], "internal_feature");
    assert_eq!(wire["attributionScope"], "internal_feature");
}

#[test]
fn list_item_wire_preserves_id_and_ms_acronyms() {
    let mut value = event(
        "wire-acronyms",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.request_id = Some("request-wire".into());
    value.session_id = Some("session-wire".into());
    value.endpoint_id = Some("endpoint-wire".into());
    value.pool_id = Some("primary".into());
    value.message = Some("bridge openai-responses".into());
    value.duration_ms = 42;
    value.ttfb_ms = Some(7);
    let wire = serde_json::to_value(RuntimeEventListItem::from_change(1, 2, value)).unwrap();

    assert_eq!(wire["requestID"], "request-wire");
    assert_eq!(wire["sessionID"], "session-wire");
    assert_eq!(wire["endpointID"], "endpoint-wire");
    assert_eq!(wire["message"], "bridge openai-responses");
    assert_eq!(wire["durationMS"], 42);
    assert_eq!(wire["ttfbMS"], 7);
    assert!(
        wire.get("poolID").is_none(),
        "新列表 wire 不得输出废弃 poolID"
    );
    for legacy in ["requestId", "endpointId", "durationMs", "ttfbMs"] {
        assert!(wire.get(legacy).is_none(), "unexpected wire key {legacy}");
    }
}

#[test]
fn runtime_change_wire_omits_legacy_pool_id_from_detail_and_sse_shapes() {
    let mut value = event(
        "wire-detail-pool",
        KIND_CLIENT,
        200,
        RuntimeEventPhase::Completed,
        Some(RuntimeEventOutcome::Succeeded),
        event_now(),
    );
    value.pool_id = Some("primary".into());
    let wire = serde_json::to_value(RuntimeChange {
        seq: 7,
        change_seq: 8,
        event: value,
    })
    .unwrap();
    assert!(wire["event"].get("poolID").is_none());
    assert!(wire["event"].get("poolId").is_none());
}
