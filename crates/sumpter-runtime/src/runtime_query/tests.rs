use super::MAX_TREND_POINTS;
use super::export_queries::stream_export_on;
use crate::runtime_query::*;
use rusqlite::{Connection, params};

fn test_connection() -> Connection {
    let connection = Connection::open_in_memory().expect("open in-memory runtime db");
    connection
            .execute_batch(
                "PRAGMA journal_mode=MEMORY;
                 PRAGMA synchronous=OFF;
                 CREATE TABLE runtime_meta(key TEXT PRIMARY KEY,value TEXT NOT NULL);
                 INSERT INTO runtime_meta(key,value) VALUES
                   ('schema_version','3'),('reset_generation','0'),
                   ('history_generation','0'),('retained_from_seq','1'),
                   ('projection_backfill_cursor','100000'),
                   ('projection_backfill_complete','1'),('projection_indexes_ready','1'),
                   ('user_deleted_events','0'),('user_deleted_requests','0');
                 CREATE TABLE runtime_events(
                   seq INTEGER PRIMARY KEY,change_seq INTEGER NOT NULL UNIQUE,
                   event_id TEXT NOT NULL UNIQUE,request_id TEXT,timestamp REAL NOT NULL,
                   kind TEXT NOT NULL,phase TEXT,outcome TEXT,status_code INTEGER NOT NULL,
                   client_kind TEXT,request_purpose TEXT,endpoint_id TEXT,failure_kind TEXT,
                   is_in_flight INTEGER NOT NULL,payload_json TEXT NOT NULL,
                   projection_version INTEGER NOT NULL DEFAULT 0,
                   payload_bytes INTEGER NOT NULL DEFAULT 0,session_key TEXT,session_source TEXT,
                   project_id TEXT,project_name TEXT,project_source TEXT,local_user TEXT,workspace_paths_json TEXT,
                   endpoint_name TEXT,pool_id TEXT,feature_rule_id TEXT,client_model TEXT,
                   effective_model TEXT,upstream_model TEXT,failure_phase TEXT,
                   source_format TEXT,target_format TEXT,route_mode TEXT,
                   upstream_status_code INTEGER,duration_ms INTEGER,ttfb_ms INTEGER,
                   failover INTEGER,stream_terminal TEXT,codex_metadata_present INTEGER,
                   usage_present INTEGER,input_tokens INTEGER,output_tokens INTEGER,
                   cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER,
                   reasoning_tokens INTEGER,uncached_input_tokens INTEGER,
                   processed_input_tokens INTEGER,processed_total_tokens INTEGER,
                   token_accounting_semantics TEXT,token_accounting_quality TEXT,
                   tool_calls_json TEXT,
                   codex_thread_class TEXT,attribution_scope TEXT,
                   request_method TEXT,request_path TEXT,route_intent TEXT,
                   model_group_id TEXT,model_group_name TEXT
                 );
                 CREATE INDEX runtime_events_kind_seq ON runtime_events(kind,seq DESC);
                 CREATE INDEX runtime_events_request_id ON runtime_events(request_id);
                 CREATE INDEX runtime_events_timestamp ON runtime_events(timestamp);
                 CREATE INDEX runtime_events_inflight_change_v2 ON runtime_events(change_seq)
                   WHERE is_in_flight=1;
                 CREATE INDEX runtime_events_outcome_seq_v2 ON runtime_events(outcome,seq DESC)
                   WHERE is_in_flight=0;
                 CREATE INDEX runtime_events_time_seq_v2 ON runtime_events(timestamp DESC,seq DESC)
                   WHERE is_in_flight=0;
                 CREATE INDEX runtime_events_session_time_v2
                   ON runtime_events(session_key,timestamp DESC,seq DESC)
                   WHERE is_in_flight=0 AND session_key IS NOT NULL;
                 CREATE INDEX runtime_events_project_time_v2
                   ON runtime_events(project_id,timestamp DESC,seq DESC)
                   WHERE is_in_flight=0 AND project_id IS NOT NULL;
                 CREATE INDEX runtime_events_endpoint_time_v2
                   ON runtime_events(endpoint_id,timestamp DESC,seq DESC)
                   WHERE is_in_flight=0 AND endpoint_id IS NOT NULL;
                 CREATE INDEX runtime_events_model_time_v2
                   ON runtime_events(effective_model,timestamp DESC,seq DESC)
                   WHERE is_in_flight=0 AND effective_model IS NOT NULL;
                 CREATE INDEX runtime_events_failure_time_v2
                   ON runtime_events(failure_kind,failure_phase,timestamp DESC,seq DESC)
                   WHERE is_in_flight=0 AND outcome='failed';
                 CREATE TABLE runtime_hourly_rollups(
                   bucket_start INTEGER PRIMARY KEY,
                   client_requests INTEGER NOT NULL DEFAULT 0,
                   client_successes INTEGER NOT NULL DEFAULT 0,
                   client_failures INTEGER NOT NULL DEFAULT 0,
                   client_cancelled INTEGER NOT NULL DEFAULT 0,
                   failovers INTEGER NOT NULL DEFAULT 0,
                   duration_ms_sum INTEGER NOT NULL DEFAULT 0,duration_count INTEGER NOT NULL DEFAULT 0,
                   ttfb_ms_sum INTEGER NOT NULL DEFAULT 0,ttfb_count INTEGER NOT NULL DEFAULT 0,
                   input_tokens INTEGER NOT NULL DEFAULT 0,output_tokens INTEGER NOT NULL DEFAULT 0,
                   cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                   cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                   reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                   uncached_input_tokens INTEGER NOT NULL DEFAULT 0,
                   processed_input_tokens INTEGER NOT NULL DEFAULT 0,
                   processed_total_tokens INTEGER NOT NULL DEFAULT 0,
                   usage_present_requests INTEGER NOT NULL DEFAULT 0,
                   cache_eligible_requests INTEGER NOT NULL DEFAULT 0,
                   cache_unknown_requests INTEGER NOT NULL DEFAULT 0
                 );
                 CREATE TABLE runtime_hourly_rollup_dirty(bucket_start INTEGER PRIMARY KEY);
                 CREATE TABLE runtime_retention(
                   id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL DEFAULT 1,
                   updated_at REAL NOT NULL
                 );
                 INSERT INTO runtime_retention VALUES(1,1,0);
                 CREATE TABLE runtime_pricing_meta(
                   id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL DEFAULT 1,
                   currency TEXT NOT NULL DEFAULT 'USD',updated_at REAL NOT NULL
                 );
                 INSERT INTO runtime_pricing_meta VALUES(1,7,'USD',0);
                 CREATE TABLE runtime_model_prices(
                   id INTEGER PRIMARY KEY,model_key TEXT NOT NULL,effective_from REAL NOT NULL,
                   effective_to REAL,input_per_million_micros INTEGER,
                   output_per_million_micros INTEGER,cache_read_per_million_micros INTEGER,
                   cache_creation_per_million_micros INTEGER,created_at REAL NOT NULL,
                   updated_at REAL NOT NULL,UNIQUE(model_key,effective_from)
                 );
                 INSERT INTO runtime_model_prices VALUES
                   (1,'gpt-test',0,NULL,1000000,2000000,100000,500000,0,0);",
            )
            .expect("create runtime query schema");
    connection
}

