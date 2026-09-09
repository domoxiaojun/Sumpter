//! Runtime query trends domain.

use crate::database::params_from_iter;

use super::{
    API_VERSION, BTreeMap, Connection, CostCoverage, DURATION_CRITICAL_MS, DURATION_SLOW_MS,
    HistorySnapshot, LatencyMetrics, LatencyThresholdBucket, LatencyThresholds, MAX_TREND_POINTS,
    PROJECTION_VERSION, Path, PriceCatalog, QueryResult, RuntimeFilter, RuntimeQueryError,
    SqlFilter, TTFB_CRITICAL_MS, TTFB_SLOW_MS, TokenMetrics, Transaction, TransactionBehavior,
    TrendAccumulator, TrendGranularity, TrendPoint, TrendQuery, TrendRequest, TrendRow,
    TrendSeries, UsageFieldPresence, append_runtime_filter, history_snapshot, load_price_catalog,
    merge_accounting_label, merge_label, meta_i64, ratio, read_connection, require_projection,
    round_cost_numerator,
};

pub fn trends(path: &Path, request: &TrendQuery) -> QueryResult<TrendSeries> {
    let mut connection = read_connection(path)?;
    trends_on(&mut connection, request)
}

pub fn trends_on(connection: &mut Connection, request: &TrendRequest) -> QueryResult<TrendSeries> {
    if !request.from.is_finite() || !request.to.is_finite() || request.from >= request.to {
        return Err(RuntimeQueryError::InvalidInput(
            "trend range must contain finite from < to timestamps".into(),
        ));
    }
    let requested_seconds = match request.granularity {
        TrendGranularity::Hour => 3_600_i64,
        TrendGranularity::Day => 86_400_i64,
        TrendGranularity::Auto | TrendGranularity::MultiDay => 3_600_i64,
    };
    let (seconds, granularity) = choose_trend_bucket(request.from, request.to, requested_seconds)?;
    let point_count = trend_bucket_count(request.from, request.to, seconds)?;
    let mut filters = request.filter.normalized()?;
    filters.from = Some(
        filters
            .from
            .map_or(request.from, |value| value.max(request.from)),
    );
    filters.to = Some(filters.to.map_or(request.to, |value| value.min(request.to)));
    if filters
        .from
        .zip(filters.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RuntimeQueryError::InvalidInput(
            "trend range does not overlap filter range".into(),
        ));
    }

    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(
        &transaction,
        request.snapshot_seq,
        request.history_generation,
    )?;
    let prices = load_price_catalog(&transaction)?;
    if let Some((points, totals)) = try_hourly_rollup(
        &transaction,
        &filters,
        granularity,
        request.from,
        request.to,
        &snapshot,
        &prices,
    )? {
        transaction.commit()?;
        return Ok(TrendSeries {
            api_version: API_VERSION,
            rollup_used: true,
            granularity,
            bucket_seconds: seconds,
            from: request.from,
            to: request.to,
            snapshot_seq: snapshot.snapshot_seq,
            history_generation: snapshot.history_generation,
            retained_from_seq: snapshot.retained_from_seq,
            thresholds: LatencyThresholds::default(),
            points,
            totals,
            filters,
        });
    }
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    builder.raw("kind IN ('client','upstream')");
    builder.le_i64("seq", snapshot.snapshot_seq);
    let mut dimension_filters = filters.clone();
    dimension_filters.from = None;
    dimension_filters.to = None;
    append_runtime_filter(&mut builder, &dimension_filters);
    builder.ge_real("timestamp", filters.from);
    builder.le_real("timestamp", filters.to);
    let sql = format!(
        "SELECT kind,timestamp,outcome,failover,duration_ms,ttfb_ms,request_purpose,endpoint_id,\
                effective_model,usage_present,input_tokens,output_tokens,\
                cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,\
                uncached_input_tokens,processed_input_tokens,processed_total_tokens,\
                token_accounting_semantics,token_accounting_quality \
         FROM runtime_events{} ORDER BY timestamp ASC,seq ASC",
        builder.where_sql()
    );
    let mut buckets = BTreeMap::<i64, TrendAccumulator>::new();
    let first_bucket = bucket_start(request.from, seconds);
    for index in 0..point_count {
        buckets.insert(
            first_bucket.saturating_add((index as i64).saturating_mul(seconds)),
            TrendAccumulator::default(),
        );
    }
    let mut totals = TrendAccumulator::default();
    {
        let mut statement = transaction.prepare(&sql)?;
        let mut rows = statement.query(params_from_iter(builder.values.iter()))?;
        while let Some(row) = rows.next()? {
            let row = TrendRow {
                kind: row.get(0)?,
                timestamp: row.get(1)?,
                outcome: row.get(2)?,
                failover: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                duration_ms: row.get(4)?,
                ttfb_ms: row.get(5)?,
                request_purpose: row.get(6)?,
                endpoint_id: row.get(7)?,
                effective_model: row.get(8)?,
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
            };
            let bucket = bucket_start(row.timestamp, seconds);
            if let Some(accumulator) = buckets.get_mut(&bucket) {
                accumulator.add(&row, &prices);
            }
            totals.add(&row, &prices);
        }
    }
    let points = buckets
        .into_iter()
        .map(|(start, accumulator)| {
            accumulator.finish(start as f64, start.saturating_add(seconds) as f64, &prices)
        })
        .collect();
    let totals = totals.finish(request.from, request.to, &prices);
    transaction.commit()?;
    Ok(TrendSeries {
        api_version: API_VERSION,
        rollup_used: false,
        granularity,
        bucket_seconds: seconds,
        from: request.from,
        to: request.to,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
        thresholds: LatencyThresholds::default(),
        points,
        totals,
        filters,
    })
}

