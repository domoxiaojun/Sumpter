use super::{
    Connection, DURATION_CRITICAL_MS, DURATION_SLOW_MS, HashSet, KIND_CLIENT, KIND_UPSTREAM,
    OptionalExtension, PROJECTION_BACKFILL_BATCH, PROJECTION_VERSION, RuntimeEvent, SCHEMA_VERSION,
    TTFB_CRITICAL_MS, TTFB_SLOW_MS, meta_i64, now, params, set_meta, update_event_projection,
};

pub(super) fn setup_connection(connection: &mut Connection) -> crate::database::Result<()> {
    if let Some(issue) =
        database_issue_on(connection).map_err(crate::database::Error::InvalidParameterName)?
    {
        return Err(crate::database::Error::InvalidParameterName(issue.message));
    }
    connection.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = FULL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;
         PRAGMA wal_autocheckpoint = 1000;
         PRAGMA cache_size = -2048;",
    )?;
    crate::entities::create_schema(connection)?;
    let previous_version = meta_i64(connection, "schema_version")?.unwrap_or(0);
    if previous_version > SCHEMA_VERSION {
        return Err(crate::database::Error::InvalidParameterName(format!(
            "runtime schema {previous_version} is newer than supported {SCHEMA_VERSION}"
        )));
    }
    let hourly_rollup_meta_exists = meta_i64(connection, "hourly_rollup_complete")?.is_some();
    let unprojected_exists = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events
                       WHERE projection_version>=0 AND projection_version<>?1 LIMIT 1)",
        params![PROJECTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    let retained_event_count = connection.query_row(
        "SELECT COUNT(*) FROM runtime_events",
        crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    let retained_from_seq = connection.query_row(
        "SELECT COALESCE(MIN(seq), COALESCE((SELECT CAST(value AS INTEGER) FROM runtime_meta WHERE key='next_seq'), 1)) FROM runtime_events", crate::database::params![],
        |row| row.get::<_, i64>(0),
    )?;
    let defaults = [
        ("reset_generation", "0".to_string()),
        ("next_seq", "1".to_string()),
        ("next_change_seq", "1".to_string()),
        ("history_generation", "0".to_string()),
        ("retained_from_seq", retained_from_seq.to_string()),
        ("projection_backfill_cursor", "0".to_string()),
        (
            "projection_backfill_complete",
            i64::from(!unprojected_exists).to_string(),
        ),
        ("projection_indexes_ready", "0".to_string()),
        ("projection_backfill_failed", "0".to_string()),
        ("hourly_rollup_complete", "0".to_string()),
        ("hourly_rollup_max_seq", "0".to_string()),
        (
            "hourly_rollup_history_generation",
            meta_i64(connection, "history_generation")?
                .unwrap_or(0)
                .to_string(),
        ),
        ("hourly_rollup_failed", "0".to_string()),
        ("user_deleted_events", "0".to_string()),
        ("user_deleted_requests", "0".to_string()),
        ("retained_event_count", retained_event_count.to_string()),
    ];
    for (key, value) in defaults {
        connection.execute(
            "INSERT INTO runtime_meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO NOTHING",
            params![key, value],
        )?;
    }
    if unprojected_exists {
        set_meta(connection, "projection_backfill_complete", 0)?;
    }
    connection.execute(
        "INSERT INTO runtime_counters VALUES(1,0,0,0,0,0,0,0) ON CONFLICT(id) DO NOTHING",
        crate::database::params![],
    )?;
    connection.execute(
        "INSERT INTO runtime_retention(
            id,revision,updated_at
         ) VALUES(1,1,?1)
         ON CONFLICT(id) DO NOTHING",
        params![now()],
    )?;
    connection.execute(
        "INSERT INTO runtime_pricing_meta(id,revision,currency,updated_at)
         VALUES(1,1,'USD',?1) ON CONFLICT(id) DO NOTHING",
        params![now()],
    )?;
    if previous_version < SCHEMA_VERSION || !hourly_rollup_meta_exists {
        // Rollup columns and threshold semantics changed in v3. Rebuild all
        // retained buckets from projection columns; never read payload_json
        // for analytics.
        connection.execute(
            "DELETE FROM runtime_hourly_rollups",
            crate::database::params![],
        )?;
        connection.execute(
            "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
             SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
             FROM runtime_events
             WHERE is_in_flight=0 AND kind IN (?1,?2)",
            params![KIND_CLIENT, KIND_UPSTREAM],
        )?;
        let dirty_exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
            crate::database::params![],
            |row| row.get::<_, bool>(0),
        )?;
        set_meta(
            connection,
            "hourly_rollup_complete",
            i64::from(!unprojected_exists && !dirty_exists),
        )?;
    }
    set_meta(connection, "schema_version", SCHEMA_VERSION)?;
    set_meta(connection, "projection_version", PROJECTION_VERSION)?;
    if !unprojected_exists {
        set_meta(connection, "projection_backfill_complete", 1)?;
        if retained_event_count == 0 {
            create_projection_indexes(connection)?;
        }
    }
    Ok(())
}

