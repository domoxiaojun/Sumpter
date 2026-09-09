//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_hourly_rollup_dirty")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub bucket_start: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
