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
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
