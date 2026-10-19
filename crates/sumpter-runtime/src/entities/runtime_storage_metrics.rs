//! Derived storage totals, maintained atomically by runtime_events triggers.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_storage_metrics")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub id: i64,
    #[sea_orm(column_type = "Integer")]
    pub event_count: i64,
    #[sea_orm(column_type = "Integer")]
    pub completed_events: i64,
    #[sea_orm(column_type = "Integer")]
    pub in_flight_events: i64,
    #[sea_orm(column_type = "Integer")]
    pub payload_bytes: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
