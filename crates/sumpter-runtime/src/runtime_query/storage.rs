//! Runtime query storage domain.

use super::{
    API_VERSION, Connection, PROJECTION_VERSION, Path, PathBuf, QueryResult, RetentionStatus,
    StorageProbe, TransactionBehavior, meta_i64, read_connection,
};

pub fn storage_details(path: &Path) -> QueryResult<StorageProbe> {
    let mut connection = read_connection(path)?;
    storage_details_on(&mut connection, Some(path))
}

fn has_legacy_retention_columns(connection: &Connection) -> QueryResult<bool> {
    const LEGACY_COLUMNS: [&str; 7] = [
        "max_events",
        "max_age_days",
        "max_live_bytes",
        "evicted_events",
        "evicted_requests",
        "over_limit",
        "last_pruned_at",
    ];
    let mut statement = connection.prepare("PRAGMA table_info(runtime_retention)")?;
    let columns = statement
        .query_map(crate::database::params![], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns
        .iter()
        .any(|column| LEGACY_COLUMNS.contains(&column.as_str())))
}

pub fn storage_details_on(
    connection: &mut Connection,
    path: Option<&Path>,
) -> QueryResult<StorageProbe> {
    const REQUIRED_INDEXES: [&str; 8] = [
        "runtime_events_inflight_change_v2",
        "runtime_events_outcome_seq_v2",
        "runtime_events_time_seq_v2",
        "runtime_events_session_time_v2",
        "runtime_events_project_time_v2",
        "runtime_events_endpoint_time_v2",
        "runtime_events_model_time_v2",
        "runtime_events_failure_time_v2",
    ];
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let (
        retained_events,
        completed_events,
        in_flight_events,
        min_seq,
        max_seq,
        earliest_timestamp,
        latest_timestamp,
        payload_bytes,
    ) = transaction.query_row(
        "SELECT COUNT(*),\
                COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),\
                COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),\
                MIN(seq),MAX(seq),MIN(timestamp),MAX(timestamp),\
                COALESCE(SUM(payload_bytes),0) \
         FROM runtime_events",
        crate::database::params![],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        },
    )?;
    let max_age_days = meta_i64(&transaction, "retention_max_age_days")?;
    let storage_limit_bytes = meta_i64(&transaction, "storage_limit_bytes")?;
    let reset_generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0);
    let legacy_retention_detected = has_legacy_retention_columns(&transaction)?;
    let retention = transaction.query_row(
        "SELECT revision \
         FROM runtime_retention WHERE id=1",
        crate::database::params![],
        |row| {
            Ok(RetentionStatus {
                revision: row.get(0)?,
                max_age_days,
                storage_limit_bytes,
            })
        },
    )?;
    let existing_indexes = {
        let mut statement = transaction.prepare(
            "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'runtime_events_%_v2'",
        )?;
        statement
            .query_map(crate::database::params![], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let missing_indexes = REQUIRED_INDEXES
        .into_iter()
        .filter(|required| !existing_indexes.iter().any(|actual| actual == required))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let page_size = transaction
        .query_row("PRAGMA page_size", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?
        .max(0) as u64;
    let page_count = transaction
        .query_row("PRAGMA page_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?
        .max(0) as u64;
    let freelist_count = transaction
        .query_row("PRAGMA freelist_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?
        .max(0) as u64;
    let allocated_bytes = page_size.saturating_mul(page_count);
    let freelist_bytes = page_size.saturating_mul(freelist_count);
    let live_bytes = allocated_bytes.saturating_sub(freelist_bytes);
    let (database_bytes, wal_bytes) = path.map_or((0, 0), database_file_sizes);
    let projection_indexes_ready = meta_i64(&transaction, "projection_indexes_ready")?.unwrap_or(0)
        == 1
        && missing_indexes.is_empty();
    let hourly_rollup_dirty_buckets = transaction.query_row(
        "SELECT COUNT(*) FROM runtime_hourly_rollup_dirty",
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    let result = StorageProbe {
        api_version: API_VERSION,
        backend: "sqlite",
        schema_version: meta_i64(&transaction, "schema_version")?.unwrap_or(0),
        projection_version: PROJECTION_VERSION,
        projection_backfill_cursor: meta_i64(&transaction, "projection_backfill_cursor")?
            .unwrap_or(0),
        projection_backfill_complete: meta_i64(&transaction, "projection_backfill_complete")?
            .unwrap_or(0)
            == 1,
        projection_indexes_ready,
        missing_indexes,
        hourly_rollup_complete: meta_i64(&transaction, "hourly_rollup_complete")?.unwrap_or(0) == 1,
        hourly_rollup_max_seq: meta_i64(&transaction, "hourly_rollup_max_seq")?.unwrap_or(0),
        hourly_rollup_history_generation: meta_i64(
            &transaction,
            "hourly_rollup_history_generation",
        )?
        .unwrap_or(0),
        hourly_rollup_failed: meta_i64(&transaction, "hourly_rollup_failed")?.unwrap_or(0) == 1,
        hourly_rollup_dirty_buckets,
        retained_events,
        completed_events,
        in_flight_events,
        min_seq,
        max_seq,
        earliest_timestamp,
        latest_timestamp,
        retained_from_seq: meta_i64(&transaction, "retained_from_seq")?.unwrap_or(0),
        history_generation: meta_i64(&transaction, "history_generation")?.unwrap_or(0),
        reset_generation,
        user_deleted_events: meta_i64(&transaction, "user_deleted_events")?.unwrap_or(0),
        user_deleted_requests: meta_i64(&transaction, "user_deleted_requests")?.unwrap_or(0),
        payload_bytes,
        database_bytes,
        live_bytes,
        allocated_bytes,
        freelist_bytes,
        wal_bytes,
        pending_events: None,
        pending_bytes: None,
        retention,
        legacy_retention_detected,
    };
    transaction.commit()?;
    Ok(result)
}

fn database_file_sizes(path: &Path) -> (u64, u64) {
    let database_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    (database_bytes, wal_bytes)
}
