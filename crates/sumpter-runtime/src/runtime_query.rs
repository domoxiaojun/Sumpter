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

use rusqlite::types::Value as SqlValue;
use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params_from_iter,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sumpter_core::config::ProviderProtocol;
use sumpter_core::events::{
    ClientKind, RuntimeEventOutcome, RuntimeEventPhase, RuntimeFailureKind, RuntimeFailurePhase,
};
use sumpter_core::routing::{RequestPurpose, RouteMode};

use crate::runtime_store::{RuntimeChange, RuntimeEventListItem};

const API_VERSION: u8 = 3;
const PROJECTION_VERSION: i64 = 6;
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
    Sql(rusqlite::Error),
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

impl From<rusqlite::Error> for RuntimeQueryError {
    fn from(error: rusqlite::Error) -> Self {
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
    #[serde(rename = "endpointID")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "projectID")]
    pub project_id: Option<String>,
    pub project: Option<String>,
    #[serde(rename = "sessionID")]
    pub session_id: Option<String>,
}

impl RuntimeFilter {
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
#[derive(Debug)]
struct EventListProjection {
    seq: i64,
    change_seq: i64,
    id: String,
    timestamp: f64,
    kind: String,
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
        codex_metadata: None,
        client_declared: None,
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
    let has_client_selection = filter.client_kind.is_some()
        || filter.project_id.is_some()
        || filter.project_name.is_some()
        || filter.session_id.is_some();
    if has_client_selection {
        let client = analytics_client_builder(filter, snapshot_seq);
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
            [],
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
    let rows = statement.query_map([], |row| {
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
        [],
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

pub fn events_page(path: &Path, request: &EventPageQuery) -> QueryResult<EventPage> {
    let mut connection = read_connection(path)?;
    events_page_on(&mut connection, request)
}

pub fn events_page_on(
    connection: &mut Connection,
    request: &EventPageRequest,
) -> QueryResult<EventPage> {
    validate_page(request.page, request.page_size)?;
    let filters = request.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    // Event pages always select normalized projection columns.  Requiring the
    // projection even for an unfiltered page prevents partially backfilled
    // rows from surfacing as empty values and keeps pagination/filter totals
    // consistent while the background backfill is running.
    require_projection(&transaction)?;
    let snapshot = history_snapshot(
        &transaction,
        request.snapshot_seq,
        request.history_generation,
    )?;
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.le_i64("seq", snapshot.snapshot_seq);
    append_runtime_filter(&mut builder, &filters);
    let where_sql = builder.where_sql();
    let total_count = transaction.query_row(
        &format!("SELECT COUNT(*) FROM runtime_events{where_sql}"),
        params_from_iter(builder.values.iter()),
        |row| row.get::<_, i64>(0),
    )?;
    let total_pages = if total_count == 0 {
        0
    } else {
        ((total_count as usize).saturating_add(request.page_size - 1)) / request.page_size
    };
    let offset = request
        .page
        .saturating_sub(1)
        .checked_mul(request.page_size)
        .ok_or_else(|| RuntimeQueryError::InvalidInput("page offset is too large".into()))?;
    if request.snapshot_seq.is_some()
        && request.page > 1
        && total_count > 0
        && offset >= total_count as usize
        && snapshot.retained_from_seq > 1
    {
        return Err(RuntimeQueryError::SnapshotTrimmed {
            snapshot_seq: snapshot.snapshot_seq,
            retained_from_seq: snapshot.retained_from_seq,
        });
    }

    let mut values = builder.values;
    values.push(SqlValue::Integer(request.page_size as i64));
    values.push(SqlValue::Integer(offset.min(i64::MAX as usize) as i64));
    let sql = format!(
        "SELECT seq,change_seq,event_id,timestamp,kind,phase,outcome,status_code,\
                request_id,session_key,session_source,client_kind,request_purpose,\
                endpoint_id,endpoint_name,feature_rule_id,client_model,\
                effective_model,upstream_model,failure_kind,failure_phase,\
                source_format,target_format,route_mode,upstream_status_code,\
                duration_ms,ttfb_ms,failover,project_name,project_source,local_user,\
                codex_thread_class,attribution_scope \
                ,request_method,request_path,route_intent \
         FROM runtime_events{where_sql} ORDER BY seq DESC LIMIT ? OFFSET ?"
    );
    let events = {
        let mut statement = transaction.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values.iter()), |row| {
            Ok(EventListProjection {
                seq: row.get(0)?,
                change_seq: row.get(1)?,
                id: row.get(2)?,
                timestamp: row.get(3)?,
                kind: row.get(4)?,
                phase: row.get(5)?,
                outcome: row.get(6)?,
                status_code: row.get(7)?,
                request_id: row.get(8)?,
                session_key: row.get(9)?,
                session_source: row.get(10)?,
                client_kind: row.get(11)?,
                request_purpose: row.get(12)?,
                endpoint_id: row.get(13)?,
                endpoint_name: row.get(14)?,
                feature_rule_id: row.get(15)?,
                client_model: row.get(16)?,
                effective_model: row.get(17)?,
                upstream_model: row.get(18)?,
                failure_kind: row.get(19)?,
                failure_phase: row.get(20)?,
                source_format: row.get(21)?,
                target_format: row.get(22)?,
                route_mode: row.get(23)?,
                upstream_status_code: row.get(24)?,
                duration_ms: row.get::<_, Option<i64>>(25)?.unwrap_or_default(),
                ttfb_ms: row.get(26)?,
                failover: row.get::<_, Option<i64>>(27)?.unwrap_or_default(),
                project_name: row.get(28)?,
                project_source: row.get(29)?,
                local_user: row.get(30)?,
                codex_thread_class: row.get(31)?,
                attribution_scope: row.get(32)?,
                request_method: row.get(33)?,
                request_path: row.get(34)?,
                route_intent: row.get(35)?,
            })
        })?;
        rows.map(|row| row.map(event_list_item_from_projection))
            .collect::<Result<Vec<_>, _>>()?
    };
    transaction.commit()?;
    Ok(EventPage {
        api_version: API_VERSION,
        next_cursor: events.last().map(|event| event.seq),
        previous_cursor: events.first().map(|event| event.seq),
        has_next: request.page < total_pages,
        has_previous: request.page > 1 && total_pages > 0,
        events,
        page: request.page,
        page_size: request.page_size,
        total_count,
        total_pages,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        reset_generation: snapshot.reset_generation,
        retained_from_seq: snapshot.retained_from_seq,
        filters,
    })
}

pub fn request_chain(path: &Path, request_id: &str) -> QueryResult<RequestChain> {
    let connection = read_connection(path)?;
    request_chain_on(&connection, request_id)
}

pub fn request_chain_with_recent(
    path: &Path,
    request_id: &str,
    recent_changes: &[RuntimeChange],
) -> QueryResult<RequestChain> {
    let connection = read_connection(path)?;
    request_chain_with_recent_on(&connection, request_id, recent_changes)
}

pub fn request_chain_on(connection: &Connection, request_id: &str) -> QueryResult<RequestChain> {
    request_chain_with_recent_on(connection, request_id, &[])
}

pub fn request_chain_with_recent_on(
    connection: &Connection,
    request_id: &str,
    recent_changes: &[RuntimeChange],
) -> QueryResult<RequestChain> {
    let request_id = request_id.trim();
    if request_id.is_empty() || request_id.len() > 512 {
        return Err(RuntimeQueryError::InvalidInput(
            "requestID must contain 1 to 512 bytes".into(),
        ));
    }
    let mut statement = connection.prepare(
        "SELECT seq,change_seq,event_id,payload_json FROM runtime_events \
         WHERE request_id=?1 ORDER BY seq ASC,change_seq ASC LIMIT ?2",
    )?;
    let mut rows = statement.query((request_id, (REQUEST_CHAIN_LIMIT + 1) as i64))?;
    let mut persisted = Vec::new();
    while let Some(row) = rows.next()? {
        let seq = row.get::<_, i64>(0)?;
        let change_seq = row.get::<_, i64>(1)?;
        let event_id = row.get::<_, String>(2)?;
        let payload = row.get::<_, String>(3)?;
        let event =
            serde_json::from_str(&payload).map_err(|error| RuntimeQueryError::CorruptPayload {
                event_id,
                detail: error.to_string(),
            })?;
        persisted.push(RuntimeEventListItem::from_change(seq, change_seq, event));
    }
    let mut truncated = persisted.len() > REQUEST_CHAIN_LIMIT;
    persisted.truncate(REQUEST_CHAIN_LIMIT);
    let mut merged = persisted
        .into_iter()
        .map(|event| (event.id.clone(), event))
        .collect::<HashMap<_, _>>();
    for change in recent_changes
        .iter()
        .filter(|change| change.event.request_id.as_deref() == Some(request_id))
    {
        let item =
            RuntimeEventListItem::from_change(change.seq, change.change_seq, change.event.clone());
        if merged
            .get(&item.id)
            .is_none_or(|current| item.change_seq > current.change_seq)
        {
            merged.insert(item.id.clone(), item);
        }
    }
    let mut events = merged.into_values().collect::<Vec<_>>();
    events.sort_by_key(|event| (event.seq, event.change_seq));
    if events.len() > REQUEST_CHAIN_LIMIT {
        events.truncate(REQUEST_CHAIN_LIMIT);
        truncated = true;
    }
    if events.is_empty() {
        return Err(RuntimeQueryError::NotFound(format!(
            "runtime request {request_id} was not found"
        )));
    }
    Ok(RequestChain {
        api_version: API_VERSION,
        request_id: request_id.to_owned(),
        events,
        truncated,
    })
}

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
            endpoint_id: filters.endpoint_id.clone(),
            project_id: filters.project_id.clone(),
            project: filters.project_name.clone(),
            session_id: filters.session_id.clone(),
        },
    })
}

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