pub(super) fn create_projection_indexes(connection: &Connection) -> crate::database::Result<()> {
    connection.execute_batch(
        "CREATE INDEX IF NOT EXISTS runtime_events_inflight_change_v2
             ON runtime_events(change_seq) WHERE is_in_flight=1;
         CREATE INDEX IF NOT EXISTS runtime_events_outcome_seq_v2
             ON runtime_events(outcome,seq DESC) WHERE is_in_flight=0;
         CREATE INDEX IF NOT EXISTS runtime_events_time_seq_v2
             ON runtime_events(timestamp DESC,seq DESC) WHERE is_in_flight=0;
         CREATE INDEX IF NOT EXISTS runtime_events_session_time_v2
             ON runtime_events(session_key,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND session_key IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_project_time_v2
             ON runtime_events(project_id,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND project_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_endpoint_time_v2
             ON runtime_events(endpoint_id,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND endpoint_id IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_model_time_v2
             ON runtime_events(effective_model,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND effective_model IS NOT NULL;
         CREATE INDEX IF NOT EXISTS runtime_events_failure_time_v2
             ON runtime_events(failure_kind,failure_phase,timestamp DESC,seq DESC)
             WHERE is_in_flight=0 AND outcome='failed';
         DROP INDEX IF EXISTS runtime_events_seq;
         DROP INDEX IF EXISTS runtime_events_change_seq;
         PRAGMA optimize;",
    )?;
    set_meta(connection, "projection_indexes_ready", 1)
}

pub(super) fn hourly_bucket_start(timestamp: f64) -> i64 {
    (timestamp / 3_600.0).floor() as i64 * 3_600
}

pub(super) fn mark_hourly_rollup_bucket(
    connection: &Connection,
    bucket_start: i64,
) -> crate::database::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start) VALUES(?1)",
        params![bucket_start],
    )?;
    set_meta(connection, "hourly_rollup_complete", 0)
}

pub(super) fn mark_event_hourly_rollup_dirty(
    connection: &Connection,
    event: &RuntimeEvent,
) -> crate::database::Result<()> {
    if event.kind == KIND_CLIENT && !event.is_in_flight() {
        mark_hourly_rollup_bucket(connection, hourly_bucket_start(event.timestamp))?;
    }
    Ok(())
}

pub(super) fn mark_request_hourly_rollups_dirty(
    connection: &Connection,
    request_id: &str,
) -> crate::database::Result<()> {
    connection.execute(
        "INSERT OR IGNORE INTO runtime_hourly_rollup_dirty(bucket_start)
         SELECT DISTINCT (CAST(timestamp AS INTEGER) / 3600) * 3600
         FROM runtime_events
         WHERE request_id=?1 AND kind=?2 AND is_in_flight=0",
        params![request_id, KIND_CLIENT],
    )?;
    set_meta(connection, "hourly_rollup_complete", 0)
}

#[derive(Default)]
pub(super) struct HourlyRollupAccumulator {
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
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
    uncached_input_tokens: i64,
    processed_input_tokens: i64,
    processed_total_tokens: i64,
    usage_present_requests: i64,
    accounting_known_requests: i64,
    accounting_unknown_requests: i64,
    cache_read_reported_requests: i64,
    cache_read_hit_requests: i64,
    cache_eligible_requests: i64,
    cache_unknown_requests: i64,
    cache_read_token_numerator: i64,
    cache_read_token_denominator: i64,
    input_tokens_present: i64,
    output_tokens_present: i64,
    cache_read_input_tokens_present: i64,
    cache_creation_input_tokens_present: i64,
    reasoning_tokens_present: i64,
    cost_accounting_complete_requests: i64,
    cost_unknown_accounting_requests: i64,
    cost_numerator: i128,
    cost_priced_requests: i64,
    cost_unpriced_requests: i64,
    cost_unknown_requests: i64,
}

#[derive(Clone)]
pub(super) struct RollupPrice {
    pub(super) endpoint_id: Option<String>,
    pub(super) model_key: String,
    pub(super) effective_from: f64,
    pub(super) effective_to: Option<f64>,
    pub(super) input_per_million_micros: Option<i64>,
    pub(super) output_per_million_micros: Option<i64>,
    pub(super) cache_read_per_million_micros: Option<i64>,
    pub(super) cache_creation_per_million_micros: Option<i64>,
}

const SCOPED_PRICE_SEPARATOR: char = '\u{1f}';

pub(super) fn split_rollup_price_key(value: String) -> (Option<String>, String) {
    match value.split_once(SCOPED_PRICE_SEPARATOR) {
        Some((endpoint_id, model_key)) if !endpoint_id.is_empty() && !model_key.is_empty() => {
            (Some(endpoint_id.to_owned()), model_key.to_owned())
        }
        _ => (None, value),
    }
}

pub(super) fn load_rollup_prices(
    transaction: &crate::database::Transaction<'_>,
) -> crate::database::Result<(Option<i64>, Vec<RollupPrice>)> {
    let revision = transaction
        .query_row(
            "SELECT revision FROM runtime_pricing_meta WHERE id=1",
            crate::database::params![],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let mut statement = transaction.prepare(
        "SELECT model_key,effective_from,effective_to,input_per_million_micros,
                output_per_million_micros,cache_read_per_million_micros,
                cache_creation_per_million_micros
         FROM runtime_model_prices ORDER BY model_key,effective_from DESC,id DESC",
    )?;
    let prices = statement
        .query_map(crate::database::params![], |row| {
            let (endpoint_id, model_key) = split_rollup_price_key(row.get(0)?);
            Ok(RollupPrice {
                endpoint_id,
                model_key,
                effective_from: row.get(1)?,
                effective_to: row.get(2)?,
                input_per_million_micros: row.get(3)?,
                output_per_million_micros: row.get(4)?,
                cache_read_per_million_micros: row.get(5)?,
                cache_creation_per_million_micros: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok((revision, prices))
}

pub(super) fn resolve_rollup_price<'a>(
    prices: &'a [RollupPrice],
    endpoint_id: Option<&str>,
    model: &str,
    timestamp: f64,
) -> Option<&'a RollupPrice> {
    let matches_timestamp = |price: &&RollupPrice| {
        price.model_key == model
            && timestamp >= price.effective_from
            && price.effective_to.is_none_or(|to| timestamp < to)
    };
    if let Some(endpoint_price) =
        endpoint_id
            .filter(|value| !value.is_empty())
            .and_then(|endpoint| {
                prices.iter().find(|price| {
                    price.endpoint_id.as_deref() == Some(endpoint) && matches_timestamp(price)
                })
            })
    {
        return Some(endpoint_price);
    }
    prices
        .iter()
        .find(|price| price.endpoint_id.is_none() && matches_timestamp(price))
}

// rollup 的一行成本由多个互不相关的维度共同决定;打包成结构体只会多一层
// 只在这里用到的类型。
#[allow(clippy::too_many_arguments)]
pub(super) fn add_rollup_cost(
    accumulator: &mut HourlyRollupAccumulator,
    request_purpose: Option<&str>,
    timestamp: f64,
    endpoint_id: Option<&str>,
    effective_model: Option<&str>,
    uncached_input: Option<i64>,
    cache_read: Option<i64>,
    cache_creation: Option<i64>,
    output: Option<i64>,
    semantics: Option<&str>,
    quality: Option<&str>,
    prices: &[RollupPrice],
) {
    if request_purpose == Some("token_count") {
        return;
    }
    let accounting_known =
        matches!(semantics, Some("subset" | "independent")) && quality == Some("complete");
    let Some((uncached_input, cache_read, cache_creation, output)) = accounting_known
        .then_some((uncached_input, cache_read, cache_creation, output))
        .and_then(|values| match values {
            (Some(uncached), Some(read), Some(creation), Some(output)) => {
                Some((uncached.max(0), read.max(0), creation.max(0), output.max(0)))
            }
            _ => None,
        })
    else {
        accumulator.cost_unknown_accounting_requests = accumulator
            .cost_unknown_accounting_requests
            .saturating_add(1);
        accumulator.cost_unknown_requests = accumulator.cost_unknown_requests.saturating_add(1);
        return;
    };
    let Some(model) = effective_model else {
        accumulator.cost_unpriced_requests = accumulator.cost_unpriced_requests.saturating_add(1);
        return;
    };
    let Some(price) = resolve_rollup_price(prices, endpoint_id, model, timestamp) else {
        accumulator.cost_unpriced_requests = accumulator.cost_unpriced_requests.saturating_add(1);
        return;
    };
    let components = [
        (uncached_input, price.input_per_million_micros),
        (cache_read, price.cache_read_per_million_micros),
        (cache_creation, price.cache_creation_per_million_micros),
        (output, price.output_per_million_micros),
    ];
    let mut request_numerator = 0_i128;
    for (tokens, rate) in components {
        if tokens == 0 {
            continue;
        }
        let Some(rate) = rate else {
            accumulator.cost_unpriced_requests =
                accumulator.cost_unpriced_requests.saturating_add(1);
            return;
        };
        request_numerator =
            request_numerator.saturating_add((tokens as i128).saturating_mul(rate as i128));
    }
    accumulator.cost_numerator = accumulator.cost_numerator.saturating_add(request_numerator);
    accumulator.cost_priced_requests = accumulator.cost_priced_requests.saturating_add(1);
}

pub(super) fn add_optional_token(total: &mut i64, presence: &mut i64, value: Option<i64>) {
    if let Some(value) = value {
        *total = total.saturating_add(value.max(0));
        *presence = presence.saturating_add(1);
    }
}

pub(super) fn rebuild_hourly_rollup_bucket(
    connection: &mut Connection,
    bucket_start: i64,
) -> crate::database::Result<bool> {
    let transaction = connection.transaction()?;
    let bucket_end = bucket_start.saturating_add(3_600);
    let mut accumulator = HourlyRollupAccumulator::default();
    let (price_revision, prices) = load_rollup_prices(&transaction)?;
    {
        let mut statement = transaction.prepare(
            "SELECT seq,kind,outcome,failover,duration_ms,ttfb_ms,request_purpose,usage_present,
                    input_tokens,output_tokens,cache_read_input_tokens,
                    cache_creation_input_tokens,reasoning_tokens,uncached_input_tokens,
                    processed_input_tokens,processed_total_tokens,
                    token_accounting_semantics,token_accounting_quality,endpoint_id,timestamp,effective_model
             FROM runtime_events
             WHERE projection_version=?1 AND is_in_flight=0 AND kind IN ('client','upstream')
               AND timestamp>=?2 AND timestamp<?3
             ORDER BY seq ASC",
        )?;
        let mut rows = statement.query(params![
            PROJECTION_VERSION,
            bucket_start as f64,
            bucket_end as f64
        ])?;
        while let Some(row) = rows.next()? {
            accumulator.max_seq = accumulator.max_seq.max(row.get::<_, i64>(0)?);
            let kind = row.get::<_, String>(1)?;
            if kind == KIND_UPSTREAM {
                accumulator.upstream_attempts = accumulator.upstream_attempts.saturating_add(1);
                match row.get::<_, Option<String>>(2)?.as_deref() {
                    Some("succeeded") => {
                        accumulator.upstream_successes =
                            accumulator.upstream_successes.saturating_add(1)
                    }
                    Some("failed") => {
                        accumulator.upstream_failures =
                            accumulator.upstream_failures.saturating_add(1)
                    }
                    _ => {}
                }
                continue;
            }
            accumulator.client_requests = accumulator.client_requests.saturating_add(1);
            let outcome = row.get::<_, Option<String>>(2)?;
            match outcome.as_deref() {
                Some("succeeded") => {
                    accumulator.client_successes = accumulator.client_successes.saturating_add(1)
                }
                Some("failed") => {
                    accumulator.client_failures = accumulator.client_failures.saturating_add(1)
                }
                Some("cancelled") => {
                    accumulator.client_cancelled = accumulator.client_cancelled.saturating_add(1)
                }
                _ => {
                    accumulator.client_unknown_results =
                        accumulator.client_unknown_results.saturating_add(1)
                }
            }
            let failover = row.get::<_, Option<i64>>(3)?.unwrap_or(0) != 0;
            if failover {
                accumulator.failovers = accumulator.failovers.saturating_add(1);
                if outcome.is_some() {
                    accumulator.failover_terminal_requests =
                        accumulator.failover_terminal_requests.saturating_add(1);
                    if outcome.as_deref() == Some("succeeded") {
                        accumulator.failover_recovered_requests =
                            accumulator.failover_recovered_requests.saturating_add(1);
                    }
                }
            }
            if let Some(value) = row.get::<_, Option<i64>>(4)?.filter(|value| *value >= 0) {
                accumulator.duration_ms_sum = accumulator.duration_ms_sum.saturating_add(value);
                accumulator.duration_count = accumulator.duration_count.saturating_add(1);
                if value > DURATION_SLOW_MS {
                    accumulator.duration_slow_count =
                        accumulator.duration_slow_count.saturating_add(1);
                }
                if value > DURATION_CRITICAL_MS {
                    accumulator.duration_critical_count =
                        accumulator.duration_critical_count.saturating_add(1);
                }
            }
            if let Some(value) = row.get::<_, Option<i64>>(5)?.filter(|value| *value >= 0) {
                accumulator.ttfb_ms_sum = accumulator.ttfb_ms_sum.saturating_add(value);
                accumulator.ttfb_count = accumulator.ttfb_count.saturating_add(1);
                if value > TTFB_SLOW_MS {
                    accumulator.ttfb_slow_count = accumulator.ttfb_slow_count.saturating_add(1);
                }
                if value > TTFB_CRITICAL_MS {
                    accumulator.ttfb_critical_count =
                        accumulator.ttfb_critical_count.saturating_add(1);
                }
            }
            let request_purpose = row.get::<_, Option<String>>(6)?;
            let usage_present = row.get::<_, Option<i64>>(7)?.unwrap_or(0) != 0;
            let input = row.get::<_, Option<i64>>(8)?;
            let output = row.get::<_, Option<i64>>(9)?;
            let cache_read = row.get::<_, Option<i64>>(10)?;
            let cache_creation = row.get::<_, Option<i64>>(11)?;
            let reasoning = row.get::<_, Option<i64>>(12)?;
            let uncached = row.get::<_, Option<i64>>(13)?;
            let processed_input = row.get::<_, Option<i64>>(14)?;
            let processed_total = row.get::<_, Option<i64>>(15)?;
            let semantics = row.get::<_, Option<String>>(16)?;
            let quality = row.get::<_, Option<String>>(17)?;
            let endpoint_id = row.get::<_, Option<String>>(18)?;
            let event_timestamp = row.get::<_, f64>(19)?;
            let effective_model = row.get::<_, Option<String>>(20)?;
            add_rollup_cost(
                &mut accumulator,
                request_purpose.as_deref(),
                event_timestamp,
                endpoint_id.as_deref(),
                effective_model.as_deref(),
                uncached,
                cache_read,
                cache_creation,
                output,
                semantics.as_deref(),
                quality.as_deref(),
                &prices,
            );
            if request_purpose.as_deref() == Some("token_count") || !usage_present {
                continue;
            }
            accumulator.usage_present_requests =
                accumulator.usage_present_requests.saturating_add(1);
            add_optional_token(
                &mut accumulator.input_tokens,
                &mut accumulator.input_tokens_present,
                input,
            );
            add_optional_token(
                &mut accumulator.output_tokens,
                &mut accumulator.output_tokens_present,
                output,
            );
            add_optional_token(
                &mut accumulator.cache_read_input_tokens,
                &mut accumulator.cache_read_input_tokens_present,
                cache_read,
            );
            add_optional_token(
                &mut accumulator.cache_creation_input_tokens,
                &mut accumulator.cache_creation_input_tokens_present,
                cache_creation,
            );
            add_optional_token(
                &mut accumulator.reasoning_tokens,
                &mut accumulator.reasoning_tokens_present,
                reasoning,
            );
            if let Some(value) = uncached {
                accumulator.uncached_input_tokens = accumulator
                    .uncached_input_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = processed_input {
                accumulator.processed_input_tokens = accumulator
                    .processed_input_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = processed_total {
                accumulator.processed_total_tokens = accumulator
                    .processed_total_tokens
                    .saturating_add(value.max(0));
            }
            if let Some(value) = cache_read {
                accumulator.cache_read_reported_requests =
                    accumulator.cache_read_reported_requests.saturating_add(1);
                if value > 0 {
                    accumulator.cache_read_hit_requests =
                        accumulator.cache_read_hit_requests.saturating_add(1);
                }
            }
            let accounting_known = matches!(semantics.as_deref(), Some("subset" | "independent"));
            if accounting_known {
                accumulator.accounting_known_requests =
                    accumulator.accounting_known_requests.saturating_add(1);
            } else {
                accumulator.accounting_unknown_requests =
                    accumulator.accounting_unknown_requests.saturating_add(1);
            }
            if accounting_known {
                if let (Some(cache_read), Some(processed_input)) = (cache_read, processed_input) {
                    accumulator.cache_eligible_requests =
                        accumulator.cache_eligible_requests.saturating_add(1);
                    accumulator.cache_read_token_numerator = accumulator
                        .cache_read_token_numerator
                        .saturating_add(cache_read.max(0));
                    accumulator.cache_read_token_denominator = accumulator
                        .cache_read_token_denominator
                        .saturating_add(processed_input.max(0));
                } else {
                    accumulator.cache_unknown_requests =
                        accumulator.cache_unknown_requests.saturating_add(1);
                }
            } else {
                accumulator.cache_unknown_requests =
                    accumulator.cache_unknown_requests.saturating_add(1);
            }
            let cost_complete = accounting_known
                && quality.as_deref() == Some("complete")
                && uncached.is_some()
                && cache_read.is_some()
                && cache_creation.is_some()
                && output.is_some();
            if cost_complete {
                accumulator.cost_accounting_complete_requests = accumulator
                    .cost_accounting_complete_requests
                    .saturating_add(1);
            } else {
                accumulator.cost_unknown_accounting_requests = accumulator
                    .cost_unknown_accounting_requests
                    .saturating_add(1);
            }
        }
    }
    if accumulator.client_requests == 0 {
        transaction.execute(
            "DELETE FROM runtime_hourly_rollups WHERE bucket_start=?1",
            params![bucket_start],
        )?;
    } else {
        transaction.execute(
            "INSERT OR REPLACE INTO runtime_hourly_rollups(
                bucket_start,bucket_end,max_seq,client_requests,client_successes,
                client_failures,client_cancelled,client_unknown_results,failovers,
                failover_terminal_requests,failover_recovered_requests,upstream_attempts,
                upstream_successes,upstream_failures,duration_ms_sum,duration_count,
                duration_slow_count,duration_critical_count,ttfb_ms_sum,ttfb_count,
                ttfb_slow_count,ttfb_critical_count,input_tokens,output_tokens,
                cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,
                uncached_input_tokens,processed_input_tokens,processed_total_tokens,
                usage_present_requests,accounting_known_requests,accounting_unknown_requests,
                cache_read_reported_requests,cache_read_hit_requests,cache_eligible_requests,
                cache_unknown_requests,cache_read_token_numerator,cache_read_token_denominator,
                input_tokens_present,output_tokens_present,cache_read_input_tokens_present,
                cache_creation_input_tokens_present,reasoning_tokens_present,
                cost_accounting_complete_requests,cost_unknown_accounting_requests,
                cost_numerator,cost_priced_requests,cost_unpriced_requests,cost_unknown_requests,
                cost_price_revision,updated_at
             ) VALUES(
                ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,
                ?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,
                ?33,?34,?35,?36,?37,?38,?39,?40,?41,?42,?43,?44,?45,?46,?47,
                ?48,?49,?50,?51,?52)",
            params![
                bucket_start,
                bucket_end,
                accumulator.max_seq,
                accumulator.client_requests,
                accumulator.client_successes,
                accumulator.client_failures,
                accumulator.client_cancelled,
                accumulator.client_unknown_results,
                accumulator.failovers,
                accumulator.failover_terminal_requests,
                accumulator.failover_recovered_requests,
                accumulator.upstream_attempts,
                accumulator.upstream_successes,
                accumulator.upstream_failures,
                accumulator.duration_ms_sum,
                accumulator.duration_count,
                accumulator.duration_slow_count,
                accumulator.duration_critical_count,
                accumulator.ttfb_ms_sum,
                accumulator.ttfb_count,
                accumulator.ttfb_slow_count,
                accumulator.ttfb_critical_count,
                accumulator.input_tokens,
                accumulator.output_tokens,
                accumulator.cache_read_input_tokens,
                accumulator.cache_creation_input_tokens,
                accumulator.reasoning_tokens,
                accumulator.uncached_input_tokens,
                accumulator.processed_input_tokens,
                accumulator.processed_total_tokens,
                accumulator.usage_present_requests,
                accumulator.accounting_known_requests,
                accumulator.accounting_unknown_requests,
                accumulator.cache_read_reported_requests,
                accumulator.cache_read_hit_requests,
                accumulator.cache_eligible_requests,
                accumulator.cache_unknown_requests,
                accumulator.cache_read_token_numerator,
                accumulator.cache_read_token_denominator,
                accumulator.input_tokens_present,
                accumulator.output_tokens_present,
                accumulator.cache_read_input_tokens_present,
                accumulator.cache_creation_input_tokens_present,
                accumulator.reasoning_tokens_present,
                accumulator.cost_accounting_complete_requests,
                accumulator.cost_unknown_accounting_requests,
                accumulator
                    .cost_numerator
                    .clamp(i64::MIN as i128, i64::MAX as i128) as i64,
                accumulator.cost_priced_requests,
                accumulator.cost_unpriced_requests,
                accumulator.cost_unknown_requests,
                price_revision,
                now(),
            ],
        )?;
    }
    transaction.execute(
        "DELETE FROM runtime_hourly_rollup_dirty WHERE bucket_start=?1",
        params![bucket_start],
    )?;
    let dirty_exists = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
        crate::database::params![],
        |row| row.get::<_, bool>(0),
    )?;
    if !dirty_exists && meta_i64(&transaction, "projection_backfill_complete")?.unwrap_or(0) == 1 {
        let max_seq = transaction.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM runtime_events WHERE is_in_flight=0",
            crate::database::params![],
            |row| row.get::<_, i64>(0),
        )?;
        set_meta(&transaction, "hourly_rollup_max_seq", max_seq)?;
        set_meta(
            &transaction,
            "hourly_rollup_history_generation",
            meta_i64(&transaction, "history_generation")?.unwrap_or(0),
        )?;
        set_meta(&transaction, "hourly_rollup_complete", 1)?;
    }
    transaction.commit()?;
    Ok(dirty_exists)
}

pub(super) fn projection_maintenance_needed(
    connection: &Connection,
) -> crate::database::Result<bool> {
    let rollup_dirty = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_hourly_rollup_dirty LIMIT 1)",
        crate::database::params![],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(
        meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) == 0
            || meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) == 0
            || meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) == 0
            || rollup_dirty,
    )
}

