use super::{
    AnalyticsFilter, Arc, Connection, Duration, HashSet, Inner, KIND_CLIENT, Ordering,
    PendingBatch, RuntimeCleanupMutation, RuntimeCleanupPreview, RuntimePricingMutation,
    RuntimePricingUpdate, RuntimeRetentionMutation, RuntimeRetentionUpdate, StorageRotation,
    counters_from_connection, hourly_bucket_start, load_cached_storage, load_state,
    mark_hourly_rollup_bucket, mark_request_hourly_rollups_dirty, meta_i64, now, params,
    rotate_retention_now, set_meta, set_retention_max_age_meta, set_storage_limit_meta,
    setup_connection, write_batch,
};

impl AnalyticsFilter {
    pub fn is_active(&self) -> bool {
        self.client_kind.is_some()
            || self.endpoint_id.is_some()
            || self.project_id.is_some()
            || self.project.is_some()
            || self.session_id.is_some()
            || self.from.is_some()
            || self.to.is_some()
    }
}

pub(super) fn refresh_cached_storage(
    inner: &Arc<Inner>,
    connection: &Connection,
    force: bool,
) -> rusqlite::Result<()> {
    let should_refresh = force
        || inner.state.lock().unwrap().storage_refreshed_at.elapsed() >= Duration::from_secs(1);
    if !should_refresh {
        return Ok(());
    }
    let storage = load_cached_storage(connection)?;
    let mut state = inner.state.lock().unwrap();
    state.storage = storage;
    state.history_generation = meta_i64(connection, "history_generation")?.unwrap_or(0);
    state.storage_refreshed_at = std::time::Instant::now();
    Ok(())
}

pub(super) fn reconcile_rotation_state(
    inner: &Arc<Inner>,
    connection: &Connection,
    rotation: &StorageRotation,
) -> rusqlite::Result<()> {
    if rotation.deleted_event_ids.is_empty() {
        return Ok(());
    }
    let refreshed = load_state(connection)?;
    let rotated = rotation.deleted_event_ids.iter().collect::<HashSet<_>>();
    let mut state = inner.state.lock().unwrap();
    state
        .recent_changes
        .retain(|change| !rotated.contains(&change.event.id));
    state.counters = refreshed.counters;
    state.latest_event = refreshed.latest_event;
    state.history_generation = refreshed.history_generation;
    Ok(())
}

pub(super) fn run_retention_maintenance(
    inner: &Arc<Inner>,
    connection: &mut Connection,
) -> rusqlite::Result<bool> {
    let rotation = rotate_retention_now(connection)?;
    if rotation.deleted_event_ids.is_empty() {
        return Ok(false);
    }
    refresh_cached_storage(inner, connection, true)?;
    reconcile_rotation_state(inner, connection, &rotation)?;
    Ok(true)
}

pub(super) fn commit_pending(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    pending: &mut PendingBatch,
) -> rusqlite::Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    #[cfg(test)]
    if inner.fail_writes.load(Ordering::Acquire) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let mut batch = pending.take();
    batch.sort_by_key(|message| message.change_seq);
    let bytes: usize = batch.iter().map(|item| item.bytes).sum();
    let rotation = match write_batch(connection, &batch) {
        Ok(rotation) => rotation,
        Err(error) => {
            for message in batch {
                pending.messages.insert(message.event.id.clone(), message);
            }
            pending.bytes = bytes;
            pending.first_at.get_or_insert_with(std::time::Instant::now);
            return Err(error);
        }
    };
    refresh_cached_storage(inner, connection, !rotation.deleted_event_ids.is_empty())?;
    inner
        .pending_events
        .fetch_sub(batch.len(), Ordering::AcqRel);
    inner.pending_bytes.fetch_sub(bytes, Ordering::AcqRel);
    reconcile_rotation_state(inner, connection, &rotation)?;
    let mut state = inner.state.lock().unwrap();
    state.last_commit_at = Some(now());
    state.last_error = None;
    inner.backpressure.store(false, Ordering::Release);
    inner.hard_backpressure.store(false, Ordering::Release);
    Ok(())
}