fn try_hourly_rollup(
    transaction: &Transaction<'_>,
    filters: &RuntimeFilter,
    granularity: TrendGranularity,
    from: f64,
    to: f64,
    snapshot: &HistorySnapshot,
    prices: &PriceCatalog,
) -> QueryResult<Option<(Vec<TrendPoint>, TrendPoint)>> {
    // Rollups are global hour buckets. Any high-cardinality filter, a partial
    // hour edge, or a historical snapshot needs exact detail rows instead of
    // mixing data outside the snapshot. Cost is safe to serve from a rollup
    // only when every bucket was built with the current pricing revision.
    let only_time_filters = !filters.has_agent_filter()
        && filters.kind.is_none()
        && filters.outcome.is_none()
        && filters.client_kind.is_none()
        && filters.client_variant.is_none()
        && filters.agent_role.is_none()
        && filters.agent_name.is_none()
        && filters.parent_thread_id.is_none()
        && filters.parent_turn_id.is_none()
        && filters.root_turn_id.is_none()
        && filters.request_purpose.is_none()
        && filters.request_id.is_none()
        && filters.endpoint_id.is_none()
        && filters.model.is_none()
        && filters.project_id.is_none()
        && filters.project_name.is_none()
        && filters.session_id.is_none()
        && filters.failure_kind.is_none()
        && filters.failure_phase.is_none();
    let aligned = from == bucket_start(from, 3_600) as f64
        && to == bucket_start(to, 3_600) as f64
        && to > from;
    let complete = meta_i64(transaction, "hourly_rollup_complete")?.unwrap_or(0) == 1;
    let failed = meta_i64(transaction, "hourly_rollup_failed")?.unwrap_or(0) != 0;
    let max_seq = meta_i64(transaction, "hourly_rollup_max_seq")?.unwrap_or(0);
    let generation = meta_i64(transaction, "hourly_rollup_history_generation")?.unwrap_or(-1);
    if !only_time_filters
        || granularity != TrendGranularity::Hour
        || !aligned
        || !complete
        || failed
        || max_seq != snapshot.snapshot_seq
        || generation != snapshot.history_generation
    {
        return Ok(None);
    }
    let rows = load_hourly_rollups(transaction, from, to)?;
    if rows
        .iter()
        .any(|row| row.cost_price_revision != prices.revision)
    {
        return Ok(None);
    }
    let expected_hours = ((to - from) / 3_600.0) as usize;
    let mut by_start = rows
        .iter()
        .cloned()
        .map(|row| (row.bucket_start, row))
        .collect::<BTreeMap<_, _>>();
    let mut points = Vec::with_capacity(expected_hours);
    for index in 0..expected_hours {
        let start = bucket_start(from, 3_600).saturating_add(index as i64 * 3_600);
        points.push(by_start.remove(&start).map_or_else(
            || TrendPoint {
                bucket_start: start as f64,
                bucket_end: start.saturating_add(3_600) as f64,
                ..TrendPoint::default()
            },
            |row| row.into_point(prices),
        ));
    }
    let totals = merge_rollup_points(&rows, from, to, prices);
    Ok(Some((points, totals)))
}

