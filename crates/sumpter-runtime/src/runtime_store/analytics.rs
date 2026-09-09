use super::{
    BTreeMap, BTreeSet, Digest, HashMap, HashSet, KIND_CLIENT, KIND_UPSTREAM,
    MAX_PROJECT_WORKSPACE_PATHS, RESOURCE_ROUTING_MODEL, RequestPurpose, RuntimeEvent, Sha256,
    Value, codex_attribution_scope, json, local_user_from_workspace_path, option_token,
};

#[derive(Default)]
pub(super) struct DimensionRow {
    attempts: i64,
    successes: i64,
    failures: i64,
    cancelled: i64,
    failovers: i64,
    duration_total: f64,
    duration_count: i64,
    ttfb_total: f64,
    ttfb_count: i64,
    event_ids: Vec<String>,
    token_usage: TokenUsage,
    /// Sanitized workspace suffixes carried by project rows only.  The core
    /// parser has already removed the absolute prefix before events reach the
    /// store, so this is safe to expose as a local Finder locator.
    workspace_paths: BTreeSet<String>,
    /// Only project rows populate this field.  It remains optional in the
    /// wire shape so older/non-project dimensions keep their compact schema.
    project_source: Option<String>,
}

#[derive(Default)]
pub(super) struct SessionContext {
    projects: BTreeSet<String>,
    client_kinds: BTreeSet<String>,
}

#[derive(Default, Clone, Copy)]
pub(super) struct TokenUsage {
    cache_read: sumpter_core::cache_read::CacheReadStatistics,
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
    uncached_input_tokens: i64,
    processed_input_tokens: i64,
    processed_total_tokens: i64,
    observed_requests: i64,
    cache_read_reported_requests: i64,
    cache_read_hit_requests: i64,
    cache_read_token_eligible_requests: i64,
    cache_read_token_unknown_requests: i64,
    cache_read_token_numerator: i64,
    cache_read_token_denominator: i64,
    semantics: TokenAccountingSemantics,
    quality: TokenAccountingQuality,
    usage_field_presence: UsageFieldPresence,
}

