//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_counters")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub id: i64,
    #[sea_orm(column_type = "Integer")]
    pub client_requests: i64,
    #[sea_orm(column_type = "Integer")]
    pub client_successes: i64,
    #[sea_orm(column_type = "Integer")]
    pub client_failures: i64,
    #[sea_orm(column_type = "Integer")]
    pub upstream_attempts: i64,
    #[sea_orm(column_type = "Integer")]
    pub upstream_successes: i64,
    #[sea_orm(column_type = "Integer")]
    pub upstream_failures: i64,
    #[sea_orm(column_type = "Integer")]
    pub failovers: i64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