#[derive(Clone)]
struct HourlyRollup {
    bucket_start: i64,
    bucket_end: i64,
    max_seq: i64,
    client_requests: i64,
    client_successes: i64,
    client_failures: i64,
    client_cancelled: i64,
    client_unknown_results: i64,
    failovers: i64,
    failover_terminal_requests: i64,
    failover_recovered_requests: i64,
    upstream_attempts: i64,
    upstream_successes: i64,
    upstream_failures: i64,
    duration_ms_sum: i64,
    duration_count: i64,
    duration_slow_count: i64,
    duration_critical_count: i64,
    ttfb_ms_sum: i64,
    ttfb_count: i64,
    ttfb_slow_count: i64,
    ttfb_critical_count: i64,
    tokens: TokenMetrics,
    cache_read_token_numerator: i64,
    cache_read_token_denominator: i64,
    _cost_accounting_complete_requests: i64,
    cost_unknown_accounting_requests: i64,
    cost_numerator: i64,
    cost_priced_requests: i64,
    cost_unpriced_requests: i64,
    cost_unknown_requests: i64,
    cost_price_revision: Option<i64>,
}

impl HourlyRollup {
    fn into_point(self, prices: &PriceCatalog) -> TrendPoint {
        let cache_read_token_rate = ratio(
            self.cache_read_token_numerator as i128,
            self.cache_read_token_denominator as i128,
        );
        let cache_read_request_rate = ratio(
            self.tokens.cache_read_hit_requests as i128,
            self.tokens.cache_read_reported_requests as i128,
        );
        let mut tokens = self.tokens;
        tokens.total_tokens = tokens.input_tokens.saturating_add(tokens.output_tokens);
        tokens.cache_read_token_rate = cache_read_token_rate;
        tokens.cache_read_request_rate = cache_read_request_rate;
        TrendPoint {
            bucket_start: self.bucket_start as f64,
            bucket_end: self.bucket_end as f64,
            client_requests: self.client_requests,
            client_successes: self.client_successes,
            client_failures: self.client_failures,
            client_cancelled: self.client_cancelled,
            client_terminal_requests: self
                .client_successes
                .saturating_add(self.client_failures)
                .saturating_add(self.client_cancelled),
            client_unknown_results: self.client_unknown_results,
            failovers: self.failovers,
            failover_terminal_requests: self.failover_terminal_requests,
            failover_recovered_requests: self.failover_recovered_requests,
            failover_recovery_rate: ratio(
                self.failover_recovered_requests as i128,
                self.failover_terminal_requests as i128,
            ),
            upstream_attempts: self.upstream_attempts,
            upstream_successes: self.upstream_successes,
            upstream_failures: self.upstream_failures,
            tokens,
            ttfb_ms: LatencyMetrics {
                observed_requests: self.ttfb_count,
                sum_ms: self.ttfb_ms_sum,
                average_ms: (self.ttfb_count > 0)
                    .then(|| self.ttfb_ms_sum as f64 / self.ttfb_count as f64),
                threshold_buckets: vec![
                    LatencyThresholdBucket {
                        threshold_ms: TTFB_SLOW_MS,
                        exceeded_requests: self.ttfb_slow_count,
                    },
                    LatencyThresholdBucket {
                        threshold_ms: TTFB_CRITICAL_MS,
                        exceeded_requests: self.ttfb_critical_count,
                    },
                ],
            },
            duration_ms: LatencyMetrics {
                observed_requests: self.duration_count,
                sum_ms: self.duration_ms_sum,
                average_ms: (self.duration_count > 0)
                    .then(|| self.duration_ms_sum as f64 / self.duration_count as f64),
                threshold_buckets: vec![
                    LatencyThresholdBucket {
                        threshold_ms: DURATION_SLOW_MS,
                        exceeded_requests: self.duration_slow_count,
                    },
                    LatencyThresholdBucket {
                        threshold_ms: DURATION_CRITICAL_MS,
                        exceeded_requests: self.duration_critical_count,
                    },
                ],
            },
            cost: CostCoverage {
                estimated_cost_micros: round_cost_numerator(self.cost_numerator as i128),
                priced_requests: self.cost_priced_requests,
                unpriced_requests: self.cost_unpriced_requests,
                unknown_accounting_requests: self
                    .cost_unknown_requests
                    .max(self.cost_unknown_accounting_requests),
                complete: self.cost_unpriced_requests == 0 && self.cost_unknown_requests == 0,
                currency: prices.currency.clone(),
                price_version: prices.revision,
            },
        }
    }
}