#[derive(Default, Clone, Copy)]
pub(super) struct UsageFieldPresence {
    input_tokens: i64,
    output_tokens: i64,
    cache_read_input_tokens: i64,
    cache_creation_input_tokens: i64,
    reasoning_tokens: i64,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenAccountingSemantics {
    #[default]
    Unknown,
    Subset,
    Independent,
    Mixed,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum TokenAccountingQuality {
    #[default]
    Unknown,
    Partial,
    Complete,
    Mixed,
}

impl TokenUsage {
    fn add_trace(&mut self, event: &RuntimeEvent) {
        if event.phase == Some(sumpter_core::events::RuntimeEventPhase::Completed) {
            let cache = event.observed_cache_read();
            self.cache_read.add(cache.state, cache.reason, 1);
        }
        if event.request_purpose == Some(RequestPurpose::TokenCount) {
            return;
        }
        let Some(usage) = event
            .stream_trace
            .as_ref()
            .and_then(|trace| trace.usage.as_ref())
        else {
            return;
        };
        self.usage_field_presence.input_tokens += i64::from(usage.input_tokens.is_some());
        self.usage_field_presence.output_tokens += i64::from(usage.output_tokens.is_some());
        self.usage_field_presence.cache_read_input_tokens +=
            i64::from(usage.cache_read_input_tokens.is_some());
        self.usage_field_presence.cache_creation_input_tokens +=
            i64::from(usage.cache_creation_input_tokens.is_some());
        self.usage_field_presence.reasoning_tokens += i64::from(usage.reasoning_tokens.is_some());
        self.input_tokens = self
            .input_tokens
            .saturating_add(usage.input_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        self.output_tokens = self
            .output_tokens
            .saturating_add(usage.output_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        self.cache_read_input_tokens = self.cache_read_input_tokens.saturating_add(
            usage
                .cache_read_input_tokens
                .unwrap_or(0)
                .min(i64::MAX as u64) as i64,
        );
        self.cache_creation_input_tokens = self.cache_creation_input_tokens.saturating_add(
            usage
                .cache_creation_input_tokens
                .unwrap_or(0)
                .min(i64::MAX as u64) as i64,
        );
        self.reasoning_tokens = self
            .reasoning_tokens
            .saturating_add(usage.reasoning_tokens.unwrap_or(0).min(i64::MAX as u64) as i64);
        let (semantics, processed, uncached) = super::normalized_input_tokens(
            event.target_format.or(event.source_format),
            super::token_i64(usage.input_tokens),
            super::token_i64(usage.cache_read_input_tokens),
            super::token_i64(usage.cache_creation_input_tokens),
        );
        let semantics = match semantics {
            "independent" => TokenAccountingSemantics::Independent,
            "subset" => TokenAccountingSemantics::Subset,
            _ => TokenAccountingSemantics::Unknown,
        };
        let processed_input = processed.unwrap_or(0);
        let uncached_input = uncached.unwrap_or(0);
        self.uncached_input_tokens = self.uncached_input_tokens.saturating_add(uncached_input);
        self.processed_input_tokens = self.processed_input_tokens.saturating_add(processed_input);
        self.processed_total_tokens = self
            .processed_total_tokens
            .saturating_add(processed_input)
            .saturating_add(if processed.is_some() {
                usage.output_tokens.unwrap_or(0).min(i64::MAX as u64) as i64
            } else {
                0
            });
        if let Some(cache_read) = usage.cache_read_input_tokens {
            let cache_read = cache_read.min(i64::MAX as u64) as i64;
            self.cache_read_reported_requests = self.cache_read_reported_requests.saturating_add(1);
            if cache_read > 0 {
                self.cache_read_hit_requests = self.cache_read_hit_requests.saturating_add(1);
            }
            if matches!(
                semantics,
                TokenAccountingSemantics::Subset | TokenAccountingSemantics::Independent
            ) && usage.input_tokens.is_some()
            {
                self.cache_read_token_eligible_requests =
                    self.cache_read_token_eligible_requests.saturating_add(1);
                self.cache_read_token_numerator = self
                    .cache_read_token_numerator
                    .saturating_add(cache_read.max(0));
                self.cache_read_token_denominator = self
                    .cache_read_token_denominator
                    .saturating_add(processed_input.max(0));
            } else {
                self.cache_read_token_unknown_requests =
                    self.cache_read_token_unknown_requests.saturating_add(1);
            }
        } else {
            self.cache_read_token_unknown_requests =
                self.cache_read_token_unknown_requests.saturating_add(1);
        }
        self.semantics = if self.observed_requests == 0 {
            semantics
        } else {
            merge_semantics(self.semantics, semantics)
        };
        let quality = if usage.input_tokens.is_some() && usage.output_tokens.is_some() {
            TokenAccountingQuality::Complete
        } else {
            TokenAccountingQuality::Partial
        };
        self.quality = merge_quality(self.quality, quality);
        self.observed_requests = self.observed_requests.saturating_add(1);
    }

    pub(super) fn value(self) -> Value {
        json!({
            "inputTokens": self.input_tokens,
            "outputTokens": self.output_tokens,
            "cacheReadInputTokens": self.cache_read_input_tokens,
            "cacheCreationInputTokens": self.cache_creation_input_tokens,
            "reasoningTokens": self.reasoning_tokens,
            "uncachedInputTokens": self.uncached_input_tokens,
            "processedInputTokens": self.processed_input_tokens,
            "processedTotalTokens": self.processed_total_tokens,
            "totalTokens": self.input_tokens.saturating_add(self.output_tokens),
            "observedRequests": self.observed_requests,
            "cacheReadReportedRequests": self.cache_read_reported_requests,
            "cacheReadHitRequests": self.cache_read_hit_requests,
            "cacheReadTokenEligibleRequests": self.cache_read_token_eligible_requests,
            "cacheReadTokenUnknownRequests": self.cache_read_token_unknown_requests,
            "cacheReadTokenRate": (self.cache_read_token_denominator > 0).then(|| {
                (self.cache_read_token_numerator as f64
                    / self.cache_read_token_denominator as f64)
                    .clamp(0.0, 1.0)
            }),
            "cacheRead": self.cache_read,
            "cacheReadRequestRate": (self.cache_read_reported_requests > 0).then(|| {
                (self.cache_read_hit_requests as f64
                    / self.cache_read_reported_requests as f64)
                    .clamp(0.0, 1.0)
            }),
            "tokenAccountingSemantics": self.semantics.as_str(),
            "tokenAccountingQuality": self.quality.as_str(),
            "usageFieldPresence": {
                "inputTokens": self.usage_field_presence.input_tokens,
                "outputTokens": self.usage_field_presence.output_tokens,
                "cacheReadInputTokens": self.usage_field_presence.cache_read_input_tokens,
                "cacheCreationInputTokens": self.usage_field_presence.cache_creation_input_tokens,
                "reasoningTokens": self.usage_field_presence.reasoning_tokens,
            },
        })
    }
}

impl TokenAccountingSemantics {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Subset => "subset",
            Self::Independent => "independent",
            Self::Mixed => "mixed",
        }
    }
}

impl TokenAccountingQuality {
    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Partial => "partial",
            Self::Complete => "complete",
            Self::Mixed => "mixed",
        }
    }
}

