//! Async Admin boundary backed by runtime-owned query workers.
use super::Engine;
use crate::runtime_query::{self, *};
use crate::runtime_store::{AnalyticsFilter, RuntimePricingUpdate, RuntimeRetentionUpdate};
use serde_json::Value;

impl Engine {
    pub async fn runtime_events_page_async(
        &self,
        query: &EventPageQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let query = query.clone();
        let value = runtime_query::asynchronous::events_page(path, query).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_trends_async(
        &self,
        query: &TrendQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let query = query.clone();
        let value = runtime_query::asynchronous::trends(path, query).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_facets_async(
        &self,
        filter: &RuntimeFilter,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let filter = filter.clone();
        let value = runtime_query::asynchronous::facets(path, filter).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_error_groups_async(
        &self,
        query: &ErrorPageQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let query = query.clone();
        let value = runtime_query::asynchronous::error_groups(path, query).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_dimension_page_async(
        &self,
        kind: DimensionKind,
        query: &DimensionPageQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let query = query.clone();
        let value = runtime_query::asynchronous::dimension_page(path, kind, query).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_storage_details_async(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();

        let value = runtime_query::asynchronous::storage_details(path).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_pricing_async(&self) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();

        let value = runtime_query::asynchronous::pricing(path).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_export_estimate_async(
        &self,
        query: &ExportQuery,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let query = query.clone();
        let value = runtime_query::asynchronous::export_estimate(path, query).await?;
        Self::runtime_query_value(value)
    }
    pub async fn runtime_request_chain_async(
        &self,
        request_id: &str,
    ) -> Result<Value, RuntimeQueryError> {
        let path = self.runtime_query_path()?.to_path_buf();
        let recent = self
            .inner
            .runtime_store
            .get()
            .map(|store| store.recent_changes_for_request(request_id))
            .unwrap_or_default();
        Self::runtime_query_value(
            runtime_query::asynchronous::request_chain_with_recent(
                path,
                request_id.to_owned(),
                recent,
            )
            .await?,
        )
    }
    pub async fn runtime_stream_export_async<F>(
        &self,
        query: &ExportQuery,
        sink: F,
    ) -> Result<ExportManifest, RuntimeQueryError>
    where
        F: FnMut(Vec<u8>) -> Result<(), String> + Send + 'static,
    {
        runtime_query::asynchronous::stream_export(
            self.runtime_query_path()?.to_path_buf(),
            query.clone(),
            sink,
        )
        .await
    }
    pub async fn runtime_event_async(&self, id: &str) -> Result<Option<Value>, String> {
        if let Some(store) = self.inner.runtime_store.get() {
            return store.event_async(id.to_owned()).await.map(|change| {
                change.map(|value| serde_json::to_value(value).unwrap_or(Value::Null))
            });
        }
        self.runtime_event(id)
    }
    pub async fn runtime_analytics_filtered_async(
        &self,
        range: &str,
        filter: &AnalyticsFilter,
    ) -> Result<Value, String> {
        if let Some(store) = self.inner.runtime_store.get() {
            return runtime_query::asynchronous::analytics(
                store.database_path().to_path_buf(),
                range.to_owned(),
                filter.normalized().to_runtime_filter(),
            )
            .await
            .map_err(|error| error.to_string())
            .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string()));
        }
        self.runtime_analytics_filtered(range, filter)
    }
    pub async fn runtime_set_retention_async(
        &self,
        update: RuntimeRetentionUpdate,
    ) -> Result<Value, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run_mutation(move || engine.runtime_set_retention(update))
            .await?
    }
    pub async fn runtime_replace_pricing_async(
        &self,
        update: RuntimePricingUpdate,
    ) -> Result<Value, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run_mutation(move || {
            engine.runtime_replace_pricing(update)
        })
        .await?
    }
    pub async fn runtime_cleanup_preview_async(&self, older_than: f64) -> Result<Value, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run(move || engine.runtime_cleanup_preview(older_than))
            .await?
    }
    pub async fn runtime_cleanup_async(&self, older_than: f64) -> Result<Value, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run_mutation(move || engine.runtime_cleanup(older_than))
            .await?
    }
    pub async fn delete_runtime_session_confirmed_async(
        &self,
        id: &str,
        confirm: bool,
    ) -> Result<Value, String> {
        let engine = self.clone();
        let id = id.to_owned();
        sumpter_runtime::query_executor::run_mutation(move || {
            engine.delete_runtime_session_confirmed(&id, confirm)
        })
        .await?
    }
    pub async fn export_runtime_session_async(&self, id: &str) -> Result<Value, String> {
        let engine = self.clone();
        let id = id.to_owned();
        sumpter_runtime::query_executor::run(move || engine.export_runtime_session(&id)).await?
    }
    pub async fn clear_project_sticky_async(&self, id: &str) -> Result<Value, String> {
        let engine = self.clone();
        let id = id.to_owned();
        sumpter_runtime::query_executor::run_mutation(move || engine.clear_project_sticky(&id))
            .await?
    }
    pub async fn clear_runtime_session_sticky_async(&self, id: &str) -> Result<Value, String> {
        let engine = self.clone();
        let id = id.to_owned();
        sumpter_runtime::query_executor::run_mutation(move || {
            engine.clear_runtime_session_sticky(&id)
        })
        .await?
    }
    pub async fn reset_runtime_async(&self) -> Result<i64, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run_mutation(move || engine.reset_runtime()).await?
    }
    pub async fn recreate_runtime_async(&self) -> Result<i64, String> {
        let engine = self.clone();

        sumpter_runtime::query_executor::run_mutation(move || engine.recreate_runtime()).await?
    }
    #[allow(clippy::too_many_arguments)]
    pub async fn runtime_events_async(
        &self,
        before: Option<i64>,
        after: Option<i64>,
        limit: usize,
        kind: Option<&str>,
        request_id: Option<&str>,
        outcome: Option<&str>,
        from: Option<f64>,
        to: Option<f64>,
    ) -> Result<Value, String> {
        let engine = self.clone();
        let kind = kind.map(str::to_owned);
        let request_id = request_id.map(str::to_owned);
        let outcome = outcome.map(str::to_owned);
        sumpter_runtime::query_executor::run(move || {
            engine.runtime_events(
                before,
                after,
                limit,
                kind.as_deref(),
                request_id.as_deref(),
                outcome.as_deref(),
                from,
                to,
            )
        })
        .await?
    }
}
