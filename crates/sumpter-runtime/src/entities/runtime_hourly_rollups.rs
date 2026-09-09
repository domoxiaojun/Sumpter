//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_hourly_rollups")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub bucket_start: i64,
    #[sea_orm(column_type = "Integer")]
    pub bucket_end: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub max_seq: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub client_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub client_successes: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub client_failures: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub client_cancelled: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub failovers: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub duration_ms_sum: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub duration_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub duration_slow_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub duration_critical_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub ttfb_ms_sum: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub ttfb_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub ttfb_slow_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub ttfb_critical_count: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub client_unknown_results: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub failover_terminal_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub failover_recovered_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub upstream_attempts: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub upstream_successes: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub upstream_failures: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub input_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub output_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_input_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_creation_input_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub reasoning_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub uncached_input_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub processed_input_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub processed_total_tokens: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub usage_present_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub accounting_known_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub accounting_unknown_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_reported_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_hit_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_eligible_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_unknown_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_token_numerator: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_token_denominator: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub input_tokens_present: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub output_tokens_present: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_read_input_tokens_present: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cache_creation_input_tokens_present: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub reasoning_tokens_present: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_accounting_complete_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_unknown_accounting_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_numerator: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_priced_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_unpriced_requests: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub cost_unknown_requests: i64,
    #[sea_orm(column_type = "Integer")]
    pub cost_price_revision: Option<i64>,
    #[sea_orm(column_type = "Double", default_value = 0)]
    pub updated_at: f64,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