// 测试夹具:直接铺一行完整事件,参数就是表的列。
#[allow(clippy::too_many_arguments)]
fn insert_endpoint_test_event(
    connection: &Connection,
    seq: i64,
    kind: &str,
    request_id: Option<&str>,
    outcome: &str,
    status_code: i64,
    client_kind: Option<&str>,
    request_purpose: Option<&str>,
    endpoint_id: Option<&str>,
    endpoint_name: Option<&str>,
) {
    connection
        .execute(
            "INSERT INTO runtime_events(
                   seq,change_seq,event_id,request_id,timestamp,kind,phase,outcome,status_code,
                   client_kind,request_purpose,endpoint_id,is_in_flight,payload_json,
                   projection_version,endpoint_name
                 ) VALUES (?1,?1,?2,?3,1.0,?4,'completed',?5,?6,?7,?8,?9,0,'{}',8,?10)",
            params![
                seq,
                format!("endpoint-test-{seq}"),
                request_id,
                kind,
                outcome,
                status_code,
                client_kind,
                request_purpose,
                endpoint_id,
                endpoint_name,
            ],
        )
        .expect("insert endpoint test event");
}

fn insert_100k(connection: &Connection) {
    connection
            .execute_batch(
                "BEGIN;
                 WITH RECURSIVE rows(seq) AS (
                   VALUES(1) UNION ALL SELECT seq+1 FROM rows WHERE seq<100000
                 )
                 INSERT INTO runtime_events(
                   seq,change_seq,event_id,request_id,timestamp,kind,phase,outcome,status_code,
                   client_kind,request_purpose,endpoint_id,failure_kind,is_in_flight,payload_json,
                   projection_version,payload_bytes,session_key,session_source,project_id,
                   project_name,project_source,workspace_paths_json,endpoint_name,effective_model,
                   failure_phase,source_format,target_format,route_mode,upstream_status_code,
                   duration_ms,ttfb_ms,failover,usage_present,input_tokens,output_tokens,
                   cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,
                   uncached_input_tokens,processed_input_tokens,processed_total_tokens,
                   token_accounting_semantics,token_accounting_quality
                 )
                 SELECT seq,seq,printf('event-%06d',seq),printf('request-%06d',seq),seq*60.0,
                   'client','completed',
                   CASE WHEN seq%10=0 THEN 'failed' WHEN seq%17=0 THEN 'cancelled' ELSE 'succeeded' END,
                   CASE WHEN seq%10=0 THEN 500 WHEN seq%17=0 THEN 499 ELSE 200 END,
                   CASE WHEN seq%2=0 THEN 'codex' ELSE 'claude_code' END,'completion',
                   'endpoint-a',CASE WHEN seq%10=0 THEN 'upstream_http_status' END,0,
                   CASE WHEN seq>99975 THEN '{\"id\":\"page-event\",\"kind\":\"client\"}'
                        ELSE '{not-json' END,
                   8,80,printf('session-%03d',seq%500),'header',printf('project-%03d',seq%100),
                   printf('Project %03d',seq%100),'workspace_local','[\".../projects/test\"]',
                   'Endpoint A','gpt-test',CASE WHEN seq%10=0 THEN 'response' END,
                   'openai-responses','openai-responses','native',
                   CASE WHEN seq%10=0 THEN 500 END,100+(seq%100),50+(seq%50),seq%7=0,
                   1,100,20,CASE WHEN seq%2=0 THEN 20 ELSE 0 END,0,5,
                   CASE WHEN seq%2=0 THEN 80 ELSE 100 END,100,120,'subset','complete'
                 FROM rows;
                 COMMIT;",
            )
            .expect("insert 100k projected rows");
}

