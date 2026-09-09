//! Runtime query facets domain.

use crate::database::params_from_iter;

use super::{
    API_VERSION, AnalyticsFacetRow, AnalyticsFacets, Connection, HashMap, PROJECTION_VERSION, Path,
    QueryResult, RuntimeFacetSnapshot, RuntimeFilter, SqlFilter, Transaction, TransactionBehavior,
    history_snapshot, read_connection, require_projection,
};

/// Read only the picker dimensions in one transaction. The old
/// `/runtime/analytics` response still contains this data for compatibility,
/// but it also computes every aggregate/table; this path is the hot refresh
/// path used by both dashboards.
pub fn facets(path: &Path, filter: &RuntimeFilter) -> QueryResult<RuntimeFacetSnapshot> {
    let mut connection = read_connection(path)?;
    facets_on(&mut connection, filter)
}

pub fn facets_on(
    connection: &mut Connection,
    filter: &RuntimeFilter,
) -> QueryResult<RuntimeFacetSnapshot> {
    let filters = filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, None, None)?;
    let facets = analytics_facets_for_filter(&transaction, &filters, snapshot.snapshot_seq)?;
    transaction.commit()?;
    Ok(RuntimeFacetSnapshot {
        api_version: API_VERSION,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
        facets,
    })
}