fn load_hourly_rollups(
    transaction: &Transaction<'_>,
    from: f64,
    to: f64,
) -> QueryResult<Vec<HourlyRollup>> {
    let mut statement = transaction.prepare(
        "SELECT bucket_start,bucket_end,max_seq,client_requests,client_successes,\
                client_failures,client_cancelled,client_unknown_results,failovers,\
                failover_terminal_requests,failover_recovered_requests,upstream_attempts,\
                upstream_successes,upstream_failures,duration_ms_sum,duration_count,\
                duration_slow_count,duration_critical_count,ttfb_ms_sum,ttfb_count,\
                ttfb_slow_count,ttfb_critical_count,input_tokens,output_tokens,cache_read_input_tokens,\
                cache_creation_input_tokens,reasoning_tokens,uncached_input_tokens,\
                processed_input_tokens,processed_total_tokens,usage_present_requests,\
                accounting_known_requests,accounting_unknown_requests,\
                cache_read_reported_requests,cache_read_hit_requests,cache_eligible_requests,\
                cache_unknown_requests,cache_read_token_numerator,cache_read_token_denominator,\
                input_tokens_present,output_tokens_present,cache_read_input_tokens_present,\
                cache_creation_input_tokens_present,reasoning_tokens_present,\
                cost_accounting_complete_requests,cost_unknown_accounting_requests,\
                cost_numerator,cost_priced_requests,cost_unpriced_requests,cost_unknown_requests,\
                cost_price_revision \
         FROM runtime_hourly_rollups WHERE bucket_start>=?1 AND bucket_end<=?2 \
         ORDER BY bucket_start ASC",
    )?;
    let rows = statement.query_map((from, to), |row| {
        Ok(HourlyRollup {
            bucket_start: row.get(0)?,
            bucket_end: row.get(1)?,
            max_seq: row.get(2)?,
            client_requests: row.get(3)?,
            client_successes: row.get(4)?,
            client_failures: row.get(5)?,
            client_cancelled: row.get(6)?,
            client_unknown_results: row.get(7)?,
            failovers: row.get(8)?,
            failover_terminal_requests: row.get(9)?,
            failover_recovered_requests: row.get(10)?,
            upstream_attempts: row.get(11)?,
            upstream_successes: row.get(12)?,
            upstream_failures: row.get(13)?,
            duration_ms_sum: row.get(14)?,
            duration_count: row.get(15)?,
            duration_slow_count: row.get(16)?,
            duration_critical_count: row.get(17)?,
            ttfb_ms_sum: row.get(18)?,
            ttfb_count: row.get(19)?,
            ttfb_slow_count: row.get(20)?,
            ttfb_critical_count: row.get(21)?,
            tokens: TokenMetrics {
                input_tokens: row.get(22)?,
                output_tokens: row.get(23)?,
                cache_read_input_tokens: row.get(24)?,
                cache_creation_input_tokens: row.get(25)?,
                reasoning_tokens: row.get(26)?,
                uncached_input_tokens: row.get(27)?,
                processed_input_tokens: row.get(28)?,
                processed_total_tokens: row.get(29)?,
                total_tokens: 0,
                observed_requests: row.get(30)?,
                accounting_known_requests: row.get(31)?,
                accounting_unknown_requests: row.get(32)?,
                cache_read_reported_requests: row.get(33)?,
                cache_read_hit_requests: row.get(34)?,
                cache_read_token_eligible_requests: row.get(35)?,
                cache_read_token_unknown_requests: row.get(36)?,
                cache_read_token_rate: None,
                cache_read_request_rate: None,
                token_accounting_semantics: String::new(),
                token_accounting_quality: String::new(),
                usage_field_presence: UsageFieldPresence {
                    input_tokens: row.get(39)?,
                    output_tokens: row.get(40)?,
                    cache_read_input_tokens: row.get(41)?,
                    cache_creation_input_tokens: row.get(42)?,
                    reasoning_tokens: row.get(43)?,
                },
            },
            cache_read_token_numerator: row.get(37)?,
            cache_read_token_denominator: row.get(38)?,
            _cost_accounting_complete_requests: row.get(44)?,
            cost_unknown_accounting_requests: row.get(45)?,
            cost_numerator: row.get(46)?,
            cost_priced_requests: row.get(47)?,
            cost_unpriced_requests: row.get(48)?,
            cost_unknown_requests: row.get(49)?,
            cost_price_revision: row.get(50)?,
        })
    })?;
    let rows = rows.collect::<Result<Vec<_>, _>>()?;
    let rollup_max_seq = meta_i64(transaction, "hourly_rollup_max_seq")?.unwrap_or(0);
    if rows
        .iter()
        .any(|row| row.max_seq <= 0 || row.max_seq > rollup_max_seq)
    {
        return Ok(Vec::new());
    }
    Ok(rows)
}