pub(super) fn merge_semantics(
    current: TokenAccountingSemantics,
    next: TokenAccountingSemantics,
) -> TokenAccountingSemantics {
    // An unknown protocol cannot be safely folded into a known denominator.
    // Propagate it instead of labelling the aggregate merely `mixed`, which
    // would make the UI present a plausible but unverifiable hit rate.
    if current == TokenAccountingSemantics::Unknown || next == TokenAccountingSemantics::Unknown {
        return TokenAccountingSemantics::Unknown;
    }
    match (current, next) {
        (left, right) if left == right => left,
        _ => TokenAccountingSemantics::Mixed,
    }
}

pub(super) fn merge_quality(
    current: TokenAccountingQuality,
    next: TokenAccountingQuality,
) -> TokenAccountingQuality {
    match (current, next) {
        (TokenAccountingQuality::Unknown, value) | (value, TokenAccountingQuality::Unknown) => {
            value
        }
        (left, right) if left == right => left,
        _ => TokenAccountingQuality::Mixed,
    }
}

#[derive(Default)]
pub(super) struct AnalyticsAggregate {
    pub(super) upstream_request_ids: HashSet<String>,
    pub(super) request_endpoints: HashMap<String, String>,
    pub(super) endpoints: BTreeMap<String, DimensionRow>,
    pub(super) models: BTreeMap<String, DimensionRow>,
    pub(super) client_kinds: BTreeMap<String, DimensionRow>,
    pub(super) request_purposes: BTreeMap<String, DimensionRow>,
    pub(super) feature_rules: BTreeMap<String, DimensionRow>,
    pub(super) protocol_routes: BTreeMap<String, DimensionRow>,
    pub(super) failure_kinds: BTreeMap<String, DimensionRow>,
    pub(super) failure_phases: BTreeMap<String, DimensionRow>,
    pub(super) upstream_statuses: BTreeMap<String, DimensionRow>,
    pub(super) stream_terminals: BTreeMap<String, DimensionRow>,
    pub(super) projects: BTreeMap<String, DimensionRow>,
    pub(super) sessions: BTreeMap<String, DimensionRow>,
    pub(super) session_context: BTreeMap<String, SessionContext>,
    pub(super) tool_calls: BTreeMap<String, i64>,
    pub(super) codex_metadata_present: i64,
    pub(super) token_usage: TokenUsage,
    pub(super) client_requests: i64,
    pub(super) client_successes: i64,
    pub(super) client_failures: i64,
    pub(super) client_cancelled: i64,
    pub(super) upstream_attempts: i64,
    pub(super) upstream_successes: i64,
    pub(super) upstream_failures: i64,
    pub(super) failovers: i64,
}