fn analytics_facets_for_filter(
    transaction: &Transaction<'_>,
    filters: &RuntimeFilter,
    snapshot_seq: i64,
) -> QueryResult<AnalyticsFacets> {
    // A facet ignores its own active filter while retaining every other
    // filter. This prevents a selected value from making the next picker
    // appear empty; the selected value is retained with count 0 when the
    // remaining filters are incompatible.
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
                failure_kind,failure_phase,attribution_scope,timestamp
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
            attribution_scope: row.get(11)?,
            timestamp: row.get(12)?,
        })
    })?;
    let mut client_counts = HashMap::<String, i64>::new();
    let mut endpoint_counts = HashMap::<String, i64>::new();
    let mut project_counts = HashMap::<String, i64>::new();
    let mut session_counts = HashMap::<String, i64>::new();
    let mut model_counts = HashMap::<String, i64>::new();
    let mut purpose_counts = HashMap::<String, i64>::new();
    let mut failure_kind_counts = HashMap::<String, i64>::new();
    let mut failure_phase_counts = HashMap::<String, i64>::new();
    for row in rows {
        let row = row?;
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
    let mut endpoints = facet_rows_from_counts(endpoint_counts);
    let mut projects = facet_rows_from_counts(project_counts);
    let mut sessions = facet_rows_from_counts(session_counts);
    let mut models = facet_rows_from_counts(model_counts);
    let mut request_purposes = facet_rows_from_counts(purpose_counts);
    let mut failure_kinds = facet_rows_from_counts(failure_kind_counts);
    let mut failure_phases = facet_rows_from_counts(failure_phase_counts);
    ensure_facet_selection(&mut client_kinds, filters.client_kind.as_deref());
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
    attribution_scope: Option<String>,
    timestamp: f64,
}

