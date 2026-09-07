use super::{
    APPLE_EPOCH_OFFSET_SECS, CachedStorageMetrics, Connection, EventProjection, HashMap,
    KIND_CLIENT, KIND_UPSTREAM, OpenFlags, OptionalExtension, PROJECTION_VERSION, Path,
    RuntimeChange, RuntimeCounters, RuntimeEvent, RuntimeEventOutcome, RuntimeEventPhase,
    RuntimeFailureKind, RuntimeFailurePhase, SCHEMA_VERSION, STATUS_CLIENT_DISCONNECTED,
    StoreState, VecDeque, WriteMessage, counters_from_connection, hourly_bucket_start,
    mark_event_hourly_rollup_dirty, mark_hourly_rollup_bucket, mark_request_hourly_rollups_dirty,
    now, option_token, params,
};

pub(super) fn check_existing_schema(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| error.to_string())?;
    let has_meta = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='runtime_meta'",
            [],
            |_| Ok(()),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .is_some();
    if !has_meta {
        return Ok(());
    }
    let version = meta_i64(&connection, "schema_version")
        .map_err(|error| error.to_string())?
        .unwrap_or(0);
    if version > SCHEMA_VERSION {
        return Err(format!(
            "runtime schema {version} is newer than supported {SCHEMA_VERSION}"
        ));
    }
    Ok(())
}

pub(super) fn harden_database_file(path: &Path) {
    #[cfg(unix)]
    if let Ok(metadata) = std::fs::metadata(path) {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        let _ = std::fs::set_permissions(path, permissions);
    }
}

/// Resolve crash leftovers before exposing the first snapshot. A request that
/// never received headers is not useful history. A request that did receive
/// headers is conservatively finalized as failed (or cancelled for 499): a
/// daemon restart means no terminal success was observed, even when HTTP 2xx
/// headers had already arrived.
pub(super) fn normalize_startup(
    connection: &mut Connection,
) -> rusqlite::Result<Vec<RuntimeChange>> {
    let transaction = connection.transaction()?;
    let stored_next_change_seq = meta_i64(&transaction, "next_change_seq")?.unwrap_or(1);
    let max_change_seq = transaction.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let mut next_change_seq = stored_next_change_seq.max(max_change_seq + 1).max(1);
    let mut retained_event_count = meta_i64(&transaction, "retained_event_count")?.unwrap_or(0);
    let mut normalized_changes = Vec::new();
    let mut statement = transaction.prepare(
        "SELECT seq,event_id,status_code,is_in_flight,payload_json
         FROM runtime_events WHERE is_in_flight = 1 ORDER BY change_seq ASC",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);
    for (seq, event_id, status_code, is_in_flight, payload) in rows {
        if is_in_flight != 0 && status_code == 0 {
            transaction.execute(
                "DELETE FROM runtime_events WHERE event_id=?1",
                params![event_id],
            )?;
            retained_event_count = retained_event_count.saturating_sub(1);
            continue;
        }
        let mut event: RuntimeEvent = serde_json::from_str(&payload).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
        event.phase = Some(RuntimeEventPhase::Completed);
        if status_code == STATUS_CLIENT_DISCONNECTED {
            event.outcome = Some(RuntimeEventOutcome::Cancelled);
            event
                .failure_kind
                .get_or_insert(RuntimeFailureKind::ClientCancelled);
        } else {
            event.outcome = Some(RuntimeEventOutcome::Failed);
            if (200..=399).contains(&status_code) {
                event
                    .failure_kind
                    .get_or_insert(RuntimeFailureKind::StreamInterrupted);
                event
                    .failure_phase
                    .get_or_insert(RuntimeFailurePhase::ResponseStream);
            }
        }
        let outcome = option_token(event.outcome);
        let failure_kind = option_token(event.failure_kind);
        let payload = serde_json::to_string(&event)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        transaction.execute(
            "UPDATE runtime_events SET change_seq=?2,is_in_flight=0,phase='completed',
             outcome=?3,failure_kind=?4,payload_json=?5,updated_at=?6 WHERE event_id=?1",
            params![
                event_id,
                next_change_seq,
                outcome,
                failure_kind,
                payload,
                now()
            ],
        )?;
        update_event_projection(&transaction, seq, &event, &payload)?;
        mark_event_hourly_rollup_dirty(&transaction, &event)?;
        normalized_changes.push(RuntimeChange {
            seq,
            change_seq: next_change_seq,
            event: event.clone(),
        });
        match event.kind.as_str() {
            KIND_CLIENT => {
                transaction.execute(
                    "UPDATE runtime_counters SET client_requests=client_requests+1,
                     client_failures=client_failures+?1,failovers=failovers+?2 WHERE id=1",
                    params![i64::from(event.is_failed()), i64::from(event.failover)],
                )?;
            }
            KIND_UPSTREAM => {
                transaction.execute(
                    "UPDATE runtime_counters SET upstream_attempts=upstream_attempts+1,
                     upstream_failures=upstream_failures+?1 WHERE id=1",
                    params![i64::from(event.is_failed())],
                )?;
            }
            _ => {}
        }
        next_change_seq += 1;
    }
    set_meta(&transaction, "next_change_seq", next_change_seq)?;
    set_meta(&transaction, "retained_event_count", retained_event_count)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER)
         FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    transaction.commit()?;
    Ok(normalized_changes)
}