pub(super) fn validate_retention_update(update: &RuntimeRetentionUpdate) -> Result<(), String> {
    if update.expected_revision < 1 {
        return Err("expectedRevision 必须大于 0".into());
    }
    if update.max_age_days.is_some_and(|value| value < 1) {
        return Err("maxAgeDays 必须为空或至少为 1 天".into());
    }
    if update
        .storage_limit_bytes
        .is_some_and(|value| !(1_048_576..=i64::MAX).contains(&value))
    {
        return Err("storageLimitBytes 必须为空或至少为 1 MiB".into());
    }
    Ok(())
}

pub(super) fn load_retention_mutation(
    connection: &Connection,
) -> Result<RuntimeRetentionMutation, String> {
    connection
        .query_row(
            "SELECT revision
             FROM runtime_retention WHERE id=1",
            [],
            |row| {
                Ok(RuntimeRetentionMutation {
                    revision: row.get(0)?,
                    max_age_days: meta_i64(connection, "retention_max_age_days")?,
                    storage_limit_bytes: meta_i64(connection, "storage_limit_bytes")?,
                })
            },
        )
        .map_err(|error| error.to_string())
}

pub(super) fn set_retention_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    update: RuntimeRetentionUpdate,
) -> Result<RuntimeRetentionMutation, String> {
    validate_retention_update(&update)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_retention WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    if revision != update.expected_revision {
        return Err(format!(
            "retention_revision_conflict: expected {}, current {revision}",
            update.expected_revision
        ));
    }
    let next_revision = revision.saturating_add(1);
    transaction
        .execute(
            "UPDATE runtime_retention SET revision=?1,updated_at=?2 WHERE id=1",
            params![next_revision, now()],
        )
        .map_err(|error| error.to_string())?;
    set_retention_max_age_meta(&transaction, update.max_age_days)
        .map_err(|error| error.to_string())?;
    set_storage_limit_meta(&transaction, update.storage_limit_bytes)
        .map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    let rotation = rotate_retention_now(connection).map_err(|error| error.to_string())?;
    refresh_cached_storage(inner, connection, true).map_err(|error| error.to_string())?;
    reconcile_rotation_state(inner, connection, &rotation).map_err(|error| error.to_string())?;
    load_retention_mutation(connection)
}

pub(super) fn validate_pricing_update(update: &RuntimePricingUpdate) -> Result<(), String> {
    if update.expected_revision < 1 {
        return Err("expectedRevision 必须大于 0".into());
    }
    let currency = update.currency.trim();
    if currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase()) {
        return Err("currency 必须是三个大写 ASCII 字母".into());
    }
    if update.prices.len() > 2_000 {
        return Err("价格规则不能超过 2000 条".into());
    }
    let mut unique = HashSet::new();
    for price in &update.prices {
        if price
            .endpoint_id
            .as_deref()
            .is_some_and(|value| value.contains('\u{1f}'))
        {
            return Err("endpointID 不能包含保留分隔符".into());
        }
        let model = price.model_key.trim();
        if model.is_empty() || model.len() > 512 {
            return Err("modelKey 不能为空且不能超过 512 字节".into());
        }
        if model.contains('\u{1f}') {
            return Err("modelKey 不能包含保留分隔符".into());
        }
        if !price.effective_from.is_finite()
            || price.effective_to.is_some_and(|value| !value.is_finite())
            || price
                .effective_to
                .is_some_and(|value| value <= price.effective_from)
        {
            return Err("价格生效时间无效".into());
        }
        for rate in [
            price.input_per_million_micros,
            price.output_per_million_micros,
            price.cache_read_per_million_micros,
            price.cache_creation_per_million_micros,
        ] {
            if rate.is_some_and(|value| value < 0) {
                return Err("价格不能为负数".into());
            }
        }
        let storage_key = price
            .endpoint_id
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|endpoint| format!("{endpoint}\u{1f}{model}"))
            .unwrap_or_else(|| model.to_owned());
        if !unique.insert((storage_key, price.effective_from.to_bits())) {
            return Err("同一 modelKey 与 effectiveFrom 不能重复".into());
        }
    }
    Ok(())
}

