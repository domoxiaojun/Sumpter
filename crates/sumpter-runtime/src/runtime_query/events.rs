//! Runtime query events domain.

use rusqlite::params_from_iter;

use super::{
    API_VERSION, Connection, EventListProjection, EventPage, EventPageQuery, EventPageRequest,
    HashMap, Path, QueryResult, REQUEST_CHAIN_LIMIT, RequestChain, RuntimeChange,
    RuntimeEventListItem, RuntimeQueryError, SqlFilter, SqlValue, TransactionBehavior,
    append_runtime_filter, event_list_item_from_projection, history_snapshot, read_connection,
    require_projection, validate_page,
};

pub fn events_page(path: &Path, request: &EventPageQuery) -> QueryResult<EventPage> {
    let mut connection = read_connection(path)?;
    events_page_on(&mut connection, request)
}

pub fn events_page_on(
    connection: &mut Connection,
    request: &EventPageRequest,
) -> QueryResult<EventPage> {
    validate_page(request.page, request.page_size)?;
    let filters = request.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    // Event pages always select normalized projection columns.  Requiring the
    // projection even for an unfiltered page prevents partially backfilled
    // rows from surfacing as empty values and keeps pagination/filter totals
    // consistent while the background backfill is running.
    require_projection(&transaction)?;
    let snapshot = history_snapshot(
        &transaction,
        request.snapshot_seq,
        request.history_generation,
    )?;
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.le_i64("seq", snapshot.snapshot_seq);
    append_runtime_filter(&mut builder, &filters);
    let where_sql = builder.where_sql();
    let total_count = transaction.query_row(
        &format!("SELECT COUNT(*) FROM runtime_events{where_sql}"),
        params_from_iter(builder.values.iter()),
        |row| row.get::<_, i64>(0),
    )?;
    let total_pages = if total_count == 0 {
        0
    } else {
        ((total_count as usize).saturating_add(request.page_size - 1)) / request.page_size
    };
    let offset = request
        .page
        .saturating_sub(1)
        .checked_mul(request.page_size)
        .ok_or_else(|| RuntimeQueryError::InvalidInput("page offset is too large".into()))?;
    if request.snapshot_seq.is_some()
        && request.page > 1
        && total_count > 0
        && offset >= total_count as usize
        && snapshot.retained_from_seq > 1
    {
        return Err(RuntimeQueryError::SnapshotTrimmed {
            snapshot_seq: snapshot.snapshot_seq,
            retained_from_seq: snapshot.retained_from_seq,
        });
    }

    let mut values = builder.values;
    values.push(SqlValue::Integer(request.page_size as i64));
    values.push(SqlValue::Integer(offset.min(i64::MAX as usize) as i64));
    let sql = format!(
        "SELECT seq,change_seq,event_id,timestamp,kind,phase,outcome,status_code,\
                request_id,session_key,session_source,client_kind,request_purpose,\
                endpoint_id,endpoint_name,feature_rule_id,client_model,\
                effective_model,upstream_model,failure_kind,failure_phase,\
                source_format,target_format,route_mode,upstream_status_code,\
                duration_ms,ttfb_ms,failover,project_name,project_source,local_user,\
                codex_thread_class,attribution_scope \
                ,request_method,request_path,route_intent \
         FROM runtime_events{where_sql} ORDER BY seq DESC LIMIT ? OFFSET ?"
    );
    let events = {
        let mut statement = transaction.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(EventListProjection {
                seq: row.get(0)?,
                change_seq: row.get(1)?,
                id: row.get(2)?,
                timestamp: row.get(3)?,
                kind: row.get(4)?,
                phase: row.get(5)?,
                outcome: row.get(6)?,
                status_code: row.get(7)?,
                request_id: row.get(8)?,
                session_key: row.get(9)?,
                session_source: row.get(10)?,
                client_kind: row.get(11)?,
                request_purpose: row.get(12)?,
                endpoint_id: row.get(13)?,
                endpoint_name: row.get(14)?,
                feature_rule_id: row.get(15)?,
                client_model: row.get(16)?,
                effective_model: row.get(17)?,
                upstream_model: row.get(18)?,
                failure_kind: row.get(19)?,
                failure_phase: row.get(20)?,
                source_format: row.get(21)?,
                target_format: row.get(22)?,
                route_mode: row.get(23)?,
                upstream_status_code: row.get(24)?,
                duration_ms: row.get::<_, Option<i64>>(25)?.unwrap_or_default(),
                ttfb_ms: row.get(26)?,
                failover: row.get::<_, Option<i64>>(27)?.unwrap_or_default(),
                project_name: row.get(28)?,
                project_source: row.get(29)?,
                local_user: row.get(30)?,
                codex_thread_class: row.get(31)?,
                attribution_scope: row.get(32)?,
                request_method: row.get(33)?,
                request_path: row.get(34)?,
                route_intent: row.get(35)?,
            })
        })?;
        rows.map(|row| row.map(event_list_item_from_projection))
            .collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(EventPage {
        api_version: API_VERSION,
        next_cursor: events.last().map(|event| event.seq),
        previous_cursor: events.first().map(|event| event.seq),
        has_next: request.page < total_pages,
        has_previous: request.page > 1 && total_pages > 0,
        events,
        page: request.page,
        page_size: request.page_size,
        total_count,
        total_pages,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        reset_generation: snapshot.reset_generation,
        retained_from_seq: snapshot.retained_from_seq,
        filters,
    })
}

