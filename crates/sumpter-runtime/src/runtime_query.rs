//! Read-only, projection-backed runtime history queries.
//!
//! This module deliberately accepts a SQLite [`Connection`] instead of a
//! [`RuntimeStore`](crate::runtime_store::RuntimeStore).  The write worker owns
//! schema upgrades and projection backfill; Admin can keep its compatibility
//! endpoints while opting individual v2 routes into these bounded queries.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::database::types::Value as SqlValue;
use crate::database::{Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::{
    ClientKind, RuntimeEventOutcome, RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase,
};
use sumpter_core::routing::{RequestPurpose, RouteMode};

use crate::runtime_store::{RuntimeChange, RuntimeEventListItem};

const API_VERSION: u8 = 3;
use crate::runtime_store::PROJECTION_VERSION;
const PAGE_SIZES: [usize; 5] = [10, 25, 50, 100, 200];
const REQUEST_CHAIN_LIMIT: usize = 512;
const MAX_TREND_POINTS: usize = 240;
const SCOPED_PRICE_SEPARATOR: char = '\u{1f}';
const TTFB_SLOW_MS: i64 = 5_000;
const TTFB_CRITICAL_MS: i64 = 15_000;
const DURATION_SLOW_MS: i64 = 3_000;
const DURATION_CRITICAL_MS: i64 = 6_000;

fn read_connection(path: &Path) -> QueryResult<Connection> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA foreign_keys = ON; PRAGMA cache_size = -2048;")?;
    Ok(connection)
}

#[derive(Debug)]
pub enum RuntimeQueryError {
    InvalidInput(String),
    SnapshotExpired {
        requested: i64,
        current: i64,
    },
    SnapshotTrimmed {
        snapshot_seq: i64,
        retained_from_seq: i64,
    },
    NotFound(String),
    ProjectionNotReady {
        backfill_cursor: i64,
    },
    CorruptPayload {
        event_id: String,
        detail: String,
    },
    Output(String),
    Sql(crate::database::Error),
}

impl fmt::Display for RuntimeQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => formatter.write_str(message),
            Self::SnapshotExpired { requested, current } => write!(
                formatter,
                "runtime snapshot generation {requested} expired; current generation is {current}"
            ),
            Self::SnapshotTrimmed {
                snapshot_seq,
                retained_from_seq,
            } => write!(
                formatter,
                "runtime snapshot {snapshot_seq} reaches manually deleted history; retained rows start at {retained_from_seq}"
            ),
            Self::NotFound(message) => formatter.write_str(message),
            Self::ProjectionNotReady { backfill_cursor } => write!(
                formatter,
                "runtime projection backfill is incomplete at seq {backfill_cursor}"
            ),
            Self::CorruptPayload { event_id, detail } => {
                write!(
                    formatter,
                    "runtime event {event_id} has invalid payload: {detail}"
                )
            }
            Self::Output(message) => formatter.write_str(message),
            Self::Sql(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for RuntimeQueryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sql(error) => Some(error),
            _ => None,
        }
    }
}

impl From<crate::database::Error> for RuntimeQueryError {
    fn from(error: crate::database::Error) -> Self {
        Self::Sql(error)
    }
}

pub type QueryResult<T> = Result<T, RuntimeQueryError>;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFilter {
    pub kind: Option<String>,
    pub outcome: Option<String>,
    pub client_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentThreadID")]
    pub parent_thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentTurnID")]
    pub parent_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rootTurnID")]
    pub root_turn_id: Option<String>,
    pub request_purpose: Option<String>,
    #[serde(rename = "requestID")]
    pub request_id: Option<String>,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    pub model: Option<String>,
    #[serde(rename = "projectID")]
    pub project_id: Option<String>,
    /// Human-readable project-name compatibility alias. Keep it independent
    /// from the stable hashed `projectID`; when both are present the query
    /// applies them as an AND filter. Serializing the alias keeps the
    /// `filters`/`appliedFilters` response truthful for clients that use the
    /// display-name picker.
    #[serde(rename = "project")]
    pub project_name: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
    pub failure_kind: Option<String>,
    pub failure_phase: Option<String>,
    pub from: Option<f64>,
    pub to: Option<f64>,
}

/// The analytics response reports the user-facing dimension filters that can
/// be changed from the statistics control bar. Time/range and technical
/// predicates are applied to the query but are not echoed as facet state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsAppliedFilters {
    pub client_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_variant: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentThreadID")]
    pub parent_thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "parentTurnID")]
    pub parent_turn_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "rootTurnID")]
    pub root_turn_id: Option<String>,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "projectID")]
    pub project_id: Option<String>,
    pub project: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
}

impl RuntimeFilter {
    fn has_agent_filter(&self) -> bool {
        self.client_variant.is_some()
            || self.agent_role.is_some()
            || self.agent_name.is_some()
            || self.parent_thread_id.is_some()
            || self.parent_turn_id.is_some()
            || self.root_turn_id.is_some()
    }

    pub fn normalized(&self) -> QueryResult<Self> {
        fn string(value: &Option<String>, label: &str) -> QueryResult<Option<String>> {
            let value = value
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty());
            if value.is_some_and(|value| value.len() > 512) {
                return Err(RuntimeQueryError::InvalidInput(format!(
                    "{label} must not exceed 512 bytes"
                )));
            }
            Ok(value.map(ToOwned::to_owned))
        }

        if self.from.is_some_and(|value| !value.is_finite())
            || self.to.is_some_and(|value| !value.is_finite())
        {
            return Err(RuntimeQueryError::InvalidInput(
                "from/to must be finite timestamps".into(),
            ));
        }
        if self.from.zip(self.to).is_some_and(|(from, to)| from > to) {
            return Err(RuntimeQueryError::InvalidInput(
                "from must not be later than to".into(),
            ));
        }
        Ok(Self {
            kind: string(&self.kind, "kind")?,
            outcome: string(&self.outcome, "outcome")?,
            client_kind: string(&self.client_kind, "clientKind")?,
            client_variant: string(&self.client_variant, "clientVariant")?,
            agent_role: string(&self.agent_role, "agentRole")?,
            agent_name: string(&self.agent_name, "agentName")?,
            parent_thread_id: string(&self.parent_thread_id, "parentThreadID")?,
            parent_turn_id: string(&self.parent_turn_id, "parentTurnID")?,
            root_turn_id: string(&self.root_turn_id, "rootTurnID")?,
            request_purpose: string(&self.request_purpose, "requestPurpose")?,
            request_id: string(&self.request_id, "requestID")?,
            endpoint_id: string(&self.endpoint_id, "endpointID")?,
            model: string(&self.model, "model")?,
            project_id: string(&self.project_id, "projectID")?,
            project_name: string(&self.project_name, "project")?,
            session_id: string(&self.session_id, "sessionID")?,
            failure_kind: string(&self.failure_kind, "failureKind")?,
            failure_phase: string(&self.failure_phase, "failurePhase")?,
            from: self.from,
            to: self.to,
        })
    }
}

