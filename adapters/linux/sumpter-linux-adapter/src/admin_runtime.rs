use super::{AdminState, api_error, deserialize_optional_f64, json_ok};
use crate::health;
use crate::runtime_query::{
    DimensionKind, DimensionPageQuery, DimensionSort, ErrorPageQuery, EventPageQuery, ExportFormat,
    ExportPrivacy, ExportQuery, ExportScope, RuntimeFilter, RuntimeQueryError, SortOrder,
    TrendGranularity, TrendQuery,
};
use crate::runtime_store::{
    AnalyticsFilter, RuntimeModelPriceInput, RuntimePricingUpdate, RuntimeRetentionUpdate,
};
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, Query, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::response::Response;
use bytes::Bytes;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) async fn status(State(state): State<AdminState>) -> Response {
    let config = state.inner.engine.config();
    let snapshot = state.inner.engine.runtime_snapshot();
    let proxy = state.inner.proxy.status().await;
    let health = health::evaluate(&snapshot.recent_events, proxy.running);
    json_ok(&json!({
        "running": proxy.running && state.inner.engine.runtime_database_issue().is_none(),
        "runtimeDatabaseIssue": state.inner.engine.runtime_database_issue(),
        "daemonRunning": true,
        "runtimeApiVersion": 1,
        "state": proxy.state,
        "version": env!("CARGO_PKG_VERSION"),
        "generation": state.inner.engine.generation(),
        "uptimeSeconds": state.inner.engine.uptime_seconds(),
        "listener": {
            "host": config.listener.host,
            "port": config.listener.port,
            "allowedCIDRs": config.listener.allowed_cidrs,
            "hasAuthToken": !config.listener.auth_token.is_empty(),
        },
        "providers": config.endpoints.len(),
        "endpoints": config.endpoints.len(),
        "counters": {
            "clientRequests": snapshot.client_requests,
            "clientSuccesses": snapshot.client_successes,
            "clientFailures": snapshot.client_failures,
            "upstreamAttempts": snapshot.upstream_attempts,
            "failovers": snapshot.failovers,
        },
        "health": health,
        "lastError": state.inner.engine.last_error(),
    }))
}

pub(crate) type JsonPayload<T> = Result<Json<T>, JsonRejection>;

#[allow(clippy::result_large_err)]
pub(crate) fn require_json<T>(payload: JsonPayload<T>) -> Result<T, Response> {
    payload.map(|Json(value)| value).map_err(|rejection| {
        api_error(
            StatusCode::BAD_REQUEST,
            "invalid_json",
            &rejection.to_string(),
        )
    })
}

pub(crate) async fn proxy_start(
    State(state): State<AdminState>,
    payload: JsonPayload<Value>,
) -> Response {
    if let Err(response) = require_json(payload) {
        return response;
    }
    match state.inner.proxy.start().await {
        Ok(proxy) => {
            state.inner.engine.set_last_error(None);
            json_ok(&json!({"proxy": proxy, "running": true}))
        }
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(StatusCode::CONFLICT, "proxy_start_failed", &message)
        }
    }
}

pub(crate) async fn proxy_stop(
    State(state): State<AdminState>,
    payload: JsonPayload<Value>,
) -> Response {
    if let Err(response) = require_json(payload) {
        return response;
    }
    let proxy = state.inner.proxy.stop().await;
    json_ok(&json!({"proxy": proxy, "running": false}))
}