#[test]
fn projected_queries_are_bounded_on_100k_rows() {
    let mut connection = test_connection();
    insert_100k(&connection);

    let events = events_page_on(
        &mut connection,
        &EventPageRequest {
            page: 1,
            page_size: 25,
            ..EventPageRequest::default()
        },
    )
    .expect("query exact event page");
    assert_eq!(events.total_count, 100_000);
    assert_eq!(events.events.len(), 25);
    assert_eq!(events.total_pages, 4_000);
    let newest = &events.events[0];
    assert_eq!(newest.phase, Some(RuntimeEventPhase::Completed));
    assert_eq!(newest.outcome, Some(RuntimeEventOutcome::Failed));
    assert_eq!(newest.client_kind, Some(ClientKind::Codex));
    assert_eq!(
        newest.source_format,
        Some(ProviderProtocol::OpenAIResponses)
    );
    assert_eq!(
        newest.target_format,
        Some(ProviderProtocol::OpenAIResponses)
    );
    assert_eq!(newest.route_mode, Some(RouteMode::Native));

    let trends = trends_on(
        &mut connection,
        &TrendRequest {
            from: 0.0,
            to: 6_000_000.0,
            granularity: TrendGranularity::Auto,
            snapshot_seq: None,
            history_generation: None,
            filter: RuntimeFilter::default(),
        },
    )
    .expect("query projected trends without payload JSON");
    assert!(trends.points.len() <= MAX_TREND_POINTS);
    assert_eq!(trends.totals.client_requests, 100_000);
    assert!((trends.totals.tokens.cache_read_token_rate.unwrap() - 0.1).abs() < 0.000_001);
    assert!((trends.totals.tokens.cache_read_request_rate.unwrap() - 0.5).abs() < 0.000_001);
    assert_eq!(trends.totals.cost.priced_requests, 100_000);
    assert!(trends.totals.cost.complete);

    let errors = error_groups_on(&mut connection, &ErrorPageQuery::default())
        .expect("aggregate projected errors without payload JSON");
    assert_eq!(errors.total_count, 1);
    assert_eq!(errors.groups[0].occurrences, 10_000);

    let projects = dimension_page_on(
        &mut connection,
        DimensionKind::Project,
        &DimensionPageQuery {
            page_size: 25,
            sort: DimensionSort::Name,
            order: SortOrder::Asc,
            ..DimensionPageQuery::default()
        },
    )
    .expect("paginate projected projects without payload JSON");
    assert_eq!(projects.total_count, 100);
    assert_eq!(projects.rows.len(), 25);
    assert_eq!(projects.rows[0].cache_read_reported_requests, 1_000);
    assert_eq!(projects.rows[0].key, "project-000");
    assert_eq!(projects.rows[0].cache_read_hit_requests, 1_000);
    assert_eq!(projects.rows[0].cache_read_token_eligible_requests, 1_000);
    assert_eq!(projects.rows[0].cache_read_token_unknown_requests, 0);
    assert_eq!(projects.rows[0].cache_read_token_rate, Some(0.2));
    assert_eq!(projects.rows[0].cache_read_request_rate, Some(1.0));

    let analytics = analytics_on(&mut connection, "all", &RuntimeFilter::default())
        .expect("aggregate legacy analytics dimensions without payload JSON");
    let endpoint = analytics
        .endpoints
        .iter()
        .find(|row| row.name == "Endpoint A")
        .expect("endpoint dimension");
    assert_eq!(endpoint.cache_read_token_rate, Some(0.1));
    assert_eq!(endpoint.cache_read_request_rate, Some(0.5));

    let sessions = dimension_page_on(
        &mut connection,
        DimensionKind::Session,
        &DimensionPageQuery {
            page_size: 25,
            ..DimensionPageQuery::default()
        },
    )
    .expect("paginate projected sessions without payload JSON");
    assert_eq!(sessions.total_count, 500);
    assert_eq!(sessions.rows.len(), 25);

    let storage = storage_details_on(&mut connection, None).expect("probe runtime storage");
    assert_eq!(storage.retained_events, 100_000);
    assert!(storage.projection_indexes_ready);
}