pub(super) fn meta_i64(connection: &Connection, key: &str) -> rusqlite::Result<Option<i64>> {
    let value = connection
        .query_row(
            "SELECT value FROM runtime_meta WHERE key=?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(value.map(|value| value.parse::<i64>().unwrap_or_default()))
}

pub(super) fn set_meta(connection: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO runtime_meta(key,value) VALUES(?1,?2)
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        params![key, value.to_string()],
    )?;
    Ok(())
}

pub(super) fn set_meta_max(connection: &Connection, key: &str, value: i64) -> rusqlite::Result<()> {
    let current = meta_i64(connection, key)?.unwrap_or(value);
    set_meta(connection, key, current.max(value))
}

pub(super) fn set_storage_limit_meta(
    connection: &Connection,
    value: Option<i64>,
) -> rusqlite::Result<()> {
    match value {
        Some(value) => set_meta(connection, "storage_limit_bytes", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='storage_limit_bytes'",
                [],
            )?;
            Ok(())
        }
    }
}

pub(super) fn set_retention_max_age_meta(
    connection: &Connection,
    value: Option<i64>,
) -> rusqlite::Result<()> {
    match value {
        Some(value) => set_meta(connection, "retention_max_age_days", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='retention_max_age_days'",
                [],
            )?;
            Ok(())
        }
    }
}

pub(super) fn load_cached_storage(
    connection: &Connection,
) -> rusqlite::Result<CachedStorageMetrics> {
    let (
        event_count,
        completed_event_count,
        in_flight_event_count,
        oldest_event_at,
        newest_event_at,
        payload_bytes,
    ) = connection.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),
                COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),
                MIN(timestamp),MAX(timestamp),COALESCE(SUM(payload_bytes),0)
         FROM runtime_events",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<f64>>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    )?;
    let page_size = connection.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
    let page_count = connection.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?;
    let rollup_dirty_buckets = connection.query_row(
        "SELECT COUNT(*) FROM runtime_hourly_rollup_dirty",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let page_size = page_size.max(0) as u64;
    let page_count = page_count.max(0) as u64;
    let freelist_count = freelist_count.max(0) as u64;
    Ok(CachedStorageMetrics {
        event_count,
        completed_event_count,
        in_flight_event_count,
        oldest_event_at,
        newest_event_at,
        retained_from_seq: meta_i64(connection, "retained_from_seq")?.unwrap_or(1),
        payload_bytes: payload_bytes.max(0) as u64,
        live_bytes: page_count
            .saturating_sub(freelist_count)
            .saturating_mul(page_size),
        allocated_bytes: page_count.saturating_mul(page_size),
        schema_version: meta_i64(connection, "schema_version")?.unwrap_or(0),
        backfill_cursor: meta_i64(connection, "projection_backfill_cursor")?.unwrap_or(0),
        backfill_complete: meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) != 0,
        backfill_failed: meta_i64(connection, "projection_backfill_failed")?.unwrap_or(0),
        indexes_ready: meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) != 0,
        rollup_complete: meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) != 0,
        rollup_max_seq: meta_i64(connection, "hourly_rollup_max_seq")?.unwrap_or(0),
        rollup_history_generation: meta_i64(connection, "hourly_rollup_history_generation")?
            .unwrap_or(0),
        rollup_failed: meta_i64(connection, "hourly_rollup_failed")?.unwrap_or(0),
        rollup_dirty_buckets,
        user_deleted_events: meta_i64(connection, "user_deleted_events")?.unwrap_or(0),
        user_deleted_requests: meta_i64(connection, "user_deleted_requests")?.unwrap_or(0),
    })
}

