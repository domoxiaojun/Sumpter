//! Runtime query dimensions domain.

use crate::database::params_from_iter;

use super::csv_values;

use super::{
    API_VERSION, Connection, CostAccumulator, CostCoverage, DURATION_CRITICAL_MS, DURATION_SLOW_MS,
    DimensionKind, DimensionPage, DimensionPageQuery, DimensionQuery, DimensionRow, DimensionSort,
    HashMap, PROJECTION_VERSION, Path, PriceCatalog, QueryResult, RuntimeQueryError, SortOrder,
    SqlFilter, SqlValue, TTFB_CRITICAL_MS, TTFB_SLOW_MS, Transaction, TransactionBehavior,
    TrendRow, append_runtime_filter, history_snapshot, load_price_catalog, ratio, read_connection,
    require_projection, validate_page,
};

pub fn dimension_page(
    path: &Path,
    kind: DimensionKind,
    request: &DimensionQuery,
) -> QueryResult<DimensionPage> {
    let mut connection = read_connection(path)?;
    dimension_page_on(&mut connection, kind, request)
}

pub fn dimension_page_on(
    connection: &mut Connection,
    kind: DimensionKind,
    request: &DimensionPageQuery,
) -> QueryResult<DimensionPage> {
    validate_page(request.page, request.page_size)?;
    let filters = request.filter.normalized()?;
    let search = request
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    if search.as_ref().is_some_and(|value| value.len() > 256) {
        return Err(RuntimeQueryError::InvalidInput(
            "search must not exceed 256 bytes".into(),
        ));
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(
        &transaction,
        request.snapshot_seq,
        request.history_generation,
    )?;
    let (
        key_expression,
        name_expression,
        source_expression,
        related_expression,
        workspace_expr,
        extra_clause,
    ) = match kind {
        DimensionKind::Endpoint => (
            "COALESCE(endpoint_id,'unassigned_endpoint')",
            "COALESCE(endpoint_name,endpoint_id,'unassigned_endpoint')",
            "COALESCE(endpoint_id,'missing_endpoint_metadata')",
            "session_key",
            "NULL",
            "endpoint_id IS NOT NULL",
        ),
        DimensionKind::Model => (
            "COALESCE(effective_model,upstream_model,'unidentified_model')",
            "COALESCE(effective_model,upstream_model,'unidentified_model')",
            "COALESCE(upstream_model,effective_model,'missing_model_metadata')",
            "session_key",
            "NULL",
            "",
        ),
        DimensionKind::ClientKind => (
            "COALESCE(client_kind,'legacy_client_kind_missing')",
            "COALESCE(client_kind,'legacy_client_kind_missing')",
            "COALESCE(client_kind,'missing_client_kind_metadata')",
            "project_id",
            "NULL",
            "",
        ),
        DimensionKind::ClientVariant => (
            "COALESCE(client_variant,'unknown')",
            "COALESCE(client_variant,'unknown')",
            "COALESCE(client_variant,'unknown')",
            "project_id",
            "NULL",
            "",
        ),
        DimensionKind::AgentRole => (
            "COALESCE(agent_role,'unknown')",
            "COALESCE(agent_role,'unknown')",
            "COALESCE(agent_role,'unknown')",
            "parent_thread_id",
            "NULL",
            "",
        ),
        DimensionKind::AgentName => (
            "COALESCE(agent_name,'unknown')",
            "COALESCE(agent_name,'unknown')",
            "COALESCE(agent_name,'unknown')",
            "parent_thread_id",
            "NULL",
            "",
        ),
        DimensionKind::ParentThread => (
            "COALESCE(parent_thread_id,'unknown')",
            "COALESCE(parent_thread_id,'unknown')",
            "COALESCE(parent_thread_id,'unknown')",
            "agent_role",
            "NULL",
            "",
        ),
        DimensionKind::ParentTurn => (
            "COALESCE(parent_turn_id,'unknown')",
            "COALESCE(parent_turn_id,'unknown')",
            "COALESCE(parent_turn_id,'unknown')",
            "parent_thread_id",
            "NULL",
            "",
        ),
        DimensionKind::RootTurn => (
            "COALESCE(root_turn_id,'unknown')",
            "COALESCE(root_turn_id,'unknown')",
            "COALESCE(root_turn_id,'unknown')",
            "parent_thread_id",
            "NULL",
            "",
        ),
        DimensionKind::Purpose => (
            "COALESCE(request_purpose,'legacy_purpose_missing')",
            "COALESCE(request_purpose,'legacy_purpose_missing')",
            "COALESCE(request_purpose,'missing_purpose_metadata')",
            "session_key",
            "NULL",
            "",
        ),
        DimensionKind::FailureKind => (
            "COALESCE(failure_kind,'unclassified_failure')",
            "COALESCE(failure_kind,'unclassified_failure')",
            "COALESCE(failure_phase,'missing_failure_phase')",
            "session_key",
            "NULL",
            "outcome = 'failed'",
        ),
        DimensionKind::FailurePhase => (
            "COALESCE(failure_phase,'unclassified_failure_phase')",
            "COALESCE(failure_phase,'unclassified_failure_phase')",
            "COALESCE(failure_kind,'missing_failure_kind')",
            "session_key",
            "NULL",
            "outcome = 'failed'",
        ),
        DimensionKind::Protocol => (
            "COALESCE(target_format,source_format,'unknown_protocol')",
            "COALESCE(source_format,'unknown_source') || ' → ' || COALESCE(target_format,'unknown_target')",
            "COALESCE(route_mode,'unknown_route_mode')",
            "session_key",
            "NULL",
            "",
        ),
        DimensionKind::StreamTerminal => (
            "COALESCE(stream_terminal,'stream_terminal_missing')",
            "COALESCE(stream_terminal,'stream_terminal_missing')",
            "COALESCE(outcome,'unknown_outcome')",
            "session_key",
            "NULL",
            "",
        ),
        DimensionKind::Project => (
            "COALESCE(project_id,'unidentified_project')",
            "COALESCE(project_name,project_id,'unidentified_project')",
            "COALESCE(project_source,'missing_workspace_metadata')",
            "session_key",
            "MAX(workspace_paths_json)",
            "COALESCE(attribution_scope,'unknown') != 'internal_feature'",
        ),
        DimensionKind::Session => (
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_source,'missing_session_metadata')",
            "project_id",
            "NULL",
            "",
        ),
    };
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    builder.raw("kind = 'client'");
    if !extra_clause.is_empty() {
        builder.raw(extra_clause);
    }
    builder.le_i64("seq", snapshot.snapshot_seq);
    append_runtime_filter(&mut builder, &filters);
    if let Some(search) = search.as_ref() {
        builder.text_values(
            format!(
                "(instr(lower(COALESCE(({name_expression}),'')),lower(?))>0 OR \
                  instr(lower(COALESCE(({key_expression}),'')),lower(?))>0)"
            ),
            [search.clone(), search.clone()],
        );
    }
    let where_sql = builder.where_sql();
    let group_columns = format!("{key_expression},{name_expression},{source_expression}");
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
    let sort_column = match request.sort {
        DimensionSort::Name => "name COLLATE NOCASE",
        DimensionSort::Requests => "requests",
        // ORDER BY may refer to aggregate aliases in SQLite. Keep the
        // denominator explicit; NULL means that no terminal result exists and
        // is handled below so an unknown value stays after measured values in
        // either direction.
        DimensionSort::SuccessRate => {
            "successes * 1.0 / NULLIF(successes + failures + cancelled, 0)"
        }
        DimensionSort::Failures => "failures",
        DimensionSort::InputTokens => "input_tokens",
        DimensionSort::OutputTokens => "output_tokens",
        DimensionSort::CacheReadTokens => "cache_read_input_tokens",
        DimensionSort::CacheWriteTokens => "cache_creation_input_tokens",
        DimensionSort::Tokens => "processed_total_tokens",
        DimensionSort::AverageDuration => "average_duration_ms",
        DimensionSort::LastSeen => "last_seen",
    };
    let order = match request.order {
        SortOrder::Asc => "ASC",
        SortOrder::Desc => "DESC",
    };
    let nulls_last = matches!(
        request.sort,
        DimensionSort::SuccessRate | DimensionSort::AverageDuration
    );
    let order_by = if nulls_last {
        format!("{sort_column} IS NULL ASC, {sort_column} {order}")
    } else {
        format!("{sort_column} {order}")
    };
    let prices = load_price_catalog(&transaction)?;
    let dimension_cost_map = dimension_costs(&transaction, &builder, key_expression, &prices)?;
    let mut values = builder.values;
    values.push(SqlValue::Integer(request.page_size as i64));
    values.push(SqlValue::Integer(offset.min(i64::MAX as usize) as i64));
    let sql = format!(
        r#"SELECT {key_expression} AS key,
                      {name_expression} AS name,
                      {source_expression} AS source,
                      COUNT(*) AS requests,
                      SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END) AS successes,
                      SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END) AS failures,
                      SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END) AS cancelled,
                      SUM(CASE WHEN failover=1 THEN 1 ELSE 0 END) AS failovers,
                      SUM(CASE WHEN duration_ms > {DURATION_SLOW_MS} THEN 1 ELSE 0 END) AS slow_duration_requests,
                      SUM(CASE WHEN duration_ms > {DURATION_CRITICAL_MS} THEN 1 ELSE 0 END) AS critical_duration_requests,
                      SUM(CASE WHEN ttfb_ms > {TTFB_SLOW_MS} THEN 1 ELSE 0 END) AS slow_ttfb_requests,
                      SUM(CASE WHEN ttfb_ms > {TTFB_CRITICAL_MS} THEN 1 ELSE 0 END) AS critical_ttfb_requests,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(input_tokens, 0) ELSE 0 END) AS input_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(output_tokens, 0) ELSE 0 END) AS output_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(cache_read_input_tokens, 0) ELSE 0 END) AS cache_read_input_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(cache_creation_input_tokens, 0) ELSE 0 END) AS cache_creation_input_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(processed_input_tokens, 0) ELSE 0 END) AS processed_input_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(processed_total_tokens, 0) ELSE 0 END) AS processed_total_tokens,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND cache_read_input_tokens IS NOT NULL
                               THEN 1 ELSE 0 END) AS cache_read_reported_requests,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND cache_read_input_tokens > 0
                               THEN 1 ELSE 0 END) AS cache_read_hit_requests,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                               THEN 1 ELSE 0 END) AS cache_read_token_eligible_requests,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND (
                                      token_accounting_semantics IS NULL
                                      OR token_accounting_semantics NOT IN ('subset', 'independent')
                                      OR cache_read_input_tokens IS NULL
                                      OR processed_input_tokens IS NULL
                                    )
                               THEN 1 ELSE 0 END) AS cache_read_token_unknown_requests,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                               THEN MAX(cache_read_input_tokens, 0) ELSE 0 END) AS cache_read_token_numerator,
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                               THEN MAX(processed_input_tokens, 0) ELSE 0 END) AS cache_read_token_denominator,
                      MIN(timestamp) AS first_seen,
                      MAX(timestamp) AS last_seen,
                      AVG(CASE WHEN duration_ms >= 0 THEN duration_ms END) AS average_duration_ms,
                      AVG(CASE WHEN ttfb_ms >= 0 THEN ttfb_ms END) AS average_ttfb_ms,
                      COUNT(DISTINCT {related_expression}) AS related_count,
                      {workspace_expr} AS workspace_paths_json,
                      GROUP_CONCAT(DISTINCT COALESCE(client_kind,'unrecorded_client')) AS client_kinds
               FROM runtime_events{where_sql}
              GROUP BY {group_columns}
              ORDER BY {order_by}, key ASC
              LIMIT ? OFFSET ?"#
    );
    let rows = {
        let mut statement = transaction.prepare(&sql)?;
        let mapped = statement.query_map(params_from_iter(values.iter()), |row| {
            let paths_json = row.get::<_, Option<String>>(29)?;
            let client_kinds = csv_values(row.get::<_, Option<String>>(30)?);
            let cache_read_token_numerator = row.get::<_, i64>(22)?;
            let cache_read_token_denominator = row.get::<_, i64>(23)?;
            Ok((
                DimensionRow {
                    key: row.get(0)?,
                    name: row.get(1)?,
                    source: row.get(2)?,
                    requests: row.get(3)?,
                    successes: row.get(4)?,
                    failures: row.get(5)?,
                    cancelled: row.get(6)?,
                    failovers: row.get(7)?,
                    slow_duration_requests: row.get(8)?,
                    critical_duration_requests: row.get(9)?,
                    slow_ttfb_requests: row.get(10)?,
                    critical_ttfb_requests: row.get(11)?,
                    input_tokens: row.get(12)?,
                    output_tokens: row.get(13)?,
                    cache_read_input_tokens: row.get(14)?,
                    cache_creation_input_tokens: row.get(15)?,
                    processed_input_tokens: row.get(16)?,
                    processed_total_tokens: row.get(17)?,
                    cache_read_reported_requests: row.get(18)?,
                    cache_read_hit_requests: row.get(19)?,
                    cache_read_token_eligible_requests: row.get(20)?,
                    cache_read_token_unknown_requests: row.get(21)?,
                    cache_read_token_rate: ratio(
                        cache_read_token_numerator.max(0) as i128,
                        cache_read_token_denominator.max(0) as i128,
                    ),
                    cache_read_request_rate: ratio(
                        row.get::<_, i64>(19)?.max(0) as i128,
                        row.get::<_, i64>(18)?.max(0) as i128,
                    ),
                    first_seen: row.get(24)?,
                    last_seen: row.get(25)?,
                    average_duration_ms: row.get(26)?,
                    average_ttfb_ms: row.get(27)?,
                    related_count: row.get(28)?,
                    workspace_paths: Vec::new(),
                    client_kinds,
                    cost: CostCoverage::default(),
                },
                paths_json,
            ))
        })?;
        let mut result = Vec::new();
        for mapped in mapped {
            let (mut row, paths_json) = mapped?;
            if let Some(paths_json) = paths_json {
                row.workspace_paths = serde_json::from_str(&paths_json).map_err(|error| {
                    RuntimeQueryError::InvalidInput(format!(
                        "project {} has invalid workspace path projection: {error}",
                        row.key
                    ))
                })?;
            }
            result.push(row);
        }
        result
    };
    let mut rows = rows;
    for row in &mut rows {
        row.cost = dimension_cost_map
            .get(&row.key)
            .cloned()
            .unwrap_or_default();
    }
    transaction.commit()?;
    Ok(DimensionPage {
        api_version: API_VERSION,
        kind,
        rows,
        page: request.page,
        page_size: request.page_size,
        total_count,
        total_pages,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
        has_next: request.page < total_pages,
        has_previous: request.page > 1 && total_pages > 0,
        search,
        sort: request.sort,
        order: request.order,
        filters,
    })
}