#[test]
fn facets_apply_common_range_and_outcome_filters() {
    let mut connection = test_connection();
    insert_100k(&connection);

    let facets = facets_on(
        &mut connection,
        &RuntimeFilter {
            from: Some(99_991.0 * 60.0),
            to: Some(100_000.0 * 60.0),
            outcome: Some("failed".into()),
            ..RuntimeFilter::default()
        },
    )
    .expect("query facets from the projected range");

    // Only event 100000 is failed in the selected ten-row window. Every
    // facet still reports that row, while its own selected dimension is
    // intentionally ignored by the picker semantics.
    assert_eq!(facets.facets.client_kinds[0].count, 1);
    assert_eq!(facets.facets.endpoints[0].count, 1);
    assert_eq!(facets.facets.projects[0].count, 1);
    assert_eq!(facets.facets.sessions[0].count, 1);
}

#[test]
fn today_range_prefers_explicit_local_midnight_but_relative_ranges_stay_bounded() {
    let utc_midnight = 900_000_000.0;
    let local_midnight = utc_midnight - 8.0 * 3_600.0;
    assert_eq!(
        merge_range_lower_bound("today", Some(local_midnight), Some(utc_midnight)),
        Some(local_midnight)
    );
    assert_eq!(
        merge_range_lower_bound("today", None, Some(utc_midnight)),
        Some(utc_midnight)
    );
    assert_eq!(
        merge_range_lower_bound(
            "24h",
            Some(utc_midnight - 30.0 * 86_400.0),
            Some(utc_midnight)
        ),
        Some(utc_midnight)
    );
}

