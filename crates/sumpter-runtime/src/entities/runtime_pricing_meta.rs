//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_pricing_meta")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub id: i64,
    #[sea_orm(column_type = "Integer", default_value = 1)]
    pub revision: i64,
    #[sea_orm(default_value = "USD")]
    pub currency: String,
    #[sea_orm(column_type = "Double")]
    pub updated_at: f64,
    pub official_source: Option<String>,
    #[sea_orm(column_type = "Double")]
    pub official_updated_at: Option<f64>,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub official_model_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub official_tiered_model_count: i64,
    pub official_sync_error: Option<String>,
    pub official_digest: Option<String>,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub official_snapshot_version: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