pub(crate) async fn runtime_summary(State(state): State<AdminState>) -> Response {
    json_ok(&state.inner.engine.runtime_summary_value())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeEventsQuery {
    view: Option<String>,
    page: Option<usize>,
    #[serde(rename = "pageSize", alias = "page_size")]
    page_size: Option<usize>,
    #[serde(rename = "snapshotSeq", alias = "snapshot_seq")]
    snapshot_seq: Option<i64>,
    #[serde(rename = "historyGeneration", alias = "history_generation")]
    history_generation: Option<i64>,
    pub(crate) before_seq: Option<i64>,
    pub(crate) after_change_seq: Option<i64>,
    limit: Option<usize>,
    kind: Option<String>,
    #[serde(rename = "requestID", alias = "requestId")]
    pub(crate) request_id: Option<String>,
    outcome: Option<String>,
    #[serde(rename = "clientKind", alias = "client_kind")]
    client_kind: Option<String>,
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
    #[serde(rename = "requestPurpose", alias = "request_purpose")]
    request_purpose: Option<String>,
    #[serde(rename = "endpointID", alias = "endpoint_id")]
    endpoint_id: Option<String>,
    model: Option<String>,
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: Option<String>,
    /// Human-readable project name filter. Keep projectID/project_id as the
    /// stable identity filter; `project` is an additive compatibility alias
    /// used by the WebUI selector.
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "failureKind", alias = "failure_kind")]
    failure_kind: Option<String>,
    #[serde(rename = "failurePhase", alias = "failure_phase")]
    failure_phase: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub(crate) from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    pub(crate) to: Option<f64>,
}

pub(crate) async fn runtime_events(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeEventsQuery>,
) -> Response {
    if query.view.as_deref() == Some("page") {
        if query.before_seq.is_some() || query.after_change_seq.is_some() {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_runtime_query",
                "view=page 不能与 beforeSeq/afterChangeSeq 同时使用",
            );
        }
        let request = EventPageQuery {
            page: query.page.unwrap_or(1),
            page_size: query.page_size.unwrap_or(10),
            snapshot_seq: query.snapshot_seq,
            history_generation: query.history_generation,
            filter: runtime_filter_from_events_query(&query),
        };
        return match state.inner.engine.runtime_events_page(&request) {
            Ok(value) => json_ok(&value),
            Err(error) => runtime_query_error_response(error),
        };
    }
    if query.view.is_some() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_runtime_query",
            "view 只支持 page",
        );
    }
    match state.inner.engine.runtime_events(
        query.before_seq,
        query.after_change_seq,
        query.limit.unwrap_or(10),
        query.kind.as_deref(),
        query.request_id.as_deref(),
        query.outcome.as_deref(),
        query.from,
        query.to,
    ) {
        Ok(value) => json_ok(&value),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

pub(crate) fn runtime_filter_from_events_query(query: &RuntimeEventsQuery) -> RuntimeFilter {
    RuntimeFilter {
        kind: query.kind.clone(),
        outcome: query.outcome.clone(),
        client_kind: query.client_kind.clone(),
        client_variant: query.client_variant.clone(),
        agent_role: query.agent_role.clone(),
        agent_name: query.agent_name.clone(),
        parent_thread_id: query.parent_thread_id.clone(),
        parent_turn_id: query.parent_turn_id.clone(),
        root_turn_id: query.root_turn_id.clone(),
        request_purpose: query.request_purpose.clone(),
        request_id: query.request_id.clone(),
        endpoint_id: query.endpoint_id.clone(),
        model: query.model.clone(),
        project_id: query.project_id.clone(),
        project_name: query.project.clone(),
        session_id: query.session_id.clone(),
        failure_kind: query.failure_kind.clone(),
        failure_phase: query.failure_phase.clone(),
        from: query.from,
        to: query.to,
    }
}

pub(crate) async fn runtime_event_detail(
    State(state): State<AdminState>,
    Path(id): Path<String>,
) -> Response {
    match state.inner.engine.runtime_event(&id) {
        Ok(Some(value)) => json_ok(&value),
        Ok(None) => api_error(
            StatusCode::NOT_FOUND,
            "runtime_event_not_found",
            "运行事件不存在",
        ),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeRequestChainQuery {
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
}

pub(crate) async fn runtime_request_chain(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeRequestChainQuery>,
) -> Response {
    let Some(request_id) = query
        .request_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "request_id_required",
            "必须提供 requestID",
        );
    };
    match state.inner.engine.runtime_request_chain(request_id) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeFilterQuery {
    kind: Option<String>,
    outcome: Option<String>,
    client_kind: Option<String>,
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
    request_purpose: Option<String>,
    #[serde(rename = "requestID", alias = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "endpointID", alias = "endpointId")]
    endpoint_id: Option<String>,
    model: Option<String>,
    #[serde(rename = "projectID", alias = "projectId")]
    project_id: Option<String>,
    /// Human-readable project name filter; projectID remains the stable ID.
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "sessionId")]
    session_id: Option<String>,
    failure_kind: Option<String>,
    failure_phase: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    to: Option<f64>,
}