impl AnalyticsAggregate {
    pub(super) fn with_request_metadata(
        upstream_request_ids: HashSet<String>,
        request_endpoints: HashMap<String, String>,
    ) -> Self {
        Self {
            upstream_request_ids,
            request_endpoints,
            ..Self::default()
        }
    }
    pub(super) fn add(&mut self, event: &RuntimeEvent, project_keys: &HashMap<String, String>) {
        if event.kind == KIND_CLIENT {
            self.client_requests += 1;
            if event.is_succeeded() {
                self.client_successes += 1;
            } else if event.is_failed() {
                self.client_failures += 1;
            } else if event.is_cancelled() {
                self.client_cancelled += 1;
            }
            self.failovers += i64::from(event.failover);
        } else if event.kind == KIND_UPSTREAM {
            self.upstream_attempts += 1;
            if event.is_succeeded() {
                self.upstream_successes += 1;
            } else if event.is_failed() {
                self.upstream_failures += 1;
            }
        }
        if event.kind == KIND_CLIENT {
            let model = event
                .effective_model
                .clone()
                .or_else(|| event.client_model.clone())
                .or_else(|| event.upstream_model.clone())
                .unwrap_or_else(|| "unrecorded".into());
            // Keep the client dimension key identical to the filter/facet key;
            // otherwise the UI cannot select the legacy bucket it displays.
            let client_kind =
                option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into());
            let request_purpose =
                option_token(event.request_purpose).unwrap_or_else(|| "unrecorded".into());
            let feature_rule = event
                .feature_rule_id
                .clone()
                .unwrap_or_else(|| "none".into());
            let source = event
                .source_format
                .map(|value| value.token())
                .unwrap_or("unrecorded");
            let target = event
                .target_format
                .map(|value| value.token())
                .unwrap_or("unrecorded");
            let mode = option_token(event.route_mode).unwrap_or_else(|| "unrecorded".into());
            for (rows, key) in [
                (&mut self.models, model),
                (&mut self.client_kinds, client_kind),
                (&mut self.request_purposes, request_purpose),
                (&mut self.feature_rules, feature_rule),
                (
                    &mut self.protocol_routes,
                    format!("{source}->{target}:{mode}"),
                ),
            ] {
                update_dimension(rows.entry(key).or_default(), event);
            }
            // A completed client event and its upstream attempt(s) describe
            // the same request. Use upstream rows for endpoint counts so
            // failover attempts are visible without double-counting the
            // winning client row. Early routing failures have no upstream row
            // and keep the client-side endpoint as a fallback.
            let fallback_endpoint = event
                .endpoint_name
                .clone()
                .or_else(|| event.endpoint_id.clone())
                .unwrap_or_else(|| "unassigned".into());
            if let Some(request_id) = event
                .request_id
                .as_ref()
                .filter(|request_id| self.upstream_request_ids.contains(*request_id))
            {
                if let Some(endpoint) = self.request_endpoints.get(request_id) {
                    // Client usage is the authoritative response usage. Attribute
                    // it to the final successful endpoint (or the last attempt)
                    // without adding a duplicate endpoint attempt.
                    self.endpoints
                        .entry(endpoint.clone())
                        .or_default()
                        .token_usage
                        .add_trace(event);
                }
            } else {
                update_dimension(self.endpoints.entry(fallback_endpoint).or_default(), event);
            }
            for name in event.tool_calls.as_deref().unwrap_or_default() {
                *self.tool_calls.entry(name.clone()).or_default() += 1;
            }
            if let Some(trace) = &event.stream_trace {
                let terminal = trace
                    .terminal_event
                    .clone()
                    .unwrap_or_else(|| "unobserved".into());
                update_dimension(self.stream_terminals.entry(terminal).or_default(), event);
            }
            if event.codex_metadata.is_some() {
                self.codex_metadata_present += 1;
            }
            self.token_usage.add_trace(event);
            let project = project_key(event, project_keys);
            let project_row = self.projects.entry(project.clone()).or_default();
            add_workspace_paths(project_row, event);
            merge_project_source(&mut project_row.project_source, project_source(event));
            update_dimension(project_row, event);
            let session = session_key(event);
            update_dimension(self.sessions.entry(session.clone()).or_default(), event);
            let context = self.session_context.entry(session).or_default();
            context.projects.insert(project);
            context.client_kinds.insert(
                option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into()),
            );
        } else if event.kind == KIND_UPSTREAM {
            let endpoint = event
                .endpoint_name
                .clone()
                .or_else(|| event.endpoint_id.clone())
                .unwrap_or_else(|| "unassigned".into());
            update_dimension(self.endpoints.entry(endpoint).or_default(), event);
            let upstream_status = event
                .upstream_status_code
                .map(|status| status.to_string())
                .unwrap_or_else(|| "before_headers".into());
            update_dimension(
                self.upstream_statuses.entry(upstream_status).or_default(),
                event,
            );
        }
        if event.kind == KIND_CLIENT && event.is_failed() {
            let failure_kind =
                option_token(event.failure_kind).unwrap_or_else(|| "unrecorded".into());
            let failure_phase =
                option_token(event.failure_phase).unwrap_or_else(|| "unrecorded".into());
            update_dimension(self.failure_kinds.entry(failure_kind).or_default(), event);
            update_dimension(self.failure_phases.entry(failure_phase).or_default(), event);
        }
    }
}

