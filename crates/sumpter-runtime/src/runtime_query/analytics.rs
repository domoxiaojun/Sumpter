//! Runtime query analytics domain.

use super::facets_queries::analytics_facets_for_filter;

use rusqlite::params_from_iter;

use super::csv_values;

use super::{
    API_VERSION, AnalyticsAppliedFilters, AnalyticsDimensionRow, AnalyticsSummary, BTreeMap,
    Connection, HashMap, Path, QueryResult, RuntimeFilter, RuntimeQueryError, SqlFilter,
    TokenAccumulator, TokenMetrics, ToolCallRow, Transaction, TransactionBehavior, TrendRow,
    analytics_client_builder, analytics_upstream_builder, history_snapshot,
    merge_range_lower_bound, ratio, read_connection, require_projection,
};

pub fn analytics(
    path: &Path,
    range: &str,
    filter: &RuntimeFilter,
) -> QueryResult<AnalyticsSummary> {
    let mut connection = read_connection(path)?;
    analytics_on(&mut connection, range, filter)
}

pub fn analytics_on(
    connection: &mut Connection,
    range: &str,
    filter: &RuntimeFilter,
) -> QueryResult<AnalyticsSummary> {
    let now = current_apple_timestamp();
    let from = match range {
        "1h" => Some(now - 3_600.0),
        "24h" => Some(now - 86_400.0),
        // HTTP callers may provide a local-midnight `from`; when they do not,
        // use a deterministic UTC calendar-day boundary rather than querying
        // the entire database.
        "today" => Some(utc_today_start(now)),
        "7d" => Some(now - 7.0 * 86_400.0),
        "30d" => Some(now - 30.0 * 86_400.0),
        "all" => None,
        _ => {
            return Err(RuntimeQueryError::InvalidInput(
                "range must be today, 1h, 24h, 7d, 30d, or all".into(),
            ));
        }
    };
    let mut filters = filter.normalized()?;
    filters.from = merge_range_lower_bound(range, filters.from, from);
    filters.to = Some(filters.to.map_or(now, |value| value.min(now)));
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, None, None)?;
    let client_builder = analytics_client_builder(&filters, snapshot.snapshot_seq);
    let client_where_sql = client_builder.where_sql();
    let aggregate_sql = format!(
        "SELECT COUNT(*),\
           SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END),\
           SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END),\
           SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END),\
           SUM(CASE WHEN failover=1 THEN 1 ELSE 0 END),\
           AVG(CASE WHEN duration_ms>=0 THEN duration_ms END),\
           AVG(CASE WHEN ttfb_ms>=0 THEN ttfb_ms END),\
           SUM(CASE WHEN duration_ms<1000 THEN 1 ELSE 0 END),\
           SUM(CASE WHEN duration_ms>=1000 AND duration_ms<3000 THEN 1 ELSE 0 END),\
           SUM(CASE WHEN duration_ms>=3000 AND duration_ms<6000 THEN 1 ELSE 0 END),\
           SUM(CASE WHEN duration_ms>=6000 THEN 1 ELSE 0 END),\
           SUM(CASE WHEN codex_metadata_present=1 THEN 1 ELSE 0 END),\
           SUM(CASE WHEN attribution_scope='internal_feature' THEN 1 ELSE 0 END)\
         FROM runtime_events{client_where_sql}"
    );
    let (
        client_requests,
        client_successes,
        client_failures,
        client_cancelled,
        failovers,
        average_duration_ms,
        average_ttfb_ms,
        latency_under_1s,
        latency_1s_to_3s,
        latency_3s_to_6s,
        latency_over_6s,
        codex_metadata_present,
        internal_feature_requests,
    ) = transaction.query_row(
        &aggregate_sql,
        params_from_iter(client_builder.values.iter()),
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, Option<i64>>(7)?.unwrap_or(0),
                row.get::<_, Option<i64>>(8)?.unwrap_or(0),
                row.get::<_, Option<i64>>(9)?.unwrap_or(0),
                row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                row.get::<_, Option<i64>>(11)?.unwrap_or(0),
                row.get::<_, Option<i64>>(12)?.unwrap_or(0),
            ))
        },
    )?;
    let upstream_builder = analytics_upstream_builder(&filters, snapshot.snapshot_seq);
    let upstream_sql = format!(
        "SELECT COUNT(*),SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END)\
         FROM runtime_events{}",
        upstream_builder.where_sql()
    );
    let (upstream_attempts, upstream_successes, upstream_failures) = transaction.query_row(
        &upstream_sql,
        params_from_iter(upstream_builder.values.iter()),
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?.unwrap_or(0),
                row.get::<_, Option<i64>>(1)?.unwrap_or(0),
                row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            ))
        },
    )?;
    let token_usage = analytics_token_metrics(&transaction, &client_builder)?;
    let dimensions = AnalyticsDimensions {
        endpoints: endpoint_dimensions(&transaction, &client_builder, &upstream_builder)?,
        models: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(effective_model,client_model,upstream_model,'unrecorded')",
            Some("kind='client'"),
        )?,
        client_variants: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(client_variant,'unknown')",
            None,
        )?,
        agent_roles: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(agent_role,'unknown')",
            None,
        )?,
        agent_names: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(agent_name,'unknown')",
            None,
        )?,
        parent_threads: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(parent_thread_id,'unknown')",
            None,
        )?,
        parent_turns: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(parent_turn_id,'unknown')",
            None,
        )?,
        root_turns: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(root_turn_id,'unknown')",
            None,
        )?,
        client_kinds: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(client_kind,'unrecorded_client')",
            Some("kind='client'"),
        )?,
        request_purposes: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(request_purpose,'unrecorded')",
            Some("kind='client'"),
        )?,
        feature_rules: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(feature_rule_id,'none')",
            Some("kind='client'"),
        )?,
        protocol_routes: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(source_format,'unrecorded') || '->' || \
             COALESCE(target_format,'unrecorded') || ':' || COALESCE(route_mode,'unrecorded')",
            Some("kind='client'"),
        )?,
        failure_kinds: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(failure_kind,'unclassified')",
            Some("outcome='failed'"),
        )?,
        failure_phases: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(failure_phase,'unclassified')",
            Some("outcome='failed'"),
        )?,
        upstream_statuses: analytics_dimension(
            &transaction,
            &upstream_builder,
            "COALESCE(CAST(upstream_status_code AS TEXT),'none')",
            Some("kind='upstream'"),
        )?,
        stream_terminals: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(stream_terminal,'unobserved')",
            Some("kind='client'"),
        )?,
        projects: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(project_name,project_id,'unidentified_project')",
            Some("kind='client' AND COALESCE(attribution_scope,'unknown') != 'internal_feature'"),
        )?,
        internal_features: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(codex_thread_class,'unknown')",
            Some("kind='client' AND attribution_scope='internal_feature'"),
        )?,
        sessions: analytics_dimension(
            &transaction,
            &client_builder,
            "COALESCE(session_key,'unidentified_session')",
            Some("kind='client'"),
        )?,
    };
    let facets = analytics_facets_for_filter(&transaction, &filters, snapshot.snapshot_seq)?;
    let tool_calls = analytics_tool_calls(&transaction, &client_builder)?;
    transaction.commit()?;
    let completed = client_successes
        .saturating_add(client_failures)
        .saturating_add(client_cancelled);
    Ok(AnalyticsSummary {
        api_version: API_VERSION,
        range: range.to_owned(),
        // Report the effective lower bound after applying the caller's
        // explicit local-midnight filter and the range guard.
        from: filters.from,
        to: Some(now),
        client_requests,
        client_successes,
        client_failures,
        client_cancelled,
        client_pending: client_requests.saturating_sub(completed),
        client_success_rate: ratio(client_successes as i128, completed as i128)
            .map(|value| value * 100.0),
        upstream_attempts,
        upstream_successes,
        upstream_failures,
        failovers,
        average_duration_ms,
        average_ttfb_ms,
        latency_buckets: BTreeMap::from([
            ("under1s".into(), latency_under_1s),
            ("from1sTo3s".into(), latency_1s_to_3s),
            ("from3sTo6s".into(), latency_3s_to_6s),
            ("over6s".into(), latency_over_6s),
        ]),
        token_usage,
        endpoints: dimensions.endpoints,
        models: dimensions.models,
        client_kinds: dimensions.client_kinds,
        client_variants: dimensions.client_variants,
        agent_roles: dimensions.agent_roles,
        agent_names: dimensions.agent_names,
        parent_threads: dimensions.parent_threads,
        parent_turns: dimensions.parent_turns,
        root_turns: dimensions.root_turns,

        request_purposes: dimensions.request_purposes,
        feature_rules: dimensions.feature_rules,
        protocol_routes: dimensions.protocol_routes,
        failure_kinds: dimensions.failure_kinds,
        failure_phases: dimensions.failure_phases,
        upstream_statuses: dimensions.upstream_statuses,
        stream_terminals: dimensions.stream_terminals,
        projects: dimensions.projects,
        sessions: dimensions.sessions,
        tool_calls,
        codex_metadata_present,
        internal_feature_requests,
        internal_features: dimensions.internal_features,
        facets,
        skipped_events: 0,
        truncated: false,
        filters_applied: true,
        filter_warning: None,
        applied_filters: AnalyticsAppliedFilters {
            client_kind: filters.client_kind.clone(),
            client_variant: filters.client_variant.clone(),
            agent_role: filters.agent_role.clone(),
            agent_name: filters.agent_name.clone(),
            parent_thread_id: filters.parent_thread_id.clone(),
            parent_turn_id: filters.parent_turn_id.clone(),
            root_turn_id: filters.root_turn_id.clone(),
            endpoint_id: filters.endpoint_id.clone(),
            project_id: filters.project_id.clone(),
            project: filters.project_name.clone(),
            session_id: filters.session_id.clone(),
        },
    })
}