impl From<RuntimeFilterQuery> for RuntimeFilter {
    fn from(query: RuntimeFilterQuery) -> Self {
        Self {
            kind: query.kind,
            outcome: query.outcome,
            client_kind: query.client_kind,
            client_variant: query.client_variant,
            agent_role: query.agent_role,
            agent_name: query.agent_name,
            parent_thread_id: query.parent_thread_id,
            parent_turn_id: query.parent_turn_id,
            root_turn_id: query.root_turn_id,
            request_purpose: query.request_purpose,
            request_id: query.request_id,
            endpoint_id: query.endpoint_id,
            model: query.model,
            project_id: query.project_id,
            project_name: query.project,
            session_id: query.session_id,
            failure_kind: query.failure_kind,
            failure_phase: query.failure_phase,
            from: query.from,
            to: query.to,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeTrendQuery {
    range: Option<String>,
    granularity: Option<String>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeFacetsQuery {
    range: Option<String>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

pub(crate) async fn runtime_facets(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeFacetsQuery>,
) -> Response {
    let now = admin_apple_timestamp();
    let range = query.range.as_deref().unwrap_or("24h");
    let range_from = match range {
        "1h" => Some(now - 3_600.0),
        "24h" => Some(now - 86_400.0),
        // The UI supplies the user's local midnight as `from`. The UTC
        // fallback keeps API callers deterministic instead of silently
        // treating an omitted boundary as "all".
        "today" => Some(utc_today_start(now)),
        "7d" => Some(now - 7.0 * 86_400.0),
        "30d" => Some(now - 30.0 * 86_400.0),
        "all" => None,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "range 必须是 today、1h、24h、7d、30d 或 all",
            );
        }
    };
    let mut filter: RuntimeFilter = query.filters.into();
    filter.from = crate::runtime_query::merge_range_lower_bound(range, filter.from, range_from);
    filter.to = Some(filter.to.map_or(now, |value| value.min(now)));
    match state.inner.engine.runtime_facets(&filter) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

pub(crate) async fn runtime_trends(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeTrendQuery>,
) -> Response {
    let now = admin_apple_timestamp();
    let (default_from, default_to) = match query.range.as_deref().unwrap_or("24h") {
        "1h" => (now - 3_600.0, now),
        "24h" => (now - 86_400.0, now),
        // The client-provided local midnight wins when present; otherwise use
        // a deterministic UTC calendar-day fallback.
        "today" => (utc_today_start(now), now),
        "7d" => (now - 7.0 * 86_400.0, now),
        "30d" => (now - 30.0 * 86_400.0, now),
        "all" => (0.0, now),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_range",
                "range 必须是 today、1h、24h、7d、30d 或 all",
            );
        }
    };
    let granularity = match query.granularity.as_deref().unwrap_or("auto") {
        "auto" => TrendGranularity::Auto,
        "hour" => TrendGranularity::Hour,
        "day" => TrendGranularity::Day,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_granularity",
                "granularity 必须是 auto、hour 或 day",
            );
        }
    };
    let filters = RuntimeFilter::from(query.filters);
    let request = TrendQuery {
        from: filters.from.unwrap_or(default_from),
        to: filters.to.unwrap_or(default_to),
        granularity,
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: filters,
    };
    match state.inner.engine.runtime_trends(&request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimePagedQuery {
    page: Option<usize>,
    page_size: Option<usize>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    search: Option<String>,
    sort: Option<String>,
    order: Option<String>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

pub(crate) async fn runtime_errors(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let request = ErrorPageQuery {
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(10),
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    };
    match state.inner.engine.runtime_error_groups(&request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

pub(crate) async fn runtime_projects(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(state, DimensionKind::Project, query)
}

pub(crate) async fn runtime_sessions(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    runtime_dimension_response(state, DimensionKind::Session, query)
}

pub(crate) async fn runtime_dimensions(
    State(state): State<AdminState>,
    Query(query): Query<RuntimePagedQuery>,
) -> Response {
    let Some(kind) = parse_dimension_kind(query.filters.kind.as_deref()) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_dimension_kind",
            "kind 必须是 endpoint、model、clientKind、purpose、failureKind、failurePhase、protocol、streamTerminal、project 或 session",
        );
    };
    let mut query = query;
    query.filters.kind = None;
    runtime_dimension_response(state, kind, query)
}

pub(crate) fn parse_dimension_kind(value: Option<&str>) -> Option<DimensionKind> {
    match value? {
        "endpoint" => Some(DimensionKind::Endpoint),
        "model" => Some(DimensionKind::Model),
        "clientKind" | "client_kind" => Some(DimensionKind::ClientKind),
        "clientVariant" | "client_variant" => Some(DimensionKind::ClientVariant),
        "agentRole" | "agent_role" => Some(DimensionKind::AgentRole),
        "agentName" | "agent_name" => Some(DimensionKind::AgentName),
        "parentThread" | "parent_thread_id" => Some(DimensionKind::ParentThread),
        "parentTurn" | "parent_turn_id" => Some(DimensionKind::ParentTurn),
        "rootTurn" | "root_turn_id" => Some(DimensionKind::RootTurn),
        "purpose" | "requestPurpose" | "request_purpose" => Some(DimensionKind::Purpose),
        "failureKind" | "failure_kind" => Some(DimensionKind::FailureKind),
        "failurePhase" | "failure_phase" => Some(DimensionKind::FailurePhase),
        "protocol" => Some(DimensionKind::Protocol),
        "streamTerminal" | "stream_terminal" => Some(DimensionKind::StreamTerminal),
        "project" => Some(DimensionKind::Project),
        "session" => Some(DimensionKind::Session),
        _ => None,
    }
}

pub(crate) fn runtime_dimension_response(
    state: AdminState,
    kind: DimensionKind,
    query: RuntimePagedQuery,
) -> Response {
    let sort = match query.sort.as_deref().unwrap_or("last_seen") {
        "name" => DimensionSort::Name,
        "requests" => DimensionSort::Requests,
        "success_rate" | "successRate" => DimensionSort::SuccessRate,
        "failures" => DimensionSort::Failures,
        "input_tokens" | "inputTokens" => DimensionSort::InputTokens,
        "output_tokens" | "outputTokens" => DimensionSort::OutputTokens,
        "cache_read" | "cacheRead" => DimensionSort::CacheReadTokens,
        "cache_write" | "cacheWrite" => DimensionSort::CacheWriteTokens,
        "tokens" => DimensionSort::Tokens,
        "average_duration" | "averageDuration" => DimensionSort::AverageDuration,
        "last_seen" | "lastSeen" => DimensionSort::LastSeen,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_sort",
                "sort 必须是 name、requests、success_rate、failures、input_tokens、output_tokens、cache_read、cache_write、tokens、average_duration 或 last_seen",
            );
        }
    };
    let order = match query.order.as_deref().unwrap_or("desc") {
        "asc" => SortOrder::Asc,
        "desc" => SortOrder::Desc,
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "invalid_order",
                "order 必须是 asc 或 desc",
            );
        }
    };
    let request = DimensionPageQuery {
        page: query.page.unwrap_or(1),
        page_size: query.page_size.unwrap_or(10),
        search: query.search,
        sort,
        order,
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    };
    match state.inner.engine.runtime_dimension_page(kind, &request) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