pub(super) fn load_state(connection: &Connection) -> rusqlite::Result<StoreState> {
    let counters = connection.query_row(
        "SELECT client_requests,client_successes,client_failures,upstream_attempts,
                upstream_successes,upstream_failures,failovers FROM runtime_counters WHERE id=1",
        [],
        |row| {
            Ok(RuntimeCounters {
                client_requests: row.get(0)?,
                client_successes: row.get(1)?,
                client_failures: row.get(2)?,
                upstream_attempts: row.get(3)?,
                upstream_successes: row.get(4)?,
                upstream_failures: row.get(5)?,
                failovers: row.get(6)?,
            })
        },
    )?;
    let max_seq = connection.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let max_change_seq = connection.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let next_seq = meta_i64(connection, "next_seq")?
        .unwrap_or(1)
        .max(max_seq + 1)
        .max(1);
    let next_change_seq = meta_i64(connection, "next_change_seq")?
        .unwrap_or(1)
        .max(max_change_seq + 1)
        .max(1);
    let reset_generation = meta_i64(connection, "reset_generation")?.unwrap_or(0);
    let history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0);
    let latest_event = connection
        .query_row(
            "SELECT payload_json FROM runtime_events ORDER BY seq DESC LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|payload| serde_json::from_str(&payload))
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?;
    Ok(StoreState {
        next_seq,
        next_change_seq,
        reset_generation,
        history_generation,
        counters,
        active_sequences: HashMap::new(),
        recent_changes: VecDeque::new(),
        latest_event,
        last_commit_at: None,
        last_error: None,
        storage: load_cached_storage(connection)?,
        storage_refreshed_at: std::time::Instant::now(),
    })
}

pub(super) fn update_event_projection(
    connection: &Connection,
    seq: i64,
    event: &RuntimeEvent,
    payload: &str,
) -> rusqlite::Result<()> {
    let projection = EventProjection::from_event(event, payload.len());
    connection.execute(
        "UPDATE runtime_events SET
            projection_version=?2,payload_bytes=?3,session_key=?4,session_source=?5,
            project_id=?6,project_name=?7,project_source=?8,local_user=?9,workspace_paths_json=?10,
            endpoint_name=?11,feature_rule_id=?12,client_model=?13,
            effective_model=?14,upstream_model=?15,failure_phase=?16,source_format=?17,
            target_format=?18,route_mode=?19,upstream_status_code=?20,duration_ms=?21,
            ttfb_ms=?22,failover=?23,stream_terminal=?24,codex_metadata_present=?25,
            usage_present=?26,input_tokens=?27,output_tokens=?28,
            cache_read_input_tokens=?29,cache_creation_input_tokens=?30,
            reasoning_tokens=?31,uncached_input_tokens=?32,processed_input_tokens=?33,
            processed_total_tokens=?34,token_accounting_semantics=?35,
            token_accounting_quality=?36,tool_calls_json=?37
            ,codex_thread_class=?38,attribution_scope=?39,request_method=?40,
            request_path=?41,route_intent=?42,model_group_id=?43,model_group_name=?44,
            sticky_key=?45
         WHERE seq=?1",
        params![
            seq,
            PROJECTION_VERSION,
            projection.payload_bytes,
            projection.session_key,
            projection.session_source,
            projection.project_id,
            projection.project_name,
            projection.project_source,
            projection.local_user,
            projection.workspace_paths_json,
            projection.endpoint_name,
            projection.feature_rule_id,
            projection.client_model,
            projection.effective_model,
            projection.upstream_model,
            projection.failure_phase,
            projection.source_format,
            projection.target_format,
            projection.route_mode,
            projection.upstream_status_code,
            projection.duration_ms,
            projection.ttfb_ms,
            projection.failover,
            projection.stream_terminal,
            projection.codex_metadata_present,
            projection.usage_present,
            projection.input_tokens,
            projection.output_tokens,
            projection.cache_read_input_tokens,
            projection.cache_creation_input_tokens,
            projection.reasoning_tokens,
            projection.uncached_input_tokens,
            projection.processed_input_tokens,
            projection.processed_total_tokens,
            projection.token_accounting_semantics,
            projection.token_accounting_quality,
            projection.tool_calls_json,
            projection.codex_thread_class,
            projection.attribution_scope,
            projection.request_method,
            projection.request_path,
            projection.route_intent,
            projection.model_group_id,
            projection.model_group_name,
            projection.sticky_key,
        ],
    )?;
    Ok(())
}

