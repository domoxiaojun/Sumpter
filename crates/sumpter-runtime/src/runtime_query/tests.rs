use super::MAX_TREND_POINTS;
use super::export_queries::stream_export_on;
use crate::database::{Connection, params};
use crate::runtime_query::*;

#[test]
fn inbound_token_filters_keep_renames_history_costs_and_exports_without_schema_changes() {
    let mut connection = test_connection();
    for (index, identity) in [
        Some(("work", "旧名称")),
        Some(("work", "新名称")),
        Some(("personal", "个人")),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        let request = format!("request-{index}");
        for (offset, kind) in [(1, "client"), (2, "upstream")] {
            let seq = index as i64 * 2 + offset;
            insert_endpoint_test_event(
                &connection,
                seq,
                kind,
                Some(&request),
                "succeeded",
                200,
                Some("codex"),
                Some("completion"),
                Some("endpoint-a"),
                Some("Endpoint A"),
            );
            let payload = identity
                .map(|(id, name)| serde_json::json!({"inboundToken": {"id": id, "name": name}}))
                .unwrap_or(serde_json::json!({}))
                .to_string();
            connection.execute("UPDATE runtime_events SET timestamp=?1,payload_json=?2,effective_model='gpt-test',project_id='project-a',project_name='Project A',usage_present=1,input_tokens=1000000,output_tokens=0,cache_read_input_tokens=0,cache_creation_input_tokens=0,processed_input_tokens=1000000,processed_total_tokens=1000000,uncached_input_tokens=1000000,token_accounting_semantics='subset',token_accounting_quality='complete' WHERE seq=?1", params![seq, payload]).unwrap();
        }
    }
    let filter = RuntimeFilter {
        inbound_token_id: Some("work".into()),
        ..Default::default()
    };
    let analytics = analytics_on(&mut connection, "all", &filter).unwrap();
    assert_eq!(analytics.client_requests, 2);
    let trends = trends_on(
        &mut connection,
        &TrendRequest {
            from: 0.0,
            to: 3600.0,
            granularity: TrendGranularity::Hour,
            snapshot_seq: None,
            snapshot_change_seq: None,
            history_generation: None,
            filter: filter.clone(),
        },
    )
    .unwrap();
    assert_eq!(trends.totals.client_requests, 2);
    assert_eq!(trends.totals.upstream_attempts, 2);
    assert_eq!(trends.totals.tokens.input_tokens, 2_000_000);
    assert_eq!(trends.totals.cost.priced_requests, 2);
    assert_eq!(trends.totals.cost.estimated_cost_micros, 2_000_000);
    let facets = facets_on(&mut connection, &filter).unwrap();
    let work = facets
        .facets
        .inbound_tokens
        .iter()
        .find(|row| row.value == "work")
        .unwrap();
    assert_eq!((work.name.as_str(), work.count), ("新名称", 2));
    assert!(
        facets
            .facets
            .inbound_tokens
            .iter()
            .any(|row| row.value == "personal")
    );
    let mut client_filter = filter.clone();
    client_filter.kind = Some("client".into());
    let page = events_page_on(
        &mut connection,
        &EventPageRequest {
            filter: client_filter.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total_count, 2);
    assert!(
        page.events
            .iter()
            .all(|row| row.inbound_token.as_ref().unwrap().id == "work")
    );
    let projects = dimension_page_on(
        &mut connection,
        DimensionKind::Project,
        &DimensionPageQuery {
            filter: client_filter.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(projects.rows[0].requests, 2);
    for privacy in [ExportPrivacy::Redacted, ExportPrivacy::Stored] {
        for format in [ExportFormat::Jsonl, ExportFormat::Csv] {
            let query = ExportQuery {
                scope: ExportScope::Events,
                format,
                privacy,
                confirm_stored: true,
                snapshot_seq: None,
                snapshot_change_seq: None,
                history_generation: None,
                filter: client_filter.clone(),
            };
            assert_eq!(
                export_estimate_on(&mut connection, &query)
                    .unwrap()
                    .row_count,
                2
            );
            let mut bytes = Vec::new();
            stream_export_on(&mut connection, &query, |chunk| {
                bytes.extend(chunk);
                Ok(())
            })
            .unwrap();
            let output = String::from_utf8(bytes).unwrap();
            assert!(output.contains("work"));
            assert!(!output.contains("personal"));
            if format == ExportFormat::Csv {
                assert_eq!(output.lines().count(), 3);
            } else {
                assert_eq!(output.lines().count(), 2);
            }
        }
    }
    let history = events_page_on(
        &mut connection,
        &EventPageRequest {
            filter: RuntimeFilter {
                inbound_token_id: Some("unrecorded".into()),
                kind: Some("client".into()),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(history.total_count, 1);
    assert!(history.events[0].inbound_token.is_none());
    assert_eq!(crate::runtime_store::SCHEMA_VERSION, 5);
    assert_eq!(PROJECTION_VERSION, 11);
}

#[test]
fn cache_sort_response_uses_accepted_query_keys() {
    assert_eq!(
        serde_json::to_value(DimensionSort::CacheReadTokens).unwrap(),
        "cache_read"
    );
    assert_eq!(
        serde_json::to_value(DimensionSort::CacheWriteTokens).unwrap(),
        "cache_write"
    );
}

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
                   completed_change_seq INTEGER,
                   event_id TEXT NOT NULL UNIQUE,request_id TEXT,timestamp REAL NOT NULL,
                   kind TEXT NOT NULL,phase TEXT,outcome TEXT,status_code INTEGER NOT NULL,
                   request_kind TEXT,client_attribution_json TEXT,
                   client_variant TEXT,agent_role TEXT,agent_name TEXT,parent_thread_id TEXT,parent_turn_id TEXT,root_turn_id TEXT,
                   client_kind TEXT,request_purpose TEXT,endpoint_id TEXT,failure_kind TEXT,
                   is_in_flight INTEGER NOT NULL,payload_json TEXT NOT NULL,
                   projection_version INTEGER NOT NULL DEFAULT 0,
                   payload_bytes INTEGER NOT NULL DEFAULT 0,session_key TEXT,session_source TEXT,
                   project_id TEXT,project_name TEXT,project_source TEXT,local_user TEXT,workspace_paths_json TEXT,
                   endpoint_name TEXT,pool_id TEXT,feature_rule_id TEXT,client_model TEXT,
                   effective_model TEXT,upstream_model TEXT,failure_phase TEXT,
                   source_format TEXT,target_format TEXT,route_mode TEXT,
                   upstream_status_code INTEGER,duration_ms INTEGER,ttfb_ms INTEGER,
                   upstream_duration_ms INTEGER,upstream_ttfb_ms INTEGER,output_span_ms INTEGER,
                   failover INTEGER,stream_terminal TEXT,codex_metadata_present INTEGER,
                   usage_present INTEGER,input_tokens INTEGER,output_tokens INTEGER,
                   cache_read_state TEXT,cache_read_finality TEXT,cache_read_reason TEXT,hook_event TEXT,
                   cache_read_input_tokens INTEGER,cache_creation_input_tokens INTEGER,
                   reasoning_tokens INTEGER,uncached_input_tokens INTEGER,
                   processed_input_tokens INTEGER,processed_total_tokens INTEGER,
                   token_accounting_semantics TEXT,token_accounting_quality TEXT,
                   tool_calls_json TEXT,
                   codex_thread_class TEXT,attribution_scope TEXT,
                   request_method TEXT,request_path TEXT,route_intent TEXT,
                   model_group_id TEXT,model_group_name TEXT
                 );
                 CREATE TRIGGER test_completed_watermark AFTER INSERT ON runtime_events
                   WHEN NEW.is_in_flight=0 AND NEW.completed_change_seq IS NULL
                   BEGIN UPDATE runtime_events SET completed_change_seq=NEW.change_seq WHERE seq=NEW.seq; END;
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
                 ) VALUES (?1,?1,?2,?3,1.0,?4,'completed',?5,?6,?7,?8,?9,0,'{}',?11,?10)",
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
                PROJECTION_VERSION,
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
                   11,80,printf('session-%03d',seq%500),'header',printf('project-%03d',seq%100),
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
fn event_page_projects_tool_calls_without_payload() {
    let mut connection = test_connection();
    insert_100k(&connection);
    // 列表快路径不解 payload_json;工具名必须来自投影列,空/损坏时保持 None。
    connection
        .execute_batch(
            "UPDATE runtime_events SET tool_calls_json='[\"Read\",\"Bash\"]' WHERE seq=100000;
             UPDATE runtime_events SET tool_calls_json='[]' WHERE seq=99999;
             UPDATE runtime_events SET tool_calls_json='{broken' WHERE seq=99998;",
        )
        .expect("set tool_calls_json");

    let events = events_page_on(
        &mut connection,
        &EventPageRequest {
            page: 1,
            page_size: 10,
            ..EventPageRequest::default()
        },
    )
    .expect("query event page");
    assert!(events.events[0].details_omitted);
    assert_eq!(
        events.events[0].tool_calls.as_deref(),
        Some(&["Read".to_owned(), "Bash".to_owned()][..])
    );
    assert_eq!(events.events[1].tool_calls, None);
    assert_eq!(events.events[2].tool_calls, None);
}

#[test]
fn event_page_projects_explicit_transport_and_preserves_legacy_unknown() {
    let mut connection = test_connection();
    insert_100k(&connection);
    connection
        .execute_batch(
            "UPDATE runtime_events SET payload_json='{\"transport\":\"sse\"}' WHERE seq=100000;
             UPDATE runtime_events SET payload_json='{\"transport\":\"websocket\"}' WHERE seq=99999;
             UPDATE runtime_events SET payload_json='{}' WHERE seq=99998;",
        )
        .unwrap();
    let page = events_page_on(&mut connection, &EventPageRequest::default()).unwrap();
    assert_eq!(
        page.events[0].transport,
        Some(sumpter_core::events::RuntimeTransport::Sse)
    );
    assert_eq!(
        page.events[1].transport,
        Some(sumpter_core::events::RuntimeTransport::Websocket)
    );
    assert_eq!(page.events[2].transport, None);
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
            snapshot_change_seq: None,
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

    crate::storage_metrics::ensure(&mut connection).unwrap();
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
            snapshot_change_seq: None,
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
            snapshot_change_seq: None,
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
        snapshot_change_seq: None,
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
            crate::database::params![],
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
            crate::database::params![],
        )
        .unwrap();
    let expired = events_page_on(
        &mut connection,
        &EventPageRequest {
            snapshot_seq: Some(10),
            snapshot_change_seq: Some(10),
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
            crate::database::params![],
        )
        .unwrap();
    let trimmed = events_page_on(
        &mut connection,
        &EventPageRequest {
            snapshot_seq: Some(40),
            snapshot_change_seq: Some(40),
            history_generation: Some(3),
            ..EventPageRequest::default()
        },
    )
    .unwrap_err();
    assert!(matches!(trimmed, RuntimeQueryError::SnapshotTrimmed { .. }));
}

#[test]
fn dimension_pages_keep_exact_costs_across_price_changes() {
    let mut connection = test_connection();
    for seq in 1..=40 {
        insert_endpoint_test_event(
            &connection,
            seq,
            "client",
            None,
            "succeeded",
            200,
            Some("codex"),
            Some("completion"),
            Some("endpoint-a"),
            Some("Endpoint A"),
        );
    }
    connection
        .execute_batch(
            "UPDATE runtime_events SET
           timestamp=CASE WHEN seq<=20 THEN 1 ELSE 2 END,
           project_id=printf('project-%02d', (seq-1)%20),
           project_name=printf('Project %02d', (seq-1)%20),
           project_source='client_declared',workspace_paths_json='[]',
           effective_model='gpt-test',failover=0,usage_present=1,
           input_tokens=100,output_tokens=20,cache_read_input_tokens=0,
           cache_creation_input_tokens=0,reasoning_tokens=0,uncached_input_tokens=100,
           processed_input_tokens=100,processed_total_tokens=120,
           token_accounting_semantics='subset',token_accounting_quality='complete';
         UPDATE runtime_events SET project_id='project-00-quoted''key' WHERE (seq-1)%20=0;
         UPDATE runtime_model_prices SET effective_to=2 WHERE id=1;
         INSERT INTO runtime_model_prices VALUES
           (2,'gpt-test',2,NULL,2000000,4000000,200000,1000000,0,0);",
        )
        .unwrap();
    for page in [1, 2] {
        let result = dimension_page_on(
            &mut connection,
            DimensionKind::Project,
            &DimensionPageQuery {
                page,
                page_size: 10,
                sort: DimensionSort::Name,
                order: SortOrder::Asc,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.total_count, 20);
        assert_eq!(result.rows.len(), 10);
        for row in result.rows {
            assert_eq!(row.requests, 2);
            assert_eq!(row.cost.estimated_cost_micros, 420);
            assert_eq!(row.cost.priced_requests, 2);
        }
    }
    assert!(
        dimension_page_on(
            &mut connection,
            DimensionKind::Project,
            &DimensionPageQuery {
                page: 3,
                ..Default::default()
            }
        )
        .unwrap()
        .rows
        .is_empty()
    );

    let full = serde_json::to_value(
        analytics_on(&mut connection, "all", &RuntimeFilter::default()).unwrap(),
    )
    .unwrap();
    let diagnostic = serde_json::to_value(
        analytics_view_on(&mut connection, "all", &RuntimeFilter::default(), true).unwrap(),
    )
    .unwrap();
    for key in [
        "models",
        "requestPurposes",
        "protocolRoutes",
        "failureKinds",
        "failurePhases",
        "featureRules",
        "upstreamStatuses",
        "streamTerminals",
        "internalFeatures",
        "toolCalls",
        "tokenUsage",
        "codexRequests",
    ] {
        assert_eq!(
            full[key], diagnostic[key],
            "diagnostic projection changed {key}"
        );
    }
    assert!(!full["projects"].as_array().unwrap().is_empty());
    assert!(diagnostic["projects"].as_array().unwrap().is_empty());
    assert!(diagnostic["sessions"].as_array().unwrap().is_empty());
    assert!(
        diagnostic["facets"]["projects"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

fn insert_renamed_dimension_events(connection: &Connection) {
    for (seq, key, name) in [(1, "a", "Z old"), (2, "b", "Middle"), (3, "a", "A new")] {
        insert_endpoint_test_event(
            connection,
            seq,
            "client",
            None,
            "succeeded",
            200,
            Some("codex"),
            Some("completion"),
            Some(key),
            Some(name),
        );
    }
    connection.execute_batch(
        "UPDATE runtime_events SET timestamp=seq,project_id=endpoint_id,project_name=endpoint_name,
           project_source=CASE WHEN seq=3 THEN 'session_fallback' ELSE 'workspace_local' END,
           session_key='session-' || endpoint_id,
           session_source=CASE WHEN seq=3 THEN 'native' ELSE 'header' END,
           workspace_paths_json=CASE WHEN seq=3 THEN '[]' ELSE '[\".../repo\"]' END,
           effective_model='gpt-test',failover=0,usage_present=1,
           input_tokens=CASE WHEN seq=2 THEN 150 ELSE 100 END,output_tokens=20,
           cache_read_input_tokens=0,cache_creation_input_tokens=0,
           uncached_input_tokens=CASE WHEN seq=2 THEN 150 ELSE 100 END,
           processed_input_tokens=CASE WHEN seq=2 THEN 150 ELSE 100 END,
           processed_total_tokens=CASE WHEN seq=2 THEN 170 ELSE 120 END,
           token_accounting_semantics='subset',token_accounting_quality='complete',
           duration_ms=1000,ttfb_ms=100,upstream_duration_ms=1000,upstream_ttfb_ms=100;"
    ).unwrap();
}

#[test]
fn dimension_identity_survives_renames_and_mixed_sources_for_every_sort() {
    let mut connection = test_connection();
    insert_renamed_dimension_events(&connection);
    for kind in [
        DimensionKind::Endpoint,
        DimensionKind::Project,
        DimensionKind::Session,
    ] {
        for sort in [
            DimensionSort::Name,
            DimensionSort::Requests,
            DimensionSort::SuccessRate,
            DimensionSort::Failures,
            DimensionSort::InputTokens,
            DimensionSort::OutputTokens,
            DimensionSort::CacheReadTokens,
            DimensionSort::CacheWriteTokens,
            DimensionSort::Tokens,
            DimensionSort::AverageDuration,
            DimensionSort::OutputTokensPerSecond,
            DimensionSort::LastSeen,
        ] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                let page = dimension_page_on(
                    &mut connection,
                    kind,
                    &DimensionPageQuery {
                        sort,
                        order,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(page.total_count, 2, "{kind:?} {sort:?}");
                assert_eq!(page.rows.len(), 2);
                let row = page
                    .rows
                    .iter()
                    .find(|row| row.key == "a" || row.key == "session-a")
                    .unwrap();
                assert_eq!(row.requests, 2);
                assert_eq!(row.input_tokens, 200);
                assert_eq!(row.cost.estimated_cost_micros, 280);
                assert_eq!(row.cost.priced_requests, 2);
                if kind != DimensionKind::Endpoint {
                    assert_eq!(row.source, "mixed");
                }
                if kind == DimensionKind::Project {
                    assert_eq!(row.workspace_paths, vec![".../repo"]);
                }
                if kind != DimensionKind::Session {
                    assert_eq!(row.name, "A new");
                }
                if sort == DimensionSort::Name && order == SortOrder::Asc {
                    assert_eq!(page.rows[0].key, row.key);
                }
                if sort == DimensionSort::InputTokens && order == SortOrder::Asc {
                    assert_eq!(page.rows[0].input_tokens, 150);
                }
                assert_eq!(
                    page.rows
                        .iter()
                        .map(|row| row.cost.estimated_cost_micros)
                        .sum::<i64>(),
                    470
                );
            }
        }
    }
}

#[test]
fn dimension_display_metadata_and_cost_respect_snapshot_and_time_filters() {
    let mut connection = test_connection();
    insert_renamed_dimension_events(&connection);
    let snapshot = dimension_page_on(
        &mut connection,
        DimensionKind::Project,
        &DimensionPageQuery {
            snapshot_seq: Some(2),
            snapshot_change_seq: Some(2),
            sort: DimensionSort::Name,
            order: SortOrder::Asc,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(snapshot.rows[0].name, "Middle");
    let old = &snapshot.rows[1];
    assert_eq!(old.key, "a");
    assert_eq!(old.name, "Z old");
    assert_eq!(old.source, "workspace_local");
    assert_eq!(old.requests, 1);
    assert_eq!(old.cost.estimated_cost_micros, 140);

    let filtered = dimension_page_on(
        &mut connection,
        DimensionKind::Project,
        &DimensionPageQuery {
            filter: RuntimeFilter {
                project_id: Some("a".into()),
                from: Some(2.0),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.total_count, 1);
    assert_eq!(filtered.rows[0].name, "A new");
    assert_eq!(filtered.rows[0].source, "session_fallback");
    assert_eq!(filtered.rows[0].requests, 1);
    assert_eq!(filtered.rows[0].cost.estimated_cost_micros, 140);
}

#[test]
fn dimension_export_estimates_and_rows_share_stable_identity() {
    let mut connection = test_connection();
    insert_renamed_dimension_events(&connection);
    for scope in [ExportScope::Projects, ExportScope::Sessions] {
        for format in [ExportFormat::Jsonl, ExportFormat::Csv] {
            let query = ExportQuery {
                scope,
                format,
                privacy: ExportPrivacy::Stored,
                confirm_stored: true,
                snapshot_seq: None,
                snapshot_change_seq: None,
                history_generation: None,
                filter: RuntimeFilter::default(),
            };
            let estimate = export_estimate_on(&mut connection, &query).unwrap();
            assert_eq!(estimate.row_count, 2);
            let mut bytes = Vec::new();
            let manifest = stream_export_on(&mut connection, &query, |chunk| {
                bytes.extend(chunk);
                Ok(())
            })
            .unwrap();
            assert_eq!(manifest.row_count, estimate.row_count);
            let text = String::from_utf8(bytes).unwrap();
            assert_eq!(
                text.lines().count(),
                if format == ExportFormat::Csv { 3 } else { 2 }
            );
            if format == ExportFormat::Jsonl {
                let rows = text
                    .lines()
                    .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(rows[0]["source"], "mixed");
                assert_eq!(rows[0]["requests"], 2);
                assert_eq!(rows[0]["inputTokens"], 200);
                if scope == ExportScope::Projects {
                    assert_eq!(rows[0]["name"], "A new");
                    assert_eq!(rows[0]["workspacePaths"], serde_json::json!([".../repo"]));
                }
            }
        }
    }
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
        output_span_ms: None,
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

#[test]
fn attribution_filters_match_variant_and_parent_chain_fields() {
    let mut connection = test_connection();
    connection.execute(
        "INSERT INTO runtime_events(seq,change_seq,event_id,timestamp,kind,phase,outcome,status_code,is_in_flight,payload_json,projection_version,client_variant,agent_role,agent_name,parent_thread_id,parent_turn_id,root_turn_id)
         VALUES(1,1,'attrib',1.0,'client','completed','succeeded',200,0,'{}',11,'tui','subagent','worker','thread-1','turn-2','turn-0')", crate::database::params![],
    ).unwrap();
    let filter = RuntimeFilter {
        client_variant: Some("tui".into()),
        agent_role: Some("subagent".into()),
        parent_thread_id: Some("thread-1".into()),
        ..RuntimeFilter::default()
    };
    let page = events_page_on(
        &mut connection,
        &EventPageRequest {
            page: 1,
            page_size: 10,
            filter,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(page.total_count, 1);
}

#[test]
fn event_page_keeps_late_completion_and_websocket_diagnostics() {
    let mut connection = test_connection();
    for seq in 1..=12 {
        insert_endpoint_test_event(
            &connection,
            seq,
            "client",
            None,
            "failed",
            101,
            None,
            None,
            None,
            None,
        );
    }
    connection.execute_batch(r#"
        UPDATE runtime_events SET timestamp=seq;
        UPDATE runtime_events SET timestamp=100,change_seq=100,completed_change_seq=100,
        payload_json='{"transport":"websocket","message":"websocket relay","failureDetail":"websocket relay interrupted","streamTrace":{"websocketTrace":{"stage":"relay","closedBy":"upstream","transportErrorKind":"unexpected_eof","lastEventType":"response.completed"}}}'
        WHERE seq=1;
    "#).unwrap();
    let page = events_page_on(&mut connection, &EventPageRequest::default()).unwrap();
    assert_eq!(page.events.len(), 10);
    assert_eq!(page.events[0].seq, 1);
    let trace = page.events[0]
        .stream_trace
        .as_ref()
        .unwrap()
        .websocket_trace
        .as_ref()
        .unwrap();
    assert_eq!(
        trace.transport_error_kind.as_deref(),
        Some("unexpected_eof")
    );
    assert_eq!(
        page.events[0].failure_detail.as_deref(),
        Some("websocket relay interrupted")
    );
}

#[test]
fn reporting_aliases_are_reversible_across_queries_and_exports_without_rewriting_events() {
    let mut connection = test_connection();
    insert_endpoint_test_event(
        &connection,
        1,
        "client",
        Some("request-raw"),
        "succeeded",
        200,
        Some("openai_compat"),
        Some("completion"),
        Some("endpoint-a"),
        Some("Endpoint A"),
    );
    let attribution = serde_json::json!({"version":2,"identity":{"product":"custom-bridge","agentId":"private-child","parentAgentId":"private-parent","sessionId":"private-session"},"evidence":[],"conflicts":[],"issues":[]});
    let payload = serde_json::json!({"clientKind":"openai_compat","clientAttribution":attribution});
    connection.execute("UPDATE runtime_events SET timestamp=1,client_attribution_json=?1,payload_json=?2 WHERE seq=1", params![attribution.to_string(),payload.to_string()]).unwrap();
    let mut filter = RuntimeFilter {
        client_kind: Some("codex".into()),
        ..Default::default()
    };
    assert_eq!(
        analytics_on(&mut connection, "all", &filter)
            .unwrap()
            .client_requests,
        0
    );
    filter
        .reporting_aliases
        .insert("custom-bridge".into(), "codex".into());
    let analytics = analytics_on(&mut connection, "all", &filter).unwrap();
    assert_eq!(analytics.client_requests, 1);
    assert_eq!(analytics.client_kinds[0].name, "codex");
    let facets = facets_on(&mut connection, &filter).unwrap();
    assert!(
        facets
            .facets
            .client_kinds
            .iter()
            .any(|r| r.value == "codex")
    );
    let dimensions = dimension_page_on(
        &mut connection,
        DimensionKind::ClientKind,
        &DimensionPageQuery {
            filter: filter.clone(),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(dimensions.rows[0].name, "codex");
    let trends = trends_on(
        &mut connection,
        &TrendRequest {
            from: 0.0,
            to: 3600.0,
            granularity: TrendGranularity::Hour,
            snapshot_seq: None,
            snapshot_change_seq: None,
            history_generation: None,
            filter: filter.clone(),
        },
    )
    .unwrap();
    assert_eq!(trends.totals.client_requests, 1);
    for privacy in [ExportPrivacy::Redacted, ExportPrivacy::Stored] {
        let query = ExportQuery {
            scope: ExportScope::Events,
            format: ExportFormat::Jsonl,
            privacy,
            confirm_stored: true,
            snapshot_seq: None,
            snapshot_change_seq: None,
            history_generation: None,
            filter: filter.clone(),
        };
        let mut bytes = Vec::new();
        stream_export_on(&mut connection, &query, |chunk| {
            bytes.extend(chunk);
            Ok(())
        })
        .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["reportedClientKind"], "codex");
        if privacy == ExportPrivacy::Stored {
            assert_eq!(value["event"]["clientKind"], "openai_compat");
        } else {
            assert_eq!(value["clientKind"], "openai_compat");
            assert!(!String::from_utf8(bytes).unwrap().contains("private-child"));
        }
    }
    let raw: String = connection
        .query_row(
            "SELECT client_attribution_json FROM runtime_events WHERE seq=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&raw).unwrap(),
        attribution
    );
    filter.reporting_aliases.clear();
    assert_eq!(
        analytics_on(&mut connection, "all", &filter)
            .unwrap()
            .client_requests,
        0
    );
}
