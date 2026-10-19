use super::*;

pub(crate) fn assert_matches_scan(connection: &Connection) -> StorageMetrics {
    let expected = connection
        .query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),
                    COALESCE(SUM(payload_bytes),0),MIN(seq),MAX(seq),MIN(timestamp),MAX(timestamp)
             FROM runtime_events",
            params![],
            |row| {
                Ok(StorageMetrics {
                    event_count: row.get(0)?,
                    completed_events: row.get(1)?,
                    in_flight_events: row.get(2)?,
                    payload_bytes: row.get(3)?,
                    min_seq: row.get(4)?,
                    max_seq: row.get(5)?,
                    earliest_timestamp: row.get(6)?,
                    latest_timestamp: row.get(7)?,
                })
            },
        )
        .unwrap();
    assert_eq!(read(connection).unwrap(), expected);
    expected
}

fn fixture() -> Connection {
    let mut connection = Connection::open_in_memory().unwrap();
    crate::entities::create_schema(&mut connection).unwrap();
    connection
}

fn upsert(connection: &Connection, seq: i64, timestamp: f64, in_flight: i64, bytes: i64) {
    connection
        .execute(
            "INSERT INTO runtime_events(seq,change_seq,event_id,timestamp,kind,status_code,
             is_in_flight,payload_json,payload_bytes,created_at,updated_at)
         VALUES(?1,?1,?2,?3,'client',200,?4,'{}',?5,0,0)
         ON CONFLICT(event_id) DO UPDATE SET timestamp=excluded.timestamp,
             is_in_flight=excluded.is_in_flight,payload_bytes=excluded.payload_bytes",
            params![seq, format!("event-{seq}"), timestamp, in_flight, bytes],
        )
        .unwrap();
}

#[test]
fn incremental_totals_follow_upserts_boundaries_and_transaction_rollback() {
    let mut connection = fixture();
    ensure(&mut connection).unwrap();
    assert_eq!(assert_matches_scan(&connection).min_seq, None);
    upsert(&connection, 1, 30.0, 1, 10);
    upsert(&connection, 2, 10.0, 0, 20);
    assert_eq!(assert_matches_scan(&connection).in_flight_events, 1);
    upsert(&connection, 1, 20.0, 0, 100);
    upsert(&connection, 1, 20.0, 0, 100);
    assert_eq!(assert_matches_scan(&connection).payload_bytes, 120);
    upsert(&connection, 1, 40.0, 0, 5);
    let before = assert_matches_scan(&connection);
    {
        let transaction = connection.transaction().unwrap();
        upsert(&transaction, 3, 50.0, 1, 77);
        upsert(&transaction, 1, 0.0, 1, 11);
        transaction
            .execute("DELETE FROM runtime_events WHERE seq=2", params![])
            .unwrap();
        assert_matches_scan(&transaction);
        // Dropping the transaction rolls back both events and derived totals.
    }
    assert_eq!(assert_matches_scan(&connection), before);
    connection
        .execute("DELETE FROM runtime_events WHERE seq=2", params![])
        .unwrap();
    assert_eq!(
        assert_matches_scan(&connection).earliest_timestamp,
        Some(40.0)
    );
    connection
        .execute("DELETE FROM runtime_events", params![])
        .unwrap();
    assert_eq!(assert_matches_scan(&connection).payload_bytes, 0);
}