#[derive(Debug, Default)]
pub(super) struct StorageRotation {
    pub(super) deleted_event_ids: Vec<String>,
}

pub(super) fn sqlite_live_bytes(connection: &Connection) -> rusqlite::Result<u64> {
    let page_size = connection.query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?;
    let page_count = connection.query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?;
    Ok((page_count.max(0) as u64)
        .saturating_sub(freelist_count.max(0) as u64)
        .saturating_mul(page_size.max(0) as u64))
}

/// Return the current event-time cutoff for the rolling retention window.
/// Runtime events use Apple reference-date seconds, while metadata timestamps
/// use Unix seconds; keeping this conversion at the storage boundary avoids
/// local-time and daylight-saving surprises.
pub(super) fn retention_age_cutoff(connection: &Connection) -> rusqlite::Result<Option<f64>> {
    let Some(days) = meta_i64(connection, "retention_max_age_days")? else {
        return Ok(None);
    };
    if days < 1 {
        return Ok(None);
    }
    Ok(Some(
        now() - APPLE_EPOCH_OFFSET_SECS - (days as f64 * 86_400.0),
    ))
}

pub(super) fn has_expired_completed_event(
    connection: &Connection,
    cutoff: Option<f64>,
) -> rusqlite::Result<bool> {
    let Some(cutoff) = cutoff else {
        return Ok(false);
    };
    connection.query_row(
        "SELECT EXISTS(
             SELECT 1
             FROM runtime_events AS candidate
             WHERE candidate.is_in_flight=0
               AND candidate.timestamp < ?1
               AND (
                   candidate.request_id IS NULL
                   OR trim(candidate.request_id)=''
                   OR NOT EXISTS(
                       SELECT 1 FROM runtime_events AS newer
                       WHERE newer.request_id=candidate.request_id
                         AND newer.timestamp >= ?1
                   )
               )
               AND (
                   candidate.request_id IS NULL
                   OR trim(candidate.request_id)=''
                   OR NOT EXISTS(
                       SELECT 1 FROM runtime_events AS active
                       WHERE active.request_id=candidate.request_id
                         AND active.is_in_flight=1
                   )
               )
             LIMIT 1
         )",
        params![cutoff],
        |row| row.get::<_, bool>(0),
    )
}