#[derive(Debug, Clone, Copy)]
enum FacetDimension {
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

struct AnalyticsDimensions {
    endpoints: Vec<AnalyticsDimensionRow>,
    models: Vec<AnalyticsDimensionRow>,
    client_kinds: Vec<AnalyticsDimensionRow>,
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

fn csv_values(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
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
    let only_time_filters = filters.kind.is_none()
        && filters.outcome.is_none()
        && filters.client_kind.is_none()
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

pub fn export_estimate(path: &Path, query: &ExportQuery) -> QueryResult<ExportEstimate> {
    let mut connection = read_connection(path)?;
    export_estimate_on(&mut connection, query)
}

pub fn export_estimate_on(
    connection: &mut Connection,
    query: &ExportQuery,
) -> QueryResult<ExportEstimate> {
    let filters = query.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, query.snapshot_seq, query.history_generation)?;
    let mut builder = export_filter(&filters, snapshot.snapshot_seq, query.scope);
    let where_sql = builder.where_sql();
    let (row_count, source_bytes) = match query.scope {
        ExportScope::Events => transaction.query_row(
            &format!(
                "SELECT COUNT(*),COALESCE(SUM(payload_bytes),0) FROM runtime_events{where_sql}"
            ),
            params_from_iter(builder.values.iter()),
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?.max(0) as u64)),
        )?,
        ExportScope::Projects | ExportScope::Sessions => {
            let (key, name, source) = export_dimension_expressions(query.scope);
            let count = transaction.query_row(
                &format!(
                    "SELECT COUNT(*) FROM (SELECT 1 FROM runtime_events{where_sql} \
                     GROUP BY {key},{name},{source})"
                ),
                params_from_iter(builder.values.iter()),
                |row| row.get::<_, i64>(0),
            )?;
            (count, (count.max(0) as u64).saturating_mul(512))
        }
    };
    let format_overhead = match query.format {
        ExportFormat::Csv => 192_u64,
        ExportFormat::Jsonl => 256_u64,
    };
    let estimated_bytes =
        if query.scope == ExportScope::Events && query.privacy == ExportPrivacy::Stored {
            source_bytes.saturating_add((row_count.max(0) as u64).saturating_mul(format_overhead))
        } else {
            (row_count.max(0) as u64).saturating_mul(format_overhead.max(384))
        };
    // Keep the builder live through the count call only; it can contain a
    // large number of owned filter strings but never row data.
    builder.values.clear();
    transaction.commit()?;
    Ok(ExportEstimate {
        api_version: API_VERSION,
        scope: query.scope,
        format: query.format,
        privacy: query.privacy,
        privacy_scope: export_privacy_scope(query.privacy),
        row_count,
        estimated_bytes,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        retained_from_seq: snapshot.retained_from_seq,
    })
}

