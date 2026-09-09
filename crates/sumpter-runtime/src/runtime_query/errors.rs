//! Runtime query errors domain.

use crate::database::params_from_iter;

use super::{
    API_VERSION, Connection, ErrorGroup, ErrorPage, ErrorPageQuery, ErrorQuery, PROJECTION_VERSION,
    Path, QueryResult, RuntimeQueryError, SqlFilter, SqlValue, TransactionBehavior,
    append_runtime_filter, history_snapshot, read_connection, require_projection, validate_page,
};

pub fn error_groups(path: &Path, request: &ErrorQuery) -> QueryResult<ErrorPage> {
    let mut connection = read_connection(path)?;
    error_groups_on(&mut connection, request)
}

pub fn error_groups_on(
    connection: &mut Connection,
    request: &ErrorPageQuery,
) -> QueryResult<ErrorPage> {
    validate_page(request.page, request.page_size)?;
    let filters = request.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(
        &transaction,
        request.snapshot_seq,
        request.history_generation,
    )?;
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    builder.raw("outcome = 'failed'");
    builder.le_i64("seq", snapshot.snapshot_seq);
    append_runtime_filter(&mut builder, &filters);
    let where_sql = builder.where_sql();
    let group_columns = "failure_kind,failure_phase,endpoint_id,endpoint_name,\
                         effective_model,upstream_status_code";
    let total_count = transaction.query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT 1 FROM runtime_events{where_sql} \
             GROUP BY {group_columns})"
        ),
        params_from_iter(builder.values.iter()),
        |row| row.get::<_, i64>(0),
    )?;
    let total_pages = if total_count == 0 {
        0
    } else {
        (total_count as usize).saturating_add(request.page_size - 1) / request.page_size
    };
    let offset = request
        .page
        .saturating_sub(1)
        .checked_mul(request.page_size)
        .ok_or_else(|| RuntimeQueryError::InvalidInput("page offset is too large".into()))?;
    let mut values = vec![SqlValue::Integer(snapshot.snapshot_seq)];
    values.extend(builder.values);
    values.push(SqlValue::Integer(request.page_size as i64));
    values.push(SqlValue::Integer(offset.min(i64::MAX as usize) as i64));
    let sql = format!(
        "WITH recovered_requests AS (\
             SELECT DISTINCT request_id FROM runtime_events \
             WHERE request_id IS NOT NULL AND kind='client' AND outcome='succeeded' \
               AND failover=1 AND is_in_flight=0 AND seq<=?\
         )\
         SELECT failure_kind,failure_phase,endpoint_id,endpoint_name,effective_model,\
                upstream_status_code,COUNT(*) AS occurrences,\
                COUNT(DISTINCT request_id) AS affected_requests,\
                COUNT(DISTINCT session_key) AS affected_sessions,\
                COUNT(DISTINCT CASE WHEN request_id IN \
                    (SELECT request_id FROM recovered_requests) THEN request_id END) \
                    AS recovered_after_failover,\
                MIN(timestamp),MAX(timestamp),MIN(event_id) \
         FROM runtime_events{where_sql} GROUP BY {group_columns} \
         ORDER BY occurrences DESC,MAX(timestamp) DESC,COALESCE(failure_kind,'') ASC \
         LIMIT ? OFFSET ?"
    );
    let groups = {
        let mut statement = transaction.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            let sample = row.get::<_, Option<String>>(12)?;
            Ok(ErrorGroup {
                failure_kind: row.get(0)?,
                failure_phase: row.get(1)?,
                endpoint_id: row.get(2)?,
                endpoint_name: row.get(3)?,
                model: row.get(4)?,
                upstream_status_code: row.get(5)?,
                occurrences: row.get(6)?,
                affected_requests: row.get(7)?,
                affected_sessions: row.get(8)?,
                recovered_after_failover: row.get(9)?,
                first_seen: row.get(10)?,
                last_seen: row.get(11)?,
                sample_event_ids: sample.into_iter().collect(),
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(ErrorPage {
        api_version: API_VERSION,
        groups,
        page: request.page,
        page_size: request.page_size,
        total_count,
        total_pages,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
        has_next: request.page < total_pages,
        has_previous: request.page > 1 && total_pages > 0,
        filters,
    })
}