pub(super) fn replace_pricing_database(
    connection: &mut Connection,
    update: RuntimePricingUpdate,
) -> Result<RuntimePricingMutation, String> {
    validate_pricing_update(&update)?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_pricing_meta WHERE id=1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|error| error.to_string())?;
    if revision != update.expected_revision {
        return Err(format!(
            "pricing_revision_conflict: expected {}, current {revision}",
            update.expected_revision
        ));
    }
    let next_revision = revision.saturating_add(1);
    transaction
        .execute("DELETE FROM runtime_model_prices", [])
        .map_err(|error| error.to_string())?;
    let updated_at = now();
    for price in &update.prices {
        transaction
            .execute(
                "INSERT INTO runtime_model_prices(
                    model_key,effective_from,effective_to,input_per_million_micros,
                    output_per_million_micros,cache_read_per_million_micros,
                    cache_creation_per_million_micros,created_at,updated_at
                 ) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?8)",
                params![
                    price
                        .endpoint_id
                        .as_deref()
                        .filter(|value| !value.trim().is_empty())
                        .map(|endpoint| format!("{endpoint}\u{1f}{}", price.model_key.trim()))
                        .unwrap_or_else(|| price.model_key.trim().to_owned()),
                    price.effective_from,
                    price.effective_to,
                    price.input_per_million_micros,
                    price.output_per_million_micros,
                    price.cache_read_per_million_micros,
                    price.cache_creation_per_million_micros,
                    updated_at,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    let currency = update.currency.trim().to_owned();
    transaction
        .execute(
            "UPDATE runtime_pricing_meta SET revision=?1,currency=?2,updated_at=?3 WHERE id=1",
            params![next_revision, currency, updated_at],
        )
        .map_err(|error| error.to_string())?;
    // Pricing is part of the rollup contract.  Never serve a bucket whose
    // cost was computed with the previous revision: clear the materialized
    // rows, mark every retained terminal-event bucket dirty, and let the
    // normal worker rebuild it before a rollup-backed trend is considered
    // complete.  The original event projections remain untouched.
    transaction
        .execute("DELETE FROM runtime_hourly_rollups", [])
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
             SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
             FROM runtime_events WHERE is_in_flight=0 AND kind IN ('client','upstream')",
            [],
        )
        .map_err(|error| error.to_string())?;
    let dirty = transaction
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
            [],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| error.to_string())?;
    set_meta(
        &transaction,
        "hourly_rollup_complete",
        if dirty { 0 } else { 1 },
    )
    .map_err(|error| error.to_string())?;
    set_meta(&transaction, "hourly_rollup_max_seq", 0).map_err(|error| error.to_string())?;
    set_meta(&transaction, "hourly_rollup_failed", 0).map_err(|error| error.to_string())?;
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(RuntimePricingMutation {
        revision: next_revision,
        currency,
        price_count: update.prices.len(),
    })
}