pub(super) fn backfill_projection_batch(
    connection: &mut Connection,
) -> crate::database::Result<bool> {
    let transaction = connection.transaction()?;
    let rows = {
        let mut statement = transaction.prepare(
            "SELECT seq,payload_json FROM runtime_events
             WHERE projection_version>=0 AND projection_version<>?1
             ORDER BY seq ASC LIMIT ?2",
        )?;
        statement
            .query_map(
                params![PROJECTION_VERSION, PROJECTION_BACKFILL_BATCH as i64],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };
    if rows.is_empty() {
        set_meta(&transaction, "projection_backfill_complete", 1)?;
        transaction.commit()?;
        return Ok(false);
    }
    let mut cursor = 0;
    let mut failed = 0i64;
    for (seq, payload) in &rows {
        cursor = cursor.max(*seq);
        match serde_json::from_str::<RuntimeEvent>(payload) {
            Ok(event) => {
                update_event_projection(&transaction, *seq, &event, payload)?;
                mark_event_hourly_rollup_dirty(&transaction, &event)?;
            }
            Err(_) => {
                failed += 1;
                transaction.execute(
                    "UPDATE runtime_events SET projection_version=-1,payload_bytes=?2 WHERE seq=?1",
                    params![seq, payload.len().min(i64::MAX as usize) as i64],
                )?;
            }
        }
    }
    set_meta(&transaction, "projection_backfill_cursor", cursor)?;
    if failed > 0 {
        let total = meta_i64(&transaction, "projection_backfill_failed")?
            .unwrap_or(0)
            .saturating_add(failed);
        set_meta(&transaction, "projection_backfill_failed", total)?;
    }
    let more = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events
                       WHERE projection_version>=0 AND projection_version<>?1 LIMIT 1)",
        params![PROJECTION_VERSION],
        |row| row.get::<_, bool>(0),
    )?;
    if !more {
        set_meta(&transaction, "projection_backfill_complete", 1)?;
    }
    transaction.commit()?;
    Ok(more)
}