#[test]
fn pre_routing_rejections_are_not_endpoint_usage() {
    let mut connection = test_connection();
    insert_endpoint_test_event(
        &connection,
        1,
        "client",
        Some("request-auth"),
        "failed",
        401,
        Some("codex"),
        Some("completion"),
        None,
        None,
    );
    insert_endpoint_test_event(
        &connection,
        2,
        "upstream",
        Some("request-ok"),
        "succeeded",
        200,
        None,
        None,
        Some("endpoint-a"),
        Some("Endpoint A"),
    );
    insert_endpoint_test_event(
        &connection,
        3,
        "client",
        Some("request-ok"),
        "succeeded",
        200,
        Some("codex"),
        Some("completion"),
        Some("endpoint-a"),
        Some("Endpoint A"),
    );

    let analytics =
        analytics_on(&mut connection, "all", &RuntimeFilter::default()).expect("analytics");
    assert_eq!(analytics.client_requests, 2);
    assert_eq!(analytics.client_failures, 1);
    assert_eq!(analytics.endpoints.len(), 1);
    assert_eq!(analytics.endpoints[0].name, "Endpoint A");

    let page = dimension_page_on(
        &mut connection,
        DimensionKind::Endpoint,
        &DimensionPageQuery::default(),
    )
    .expect("endpoint dimension page");
    assert_eq!(page.total_count, 1);
    assert_eq!(page.rows[0].key, "endpoint-a");
    assert!(!page.rows.iter().any(|row| row.name.contains("unassigned")));

    let facets = facets_on(&mut connection, &RuntimeFilter::default()).expect("facets");
    assert_eq!(facets.facets.endpoints.len(), 1);
    assert_eq!(facets.facets.endpoints[0].value, "endpoint-a");
    assert_eq!(facets.facets.endpoints[0].count, 1);
}

#[test]
fn upstream_only_facets_keep_selected_placeholders() {
    let mut connection = test_connection();
    let facets = facets_on(
        &mut connection,
        &RuntimeFilter {
            kind: Some("upstream".into()),
            endpoint_id: Some("endpoint-selected".into()),
            session_id: Some("session-selected".into()),
            ..RuntimeFilter::default()
        },
    )
    .expect("query empty upstream facet picker");
    assert_eq!(facets.facets.endpoints[0].value, "endpoint-selected");
    assert_eq!(facets.facets.endpoints[0].count, 0);
    assert_eq!(facets.facets.sessions[0].value, "session-selected");
    assert_eq!(facets.facets.sessions[0].count, 0);
}

#[test]
fn event_page_defaults_to_ten_rows() {
    assert_eq!(EventPageRequest::default().page_size, 10);
}

#[test]
fn all_range_trends_coarsen_instead_of_returning_point_limit_error() {
    let mut connection = test_connection();
    let trends = trends_on(
        &mut connection,
        &TrendRequest {
            from: 0.0,
            to: 9_368.0 * 3_600.0,
            granularity: TrendGranularity::Auto,
            snapshot_seq: None,
            history_generation: None,
            filter: RuntimeFilter::default(),
        },
    )
    .expect("long all-range trend should adapt its bucket");
    assert!(trends.points.len() <= MAX_TREND_POINTS);
    assert_eq!(trends.granularity, TrendGranularity::MultiDay);
    assert!(trends.bucket_seconds > 86_400);
}