/// Combine a caller-provided lower bound with the selected range's default.
/// `today` uses a calendar-day fallback only when the caller omitted `from`;
/// the explicit value is a local-midnight boundary that the daemon cannot
/// reinterpret in its own timezone. Relative windows still take the
/// intersection so an old `from` cannot widen a `24h`/`7d` query.
pub fn merge_range_lower_bound(
    range: &str,
    existing: Option<f64>,
    range_from: Option<f64>,
) -> Option<f64> {
    match (existing, range_from) {
        (Some(existing), Some(_)) if range == "today" => Some(existing),
        (Some(existing), Some(range_from)) => Some(existing.max(range_from)),
        (None, range_from) => range_from,
        (existing, None) => existing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistorySnapshot {
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub reset_generation: i64,
    pub retained_from_seq: i64,
}

#[derive(Debug, Clone)]
pub struct EventPageRequest {
    pub page: usize,
    pub page_size: usize,
    pub snapshot_seq: Option<i64>,
    pub history_generation: Option<i64>,
    pub filter: RuntimeFilter,
}

pub type EventPageQuery = EventPageRequest;

impl Default for EventPageRequest {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 10,
            snapshot_seq: None,
            history_generation: None,
            filter: RuntimeFilter::default(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventPage {
    pub api_version: u8,
    pub events: Vec<RuntimeEventListItem>,
    pub page: usize,
    pub page_size: usize,
    pub total_count: i64,
    pub total_pages: usize,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub reset_generation: i64,
    pub retained_from_seq: i64,
    pub has_next: bool,
    pub has_previous: bool,
    pub next_cursor: Option<i64>,
    pub previous_cursor: Option<i64>,
    pub filters: RuntimeFilter,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChain {
    pub api_version: u8,
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub events: Vec<RuntimeEventListItem>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TrendGranularity {
    #[default]
    Auto,
    Hour,
    Day,
    #[serde(rename = "multi_day")]
    MultiDay,
}

#[derive(Debug, Clone)]
pub struct TrendRequest {
    pub from: f64,
    pub to: f64,
    pub granularity: TrendGranularity,
    pub snapshot_seq: Option<i64>,
    pub history_generation: Option<i64>,
    pub filter: RuntimeFilter,
}

pub type TrendQuery = TrendRequest;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyThresholdBucket {
    #[serde(rename = "thresholdMS")]
    pub threshold_ms: i64,
    pub exceeded_requests: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyMetrics {
    pub observed_requests: i64,
    #[serde(rename = "sumMS")]
    pub sum_ms: i64,
    #[serde(rename = "averageMS")]
    pub average_ms: Option<f64>,
    pub threshold_buckets: Vec<LatencyThresholdBucket>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LatencyThresholds {
    #[serde(rename = "ttfbMS")]
    pub ttfb_ms: [i64; 2],
    #[serde(rename = "durationMS")]
    pub duration_ms: [i64; 2],
}

impl Default for LatencyThresholds {
    fn default() -> Self {
        Self {
            ttfb_ms: [TTFB_SLOW_MS, TTFB_CRITICAL_MS],
            duration_ms: [DURATION_SLOW_MS, DURATION_CRITICAL_MS],
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenMetrics {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_tokens: i64,
    pub uncached_input_tokens: i64,
    pub processed_input_tokens: i64,
    pub processed_total_tokens: i64,
    pub total_tokens: i64,
    pub observed_requests: i64,
    pub accounting_known_requests: i64,
    pub accounting_unknown_requests: i64,
    pub cache_read_reported_requests: i64,
    pub cache_read_hit_requests: i64,
    pub cache_read_token_eligible_requests: i64,
    pub cache_read_token_unknown_requests: i64,
    pub cache_read_token_rate: Option<f64>,
    pub cache_read_request_rate: Option<f64>,
    pub token_accounting_semantics: String,
    pub token_accounting_quality: String,
    pub usage_field_presence: UsageFieldPresence,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageFieldPresence {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_tokens: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CostCoverage {
    pub estimated_cost_micros: i64,
    pub priced_requests: i64,
    pub unpriced_requests: i64,
    pub unknown_accounting_requests: i64,
    pub complete: bool,
    pub currency: Option<String>,
    pub price_version: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendPoint {
    pub bucket_start: f64,
    pub bucket_end: f64,
    pub client_requests: i64,
    pub client_successes: i64,
    pub client_failures: i64,
    pub client_cancelled: i64,
    pub client_terminal_requests: i64,
    pub client_unknown_results: i64,
    pub failovers: i64,
    pub failover_terminal_requests: i64,
    pub failover_recovered_requests: i64,
    pub failover_recovery_rate: Option<f64>,
    pub upstream_attempts: i64,
    pub upstream_successes: i64,
    pub upstream_failures: i64,
    pub tokens: TokenMetrics,
    #[serde(rename = "ttfbMS")]
    pub ttfb_ms: LatencyMetrics,
    #[serde(rename = "durationMS")]
    pub duration_ms: LatencyMetrics,
    pub cost: CostCoverage,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrendSeries {
    pub api_version: u8,
    pub rollup_used: bool,
    pub granularity: TrendGranularity,
    /// Width of each returned bucket in seconds.  Long ranges use a stable
    /// multi-day bucket instead of failing once the 240-point chart limit is
    /// reached.
    pub bucket_seconds: i64,
    pub from: f64,
    pub to: f64,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub retained_from_seq: i64,
    pub thresholds: LatencyThresholds,
    pub points: Vec<TrendPoint>,
    pub totals: TrendPoint,
    pub filters: RuntimeFilter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsDimensionRow {
    pub name: String,
    pub attempts: i64,
    pub successes: i64,
    pub failures: i64,
    pub cancelled: i64,
    pub pending: i64,
    pub failovers: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub reasoning_tokens: i64,
    pub uncached_input_tokens: i64,
    pub processed_input_tokens: i64,
    pub processed_total_tokens: i64,
    pub total_tokens: i64,
    pub cache_read_reported_requests: i64,
    pub cache_read_hit_requests: i64,
    pub cache_read_token_eligible_requests: i64,
    pub cache_read_token_unknown_requests: i64,
    pub cache_read_token_rate: Option<f64>,
    pub cache_read_request_rate: Option<f64>,
    /// Project attribution is populated for project rows; session rows also
    /// expose the related project/client labels. These values come from
    /// normalized projection columns, never from payload_json.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_kinds: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsFacetRow {
    pub value: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRow {
    pub name: String,
    pub count: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsFacets {
    pub client_kinds: Vec<AnalyticsFacetRow>,
    pub client_variants: Vec<AnalyticsFacetRow>,
    pub agent_roles: Vec<AnalyticsFacetRow>,
    pub agent_names: Vec<AnalyticsFacetRow>,
    pub parent_threads: Vec<AnalyticsFacetRow>,
    pub parent_turns: Vec<AnalyticsFacetRow>,
    pub root_turns: Vec<AnalyticsFacetRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub endpoints: Vec<AnalyticsFacetRow>,
    pub projects: Vec<AnalyticsFacetRow>,
    pub sessions: Vec<AnalyticsFacetRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<AnalyticsFacetRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub request_purposes: Vec<AnalyticsFacetRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_kinds: Vec<AnalyticsFacetRow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub failure_phases: Vec<AnalyticsFacetRow>,
}

/// Lightweight picker snapshot. Unlike `AnalyticsSummary` this response
/// never computes token/cost/tool-call aggregates and is intentionally served
/// by one SQLite read transaction for all filter dimensions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeFacetSnapshot {
    pub api_version: u8,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub retained_from_seq: i64,
    pub facets: AnalyticsFacets,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyticsSummary {
    pub api_version: u8,
    pub range: String,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub client_requests: i64,
    pub client_successes: i64,
    pub client_failures: i64,
    pub client_cancelled: i64,
    pub client_pending: i64,
    pub client_success_rate: Option<f64>,
    pub upstream_attempts: i64,
    pub upstream_successes: i64,
    pub upstream_failures: i64,
    pub failovers: i64,
    #[serde(rename = "averageDurationMS")]
    pub average_duration_ms: Option<f64>,
    #[serde(rename = "averageTTFBMS")]
    pub average_ttfb_ms: Option<f64>,
    pub latency_buckets: BTreeMap<String, i64>,
    pub token_usage: TokenMetrics,
    pub endpoints: Vec<AnalyticsDimensionRow>,
    pub models: Vec<AnalyticsDimensionRow>,
    pub client_kinds: Vec<AnalyticsDimensionRow>,
    pub client_variants: Vec<AnalyticsDimensionRow>,
    pub agent_roles: Vec<AnalyticsDimensionRow>,
    pub agent_names: Vec<AnalyticsDimensionRow>,
    pub parent_threads: Vec<AnalyticsDimensionRow>,
    pub parent_turns: Vec<AnalyticsDimensionRow>,
    pub root_turns: Vec<AnalyticsDimensionRow>,

    pub request_purposes: Vec<AnalyticsDimensionRow>,
    pub feature_rules: Vec<AnalyticsDimensionRow>,
    pub protocol_routes: Vec<AnalyticsDimensionRow>,
    pub failure_kinds: Vec<AnalyticsDimensionRow>,
    pub failure_phases: Vec<AnalyticsDimensionRow>,
    pub upstream_statuses: Vec<AnalyticsDimensionRow>,
    pub stream_terminals: Vec<AnalyticsDimensionRow>,
    pub projects: Vec<AnalyticsDimensionRow>,
    pub sessions: Vec<AnalyticsDimensionRow>,
    pub tool_calls: Vec<ToolCallRow>,
    pub codex_metadata_present: i64,
    /// Client requests created by Codex background features without trusted
    /// project context. They remain in top-level totals but are excluded from
    /// the ordinary projects dimension.
    pub internal_feature_requests: i64,
    pub internal_features: Vec<AnalyticsDimensionRow>,
    pub facets: AnalyticsFacets,
    pub skipped_events: i64,
    pub truncated: bool,
    pub filters_applied: bool,
    pub filter_warning: Option<String>,
    pub applied_filters: AnalyticsAppliedFilters,
}

#[derive(Debug, Clone)]
pub struct ErrorPageQuery {
    pub page: usize,
    pub page_size: usize,
    pub snapshot_seq: Option<i64>,
    pub history_generation: Option<i64>,
    pub filter: RuntimeFilter,
}

pub type ErrorQuery = ErrorPageQuery;

impl Default for ErrorPageQuery {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 10,
            snapshot_seq: None,
            history_generation: None,
            filter: RuntimeFilter::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorGroup {
    pub failure_kind: Option<String>,
    pub failure_phase: Option<String>,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    pub endpoint_name: Option<String>,
    pub model: Option<String>,
    pub upstream_status_code: Option<i64>,
    pub occurrences: i64,
    pub affected_requests: i64,
    pub affected_sessions: i64,
    pub recovered_after_failover: i64,
    pub first_seen: f64,
    pub last_seen: f64,
    #[serde(rename = "sampleEventIDs")]
    pub sample_event_ids: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorPage {
    pub api_version: u8,
    pub groups: Vec<ErrorGroup>,
    pub page: usize,
    pub page_size: usize,
    pub total_count: i64,
    pub total_pages: usize,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub retained_from_seq: i64,
    pub has_next: bool,
    pub has_previous: bool,
    pub filters: RuntimeFilter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionKind {
    Endpoint,
    Model,
    ClientKind,
    ClientVariant,
    AgentRole,
    AgentName,
    ParentThread,
    ParentTurn,
    RootTurn,
    Purpose,
    FailureKind,
    FailurePhase,
    Protocol,
    StreamTerminal,
    Project,
    Session,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DimensionSort {
    Name,
    Requests,
    SuccessRate,
    Failures,
    InputTokens,
    OutputTokens,
    CacheReadTokens,
    CacheWriteTokens,
    Tokens,
    AverageDuration,
    #[default]
    LastSeen,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

#[derive(Debug, Clone)]
pub struct DimensionPageQuery {
    pub page: usize,
    pub page_size: usize,
    pub search: Option<String>,
    pub sort: DimensionSort,
    pub order: SortOrder,
    pub snapshot_seq: Option<i64>,
    pub history_generation: Option<i64>,
    pub filter: RuntimeFilter,
}

pub type DimensionQuery = DimensionPageQuery;

impl Default for DimensionPageQuery {
    fn default() -> Self {
        Self {
            page: 1,
            page_size: 10,
            search: None,
            sort: DimensionSort::LastSeen,
            order: SortOrder::Desc,
            snapshot_seq: None,
            history_generation: None,
            filter: RuntimeFilter::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionRow {
    pub key: String,
    pub name: String,
    pub source: String,
    pub requests: i64,
    pub successes: i64,
    pub failures: i64,
    pub cancelled: i64,
    pub failovers: i64,
    pub slow_duration_requests: i64,
    pub critical_duration_requests: i64,
    pub slow_ttfb_requests: i64,
    pub critical_ttfb_requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_input_tokens: i64,
    pub cache_creation_input_tokens: i64,
    pub processed_input_tokens: i64,
    pub processed_total_tokens: i64,
    pub cache_read_reported_requests: i64,
    pub cache_read_hit_requests: i64,
    pub cache_read_token_eligible_requests: i64,
    pub cache_read_token_unknown_requests: i64,
    pub cache_read_token_rate: Option<f64>,
    pub cache_read_request_rate: Option<f64>,
    pub first_seen: f64,
    pub last_seen: f64,
    #[serde(rename = "averageDurationMS")]
    pub average_duration_ms: Option<f64>,
    #[serde(rename = "averageTTFBMS")]
    pub average_ttfb_ms: Option<f64>,
    pub related_count: i64,
    pub workspace_paths: Vec<String>,
    /// Distinct inbound clients represented by this grouped row. Project
    /// identity/source remains unchanged and can still be inspected elsewhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_kinds: Vec<String>,
    /// Cost is calculated from the same request rows and price catalog as the
    /// trend endpoint.  Keeping it nested makes missing/partial pricing
    /// explicit instead of making the UI infer cost from token totals.
    pub cost: CostCoverage,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DimensionPage {
    pub api_version: u8,
    pub kind: DimensionKind,
    pub rows: Vec<DimensionRow>,
    pub page: usize,
    pub page_size: usize,
    pub total_count: i64,
    pub total_pages: usize,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub retained_from_seq: i64,
    pub has_next: bool,
    pub has_previous: bool,
    pub search: Option<String>,
    pub sort: DimensionSort,
    pub order: SortOrder,
    pub filters: RuntimeFilter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionStatus {
    pub revision: i64,
    pub max_age_days: Option<i64>,
    pub storage_limit_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageProbe {
    pub api_version: u8,
    pub backend: &'static str,
    pub schema_version: i64,
    pub projection_version: i64,
    pub projection_backfill_cursor: i64,
    pub projection_backfill_complete: bool,
    pub projection_indexes_ready: bool,
    pub missing_indexes: Vec<String>,
    pub hourly_rollup_complete: bool,
    pub hourly_rollup_max_seq: i64,
    pub hourly_rollup_history_generation: i64,
    pub hourly_rollup_failed: bool,
    pub hourly_rollup_dirty_buckets: i64,
    pub retained_events: i64,
    pub completed_events: i64,
    pub in_flight_events: i64,
    pub min_seq: Option<i64>,
    pub max_seq: Option<i64>,
    pub earliest_timestamp: Option<f64>,
    pub latest_timestamp: Option<f64>,
    pub retained_from_seq: i64,
    pub history_generation: i64,
    pub reset_generation: i64,
    pub user_deleted_events: i64,
    pub user_deleted_requests: i64,
    pub payload_bytes: i64,
    pub database_bytes: u64,
    pub live_bytes: u64,
    pub allocated_bytes: u64,
    pub freelist_bytes: u64,
    #[serde(rename = "walBytes")]
    pub wal_bytes: u64,
    pub pending_events: Option<usize>,
    pub pending_bytes: Option<usize>,
    pub retention: RetentionStatus,
    /// True when an older runtime_retention table still carries the removed
    /// automatic-pruning columns. They are intentionally ignored; the UI
    /// uses this flag to explain the manual-reset cutover to the user.
    pub legacy_retention_detected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportScope {
    Events,
    Projects,
    Sessions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportFormat {
    Csv,
    Jsonl,
}

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Jsonl => "jsonl",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Jsonl => "application/x-ndjson; charset=utf-8",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportPrivacy {
    Redacted,
    Stored,
}

#[derive(Debug, Clone)]
pub struct ExportQuery {
    pub scope: ExportScope,
    pub format: ExportFormat,
    pub privacy: ExportPrivacy,
    pub confirm_stored: bool,
    pub snapshot_seq: Option<i64>,
    pub history_generation: Option<i64>,
    pub filter: RuntimeFilter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEstimate {
    pub api_version: u8,
    pub scope: ExportScope,
    pub format: ExportFormat,
    pub privacy: ExportPrivacy,
    pub privacy_scope: &'static str,
    pub row_count: i64,
    pub estimated_bytes: u64,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub retained_from_seq: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportManifest {
    pub row_count: i64,
    pub bytes_written: u64,
    pub snapshot_seq: i64,
    pub history_generation: i64,
    pub privacy: ExportPrivacy,
}

#[derive(Default)]
struct TokenAccumulator {
    metrics: TokenMetrics,
    cache_read_token_numerator: i128,
    cache_read_token_denominator: i128,
    semantics: Option<String>,
    quality: Option<String>,
}

impl TokenAccumulator {
    fn add(&mut self, row: &TrendRow) {
        if row.request_purpose.as_deref() == Some("token_count") {
            return;
        }
        if row.usage_present != 1 {
            return;
        }
        merge_accounting_label(
            &mut self.semantics,
            row.token_accounting_semantics.as_deref(),
        );
        let row_quality = row.token_accounting_quality.as_deref().unwrap_or("unknown");
        merge_label(&mut self.quality, Some(row_quality));
        self.metrics.observed_requests = self.metrics.observed_requests.saturating_add(1);
        macro_rules! add_field {
            ($field:ident) => {
                if let Some(value) = row.$field {
                    self.metrics.usage_field_presence.$field =
                        self.metrics.usage_field_presence.$field.saturating_add(1);
                    self.metrics.$field = self.metrics.$field.saturating_add(value.max(0));
                }
            };
        }
        add_field!(input_tokens);
        add_field!(output_tokens);
        add_field!(cache_read_input_tokens);
        add_field!(cache_creation_input_tokens);
        add_field!(reasoning_tokens);
        if let Some(value) = row.uncached_input_tokens {
            self.metrics.uncached_input_tokens = self
                .metrics
                .uncached_input_tokens
                .saturating_add(value.max(0));
        }
        if let Some(value) = row.processed_input_tokens {
            self.metrics.processed_input_tokens = self
                .metrics
                .processed_input_tokens
                .saturating_add(value.max(0));
        }
        if let Some(value) = row.processed_total_tokens {
            self.metrics.processed_total_tokens = self
                .metrics
                .processed_total_tokens
                .saturating_add(value.max(0));
        }

        if let Some(cache_read) = row.cache_read_input_tokens {
            self.metrics.cache_read_reported_requests =
                self.metrics.cache_read_reported_requests.saturating_add(1);
            if cache_read > 0 {
                self.metrics.cache_read_hit_requests =
                    self.metrics.cache_read_hit_requests.saturating_add(1);
            }
        }
        let accounting_known = matches!(
            row.token_accounting_semantics.as_deref(),
            Some("subset" | "independent")
        );
        if accounting_known {
            self.metrics.accounting_known_requests =
                self.metrics.accounting_known_requests.saturating_add(1);
        } else {
            self.metrics.accounting_unknown_requests =
                self.metrics.accounting_unknown_requests.saturating_add(1);
        }
        if accounting_known {
            if let (Some(cache_read), Some(processed_input)) =
                (row.cache_read_input_tokens, row.processed_input_tokens)
            {
                self.metrics.cache_read_token_eligible_requests = self
                    .metrics
                    .cache_read_token_eligible_requests
                    .saturating_add(1);
                self.cache_read_token_numerator = self
                    .cache_read_token_numerator
                    .saturating_add(cache_read.max(0) as i128);
                self.cache_read_token_denominator = self
                    .cache_read_token_denominator
                    .saturating_add(processed_input.max(0) as i128);
            } else {
                self.metrics.cache_read_token_unknown_requests = self
                    .metrics
                    .cache_read_token_unknown_requests
                    .saturating_add(1);
            }
        } else {
            self.metrics.cache_read_token_unknown_requests = self
                .metrics
                .cache_read_token_unknown_requests
                .saturating_add(1);
        }
    }

    fn finish(mut self) -> TokenMetrics {
        self.metrics.total_tokens = self
            .metrics
            .input_tokens
            .saturating_add(self.metrics.output_tokens);
        self.metrics.cache_read_token_rate = ratio(
            self.cache_read_token_numerator,
            self.cache_read_token_denominator,
        );
        self.metrics.cache_read_request_rate = ratio(
            self.metrics.cache_read_hit_requests as i128,
            self.metrics.cache_read_reported_requests as i128,
        );
        self.metrics.token_accounting_semantics =
            self.semantics.unwrap_or_else(|| "unknown".into());
        self.metrics.token_accounting_quality = self.quality.unwrap_or_else(|| "unknown".into());
        self.metrics
    }
}

#[derive(Default)]
struct LatencyAccumulator {
    sum_ms: i64,
    observed_requests: i64,
    slow_requests: i64,
    critical_requests: i64,
}

impl LatencyAccumulator {
    fn add(&mut self, value: Option<i64>, slow_ms: i64, critical_ms: i64) {
        let Some(value) = value.filter(|value| *value >= 0) else {
            return;
        };
        self.sum_ms = self.sum_ms.saturating_add(value);
        self.observed_requests = self.observed_requests.saturating_add(1);
        if value > slow_ms {
            self.slow_requests = self.slow_requests.saturating_add(1);
        }
        if value > critical_ms {
            self.critical_requests = self.critical_requests.saturating_add(1);
        }
    }

    fn finish(self, slow_ms: i64, critical_ms: i64) -> LatencyMetrics {
        LatencyMetrics {
            observed_requests: self.observed_requests,
            sum_ms: self.sum_ms,
            average_ms: (self.observed_requests > 0)
                .then(|| self.sum_ms as f64 / self.observed_requests as f64),
            threshold_buckets: vec![
                LatencyThresholdBucket {
                    threshold_ms: slow_ms,
                    exceeded_requests: self.slow_requests,
                },
                LatencyThresholdBucket {
                    threshold_ms: critical_ms,
                    exceeded_requests: self.critical_requests,
                },
            ],
        }
    }
}

#[derive(Default)]
struct TrendAccumulator {
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
    tokens: TokenAccumulator,
    ttfb: LatencyAccumulator,
    duration: LatencyAccumulator,
    cost: CostAccumulator,
}

impl TrendAccumulator {
    fn add(&mut self, row: &TrendRow, prices: &PriceCatalog) {
        if row.kind == "upstream" {
            self.upstream_attempts = self.upstream_attempts.saturating_add(1);
            match row.outcome.as_deref() {
                Some("succeeded") => {
                    self.upstream_successes = self.upstream_successes.saturating_add(1)
                }
                Some("failed") => self.upstream_failures = self.upstream_failures.saturating_add(1),
                _ => {}
            }
            return;
        }
        self.client_requests = self.client_requests.saturating_add(1);
        let terminal = match row.outcome.as_deref() {
            Some("succeeded") => {
                self.client_successes = self.client_successes.saturating_add(1);
                true
            }
            Some("failed") => {
                self.client_failures = self.client_failures.saturating_add(1);
                true
            }
            Some("cancelled") => {
                self.client_cancelled = self.client_cancelled.saturating_add(1);
                true
            }
            _ => {
                self.client_unknown_results = self.client_unknown_results.saturating_add(1);
                false
            }
        };
        self.failovers = self.failovers.saturating_add(i64::from(row.failover != 0));
        if row.failover != 0 && terminal {
            self.failover_terminal_requests = self.failover_terminal_requests.saturating_add(1);
            if row.outcome.as_deref() == Some("succeeded") {
                self.failover_recovered_requests =
                    self.failover_recovered_requests.saturating_add(1);
            }
        }
        self.ttfb.add(row.ttfb_ms, TTFB_SLOW_MS, TTFB_CRITICAL_MS);
        self.duration
            .add(row.duration_ms, DURATION_SLOW_MS, DURATION_CRITICAL_MS);
        self.tokens.add(row);
        self.cost.add(row, prices);
    }

    fn finish(self, bucket_start: f64, bucket_end: f64, prices: &PriceCatalog) -> TrendPoint {
        TrendPoint {
            bucket_start,
            bucket_end,
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
            tokens: self.tokens.finish(),
            ttfb_ms: self.ttfb.finish(TTFB_SLOW_MS, TTFB_CRITICAL_MS),
            duration_ms: self.duration.finish(DURATION_SLOW_MS, DURATION_CRITICAL_MS),
            cost: self.cost.finish(prices),
        }
    }
}

struct TrendRow {
    kind: String,
    timestamp: f64,
    outcome: Option<String>,
    failover: i64,
    duration_ms: Option<i64>,
    ttfb_ms: Option<i64>,
    request_purpose: Option<String>,
    endpoint_id: Option<String>,
    effective_model: Option<String>,
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

struct ExportEventRow {
    seq: i64,
    change_seq: i64,
    event_id: String,
    request_id: Option<String>,
    timestamp: f64,
    kind: String,
    outcome: Option<String>,
    status_code: i64,
    client_kind: Option<String>,
    client_variant: Option<String>,
    agent_role: Option<String>,
    agent_name: Option<String>,
    parent_thread_id: Option<String>,
    parent_turn_id: Option<String>,
    root_turn_id: Option<String>,

    request_purpose: Option<String>,
    endpoint_id: Option<String>,
    endpoint_name: Option<String>,
    effective_model: Option<String>,
    project_id: Option<String>,
    project_name: Option<String>,
    session_key: Option<String>,
    failure_kind: Option<String>,
    failure_phase: Option<String>,
    upstream_status_code: Option<i64>,
    duration_ms: Option<i64>,
    ttfb_ms: Option<i64>,
    failover: bool,
    input_tokens: Option<i64>,
    output_tokens: Option<i64>,
    cache_read_input_tokens: Option<i64>,
    cache_creation_input_tokens: Option<i64>,
    reasoning_tokens: Option<i64>,
    processed_input_tokens: Option<i64>,
    processed_total_tokens: Option<i64>,
    payload_json: Option<String>,
}

/// The event list is intentionally a projection, not a detail endpoint.  Keep
/// the fields needed to render/filter the list in typed SQLite columns and do
/// not deserialize `payload_json` here.  Optional diagnostic fields (Codex
/// metadata, stream trace, tool calls, message and raw failure detail) remain
/// available through `/runtime/events/{id}`.
#[derive(Debug, sea_orm::FromQueryResult)]
struct EventListProjection {
    seq: i64,
    change_seq: i64,
    id: String,
    timestamp: f64,
    kind: String,
    client_variant: Option<String>,
    agent_role: Option<String>,
    agent_name: Option<String>,
    parent_thread_id: Option<String>,
    parent_turn_id: Option<String>,
    root_turn_id: Option<String>,
    phase: Option<String>,
    outcome: Option<String>,
    status_code: i64,
    request_id: Option<String>,
    session_key: Option<String>,
    session_source: Option<String>,
    client_kind: Option<String>,
    request_purpose: Option<String>,
    endpoint_id: Option<String>,
    endpoint_name: Option<String>,
    model_group_id: Option<String>,
    model_group_name: Option<String>,
    feature_rule_id: Option<String>,
    client_model: Option<String>,
    effective_model: Option<String>,
    upstream_model: Option<String>,
    failure_kind: Option<String>,
    failure_phase: Option<String>,
    source_format: Option<String>,
    target_format: Option<String>,
    route_mode: Option<String>,
    upstream_status_code: Option<i64>,
    duration_ms: i64,
    ttfb_ms: Option<i64>,
    failover: i64,
    project_name: Option<String>,
    project_source: Option<String>,
    local_user: Option<String>,
    codex_thread_class: Option<String>,
    attribution_scope: Option<String>,
    request_method: Option<String>,
    request_path: Option<String>,
    route_intent: Option<String>,
}

fn decode_projection_enum<T: DeserializeOwned>(value: Option<String>) -> Option<T> {
    // SQLite stores enum projections as plain text tokens (`completed`,
    // `succeeded`, ...), not as quoted JSON string documents.  Feeding the
    // bare token to `from_str` makes every decode fail and silently turns
    // current list rows into legacy-looking events with no phase/outcome.
    value.and_then(|value| serde_json::from_value(serde_json::Value::String(value)).ok())
}

fn event_list_item_from_projection(row: EventListProjection) -> RuntimeEventListItem {
    // `session_key` can be derived from Codex metadata or a thread when an
    // event has no explicit session header.  Preserve the old list contract:
    // expose `sessionID` only when it came from the event itself; the derived
    // key remains available to server-side filters and dimension pages.
    let session_id = (row.session_source.as_deref() == Some("event"))
        .then_some(row.session_key)
        .flatten();
    RuntimeEventListItem {
        seq: row.seq,
        change_seq: row.change_seq,
        id: row.id,
        timestamp: row.timestamp,
        kind: row.kind,
        client_variant: row.client_variant,
        agent_role: row.agent_role,
        agent_name: row.agent_name,
        parent_thread_id: row.parent_thread_id,
        parent_turn_id: row.parent_turn_id,
        root_turn_id: row.root_turn_id,
        codex_metadata: None,
        client_declared: None,
        grok_metadata: None,
        // 投影快路径不解 payload_json,归因只能靠这两个投影列(见 RuntimeEventListItem 注释)。
        project_name: row.project_name,
        project_source: row.project_source,
        local_user: row.local_user,
        codex_thread_class: row.codex_thread_class,
        attribution_scope: row.attribution_scope,
        client_model: row.client_model,
        source_format: decode_projection_enum::<ProviderProtocol>(row.source_format),
        target_format: decode_projection_enum::<ProviderProtocol>(row.target_format),
        route_mode: decode_projection_enum::<RouteMode>(row.route_mode),
        phase: decode_projection_enum::<RuntimeEventPhase>(row.phase),
        outcome: decode_projection_enum::<RuntimeEventOutcome>(row.outcome),
        status_code: row.status_code,
        request_id: row.request_id,
        request_method: row.request_method,
        request_path: row.request_path,
        route_intent: row.route_intent,
        session_id,
        client_kind: decode_projection_enum::<ClientKind>(row.client_kind),
        request_purpose: decode_projection_enum::<RequestPurpose>(row.request_purpose),
        endpoint_id: row.endpoint_id,
        endpoint_name: row.endpoint_name,
        model_group_id: row.model_group_id,
        model_group_name: row.model_group_name,
        feature_rule_id: row.feature_rule_id,
        effective_model: row.effective_model,
        upstream_model: row.upstream_model,
        failure_kind: decode_projection_enum::<RuntimeFailureKind>(row.failure_kind),
        failure_phase: decode_projection_enum::<RuntimeFailurePhase>(row.failure_phase),
        failure_detail: None,
        message: None,
        stream_trace: None,
        tool_calls: None,
        timeout_ms: None,
        upstream_host: None,
        upstream_status_code: row.upstream_status_code,
        upstream_request_id: None,
        duration_ms: row.duration_ms,
        ttfb_ms: row.ttfb_ms,
        failover: row.failover != 0,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrice {
    pub id: i64,
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    pub model_key: String,
    pub effective_from: f64,
    pub effective_to: Option<f64>,
    pub input_per_million_micros: Option<i64>,
    pub output_per_million_micros: Option<i64>,
    pub cache_read_per_million_micros: Option<i64>,
    pub cache_creation_per_million_micros: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pricing {
    pub api_version: u8,
    pub revision: i64,
    pub currency: String,
    pub prices: Vec<ModelPrice>,
}

#[derive(Default)]
struct PriceCatalog {
    revision: Option<i64>,
    currency: Option<String>,
    by_model: HashMap<String, Vec<ModelPrice>>,
}

impl PriceCatalog {
    fn resolve(
        &self,
        endpoint_id: Option<&str>,
        model: &str,
        timestamp: f64,
    ) -> Option<&ModelPrice> {
        let matches_timestamp = |price: &&ModelPrice| {
            timestamp >= price.effective_from
                && price
                    .effective_to
                    .is_none_or(|effective_to| timestamp < effective_to)
        };
        if let Some(endpoint_prices) = endpoint_id
            .filter(|id| !id.is_empty())
            .and_then(|id| self.by_model.get(&stored_price_key(Some(id), model)))
            && let Some(price) = endpoint_prices.iter().find(matches_timestamp)
        {
            return Some(price);
        }
        self.by_model.get(model)?.iter().find(matches_timestamp)
    }
}

#[derive(Default)]
struct CostAccumulator {
    cost_numerator: i128,
    priced_requests: i64,
    unpriced_requests: i64,
    unknown_accounting_requests: i64,
}

impl CostAccumulator {
    fn add(&mut self, row: &TrendRow, prices: &PriceCatalog) {
        if row.request_purpose.as_deref() == Some("token_count") {
            return;
        }
        let accounting_known = matches!(
            row.token_accounting_semantics.as_deref(),
            Some("subset" | "independent")
        ) && row.token_accounting_quality.as_deref() == Some("complete");
        let Some((uncached_input, cache_read, cache_creation, output)) = accounting_known
            .then_some((
                row.uncached_input_tokens,
                row.cache_read_input_tokens,
                row.cache_creation_input_tokens,
                row.output_tokens,
            ))
            .and_then(|values| match values {
                (Some(uncached), Some(read), Some(creation), Some(output)) => {
                    Some((uncached.max(0), read.max(0), creation.max(0), output.max(0)))
                }
                _ => None,
            })
        else {
            self.unknown_accounting_requests = self.unknown_accounting_requests.saturating_add(1);
            return;
        };
        let Some(model) = row.effective_model.as_deref() else {
            self.unpriced_requests = self.unpriced_requests.saturating_add(1);
            return;
        };
        let Some(price) = prices.resolve(row.endpoint_id.as_deref(), model, row.timestamp) else {
            self.unpriced_requests = self.unpriced_requests.saturating_add(1);
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
                self.unpriced_requests = self.unpriced_requests.saturating_add(1);
                return;
            };
            request_numerator =
                request_numerator.saturating_add((tokens as i128).saturating_mul(rate as i128));
        }
        self.cost_numerator = self.cost_numerator.saturating_add(request_numerator);
        self.priced_requests = self.priced_requests.saturating_add(1);
    }

    fn finish(self, prices: &PriceCatalog) -> CostCoverage {
        let rounded_micros = self.cost_numerator.saturating_add(500_000) / 1_000_000;
        CostCoverage {
            estimated_cost_micros: rounded_micros.min(i64::MAX as i128) as i64,
            priced_requests: self.priced_requests,
            unpriced_requests: self.unpriced_requests,
            unknown_accounting_requests: self.unknown_accounting_requests,
            complete: self.unpriced_requests == 0 && self.unknown_accounting_requests == 0,
            currency: prices.currency.clone(),
            price_version: prices.revision,
        }
    }
}

#[derive(Default, Clone)]
struct SqlFilter {
    clauses: Vec<String>,
    values: Vec<SqlValue>,
}

impl SqlFilter {
    fn raw(&mut self, clause: impl Into<String>) {
        self.clauses.push(clause.into());
    }

    fn text_values(&mut self, clause: impl Into<String>, values: impl IntoIterator<Item = String>) {
        self.clauses.push(clause.into());
        self.values.extend(values.into_iter().map(SqlValue::Text));
    }

    fn eq_text(&mut self, column: &'static str, value: Option<&str>) {
        if let Some(value) = value {
            self.clauses.push(format!("{column} = ?"));
            self.values.push(SqlValue::Text(value.to_owned()));
        }
    }

    fn ge_real(&mut self, column: &'static str, value: Option<f64>) {
        if let Some(value) = value {
            self.clauses.push(format!("{column} >= ?"));
            self.values.push(SqlValue::Real(value));
        }
    }

    fn le_real(&mut self, column: &'static str, value: Option<f64>) {
        if let Some(value) = value {
            self.clauses.push(format!("{column} <= ?"));
            self.values.push(SqlValue::Real(value));
        }
    }

    fn le_i64(&mut self, column: &'static str, value: i64) {
        self.clauses.push(format!("{column} <= ?"));
        self.values.push(SqlValue::Integer(value));
    }

    fn where_sql(&self) -> String {
        if self.clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.clauses.join(" AND "))
        }
    }
}

fn append_runtime_filter(builder: &mut SqlFilter, filter: &RuntimeFilter) {
    builder.eq_text("kind", filter.kind.as_deref());
    builder.eq_text("outcome", filter.outcome.as_deref());
    builder.eq_text("client_kind", filter.client_kind.as_deref());
    builder.eq_text(
        "COALESCE(client_variant,'unknown')",
        filter.client_variant.as_deref(),
    );
    builder.eq_text(
        "COALESCE(agent_role,'unknown')",
        filter.agent_role.as_deref(),
    );
    builder.eq_text(
        "COALESCE(agent_name,'unknown')",
        filter.agent_name.as_deref(),
    );
    builder.eq_text(
        "COALESCE(parent_thread_id,'unknown')",
        filter.parent_thread_id.as_deref(),
    );
    builder.eq_text(
        "COALESCE(parent_turn_id,'unknown')",
        filter.parent_turn_id.as_deref(),
    );
    builder.eq_text(
        "COALESCE(root_turn_id,'unknown')",
        filter.root_turn_id.as_deref(),
    );
    builder.eq_text("request_purpose", filter.request_purpose.as_deref());
    builder.eq_text("request_id", filter.request_id.as_deref());
    builder.eq_text("endpoint_id", filter.endpoint_id.as_deref());
    builder.eq_text("effective_model", filter.model.as_deref());
    builder.eq_text("project_id", filter.project_id.as_deref());
    builder.eq_text("project_name", filter.project_name.as_deref());
    builder.eq_text("session_key", filter.session_id.as_deref());
    builder.eq_text("failure_kind", filter.failure_kind.as_deref());
    builder.eq_text("failure_phase", filter.failure_phase.as_deref());
    builder.ge_real("timestamp", filter.from);
    builder.le_real("timestamp", filter.to);
}

fn analytics_filter_builder(filter: &RuntimeFilter, snapshot_seq: i64) -> SqlFilter {
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    builder.le_i64("seq", snapshot_seq);
    append_runtime_filter(&mut builder, filter);
    builder
}

fn analytics_client_builder(filter: &RuntimeFilter, snapshot_seq: i64) -> SqlFilter {
    let mut builder = analytics_filter_builder(filter, snapshot_seq);
    builder.raw("kind = 'client'");
    builder
}

/// Build the upstream side of an analytics query.  Client-facing facet
/// filters (client/project/session) select a request first; all upstream
/// attempts belonging to those requests are then included so failover counts
/// and endpoint attribution remain truthful.
fn analytics_upstream_builder(filter: &RuntimeFilter, snapshot_seq: i64) -> SqlFilter {
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    builder.le_i64("seq", snapshot_seq);
    builder.eq_text("kind", filter.kind.as_deref());
    builder.eq_text("outcome", filter.outcome.as_deref());
    builder.eq_text("request_purpose", filter.request_purpose.as_deref());
    builder.eq_text("request_id", filter.request_id.as_deref());
    builder.eq_text("endpoint_id", filter.endpoint_id.as_deref());
    builder.eq_text("effective_model", filter.model.as_deref());
    builder.eq_text("failure_kind", filter.failure_kind.as_deref());
    builder.eq_text("failure_phase", filter.failure_phase.as_deref());
    builder.ge_real("timestamp", filter.from);
    builder.le_real("timestamp", filter.to);
    if filter
        .kind
        .as_deref()
        .is_some_and(|kind| kind != "upstream")
    {
        builder.raw("0");
    } else {
        builder.raw("kind = 'upstream'");
    }
    let has_client_selection = filter.has_agent_filter()
        || filter.client_kind.is_some()
        || filter.project_id.is_some()
        || filter.project_name.is_some()
        || filter.session_id.is_some();
    if has_client_selection {
        let mut client_filter = filter.clone();
        client_filter.kind = None;
        let client = analytics_client_builder(&client_filter, snapshot_seq);
        builder.raw(format!(
            "request_id IN (SELECT request_id FROM runtime_events{} )",
            client.where_sql()
        ));
        // The subquery is appended after all outer placeholders in the SQL.
        builder.values.extend(client.values);
    }
    builder
}

fn ratio(numerator: i128, denominator: i128) -> Option<f64> {
    (denominator > 0).then(|| (numerator as f64 / denominator as f64).clamp(0.0, 1.0))
}

fn csv_values(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn merge_label(current: &mut Option<String>, next: Option<&str>) {
    let Some(next) = next.filter(|value| !value.is_empty()) else {
        return;
    };
    match current.as_deref() {
        None => *current = Some(next.to_owned()),
        Some(value) if value == next || value == "mixed" => {}
        Some(_) => *current = Some("mixed".into()),
    }
}

fn merge_accounting_label(current: &mut Option<String>, next: Option<&str>) {
    let Some(next) = next.filter(|value| !value.is_empty()) else {
        return;
    };
    match current.as_deref() {
        None => *current = Some(next.to_owned()),
        Some(value) if value == "unknown" || next == "unknown" => *current = Some("unknown".into()),
        Some(value) if value == next || value == "mixed" => {}
        Some(_) => *current = Some("mixed".into()),
    }
}

/// Rollup prices are stored as a token*rate numerator where the rate is in
/// micros per million tokens. Round only after merging buckets so a long
/// range does not accumulate per-bucket truncation error.
fn round_cost_numerator(numerator: i128) -> i64 {
    if numerator <= 0 {
        return 0;
    }
    numerator
        .saturating_add(500_000)
        .checked_div(1_000_000)
        .unwrap_or(i128::MAX)
        .min(i64::MAX as i128) as i64
}

fn load_price_catalog(transaction: &Transaction<'_>) -> QueryResult<PriceCatalog> {
    let meta = transaction
        .query_row(
            "SELECT revision,currency FROM runtime_pricing_meta WHERE id=1",
            crate::database::params![],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let mut catalog = PriceCatalog {
        revision: meta.as_ref().map(|value| value.0),
        currency: meta.map(|value| value.1),
        by_model: HashMap::new(),
    };
    let mut statement = transaction.prepare(
        "SELECT id,model_key,effective_from,effective_to,input_per_million_micros,\
                output_per_million_micros,cache_read_per_million_micros,\
                cache_creation_per_million_micros \
         FROM runtime_model_prices ORDER BY model_key,effective_from DESC,id DESC",
    )?;
    let rows = statement.query_map(crate::database::params![], |row| {
        Ok(ModelPrice {
            id: row.get(0)?,
            endpoint_id: None,
            model_key: row.get(1)?,
            effective_from: row.get(2)?,
            effective_to: row.get(3)?,
            input_per_million_micros: row.get(4)?,
            output_per_million_micros: row.get(5)?,
            cache_read_per_million_micros: row.get(6)?,
            cache_creation_per_million_micros: row.get(7)?,
        })
    })?;
    for row in rows {
        let mut price = row?;
        if let Some((endpoint_id, model_key)) = split_stored_price_key(&price.model_key) {
            price.endpoint_id = Some(endpoint_id);
            price.model_key = model_key;
        }
        catalog
            .by_model
            .entry(stored_price_key(
                price.endpoint_id.as_deref(),
                &price.model_key,
            ))
            .or_default()
            .push(price);
    }
    Ok(catalog)
}

fn stored_price_key(endpoint_id: Option<&str>, model_key: &str) -> String {
    endpoint_id
        .filter(|value| !value.is_empty())
        .map(|value| format!("{value}{SCOPED_PRICE_SEPARATOR}{model_key}"))
        .unwrap_or_else(|| model_key.to_owned())
}

fn split_stored_price_key(value: &str) -> Option<(String, String)> {
    let (endpoint, model) = value.split_once(SCOPED_PRICE_SEPARATOR)?;
    if endpoint.is_empty() || model.is_empty() {
        return None;
    }
    Some((endpoint.to_owned(), model.to_owned()))
}

pub fn pricing(path: &Path) -> QueryResult<Pricing> {
    let mut connection = read_connection(path)?;
    pricing_on(&mut connection)
}

pub fn pricing_on(connection: &mut Connection) -> QueryResult<Pricing> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let catalog = load_price_catalog(&transaction)?;
    let mut prices = catalog.by_model.into_values().flatten().collect::<Vec<_>>();
    prices.sort_by(|left, right| {
        left.model_key
            .cmp(&right.model_key)
            .then_with(|| right.effective_from.total_cmp(&left.effective_from))
            .then_with(|| right.id.cmp(&left.id))
    });
    let result = Pricing {
        api_version: API_VERSION,
        revision: catalog.revision.unwrap_or(0),
        currency: catalog.currency.unwrap_or_else(|| "USD".into()),
        prices,
    };
    transaction.commit()?;
    Ok(result)
}

fn validate_page(page: usize, page_size: usize) -> QueryResult<()> {
    if page == 0 {
        return Err(RuntimeQueryError::InvalidInput(
            "page must be at least 1".into(),
        ));
    }
    if !PAGE_SIZES.contains(&page_size) {
        return Err(RuntimeQueryError::InvalidInput(
            "pageSize must be one of 10, 25, 50, 100, 200".into(),
        ));
    }
    Ok(())
}

fn meta_i64(transaction: &Transaction<'_>, key: &str) -> QueryResult<Option<i64>> {
    let value = transaction
        .query_row(
            "SELECT value FROM runtime_meta WHERE key=?1",
            [key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    value
        .map(|value| {
            value.parse::<i64>().map_err(|_| {
                RuntimeQueryError::InvalidInput(format!(
                    "runtime_meta {key} is not a valid integer"
                ))
            })
        })
        .transpose()
}

fn projection_ready(transaction: &Transaction<'_>) -> QueryResult<bool> {
    Ok(
        meta_i64(transaction, "projection_backfill_complete")?.unwrap_or(0) == 1
            && meta_i64(transaction, "projection_indexes_ready")?.unwrap_or(0) == 1,
    )
}

fn require_projection(transaction: &Transaction<'_>) -> QueryResult<()> {
    if projection_ready(transaction)? {
        Ok(())
    } else {
        Err(RuntimeQueryError::ProjectionNotReady {
            backfill_cursor: meta_i64(transaction, "projection_backfill_cursor")?.unwrap_or(0),
        })
    }
}

fn history_snapshot(
    transaction: &Transaction<'_>,
    requested_seq: Option<i64>,
    requested_generation: Option<i64>,
) -> QueryResult<HistorySnapshot> {
    if requested_seq.is_none() && requested_generation.is_some() {
        return Err(RuntimeQueryError::InvalidInput(
            "historyGeneration requires snapshotSeq".into(),
        ));
    }
    if requested_seq.is_some_and(|value| value < 0) {
        return Err(RuntimeQueryError::InvalidInput(
            "snapshotSeq must not be negative".into(),
        ));
    }
    let history_generation = meta_i64(transaction, "history_generation")?.unwrap_or(0);
    if let Some(requested) = requested_generation
        && requested != history_generation
    {
        return Err(RuntimeQueryError::SnapshotExpired {
            requested,
            current: history_generation,
        });
    }
    let max_seq = transaction.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM runtime_events WHERE is_in_flight=0",
        crate::database::params![],
        |row| row.get(0),
    )?;
    let snapshot_seq = requested_seq.unwrap_or(max_seq);
    let retained_from_seq =
        meta_i64(transaction, "retained_from_seq")?.unwrap_or(i64::from(max_seq != 0));
    if requested_seq.is_some()
        && snapshot_seq > 0
        && retained_from_seq > 0
        && snapshot_seq < retained_from_seq
    {
        return Err(RuntimeQueryError::SnapshotTrimmed {
            snapshot_seq,
            retained_from_seq,
        });
    }
    Ok(HistorySnapshot {
        snapshot_seq,
        history_generation,
        reset_generation: meta_i64(transaction, "reset_generation")?.unwrap_or(0),
        retained_from_seq,
    })
}

#[path = "runtime_query/analytics.rs"]
mod analytics_queries;
#[path = "runtime_query/dimensions.rs"]
mod dimensions_queries;
#[path = "runtime_query/errors.rs"]
mod errors_queries;
#[path = "runtime_query/events.rs"]
mod events_queries;
#[path = "runtime_query/export.rs"]
mod export_queries;
#[path = "runtime_query/facets.rs"]
mod facets_queries;
#[path = "runtime_query/storage.rs"]
mod storage_queries;
#[path = "runtime_query/trends.rs"]
mod trends_queries;

pub use analytics_queries::*;
pub use dimensions_queries::*;
pub use errors_queries::*;
pub use events_queries::*;
pub use export_queries::*;
pub use facets_queries::*;
pub use storage_queries::*;
pub use trends_queries::*;

#[cfg(test)]
#[path = "runtime_query/tests.rs"]
mod tests;