pub(super) fn advance_projection_indexes(connection: &Connection) -> crate::database::Result<bool> {
    let indexes = [
        (
            "runtime_events_inflight_change_v2",
            "CREATE INDEX runtime_events_inflight_change_v2 ON runtime_events(change_seq) WHERE is_in_flight=1",
        ),
        (
            "runtime_events_outcome_seq_v2",
            "CREATE INDEX runtime_events_outcome_seq_v2 ON runtime_events(outcome,seq DESC) WHERE is_in_flight=0",
        ),
        (
            "runtime_events_time_seq_v2",
            "CREATE INDEX runtime_events_time_seq_v2 ON runtime_events(timestamp DESC,seq DESC) WHERE is_in_flight=0",
        ),
        (
            "runtime_events_session_time_v2",
            "CREATE INDEX runtime_events_session_time_v2 ON runtime_events(session_key,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND session_key IS NOT NULL",
        ),
        (
            "runtime_events_project_time_v2",
            "CREATE INDEX runtime_events_project_time_v2 ON runtime_events(project_id,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND project_id IS NOT NULL",
        ),
        (
            "runtime_events_endpoint_time_v2",
            "CREATE INDEX runtime_events_endpoint_time_v2 ON runtime_events(endpoint_id,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND endpoint_id IS NOT NULL",
        ),
        (
            "runtime_events_model_time_v2",
            "CREATE INDEX runtime_events_model_time_v2 ON runtime_events(effective_model,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND effective_model IS NOT NULL",
        ),
        (
            "runtime_events_failure_time_v2",
            "CREATE INDEX runtime_events_failure_time_v2 ON runtime_events(failure_kind,failure_phase,timestamp DESC,seq DESC) WHERE is_in_flight=0 AND outcome='failed'",
        ),
    ];
    for (name, sql) in indexes {
        let exists = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='index' AND name=?1)",
            params![name],
            |row| row.get::<_, bool>(0),
        )?;
        if !exists {
            connection.execute(sql, crate::database::params![])?;
            return Ok(true);
        }
    }
    connection.execute_batch(
        "DROP INDEX IF EXISTS runtime_events_seq;
         DROP INDEX IF EXISTS runtime_events_change_seq;
         PRAGMA optimize;",
    )?;
    set_meta(connection, "projection_indexes_ready", 1)?;
    Ok(false)
}