#[test]
fn scoped_price_prefers_endpoint_and_falls_back_to_global() {
    let mut catalog = PriceCatalog {
        revision: Some(1),
        currency: Some("USD".into()),
        by_model: HashMap::new(),
    };
    let global = ModelPrice {
        id: 1,
        endpoint_id: None,
        model_key: "model-a".into(),
        effective_from: 0.0,
        effective_to: None,
        input_per_million_micros: Some(10),
        output_per_million_micros: Some(10),
        cache_read_per_million_micros: None,
        cache_creation_per_million_micros: None,
    };
    let scoped = ModelPrice {
        id: 2,
        endpoint_id: Some("endpoint-a".into()),
        model_key: "model-a".into(),
        effective_from: 100.0,
        effective_to: Some(200.0),
        input_per_million_micros: Some(20),
        output_per_million_micros: Some(20),
        cache_read_per_million_micros: None,
        cache_creation_per_million_micros: None,
    };
    catalog
        .by_model
        .entry("model-a".into())
        .or_default()
        .push(global);
    catalog
        .by_model
        .entry(stored_price_key(Some("endpoint-a"), "model-a"))
        .or_default()
        .push(scoped);
    assert_eq!(
        catalog
            .resolve(Some("endpoint-a"), "model-a", 150.0)
            .unwrap()
            .input_per_million_micros,
        Some(20)
    );
    assert_eq!(
        catalog
            .resolve(Some("endpoint-a"), "model-a", 250.0)
            .unwrap()
            .input_per_million_micros,
        Some(10)
    );
    assert_eq!(
        catalog
            .resolve(Some("endpoint-b"), "model-a", 150.0)
            .unwrap()
            .input_per_million_micros,
        Some(10)
    );
}

#[test]
fn project_id_and_display_alias_are_independent_and_consistent() {
    let mut connection = test_connection();
    insert_100k(&connection);
    let filter = RuntimeFilter {
        project_id: Some("project-040".into()),
        project_name: Some("Project 040".into()),
        ..RuntimeFilter::default()
    };
    let wire = serde_json::to_value(&filter).expect("serialize project filters");
    assert_eq!(wire["projectID"], "project-040");
    assert_eq!(wire["project"], "Project 040");

    // Both project predicates are applied as AND conditions. The fixture
    // has exactly 1,000 rows for project-040, so a display-name-only
    // filter and the combined stable-ID/name filter must agree.
    let combined_events = events_page_on(
        &mut connection,
        &EventPageRequest {
            page_size: 25,
            filter: filter.clone(),
            ..EventPageRequest::default()
        },
    )
    .expect("project alias event page");
    assert_eq!(combined_events.total_count, 1_000);
    assert_eq!(combined_events.filters.project_id, filter.project_id);
    assert_eq!(combined_events.filters.project_name, filter.project_name);

    let alias_events = events_page_on(
        &mut connection,
        &EventPageRequest {
            page_size: 25,
            filter: RuntimeFilter {
                project_name: Some("Project 040".into()),
                ..RuntimeFilter::default()
            },
            ..EventPageRequest::default()
        },
    )
    .expect("project display alias event page");
    assert_eq!(alias_events.total_count, combined_events.total_count);

    let trends = trends_on(
        &mut connection,
        &TrendRequest {
            from: 0.0,
            to: 6_000_000.0,
            granularity: TrendGranularity::Auto,
            snapshot_seq: None,
            history_generation: None,
            filter: filter.clone(),
        },
    )
    .expect("project alias trends");
    assert_eq!(trends.totals.client_requests, 1_000);

    let errors = error_groups_on(
        &mut connection,
        &ErrorPageQuery {
            filter: RuntimeFilter {
                outcome: Some("failed".into()),
                ..filter.clone()
            },
            ..ErrorPageQuery::default()
        },
    )
    .expect("project alias errors");
    assert_eq!(errors.total_count, 1);
    assert_eq!(errors.groups[0].occurrences, 1_000);

    let projects = dimension_page_on(
        &mut connection,
        DimensionKind::Project,
        &DimensionPageQuery {
            page_size: 25,
            filter: filter.clone(),
            ..DimensionPageQuery::default()
        },
    )
    .expect("project alias project dimension");
    assert_eq!(projects.total_count, 1);
    assert_eq!(projects.rows[0].key, "project-040");
    assert_eq!(projects.rows[0].client_kinds, vec!["codex"]);

    let sessions = dimension_page_on(
        &mut connection,
        DimensionKind::Session,
        &DimensionPageQuery {
            page_size: 25,
            filter: filter.clone(),
            ..DimensionPageQuery::default()
        },
    )
    .expect("project alias session dimension");
    assert_eq!(sessions.total_count, 5);

    let export_query = ExportQuery {
        scope: ExportScope::Events,
        format: ExportFormat::Jsonl,
        privacy: ExportPrivacy::Redacted,
        confirm_stored: false,
        snapshot_seq: None,
        history_generation: None,
        filter,
    };
    let estimate =
        export_estimate_on(&mut connection, &export_query).expect("project alias export estimate");
    let mut chunks = Vec::new();
    let manifest = stream_export_on(&mut connection, &export_query, |chunk| {
        chunks.extend(chunk);
        Ok(())
    })
    .expect("project alias export stream");
    assert_eq!(estimate.row_count, 1_000);
    assert_eq!(manifest.row_count, estimate.row_count);
    assert_eq!(chunks.iter().filter(|byte| **byte == b'\n').count(), 1_000);
}

