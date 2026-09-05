use super::{
    AnalyticsAggregate, Arc, BTreeSet, Connection, Duration, HashSet, Inner, KIND_CLIENT,
    KIND_NOTIFY, KIND_UPSTREAM, OpenFlags, Path, PathBuf, RuntimeChange, RuntimeCounters,
    RuntimeEvent, RuntimeSnapshot, SessionMutation, Value, build_project_keys,
    build_request_endpoints, client_key, dimension_value, json, mark_request_hourly_rollups_dirty,
    meta_i64, now, params, project_key, refresh_cached_storage, set_meta,
};

pub(super) fn decode_runtime_event(payload: &str) -> rusqlite::Result<RuntimeEvent> {
    serde_json::from_str(payload).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
    })
}

pub(super) fn counters_from_connection(
    connection: &Connection,
) -> rusqlite::Result<RuntimeCounters> {
    connection.query_row(
        "SELECT
            SUM(CASE WHEN kind=?1 THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND outcome='succeeded' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND outcome='failed' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 AND outcome='succeeded' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?2 AND outcome='failed' THEN 1 ELSE 0 END),
            SUM(CASE WHEN kind=?1 AND COALESCE(failover,0)<>0 THEN 1 ELSE 0 END)
         FROM runtime_events WHERE is_in_flight=0",
        params![KIND_CLIENT, KIND_UPSTREAM],
        |row| {
            Ok(RuntimeCounters {
                client_requests: row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                client_successes: row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                client_failures: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                upstream_attempts: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                upstream_successes: row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                upstream_failures: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
                failovers: row.get::<_, Option<i64>>(6)?.unwrap_or(0),
            })
        },
    )
}

pub(super) fn delete_session_database(
    inner: &Arc<Inner>,
    connection: &mut Connection,
    session_id: &str,
) -> rusqlite::Result<SessionMutation> {
    let transaction = connection.transaction()?;
    let (client_event_ids, request_ids) = {
        let mut statement = transaction.prepare(
            "SELECT event_id,request_id FROM runtime_events
             WHERE kind=?1 AND session_key=?2",
        )?;
        let rows = statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        (
            rows.iter().map(|row| row.0.clone()).collect::<HashSet<_>>(),
            rows.into_iter()
                .filter_map(|row| row.1)
                .collect::<HashSet<_>>(),
        )
    };
    if client_event_ids.is_empty() {
        return Err(rusqlite::Error::QueryReturnedNoRows);
    }
    let mut delete_ids = client_event_ids.clone();
    for request_id in &request_ids {
        mark_request_hourly_rollups_dirty(&transaction, request_id)?;
        let mut statement =
            transaction.prepare("SELECT event_id FROM runtime_events WHERE request_id=?1")?;
        delete_ids.extend(
            statement
                .query_map(params![request_id], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    for event_id in &delete_ids {
        transaction.execute(
            "DELETE FROM runtime_events WHERE event_id=?1",
            params![event_id],
        )?;
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
    let generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "reset_generation", generation)?;
    let history_generation = meta_i64(&transaction, "history_generation")?.unwrap_or(0) + 1;
    set_meta(&transaction, "history_generation", history_generation)?;
    let remaining = meta_i64(&transaction, "retained_event_count")?
        .unwrap_or(0)
        .saturating_sub(delete_ids.len() as i64);
    set_meta(&transaction, "retained_event_count", remaining)?;
    let retained_from_seq = transaction.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'),1)) FROM runtime_events",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    set_meta(&transaction, "retained_from_seq", retained_from_seq)?;
    let user_deleted_events = meta_i64(&transaction, "user_deleted_events")?
        .unwrap_or(0)
        .saturating_add(delete_ids.len() as i64);
    let user_deleted_requests = meta_i64(&transaction, "user_deleted_requests")?
        .unwrap_or(0)
        .saturating_add(client_event_ids.len() as i64);
    set_meta(&transaction, "user_deleted_events", user_deleted_events)?;
    set_meta(&transaction, "user_deleted_requests", user_deleted_requests)?;
    set_meta(
        &transaction,
        "hourly_rollup_history_generation",
        history_generation,
    )?;
    transaction.commit()?;
    refresh_cached_storage(inner, connection, true)?;
    Ok(SessionMutation {
        reset_generation: generation,
        deleted_events: delete_ids.len() as i64,
        deleted_requests: client_event_ids.len() as i64,
    })
}

pub(super) fn export_session_json(
    connection: &Connection,
    session_id: &str,
) -> Result<Value, String> {
    let request_ids = {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT request_id FROM runtime_events
                 WHERE kind=?1 AND session_key=?2 AND request_id IS NOT NULL",
            )
            .map_err(|error| error.to_string())?;
        statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                row.get::<_, String>(0)
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|error| error.to_string())?
    };
    let mut selected = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT seq,change_seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND session_key=?2 ORDER BY seq ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![KIND_CLIENT, session_id], |row| {
                Ok(RuntimeChange {
                    seq: row.get(0)?,
                    change_seq: row.get(1)?,
                    event: decode_runtime_event(&row.get::<_, String>(2)?)?,
                })
            })
            .map_err(|error| error.to_string())?;
        selected.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
    }
    for request_id in &request_ids {
        let mut statement = connection
            .prepare(
                "SELECT seq,change_seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND request_id=?2 ORDER BY seq ASC",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![KIND_UPSTREAM, request_id], |row| {
                Ok(RuntimeChange {
                    seq: row.get(0)?,
                    change_seq: row.get(1)?,
                    event: decode_runtime_event(&row.get::<_, String>(2)?)?,
                })
            })
            .map_err(|error| error.to_string())?;
        selected.extend(
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?,
        );
    }
    selected.sort_by_key(|change| change.seq);
    if selected.is_empty() {
        return Err("会话不存在".into());
    }
    let selected_events = selected
        .iter()
        .map(|change| change.event.clone())
        .collect::<Vec<_>>();
    let project_keys = build_project_keys(&selected_events);
    let request_endpoints = build_request_endpoints(
        &selected
            .iter()
            .map(|change| change.event.clone())
            .collect::<Vec<_>>(),
    );
    let mut aggregate =
        AnalyticsAggregate::with_request_metadata(request_ids.clone(), request_endpoints);
    for change in &selected {
        aggregate.add(&change.event, &project_keys);
    }
    let projects = selected
        .iter()
        .filter(|change| change.event.kind == KIND_CLIENT)
        .map(|change| project_key(&change.event, &project_keys))
        .collect::<BTreeSet<_>>();
    let client_kinds = selected
        .iter()
        .filter(|change| change.event.kind == KIND_CLIENT)
        .map(|change| client_key(&change.event))
        .collect::<BTreeSet<_>>();
    let events = selected
        .iter()
        .map(|change| json!({"seq": change.seq, "changeSeq": change.change_seq, "event": change.event}))
        .collect::<Vec<_>>();
    Ok(json!({
        "format": "sumpter-session-export-v1",
        "exportedAt": now(),
        "sessionID": session_id,
        "projects": projects,
        "clientKinds": client_kinds,
        "eventCount": events.len(),
        "analytics": {
            "clientRequests": aggregate.client_requests,
            "clientSuccesses": aggregate.client_successes,
            "clientFailures": aggregate.client_failures,
            "clientCancelled": aggregate.client_cancelled,
            "clientPending": aggregate.client_requests.saturating_sub(aggregate.client_successes + aggregate.client_failures + aggregate.client_cancelled),
            "upstreamAttempts": aggregate.upstream_attempts,
            "upstreamSuccesses": aggregate.upstream_successes,
            "upstreamFailures": aggregate.upstream_failures,
            "failovers": aggregate.failovers,
            "tokenUsage": aggregate.token_usage.value(),
            "endpoints": dimension_value(&aggregate.endpoints),
        },
        "events": events,
    }))
}

