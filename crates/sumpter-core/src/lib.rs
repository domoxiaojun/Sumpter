//! Sumpter纯逻辑层:配置模型、路由、粘性调度、访问控制、协议桥接(纯函数部分)。
//! 对应 Swift 版 SumpterCore —— 无网络、无 UI、无平台依赖。

pub mod access;
pub mod bridge;
pub mod bridge_in;
pub mod capability;
pub mod config;
pub mod config_store;
pub mod events;
pub mod model_name;
pub mod routing;
pub mod scheduler;
pub mod stream_terminal;
pub mod warnings;

pub use capability::ModelCapability;
pub use config::{
    AppConfig, ContextMode, Endpoint, EndpointCatalog, EndpointProtocolMode, FeatureRule,
    FeatureRuleMatch, FeatureRuleTarget, ListenerConfig, ModelMapping, ProviderProtocol,
    RequestKind, RetryPolicy, ThinkingMode,
};
pub use model_name::ReasoningEffort;
pub use routing::{
    PlannedEndpoint, RequestPurpose, RouteMode, RoutePlan, RoutePlanError, RoutePlanner,
    RoutingRequest,
};
