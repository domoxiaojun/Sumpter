//! Named projection mapping: field edits are checked against the ORM entity.
use super::{EventProjection, PROJECTION_VERSION};
use crate::entities::runtime_events;
use sea_orm::Set;

pub(super) fn load_counters(
    connection: &crate::database::Connection,
) -> crate::database::Result<super::RuntimeCounters> {
    use crate::entities::runtime_counters;
    use sea_orm::EntityTrait;
    let model = connection
        .orm(|db| async move {
            runtime_counters::Entity::find_by_id(1_i64)
                .one(db.as_ref())
                .await
        })?
        .ok_or(crate::database::Error::QueryReturnedNoRows)?;
    Ok(super::RuntimeCounters {
        client_requests: model.client_requests,
        client_successes: model.client_successes,
        client_failures: model.client_failures,
        upstream_attempts: model.upstream_attempts,
        upstream_successes: model.upstream_successes,
        upstream_failures: model.upstream_failures,
        failovers: model.failovers,
    })
}

pub(super) fn save_counters(
    connection: &crate::database::Connection,
    counters: &super::RuntimeCounters,
) -> crate::database::Result<()> {
    use crate::entities::runtime_counters;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let model = runtime_counters::ActiveModel {
        client_requests: Set(counters.client_requests),
        client_successes: Set(counters.client_successes),
        client_failures: Set(counters.client_failures),
        upstream_attempts: Set(counters.upstream_attempts),
        upstream_successes: Set(counters.upstream_successes),
        upstream_failures: Set(counters.upstream_failures),
        failovers: Set(counters.failovers),
        ..Default::default()
    };
    connection.orm(move |db| async move {
        runtime_counters::Entity::update_many()
            .set(model)
            .filter(runtime_counters::Column::Id.eq(1_i64))
            .exec(db.as_ref())
            .await
            .map(|_| ())
    })
}

pub(super) fn retention(
    connection: &crate::database::Connection,
) -> crate::database::Result<crate::entities::runtime_retention::Model> {
    use sea_orm::EntityTrait;
    connection
        .orm(|db| async move {
            crate::entities::runtime_retention::Entity::find_by_id(1_i64)
                .one(db.as_ref())
                .await
        })?
        .ok_or(crate::database::Error::QueryReturnedNoRows)
}

pub(super) fn save_retention_revision(
    connection: &crate::database::Connection,
    revision: i64,
) -> crate::database::Result<()> {
    use crate::entities::runtime_retention;
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let model = runtime_retention::ActiveModel {
        revision: Set(revision),
        updated_at: Set(super::now()),
        ..Default::default()
    };
    connection.orm(move |db| async move {
        runtime_retention::Entity::update_many()
            .set(model)
            .filter(runtime_retention::Column::Id.eq(1_i64))
            .exec(db.as_ref())
            .await
            .map(|_| ())
    })
}

pub(super) fn pricing_revision(
    connection: &crate::database::Connection,
) -> crate::database::Result<i64> {
    use sea_orm::EntityTrait;
    let model = connection
        .orm(|db| async move {
            crate::entities::runtime_pricing_meta::Entity::find_by_id(1_i64)
                .one(db.as_ref())
                .await
        })?
        .ok_or(crate::database::Error::QueryReturnedNoRows)?;
    Ok(model.revision)
}

pub(super) fn save_pricing(
    connection: &crate::database::Connection,
    update: &super::RuntimePricingUpdate,
    revision: i64,
) -> crate::database::Result<()> {
    use crate::entities::{runtime_model_prices as prices, runtime_pricing_meta as meta};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let updated_at = super::now();
    let models = update
        .prices
        .iter()
        .map(|price| {
            let tiers_json = crate::pricing_matcher::serialize_tiers_json(&price.tiers)
                .map_err(crate::database::Error::InvalidParameterName)?;
            Ok(prices::ActiveModel {
                model_key: Set(price
                    .endpoint_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|endpoint| format!("{endpoint}\u{1f}{}", price.model_key.trim()))
                    .unwrap_or_else(|| price.model_key.trim().to_owned())),
                effective_from: Set(price.effective_from),
                effective_to: Set(price.effective_to),
                input_per_million_micros: Set(price.input_per_million_micros),
                output_per_million_micros: Set(price.output_per_million_micros),
                cache_read_per_million_micros: Set(price.cache_read_per_million_micros),
                cache_creation_per_million_micros: Set(price.cache_creation_per_million_micros),
                tiers_json: Set(tiers_json),
                created_at: Set(updated_at),
                updated_at: Set(updated_at),
                ..Default::default()
            })
        })
        .collect::<crate::database::Result<Vec<_>>>()?;
    let metadata = meta::ActiveModel {
        revision: Set(revision),
        currency: Set(update.currency.trim().to_owned()),
        updated_at: Set(updated_at),
        ..Default::default()
    };
    connection.orm(move |db| async move {
        prices::Entity::delete_many().exec(db.as_ref()).await?;
        // Keep bulk inserts below SQLite's parameter limit for large catalogs.
        for chunk in models.chunks(128) {
            prices::Entity::insert_many(chunk.iter().cloned())
                .exec_without_returning(db.as_ref())
                .await?;
        }
        meta::Entity::update_many()
            .set(metadata)
            .filter(meta::Column::Id.eq(1_i64))
            .exec(db.as_ref())
            .await?;
        Ok(())
    })
}

