//! Runtime snapshots, queries, exports and serialized storage mutations.
//! SQL and worker ownership remain in `sumpter-runtime`.

use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use sumpter_core::config_store::ConfigDir;

use crate::runtime_query::{
    self, DimensionKind, DimensionPageQuery, ErrorPageQuery, EventPageQuery, ExportManifest,
    ExportQuery, RuntimeFilter, RuntimeQueryError, TrendQuery,
};
use crate::runtime_store::{
    AnalyticsFilter, RuntimeCleanupMutation, RuntimeCleanupPreview, RuntimeCounters,
    RuntimeEventListItem, RuntimePricingUpdate, RuntimeRetentionUpdate, RuntimeStore,
};

use super::Engine;
use super::events::{EngineNotice, RuntimeSnapshot, runtime_outcome_token};

impl Engine {
    pub fn runtime_snapshot(&self) -> RuntimeSnapshot {
        self.inner.state.lock().unwrap().runtime.clone()
    }

    pub fn runtime_summary_value(&self) -> Value {
        let (counters, latest_event, recent_event_count) = {
            let state = self.inner.state.lock().unwrap();
            (
                RuntimeCounters::from_snapshot(&state.runtime),
                state.runtime.recent_events.first().cloned(),
                state.runtime.recent_events.len(),
            )
        };
        if let Some(store) = self.inner.runtime_store.get() {
            let mut summary = store.summary();
            // Memory is the real-time source of truth; storage fields describe
            // durability and may lag while a batch is pending.
            summary.counters = counters;
            summary.latest_event = latest_event;
            return serde_json::to_value(summary).unwrap_or(Value::Null);
        }
        json!({
            "apiVersion": 1,
            "storage": {
                "backend": "sqlite",
                "state": self.runtime_database_issue().map_or("degraded", |issue| issue.code),
                "pendingEvents": 0,
                "eventCount": recent_event_count,
                "dbBytes": 0,
                "walBytes": 0,
                "lastCommitAt": Value::Null,
                "lastError": self.last_error(),
            },
            "resetGeneration": 0,
            "startupIssue": self.runtime_database_issue(),
            "counters": counters,
            "latestEvent": latest_event,
        })
    }

    // 事件查询的过滤维度直接透传给 runtime,合并成 struct 只会多一层转换。
    #[allow(clippy::too_many_arguments)]
    pub fn runtime_events(
        &self,
        before_seq: Option<i64>,
        after_change_seq: Option<i64>,
        limit: usize,
        kind: Option<&str>,
        request_id: Option<&str>,
        outcome: Option<&str>,
        from: Option<f64>,
        to: Option<f64>,
    ) -> Result<Value, String> {
        if let Some(store) = self.inner.runtime_store.get() {
            let cursor_valid = store.change_cursor_valid(after_change_seq)?;
            let mut events = store.events(
                before_seq,
                after_change_seq,
                limit,
                kind,
                request_id,
                outcome,
                from,
                to,
            )?;
            let page_limit = limit.clamp(1, 200);
            let has_more = events.len() > page_limit;
            events.truncate(page_limit);
            return Ok(json!({
                "events": events,
                "hasMore": has_more,
                "resetGeneration": store.summary().reset_generation,
                "cursorValid": cursor_valid,
            }));
        }
        let snapshot = self.runtime_snapshot();
        let events = snapshot
            .recent_events
            .into_iter()
            .enumerate()
            .filter(|(_, event)| kind.is_none_or(|value| value == event.kind))
            .filter(|(_, event)| {
                request_id.is_none_or(|value| event.request_id.as_deref() == Some(value))
            })
            .filter(|(_, event)| {
                outcome.is_none_or(|value| runtime_outcome_token(event) == Some(value))
            })
            .filter(|(_, event)| from.is_none_or(|value| event.timestamp >= value))
            .filter(|(_, event)| to.is_none_or(|value| event.timestamp <= value))
            .map(|(index, event)| {
                RuntimeEventListItem::from_change(index as i64 + 1, index as i64 + 1, event)
            })
            .collect::<Vec<_>>();
        Ok(
            json!({"events": events.into_iter().take(limit.clamp(1, 200)).collect::<Vec<_>>(), "hasMore": false, "resetGeneration": 0, "cursorValid": false}),
        )
    }

