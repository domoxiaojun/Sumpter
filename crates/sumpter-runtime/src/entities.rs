//! SeaORM models for the current schema. Entity types never cross the Admin API.

pub(crate) mod runtime_counters;
pub(crate) mod runtime_events;
pub(crate) mod runtime_hourly_rollup_dirty;
pub(crate) mod runtime_hourly_rollups;
pub(crate) mod runtime_meta;
pub(crate) mod runtime_model_prices;
pub(crate) mod runtime_pricing_meta;
pub(crate) mod runtime_retention;

/// Create the current model directly. Legacy databases are rejected before
/// this function; entity changes never silently alter an existing table.
pub(crate) fn create_schema(
    connection: &mut crate::database::Connection,
) -> crate::database::Result<()> {
    use sea_orm::{
        ConnectionTrait, DbBackend, Schema,
        sea_query::{Expr, Index},
    };
    let transaction = connection.transaction()?;
    transaction.orm(|db| async move {
        let backend = DbBackend::Sqlite;
        let schema = Schema::new(backend);
        let mut tables = vec![
            schema.create_table_from_entity(runtime_meta::Entity),
            schema.create_table_from_entity(runtime_events::Entity),
            schema.create_table_from_entity(runtime_hourly_rollups::Entity),
            schema.create_table_from_entity(runtime_hourly_rollup_dirty::Entity),
        ];
        let mut counters = schema.create_table_from_entity(runtime_counters::Entity);
        counters.check(Expr::col(runtime_counters::Column::Id).eq(1));
        tables.push(counters);
        let mut retention = schema.create_table_from_entity(runtime_retention::Entity);
        retention.check(Expr::col(runtime_retention::Column::Id).eq(1));
        tables.push(retention);
        let mut pricing = schema.create_table_from_entity(runtime_pricing_meta::Entity);
        pricing.check(Expr::col(runtime_pricing_meta::Column::Id).eq(1));
        tables.push(pricing);
        use runtime_model_prices::Column as Price;
        let mut prices = schema.create_table_from_entity(runtime_model_prices::Entity);
        prices.index(Index::create().unique().col(Price::ModelKey).col(Price::EffectiveFrom));
        prices.check(Expr::col(Price::EffectiveTo).is_null().or(Expr::col(Price::EffectiveTo).gt(Expr::col(Price::EffectiveFrom))));
        for column in [Price::InputPerMillionMicros, Price::OutputPerMillionMicros, Price::CacheReadPerMillionMicros, Price::CacheCreationPerMillionMicros] {
            prices.check(Expr::col(column).is_null().or(Expr::col(column).gte(0)));
        }
        tables.push(prices);
        for mut table in tables {
            db.execute(backend.build(table.if_not_exists())).await?;
        }
        // Query-specific indexes remain explicit, alongside their query shape.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS runtime_events_kind_seq ON runtime_events(kind, seq DESC);
             CREATE INDEX IF NOT EXISTS runtime_events_request_id ON runtime_events(request_id);
             CREATE INDEX IF NOT EXISTS runtime_events_timestamp ON runtime_events(timestamp);
             CREATE INDEX IF NOT EXISTS runtime_model_prices_lookup_v2 ON runtime_model_prices(model_key,effective_from DESC);"
        ).await?;
        Ok(())
    })?;
    transaction.commit()
}