pub(super) fn read_connection(path: &Path) -> Result<Connection, String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON; PRAGMA cache_size = -2048;")
        .map_err(|e| e.to_string())?;
    Ok(connection)
}

pub(super) fn load_snapshot(connection: &Connection) -> Result<RuntimeSnapshot, String> {
    let mut snapshot = RuntimeSnapshot {
        client_requests: 0,
        client_successes: 0,
        client_failures: 0,
        failovers: 0,
        recent_events: Vec::new(),
        upstream_attempts: 0,
        upstream_successes: 0,
        upstream_failures: 0,
    };
    let counters = connection.query_row("SELECT client_requests,client_successes,client_failures,upstream_attempts,upstream_successes,upstream_failures,failovers FROM runtime_counters WHERE id=1", [], |row| Ok(RuntimeCounters { client_requests: row.get(0)?, client_successes: row.get(1)?, client_failures: row.get(2)?, upstream_attempts: row.get(3)?, upstream_successes: row.get(4)?, upstream_failures: row.get(5)?, failovers: row.get(6)? })).map_err(|e| e.to_string())?;
    snapshot.client_requests = counters.client_requests;
    snapshot.client_successes = counters.client_successes;
    snapshot.client_failures = counters.client_failures;
    snapshot.upstream_attempts = counters.upstream_attempts;
    snapshot.upstream_successes = counters.upstream_successes;
    snapshot.upstream_failures = counters.upstream_failures;
    snapshot.failovers = counters.failovers;
    let mut recent = Vec::new();
    for kind in [KIND_CLIENT, KIND_UPSTREAM, KIND_NOTIFY] {
        let mut statement = connection
            .prepare(
                "SELECT seq,payload_json FROM runtime_events
                 WHERE kind=?1 AND is_in_flight=0 ORDER BY seq DESC LIMIT 200",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![kind], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            recent.push(row.map_err(|error| error.to_string())?);
        }
    }
    recent.sort_by_key(|(seq, _)| *seq);
    for (_, payload) in recent {
        let event: RuntimeEvent =
            serde_json::from_str(&payload).map_err(|error| error.to_string())?;
        snapshot.upsert_event(event);
    }
    Ok(snapshot)
}

pub(super) fn database_file_sizes(path: &Path) -> (u64, u64) {
    let db_bytes = std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let wal = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal).map(|meta| meta.len()).unwrap_or(0);
    (db_bytes, wal_bytes)
}