pub fn request_chain(path: &Path, request_id: &str) -> QueryResult<RequestChain> {
    let connection = read_connection(path)?;
    request_chain_on(&connection, request_id)
}

pub fn request_chain_with_recent(
    path: &Path,
    request_id: &str,
    recent_changes: &[RuntimeChange],
) -> QueryResult<RequestChain> {
    let connection = read_connection(path)?;
    request_chain_with_recent_on(&connection, request_id, recent_changes)
}

pub fn request_chain_on(connection: &Connection, request_id: &str) -> QueryResult<RequestChain> {
    request_chain_with_recent_on(connection, request_id, &[])
}

pub fn request_chain_with_recent_on(
    connection: &Connection,
    request_id: &str,
    recent_changes: &[RuntimeChange],
) -> QueryResult<RequestChain> {
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.len() > 512 {
        return Err(RuntimeQueryError::InvalidInput(
            "requestID must contain 1 to 512 bytes".into(),
        ));
    }
    let mut statement = connection.prepare(
        "SELECT seq,change_seq,event_id,payload_json FROM runtime_events \
         WHERE request_id=?1 ORDER BY seq ASC,change_seq ASC LIMIT ?2",
    )?;
    let mut rows = statement.query((request_id, (REQUEST_CHAIN_LIMIT + 1) as i64))?;
    let mut persisted = Vec::new();
    while let Some(row) = rows.next()? {
        let seq = row.get::<_, i64>(0)?;
        let change_seq = row.get::<_, i64>(1)?;
        let event_id = row.get::<_, String>(2)?;
        let payload = row.get::<_, String>(3)?;
        let event =
            serde_json::from_str(&payload).map_err(|error| RuntimeQueryError::CorruptPayload {
                event_id,
                detail: error.to_string(),
            })?;
        persisted.push(RuntimeEventListItem::from_change(seq, change_seq, event));
    }
    let mut truncated = persisted.len() > REQUEST_CHAIN_LIMIT;
    persisted.truncate(REQUEST_CHAIN_LIMIT);
    let mut merged = persisted
        .into_iter()
        .map(|event| (event.id.clone(), event))
        .collect::<HashMap<_, _>>();
    for change in recent_changes
        .iter()
        .filter(|change| change.event.request_id.as_deref() == Some(request_id))
    {
        let item =
            RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event.clone());
        if merged
            .get(&item.id)
            .is_none_or(|current| item.change_seq > current.change_seq)
        {
            merged.insert(item.id.clone(), item);
        }
    }
    let mut events = merged.into_values().collect::<Vec<_>>();
    events.sort_by_key(|event| (event.seq, event.change_seq));
    if events.len() > REQUEST_CHAIN_LIMIT {
        events.truncate(REQUEST_CHAIN_LIMIT);
        truncated = true;
    }
    if events.is_empty() {
        return Err(RuntimeQueryError::NotFound(format!(
            "runtime request {request_id} was not found"
        )));
    }
    Ok(RequestChain {
        api_version: API_VERSION,
        request_id: request_id.to_owned(),
        events,
        truncated,
    })
}