pub fn stream_export<F>(path: &Path, query: &ExportQuery, sink: F) -> QueryResult<ExportManifest>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let mut connection = read_connection(path)?;
    stream_export_on(&mut connection, query, sink)
}

// Kept separate from the path-opening wrapper so the estimate/stream contract
// can be exercised against one read transaction in unit tests. Production
// callers still use `stream_export`, which opens the database read-only.
fn stream_export_on<F>(
    connection: &mut Connection,
    query: &ExportQuery,
    mut sink: F,
) -> QueryResult<ExportManifest>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    if query.privacy == ExportPrivacy::Stored && !query.confirm_stored {
        return Err(RuntimeQueryError::InvalidInput(
            "privacy=stored requires confirmStored=true".into(),
        ));
    }
    let filters = query.filter.normalized()?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    require_projection(&transaction)?;
    let snapshot = history_snapshot(&transaction, query.snapshot_seq, query.history_generation)?;
    let builder = export_filter(&filters, snapshot.snapshot_seq, query.scope);
    let mut row_count = 0_i64;
    let mut bytes_written = 0_u64;
    if query.format == ExportFormat::Csv {
        let header = match query.scope {
            ExportScope::Events if query.privacy == ExportPrivacy::Stored => {
                "seq,changeSeq,eventID,requestID,timestamp,kind,payloadJSON\n"
            }
            ExportScope::Events => {
                "seq,changeSeq,eventID,requestID,timestamp,kind,outcome,statusCode,clientKind,requestPurpose,endpointID,endpointName,model,projectID,projectName,sessionID,failureKind,failurePhase,upstreamStatusCode,durationMS,ttfbMS,failover,inputTokens,outputTokens,cacheReadInputTokens,cacheCreationInputTokens,reasoningTokens,processedInputTokens,processedTotalTokens\n"
            }
            ExportScope::Projects | ExportScope::Sessions => {
                "key,name,source,requests,successes,failures,cancelled,failovers,inputTokens,outputTokens,cacheReadInputTokens,cacheCreationInputTokens,processedTotalTokens,firstSeen,lastSeen,relatedCount,workspacePaths\n"
            }
        };
        export_send(&mut sink, header.as_bytes().to_vec(), &mut bytes_written)?;
    }
    match query.scope {
        ExportScope::Events => stream_event_rows(
            &transaction,
            &builder,
            query,
            &mut sink,
            &mut row_count,
            &mut bytes_written,
        )?,
        ExportScope::Projects | ExportScope::Sessions => stream_dimension_rows(
            &transaction,
            &builder,
            query,
            &mut sink,
            &mut row_count,
            &mut bytes_written,
        )?,
    }
    transaction.commit()?;
    Ok(ExportManifest {
        row_count,
        bytes_written,
        snapshot_seq: snapshot.snapshot_seq,
        history_generation: snapshot.history_generation,
        privacy: query.privacy,
    })
}

fn export_filter(filter: &RuntimeFilter, snapshot_seq: i64, scope: ExportScope) -> SqlFilter {
    let mut builder = SqlFilter::default();
    builder.raw("is_in_flight = 0");
    builder.raw(format!("projection_version = {PROJECTION_VERSION}"));
    if scope != ExportScope::Events {
        builder.raw("kind = 'client'");
        if scope == ExportScope::Projects {
            builder.raw("COALESCE(attribution_scope,'unknown') != 'internal_feature'");
        }
    }
    builder.le_i64("seq", snapshot_seq);
    append_runtime_filter(&mut builder, filter);
    builder
}

fn export_dimension_expressions(scope: ExportScope) -> (&'static str, &'static str, &'static str) {
    match scope {
        ExportScope::Projects => (
            "COALESCE(project_id,'unidentified_project')",
            "COALESCE(project_name,project_id,'unidentified_project')",
            "COALESCE(project_source,'missing_workspace_metadata')",
        ),
        ExportScope::Sessions => (
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_key,'unidentified_session')",
            "COALESCE(session_source,'missing_session_metadata')",
        ),
        ExportScope::Events => unreachable!("events are not a grouped dimension"),
    }
}

fn export_privacy_scope(privacy: ExportPrivacy) -> &'static str {
    match privacy {
        ExportPrivacy::Redacted => {
            "stable masks for event/request/session IDs; workspace locators removed"
        }
        ExportPrivacy::Stored => {
            "SQLite-stored runtime fields only; no prompt, body, headers, credentials, or full paths"
        }
    }
}