pub(crate) async fn runtime_storage(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_storage_details() {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

pub(crate) async fn runtime_retention(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_storage_details() {
        Ok(value) => json_ok(&value["retention"]),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeRetentionPayload {
    expected_revision: i64,
    #[serde(default)]
    max_age_days: Option<i64>,
    #[serde(default)]
    storage_limit_bytes: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RuntimeCleanupPayload {
    /// Apple reference-date seconds; rows strictly older than this cutoff
    /// are eligible when their complete request group is also old.
    older_than: f64,
}

pub(crate) async fn runtime_cleanup_preview(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state
        .inner
        .engine
        .runtime_cleanup_preview(payload.older_than)
    {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => api_error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

pub(crate) async fn runtime_cleanup(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeCleanupPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state.inner.engine.runtime_cleanup(payload.older_than) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("olderThan") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_cleanup", &message)
        }
        Err(message) => api_error(StatusCode::CONFLICT, "runtime_cleanup_failed", &message),
    }
}

pub(crate) async fn runtime_retention_update(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimeRetentionPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    match state
        .inner
        .engine
        .runtime_set_retention(RuntimeRetentionUpdate {
            expected_revision: payload.expected_revision,
            max_age_days: payload.max_age_days,
            storage_limit_bytes: payload.storage_limit_bytes,
        }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            api_error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_retention", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_retention_failed",
            &message,
        ),
    }
}

pub(crate) async fn runtime_pricing(State(state): State<AdminState>) -> Response {
    match state.inner.engine.runtime_pricing() {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeModelPricePayload {
    endpoint_id: Option<String>,
    model_key: String,
    effective_from: f64,
    effective_to: Option<f64>,
    input_per_million_micros: Option<i64>,
    output_per_million_micros: Option<i64>,
    cache_read_per_million_micros: Option<i64>,
    cache_creation_per_million_micros: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimePricingPayload {
    expected_revision: i64,
    currency: String,
    prices: Vec<RuntimeModelPricePayload>,
}

pub(crate) async fn runtime_pricing_update(
    State(state): State<AdminState>,
    payload: JsonPayload<RuntimePricingPayload>,
) -> Response {
    let payload = match require_json(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let prices = payload
        .prices
        .into_iter()
        .map(|price| RuntimeModelPriceInput {
            endpoint_id: price.endpoint_id,
            model_key: price.model_key,
            effective_from: price.effective_from,
            effective_to: price.effective_to,
            input_per_million_micros: price.input_per_million_micros,
            output_per_million_micros: price.output_per_million_micros,
            cache_read_per_million_micros: price.cache_read_per_million_micros,
            cache_creation_per_million_micros: price.cache_creation_per_million_micros,
        })
        .collect();
    match state
        .inner
        .engine
        .runtime_replace_pricing(RuntimePricingUpdate {
            expected_revision: payload.expected_revision,
            currency: payload.currency,
            prices,
        }) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("revision") => {
            api_error(StatusCode::CONFLICT, "runtime_revision_conflict", &message)
        }
        Err(message) if message.contains("必须") || message.contains("must") => {
            api_error(StatusCode::BAD_REQUEST, "invalid_pricing", &message)
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_pricing_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RuntimeExportQuery {
    scope: Option<String>,
    format: Option<String>,
    privacy: Option<String>,
    confirm_stored: Option<bool>,
    snapshot_seq: Option<i64>,
    history_generation: Option<i64>,
    #[serde(flatten)]
    filters: RuntimeFilterQuery,
}

// Err 侧是已构造好的 axum Response,直接返回给调用方;装箱只会多一次堆分配。
#[allow(clippy::result_large_err)]
pub(crate) fn parse_runtime_export_query(
    query: RuntimeExportQuery,
) -> Result<ExportQuery, Response> {
    let scope = match query.scope.as_deref().unwrap_or("events") {
        "events" => ExportScope::Events,
        "projects" => ExportScope::Projects,
        "sessions" => ExportScope::Sessions,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_scope",
                "scope 必须是 events、projects 或 sessions",
            ));
        }
    };
    let format = match query.format.as_deref().unwrap_or("jsonl") {
        "csv" => ExportFormat::Csv,
        "jsonl" => ExportFormat::Jsonl,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_format",
                "format 必须是 csv 或 jsonl",
            ));
        }
    };
    let privacy = match query.privacy.as_deref().unwrap_or("stored") {
        "redacted" => ExportPrivacy::Redacted,
        "stored" => ExportPrivacy::Stored,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "invalid_export_privacy",
                "privacy 必须是 redacted 或 stored",
            ));
        }
    };
    Ok(ExportQuery {
        scope,
        format,
        privacy,
        confirm_stored: query.confirm_stored == Some(true),
        snapshot_seq: query.snapshot_seq,
        history_generation: query.history_generation,
        filter: query.filters.into(),
    })
}

pub(crate) async fn runtime_export_estimate(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    match state.inner.engine.runtime_export_estimate(&query) {
        Ok(value) => json_ok(&value),
        Err(error) => runtime_query_error_response(error),
    }
}

pub(crate) async fn runtime_export(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeExportQuery>,
) -> Response {
    let query = match parse_runtime_export_query(query) {
        Ok(query) => query,
        Err(response) => return response,
    };
    if query.privacy == ExportPrivacy::Stored && !query.confirm_stored {
        return api_error(
            StatusCode::BAD_REQUEST,
            "stored_export_confirmation_required",
            "privacy=stored 必须同时提供 confirmStored=true",
        );
    }
    // Resolve and validate the exact snapshot before response headers are
    // committed. The streaming worker reuses these values, so estimate and
    // export cannot drift to a newer history window.
    let estimate = match state.inner.engine.runtime_export_estimate(&query) {
        Ok(value) => value,
        Err(error) => return runtime_query_error_response(error),
    };
    let snapshot_seq = estimate["snapshotSeq"].as_i64().unwrap_or(0);
    let history_generation = estimate["historyGeneration"].as_i64().unwrap_or(0);
    let row_count = estimate["rowCount"].as_i64().unwrap_or(0);
    let mut stream_query = query.clone();
    stream_query.snapshot_seq = Some(snapshot_seq);
    stream_query.history_generation = Some(history_generation);

    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let engine = state.inner.engine.clone();
    tokio::task::spawn_blocking(move || {
        let worker_sender = sender.clone();
        let result = engine.runtime_stream_export(&stream_query, |chunk| {
            worker_sender
                .blocking_send(Ok(Bytes::from(chunk)))
                .map_err(|_| "export client disconnected".to_string())
        });
        if let Err(error) = result {
            let _ = sender.blocking_send(Err(std::io::Error::other(error.to_string())));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|item| (item, receiver))
    });
    let filename = format!(
        "sumpter-runtime-{}-{}.{}",
        match query.scope {
            ExportScope::Events => "events",
            ExportScope::Projects => "projects",
            ExportScope::Sessions => "sessions",
        },
        match query.privacy {
            ExportPrivacy::Redacted => "redacted",
            ExportPrivacy::Stored => "stored",
        },
        query.format.extension(),
    );
    let mut response = Response::new(Body::from_stream(stream));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(query.format.content_type()),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    for (name, value) in [
        ("x-sumpter-snapshot-seq", snapshot_seq.to_string()),
        (
            "x-sumpter-history-generation",
            history_generation.to_string(),
        ),
        ("x-sumpter-row-count", row_count.to_string()),
        (
            "x-sumpter-privacy",
            match query.privacy {
                ExportPrivacy::Redacted => "redacted".into(),
                ExportPrivacy::Stored => "stored".into(),
            },
        ),
    ] {
        if let Ok(value) = HeaderValue::from_str(&value) {
            headers.insert(HeaderName::from_static(name), value);
        }
    }
    response
}

pub(crate) fn admin_apple_timestamp() -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs_f64() - APPLE_EPOCH_OFFSET_SECS)
        .unwrap_or(0.0)
}