fn merge_rollup_points(
    rows: &[HourlyRollup],
    from: f64,
    to: f64,
    prices: &PriceCatalog,
) -> TrendPoint {
    if rows.len() == 1 {
        return rows[0].clone().into_point(prices);
    }
    let mut tokens = TokenMetrics::default();
    let mut semantics_label = None;
    let mut quality_label = None;
    let mut cache_numerator = 0_i128;
    let mut cache_denominator = 0_i128;
    let mut client_requests = 0_i64;
    let mut client_successes = 0_i64;
    let mut client_failures = 0_i64;
    let mut client_cancelled = 0_i64;
    let mut client_unknown_results = 0_i64;
    let mut failovers = 0_i64;
    let mut failover_terminal_requests = 0_i64;
    let mut failover_recovered_requests = 0_i64;
    let mut upstream_attempts = 0_i64;
    let mut upstream_successes = 0_i64;
    let mut upstream_failures = 0_i64;
    let mut duration_sum = 0_i64;
    let mut duration_count = 0_i64;
    let mut duration_slow = 0_i64;
    let mut duration_critical = 0_i64;
    let mut ttfb_sum = 0_i64;
    let mut ttfb_count = 0_i64;
    let mut ttfb_slow = 0_i64;
    let mut ttfb_critical = 0_i64;
    let mut cost_unknown = 0_i64;
    let mut cost_numerator = 0_i128;
    let mut cost_priced = 0_i64;
    let mut cost_unpriced = 0_i64;
    for row in rows {
        merge_accounting_label(
            &mut semantics_label,
            Some(row.tokens.token_accounting_semantics.as_str()),
        );
        merge_label(
            &mut quality_label,
            Some(row.tokens.token_accounting_quality.as_str()),
        );
        client_requests = client_requests.saturating_add(row.client_requests);
        client_successes = client_successes.saturating_add(row.client_successes);
        client_failures = client_failures.saturating_add(row.client_failures);
        client_cancelled = client_cancelled.saturating_add(row.client_cancelled);
        client_unknown_results = client_unknown_results.saturating_add(row.client_unknown_results);
        failovers = failovers.saturating_add(row.failovers);
        failover_terminal_requests =
            failover_terminal_requests.saturating_add(row.failover_terminal_requests);
        failover_recovered_requests =
            failover_recovered_requests.saturating_add(row.failover_recovered_requests);
        upstream_attempts = upstream_attempts.saturating_add(row.upstream_attempts);
        upstream_successes = upstream_successes.saturating_add(row.upstream_successes);
        upstream_failures = upstream_failures.saturating_add(row.upstream_failures);
        duration_sum = duration_sum.saturating_add(row.duration_ms_sum);
        duration_count = duration_count.saturating_add(row.duration_count);
        duration_slow = duration_slow.saturating_add(row.duration_slow_count);
        duration_critical = duration_critical.saturating_add(row.duration_critical_count);
        ttfb_sum = ttfb_sum.saturating_add(row.ttfb_ms_sum);
        ttfb_count = ttfb_count.saturating_add(row.ttfb_count);
        ttfb_slow = ttfb_slow.saturating_add(row.ttfb_slow_count);
        ttfb_critical = ttfb_critical.saturating_add(row.ttfb_critical_count);
        macro_rules! add_token {
            ($field:ident) => {
                tokens.$field = tokens.$field.saturating_add(row.tokens.$field);
            };
        }
        add_token!(input_tokens);
        add_token!(output_tokens);
        add_token!(cache_read_input_tokens);
        add_token!(cache_creation_input_tokens);
        add_token!(reasoning_tokens);
        add_token!(uncached_input_tokens);
        add_token!(processed_input_tokens);
        add_token!(processed_total_tokens);
        add_token!(observed_requests);
        add_token!(accounting_known_requests);
        add_token!(accounting_unknown_requests);
        add_token!(cache_read_reported_requests);
        add_token!(cache_read_hit_requests);
        add_token!(cache_read_token_eligible_requests);
        add_token!(cache_read_token_unknown_requests);
        tokens.usage_field_presence.input_tokens = tokens
            .usage_field_presence
            .input_tokens
            .saturating_add(row.tokens.usage_field_presence.input_tokens);
        tokens.usage_field_presence.output_tokens = tokens
            .usage_field_presence
            .output_tokens
            .saturating_add(row.tokens.usage_field_presence.output_tokens);
        tokens.usage_field_presence.cache_read_input_tokens = tokens
            .usage_field_presence
            .cache_read_input_tokens
            .saturating_add(row.tokens.usage_field_presence.cache_read_input_tokens);
        tokens.usage_field_presence.cache_creation_input_tokens = tokens
            .usage_field_presence
            .cache_creation_input_tokens
            .saturating_add(row.tokens.usage_field_presence.cache_creation_input_tokens);
        tokens.usage_field_presence.reasoning_tokens = tokens
            .usage_field_presence
            .reasoning_tokens
            .saturating_add(row.tokens.usage_field_presence.reasoning_tokens);
        cache_numerator = cache_numerator.saturating_add(row.cache_read_token_numerator as i128);
        cache_denominator =
            cache_denominator.saturating_add(row.cache_read_token_denominator as i128);
        cost_numerator = cost_numerator.saturating_add(row.cost_numerator as i128);
        cost_priced = cost_priced.saturating_add(row.cost_priced_requests);
        cost_unpriced = cost_unpriced.saturating_add(row.cost_unpriced_requests);
        cost_unknown = cost_unknown.saturating_add(
            row.cost_unknown_requests
                .max(row.cost_unknown_accounting_requests),
        );
    }
    tokens.cache_read_token_rate = ratio(cache_numerator, cache_denominator);
    tokens.cache_read_request_rate = ratio(
        tokens.cache_read_hit_requests as i128,
        tokens.cache_read_reported_requests as i128,
    );
    tokens.total_tokens = tokens.input_tokens.saturating_add(tokens.output_tokens);
    tokens.token_accounting_semantics = semantics_label.unwrap_or_else(|| "unknown".into());
    tokens.token_accounting_quality = quality_label.unwrap_or_else(|| "unknown".into());
    TrendPoint {
        bucket_start: from,
        bucket_end: to,
        client_requests,
        client_successes,
        client_failures,
        client_cancelled,
        client_terminal_requests: client_successes
            .saturating_add(client_failures)
            .saturating_add(client_cancelled),
        client_unknown_results,
        failovers,
        failover_terminal_requests,
        failover_recovered_requests,
        failover_recovery_rate: ratio(
            failover_recovered_requests as i128,
            failover_terminal_requests as i128,
        ),
        upstream_attempts,
        upstream_successes,
        upstream_failures,
        tokens,
        ttfb_ms: LatencyMetrics {
            observed_requests: ttfb_count,
            sum_ms: ttfb_sum,
            average_ms: (ttfb_count > 0).then(|| ttfb_sum as f64 / ttfb_count as f64),
            threshold_buckets: vec![
                LatencyThresholdBucket {
                    threshold_ms: TTFB_SLOW_MS,
                    exceeded_requests: ttfb_slow,
                },
                LatencyThresholdBucket {
                    threshold_ms: TTFB_CRITICAL_MS,
                    exceeded_requests: ttfb_critical,
                },
            ],
        },
        duration_ms: LatencyMetrics {
            observed_requests: duration_count,
            sum_ms: duration_sum,
            average_ms: (duration_count > 0).then(|| duration_sum as f64 / duration_count as f64),
            threshold_buckets: vec![
                LatencyThresholdBucket {
                    threshold_ms: DURATION_SLOW_MS,
                    exceeded_requests: duration_slow,
                },
                LatencyThresholdBucket {
                    threshold_ms: DURATION_CRITICAL_MS,
                    exceeded_requests: duration_critical,
                },
            ],
        },
        cost: CostCoverage {
            estimated_cost_micros: round_cost_numerator(cost_numerator),
            priced_requests: cost_priced,
            unpriced_requests: cost_unpriced,
            unknown_accounting_requests: cost_unknown,
            complete: cost_unpriced == 0 && cost_unknown == 0,
            currency: prices.currency.clone(),
            price_version: prices.revision,
        },
    }
}

