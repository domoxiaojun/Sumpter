use super::*;
use crate::runtime_query::{
    DimensionKind, DimensionPageQuery, ErrorPageQuery, EventPageQuery, ExportFormat, ExportPrivacy,
    ExportQuery, ExportScope, RuntimeFilter, RuntimeQueryError, TrendGranularity, TrendQuery,
};

fn fixture(label: &str) -> (PathBuf, RuntimeStore) {
    let dir = std::env::temp_dir().join(format!(
        "sumpter-runtime-regression-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let (store, _) = RuntimeStore::new(dir.join("runtime.sqlite3")).unwrap();
    (dir, store)
}

fn sample(id: &str, in_flight: bool, timestamp: f64) -> RuntimeEvent {
    serde_json::from_value(json!({
        "id": id, "kind": "client", "timestamp": timestamp,
        "phase": if in_flight { "inFlight" } else { "completed" },
        "outcome": if in_flight { Value::Null } else { json!("failed") },
        "statusCode": if in_flight { 0 } else { 500 },
        "requestID": format!("request-{id}"), "sessionID": format!("session-{id}"),
        "agentRole": "subagent", "clientVariant": "cli", "agentName": "worker",
        "parentThreadId": "parent", "parentTurnId": "turn", "rootTurnId": "root"
    }))
    .unwrap()
}

fn counts(n: i64) -> RuntimeCounters {
    RuntimeCounters {
        client_requests: n,
        client_failures: n,
        ..Default::default()
    }
}

#[test]
fn analytics_filter_conversion_keeps_every_agent_dimension() {
    let filter = AnalyticsFilter {
        client_variant: Some(" cli ".into()),
        agent_role: Some(" subagent ".into()),
        agent_name: Some(" worker ".into()),
        parent_thread_id: Some(" parent ".into()),
        parent_turn_id: Some(" turn ".into()),
        root_turn_id: Some(" root ".into()),
        ..Default::default()
    };
    let query = filter.to_runtime_filter();
    assert_eq!(query.client_variant.as_deref(), Some("cli"));
    assert_eq!(query.agent_role.as_deref(), Some("subagent"));
    assert_eq!(query.agent_name.as_deref(), Some("worker"));
    assert_eq!(query.parent_thread_id.as_deref(), Some("parent"));
    assert_eq!(query.parent_turn_id.as_deref(), Some("turn"));
    assert_eq!(query.root_turn_id.as_deref(), Some("root"));
    for field in ["variant", "role", "name", "thread", "turn", "root"] {
        let mut one = AnalyticsFilter::default();
        match field {
            "variant" => one.client_variant = Some("cli".into()),
            "role" => one.agent_role = Some("subagent".into()),
            "name" => one.agent_name = Some("worker".into()),
            "thread" => one.parent_thread_id = Some("parent".into()),
            "turn" => one.parent_turn_id = Some("turn".into()),
            _ => one.root_turn_id = Some("root".into()),
        }
        assert!(one.is_active(), "{field}");
    }
    assert!(
        !AnalyticsFilter {
            agent_role: Some(" ".into()),
            ..Default::default()
        }
        .is_active()
    );
}

#[test]
fn retention_then_next_write_keeps_realtime_and_durable_counters_aligned() {
    let (dir, store) = fixture("retention-counters");
    store
        .enqueue(
            sample("old", false, event_now() - 3.0 * 86_400.0),
            counts(1),
        )
        .unwrap();
    store
        .enqueue(sample("fresh", false, event_now()), counts(2))
        .unwrap();
    store.flush().unwrap();
    store
        .set_retention(RuntimeRetentionUpdate {
            expected_revision: 1,
            max_age_days: Some(1),
            storage_limit_bytes: None,
        })
        .unwrap();
    assert_eq!(store.counters().client_requests, 1);
    // 保留策略不会改变 Engine 的历史累计输入；Store 只能使用其增量。
    store
        .enqueue(sample("next", false, event_now()), counts(3))
        .unwrap();
    assert_eq!(store.counters().client_requests, 2);
    store.flush().unwrap();
    assert_eq!(store.snapshot().unwrap().client_requests, 2);
    let mut updated = sample("next", false, event_now());
    updated.message = Some("metadata update".into());
    store.enqueue(updated, counts(3)).unwrap();
    store.flush().unwrap();
    assert_eq!(store.snapshot().unwrap().client_requests, 2);
    drop(store);
    let (store, snapshot) = RuntimeStore::new(dir.join("runtime.sqlite3")).unwrap();
    assert_eq!(snapshot.client_requests, 2);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn cleanup_and_other_session_deletion_preserve_live_event_sequence() {
    let (dir, store) = fixture("live-sequence");
    let live = sample("live", true, event_now());
    let first = store.enqueue(live.clone(), counts(0)).unwrap();
    assert_eq!(store.cleanup_before(1.0).unwrap().deleted_events, 0);
    store
        .enqueue(sample("other", false, event_now()), counts(1))
        .unwrap();
    store.delete_session("session-other").unwrap();
    let mut completed = live;
    completed.phase = Some(RuntimeEventPhase::Completed);
    completed.outcome = Some(RuntimeEventOutcome::Failed);
    completed.status_code = 500;
    let finished = store.enqueue(completed, counts(1)).unwrap();
    assert_eq!(first.seq, finished.seq);
    store.flush().unwrap();
    let page = crate::runtime_query::events_page(store.path(), &EventPageQuery::default()).unwrap();
    assert_eq!(page.events[0].seq, first.seq);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn snapshot_freezes_completed_membership_across_all_query_surfaces() {
    let (dir, store) = fixture("snapshot-membership");
    let mut live = sample("e11", true, 100.0);
    for n in 1..=12 {
        let value = if n == 11 {
            live.clone()
        } else {
            sample(&format!("e{n}"), false, 100.0)
        };
        store
            .enqueue(value, counts(n - i64::from(n >= 11)))
            .unwrap();
    }
    store.flush().unwrap();
    let first =
        crate::runtime_query::events_page(store.path(), &EventPageQuery::default()).unwrap();
    assert_eq!(first.total_count, 11);
    live.phase = Some(RuntimeEventPhase::Completed);
    live.outcome = Some(RuntimeEventOutcome::Failed);
    live.status_code = 500;
    store.enqueue(live, counts(12)).unwrap();
    let mut metadata_update = sample("e12", false, 100.0);
    metadata_update.message = Some("补充详情不改变首次完成水位".into());
    store.enqueue(metadata_update, counts(12)).unwrap();
    store.flush().unwrap();
    let snapshot_seq = Some(first.snapshot_seq);
    let snapshot_change_seq = Some(first.snapshot_change_seq);
    let history_generation = Some(first.history_generation);
    let page = crate::runtime_query::events_page(
        store.path(),
        &EventPageQuery {
            page: 2,
            snapshot_seq,
            snapshot_change_seq,
            history_generation,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total_count, 11);
    assert_eq!(
        page.events
            .iter()
            .map(|event| event.id.as_str())
            .collect::<Vec<_>>(),
        vec!["e1"]
    );
    let refreshed_first = crate::runtime_query::events_page(
        store.path(),
        &EventPageQuery {
            snapshot_seq,
            snapshot_change_seq,
            history_generation,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(refreshed_first.events.iter().any(|event| event.id == "e12"));
    assert!(!refreshed_first.events.iter().any(|event| event.id == "e11"));
    let dimensions = crate::runtime_query::dimension_page(
        store.path(),
        DimensionKind::AgentRole,
        &DimensionPageQuery {
            snapshot_seq,
            snapshot_change_seq,
            history_generation,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(dimensions.rows[0].requests, 11);
    let errors = crate::runtime_query::error_groups(
        store.path(),
        &ErrorPageQuery {
            snapshot_seq,
            snapshot_change_seq,
            history_generation,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        errors.groups.iter().map(|row| row.occurrences).sum::<i64>(),
        11
    );
    let trends = crate::runtime_query::trends(
        store.path(),
        &TrendQuery {
            from: 0.0,
            to: 3600.0,
            granularity: TrendGranularity::Hour,
            snapshot_seq,
            snapshot_change_seq,
            history_generation,
            filter: RuntimeFilter::default(),
        },
    )
    .unwrap();
    assert_eq!(trends.totals.client_requests, 11);
    let export = ExportQuery {
        scope: ExportScope::Events,
        format: ExportFormat::Jsonl,
        privacy: ExportPrivacy::Redacted,
        confirm_stored: false,
        snapshot_seq,
        snapshot_change_seq,
        history_generation,
        filter: RuntimeFilter::default(),
    };
    assert_eq!(
        crate::runtime_query::export_estimate(store.path(), &export)
            .unwrap()
            .row_count,
        11
    );
    assert_eq!(
        crate::runtime_query::stream_export(store.path(), &export, |_| Ok(()))
            .unwrap()
            .row_count,
        11
    );
    assert!(matches!(
        crate::runtime_query::events_page(
            store.path(),
            &EventPageQuery {
                snapshot_seq,
                history_generation,
                ..Default::default()
            }
        ),
        Err(RuntimeQueryError::SnapshotExpired { .. })
    ));
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn completion_watermark_migration_preserves_current_database_rows() {
    let (dir, store) = fixture("completion-migration");
    store
        .enqueue(sample("kept", false, event_now()), counts(1))
        .unwrap();
    store.flush().unwrap();
    let path = store.path().to_owned();
    drop(store);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP INDEX runtime_events_completion_change; ALTER TABLE runtime_events DROP COLUMN completed_change_seq; DELETE FROM runtime_meta WHERE key='completion_watermark_version'; UPDATE runtime_counters SET client_requests=99;").unwrap();
    drop(db);
    assert!(RuntimeStore::database_issue(&path).unwrap().is_none());
    let (store, snapshot) = RuntimeStore::new(&path).unwrap();
    assert_eq!(snapshot.client_requests, 1);
    assert_eq!(snapshot.recent_events[0].id, "kept");
    let db = read_connection(&path).unwrap();
    let completion = db
        .query_row(
            "SELECT completed_change_seq FROM runtime_events WHERE event_id='kept'",
            params![],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert!(completion > 0);
    drop(db);
    drop(store);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP INDEX runtime_events_completion_change; ALTER TABLE runtime_events DROP COLUMN completed_change_seq; DELETE FROM runtime_meta WHERE key='completion_watermark_version'; UPDATE runtime_meta SET value='999' WHERE key='schema_version'; UPDATE runtime_counters SET client_requests=99;").unwrap();
    drop(db);
    let before = std::fs::read(&path).unwrap();
    assert!(RuntimeStore::new(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let db = read_connection(&path).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT client_requests FROM runtime_counters WHERE id=1",
            params![],
            |row| row.get::<_, i64>(0)
        )
        .unwrap(),
        99
    );
    assert_eq!(db.query_row("SELECT COUNT(*) FROM pragma_table_info('runtime_events') WHERE name='completed_change_seq'", params![], |row| row.get::<_, i64>(0)).unwrap(), 0);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}