pub(crate) fn utc_today_start(now: f64) -> f64 {
    const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;
    const DAY_SECS: f64 = 86_400.0;
    let unix = now + APPLE_EPOCH_OFFSET_SECS;
    unix - unix.rem_euclid(DAY_SECS) - APPLE_EPOCH_OFFSET_SECS
}

pub(crate) fn runtime_query_error_response(error: RuntimeQueryError) -> Response {
    match error {
        RuntimeQueryError::InvalidInput(message) => {
            api_error(StatusCode::BAD_REQUEST, "invalid_runtime_query", &message)
        }
        RuntimeQueryError::SnapshotExpired { requested, current } => crate::engine::json_response(
            StatusCode::CONFLICT,
            &json!({
                "error": "runtime_snapshot_expired",
                "message": format!("历史快照 {requested} 已失效"),
                "currentHistoryGeneration": current,
            }),
        ),
        RuntimeQueryError::SnapshotTrimmed {
            snapshot_seq,
            retained_from_seq,
        } => crate::engine::json_response(
            StatusCode::CONFLICT,
            &json!({
                "error": "runtime_snapshot_trimmed",
                "message": format!("快照 {snapshot_seq} 涉及已被手动删除的历史记录"),
                "retainedFromSeq": retained_from_seq,
            }),
        ),
        RuntimeQueryError::NotFound(message) => {
            api_error(StatusCode::NOT_FOUND, "runtime_not_found", &message)
        }
        RuntimeQueryError::ProjectionNotReady { backfill_cursor } => crate::engine::json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &json!({
                "error": "runtime_projection_not_ready",
                "message": "历史数据索引正在后台补齐，请稍后重试",
                "backfillCursor": backfill_cursor,
            }),
        ),
        RuntimeQueryError::CorruptPayload { event_id, detail } => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_payload_corrupt",
            &format!("事件 {event_id} 无法读取: {detail}"),
        ),
        RuntimeQueryError::Output(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_export_failed",
            &message,
        ),
        RuntimeQueryError::Sql(error) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &error.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeAnalyticsQuery {
    range: Option<String>,
    #[serde(rename = "clientKind", alias = "client_kind")]
    client_kind: Option<String>,
    #[serde(rename = "clientVariant", alias = "client_variant")]
    client_variant: Option<String>,
    #[serde(rename = "agentRole", alias = "agent_role")]
    agent_role: Option<String>,
    #[serde(rename = "agentName", alias = "agent_name")]
    agent_name: Option<String>,
    #[serde(
        rename = "parentThreadID",
        alias = "parent_thread_id",
        alias = "parentThreadId"
    )]
    parent_thread_id: Option<String>,
    #[serde(
        rename = "parentTurnID",
        alias = "parent_turn_id",
        alias = "parentTurnId"
    )]
    parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", alias = "root_turn_id", alias = "rootTurnId")]
    root_turn_id: Option<String>,
    #[serde(rename = "endpointID", alias = "endpoint_id")]
    endpoint_id: Option<String>,
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: Option<String>,
    project: Option<String>,
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    from: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_optional_f64")]
    to: Option<f64>,
}