pub(super) fn build_request_endpoints(events: &[RuntimeEvent]) -> HashMap<String, String> {
    let mut result = HashMap::<String, (bool, String)>::new();
    for event in events.iter().filter(|event| event.kind == KIND_UPSTREAM) {
        let Some(request_id) = event.request_id.as_ref() else {
            continue;
        };
        let Some(endpoint) = event
            .endpoint_name
            .clone()
            .or_else(|| event.endpoint_id.clone())
        else {
            continue;
        };
        let succeeded = event.is_succeeded();
        match result.get(request_id) {
            Some((true, _)) if !succeeded => {}
            _ => {
                result.insert(request_id.clone(), (succeeded, endpoint));
            }
        }
    }
    result
        .into_iter()
        .map(|(request_id, (_, endpoint))| (request_id, endpoint))
        .collect()
}

pub(super) fn update_dimension(row: &mut DimensionRow, event: &RuntimeEvent) {
    row.attempts += 1;
    if event.is_succeeded() {
        row.successes += 1;
    } else if event.is_failed() {
        row.failures += 1;
    } else if event.is_cancelled() {
        row.cancelled += 1;
    }
    row.failovers += i64::from(event.failover);
    if event.duration_ms >= 0 {
        row.duration_total += event.duration_ms as f64;
        row.duration_count += 1;
    }
    if let Some(ttfb) = event.ttfb_ms {
        row.ttfb_total += ttfb as f64;
        row.ttfb_count += 1;
    }
    if row.event_ids.len() < 3 && !row.event_ids.contains(&event.id) {
        row.event_ids.push(event.id.clone());
    }
    if event.kind == KIND_CLIENT {
        row.token_usage.add_trace(event);
    }
}

pub(super) fn dimension_value(rows: &BTreeMap<String, DimensionRow>) -> Value {
    Value::Array(
        rows.iter()
            .map(|(name, row)| dimension_row_value(name, row))
            .collect(),
    )
}