#[test]
fn bootstrap_repairs_missing_structures_and_retries_atomically() {
    let mut connection = fixture();
    upsert(&connection, 1, 10.0, 1, 123);
    upsert(&connection, 2, 20.0, 0, 456);
    ensure(&mut connection).unwrap();
    let baseline = assert_matches_scan(&connection);
    // A ready database must not rewrite its baseline on every worker open.
    connection
        .execute_batch(
            "CREATE TRIGGER test_reject_metrics_update BEFORE UPDATE ON runtime_storage_metrics
         BEGIN SELECT RAISE(ABORT,'test initialization rollback'); END;",
        )
        .unwrap();
    ensure(&mut connection).unwrap();
    connection
        .execute_batch("DROP TRIGGER runtime_storage_metrics_update_v1;")
        .unwrap();
    upsert(&connection, 1, 15.0, 0, 9);
    assert!(ensure(&mut connection).is_err());
    assert_eq!(
        read(&connection).unwrap().payload_bytes,
        baseline.payload_bytes
    );
    connection
        .execute_batch("DROP TRIGGER test_reject_metrics_update;")
        .unwrap();
    ensure(&mut connection).unwrap();
    assert_matches_scan(&connection);
    connection
        .execute("DELETE FROM runtime_storage_metrics", params![])
        .unwrap();
    ensure(&mut connection).unwrap();
    assert_matches_scan(&connection);
    connection
        .execute_batch("DROP TABLE runtime_storage_metrics;")
        .unwrap();
    ensure(&mut connection).unwrap();
    assert_matches_scan(&connection);
    // Simulate an older binary recreating only the event schema: its DROP
    // removes event triggers but leaves our now-stale auxiliary row intact.
    connection
        .execute_batch("DROP TABLE runtime_events;")
        .unwrap();
    crate::entities::create_schema(&mut connection).unwrap();
    ensure(&mut connection).unwrap();
    assert_eq!(assert_matches_scan(&connection).event_count, 0);
    upsert(&connection, 9, 90.0, 1, 90);
    assert_eq!(assert_matches_scan(&connection).event_count, 1);
}

#[test]
fn storage_metrics_query_uses_singleton_and_index_boundaries_without_aggregation() {
    let mut connection = fixture();
    ensure(&mut connection).unwrap();
    let plan = connection
        .prepare(&format!("EXPLAIN QUERY PLAN {SELECT}"))
        .unwrap()
        .query_map(params![], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap()
        .join("\n");
    assert!(plan.contains("SEARCH runtime_storage_metrics"), "{plan}");
    assert!(
        plan.contains("COVERING INDEX runtime_events_timestamp"),
        "{plan}"
    );
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
    // SQLite may describe ORDER BY ... LIMIT 1 as SCAN even though it visits
    // only one boundary entry. Check bytecode as well as the query plan.
    let opcodes = connection
        .prepare(&format!("EXPLAIN {SELECT}"))
        .unwrap()
        .query_map(params![], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>>>()
        .unwrap();
    assert!(!opcodes.iter().any(|opcode| opcode.starts_with("Agg")));
    assert_eq!(
        opcodes
            .iter()
            .filter(|opcode| *opcode == "DecrJumpZero")
            .count(),
        4
    );
}

#[test]
#[ignore = "生成隔离的 1 万/10 万条基准库；设置 SUMPTER_STORAGE_BENCH_DIR 后手动运行"]
fn storage_metrics_benchmark_fixtures() {
    let directory = std::path::PathBuf::from(std::env::var("SUMPTER_STORAGE_BENCH_DIR").unwrap());
    std::fs::create_dir_all(&directory).unwrap();
    for count in [10_000, 100_000] {
        let path = directory.join(format!("{count}.sqlite3"));
        assert!(!path.exists(), "refuse to overwrite benchmark database");
        let mut connection = Connection::open(&path).unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;")
            .unwrap();
        crate::entities::create_schema(&mut connection).unwrap();
        ensure(&mut connection).unwrap();
        let transaction = connection.transaction().unwrap();
        transaction
            .execute_batch(&format!(
                "WITH RECURSIVE samples(n) AS (
                 VALUES(1) UNION ALL SELECT n+1 FROM samples WHERE n<{count}
             ) INSERT INTO runtime_events(seq,change_seq,event_id,timestamp,kind,status_code,
                 is_in_flight,payload_json,payload_bytes,created_at,updated_at)
             SELECT n,n,printf('sample-%d',n),n*1.0,'client',200,n%2,
                 printf('%02048d',n),2048,0,0 FROM samples;"
            ))
            .unwrap();
        transaction.commit().unwrap();
        assert_eq!(assert_matches_scan(&connection).event_count, count);
        connection
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
    }
}