fn bucket_start(timestamp: f64, seconds: i64) -> i64 {
    (timestamp / seconds as f64).floor() as i64 * seconds
}

fn trend_bucket_count(from: f64, to: f64, seconds: i64) -> QueryResult<usize> {
    let first = bucket_start(from, seconds);
    let end_bucket = ((to / seconds as f64).ceil() as i64).saturating_mul(seconds);
    let count = end_bucket
        .saturating_sub(first)
        .checked_div(seconds)
        .unwrap_or(0)
        .max(1);
    usize::try_from(count)
        .map_err(|_| RuntimeQueryError::InvalidInput("trend range is too large".into()))
}

/// Select a chart bucket that always fits the bounded response contract.
/// Hour/day remain the common cases; older retained histories can span years,
/// so Auto (and an explicit fine-grained request) progressively widens to an
/// integral number of days instead of returning a 400 error.
fn choose_trend_bucket(
    from: f64,
    to: f64,
    requested_seconds: i64,
) -> QueryResult<(i64, TrendGranularity)> {
    let requested_seconds = requested_seconds.max(3_600);
    if trend_bucket_count(from, to, requested_seconds)? <= MAX_TREND_POINTS {
        return Ok((
            requested_seconds,
            match requested_seconds {
                3_600 => TrendGranularity::Hour,
                86_400 => TrendGranularity::Day,
                _ => TrendGranularity::MultiDay,
            },
        ));
    }

    let mut days = (requested_seconds / 86_400).max(1);
    loop {
        let seconds = days
            .checked_mul(86_400)
            .ok_or_else(|| RuntimeQueryError::InvalidInput("trend range is too large".into()))?;
        if trend_bucket_count(from, to, seconds)? <= MAX_TREND_POINTS {
            return Ok((
                seconds,
                if days == 1 {
                    TrendGranularity::Day
                } else {
                    TrendGranularity::MultiDay
                },
            ));
        }
        days = days
            .checked_add(1)
            .ok_or_else(|| RuntimeQueryError::InvalidInput("trend range is too large".into()))?;
    }
}