pub(super) fn dimension_row_value(name: &str, row: &DimensionRow) -> Value {
    let pending = row
        .attempts
        .saturating_sub(row.successes + row.failures + row.cancelled);
    let mut value = json!({
        "name": name,
        "attempts": row.attempts,
        "successes": row.successes,
        "failures": row.failures,
        "cancelled": row.cancelled,
        "pending": pending,
        "successRate": success_rate(row.successes, row.failures, row.cancelled),
                    "failovers": row.failovers,
                    "averageDurationMS": (row.duration_count > 0).then_some(row.duration_total / row.duration_count as f64),
                    "averageTTFBMS": (row.ttfb_count > 0).then_some(row.ttfb_total / row.ttfb_count as f64),
                    "eventIDs": row.event_ids,
                    "inputTokens": row.token_usage.input_tokens,
                    "outputTokens": row.token_usage.output_tokens,
                    "cacheReadInputTokens": row.token_usage.cache_read_input_tokens,
                    "cacheCreationInputTokens": row.token_usage.cache_creation_input_tokens,
                    "reasoningTokens": row.token_usage.reasoning_tokens,
                    "uncachedInputTokens": row.token_usage.uncached_input_tokens,
                    "processedInputTokens": row.token_usage.processed_input_tokens,
                    "processedTotalTokens": row.token_usage.processed_total_tokens,
                    "totalTokens": row.token_usage.input_tokens.saturating_add(row.token_usage.output_tokens),
                    "observedRequests": row.token_usage.observed_requests,
                    "tokenAccountingSemantics": row.token_usage.semantics.as_str(),
                    "tokenAccountingQuality": row.token_usage.quality.as_str(),
                    "usageFieldPresence": {
                        "inputTokens": row.token_usage.usage_field_presence.input_tokens,
                        "outputTokens": row.token_usage.usage_field_presence.output_tokens,
                        "cacheReadInputTokens": row.token_usage.usage_field_presence.cache_read_input_tokens,
                        "cacheCreationInputTokens": row.token_usage.usage_field_presence.cache_creation_input_tokens,
                        "reasoningTokens": row.token_usage.usage_field_presence.reasoning_tokens,
                    },
    });
    if let Some(source) = row.project_source.as_deref()
        && let Some(object) = value.as_object_mut()
    {
        object.insert("projectSource".into(), Value::String(source.into()));
    }
    if !row.workspace_paths.is_empty()
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "workspacePaths".into(),
            Value::Array(
                row.workspace_paths
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    value
}

pub(super) fn add_workspace_paths(row: &mut DimensionRow, event: &RuntimeEvent) {
    let codex_paths = event
        .codex_metadata
        .as_ref()
        .into_iter()
        .flat_map(|metadata| metadata.workspaces.keys().map(String::as_str));
    // 客户端声明的 workspace 同样只以脱敏后缀暴露,与 Codex 行同口径。
    let declared_path = event
        .client_declared
        .as_ref()
        .and_then(|declared| declared.workspace.as_deref());
    for path in codex_paths.chain(declared_path) {
        let Some(locator) = workspace_locator(path) else {
            continue;
        };
        if row.workspace_paths.len() < MAX_PROJECT_WORKSPACE_PATHS
            || row.workspace_paths.contains(&locator)
        {
            row.workspace_paths.insert(locator);
        }
    }
}

/// Re-apply the workspace redaction boundary before exposing a locator from
/// analytics. This also protects older persisted events that may predate the
/// core parser's path sanitization.
pub(super) fn workspace_locator(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    let parts = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [] => None,
        [only] if *only == "workspace" => None,
        [only] => Some((*only).to_string()),
        [parent, child] => Some(format!("{parent}/{child}")),
        _ => Some(format!(
            ".../{}/{}",
            parts[parts.len() - 2],
            parts[parts.len() - 1]
        )),
    }
}

pub(super) fn merge_project_source(current: &mut Option<String>, next: &'static str) {
    match current.as_deref() {
        None => *current = Some(next.into()),
        Some(value) if value == next || value == "mixed" => {}
        Some(_) => *current = Some("mixed".into()),
    }
}

pub(super) fn success_rate(successes: i64, failures: i64, cancelled: i64) -> Option<f64> {
    let completed = successes.saturating_add(failures).saturating_add(cancelled);
    (completed > 0).then_some(successes as f64 * 100.0 / completed as f64)
}

pub(super) fn event_session_projection(event: &RuntimeEvent) -> (String, &'static str) {
    if let Some(value) = event
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let source = match event.session_source.as_deref() {
            Some("client_declared") => "client_declared",
            Some("claude_metadata") => "claude_metadata",
            Some("grok_session") => "grok_session",
            Some("codex_session") => "codex_session",
            Some("grok_conversation") => "grok_conversation",
            Some("header") => "header",
            _ => "event",
        };
        return (value.to_owned(), source);
    }
    if let Some(metadata) = event.codex_metadata.as_ref() {
        if let Some(value) = metadata
            .session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return (value.to_owned(), "codex_session");
        }
        if let Some(value) = metadata
            .thread_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            return (value.to_owned(), "codex_thread");
        }
    }
    ("unidentified_session".into(), "unidentified")
}