fn stream_event_rows<F>(
    transaction: &Transaction<'_>,
    builder: &SqlFilter,
    query: &ExportQuery,
    sink: &mut F,
    row_count: &mut i64,
    bytes_written: &mut u64,
) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let stored = query.privacy == ExportPrivacy::Stored;
    // Redacted exports are assembled entirely from normalized columns.  Do
    // not select the potentially large detail blob unless the caller has
    // explicitly confirmed a stored export.
    let payload_column = if stored { ",payload_json" } else { "" };
    let sql = format!(
        "SELECT seq,change_seq,event_id,request_id,timestamp,kind,outcome,status_code,\
                client_kind,request_purpose,endpoint_id,endpoint_name,effective_model,\
                project_id,project_name,session_key,failure_kind,failure_phase,\
                upstream_status_code,duration_ms,ttfb_ms,failover,input_tokens,output_tokens,\
                cache_read_input_tokens,cache_creation_input_tokens,reasoning_tokens,\
                processed_input_tokens,processed_total_tokens{payload_column} \
         FROM runtime_events{} ORDER BY seq ASC",
        builder.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(builder.values.iter()))?;
    while let Some(row) = rows.next()? {
        let event = ExportEventRow {
            seq: row.get(0)?,
            change_seq: row.get(1)?,
            event_id: row.get(2)?,
            request_id: row.get(3)?,
            timestamp: row.get(4)?,
            kind: row.get(5)?,
            outcome: row.get(6)?,
            status_code: row.get(7)?,
            client_kind: row.get(8)?,
            request_purpose: row.get(9)?,
            endpoint_id: row.get(10)?,
            endpoint_name: row.get(11)?,
            effective_model: row.get(12)?,
            project_id: row.get(13)?,
            project_name: row.get(14)?,
            session_key: row.get(15)?,
            failure_kind: row.get(16)?,
            failure_phase: row.get(17)?,
            upstream_status_code: row.get(18)?,
            duration_ms: row.get(19)?,
            ttfb_ms: row.get(20)?,
            failover: row.get::<_, Option<i64>>(21)?.unwrap_or(0) != 0,
            input_tokens: row.get(22)?,
            output_tokens: row.get(23)?,
            cache_read_input_tokens: row.get(24)?,
            cache_creation_input_tokens: row.get(25)?,
            reasoning_tokens: row.get(26)?,
            processed_input_tokens: row.get(27)?,
            processed_total_tokens: row.get(28)?,
            payload_json: stored.then(|| row.get(29)).transpose()?,
        };
        let bytes = export_event_bytes(&event, query)?;
        export_send(sink, bytes, bytes_written)?;
        *row_count = row_count.saturating_add(1);
    }
    Ok(())
}

fn export_event_bytes(event: &ExportEventRow, query: &ExportQuery) -> QueryResult<Vec<u8>> {
    let event_id = match query.privacy {
        ExportPrivacy::Redacted => stable_mask("event", &event.event_id),
        ExportPrivacy::Stored => event.event_id.clone(),
    };
    let request_id = redact_optional("request", event.request_id.as_deref(), query.privacy);
    if query.format == ExportFormat::Jsonl {
        let value = if query.privacy == ExportPrivacy::Stored {
            let payload_json =
                event
                    .payload_json
                    .as_deref()
                    .ok_or_else(|| RuntimeQueryError::CorruptPayload {
                        event_id: event.event_id.clone(),
                        detail: "stored export row did not include payload_json".into(),
                    })?;
            let payload =
                serde_json::from_str::<serde_json::Value>(payload_json).map_err(|error| {
                    RuntimeQueryError::CorruptPayload {
                        event_id: event.event_id.clone(),
                        detail: error.to_string(),
                    }
                })?;
            serde_json::json!({
                "seq": event.seq,
                "changeSeq": event.change_seq,
                "eventID": event_id,
                "requestID": request_id,
                "event": payload,
            })
        } else {
            serde_json::json!({
                "seq": event.seq, "changeSeq": event.change_seq,
                "eventID": event_id, "requestID": request_id,
                "timestamp": event.timestamp, "kind": event.kind, "outcome": event.outcome,
                "statusCode": event.status_code, "clientKind": event.client_kind,
                "requestPurpose": event.request_purpose, "endpointID": event.endpoint_id,
                "endpointName": event.endpoint_name, "model": event.effective_model,
                "projectID": event.project_id, "projectName": event.project_name,
                "sessionID": redact_optional("session", event.session_key.as_deref(), query.privacy),
                "failureKind": event.failure_kind, "failurePhase": event.failure_phase,
                "upstreamStatusCode": event.upstream_status_code,
                "durationMS": event.duration_ms, "ttfbMS": event.ttfb_ms,
                "failover": event.failover, "inputTokens": event.input_tokens,
                "outputTokens": event.output_tokens,
                "cacheReadInputTokens": event.cache_read_input_tokens,
                "cacheCreationInputTokens": event.cache_creation_input_tokens,
                "reasoningTokens": event.reasoning_tokens,
                "processedInputTokens": event.processed_input_tokens,
                "processedTotalTokens": event.processed_total_tokens,
            })
        };
        let mut bytes = serde_json::to_vec(&value)
            .map_err(|error| RuntimeQueryError::Output(error.to_string()))?;
        bytes.push(b'\n');
        return Ok(bytes);
    }
    let fields = if query.privacy == ExportPrivacy::Stored {
        vec![
            event.seq.to_string(),
            event.change_seq.to_string(),
            event_id,
            request_id.unwrap_or_default(),
            event.timestamp.to_string(),
            event.kind.clone(),
            event.payload_json.clone().unwrap_or_default(),
        ]
    } else {
        vec![
            event.seq.to_string(),
            event.change_seq.to_string(),
            event_id,
            request_id.unwrap_or_default(),
            event.timestamp.to_string(),
            event.kind.clone(),
            event.outcome.clone().unwrap_or_default(),
            event.status_code.to_string(),
            event.client_kind.clone().unwrap_or_default(),
            event.request_purpose.clone().unwrap_or_default(),
            event.endpoint_id.clone().unwrap_or_default(),
            event.endpoint_name.clone().unwrap_or_default(),
            event.effective_model.clone().unwrap_or_default(),
            event.project_id.clone().unwrap_or_default(),
            event.project_name.clone().unwrap_or_default(),
            redact_optional("session", event.session_key.as_deref(), query.privacy)
                .unwrap_or_default(),
            event.failure_kind.clone().unwrap_or_default(),
            event.failure_phase.clone().unwrap_or_default(),
            option_csv(event.upstream_status_code),
            option_csv(event.duration_ms),
            option_csv(event.ttfb_ms),
            event.failover.to_string(),
            option_csv(event.input_tokens),
            option_csv(event.output_tokens),
            option_csv(event.cache_read_input_tokens),
            option_csv(event.cache_creation_input_tokens),
            option_csv(event.reasoning_tokens),
            option_csv(event.processed_input_tokens),
            option_csv(event.processed_total_tokens),
        ]
    };
    Ok(csv_line(fields).into_bytes())
}