pub(super) fn reset_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
) -> Result<i64, rusqlite::Error> {
    let transaction = connection.transaction()?;
    transaction.execute("DELETE FROM runtime_events", [])?;
    transaction.execute("DELETE FROM runtime_hourly_rollups", [])?;
    transaction.execute("DELETE FROM runtime_hourly_rollup_dirty", [])?;
    transaction.execute("UPDATE runtime_counters SET client_requests=0,client_successes=0,client_failures=0,upstream_attempts=0,upstream_successes=0,upstream_failures=0,failovers=0 WHERE id=1", [])?;
    let generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "reset_generation", generation)?;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let next_seq = meta_i64(&transaction, "next_seq")?.unwrap_or(1);
    set_meta(&transaction, "retained_from_seq", next_seq)?;
    set_meta(&transaction, "retained_event_count", 0)?;
    set_meta(&transaction, "projection_backfill_cursor", 0)?;
    set_meta(&transaction, "projection_backfill_complete", 1)?;
    set_meta(&transaction, "projection_backfill_failed", 0)?;
    set_meta(&transaction, "hourly_rollup_complete", 1)?;
    set_meta(&transaction, "hourly_rollup_max_seq", 0)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    set_meta(&transaction, "hourly_rollup_failed", 0)?;
    for key in ["user_deleted_events", "user_deleted_requests"] {
        set_meta(&transaction, key, 0)?;
    }
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    inner.state.lock().unwrap().last_commit_at = Some(now());
    Ok(generation)
}

pub(super) fn validate_cleanup_cutoff(older_than: f64) -> Result<(), String> {
    if !older_than.is_finite() || older_than <= 0.0 {
        return Err("olderThan 必须是大于 0 的有限时间戳".into());
    }
    Ok(())
}

/// Select only complete request groups whose newest event is older than the
/// cutoff. Requests with any in-flight row remain wholly intact; rows without
/// a request ID are treated as independent events.
///
/// 一行是 `(event_id, request_id, kind, timestamp)`。
type ExpiredEventRow = (String, Option<String>, String, f64);

pub(super) fn cleanup_before_ids(
    connection: &Connection,
    older_than: f64,
) -> rusqlite::Result<Vec<ExpiredEventRow>> {
    let mut statement = connection.prepare(
        "SELECT event_id,request_id,kind,timestamp
         FROM runtime_events AS candidate
         WHERE candidate.is_in_flight=0
           AND (
               candidate.request_id IS NULL
               OR trim(candidate.request_id)=''
               OR (
                   NOT EXISTS(
                       SELECT 1 FROM runtime_events AS newer
                       WHERE newer.request_id=candidate.request_id
                         AND newer.timestamp >= ?1
                   )
                   AND NOT EXISTS(
                       SELECT 1 FROM runtime_events AS active
                       WHERE active.request_id=candidate.request_id
                         AND active.is_in_flight=1
                   )
               )
           )
           AND candidate.timestamp < ?1
         ORDER BY candidate.timestamp ASC,candidate.seq ASC",
    )?;
    statement
        .query_map(params![older_than], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f64>(3)?,
            ))
        })?
        .collect()
}

pub(super) fn cleanup_before_preview_database(
    connection: &Connection,
    older_than: f64,
) -> rusqlite::Result<RuntimeCleanupPreview> {
    let deletable = cleanup_before_ids(connection, older_than)?;
    let deletable_events = deletable.len() as i64;
    let deletable_requests = deletable
        .iter()
        .filter(|(_, _, kind, _)| kind == KIND_CLIENT)
        .map(|(event_id, request_id, _, _)| {
            request_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(event_id)
                .to_owned()
        })
        .collect::<HashSet<_>>()
        .len() as i64;
    let retained = connection.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
        row.get::<_, i64>(0)
    })?;
    Ok(RuntimeCleanupPreview {
        older_than,
        deletable_events,
        deletable_requests,
        remaining_events: retained.saturating_sub(deletable_events),
    })
}