pub(super) fn analytics_facets_for_filter(
    transaction: &Transaction<'_>,
    filters: &RuntimeFilter,
    snapshot_seq: i64,
) -> QueryResult<AnalyticsFacets> {
    // A facet ignores its own active filter while retaining every other
    // filter. This prevents a selected value from making the next picker
    // appear empty; the selected value is retained with count 0 when the
    // remaining filters are incompatible.
    let mut client_variant_facet_filter = filters.clone();
    client_variant_facet_filter.client_variant = None;
    let mut agent_role_facet_filter = filters.clone();
    agent_role_facet_filter.agent_role = None;
    let mut agent_name_facet_filter = filters.clone();
    agent_name_facet_filter.agent_name = None;
    let mut parent_thread_id_facet_filter = filters.clone();
    parent_thread_id_facet_filter.parent_thread_id = None;
    let mut parent_turn_id_facet_filter = filters.clone();
    parent_turn_id_facet_filter.parent_turn_id = None;
    let mut root_turn_id_facet_filter = filters.clone();
    root_turn_id_facet_filter.root_turn_id = None;
    let mut client_facet_filter = filters.clone();
    client_facet_filter.client_kind = None;
    let mut endpoint_facet_filter = filters.clone();
    endpoint_facet_filter.endpoint_id = None;
    let mut project_facet_filter = filters.clone();
    project_facet_filter.project_id = None;
    project_facet_filter.project_name = None;
    let mut session_facet_filter = filters.clone();
    session_facet_filter.session_id = None;
    let mut model_facet_filter = filters.clone();
    model_facet_filter.model = None;
    let mut purpose_facet_filter = filters.clone();
    purpose_facet_filter.request_purpose = None;
    let mut failure_kind_facet_filter = filters.clone();
    failure_kind_facet_filter.failure_kind = None;
    let mut failure_phase_facet_filter = filters.clone();
    failure_phase_facet_filter.failure_phase = None;
    // Read the scalar projection once and derive all facet maps in
    // memory. The previous implementation issued four independent GROUP BY
    // queries, each of which scanned the same client history and built
    // temporary B-trees. Keep the exact "ignore this facet's own filter"
    // semantics, but push the common predicates (especially the selected
    // time range) into SQLite first. A 24-hour refresh must not deserialize
    // every event ever retained in an `all`-range database; payload_json is
    // never touched on this path.
    let mut base = SqlFilter::default();
    base.raw("is_in_flight = 0");
    base.raw(format!("projection_version = {PROJECTION_VERSION}"));
    base.le_i64("seq", snapshot_seq);
    if filters.kind.as_deref().is_some_and(|kind| kind != "client") {
        // Preserve the legacy empty-facet behavior for an upstream-only
        // query, including the selected-value placeholders added below.
        base.raw("0");
    } else {
        base.raw("kind = 'client'");
    }
    // Predicates that are not selectable facets are safe to push down. Every
    // selectable dimension stays in `facet_row_matches`, because each picker
    // must ignore its own active value while retaining the other conditions.
    base.eq_text("outcome", filters.outcome.as_deref());
    base.eq_text("request_id", filters.request_id.as_deref());
    base.ge_real("timestamp", filters.from);
    base.le_real("timestamp", filters.to);
    let sql = format!(
        "SELECT outcome,client_kind,request_purpose,request_id,endpoint_id,
                effective_model,project_id,project_name,session_key,
                failure_kind,failure_phase,client_variant,agent_role,agent_name,parent_thread_id,parent_turn_id,root_turn_id,
                attribution_scope,timestamp
           FROM runtime_events{}",
        base.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(base.values.iter()), |row| {
        Ok(FacetProjectionRow {
            outcome: row.get(0)?,
            client_kind: row.get(1)?,
            request_purpose: row.get(2)?,
            request_id: row.get(3)?,
            endpoint_id: row.get(4)?,
            model: row.get(5)?,
            project_id: row.get(6)?,
            project_name: row.get(7)?,
            session_id: row.get(8)?,
            failure_kind: row.get(9)?,
            failure_phase: row.get(10)?,
            client_variant: row.get(11)?,
            agent_role: row.get(12)?,
            agent_name: row.get(13)?,
            parent_thread_id: row.get(14)?,
            parent_turn_id: row.get(15)?,
            root_turn_id: row.get(16)?,
            attribution_scope: row.get(17)?,
            timestamp: row.get(18)?,
        })
    })?;
    let mut client_counts = HashMap::<String, i64>::new();
    let mut client_variant_counts = HashMap::<String, i64>::new();
    let mut agent_role_counts = HashMap::<String, i64>::new();
    let mut agent_name_counts = HashMap::<String, i64>::new();
    let mut parent_thread_counts = HashMap::<String, i64>::new();
    let mut parent_turn_counts = HashMap::<String, i64>::new();
    let mut root_turn_counts = HashMap::<String, i64>::new();
    let mut endpoint_counts = HashMap::<String, i64>::new();
    let mut project_counts = HashMap::<String, i64>::new();
    let mut session_counts = HashMap::<String, i64>::new();
    let mut model_counts = HashMap::<String, i64>::new();
    let mut purpose_counts = HashMap::<String, i64>::new();
    let mut failure_kind_counts = HashMap::<String, i64>::new();
    let mut failure_phase_counts = HashMap::<String, i64>::new();
    for row in rows {
        let row = row?;
        if facet_row_matches(&row, &root_turn_id_facet_filter, FacetDimension::RootTurn) {
            increment_facet(
                &mut root_turn_counts,
                row.root_turn_id.as_deref(),
                "unknown",
            );
        }
        if facet_row_matches(
            &row,
            &parent_turn_id_facet_filter,
            FacetDimension::ParentTurn,
        ) {
            increment_facet(
                &mut parent_turn_counts,
                row.parent_turn_id.as_deref(),
                "unknown",
            );
        }
        if facet_row_matches(
            &row,
            &parent_thread_id_facet_filter,
            FacetDimension::ParentThread,
        ) {
            increment_facet(
                &mut parent_thread_counts,
                row.parent_thread_id.as_deref(),
                "unknown",
            );
        }
        if facet_row_matches(&row, &agent_name_facet_filter, FacetDimension::AgentName) {
            increment_facet(&mut agent_name_counts, row.agent_name.as_deref(), "unknown");
        }
        if facet_row_matches(&row, &agent_role_facet_filter, FacetDimension::AgentRole) {
            increment_facet(&mut agent_role_counts, row.agent_role.as_deref(), "unknown");
        }
        if facet_row_matches(
            &row,
            &client_variant_facet_filter,
            FacetDimension::ClientVariant,
        ) {
            increment_facet(
                &mut client_variant_counts,
                row.client_variant.as_deref(),
                "unknown",
            );
        }
        if facet_row_matches(&row, &client_facet_filter, FacetDimension::Client) {
            increment_facet(
                &mut client_counts,
                row.client_kind.as_deref(),
                "unrecorded_client",
            );
        }
        // An entry facet represents real provider routing only. Requests
        // rejected before routing (for example inbound 401s) have no
        // endpoint_id and must remain visible in client/failed totals without
        // becoming a synthetic "unassigned_endpoint" entry.
        if let Some(endpoint_id) = row.endpoint_id.as_deref()
            && facet_row_matches(&row, &endpoint_facet_filter, FacetDimension::Endpoint)
        {
            *endpoint_counts.entry(endpoint_id.to_owned()).or_default() += 1;
        }
        if row.attribution_scope.as_deref() != Some("internal_feature")
            && facet_row_matches(&row, &project_facet_filter, FacetDimension::Project)
        {
            increment_facet(
                &mut project_counts,
                row.project_name.as_deref().or(row.project_id.as_deref()),
                "unidentified_project",
            );
        }
        if facet_row_matches(&row, &session_facet_filter, FacetDimension::Session) {
            increment_facet(
                &mut session_counts,
                row.session_id.as_deref(),
                "unidentified_session",
            );
        }
        if facet_row_matches(&row, &model_facet_filter, FacetDimension::Model) {
            increment_facet(&mut model_counts, row.model.as_deref(), "unrecorded_model");
        }
        if facet_row_matches(&row, &purpose_facet_filter, FacetDimension::Purpose) {
            increment_facet(
                &mut purpose_counts,
                row.request_purpose.as_deref(),
                "unrecorded_purpose",
            );
        }
        if facet_row_matches(
            &row,
            &failure_kind_facet_filter,
            FacetDimension::FailureKind,
        ) {
            increment_facet(
                &mut failure_kind_counts,
                row.failure_kind.as_deref(),
                "unclassified_failure",
            );
        }
        if facet_row_matches(
            &row,
            &failure_phase_facet_filter,
            FacetDimension::FailurePhase,
        ) {
            increment_facet(
                &mut failure_phase_counts,
                row.failure_phase.as_deref(),
                "unclassified_phase",
            );
        }
    }
    let mut client_kinds = facet_rows_from_counts(client_counts);
    let mut client_variants = facet_rows_from_counts(client_variant_counts);
    let mut agent_roles = facet_rows_from_counts(agent_role_counts);
    let mut agent_names = facet_rows_from_counts(agent_name_counts);
    let mut parent_threads = facet_rows_from_counts(parent_thread_counts);
    let mut parent_turns = facet_rows_from_counts(parent_turn_counts);
    let mut root_turns = facet_rows_from_counts(root_turn_counts);
    let mut endpoints = facet_rows_from_counts(endpoint_counts);
    let mut projects = facet_rows_from_counts(project_counts);
    let mut sessions = facet_rows_from_counts(session_counts);
    let mut models = facet_rows_from_counts(model_counts);
    let mut request_purposes = facet_rows_from_counts(purpose_counts);
    let mut failure_kinds = facet_rows_from_counts(failure_kind_counts);
    let mut failure_phases = facet_rows_from_counts(failure_phase_counts);
    ensure_facet_selection(&mut client_kinds, filters.client_kind.as_deref());
    ensure_facet_selection(&mut client_variants, filters.client_variant.as_deref());
    ensure_facet_selection(&mut agent_roles, filters.agent_role.as_deref());
    ensure_facet_selection(&mut agent_names, filters.agent_name.as_deref());
    ensure_facet_selection(&mut parent_threads, filters.parent_thread_id.as_deref());
    ensure_facet_selection(&mut parent_turns, filters.parent_turn_id.as_deref());
    ensure_facet_selection(&mut root_turns, filters.root_turn_id.as_deref());
    ensure_facet_selection(&mut endpoints, filters.endpoint_id.as_deref());
    ensure_facet_selection(
        &mut projects,
        filters
            .project_name
            .as_deref()
            .or(filters.project_id.as_deref()),
    );
    ensure_facet_selection(&mut sessions, filters.session_id.as_deref());
    ensure_facet_selection(&mut models, filters.model.as_deref());
    ensure_facet_selection(&mut request_purposes, filters.request_purpose.as_deref());
    ensure_facet_selection(&mut failure_kinds, filters.failure_kind.as_deref());
    ensure_facet_selection(&mut failure_phases, filters.failure_phase.as_deref());
    Ok(AnalyticsFacets {
        client_kinds,
        client_variants,
        agent_roles,
        agent_names,
        parent_threads,
        parent_turns,
        root_turns,
        endpoints,
        projects,
        sessions,
        models,
        request_purposes,
        failure_kinds,
        failure_phases,
    })
}

