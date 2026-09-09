use super::{
    APPLE_EPOCH_OFFSET_SECS, CachedStorageMetrics, Connection, EventProjection, HashMap,
    KIND_CLIENT, KIND_UPSTREAM, OptionalExtension, Path, RuntimeChange, RuntimeEvent,
    RuntimeEventOutcome, RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase,
    STATUS_CLIENT_DISCONNECTED, StoreState, VecDeque, WriteMessage, counters_from_connection,
    hourly_bucket_start, mark_event_hourly_rollup_dirty, mark_hourly_rollup_bucket,
    mark_request_hourly_rollups_dirty, now, option_token, params,
};

/// Read-only preflight; rejected files must never enter schema setup or backfill.
pub(super) fn check_existing_schema(path: &Path) -> Result<(), String> {
    if let Some(issue) = super::RuntimeStore::database_issue(path)? {
        return Err(issue.message);
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
) -> crate::database::Result<Vec<RuntimeChange>> {
    let transaction = connection.transaction()?;
    let stored_next_change_seq = meta_i64(&transaction, "next_change_seq")?.unwrap_or(1);
    let max_change_seq = transaction.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        crate::database::params![],
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
        .query_map(crate::database::params![], |row| {
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
        let mut event: RuntimeEvent = serde_json::from_str(&payload)
            .map_err(|error| crate::database::Error::Conversion(Box::new(error)))?;
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
        event.refresh_cache_read();
        let payload = serde_json::to_string(&event)
            .map_err(|error| crate::database::Error::Conversion(Box::new(error)))?;
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
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    transaction.commit()?;
    Ok(normalized_changes)
}

pub(super) fn meta_i64(connection: &Connection, key: &str) -> crate::database::Result<Option<i64>> {
    use crate::entities::runtime_meta;
    use sea_orm::EntityTrait;
    let key = key.to_owned();
    let value = connection.orm(move |db| async move {
        runtime_meta::Entity::find_by_id(key).one(db.as_ref()).await
    })?;
    Ok(value.map(|value| value.value.parse::<i64>().unwrap_or_default()))
}

pub(super) fn set_meta(
    connection: &Connection,
    key: &str,
    value: i64,
) -> crate::database::Result<()> {
    use crate::entities::runtime_meta;
    use sea_orm::{EntityTrait, Set, sea_query::OnConflict};
    let model = runtime_meta::ActiveModel {
        key: Set(key.to_owned()),
        value: Set(value.to_string()),
    };
    connection.orm(move |db| async move {
        runtime_meta::Entity::insert(model)
            .on_conflict(
                OnConflict::column(runtime_meta::Column::Key)
                    .update_column(runtime_meta::Column::Value)
                    .to_owned(),
            )
            .exec_without_returning(db.as_ref())
            .await
            .map(|_| ())
    })
}

pub(super) fn set_meta_max(
    connection: &Connection,
    key: &str,
    value: i64,
) -> crate::database::Result<()> {
    let current = meta_i64(connection, key)?.unwrap_or(value);
    set_meta(connection, key, current.max(value))
}

pub(super) fn set_storage_limit_meta(
    connection: &Connection,
    value: Option<i64>,
) -> crate::database::Result<()> {
    match value {
        Some(value) => set_meta(connection, "storage_limit_bytes", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='storage_limit_bytes'",
                crate::database::params![],
            )?;
            Ok(())
        }
    }
}

pub(super) fn set_retention_max_age_meta(
    connection: &Connection,
    value: Option<i64>,
) -> crate::database::Result<()> {
    match value {
        Some(value) => set_meta(connection, "retention_max_age_days", value),
        None => {
            connection.execute(
                "DELETE FROM runtime_meta WHERE key='retention_max_age_days'",
                crate::database::params![],
            )?;
            Ok(())
        }
    }
}

pub(super) fn load_cached_storage(
    connection: &Connection,
) -> crate::database::Result<CachedStorageMetrics> {
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
        crate::database::params![],
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
    let page_size =
        connection.query_row("PRAGMA page_size", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    let page_count =
        connection.query_row("PRAGMA page_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    let rollup_dirty_buckets = connection.query_row(
        "SELECT COUNT(*) FROM runtime_hourly_rollup_dirty",
        crate::database::params![],
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

pub(super) fn load_state(connection: &Connection) -> crate::database::Result<StoreState> {
    let counters = super::models::load_counters(connection)?;
    let max_seq = connection.query_row(
        "SELECT COALESCE(MAX(seq), 0) FROM runtime_events",
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    let max_change_seq = connection.query_row(
        "SELECT COALESCE(MAX(change_seq), 0) FROM runtime_events",
        crate::database::params![],
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
            crate::database::params![],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .map(|payload| serde_json::from_str(&payload))
        .transpose()
        .map_err(|error| crate::database::Error::Conversion(Box::new(error)))?;
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
) -> crate::database::Result<()> {
    let projection = EventProjection::from_event(event, payload.len());
    use crate::entities::runtime_events;
    use sea_orm::{EntityTrait, Set};
    let mut model = projection.active_model();
    model.seq = Set(seq);
    connection.orm(move |db| async move {
        runtime_events::Entity::update(model)
            .exec(db.as_ref())
            .await
            .map(|_| ())
    })?;
    Ok(())
}

#[derive(Debug, Default)]
pub(super) struct StorageRotation {
    pub(super) deleted_event_ids: Vec<String>,
}

pub(super) fn sqlite_live_bytes(connection: &Connection) -> crate::database::Result<u64> {
    let page_size =
        connection.query_row("PRAGMA page_size", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    let page_count =
        connection.query_row("PRAGMA page_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    let freelist_count =
        connection.query_row("PRAGMA freelist_count", crate::database::params![], |row| {
            row.get::<_, i64>(0)
        })?;
    Ok((page_count.max(0) as u64)
        .saturating_sub(freelist_count.max(0) as u64)
        .saturating_mul(page_size.max(0) as u64))
}

/// Return the current event-time cutoff for the rolling retention window.
/// Runtime events use Apple reference-date seconds, while metadata timestamps
/// use Unix seconds; keeping this conversion at the storage boundary avoids
/// local-time and daylight-saving surprises.
pub(super) fn retention_age_cutoff(
    connection: &Connection,
) -> crate::database::Result<Option<f64>> {
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
) -> crate::database::Result<bool> {
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
pub(super) fn rotate_retention(
    connection: &Connection,
) -> crate::database::Result<StorageRotation> {
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
    let retained_event_count = connection.query_row(
        "SELECT COUNT(*) FROM runtime_events",
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(connection, "retained_event_count", retained_event_count)?;
    let retained_from_seq = connection.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER)
         FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(connection, "retained_from_seq", retained_from_seq)?;
    Ok(rotation)
}

pub(super) fn rotate_retention_now(
    connection: &mut Connection,
) -> crate::database::Result<StorageRotation> {
    let transaction = connection.transaction()?;
    let rotation = rotate_retention(&transaction)?;
    transaction.commit()?;
    Ok(rotation)
}

fn apply_session_project_fallback(
    connection: &Connection,
    projection: &mut EventProjection,
) -> crate::database::Result<()> {
    if projection.project_source != "missing_workspace_metadata"
        || projection.session_key == "unidentified_session"
    {
        return Ok(());
    }
    let mut statement = connection.prepare(
        "SELECT DISTINCT project_id,project_name,workspace_paths_json FROM runtime_events
         WHERE session_key=?1 AND project_source NOT IN ('missing_workspace_metadata','internal_feature')
         LIMIT 2",
    )?;
    let rows = statement
        .query_map(params![projection.session_key], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() == 1 {
        let (id, name, paths) = rows.into_iter().next().expect("one row");
        projection.project_id = id;
        projection.project_name = name;
        projection.project_source = "session_fallback";
        projection.workspace_paths_json = paths;
    } else if rows.len() > 1 {
        projection.project_source = "multiple_workspaces";
    }
    Ok(())
}

pub(super) fn write_batch(
    connection: &mut Connection,
    batch: &[WriteMessage],
) -> crate::database::Result<StorageRotation> {
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
            .map_err(|error| crate::database::Error::Conversion(Box::new(error)))?;
        let mut projection = EventProjection::from_event(&message.event, payload.len());
        apply_session_project_fallback(&transaction, &mut projection)?;
        let created_at = now();
        use crate::entities::runtime_events;
        use sea_orm::{EntityTrait, Iterable, Set, sea_query::OnConflict};
        let mut model = projection.active_model();
        model.seq = Set(message.seq);
        model.change_seq = Set(message.change_seq);
        model.event_id = Set(message.event.id.clone());
        model.request_id = Set(message.event.request_id.clone());
        model.timestamp = Set(message.event.timestamp);
        model.kind = Set(message.event.kind.clone());
        model.phase = Set(option_token(message.event.phase));
        model.outcome = Set(option_token(message.event.outcome));
        model.status_code = Set(message.event.status_code);
        model.client_kind = Set(option_token(message.event.client_kind));
        model.request_purpose = Set(option_token(message.event.request_purpose));
        model.endpoint_id = Set(message.event.endpoint_id.clone());
        model.failure_kind = Set(option_token(message.event.failure_kind));
        model.is_in_flight = Set(i64::from(message.event.is_in_flight()));
        model.payload_json = Set(payload);
        model.created_at = Set(created_at);
        model.updated_at = Set(created_at);
        transaction.orm(move |db| async move {
            use runtime_events::Column;
            runtime_events::Entity::insert(model)
                .on_conflict(
                    OnConflict::column(Column::EventId)
                        .update_columns(Column::iter().filter(|column| {
                            !matches!(column, Column::Seq | Column::EventId | Column::CreatedAt)
                        }))
                        .to_owned(),
                )
                .exec_without_returning(db.as_ref())
                .await
                .map(|_| ())
        })?;
        mark_event_hourly_rollup_dirty(&transaction, &message.event)?;
        if previous.is_none() {
            retained_event_count = retained_event_count.saturating_add(1);
        }
    }
    let latest = batch
        .iter()
        .max_by_key(|message| message.change_seq)
        .expect("non-empty batch");
    super::models::save_counters(&transaction, &latest.counters)?;
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