    fn runtime_query_path(&self) -> Result<&std::path::Path, RuntimeQueryError> {
        self.inner
            .runtime_store
            .get()
            .map(RuntimeStore::database_path)
            .ok_or_else(|| RuntimeQueryError::NotFound("runtime.sqlite3 不可用".into()))
    }

    fn runtime_query_value<T: serde::Serialize>(value: T) -> Result<Value, RuntimeQueryError> {
        serde_json::to_value(value)
            .map_err(|error| RuntimeQueryError::InvalidInput(error.to_string()))
    }

    pub fn runtime_events_page(&self, query: &EventPageQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::events_page(path, query)?)
    }

    pub fn runtime_request_chain(&self, request_id: &str) -> Result<Value, RuntimeQueryError> {
        let store = self
            .inner
            .runtime_store
            .get()
            .ok_or_else(|| RuntimeQueryError::NotFound("runtime.sqlite3 不可用".into()))?;
        let recent = store.recent_changes_for_request(request_id);
        Self::runtime_query_value(runtime_query::request_chain_with_recent(
            store.database_path(),
            request_id,
            &recent,
        )?)
    }

    pub fn runtime_trends(&self, query: &TrendQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::trends(path, query)?)
    }

    pub fn runtime_facets(&self, filter: &RuntimeFilter) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::facets(path, filter)?)
    }

    pub fn runtime_error_groups(&self, query: &ErrorPageQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::error_groups(path, query)?)
    }

    pub fn runtime_dimension_page(
        &self,
        kind: DimensionKind,
        query: &DimensionPageQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::dimension_page(path, kind, query)?)
    }

    pub fn runtime_storage_details(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::storage_details(path)?)
    }

    pub fn runtime_pricing(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::pricing(path)?)
    }

    pub fn runtime_export_estimate(&self, query: &ExportQuery) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?;
        Self::runtime_query_value(runtime_query::export_estimate(path, query)?)
    }

    pub fn runtime_stream_export<F>(
        &self,
        query: &ExportQuery,
        sink: F,
    ) -> Result<ExportManifest, RuntimeQueryError>
    where
        F: FnMut(Vec<u8>) -> Result<(), String>,
    {
        let path = self.runtime_query_path()?;
        runtime_query::stream_export(path, query, sink)
    }

    pub fn runtime_set_retention(&self, update: RuntimeRetentionUpdate) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法更新保留策略".into())
        })?;
        let mutation = store.set_retention(update)?;
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_cleanup_preview(&self, older_than: f64) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法预览清理范围".into())
        })?;
        let preview: RuntimeCleanupPreview = store.cleanup_before_preview(older_than)?;
        serde_json::to_value(preview).map_err(|error| error.to_string())
    }

    pub fn runtime_cleanup(&self, older_than: f64) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清理统计".into())
        })?;
        let mutation: RuntimeCleanupMutation = store.cleanup_before(older_than)?;
        let runtime = store.snapshot()?;
        self.inner.state.lock().unwrap().runtime = runtime;
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_replace_pricing(&self, update: RuntimePricingUpdate) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法更新价格表".into())
        })?;
        let mutation = store.replace_pricing(update)?;
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn runtime_event(&self, id: &str) -> Result<Option<Value>, String> {
        if let Some(store) = self.inner.runtime_store.get() {
            return store.event(id).map(|change| {
                change.map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
            });
        }
        Ok(self
            .runtime_snapshot()
            .recent_events
            .into_iter()
            .find(|event| event.id == id)
            .map(|event| json!({"seq": 0, "changeSeq": 0, "event": event})))
    }

    pub fn delete_runtime_session(&self, session_id: &str) -> Result<Value, String> {
        self.delete_runtime_session_confirmed(session_id, false)
    }

    pub fn delete_runtime_session_confirmed(
        &self,
        session_id: &str,
        confirm_unidentified: bool,
    ) -> Result<Value, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法删除会话".into())
        })?;
        let mutation = store.delete_session_confirmed(session_id, confirm_unidentified)?;
        let runtime = store.snapshot()?;
        let mut state = self.inner.state.lock().unwrap();
        state.runtime = runtime;
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        serde_json::to_value(mutation).map_err(|error| error.to_string())
    }

    pub fn export_runtime_session(&self, session_id: &str) -> Result<Value, String> {
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法导出会话".into())
        })?;
        store.export_session(session_id)
    }

    /// 清除某项目的会话粘性归属:先从 runtime 事件聚合该项目出现过的
    /// affinity 键,再交给 `clear_session_sticky` 清内存并落盘
    /// session_affinity.json。返回清除的归属条数。
    pub fn clear_project_sticky(&self, project_id: &str) -> Result<Value, String> {
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清除会话粘性归属".into())
        })?;
        let keys = store.sticky_keys_for_project(project_id)?;
        let removed = self.clear_session_sticky(&keys)?;
        Ok(json!({"cleared": removed, "matched": keys.len()}))
    }

    /// 解除选中对话的全部模型绑定，保留运行事件与统计。
    pub fn clear_runtime_session_sticky(&self, session_id: &str) -> Result<Value, String> {
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清除会话粘性归属".into())
        })?;
        let keys = store.sticky_keys_for_session(session_id)?;
        let removed = self.clear_session_sticky(&keys)?;
        Ok(json!({"cleared": removed, "matched": keys.len()}))
    }

    pub fn runtime_analytics(&self, range: &str) -> Result<Value, String> {
        self.runtime_analytics_filtered(range, &AnalyticsFilter::default())
    }

    pub fn runtime_analytics_filtered(
        &self,
        range: &str,
        filter: &AnalyticsFilter,
    ) -> Result<Value, String> {
        let filter = filter.normalized();
        if let Some(store) = self.inner.runtime_store.get() {
            let query_filter = RuntimeFilter {
                client_kind: filter.client_kind.clone(),
                endpoint_id: filter.endpoint_id.clone(),
                project_id: filter.project_id.clone(),
                project_name: filter.project.clone(),
                session_id: filter.session_id.clone(),
                from: filter.from,
                to: filter.to,
                ..RuntimeFilter::default()
            };
            return runtime_query::analytics(store.database_path(), range, &query_filter)
                .map_err(|error| error.to_string())
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()));
        }
        let counters = RuntimeCounters::from_snapshot(&self.runtime_snapshot());
        let filtered = filter.is_active();
        let count = |value: i64| if filtered { 0 } else { value };
        Ok(json!({
            "range": range,
            "from": Value::Null,
            "clientRequests": count(counters.client_requests),
            "clientSuccesses": count(counters.client_successes),
            "clientFailures": count(counters.client_failures),
            "clientCancelled": 0,
            "clientPending": 0,
            "clientSuccessRate": Value::Null,
            "upstreamAttempts": count(counters.upstream_attempts),
            "upstreamSuccesses": count(counters.upstream_successes),
            "upstreamFailures": count(counters.upstream_failures),
            "failovers": count(counters.failovers),
            "averageDurationMS": Value::Null,
            "averageTTFBMS": Value::Null,
            "latencyBuckets": {"under1s": 0, "from1sTo3s": 0, "from3sTo6s": 0, "over6s": 0},
            "tokenUsage": {
                "inputTokens": 0, "outputTokens": 0, "cacheReadInputTokens": 0,
                "cacheCreationInputTokens": 0, "reasoningTokens": 0,
                "uncachedInputTokens": 0, "processedInputTokens": 0,
                "processedTotalTokens": 0, "totalTokens": 0, "observedRequests": 0,
                "tokenAccountingSemantics": "unknown", "tokenAccountingQuality": "unknown",
                "usageFieldPresence": {
                    "inputTokens": 0, "outputTokens": 0,
                    "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0,
                    "reasoningTokens": 0
                }
            },
            "endpoints": [], "models": [], "clientKinds": [], "requestPurposes": [],
            "featureRules": [], "protocolRoutes": [], "failureKinds": [],
            "failurePhases": [], "upstreamStatuses": [], "streamTerminals": [],
            "projects": [], "sessions": [], "toolCalls": [], "codexMetadataPresent": 0,
            "facets": {"clientKinds": [], "projects": [], "sessions": []},
            "skippedEvents": 0,
            "truncated": false,
            "filtersApplied": !filtered,
            "filterWarning": if filtered {
                Value::String("SQLite analytics unavailable; filters were not applied".into())
            } else {
                Value::Null
            },
            "appliedFilters": {
                "clientKind": filter.client_kind,
                "endpointID": filter.endpoint_id,
                "projectID": filter.project_id,
                "project": filter.project,
                "sessionID": filter.session_id,
            }
        }))
    }

    pub fn last_error(&self) -> Option<String> {
        self.inner.state.lock().unwrap().last_error.clone()
    }

    pub fn set_last_error(&self, message: Option<String>) {
        self.inner.state.lock().unwrap().last_error = message;
    }

    pub fn runtime_database_issue(&self) -> Option<crate::runtime_store::RuntimeDatabaseIssue> {
        self.inner.runtime_database_issue.lock().unwrap().clone()
    }

    pub fn ensure_runtime_ready(&self) -> Result<(), String> {
        self.runtime_database_issue()
            .map_or(Ok(()), |issue| Err(issue.message))
    }

    pub fn stats_writable(&self) -> bool {
        self.inner.runtime_store.get().map_or_else(
            || self.inner.stats_writable.load(Ordering::Acquire),
            |store| store.summary().storage.state == "ready",
        )
    }

    pub fn runtime_storage_backpressured(&self) -> bool {
        self.inner
            .runtime_store
            .get()
            .is_some_and(RuntimeStore::is_backpressured)
    }

    pub fn config_dir(&self) -> Option<ConfigDir> {
        self.inner.dir.clone()
    }

    pub fn reset_runtime(&self) -> Result<i64, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法清空统计".into())
        })?;
        let mut state = self.inner.state.lock().unwrap();
        let previous_runtime = state.runtime.clone();
        let previous_warning = state.stats_durability_warning.clone();
        let previous_error = state.last_error.clone();
        state.runtime = RuntimeSnapshot::default();
        state.stats_durability_warning = None;
        state.last_error = None;
        let generation = match store.reset() {
            Ok(generation) => generation,
            Err(error) => {
                state.runtime = previous_runtime;
                state.stats_durability_warning = previous_warning;
                state.last_error = previous_error;
                return Err(error);
            }
        };
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        Ok(generation)
    }

    pub fn recreate_runtime(&self) -> Result<i64, String> {
        let _runtime_write = self.inner.runtime_write.lock().unwrap();
        if self.inner.runtime_store.get().is_none() {
            let dir = self.inner.dir.as_ref().ok_or("runtime.sqlite3 不可用")?;
            let (store, snapshot) =
                RuntimeStore::recreate_legacy(&dir.root.join("runtime.sqlite3"))?;
            let generation = store.summary().reset_generation;
            self.inner
                .runtime_store
                .set(store)
                .map_err(|_| "runtime store already initialized")?;
            *self.inner.runtime_database_issue.lock().unwrap() = None;
            self.inner.stats_writable.store(true, Ordering::Release);
            let mut state = self.inner.state.lock().unwrap();
            state.runtime = snapshot;
            state.last_error = None;
            state.stats_durability_warning = None;
            drop(state);
            let _ = self.inner.notices.send(EngineNotice::StatsReset);
            return Ok(generation);
        }
        let store = self.inner.runtime_store.get().ok_or_else(|| {
            self.last_error()
                .unwrap_or_else(|| "runtime.sqlite3 不可用，无法重置数据库".into())
        })?;
        let mut state = self.inner.state.lock().unwrap();
        let previous_runtime = state.runtime.clone();
        let previous_warning = state.stats_durability_warning.clone();
        let previous_error = state.last_error.clone();
        state.runtime = RuntimeSnapshot::default();
        state.stats_durability_warning = None;
        state.last_error = None;
        let generation = match store.recreate() {
            Ok(generation) => generation,
            Err(error) => {
                state.runtime = previous_runtime;
                state.stats_durability_warning = previous_warning;
                state.last_error = previous_error;
                return Err(error);
            }
        };
        drop(state);
        let _ = self.inner.notices.send(EngineNotice::StatsReset);
        Ok(generation)
    }

    pub fn flush_stats_if_dirty(&self) {
        if let Some(store) = self.inner.runtime_store.get()
            && let Err(error) = store.flush()
        {
            self.set_last_error(Some(error.clone()));
            tracing::warn!("runtime.sqlite3 flush failed: {error}");
        }
    }

    pub fn flush_stats(&self) -> Result<(), String> {
        self.inner
            .runtime_store
            .get()
            .map_or(Ok(()), RuntimeStore::flush)
    }
}