fn stream_dimension_rows<F>(
    transaction: &Transaction<'_>,
    builder: &SqlFilter,
    query: &ExportQuery,
    sink: &mut F,
    row_count: &mut i64,
    bytes_written: &mut u64,
) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    let (key, name, source) = export_dimension_expressions(query.scope);
    let (related, workspace) = match query.scope {
        ExportScope::Projects => ("session_key", "MAX(workspace_paths_json)"),
        ExportScope::Sessions => ("project_id", "NULL"),
        ExportScope::Events => unreachable!(),
    };
    let sql = format!(
        "SELECT {key},{name},{source},COUNT(*),\
                SUM(CASE WHEN outcome='succeeded' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN outcome='failed' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN outcome='cancelled' THEN 1 ELSE 0 END),\
                SUM(CASE WHEN failover=1 THEN 1 ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(output_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(cache_read_input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(cache_creation_input_tokens,0) ELSE 0 END),\
                SUM(CASE WHEN request_purpose!='token_count' OR request_purpose IS NULL THEN COALESCE(processed_total_tokens,0) ELSE 0 END),\
                MIN(timestamp),MAX(timestamp),COUNT(DISTINCT {related}),{workspace} \
         FROM runtime_events{} GROUP BY {key},{name},{source} ORDER BY {key} ASC",
        builder.where_sql()
    );
    let mut statement = transaction.prepare(&sql)?;
    let mut rows = statement.query(params_from_iter(builder.values.iter()))?;
    while let Some(row) = rows.next()? {
        let mut key_value = row.get::<_, String>(0)?;
        if query.privacy == ExportPrivacy::Redacted && query.scope == ExportScope::Sessions {
            key_value = stable_mask("session", &key_value);
        }
        let workspace_paths = if query.privacy == ExportPrivacy::Stored {
            row.get::<_, Option<String>>(16)?
                .unwrap_or_else(|| "[]".into())
        } else {
            "[]".into()
        };
        let fields = vec![
            key_value.clone(),
            if query.scope == ExportScope::Sessions {
                key_value.clone()
            } else {
                row.get::<_, String>(1)?
            },
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?.to_string(),
            row.get::<_, i64>(4)?.to_string(),
            row.get::<_, i64>(5)?.to_string(),
            row.get::<_, i64>(6)?.to_string(),
            row.get::<_, i64>(7)?.to_string(),
            row.get::<_, i64>(8)?.to_string(),
            row.get::<_, i64>(9)?.to_string(),
            row.get::<_, i64>(10)?.to_string(),
            row.get::<_, i64>(11)?.to_string(),
            row.get::<_, i64>(12)?.to_string(),
            row.get::<_, f64>(13)?.to_string(),
            row.get::<_, f64>(14)?.to_string(),
            row.get::<_, i64>(15)?.to_string(),
            workspace_paths,
        ];
        let bytes = if query.format == ExportFormat::Csv {
            csv_line(fields).into_bytes()
        } else {
            let value = serde_json::json!({
                "key": fields[0], "name": fields[1], "source": fields[2],
                "requests": fields[3].parse::<i64>().unwrap_or(0),
                "successes": fields[4].parse::<i64>().unwrap_or(0),
                "failures": fields[5].parse::<i64>().unwrap_or(0),
                "cancelled": fields[6].parse::<i64>().unwrap_or(0),
                "failovers": fields[7].parse::<i64>().unwrap_or(0),
                "inputTokens": fields[8].parse::<i64>().unwrap_or(0),
                "outputTokens": fields[9].parse::<i64>().unwrap_or(0),
                "cacheReadInputTokens": fields[10].parse::<i64>().unwrap_or(0),
                "cacheCreationInputTokens": fields[11].parse::<i64>().unwrap_or(0),
                "processedTotalTokens": fields[12].parse::<i64>().unwrap_or(0),
                "firstSeen": fields[13].parse::<f64>().unwrap_or(0.0),
                "lastSeen": fields[14].parse::<f64>().unwrap_or(0.0),
                "relatedCount": fields[15].parse::<i64>().unwrap_or(0),
                "workspacePaths": serde_json::from_str::<Vec<String>>(&fields[16]).unwrap_or_default(),
            });
            let mut bytes = serde_json::to_vec(&value)
                .map_err(|error| RuntimeQueryError::Output(error.to_string()))?;
            bytes.push(b'\n');
            bytes
        };
        export_send(sink, bytes, bytes_written)?;
        *row_count = row_count.saturating_add(1);
    }
    Ok(())
}

