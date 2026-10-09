use super::*;

impl RuntimeStore {
    pub async fn snapshot_async(&self) -> Result<RuntimeSnapshot, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.snapshot()).await?
    }
    pub async fn event_async(&self, id: String) -> Result<Option<RuntimeChange>, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.event(&id)).await?
    }
    pub async fn change_cursor_valid_async(&self, cursor: Option<i64>) -> Result<bool, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.change_cursor_valid(cursor)).await?
    }
    pub async fn analytics_filtered_async(
        &self,
        range: String,
        filter: AnalyticsFilter,
    ) -> Result<Value, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.analytics_filtered(&range, &filter)).await?
    }
    pub async fn export_session_async(&self, session_id: String) -> Result<Value, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.export_session(&session_id)).await?
    }
    pub async fn set_retention_async(
        &self,
        update: RuntimeRetentionUpdate,
    ) -> Result<RuntimeRetentionMutation, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || store.set_retention(update)).await?
    }
    pub async fn replace_pricing_async(
        &self,
        update: RuntimePricingUpdate,
    ) -> Result<RuntimePricingMutation, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || store.replace_pricing(update)).await?
    }
    pub async fn cleanup_before_preview_async(
        &self,
        older_than: f64,
    ) -> Result<RuntimeCleanupPreview, String> {
        let store = self.clone();
        crate::query_executor::run(move || store.cleanup_before_preview(older_than)).await?
    }
    pub async fn cleanup_before_async(
        &self,
        older_than: f64,
    ) -> Result<RuntimeCleanupMutation, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || store.cleanup_before(older_than)).await?
    }
    pub async fn reset_async(&self) -> Result<i64, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || store.reset()).await?
    }
    pub async fn recreate_async(&self) -> Result<i64, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || store.recreate()).await?
    }
    pub async fn delete_session_confirmed_async(
        &self,
        session_id: String,
        confirm_unidentified: bool,
    ) -> Result<SessionMutation, String> {
        let store = self.clone();
        crate::query_executor::run_mutation(move || {
            store.delete_session_confirmed(&session_id, confirm_unidentified)
        })
        .await?
    }
}