pub(super) fn cleanup_before_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    older_than: f64,
) -> rusqlite::Result<RuntimeCleanupMutation> {
    let transaction = connection.transaction()?;
    let deletable = cleanup_before_ids(&transaction, older_than)?;
    if deletable.is_empty() {
        let remaining_events =
            transaction.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
                row.get::<_, i64>(0)
            })?;
        let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0);
        transaction.commit()?;
        return Ok(RuntimeCleanupMutation {
            older_than,
            deleted_events: 0,
            deleted_requests: 0,
            remaining_events,
            history_generation,
        });
    }
    let mut deleted_ids = Vec::with_capacity(deletable.len());
    for (event_id, request_id, kind, timestamp) in &deletable {
        if let Some(request_id) = request_id.as_deref().filter(|id| !id.trim().is_empty()) {
            mark_request_hourly_rollups_dirty(&transaction, request_id)?;
        } else if kind == KIND_CLIENT {
            mark_hourly_rollup_bucket(&transaction, hourly_bucket_start(*timestamp))?;
        }
        transaction.execute(
            "DELETE FROM runtime_events WHERE event_id=?1 AND is_in_flight=0",
            params![event_id],
        )?;
        deleted_ids.push(event_id.clone());
    }
    let counters = counters_from_connection(&transaction)?;
    transaction.execute(
        "UPDATE runtime_counters SET client_requests=?1,client_successes=?2,client_failures=?3,
         upstream_attempts=?4,upstream_successes=?5,upstream_failures=?6,failovers=?7 WHERE id=1",
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
    let deleted_events = deleted_ids.len() as i64;
    let deleted_requests = deletable
        .iter()
        .filter(|(_, _, kind, _)| kind == KIND_CLIENT)
        .map(|(event_id, request_id, _, _)| {
            request_id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(event_id)
                .to_owned()
        })
        .collect::<HashSet<_>>()
        .len() as i64;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let retained_events =
        transaction.query_row("SELECT COUNT(*) FROM runtime_events", [], |row| {
            row.get::<_, i64>(0)
        })?;
    set_meta(&transaction, "retained_event_count", retained_events)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    let user_deleted_events = meta_i64(&transaction, "user_deleted_events")?
        .unwrap_or(0)
        .saturating_add(deleted_events);
    let user_deleted_requests = meta_i64(&transaction, "user_deleted_requests")?
        .unwrap_or(0)
        .saturating_add(deleted_requests);
    set_meta(&transaction, "user_deleted_events", user_deleted_events)?;
    set_meta(&transaction, "user_deleted_requests", user_deleted_requests)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    Ok(RuntimeCleanupMutation {
        older_than,
        deleted_events,
        deleted_requests,
        remaining_events: retained_events,
        history_generation,
    })
}

/// Drop and recreate every runtime-owned table in place. Keeping the same
/// path avoids invalidating readers while still guaranteeing that removed
/// legacy columns cannot survive the explicit user cutover.
pub(super) fn recreate_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
) -> Result<i64, rusqlite::Error> {
    let generation = rebuild_schema(connection)?;
    refresh_cached_storage(inner, connection, true)?;
    inner.state.lock().unwrap().last_commit_at = Some(now());
    Ok(generation)
}

pub(super) fn rebuild_schema(connection: &mut Connection) -> rusqlite::Result<i64> {
    let previous_generation = meta_i64(connection, "reset_generation")
        .ok()
        .flatten()
        .unwrap_or(0);
    let previous_history_generation = meta_i64(connection, "history_generation")
        .ok()
        .flatten()
        .unwrap_or(0);
    connection.execute_batch(
        "DROP TABLE IF EXISTS runtime_events;
         DROP TABLE IF EXISTS runtime_hourly_rollup_dirty;
         DROP TABLE IF EXISTS runtime_hourly_rollups;
         DROP TABLE IF EXISTS runtime_model_prices;
         DROP TABLE IF EXISTS runtime_pricing_meta;
         DROP TABLE IF EXISTS runtime_retention;
         DROP TABLE IF EXISTS runtime_counters;
         DROP TABLE IF EXISTS runtime_meta;",
    )?;
    setup_connection(connection)?;
    let generation = previous_generation.saturating_add(1);
    let history_generation = previous_history_generation.saturating_add(1);
    set_meta(connection, "reset_generation", generation)?;
    set_meta(connection, "history_generation", history_generation)?;
    set_meta(
        connection,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    // Reclaim pages from the removed tables so this is a real fresh database,
    // not merely a schema reset with old payload pages left on the freelist.
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); VACUUM;")?;
    Ok(generation)
}