struct AnalyticsDimensions {
    endpoints: Vec<AnalyticsDimensionRow>,
    models: Vec<AnalyticsDimensionRow>,
    client_kinds: Vec<AnalyticsDimensionRow>,
    client_variants: Vec<AnalyticsDimensionRow>,
    agent_roles: Vec<AnalyticsDimensionRow>,
    agent_names: Vec<AnalyticsDimensionRow>,
    parent_threads: Vec<AnalyticsDimensionRow>,
    parent_turns: Vec<AnalyticsDimensionRow>,
    root_turns: Vec<AnalyticsDimensionRow>,

    request_purposes: Vec<AnalyticsDimensionRow>,
    feature_rules: Vec<AnalyticsDimensionRow>,
    protocol_routes: Vec<AnalyticsDimensionRow>,
    failure_kinds: Vec<AnalyticsDimensionRow>,
    failure_phases: Vec<AnalyticsDimensionRow>,
    upstream_statuses: Vec<AnalyticsDimensionRow>,
    stream_terminals: Vec<AnalyticsDimensionRow>,
    projects: Vec<AnalyticsDimensionRow>,
    internal_features: Vec<AnalyticsDimensionRow>,
    sessions: Vec<AnalyticsDimensionRow>,
}

#[derive(Debug, Clone)]
struct EndpointProjectionRow {
    seq: i64,
    request_id: Option<String>,
    outcome: Option<String>,
    endpoint_id: Option<String>,
    endpoint_name: Option<String>,
    failover: i64,
    duration_ms: Option<i64>,
    ttfb_ms: Option<i64>,
    request_purpose: Option<String>,
    usage_present: i64,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    uncached_input_tokens: Option<i64>,
    processed_input_tokens: Option<i64>,
    processed_total_tokens: Option<i64>,
    token_accounting_semantics: Option<String>,
    token_accounting_quality: Option<String>,
}