/// Keep the SQLite live page footprint under the configured limit and/or the
/// rolling age cutoff. Both dimensions are OR-ed: whichever condition is
/// reached first may remove the oldest eligible completed request group.
///
/// A request group is indivisible for rotation. If any row in the group is
/// still in flight, the entire group is protected; this prevents a partial
/// request chain from appearing in analytics or exports.
pub(super) fn rotate_retention(connection: &Connection) -> rusqlite::Result<StorageRotation> {
    let capacity_limit = meta_i64(connection, "storage_limit_bytes")?
        .filter(|value| *value > 0)
        .map(|value| value as u64);
    let age_cutoff = retention_age_cutoff(connection)?;
    if capacity_limit.is_none() && age_cutoff.is_none() {
        return Ok(StorageRotation::default());
    }

    let mut rotation = StorageRotation::default();
    loop {
        let over_capacity = match capacity_limit {
            Some(limit) => sqlite_live_bytes(connection)? > limit,
            None => false,
        };
        let over_age = has_expired_completed_event(connection, age_cutoff)?;
        if !over_capacity && !over_age {
            break;
        }

        // When capacity is exceeded it is valid to evict the oldest eligible
        // group even if that group's events are newer than the age cutoff.
        // When only age is exceeded, require the whole group to be older than
        // the cutoff so no fresh row is removed as a side effect.
        let candidate = connection
            .query_row(
                "SELECT event_id,request_id,kind,timestamp
                 FROM runtime_events AS candidate
                 WHERE candidate.is_in_flight=0
                   AND (
                       candidate.request_id IS NULL
                       OR trim(candidate.request_id)=''
                       OR NOT EXISTS(
                           SELECT 1 FROM runtime_events AS active
                           WHERE active.request_id=candidate.request_id
                             AND active.is_in_flight=1
                       )
                   )
                   AND (
                       ?1=1
                       OR (
                           candidate.timestamp < ?2
                           AND (
                               candidate.request_id IS NULL
                               OR trim(candidate.request_id)=''
                               OR NOT EXISTS(
                                   SELECT 1 FROM runtime_events AS newer
                                   WHERE newer.request_id=candidate.request_id
                                     AND newer.timestamp >= ?2
                               )
                           )
                       )
                   )
                 ORDER BY candidate.timestamp ASC,candidate.seq ASC
                 LIMIT 1",
                params![i64::from(over_capacity), age_cutoff.unwrap_or(0.0)],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, f64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((event_id, request_id, kind, timestamp)) = candidate else {
            // This is the expected best-effort outcome when every old group
            // still contains an in-flight row, or only SQLite's fixed schema
            // footprint is larger than the requested capacity.
            break;
        };

        let mut event_ids = Vec::new();
        if let Some(request_id) = request_id.filter(|value| !value.trim().is_empty()) {
            mark_request_hourly_rollups_dirty(connection, &request_id)?;
            let mut statement = connection.prepare(
                "SELECT event_id FROM runtime_events
                 WHERE request_id=?1 AND is_in_flight=0
                 ORDER BY seq ASC",
            )?;
            event_ids = statement
                .query_map(params![request_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
        } else {
            if kind == KIND_CLIENT {
                mark_hourly_rollup_bucket(connection, hourly_bucket_start(timestamp))?;
            }
            event_ids.push(event_id);
        }
        if event_ids.is_empty() {
            break;
        }
        for event_id in &event_ids {
            connection.execute(
                "DELETE FROM runtime_events WHERE event_id=?1 AND is_in_flight=0",
                params![event_id],
            )?;
        }
        rotation.deleted_event_ids.extend(event_ids);
    }

    if rotation.deleted_event_ids.is_empty() {
        return Ok(rotation);
    }
    let counters = counters_from_connection(connection)?;
    connection.execute(
        "UPDATE runtime_counters SET client_requests=?1,client_successes=?2,
         client_failures=?3,upstream_attempts=?4,upstream_successes=?5,
         upstream_failures=?6,failovers=?7 WHERE id=1",
        params![
            counters.client_requests,
            counters.client_successes,
            counters.client_failures,
            counters.upstream_attempts,
            counters.upstream_successes,
            counters.upstream_failures,
            counters.failovers,
        ],
    )?;
    let history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0) + 1;
    set_meta(connection, "history_generation", history_generation)?;
    set_meta(
        connection,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    let retained_event_count =
        connection.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
    set_meta(connection, "retained_event_count", retained_event_count)?;
    let retained_from_seq = connection.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER)
         FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(connection, "retained_from_seq", retained_from_seq)?;
    Ok(rotation)
}

pub(super) fn rotate_retention_now(
    connection: &mut Connection,
) -> rusqlite::Result<StorageRotation> {
    let transaction = connection.transaction()?;
    let rotation = rotate_retention(&transaction)?;
    transaction.commit()?;
    Ok(rotation)
}

pub(super) fn write_batch(
    connection: &mut Connection,
    batch: &[WriteMessage],
) -> rusqlite::Result<StorageRotation> {
    if batch.is_empty() {
        return Ok(StorageRotation::default());
    }
    let transaction = connection.transaction()?;
    let mut retained_event_count = meta_i64(&transaction, "retained_event_count")?.unwrap_or(0);
    for message in batch {
        let previous = transaction
            .query_row(
                "SELECT kind,is_in_flight,timestamp FROM runtime_events WHERE event_id=?1",
                params![message.event.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )
            .optional()?;
        if let Some((kind, is_in_flight, timestamp)) = previous.as_ref()
            && kind == KIND_CLIENT
            && *is_in_flight == 0
        {
            mark_hourly_rollup_bucket(&transaction, hourly_bucket_start(*timestamp))?;
        }
        let payload = serde_json::to_string(&message.event)
            .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
        let projection = EventProjection::from_event(&message.event, payload.len());
        let created_at = now();
        transaction.execute(
            "INSERT INTO runtime_events(
                seq,change_seq,event_id,request_id,timestamp,kind,phase,outcome,status_code,
                client_kind,request_purpose,endpoint_id,failure_kind,is_in_flight,payload_json,
                created_at,updated_at,projection_version,payload_bytes,session_key,session_source,
                project_id,project_name,project_source,local_user,workspace_paths_json,endpoint_name,
                feature_rule_id,client_model,effective_model,upstream_model,failure_phase,
                source_format,target_format,route_mode,upstream_status_code,duration_ms,ttfb_ms,
                failover,stream_terminal,codex_metadata_present,usage_present,input_tokens,
                output_tokens,cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,
                uncached_input_tokens,processed_input_tokens,processed_total_tokens,
                token_accounting_semantics,token_accounting_quality,tool_calls_json
                ,codex_thread_class,attribution_scope,request_method,request_path,route_intent,model_group_id,model_group_name,
                sticky_key
             ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?16,
                ?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,
                ?32,?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,
                ?47,?48,?49,?50,?51,?52,?53,?54,?55,?56,?57,?58,?59,?60
             )
             ON CONFLICT(event_id) DO UPDATE SET
                change_seq=excluded.change_seq,payload_json=excluded.payload_json,
                request_id=excluded.request_id,timestamp=excluded.timestamp,kind=excluded.kind,
                phase=excluded.phase,outcome=excluded.outcome,status_code=excluded.status_code,
                client_kind=excluded.client_kind,request_purpose=excluded.request_purpose,
                endpoint_id=excluded.endpoint_id,failure_kind=excluded.failure_kind,
                is_in_flight=excluded.is_in_flight,updated_at=excluded.updated_at,
                projection_version=excluded.projection_version,payload_bytes=excluded.payload_bytes,
                session_key=excluded.session_key,session_source=excluded.session_source,
                project_id=excluded.project_id,project_name=excluded.project_name,
                project_source=excluded.project_source,
                local_user=excluded.local_user,
                workspace_paths_json=excluded.workspace_paths_json,
                endpoint_name=excluded.endpoint_name,
                feature_rule_id=excluded.feature_rule_id,client_model=excluded.client_model,
                effective_model=excluded.effective_model,upstream_model=excluded.upstream_model,
                failure_phase=excluded.failure_phase,source_format=excluded.source_format,
                target_format=excluded.target_format,route_mode=excluded.route_mode,
                upstream_status_code=excluded.upstream_status_code,duration_ms=excluded.duration_ms,
                ttfb_ms=excluded.ttfb_ms,failover=excluded.failover,
                stream_terminal=excluded.stream_terminal,
                codex_metadata_present=excluded.codex_metadata_present,
                usage_present=excluded.usage_present,input_tokens=excluded.input_tokens,
                output_tokens=excluded.output_tokens,
                cache_read_input_tokens=excluded.cache_read_input_tokens,
                cache_creation_input_tokens=excluded.cache_creation_input_tokens,
                reasoning_tokens=excluded.reasoning_tokens,
                uncached_input_tokens=excluded.uncached_input_tokens,
                processed_input_tokens=excluded.processed_input_tokens,
                processed_total_tokens=excluded.processed_total_tokens,
                token_accounting_semantics=excluded.token_accounting_semantics,
                token_accounting_quality=excluded.token_accounting_quality,
                tool_calls_json=excluded.tool_calls_json,
                codex_thread_class=excluded.codex_thread_class,
                attribution_scope=excluded.attribution_scope,
                request_method=excluded.request_method,
                request_path=excluded.request_path,
                route_intent=excluded.route_intent,
                model_group_id=excluded.model_group_id,model_group_name=excluded.model_group_name,
                sticky_key=excluded.sticky_key",
            params![
                message.seq,
                message.change_seq,
                message.event.id,
                message.event.request_id,
                message.event.timestamp,
                message.event.kind,
                option_token(message.event.phase),
                option_token(message.event.outcome),
                message.event.status_code,
                option_token(message.event.client_kind),
                option_token(message.event.request_purpose),
                message.event.endpoint_id,
                option_token(message.event.failure_kind),
                i64::from(message.event.is_in_flight()),
                payload,
                created_at,
                PROJECTION_VERSION,
                projection.payload_bytes,
                projection.session_key,
                projection.session_source,
                projection.project_id,
                projection.project_name,
                projection.project_source,
                projection.local_user,
                projection.workspace_paths_json,
                projection.endpoint_name,
                projection.feature_rule_id,
                projection.client_model,
                projection.effective_model,
                projection.upstream_model,
                projection.failure_phase,
                projection.source_format,
                projection.target_format,
                projection.route_mode,
                projection.upstream_status_code,
                projection.duration_ms,
                projection.ttfb_ms,
                projection.failover,
                projection.stream_terminal,
                projection.codex_metadata_present,
                projection.usage_present,
                projection.input_tokens,
                projection.output_tokens,
                projection.cache_read_input_tokens,
                projection.cache_creation_input_tokens,
                projection.reasoning_tokens,
                projection.uncached_input_tokens,
                projection.processed_input_tokens,
                projection.processed_total_tokens,
                projection.token_accounting_semantics,
                projection.token_accounting_quality,
                projection.tool_calls_json,
                projection.codex_thread_class,
                projection.attribution_scope,
                projection.request_method,
                projection.request_path,
                projection.route_intent,
            projection.model_group_id,
            projection.model_group_name,
            projection.sticky_key,
            ],
        )?;
        mark_event_hourly_rollup_dirty(&transaction, &message.event)?;
        if previous.is_none() {
            retained_event_count = retained_event_count.saturating_add(1);
        }
    }
    let latest = batch
        .iter()
        .max_by_key(|message| message.change_seq)
        .expect("non-empty batch");
    transaction.execute(
        "UPDATE runtime_counters SET client_requests=?2,client_successes=?3,
         client_failures=?4,upstream_attempts=?5,upstream_successes=?6,
         upstream_failures=?7,failovers=?8 WHERE id=1",
        params![
            1,
            latest.counters.client_requests,
            latest.counters.client_successes,
            latest.counters.client_failures,
            latest.counters.upstream_attempts,
            latest.counters.upstream_successes,
            latest.counters.upstream_failures,
            latest.counters.failovers,
        ],
    )?;
    let max_seq = batch.iter().map(|message| message.seq).max().unwrap_or(0);
    let max_change_seq = batch
        .iter()
        .map(|message| message.change_seq)
        .max()
        .unwrap_or(0);
    set_meta_max(&transaction, "next_seq", max_seq.max(1) + 1)?;
    set_meta_max(&transaction, "next_change_seq", max_change_seq.max(1) + 1)?;
    set_meta(&transaction, "retained_event_count", retained_event_count)?;
    let rotation = rotate_retention(&transaction)?;
    transaction.commit()?;
    Ok(rotation)
}