fn redact_optional(label: &str, value: Option<&str>, privacy: ExportPrivacy) -> Option<String> {
    value.map(|value| match privacy {
        ExportPrivacy::Redacted => stable_mask(label, value),
        ExportPrivacy::Stored => value.to_owned(),
    })
}

fn stable_mask(label: &str, value: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{label}_{hash:016x}")
}

fn option_csv<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

fn csv_line(fields: Vec<String>) -> String {
    let mut line = fields
        .into_iter()
        .map(|value| {
            if value.contains([',', '"', '\n', '\r']) {
                format!("\"{}\"", value.replace('"', "\"\""))
            } else {
                value
            }
        })
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

fn export_send<F>(sink: &mut F, bytes: Vec<u8>, bytes_written: &mut u64) -> QueryResult<()>
where
    F: FnMut(Vec<u8>) -> Result<(), String>,
{
    *bytes_written = bytes_written.saturating_add(bytes.len() as u64);
    sink(bytes).map_err(RuntimeQueryError::Output)
}

pub fn storage_details(path: &Path) -> QueryResult<StorageProbe> {
    let mut connection = read_connection(path)?;
    storage_details_on(&mut connection, Some(path))
}

fn has_legacy_retention_columns(connection: &Connection) -> QueryResult<bool> {
    const LEGACY_COLUMNS: [&str; 7] = [
        "max_events",
        "max_age_days",
        "max_live_bytes",
        "evicted_events",
        "evicted_requests",
        "over_limit",
        "last_pruned_at",
    ];
    let mut statement = connection.prepare("PRAGMA table_info(runtime_retention)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns
        .iter()
        .any(|column| LEGACY_COLUMNS.contains(&column.as_str())))
}

pub fn storage_details_on(
    connection: &mut Connection,
    path: Option<&Path>,
) -> QueryResult<StorageProbe> {
    const REQUIRED_INDEXES: [&str; 8] = [
        "runtime_events_inflight_change_v2",
        "runtime_events_outcome_seq_v2",
        "runtime_events_time_seq_v2",
        "runtime_events_session_time_v2",
        "runtime_events_project_time_v2",
        "runtime_events_endpoint_time_v2",
        "runtime_events_model_time_v2",
        "runtime_events_failure_time_v2",
    ];
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    let (
        retained_events,
        completed_events,
        in_flight_events,
        min_seq,
        max_seq,
        earliest_timestamp,
        latest_timestamp,
        payload_bytes,
    ) = transaction.query_row(
        "SELECT COUNT(*),\
                COALESCE(SUM(CASE WHEN is_in_flight=0 THEN 1 ELSE 0 END),0),\
                COALESCE(SUM(CASE WHEN is_in_flight=1 THEN 1 ELSE 0 END),0),\
                MIN(seq),MAX(seq),MIN(timestamp),MAX(timestamp),\
                COALESCE(SUM(payload_bytes),0) \
         FROM runtime_events",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
                row.get::<_, Option<f64>>(6)?,
                row.get::<_, i64>(7)?,
            ))
        },
    )?;
    let max_age_days = meta_i64(&transaction, "retention_max_age_days")?;
    let storage_limit_bytes = meta_i64(&transaction, "storage_limit_bytes")?;
    let reset_generation = meta_i64(&transaction, "reset_generation")?.unwrap_or(0);
    let legacy_retention_detected = has_legacy_retention_columns(&transaction)?;
    let retention = transaction.query_row(
        "SELECT revision \
         FROM runtime_retention WHERE id=1",
        [],
        |row| {
            Ok(RetentionStatus {
                revision: row.get(0)?,
                max_age_days,
                storage_limit_bytes,
            })
        },
    )?;
    let existing_indexes = {
        let mut statement = transaction.prepare(
            "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'runtime_events_%_v2'",
        )?;
        statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let missing_indexes = REQUIRED_INDEXES
        .into_iter()
        .filter(|required| !existing_indexes.iter().any(|actual| actual == required))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let page_size = transaction
        .query_row("PRAGMA page_size", [], |row| row.get::<_, i64>(0))?
        .max(0) as u64;
    let page_count = transaction
        .query_row("PRAGMA page_count", [], |row| row.get::<_, i64>(0))?
        .max(0) as u64;
    let freelist_count = transaction
        .query_row("PRAGMA freelist_count", [], |row| row.get::<_, i64>(0))?
        .max(0) as u64;
    let allocated_bytes = page_size.saturating_mul(page_count);
    let freelist_bytes = page_size.saturating_mul(freelist_count);
    let live_bytes = allocated_bytes.saturating_sub(freelist_bytes);
    let (database_bytes, wal_bytes) = path.map_or((0, 0), database_file_sizes);
    let projection_indexes_ready = meta_i64(&transaction, "projection_indexes_ready")?.unwrap_or(0)
        == 1
        && missing_indexes.is_empty();
    let hourly_rollup_dirty_buckets = transaction.query_row(
        "SELECT COUNT(*) FROM runtime_hourly_rollup_dirty",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    let result = StorageProbe {
        api_version: API_VERSION,
        backend: "sqlite",
        schema_version: meta_i64(&transaction, "schema_version")?.unwrap_or(0),
        projection_version: PROJECTION_VERSION,
        projection_backfill_cursor: meta_i64(&transaction, "projection_backfill_cursor")?
            .unwrap_or(0),
        projection_backfill_complete: meta_i64(&transaction, "projection_backfill_complete")?
            .unwrap_or(0)
            == 1,
        projection_indexes_ready,
        missing_indexes,
        hourly_rollup_complete: meta_i64(&transaction, "hourly_rollup_complete")?.unwrap_or(0) == 1,
        hourly_rollup_max_seq: meta_i64(&transaction, "hourly_rollup_max_seq")?.unwrap_or(0),
        hourly_rollup_history_generation: meta_i64(
            &transaction,
            "hourly_rollup_history_generation",
        )?
        .unwrap_or(0),
        hourly_rollup_failed: meta_i64(&transaction, "hourly_rollup_failed")?.unwrap_or(0) == 1,
        hourly_rollup_dirty_buckets,
        retained_events,
        completed_events,
        in_flight_events,
        min_seq,
        max_seq,
        earliest_timestamp,
        latest_timestamp,
        retained_from_seq: meta_i64(&transaction, "retained_from_seq")?.unwrap_or(0),
        history_generation: meta_i64(&transaction, "history_generation")?.unwrap_or(0),
        reset_generation,
        user_deleted_events: meta_i64(&transaction, "user_deleted_events")?.unwrap_or(0),
        user_deleted_requests: meta_i64(&transaction, "user_deleted_requests")?.unwrap_or(0),
        payload_bytes,
        database_bytes,
        live_bytes,
        allocated_bytes,
        freelist_bytes,
        wal_bytes,
        pending_events: None,
        pending_bytes: None,
        retention,
        legacy_retention_detected,
    };
    transaction.commit()?;
    Ok(result)
}

fn database_file_sizes(path: &Path) -> (u64, u64) {
    let database_bytes = std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let wal_path = PathBuf::from(format!("{}-wal", path.display()));
    let wal_bytes = std::fs::metadata(wal_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    (database_bytes, wal_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

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
                   request_method TEXT,request_path TEXT,route_intent TEXT
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
                 ) VALUES (?1,?1,?2,?3,1.0,?4,'completed',?5,?6,?7,?8,?9,0,'{}',6,?10)",
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
                   6,80,printf('session-%03d',seq%500),'header',printf('project-%03d',seq%100),
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
        let estimate = export_estimate_on(&mut connection, &export_query)
            .expect("project alias export estimate");
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
}
