//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_model_prices")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub id: i64,
    pub model_key: String,
    #[sea_orm(column_type = "Double")]
    pub effective_from: f64,
    #[sea_orm(column_type = "Double")]
    pub effective_to: Option<f64>,
    #[sea_orm(column_type = "Integer")]
    pub input_per_million_micros: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub output_per_million_micros: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub cache_read_per_million_micros: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub cache_creation_per_million_micros: Option<i64>,
    #[sea_orm(column_type = "Double")]
    pub created_at: f64,
    #[sea_orm(column_type = "Double")]
    pub updated_at: f64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