pub(super) fn run_projection_maintenance(
    connection: &mut Connection,
) -> crate::database::Result<bool> {
    if meta_i64(connection, "projection_backfill_complete")?.unwrap_or(0) == 0 {
        let more = backfill_projection_batch(connection)?;
        if more {
            return Ok(true);
        }
    }
    if meta_i64(connection, "projection_indexes_ready")?.unwrap_or(0) == 0 {
        let more = advance_projection_indexes(connection)?;
        if more {
            return Ok(true);
        }
    }
    let dirty_bucket = connection
        .query_row(
            "SELECT bucket_start FROM runtime_hourly_rollup_dirty ORDER BY bucket_start ASC LIMIT 1", crate::database::params![],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(bucket_start) = dirty_bucket {
        return rebuild_hourly_rollup_bucket(connection, bucket_start);
    }
    if meta_i64(connection, "hourly_rollup_complete")?.unwrap_or(0) == 0 {
        let max_seq = connection.query_row(
            "SELECT COALESCE(MAX(seq),0) FROM runtime_events WHERE is_in_flight=0",
            crate::database::params![],
            |row| row.get::<_, i64>(0),
        )?;
        set_meta(connection, "hourly_rollup_max_seq", max_seq)?;
        set_meta(
            connection,
            "hourly_rollup_history_generation",
            meta_i64(connection, "history_generation")?.unwrap_or(0),
        )?;
        set_meta(connection, "hourly_rollup_complete", 1)?;
    }
    Ok(false)
}

pub(super) fn database_issue_on(
    connection: &Connection,
) -> Result<Option<super::RuntimeDatabaseIssue>, String> {
    let inspect = || -> crate::database::Result<Option<super::RuntimeDatabaseIssue>> {
        let tables = connection
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'runtime_%'")?
            .query_map(crate::database::params![], |row| row.get::<_, String>(0))?
            .collect::<Result<HashSet<_>, _>>()?;
        if tables.is_empty() {
            return Ok(None);
        }
        let version = if tables.contains("runtime_meta") {
            meta_i64(connection, "schema_version")?.unwrap_or(0)
        } else {
            0
        };
        let projection = if tables.contains("runtime_meta") {
            meta_i64(connection, "projection_version")?
        } else {
            None
        };
        let newer =
            version > SCHEMA_VERSION || projection.is_some_and(|value| value > PROJECTION_VERSION);
        let mut incompatible = version != SCHEMA_VERSION
            || projection != Some(PROJECTION_VERSION)
            || !tables.contains("runtime_events");
        if !incompatible && tables.contains("runtime_events") {
            let columns = connection
                .prepare("PRAGMA table_info(runtime_events)")?
                .query_map(crate::database::params![], |row| row.get::<_, String>(1))?
                .collect::<Result<HashSet<_>, _>>()?;
            incompatible = [
                "projection_version",
                "client_variant",
                "agent_role",
                "agent_name",
                "parent_thread_id",
                "parent_turn_id",
                "root_turn_id",
                "cache_read_state",
                "cache_read_finality",
                "cache_read_reason",
                "hook_event",
            ]
            .iter()
            .any(|column| !columns.contains(*column));
        }
        if !incompatible {
            incompatible = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE projection_version != ?1 LIMIT 1)",
                [PROJECTION_VERSION], |row| row.get::<_, bool>(0),
            )?;
        }
        if !incompatible && !newer {
            return Ok(None);
        }
        Ok(Some(super::RuntimeDatabaseIssue {
            code: if newer {
                "runtime_schema_newer"
            } else {
                "runtime_recreate_required"
            },
            schema_version: version,
            supported_schema_version: SCHEMA_VERSION,
            projection_version: projection,
            supported_projection_version: PROJECTION_VERSION,
            requires_recreate: !newer,
            message: if newer {
                format!(
                    "runtime schema {version} / projection {projection:?} is newer than supported {SCHEMA_VERSION}/{PROJECTION_VERSION}; 请升级 Sumpter"
                )
            } else {
                format!(
                    "检测到旧版 runtime 数据库（schema {version}），代理已停止。请在统计页确认清空并重建；旧事件不迁移。"
                )
            },
        }))
    };
    inspect().map_err(|error| error.to_string())
}