fn official_price_active_models(
    prices: &[crate::pricing_matcher::OfficialModelPriceInput],
    updated_at: f64,
) -> crate::database::Result<Vec<crate::entities::runtime_official_model_prices::ActiveModel>> {
    use crate::entities::runtime_official_model_prices as official_prices;
    prices
        .iter()
        .map(|price| {
            let tiers_json = crate::pricing_matcher::serialize_tiers_json(&price.tiers)
                .map_err(crate::database::Error::InvalidParameterName)?;
            Ok(official_prices::ActiveModel {
                model_key: Set(price.model_key.clone()),
                provider: Set(price.provider.clone()),
                input_per_million_micros: Set(price.input_per_million_micros),
                output_per_million_micros: Set(price.output_per_million_micros),
                cache_read_per_million_micros: Set(price.cache_read_per_million_micros),
                cache_creation_per_million_micros: Set(price.cache_creation_per_million_micros),
                tiers_json: Set(tiers_json),
                updated_at: Set(updated_at),
            })
        })
        .collect()
}

pub(super) fn save_official_pricing(
    connection: &crate::database::Connection,
    source: &str,
    prices: &[crate::pricing_matcher::OfficialModelPriceInput],
    digest: &str,
    revision: i64,
    updated_at: f64,
    snapshot_version: Option<i64>,
) -> crate::database::Result<()> {
    use crate::entities::{
        runtime_official_model_prices as official_prices, runtime_pricing_meta as meta,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let model_count = prices.len() as i64;
    let tiered_count = prices.iter().filter(|item| !item.tiers.is_empty()).count() as i64;
    let models = official_price_active_models(prices, updated_at)?;
    let mut metadata = meta::ActiveModel {
        revision: Set(revision),
        official_source: Set(Some(source.trim().to_owned())),
        official_updated_at: Set(Some(updated_at)),
        official_model_count: Set(model_count),
        official_tiered_model_count: Set(tiered_count),
        official_sync_error: Set(None),
        official_digest: Set(Some(digest.to_owned())),
        ..Default::default()
    };
    if let Some(version) = snapshot_version {
        metadata.official_snapshot_version = Set(version);
    }
    connection.orm(move |db| async move {
        official_prices::Entity::delete_many()
            .exec(db.as_ref())
            .await?;
        for chunk in models.chunks(96) {
            official_prices::Entity::insert_many(chunk.iter().cloned())
                .exec_without_returning(db.as_ref())
                .await?;
        }
        meta::Entity::update_many()
            .set(metadata)
            .filter(meta::Column::Id.eq(1_i64))
            .exec(db.as_ref())
            .await?;
        Ok(())
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn supplement_official_pricing_rows(
    connection: &crate::database::Connection,
    missing_prices: &[crate::pricing_matcher::OfficialModelPriceInput],
    digest: &str,
    model_count: i64,
    tiered_count: i64,
    revision: i64,
    updated_at: f64,
    snapshot_version: i64,
) -> crate::database::Result<()> {
    use crate::entities::{
        runtime_official_model_prices as official_prices, runtime_pricing_meta as meta,
    };
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let models = official_price_active_models(missing_prices, updated_at)?;
    let metadata = meta::ActiveModel {
        revision: Set(revision),
        official_model_count: Set(model_count),
        official_tiered_model_count: Set(tiered_count),
        official_digest: Set(Some(digest.to_owned())),
        official_snapshot_version: Set(snapshot_version),
        ..Default::default()
    };
    connection.orm(move |db| async move {
        for chunk in models.chunks(96) {
            official_prices::Entity::insert_many(chunk.iter().cloned())
                .exec_without_returning(db.as_ref())
                .await?;
        }
        meta::Entity::update_many()
            .set(metadata)
            .filter(meta::Column::Id.eq(1_i64))
            .exec(db.as_ref())
            .await?;
        Ok(())
    })
}

impl EventProjection {
    pub(super) fn active_model(&self) -> runtime_events::ActiveModel {
        runtime_events::ActiveModel {
            projection_version: Set(PROJECTION_VERSION),
            client_kind: Set(self.client_kind.as_ref().map(|value| value.to_string())),
            payload_bytes: Set(self.payload_bytes),
            session_key: Set(Some(self.session_key.to_owned())),
            session_source: Set(Some(self.session_source.to_owned())),
            sticky_key: Set(self.sticky_key.as_ref().map(|value| value.to_string())),
            project_id: Set(Some(self.project_id.to_owned())),
            project_name: Set(Some(self.project_name.to_owned())),
            project_source: Set(Some(self.project_source.to_owned())),
            local_user: Set(self.local_user.as_ref().map(|value| value.to_string())),
            codex_thread_class: Set(self
                .codex_thread_class
                .as_ref()
                .map(|value| value.to_string())),
            attribution_scope: Set(self
                .attribution_scope
                .as_ref()
                .map(|value| value.to_string())),
            workspace_paths_json: Set(Some(self.workspace_paths_json.to_owned())),
            endpoint_name: Set(self.endpoint_name.as_ref().map(|value| value.to_string())),
            model_group_id: Set(self.model_group_id.as_ref().map(|value| value.to_string())),
            model_group_name: Set(self
                .model_group_name
                .as_ref()
                .map(|value| value.to_string())),
            feature_rule_id: Set(self.feature_rule_id.as_ref().map(|value| value.to_string())),
            client_model: Set(self.client_model.as_ref().map(|value| value.to_string())),
            effective_model: Set(self.effective_model.as_ref().map(|value| value.to_string())),
            upstream_model: Set(self.upstream_model.as_ref().map(|value| value.to_string())),
            failure_phase: Set(self.failure_phase.as_ref().map(|value| value.to_string())),
            source_format: Set(self.source_format.as_ref().map(|value| value.to_string())),
            target_format: Set(self.target_format.as_ref().map(|value| value.to_string())),
            route_mode: Set(self.route_mode.as_ref().map(|value| value.to_string())),
            upstream_status_code: Set(self.upstream_status_code),
            duration_ms: Set(Some(self.duration_ms)),
            ttfb_ms: Set(self.ttfb_ms),
            upstream_duration_ms: Set(self.upstream_duration_ms),
            upstream_ttfb_ms: Set(self.upstream_ttfb_ms),
            output_span_ms: Set(self.output_span_ms),
            failover: Set(Some(self.failover)),
            stream_terminal: Set(self.stream_terminal.as_ref().map(|value| value.to_string())),
            codex_metadata_present: Set(Some(self.codex_metadata_present)),
            usage_present: Set(Some(self.usage_present)),
            input_tokens: Set(self.input_tokens),
            output_tokens: Set(self.output_tokens),
            cache_read_input_tokens: Set(self.cache_read_input_tokens),
            cache_read_state: Set(Some(self.cache_read_state.clone())),
            cache_read_finality: Set(Some(self.cache_read_finality.clone())),
            cache_read_reason: Set(self.cache_read_reason.clone()),
            hook_event: Set(self.hook_event.clone()),
            cache_creation_input_tokens: Set(self.cache_creation_input_tokens),
            reasoning_tokens: Set(self.reasoning_tokens),
            uncached_input_tokens: Set(self.uncached_input_tokens),
            processed_input_tokens: Set(self.processed_input_tokens),
            processed_total_tokens: Set(self.processed_total_tokens),
            token_accounting_semantics: Set(Some(self.token_accounting_semantics.to_owned())),
            token_accounting_quality: Set(Some(self.token_accounting_quality.to_owned())),
            tool_calls_json: Set(self.tool_calls_json.as_ref().map(|value| value.to_string())),
            request_method: Set(self.request_method.as_ref().map(|value| value.to_string())),
            request_path: Set(self.request_path.as_ref().map(|value| value.to_string())),
            route_intent: Set(self.route_intent.as_ref().map(|value| value.to_string())),
            request_kind: Set(self.request_kind.clone()),
            client_attribution_json: Set(self.client_attribution_json.clone()),
            client_variant: Set(self.client_variant.as_ref().map(|value| value.to_string())),
            agent_role: Set(self.agent_role.as_ref().map(|value| value.to_string())),
            agent_name: Set(self.agent_name.as_ref().map(|value| value.to_string())),
            parent_thread_id: Set(self
                .parent_thread_id
                .as_ref()
                .map(|value| value.to_string())),
            parent_turn_id: Set(self.parent_turn_id.as_ref().map(|value| value.to_string())),
            root_turn_id: Set(self.root_turn_id.as_ref().map(|value| value.to_string())),
            ..Default::default()
        }
    }
}