#[derive(Debug)]
struct FacetProjectionRow {
    outcome: Option<String>,
    client_kind: Option<String>,
    request_purpose: Option<String>,
    request_id: Option<String>,
    endpoint_id: Option<String>,
    model: Option<String>,
    project_id: Option<String>,
    project_name: Option<String>,
    session_id: Option<String>,
    failure_kind: Option<String>,
    failure_phase: Option<String>,
    client_variant: Option<String>,
    agent_role: Option<String>,
    agent_name: Option<String>,
    parent_thread_id: Option<String>,
    parent_turn_id: Option<String>,
    root_turn_id: Option<String>,
    attribution_scope: Option<String>,
    timestamp: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FacetDimension {
    ClientVariant,
    AgentRole,
    AgentName,
    ParentThread,
    ParentTurn,
    RootTurn,
    Client,
    Endpoint,
    Project,
    Session,
    Model,
    Purpose,
    FailureKind,
    FailurePhase,
}

fn facet_row_matches(
    row: &FacetProjectionRow,
    filter: &RuntimeFilter,
    ignored: FacetDimension,
) -> bool {
    if filter.kind.as_deref().is_some_and(|kind| kind != "client") {
        return false;
    }
    let matches = |value: &Option<String>, expected: Option<&String>| {
        expected.is_none_or(|expected| value.as_deref() == Some(expected.as_str()))
    };
    if !matches(&row.outcome, filter.outcome.as_ref())
        || !matches(&row.request_purpose, filter.request_purpose.as_ref())
        || !matches(&row.request_id, filter.request_id.as_ref())
        || !matches(&row.model, filter.model.as_ref())
        || !matches(&row.failure_kind, filter.failure_kind.as_ref())
        || !matches(&row.failure_phase, filter.failure_phase.as_ref())
        || filter.from.is_some_and(|from| row.timestamp < from)
        || filter.to.is_some_and(|to| row.timestamp > to)
    {
        return false;
    }
    let matches_attribution =
        |value: &Option<String>, expected: Option<&String>, dimension: FacetDimension| {
            ignored == dimension
                || expected.is_none_or(|expected| value.as_deref().unwrap_or("unknown") == expected)
        };
    if !matches_attribution(
        &row.client_variant,
        filter.client_variant.as_ref(),
        FacetDimension::ClientVariant,
    ) || !matches_attribution(
        &row.agent_role,
        filter.agent_role.as_ref(),
        FacetDimension::AgentRole,
    ) || !matches_attribution(
        &row.agent_name,
        filter.agent_name.as_ref(),
        FacetDimension::AgentName,
    ) || !matches_attribution(
        &row.parent_thread_id,
        filter.parent_thread_id.as_ref(),
        FacetDimension::ParentThread,
    ) || !matches_attribution(
        &row.parent_turn_id,
        filter.parent_turn_id.as_ref(),
        FacetDimension::ParentTurn,
    ) || !matches_attribution(
        &row.root_turn_id,
        filter.root_turn_id.as_ref(),
        FacetDimension::RootTurn,
    ) {
        return false;
    }
    if !matches(&row.client_kind, filter.client_kind.as_ref())
        && !matches!(ignored, FacetDimension::Client)
    {
        return false;
    }
    if !matches(&row.endpoint_id, filter.endpoint_id.as_ref())
        && !matches!(ignored, FacetDimension::Endpoint)
    {
        return false;
    }
    if !matches(&row.project_id, filter.project_id.as_ref())
        && !matches!(ignored, FacetDimension::Project)
    {
        return false;
    }
    if !matches(&row.project_name, filter.project_name.as_ref())
        && !matches!(ignored, FacetDimension::Project)
    {
        return false;
    }
    if !matches(&row.session_id, filter.session_id.as_ref())
        && !matches!(ignored, FacetDimension::Session)
    {
        return false;
    }
    if !matches(&row.model, filter.model.as_ref()) && !matches!(ignored, FacetDimension::Model) {
        return false;
    }
    if !matches(&row.request_purpose, filter.request_purpose.as_ref())
        && !matches!(ignored, FacetDimension::Purpose)
    {
        return false;
    }
    if !matches(&row.failure_kind, filter.failure_kind.as_ref())
        && !matches!(ignored, FacetDimension::FailureKind)
    {
        return false;
    }
    if !matches(&row.failure_phase, filter.failure_phase.as_ref())
        && !matches!(ignored, FacetDimension::FailurePhase)
    {
        return false;
    }
    true
}

fn increment_facet(counts: &mut HashMap<String, i64>, value: Option<&str>, fallback: &str) {
    let key = value.unwrap_or(fallback).to_owned();
    *counts.entry(key).or_default() += 1;
}

fn facet_rows_from_counts(counts: HashMap<String, i64>) -> Vec<AnalyticsFacetRow> {
    let mut rows = counts
        .into_iter()
        .map(|(value, count)| AnalyticsFacetRow { value, count })
        .collect::<Vec<_>>();
    rows.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.value.cmp(&right.value))
    });
    rows.truncate(500);
    rows
}

fn ensure_facet_selection(rows: &mut Vec<AnalyticsFacetRow>, selected: Option<&str>) {
    let Some(selected) = selected else { return };
    if rows.iter().any(|row| row.value == selected) {
        return;
    }
    rows.insert(
        0,
        AnalyticsFacetRow {
            value: selected.to_owned(),
            count: 0,
        },
    );
}