pub(super) fn event_project_projection(
    event: &RuntimeEvent,
) -> (String, String, &'static str, String) {
    let identity = project_identity(event);
    let project_id = if identity == "unidentified_project" {
        identity.clone()
    } else {
        format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
    };
    let project_name = project_base(&identity);
    let source = project_source(event);
    let mut paths = BTreeSet::new();
    if let Some(metadata) = event.codex_metadata.as_ref() {
        for path in metadata.workspaces.keys() {
            if let Some(path) = workspace_locator(path) {
                paths.insert(path);
            }
        }
    }
    if let Some(path) = event
        .client_declared
        .as_ref()
        .and_then(|declared| declared.workspace.as_deref())
        .and_then(workspace_locator)
    {
        paths.insert(path);
    }
    let workspace_paths_json = serde_json::to_string(
        &paths
            .into_iter()
            .take(MAX_PROJECT_WORKSPACE_PATHS)
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".into());
    (project_id, project_name, source, workspace_paths_json)
}

pub(super) fn project_identity(event: &RuntimeEvent) -> String {
    // Resource requests (for example Codex's `/v1/models` discovery call) do
    // not belong to a project. Keep them out of `unidentified_project` so the
    // UI does not present an internal capability probe as user traffic.
    if is_internal_resource_event(event) {
        return "internal_feature".into();
    }
    let codex_workspaces = event
        .codex_metadata
        .as_ref()
        .filter(|metadata| !metadata.workspaces.is_empty());
    if let Some(metadata) = codex_workspaces {
        return metadata
            .workspaces
            .iter()
            .map(|(path, workspace)| workspace_identity(path, workspace))
            .collect::<Vec<_>>()
            .join("|");
    }
    // Wrapper 带了工作区路径时按路径归并（与 Codex 同一套末段规则），用户名不进 identity。
    event
        .client_declared
        .as_ref()
        .and_then(declared_identity)
        .unwrap_or_else(|| "unidentified_project".into())
}

pub(super) fn is_internal_resource_event(event: &RuntimeEvent) -> bool {
    event.kind == KIND_CLIENT && event.client_model.as_deref() == Some(RESOURCE_ROUTING_MODEL)
}

pub(super) fn event_attribution_scope(
    event: &RuntimeEvent,
) -> sumpter_core::events::CodexAttributionScope {
    if is_internal_resource_event(event) {
        return sumpter_core::events::CodexAttributionScope::InternalFeature;
    }
    codex_attribution_scope(
        event.codex_metadata.as_ref(),
        event.client_declared.as_ref(),
    )
}

/// 客户端声明的项目身份。工作区路径优先（升格为本地项目），否则 git remote，
/// 再否则裸项目名。用户名不进 identity。前缀与 [`workspace_identity`] 对齐。
pub(super) fn declared_identity(
    declared: &sumpter_core::events::ClientDeclaredMetadata,
) -> Option<String> {
    let nonempty = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    if let Some(workspace) = nonempty(&declared.workspace) {
        return Some(format!("path:{workspace}"));
    }
    if let Some(remote) = nonempty(&declared.git_remote) {
        return Some(format!(
            "remote:{}",
            remote.trim_end_matches('/').trim_end_matches(".git")
        ));
    }
    nonempty(&declared.project).map(|project| format!("declared:{project}"))
}

/// Explain why the project key is trustworthy.  Codex structured workspaces
/// still win.  Wrapper-supplied workspace paths (Claude Code / Grok Build)
/// are treated as local collection, not as a free-form project nickname.
pub(super) fn project_source(event: &RuntimeEvent) -> &'static str {
    if is_internal_resource_event(event) {
        return "internal_feature";
    }
    let declared_source = || declared_project_source(event.client_declared.as_ref());
    let Some(metadata) = event.codex_metadata.as_ref() else {
        return declared_source();
    };
    if metadata.workspaces.is_empty() {
        return declared_source();
    }
    if metadata.workspaces.len() > 1 {
        return "multiple_workspaces";
    }
    let (path, workspace) = metadata.workspaces.iter().next().expect("non-empty");
    if !path.trim().is_empty() {
        return "workspace_local";
    }
    if workspace
        .associated_remote_urls
        .values()
        .any(|remote| !remote.trim().is_empty())
    {
        return "workspace_remote_fallback";
    }
    "workspace_unidentified"
}