fn load_endpoint_projection_rows(
    transaction: &Transaction<'_>,
    builder: &SqlFilter,
) -> QueryResult<Vec<EndpointProjectionRow>> {
    let sql = format!(
        "SELECT seq,request_id,outcome,endpoint_id,endpoint_name,failover,duration_ms,ttfb_ms,\
                request_purpose,usage_present,input_tokens,output_tokens,cache_read_input_tokens,\
                cache_creation_input_tokens,reasoning_tokens,uncached_input_tokens,\
                processed_input_tokens,processed_total_tokens,token_accounting_semantics,\
                token_accounting_quality FROM runtime_events{} ORDER BY seq ASC",
        builder.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(builder.values.iter()), |row| {
        Ok(EndpointProjectionRow {
            seq: row.get(0)?,
            request_id: row.get(1)?,
            outcome: row.get(2)?,
            endpoint_id: row.get(3)?,
            endpoint_name: row.get(4)?,
            failover: row.get::<_, Option<i64>>(5)?.unwrap_or(0),
            duration_ms: row.get(6)?,
            ttfb_ms: row.get(7)?,
            request_purpose: row.get(8)?,
            usage_present: row.get::<_, Option<i64>>(9)?.unwrap_or(0),
            input_tokens: row.get(10)?,
            output_tokens: row.get(11)?,
            cache_read_input_tokens: row.get(12)?,
            cache_creation_input_tokens: row.get(13)?,
            reasoning_tokens: row.get(14)?,
            uncached_input_tokens: row.get(15)?,
            processed_input_tokens: row.get(16)?,
            processed_total_tokens: row.get(17)?,
            token_accounting_semantics: row.get(18)?,
            token_accounting_quality: row.get(19)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[derive(Default)]
struct EndpointAccumulator {
    attempts: i64,
    successes: i64,
    failures: i64,
    cancelled: i64,
    failovers: i64,
    tokens: TokenAccumulator,
}

impl EndpointAccumulator {
    fn add_attempt(&mut self, row: &EndpointProjectionRow) {
        self.attempts = self.attempts.saturating_add(1);
        match row.outcome.as_deref() {
            Some("succeeded") => self.successes = self.successes.saturating_add(1),
            Some("failed") => self.failures = self.failures.saturating_add(1),
            Some("cancelled") => self.cancelled = self.cancelled.saturating_add(1),
            _ => {}
        }
        self.failovers = self.failovers.saturating_add(i64::from(row.failover != 0));
    }

    fn add_tokens(&mut self, row: &EndpointProjectionRow) {
        self.tokens.add(&TrendRow {
            kind: "client".into(),
            timestamp: 0.0,
            outcome: row.outcome.clone(),
            failover: row.failover,
            duration_ms: row.duration_ms,
            ttfb_ms: row.ttfb_ms,
            request_purpose: row.request_purpose.clone(),
            endpoint_id: None,
            effective_model: None,
            usage_present: row.usage_present,
            input_tokens: row.input_tokens,
            output_tokens: row.output_tokens,
            cache_read_input_tokens: row.cache_read_input_tokens,
            cache_creation_input_tokens: row.cache_creation_input_tokens,
            reasoning_tokens: row.reasoning_tokens,
            uncached_input_tokens: row.uncached_input_tokens,
            processed_input_tokens: row.processed_input_tokens,
            processed_total_tokens: row.processed_total_tokens,
            token_accounting_semantics: row.token_accounting_semantics.clone(),
            token_accounting_quality: row.token_accounting_quality.clone(),
        });
    }

    fn into_row(self, name: String) -> AnalyticsDimensionRow {
        let metrics = self.tokens.finish();
        AnalyticsDimensionRow {
            name,
            attempts: self.attempts,
            successes: self.successes,
            failures: self.failures,
            cancelled: self.cancelled,
            pending: self
                .attempts
                .saturating_sub(self.successes + self.failures + self.cancelled),
            failovers: self.failovers,
            input_tokens: metrics.input_tokens,
            output_tokens: metrics.output_tokens,
            cache_read_input_tokens: metrics.cache_read_input_tokens,
            cache_creation_input_tokens: metrics.cache_creation_input_tokens,
            reasoning_tokens: metrics.reasoning_tokens,
            uncached_input_tokens: metrics.uncached_input_tokens,
            processed_input_tokens: metrics.processed_input_tokens,
            processed_total_tokens: metrics.processed_total_tokens,
            total_tokens: metrics.total_tokens,
            cache_read_reported_requests: metrics.cache_read_reported_requests,
            cache_read_hit_requests: metrics.cache_read_hit_requests,
            cache_read_token_eligible_requests: metrics.cache_read_token_eligible_requests,
            cache_read_token_unknown_requests: metrics.cache_read_token_unknown_requests,
            cache_read_token_rate: metrics.cache_read_token_rate,
            cache_read_request_rate: metrics.cache_read_request_rate,
            project_source: None,
            workspace_paths: Vec::new(),
            projects: Vec::new(),
            client_kinds: Vec::new(),
        }
    }
}

fn endpoint_label(row: &EndpointProjectionRow) -> Option<String> {
    let endpoint_id = row.endpoint_id.as_ref()?;
    Some(
        row.endpoint_name
            .clone()
            .unwrap_or_else(|| endpoint_id.clone()),
    )
}

fn endpoint_dimensions(
    transaction: &Transaction<'_>,
    client_builder: &SqlFilter,
    upstream_builder: &SqlFilter,
) -> QueryResult<Vec<AnalyticsDimensionRow>> {
    let clients = load_endpoint_projection_rows(transaction, client_builder)?;
    let upstreams = load_endpoint_projection_rows(transaction, upstream_builder)?;
    let mut by_request = HashMap::<String, Vec<&EndpointProjectionRow>>::new();
    for row in &upstreams {
        if let Some(request_id) = row.request_id.as_ref() {
            by_request.entry(request_id.clone()).or_default().push(row);
        }
    }
    let mut accumulators = BTreeMap::<String, EndpointAccumulator>::new();
    for row in &upstreams {
        let Some(label) = endpoint_label(row) else {
            continue;
        };
        accumulators.entry(label).or_default().add_attempt(row);
    }
    for row in &clients {
        let Some(request_id) = row.request_id.as_ref() else {
            // A client row with its own endpoint_id is already a confirmed
            // association. Rows without either endpoint metadata or an
            // upstream attempt (for example pre-routing 401s) are omitted.
            let Some(label) = endpoint_label(row) else {
                continue;
            };
            let accumulator = accumulators.entry(label).or_default();
            accumulator.add_attempt(row);
            accumulator.add_tokens(row);
            continue;
        };
        let Some(attempts) = by_request.get(request_id) else {
            let Some(label) = endpoint_label(row) else {
                // Keep client totals intact, but do not invent an entry when
                // the request has no endpoint metadata to establish one.
                continue;
            };
            let accumulator = accumulators.entry(label).or_default();
            accumulator.add_attempt(row);
            accumulator.add_tokens(row);
            continue;
        };
        // Prefer the last successful attempt; if the request never recovered,
        // use the last attempt.  Client usage is attached there without
        // creating a duplicate endpoint attempt.
        let selected = attempts
            .iter()
            .filter(|attempt| attempt.outcome.as_deref() == Some("succeeded"))
            .max_by_key(|attempt| attempt.seq)
            .copied()
            .or_else(|| attempts.iter().max_by_key(|attempt| attempt.seq).copied());
        if let Some(label) = selected
            .and_then(endpoint_label)
            .or_else(|| endpoint_label(row))
        {
            accumulators.entry(label).or_default().add_tokens(row);
        }
    }
    let mut rows = accumulators
        .into_iter()
        .map(|(name, accumulator)| accumulator.into_row(name))
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .attempts
            .cmp(&left.attempts)
            .then_with(|| left.name.cmp(&right.name))
    });
    rows.truncate(50);
    Ok(rows)
}

fn analytics_dimension(
    transaction: &Transaction<'_>,
    base: &SqlFilter,
    expression: &'static str,
    extra: Option<&'static str>,
) -> QueryResult<Vec<AnalyticsDimensionRow>> {
    let mut clauses = base.clauses.clone();
    if let Some(extra) = extra {
        clauses.push(extra.into());
    }
    let where_sql = format!(" WHERE {}", clauses.join(" AND "));
    let sql = format!(
        r#"SELECT {expression} AS name,
                      COUNT(*),
                      SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END),
                      SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END),
                      SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END),
                      SUM(CASE WHEN outcome IS NULL THEN 1 ELSE 0 END),
                      SUM(CASE WHEN failover=1 THEN 1 ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(output_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(cache_read_input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(cache_creation_input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(reasoning_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(uncached_input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(processed_input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                               THEN COALESCE(processed_total_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND cache_read_input_tokens IS NOT NULL
                               THEN 1 ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND cache_read_input_tokens > 0
                               THEN 1 ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                               THEN 1 ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND (
                                      token_accounting_semantics IS NULL
                                      OR token_accounting_semantics NOT IN ('subset', 'independent')
                                      OR cache_read_input_tokens IS NULL
                                      OR processed_input_tokens IS NULL
                                    )
                               THEN 1 ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                               THEN MAX(cache_read_input_tokens, 0) ELSE 0 END),
                      SUM(CASE WHEN (request_purpose != 'token_count' OR request_purpose IS NULL)
                                    AND usage_present=1
                                    AND token_accounting_semantics IN ('subset', 'independent')
                                    AND cache_read_input_tokens IS NOT NULL
                                    AND processed_input_tokens IS NOT NULL
                      THEN MAX(processed_input_tokens, 0) ELSE 0 END),
                      MAX(project_source),
                      MAX(workspace_paths_json),
                      GROUP_CONCAT(DISTINCT COALESCE(project_name,project_id,'unidentified_project')),
                      GROUP_CONCAT(DISTINCT COALESCE(client_kind,'unrecorded_client'))
               FROM runtime_events{where_sql}
              GROUP BY name
              ORDER BY COUNT(*) DESC, name ASC
              LIMIT 50"#
    );
    let mut statement = transaction.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(base.values.iter()), |row| {
        let input_tokens = row.get::<_, i64>(7)?;
        let output_tokens = row.get::<_, i64>(8)?;
        Ok(AnalyticsDimensionRow {
            name: row.get(0)?,
            attempts: row.get(1)?,
            successes: row.get(2)?,
            failures: row.get(3)?,
            cancelled: row.get(4)?,
            pending: row.get(5)?,
            failovers: row.get(6)?,
            input_tokens,
            output_tokens,
            cache_read_input_tokens: row.get(9)?,
            cache_creation_input_tokens: row.get(10)?,
            reasoning_tokens: row.get(11)?,
            uncached_input_tokens: row.get(12)?,
            processed_input_tokens: row.get(13)?,
            processed_total_tokens: row.get(14)?,
            total_tokens: input_tokens.saturating_add(output_tokens),
            cache_read_reported_requests: row.get(15)?,
            cache_read_hit_requests: row.get(16)?,
            cache_read_token_eligible_requests: row.get(17)?,
            cache_read_token_unknown_requests: row.get(18)?,
            cache_read_token_rate: ratio(
                row.get::<_, i64>(19)?.max(0) as i128,
                row.get::<_, i64>(20)?.max(0) as i128,
            ),
            cache_read_request_rate: ratio(
                row.get::<_, i64>(16)?.max(0) as i128,
                row.get::<_, i64>(15)?.max(0) as i128,
            ),
            project_source: row.get(21)?,
            workspace_paths: row
                .get::<_, Option<String>>(22)?
                .and_then(|value| serde_json::from_str(&value).ok())
                .unwrap_or_default(),
            projects: csv_values(row.get(23)?),
            client_kinds: csv_values(row.get(24)?),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn analytics_tool_calls(
    transaction: &Transaction<'_>,
    base: &SqlFilter,
) -> QueryResult<Vec<ToolCallRow>> {
    let mut clauses = base.clauses.clone();
    clauses.push("kind='client'".into());
    let sql = format!(
        "SELECT tool_calls_json FROM runtime_events WHERE {}",
        clauses.join(" AND ")
    );
    let mut counts = BTreeMap::<String, i64>::new();
    let mut statement = transaction.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(base.values.iter()), |row| {
        row.get::<_, Option<String>>(0)
    })?;
    for row in rows {
        let Some(raw) = row? else { continue };
        let Ok(names) = serde_json::from_str::<Vec<String>>(&raw) else {
            continue;
        };
        for name in names {
            *counts.entry(name).or_default() += 1;
        }
    }
    Ok(counts
        .into_iter()
        .map(|(name, count)| ToolCallRow { name, count })
        .collect())
}

fn analytics_token_metrics(
    transaction: &Transaction<'_>,
    base: &SqlFilter,
) -> QueryResult<TokenMetrics> {
    let mut clauses = base.clauses.clone();
    clauses.push("kind='client'".into());
    let sql = format!(
        "SELECT timestamp,outcome,failover,duration_ms,ttfb_ms,request_purpose,effective_model,\
                usage_present,input_tokens,output_tokens,cache_read_input_tokens,\
                cache_creation_input_tokens,reasoning_tokens,uncached_input_tokens,\
                processed_input_tokens,processed_total_tokens,token_accounting_semantics,\
                token_accounting_quality FROM runtime_events WHERE {}",
        clauses.join(" AND ")
    );
    let mut accumulator = TokenAccumulator::default();
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(base.values.iter()))?;
    while let Some(row) = rows.next()? {
        accumulator.add(&TrendRow {
            kind: "client".into(),
            timestamp: row.get(0)?,
            outcome: row.get(1)?,
            failover: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            duration_ms: row.get(3)?,
            ttfb_ms: row.get(4)?,
            request_purpose: row.get(5)?,
            endpoint_id: None,
            effective_model: row.get(6)?,
            usage_present: row.get::<_, Option<i64>>(7)?.unwrap_or(0),
            input_tokens: row.get(8)?,
            output_tokens: row.get(9)?,
            cache_read_input_tokens: row.get(10)?,
            cache_creation_input_tokens: row.get(11)?,
            reasoning_tokens: row.get(12)?,
            uncached_input_tokens: row.get(13)?,
            processed_input_tokens: row.get(14)?,
            processed_total_tokens: row.get(15)?,
            token_accounting_semantics: row.get(16)?,
            token_accounting_quality: row.get(17)?,
        });
    }
    Ok(accumulator.finish())
}

fn current_apple_timestamp() -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() - APPLE_EPOCH_OFFSET_SECS)
        .unwrap_or(0.0)
}

fn utc_today_start(now: f64) -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    const DAY_SECS: f64 = 86_400.0;
    let unix = now + APPLE_EPOCH_OFFSET_SECS;
    unix - unix.rem_euclid(DAY_SECS) - APPLE_EPOCH_OFFSET_SECS
}