/// Calculate cost per dimension key from the exact filtered client rows.
/// Prices can vary by endpoint, model and effective timestamp, so a simple
/// multiplication of an aggregated token total would be wrong whenever a
/// price revision or route-specific price is present.  This bounded pass
/// reuses `CostAccumulator`, the same accounting contract used by trends.
fn dimension_costs(
    transaction: &Transaction<'_>,
    base: &SqlFilter,
    key_expression: &'static str,
    prices: &PriceCatalog,
) -> QueryResult<HashMap<String, CostCoverage>> {
    let sql = format!(
        "SELECT {key_expression} AS dimension_key,timestamp,request_purpose,endpoint_id,\
                effective_model,input_tokens,output_tokens,cache_read_input_tokens,\
                cache_creation_input_tokens,uncached_input_tokens,processed_input_tokens,\
                processed_total_tokens,reasoning_tokens,token_accounting_semantics,\
                token_accounting_quality FROM runtime_events{}",
        base.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(base.values.iter()))?;
    let mut accumulators = HashMap::<String, CostAccumulator>::new();
    while let Some(row) = rows.next()? {
        let key: String = row.get(0)?;
        let trend_row = TrendRow {
            kind: "client".into(),
            timestamp: row.get(1)?,
            outcome: None,
            failover: 0,
            duration_ms: None,
            ttfb_ms: None,
            request_purpose: row.get(2)?,
            endpoint_id: row.get(3)?,
            effective_model: row.get(4)?,
            usage_present: 1,
            input_tokens: row.get(5)?,
            output_tokens: row.get(6)?,
            cache_read_input_tokens: row.get(7)?,
            cache_creation_input_tokens: row.get(8)?,
            reasoning_tokens: row.get(12)?,
            uncached_input_tokens: row.get(9)?,
            processed_input_tokens: row.get(10)?,
            processed_total_tokens: row.get(11)?,
            token_accounting_semantics: row.get(13)?,
            token_accounting_quality: row.get(14)?,
        };
        accumulators.entry(key).or_default().add(&trend_row, prices);
    }
    Ok(accumulators
        .into_iter()
        .map(|(key, accumulator)| (key, accumulator.finish(prices)))
        .collect())
}