pub(crate) fn analytics_filter_value(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub(crate) async fn runtime_analytics(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeAnalyticsQuery>,
) -> Response {
    let range = query.range.as_deref().unwrap_or("24h");
    if !matches!(range, "today" | "1h" | "24h" | "7d" | "30d" | "all") {
        return api_error(
            StatusCode::BAD_REQUEST,
            "invalid_range",
            "range 必须是 today、1h、24h、7d、30d 或 all",
        );
    }
    let filter = AnalyticsFilter {
        client_kind: analytics_filter_value(query.client_kind),
        client_variant: analytics_filter_value(query.client_variant),
        agent_role: analytics_filter_value(query.agent_role),
        agent_name: analytics_filter_value(query.agent_name),
        parent_thread_id: analytics_filter_value(query.parent_thread_id),
        parent_turn_id: analytics_filter_value(query.parent_turn_id),
        root_turn_id: analytics_filter_value(query.root_turn_id),
        endpoint_id: analytics_filter_value(query.endpoint_id),
        project_id: analytics_filter_value(query.project_id),
        project: analytics_filter_value(query.project),
        session_id: analytics_filter_value(query.session_id),
        from: query
            .from
            .or_else(|| (range == "today").then(|| utc_today_start(admin_apple_timestamp()))),
        to: query.to,
    };
    match state
        .inner
        .engine
        .runtime_analytics_filtered(range, &filter)
    {
        Ok(value) => json_ok(&value),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "runtime_query_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeSessionQuery {
    #[serde(rename = "sessionID", alias = "session_id")]
    session_id: Option<String>,
    #[serde(rename = "confirmUnidentified", alias = "confirm_unidentified")]
    confirm_unidentified: Option<bool>,
}

pub(crate) async fn delete_runtime_session(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = analytics_filter_value(query.session_id) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match state
        .inner
        .engine
        .delete_runtime_session_confirmed(&session_id, query.confirm_unidentified == Some(true))
    {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("不能删除未识别会话") => {
            api_error(StatusCode::BAD_REQUEST, "session_not_deletable", &message)
        }
        Err(message) if message.contains("Query returned no rows") => {
            api_error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_delete_failed",
            &message,
        ),
    }
}

pub(crate) async fn export_runtime_session(
    State(state): State<AdminState>,
    Query(query): Query<RuntimeSessionQuery>,
) -> Response {
    let Some(session_id) = analytics_filter_value(query.session_id) else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "session_id_required",
            "必须提供完整 sessionID",
        );
    };
    match state.inner.engine.export_runtime_session(&session_id) {
        Ok(value) => json_ok(&value),
        Err(message) if message.contains("会话不存在") => {
            api_error(StatusCode::NOT_FOUND, "session_not_found", "会话不存在")
        }
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "session_export_failed",
            &message,
        ),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProjectStickyBody {
    #[serde(rename = "projectID", alias = "project_id")]
    project_id: String,
}

pub(crate) async fn clear_project_sticky(
    State(state): State<AdminState>,
    payload: JsonPayload<ProjectStickyBody>,
) -> Response {
    let body = match require_json(payload) {
        Ok(body) => body,
        Err(response) => return response,
    };
    let project_id = body.project_id.trim().to_owned();
    if project_id.is_empty() {
        return api_error(
            StatusCode::BAD_REQUEST,
            "project_id_required",
            "必须提供 projectID",
        );
    }
    match state.inner.engine.clear_project_sticky(&project_id) {
        Ok(value) => json_ok(&value),
        Err(message) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "sticky_clear_failed",
            &message,
        ),
    }
}

pub(crate) async fn reset_runtime(State(state): State<AdminState>) -> Response {
    match state.inner.engine.reset_runtime() {
        Ok(_) => json_ok(
            &json!({"reset": true, "resetGeneration": state.inner.engine.runtime_summary_value()["resetGeneration"]}),
        ),
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_reset_failed",
                &message,
            )
        }
    }
}

pub(crate) async fn recreate_runtime(State(state): State<AdminState>) -> Response {
    match state.inner.engine.recreate_runtime() {
        Ok(generation) => json_ok(&json!({
            "reset": true,
            "recreated": true,
            "resetGeneration": generation,
        })),
        Err(message) => {
            state.inner.engine.set_last_error(Some(message.clone()));
            api_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "stats_recreate_failed",
                &message,
            )
        }
    }
}