#[test]
fn event_page_waits_for_projection_even_without_filters() {
    let mut connection = test_connection();
    connection
        .execute(
            "UPDATE runtime_meta SET value='0' WHERE key='projection_backfill_complete'",
            [],
        )
        .unwrap();
    let error = events_page_on(&mut connection, &EventPageRequest::default()).unwrap_err();
    assert!(matches!(
        error,
        RuntimeQueryError::ProjectionNotReady {
            backfill_cursor: 100000
        }
    ));
}

#[test]
fn snapshot_generation_and_retention_are_enforced() {
    let mut connection = test_connection();
    connection
        .execute(
            "UPDATE runtime_meta SET value='3' WHERE key='history_generation'",
            [],
        )
        .unwrap();
    let expired = events_page_on(
        &mut connection,
        &EventPageRequest {
            snapshot_seq: Some(10),
            history_generation: Some(2),
            ..EventPageRequest::default()
        },
    )
    .unwrap_err();
    assert!(matches!(
        expired,
        RuntimeQueryError::SnapshotExpired {
            requested: 2,
            current: 3
        }
    ));

    connection
        .execute(
            "UPDATE runtime_meta SET value='50' WHERE key='retained_from_seq'",
            [],
        )
        .unwrap();
    let trimmed = events_page_on(
        &mut connection,
        &EventPageRequest {
            snapshot_seq: Some(40),
            history_generation: Some(3),
            ..EventPageRequest::default()
        },
    )
    .unwrap_err();
    assert!(matches!(trimmed, RuntimeQueryError::SnapshotTrimmed { .. }));
}

#[test]
fn outcome_page_uses_partial_composite_index() {
    let connection = test_connection();
    let details = connection
        .prepare(
            "EXPLAIN QUERY PLAN SELECT seq FROM runtime_events \
                 WHERE is_in_flight=0 AND outcome=?1 AND seq<=?2 ORDER BY seq DESC LIMIT 25",
        )
        .unwrap()
        .query_map(("failed", 100_000_i64), |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join("\n");
    assert!(
        details.contains("runtime_events_outcome_seq_v2"),
        "unexpected query plan: {details}"
    );
}

#[test]
fn cache_request_rate_requires_explicit_cache_field() {
    let mut accumulator = TokenAccumulator::default();
    let row = TrendRow {
        kind: "client".into(),
        timestamp: 1.0,
        outcome: Some("succeeded".into()),
        failover: 0,
        duration_ms: Some(1),
        ttfb_ms: Some(1),
        request_purpose: Some("completion".into()),
        endpoint_id: None,
        effective_model: Some("gpt-test".into()),
        usage_present: 1,
        input_tokens: Some(100),
        output_tokens: Some(1),
        cache_read_input_tokens: None,
        cache_creation_input_tokens: Some(0),
        reasoning_tokens: None,
        uncached_input_tokens: Some(100),
        processed_input_tokens: Some(100),
        processed_total_tokens: Some(101),
        token_accounting_semantics: Some("subset".into()),
        token_accounting_quality: Some("complete".into()),
    };
    accumulator.add(&row);
    let metrics = accumulator.finish();
    assert_eq!(metrics.cache_read_reported_requests, 0);
    assert_eq!(metrics.cache_read_request_rate, None);
    assert_eq!(metrics.cache_read_token_rate, None);
    assert_eq!(metrics.cache_read_token_unknown_requests, 1);
}
