//! Current runtime schema entity; internal to the shared storage layer.
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "runtime_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false, column_type = "Integer")]
    pub seq: i64,
    #[sea_orm(unique, column_type = "Integer")]
    pub change_seq: i64,
    #[sea_orm(unique)]
    pub event_id: String,
    pub request_id: Option<String>,
    #[sea_orm(column_type = "Double")]
    pub timestamp: f64,
    pub kind: String,
    pub phase: Option<String>,
    pub outcome: Option<String>,
    #[sea_orm(column_type = "Integer")]
    pub status_code: i64,
    pub client_kind: Option<String>,
    pub request_purpose: Option<String>,
    pub endpoint_id: Option<String>,
    pub failure_kind: Option<String>,
    #[sea_orm(column_type = "Integer")]
    pub is_in_flight: i64,
    pub payload_json: String,
    #[sea_orm(column_type = "Double")]
    pub created_at: f64,
    #[sea_orm(column_type = "Double")]
    pub updated_at: f64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub projection_version: i64,
    #[sea_orm(column_type = "Integer", default_value = 0)]
    pub payload_bytes: i64,
    pub session_key: Option<String>,
    pub session_source: Option<String>,
    pub sticky_key: Option<String>,
    pub project_id: Option<String>,
    pub project_name: Option<String>,
    pub project_source: Option<String>,
    pub local_user: Option<String>,
    pub workspace_paths_json: Option<String>,
    pub endpoint_name: Option<String>,
    pub model_group_id: Option<String>,
    pub model_group_name: Option<String>,
    pub feature_rule_id: Option<String>,
    pub client_model: Option<String>,
    pub effective_model: Option<String>,
    pub upstream_model: Option<String>,
    pub failure_phase: Option<String>,
    pub source_format: Option<String>,
    pub target_format: Option<String>,
    pub route_mode: Option<String>,
    #[sea_orm(column_type = "Integer")]
    pub upstream_status_code: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub duration_ms: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub ttfb_ms: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub failover: Option<i64>,
    pub stream_terminal: Option<String>,
    #[sea_orm(column_type = "Integer")]
    pub codex_metadata_present: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub usage_present: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub input_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub output_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub cache_read_input_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub cache_creation_input_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub reasoning_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub uncached_input_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub processed_input_tokens: Option<i64>,
    #[sea_orm(column_type = "Integer")]
    pub processed_total_tokens: Option<i64>,
    pub token_accounting_semantics: Option<String>,
    pub token_accounting_quality: Option<String>,
    pub tool_calls_json: Option<String>,
    pub codex_thread_class: Option<String>,
    pub attribution_scope: Option<String>,
    pub request_method: Option<String>,
    pub request_path: Option<String>,
    pub route_intent: Option<String>,
    pub client_variant: Option<String>,
    pub agent_role: Option<String>,
    pub agent_name: Option<String>,
    pub parent_thread_id: Option<String>,
    pub parent_turn_id: Option<String>,
    pub root_turn_id: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