pub(super) fn declared_project_source(
    declared: Option<&sumpter_core::events::ClientDeclaredMetadata>,
) -> &'static str {
    let Some(declared) = declared else {
        return "missing_workspace_metadata";
    };
    let nonempty = |value: &Option<String>| {
        value
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some()
    };
    if nonempty(&declared.workspace) {
        "workspace_local"
    } else if nonempty(&declared.git_remote) {
        "workspace_remote_fallback"
    } else if nonempty(&declared.project) {
        "client_declared"
    } else {
        "missing_workspace_metadata"
    }
}

pub(super) fn event_local_user(event: &RuntimeEvent) -> Option<String> {
    if let Some(user) = event
        .client_declared
        .as_ref()
        .and_then(|declared| declared.user.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Some(user.to_string());
    }
    let metadata = event.codex_metadata.as_ref()?;
    metadata
        .source_workspace_paths
        .iter()
        .chain(metadata.workspaces.keys())
        .find_map(|path| local_user_from_workspace_path(path))
}

pub(super) fn workspace_identity(
    path: &str,
    workspace: &sumpter_core::events::CodexWorkspaceMetadata,
) -> String {
    let local_path = path.trim();
    if !local_path.is_empty() {
        return format!("path:{local_path}");
    }
    workspace
        .associated_remote_urls
        .values()
        .next()
        .map(|remote| {
            format!(
                "remote:{}",
                remote.trim().trim_end_matches('/').trim_end_matches(".git")
            )
        })
        .unwrap_or_else(|| "unidentified_project".into())
}

pub(super) fn project_base(identity: &str) -> String {
    if identity == "unidentified_project" {
        return identity.into();
    }
    if identity.contains('|') {
        return "multiple_workspaces".into();
    }
    identity
        .rsplit(['/', '\\', ':'])
        .find(|part| !part.is_empty())
        .unwrap_or("workspace")
        .to_string()
}

pub(super) fn project_short_identity(identity: &str) -> String {
    if identity.contains('|') {
        return identity
            .split('|')
            .map(project_short_identity)
            .collect::<Vec<_>>()
            .join(" + ");
    }
    let mut parts = identity
        .rsplit(['/', '\\', ':'])
        .filter(|part| !part.is_empty());
    let last = parts.next().unwrap_or("workspace");
    let parent = parts.next();
    parent
        .map(|value| format!("{value}/{last}"))
        .unwrap_or_else(|| last.to_string())
}

pub(super) fn build_project_keys(events: &[RuntimeEvent]) -> HashMap<String, String> {
    let mut identities = HashSet::new();
    for event in events.iter().filter(|event| event.kind == KIND_CLIENT) {
        identities.insert(project_identity(event));
    }
    let mut bases = HashMap::<String, usize>::new();
    for identity in &identities {
        *bases.entry(project_base(identity)).or_default() += 1;
    }
    identities
        .into_iter()
        .map(|identity| {
            let base = project_base(&identity);
            let key = if bases.get(&base).copied().unwrap_or(0) <= 1 {
                base
            } else {
                project_short_identity(&identity)
            };
            (identity, key)
        })
        .collect()
}

pub(super) fn project_key(event: &RuntimeEvent, project_keys: &HashMap<String, String>) -> String {
    let identity = project_identity(event);
    project_keys
        .get(&identity)
        .cloned()
        .unwrap_or_else(|| project_base(&identity))
}

pub(super) fn session_key(event: &RuntimeEvent) -> String {
    event
        .session_id
        .clone()
        .or_else(|| {
            event.codex_metadata.as_ref().and_then(|metadata| {
                metadata
                    .session_id
                    .clone()
                    .or_else(|| metadata.thread_id.clone())
            })
        })
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "unidentified_session".into())
}

pub(super) fn client_key(event: &RuntimeEvent) -> String {
    option_token(event.client_kind).unwrap_or_else(|| "unrecorded_client".into())
}
