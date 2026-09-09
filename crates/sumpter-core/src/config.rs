//! 配置模型(schema v7)。
//!
//! Linux daemon 从 XDG 配置目录读写 `config.json`；Admin PUT 是唯一远程写入口，
//! reload/SIGHUP 只读磁盘。模型继续与 macOS Rust 版共用 v4 wire 形状，因此这里的原则:
//! - serde 对未知字段宽容；配置存储层额外要求 `schemaVersion` 显式等于 7。
//! - 写对齐:键名/键序(字母序)/可选字段省略策略与既有 `pretty + sortedKeys` 输出一致,
//!   数值整值不带小数点(见 `trim_f64`)。golden 测试见 `tests/golden.rs`。
//! - Swift 在 `init(from:)` 里做的清洗(内建规则归一、upstreamModel clean、clamp、name 回退)
//!   在这里拆成显式的 [`AppConfig::normalized`] —— `from_json` 保真,引擎加载时再归一,
//!   golden round-trip 因此不受归一化重排干扰。
//!
//! 与磁盘形状相关的铁律(来自真实 config.json 与 Swift 源码 encode 实现):
//! - `retry.responseTimeoutSeconds` / `streamIdleTimeoutSeconds` 为 None 时**显式写 null**;
//! - `Endpoint.stickyGroup` / `catalog`(空目录不落盘)、
//!   `ModelMapping.failoverTimeoutSeconds`、FeatureRule 的可选匹配/目标字段为 None 时**省略键**;
//! - 分流目标的协议覆盖,JSON 键名是 `protocol`;`target.model` 恒写(含空串);
//! - `ModelMapping` 的 UI 选中 id 不落盘,Rust 侧不存在。

use serde::{Deserialize, Serialize, Serializer};

use crate::capability::ModelCapability;
use crate::model_name;

pub const SCHEMA_VERSION: u32 = 7;

fn is_none<T>(v: &Option<T>) -> bool {
    v.is_none()
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

/// 整值 f64 序列化为 JSON 整数(0 而非 0.0),对齐 Swift 输出。
fn trim_f64<S: Serializer>(v: &f64, s: S) -> Result<S::Ok, S::Error> {
    if v.is_finite() && v.fract() == 0.0 && v.abs() < 9.0e15 {
        s.serialize_i64(*v as i64)
    } else {
        s.serialize_f64(*v)
    }
}

fn trim_opt_f64<S: Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => trim_f64(x, s),
        None => s.serialize_none(),
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// 顶层
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub endpoints: Vec<Endpoint>,
    #[serde(
        rename = "modelGroups",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub model_groups: Option<Vec<crate::model_groups::ModelGroup>>,
    #[serde(rename = "featureRules", default)]
    pub feature_rules: Vec<FeatureRule>,
    #[serde(default)]
    pub listener: ListenerConfig,
    #[serde(default)]
    pub retry: RetryPolicy,
    /// 会话粘性归属的存活时长(小时)。默认 72;`<= 0` 表示永不过期(仍受
    /// 条目数上限约束)。控制面语义见 `EngineState::session_sticky_ttl_secs`。
    #[serde(
        rename = "sessionStickyTtlHours",
        default = "default_session_sticky_ttl_hours",
        serialize_with = "trim_f64"
    )]
    pub session_sticky_ttl_hours: f64,
    #[serde(rename = "schemaVersion", default = "default_schema_version")]
    pub schema_version: u32,
}

/// 会话粘性时长的默认值(小时)。旧版本固定 30 天,过长的归属让入口调整
/// 几乎无法在存量会话上生效,收敛到 3 天并允许用户按需调整。
pub const DEFAULT_SESSION_STICKY_TTL_HOURS: f64 = 72.0;

fn default_session_sticky_ttl_hours() -> f64 {
    DEFAULT_SESSION_STICKY_TTL_HOURS
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

impl AppConfig {
    /// 磁盘保真解码,不做任何归一化(golden round-trip 用)。
    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    /// pretty JSON;键序由字段声明序保证(全部按字母序声明)。
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// 引擎加载配置的入口:对齐 Swift `init(from:)` / 各类型 decode 时的清洗行为。
    ///
    /// - retry 三参数 clamp(轮数/时长 ≥0,并发 ≥1);
    /// - endpoint/rule 的 `name` 为空时回退为各自 id
    ///   (Swift 语义是「键缺失回退」;此处对显式空串也回退,视为等价修正);
    /// - stickyGroup / target.endpointID trim,空串归 None;
    /// - mapping.clientPattern trim;
    /// - mapping.upstreamModel / target.model 过 `ModelName.clean`;
    /// - 空 catalog 归 None(Swift encode 也不落盘空目录);
    /// - 内建分流规则归一:name/match 纠回 canonical,只保留用户的 enabled/target,
    ///   顺序恒为「内建三条(websearch/webfetch/classifier)在前 + 自定义原序在后」。
    pub fn normalized(mut self) -> Self {
        self.retry.max_deferred_rounds = self.retry.max_deferred_rounds.max(0);
        self.retry.max_retry_duration_seconds = self.retry.max_retry_duration_seconds.max(0.0);
        self.retry.session_sticky_retries = self.retry.session_sticky_retries.max(0);
        self.retry.max_500_retries = self.retry.max_500_retries.max(0);
        // 非有限值只可能来自手工编辑的 JSON;负数与 NaN 一样收敛到「永不过期」,
        // 与调度层 `ttl_seconds <= 0` 的语义一致。
        if !self.session_sticky_ttl_hours.is_finite() {
            self.session_sticky_ttl_hours = 0.0;
        }
        self.session_sticky_ttl_hours = self.session_sticky_ttl_hours.max(0.0);

        for endpoint in &mut self.endpoints {
            if endpoint.name.is_empty() {
                endpoint.name = endpoint.id.clone();
            }
            endpoint.priority = endpoint.priority.max(0);
            endpoint.sticky_group = normalized_group(endpoint.sticky_group.take());
            if endpoint
                .catalog
                .as_ref()
                .is_some_and(EndpointCatalog::is_empty)
            {
                endpoint.catalog = None;
            }
            for mapping in &mut endpoint.mappings {
                mapping.client_pattern = mapping.client_pattern.trim().to_string();
                mapping.upstream_model = model_name::clean(&mapping.upstream_model);
                mapping
                    .capabilities
                    .sort_by_key(|capability| capability.as_str());
                mapping.capabilities.dedup();
            }
        }

        if let Some(groups) = &mut self.model_groups {
            for group in groups {
                let trimmed_name = group.name.trim();
                if trimmed_name.is_empty() {
                    group.name = group.id.clone();
                } else {
                    group.name = trimmed_name.to_string();
                }
                group.priority = group.priority.max(0);
                for model in &mut group.models {
                    *model = model_name::clean(model);
                }
                for binding in &mut group.bindings {
                    binding.priority = binding.priority.max(0);
                    if let Some(models) = &mut binding.models {
                        for model in models {
                            *model = model_name::clean(model);
                        }
                    }
                    for override_ in &mut binding.overrides {
                        override_.model = model_name::clean(&override_.model);
                        override_.upstream_model = override_
                            .upstream_model
                            .take()
                            .map(|model| model_name::clean(&model));
                    }
                }
            }
            // 入口库是默认组顺序/优先级的单一事实源:迁移产生的默认组必须
            // 跟随 endpoints 数组的后续调整,否则入口库的排序编辑对路由无效。
            self.sync_default_group_bindings();
        }

        for rule in &mut self.feature_rules {
            if rule.name.is_empty() {
                rule.name = rule.id.clone();
            }
            rule.target.model = model_name::clean(&rule.target.model);
            rule.target.endpoint_id = normalized_group(rule.target.endpoint_id.take());
        }
        self.feature_rules = builtin_rules::normalized(std::mem::take(&mut self.feature_rules));
        self
    }

    pub fn endpoint(&self, endpoint_id: &str) -> Option<&Endpoint> {
        self.endpoints
            .iter()
            .find(|endpoint| endpoint.id == endpoint_id)
    }

    /// 粘性 TTL 的调度层秒数;`0` 表示不按 TTL 淘汰(仅条目数上限)。
    pub fn session_sticky_ttl_secs(&self) -> f64 {
        if self.session_sticky_ttl_hours.is_finite() && self.session_sticky_ttl_hours > 0.0 {
            self.session_sticky_ttl_hours * 3600.0
        } else {
            0.0
        }
    }

    /// 当前配置承接的客户端模型并集；仅由 Provider 显式映射派生。
    pub fn accepted_models(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut output = Vec::new();
        let scoped = self.routing_endpoints();
        for raw in scoped.iter().flat_map(|entry| {
            let endpoint = &entry.endpoint;
            endpoint
                .mappings
                .iter()
                .map(|mapping| mapping.client_pattern.as_str())
        }) {
            let cleaned = model_name::clean(raw);
            if cleaned.is_empty() || !seen.insert(cleaned) {
                continue;
            }
            output.push(raw.to_string());
        }
        output
    }

    pub fn matches_model(&self, model: &str) -> bool {
        self.routing_endpoints()
            .iter()
            .any(|entry| entry.endpoint.mapping_for(model).is_some())
    }

    /// 全新安装的空壳：零 Provider，内建分流规则全部停用。
    pub fn bootstrap() -> Self {
        Self {
            endpoints: vec![],
            model_groups: None,
            feature_rules: builtin_rules::canonical(),
            listener: ListenerConfig::default(),
            retry: RetryPolicy::default(),
            session_sticky_ttl_hours: DEFAULT_SESSION_STICKY_TTL_HOURS,
            schema_version: SCHEMA_VERSION,
        }
    }
}

fn normalized_group(raw: Option<String>) -> Option<String> {
    let trimmed = raw?.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListenerConfig {
    #[serde(rename = "allowedCIDRs", default)]
    pub allowed_cidrs: Vec<String>,
    #[serde(rename = "authToken", default)]
    pub auth_token: String,
    #[serde(default = "ListenerConfig::default_host")]
    pub host: String,
    /// Swift 侧为 Int,监听时才校验 u16 范围——此处保持宽容。
    #[serde(default = "ListenerConfig::default_port")]
    pub port: i64,
}

impl ListenerConfig {
    fn default_host() -> String {
        "127.0.0.1".into()
    }
    fn default_port() -> i64 {
        57878
    }

    pub fn has_inbound_auth(&self) -> bool {
        !self.auth_token.is_empty()
    }
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            allowed_cidrs: Vec::new(),
            auth_token: String::new(),
            host: Self::default_host(),
            port: Self::default_port(),
        }
    }
}

/// 全局转发/重试参数。语义(见 docs/architecture.md §2-3):
/// - `response_timeout_seconds`:流式 = 响应头截止;非流式 = 整响应截止。None = 普通请求不限；
///   原生 Realtime/Live 启动由引擎额外施加有界保护。
/// - `stream_idle_timeout_seconds`:流式两次吐字最大间隔。None = 不限。
/// - `max_deferred_rounds`:历史 JSON 键名；现为跨轮可重试故障的最大轮数,0 = 不限。
/// - `max_retry_duration_seconds`:跨轮可重试故障的墙钟总闸,0 = 不限（HTTP 500 不跨轮）。
/// - `session_sticky_retries`:同一次请求中,当前粘性调度组遇到非 500 可重试故障后额外重试的次数；
///   全部遇到可重试故障后才访问其它调度组,其它组成功立即改绑。0 = 首次失败后立即 failover。
/// - `max_500_retries`:单个入口收到 HTTP 500 后的额外重试次数；0 = 不在该入口重试,
///   直接尝试下一个入口（当 `failover_on_500` 开启时）。
/// - `failover_on_500`:当前入口 HTTP 500 重试耗尽后是否切换到下一个入口；默认开启。
/// - `retry_delay_seconds`:最终失败响应可透传的 `retry_delay` 秒数；None = 不配置。
/// - `pass_through_retry_delay`:是否把 `retry_delay` 与 `Retry-After` 透传给客户端；默认开启。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetryPolicy {
    #[serde(rename = "max500Retries", default)]
    pub max_500_retries: i64,
    #[serde(
        rename = "failoverOn500",
        default = "RetryPolicy::default_failover_on_500"
    )]
    pub failover_on_500: bool,
    #[serde(rename = "maxDeferredRounds", default)]
    pub max_deferred_rounds: i64,
    #[serde(
        rename = "maxRetryDurationSeconds",
        default = "RetryPolicy::default_max_retry_duration",
        serialize_with = "trim_f64"
    )]
    pub max_retry_duration_seconds: f64,
    /// None 序列化为显式 null(与 Swift 对齐,不省略键)。
    #[serde(
        rename = "responseTimeoutSeconds",
        default,
        serialize_with = "trim_opt_f64"
    )]
    pub response_timeout_seconds: Option<f64>,
    #[serde(rename = "retryDelaySeconds", default, serialize_with = "trim_opt_f64")]
    pub retry_delay_seconds: Option<f64>,
    #[serde(
        rename = "passThroughRetryDelay",
        default = "RetryPolicy::default_pass_through_retry_delay"
    )]
    pub pass_through_retry_delay: bool,
    #[serde(
        rename = "sessionStickyRetries",
        default = "RetryPolicy::default_session_sticky_retries"
    )]
    pub session_sticky_retries: i64,
    #[serde(
        rename = "streamIdleTimeoutSeconds",
        default,
        serialize_with = "trim_opt_f64"
    )]
    pub stream_idle_timeout_seconds: Option<f64>,
}

impl RetryPolicy {
    fn default_max_retry_duration() -> f64 {
        0.0
    }
    fn default_session_sticky_retries() -> i64 {
        2
    }
    fn default_failover_on_500() -> bool {
        true
    }
    fn default_pass_through_retry_delay() -> bool {
        true
    }

    /// 不可配、不进配置文件。只把能明确归因于当前入口的故障列为 failover：
    /// 401/402/403 = 当前入口凭据/额度/权限不可用；429/网关状态 = 当前入口暂不可用。
    /// 400 属于请求错误，换入口通常无意义且可能造成重复副作用，因此原样返回。
    pub const RETRYABLE_STATUS_CODES: [u16; 17] = [
        401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530,
    ];
    /// 兼容旧 WebUI/测试命名；跨轮面现与 retryable 完全一致。
    pub const DEFERRED_STATUS_CODES: [u16; 17] = Self::RETRYABLE_STATUS_CODES;

    pub fn is_retryable_status(status: u16) -> bool {
        Self::RETRYABLE_STATUS_CODES.contains(&status)
    }

    /// HTTP 500 仅由 `max500Retries` 控制入口内重试，不进入跨轮无限重试集合。
    pub fn is_endpoint_retryable_status(status: u16) -> bool {
        status == 500 || Self::is_retryable_status(status)
    }

    pub fn is_deferred_status(status: u16) -> bool {
        Self::is_retryable_status(status)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_500_retries: 0,
            failover_on_500: Self::default_failover_on_500(),
            max_deferred_rounds: 0,
            max_retry_duration_seconds: Self::default_max_retry_duration(),
            response_timeout_seconds: None,
            retry_delay_seconds: None,
            pass_through_retry_delay: Self::default_pass_through_retry_delay(),
            session_sticky_retries: Self::default_session_sticky_retries(),
            stream_idle_timeout_seconds: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Provider 入口
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThinkingMode {
    #[serde(rename = "disabled")]
    #[default]
    Disabled,
    #[serde(rename = "passthrough")]
    Passthrough,
    #[serde(rename = "adaptive")]
    Adaptive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ContextMode {
    /// 保留客户端自己声明的上下文 beta;代理不主动添加或移除 1M。
    #[serde(rename = "standard")]
    #[default]
    Standard,
    #[serde(rename = "oneMillion")]
    OneMillion,
    /// 移除客户端声明的 `context-1m-*` beta,让上游自行决定上下文能力。
    #[serde(rename = "strip")]
    Strip,
}

pub const ANTHROPIC_CONTEXT_1M_BETA: &str = "context-1m-2025-08-07";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ProviderProtocol {
    #[serde(rename = "anthropic")]
    #[default]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
    /// OpenAI Responses API(/v1/responses);`openai` 保留为 chat/completions。
    #[serde(rename = "openai-responses")]
    OpenAIResponses,
    /// Gemini Developer API / Google AI Studio REST protocol.
    #[serde(rename = "gemini")]
    Gemini,
}

impl ProviderProtocol {
    pub fn token(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAI => "openai",
            Self::OpenAIResponses => "openai-responses",
            Self::Gemini => "gemini",
        }
    }
}

/// 入口声明的协议能力模式。`Auto` 是配置模式，不是出站协议；
/// RoutePlanner 必须先把它解析为真实 [`ProviderProtocol`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum EndpointProtocolMode {
    #[serde(rename = "auto")]
    #[default]
    Auto,
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai")]
    OpenAI,
    #[serde(rename = "openai-responses")]
    OpenAIResponses,
    #[serde(rename = "gemini")]
    Gemini,
}

impl EndpointProtocolMode {
    pub fn fixed_protocol(self) -> Option<ProviderProtocol> {
        match self {
            Self::Auto => None,
            Self::Anthropic => Some(ProviderProtocol::Anthropic),
            Self::OpenAI => Some(ProviderProtocol::OpenAI),
            Self::OpenAIResponses => Some(ProviderProtocol::OpenAIResponses),
            Self::Gemini => Some(ProviderProtocol::Gemini),
        }
    }

    pub fn resolve(
        self,
        source: ProviderProtocol,
        override_target: Option<ProviderProtocol>,
    ) -> Option<ProviderProtocol> {
        if let Some(target) = override_target {
            return match self {
                Self::Auto => Some(target),
                _ if self.fixed_protocol() == Some(target) => Some(target),
                _ => None,
            };
        }
        self.fixed_protocol().or(Some(source))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Endpoint {
    /// 明文 API Key(配置文件权限 0600);空 = 未配置。
    #[serde(rename = "apiKey", default)]
    pub api_key: String,
    #[serde(rename = "baseURL")]
    pub base_url: String,
    /// 「获取模型」拉回的目录,纯展示,不参与路由;空目录不落盘。
    #[serde(default, skip_serializing_if = "is_none")]
    pub catalog: Option<EndpointCatalog>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 唯一标识;用量统计按它聚合,改 id 历史会断代。
    pub id: String,
    /// 【实验】出站连接复用(keep-alive)。新入口编辑器默认开启；显式 false
    /// 仍按每请求新建连接处理。true 时省去每次 TCP+TLS 握手(远程中转实测
    /// ~100ms)，真实 CC 客户端本身就复用连接。false 不落盘,老配置/老壳零感知。
    #[serde(rename = "keepAlive", default, skip_serializing_if = "is_false")]
    pub keep_alive: bool,
    /// 非空则本入口**只**承接映射声明的客户端模型；旧配置的空映射会在归一化时展开。
    #[serde(default)]
    pub mappings: Vec<ModelMapping>,
    #[serde(default)]
    pub name: String,
    /// Provider 调度优先级：数值越小越优先；同级保持配置顺序。
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub priority: i64,
    #[serde(default)]
    pub protocol: EndpointProtocolMode,
    /// 同组入口共享会话粘性与冷却；None 使用自身 id 作为独立组并参与 Provider 分流。
    #[serde(rename = "stickyGroup", default, skip_serializing_if = "is_none")]
    pub sticky_group: Option<String>,
}

impl Endpoint {
    /// 调度分组:stickyGroup ?? 自身 id;只用于会话粘性归属。
    pub fn scheduling_group(&self) -> &str {
        self.sticky_group.as_deref().unwrap_or(&self.id)
    }

    /// 入口自带映射里命中该模型的规则。
    ///
    /// 精确模型名优先于前缀通配，避免旧配置迁移出的 `foo-*` 映射遮蔽用户
    /// 后续添加的 `foo-special` 精确映射；同一匹配级别仍保持配置顺序。
    pub fn mapping_for(&self, client_model: &str) -> Option<&ModelMapping> {
        let cleaned = model_name::clean(client_model);
        best_mapping(&self.mappings, &cleaned, |_| true)
    }

    /// Find the best mapping that can serve a particular request capability.
    /// Capability filtering happens before precedence selection: an exact text
    /// mapping must not hide a wildcard video/Live mapping for the same model.
    /// Within the eligible set, exact client patterns still win over wildcard
    /// patterns and configuration order remains the final tie breaker.
    pub fn mapping_for_capability(
        &self,
        client_model: &str,
        capability: ModelCapability,
    ) -> Option<&ModelMapping> {
        let cleaned = model_name::clean(client_model);
        let serves = |mapping: &ModelMapping| {
            crate::capability::mapping_serves_capability(
                &mapping.capabilities,
                &mapping.client_pattern,
                &cleaned,
                capability,
            )
        };
        let direct = best_mapping(&self.mappings, &cleaned, serves);
        if direct.is_some() {
            return direct;
        }

        // CPA's Codex OAuth handler normalizes the public Realtime family to
        // `gpt-live-1-codex` for provider selection.  Preserve the caller's
        // logical model in the route plan, but allow an installation that
        // declares only the private mapping to serve `gpt-realtime` and
        // `realtime-preview*` requests.  This alias is intentionally scoped
        // to the Live capability; text/image routes never see it.
        if capability == ModelCapability::Live
            && crate::capability::is_realtime_model_name(&cleaned)
        {
            let live_model = "gpt-live-1-codex";
            return best_mapping(&self.mappings, live_model, |mapping| {
                crate::capability::mapping_serves_capability(
                    &mapping.capabilities,
                    &mapping.client_pattern,
                    live_model,
                    capability,
                )
            });
        }
        None
    }
}

/// Return the most specific mapping that matches `client_model`.
///
/// Mapping order is a deterministic tie breaker, not the specificity rule:
/// an earlier `gpt-*` entry must not hide a later `gpt-image-*` entry.  The
/// iterator is scanned in configuration order and only a *strictly* better
/// rank replaces the current winner, so equal-length wildcards remain stable.
fn best_mapping<'a, F>(
    mappings: &'a [ModelMapping],
    client_model: &str,
    serves: F,
) -> Option<&'a ModelMapping>
where
    F: Fn(&ModelMapping) -> bool,
{
    let mut best: Option<(&ModelMapping, (u8, usize))> = None;
    for mapping in mappings {
        if !serves(mapping) {
            continue;
        }
        let Some(rank) = mapping_match_rank(&mapping.client_pattern, client_model) else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, current)| rank > *current) {
            best = Some((mapping, rank));
        }
    }
    best.map(|(mapping, _)| mapping)
}

/// Exact matches outrank wildcards; among wildcards, the longest prefix wins.
/// `model_name::pattern_matches` currently supports only a trailing `*`, so
/// this rank deliberately mirrors that contract instead of treating an
/// embedded star as a glob.
fn mapping_match_rank(pattern: &str, client_model: &str) -> Option<(u8, usize)> {
    let cleaned_pattern = model_name::clean(pattern);
    let cleaned_model = model_name::clean(client_model);
    if cleaned_pattern == cleaned_model && !cleaned_pattern.is_empty() {
        return Some((2, cleaned_pattern.len()));
    }
    let prefix = cleaned_pattern.strip_suffix('*')?;
    // A bare `*` is the legacy catch-all mapping. It has the lowest
    // wildcard rank, but must still participate so older configurations keep
    // routing and catalog behaviour; more specific prefixes outrank it.
    if !cleaned_model.starts_with(prefix) {
        return None;
    }
    Some((1, prefix.len()))
}

/// 「获取模型」的缓存目录与状态,纯展示。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct EndpointCatalog {
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub status: String,
    #[serde(rename = "updatedAt", default)]
    pub updated_at: String,
}

impl EndpointCatalog {
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
            && self.source.is_empty()
            && self.status.is_empty()
            && self.error.is_empty()
            && self.updated_at.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelMapping {
    #[serde(rename = "clientPattern")]
    pub client_pattern: String,
    #[serde(default)]
    pub context: ContextMode,
    /// 映射级首响应超时；与全局单次超时取较小值。
    #[serde(
        rename = "failoverTimeoutSeconds",
        default,
        skip_serializing_if = "is_none",
        serialize_with = "trim_opt_f64"
    )]
    pub failover_timeout_seconds: Option<f64>,
    #[serde(default)]
    pub thinking: ThinkingMode,
    /// adaptive 模式下覆盖客户端 effort；None 表示自动跟随客户端。
    #[serde(default, skip_serializing_if = "is_none")]
    pub effort: Option<model_name::ReasoningEffort>,
    /// 空串 = 与客户端模型同名。
    #[serde(rename = "upstreamModel", default)]
    pub upstream_model: String,
    /// Mapping-level capabilities. Empty means infer from `clientPattern`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<ModelCapability>,
}

impl ModelMapping {
    /// 上游模型名:留空表示与客户端模型同名。
    pub fn upstream_model_for(&self, client_model: &str) -> String {
        if self.upstream_model.is_empty() {
            client_model.to_string()
        } else {
            self.upstream_model.clone()
        }
    }
}

// ---------------------------------------------------------------------------
// 分流规则
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestKind {
    #[serde(rename = "websearch")]
    WebSearch,
    #[serde(rename = "webfetch")]
    WebFetch,
    #[serde(rename = "classifier")]
    Classifier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureRule {
    #[serde(default)]
    pub enabled: bool,
    pub id: String,
    #[serde(rename = "match", default)]
    pub match_: FeatureRuleMatch,
    #[serde(default)]
    pub name: String,
    pub target: FeatureRuleTarget,
}

/// 匹配条件;多字段为 AND,至少要有一个条件才可能命中。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FeatureRuleMatch {
    #[serde(rename = "messagesContain", default, skip_serializing_if = "is_none")]
    pub messages_contain: Option<String>,
    #[serde(rename = "modelEquals", default, skip_serializing_if = "is_none")]
    pub model_equals: Option<String>,
    #[serde(rename = "requestKind", default, skip_serializing_if = "is_none")]
    pub request_kind: Option<RequestKind>,
    #[serde(rename = "systemContains", default, skip_serializing_if = "is_none")]
    pub system_contains: Option<String>,
    #[serde(rename = "toolTypePrefix", default, skip_serializing_if = "is_none")]
    pub tool_type_prefix: Option<String>,
}

/// 分流目标：`endpoint_id` 非空则只走该 Provider 并绕过映射筛选，
/// 入口停用/删除时自动降级为候选序列；`protocol_override` 覆盖入口自身协议
/// (JSON 键名就叫 `protocol`)。`model` 恒落盘(Swift 侧非 Optional)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FeatureRuleTarget {
    #[serde(rename = "endpointID", default, skip_serializing_if = "is_none")]
    pub endpoint_id: Option<String>,
    /// 命中规则后覆盖客户端 effort；None = 跟随原请求。
    #[serde(default, skip_serializing_if = "is_none")]
    pub effort: Option<model_name::ReasoningEffort>,
    #[serde(default)]
    pub model: String,
    #[serde(rename = "protocol", default, skip_serializing_if = "is_none")]
    pub protocol_override: Option<ProviderProtocol>,
}

/// 内建分流规则的 canonical 定义:name/match 由代码维护,配置里被改坏会被纠回,
/// 只保留用户可编辑的 enabled 与 target。对齐 Swift `BuiltInFeatureRules`。
pub mod builtin_rules {
    use super::*;

    pub const IDS: [&str; 3] = ["websearch", "webfetch", "classifier"];

    pub fn is_builtin(id: &str) -> bool {
        IDS.contains(&id)
    }

    pub fn canonical() -> Vec<FeatureRule> {
        let target = |_: &str| FeatureRuleTarget {
            endpoint_id: None,
            effort: None,
            model: "claude-haiku-4-5-20251001".into(),
            protocol_override: None,
        };
        vec![
            FeatureRule {
                enabled: false,
                id: "websearch".into(),
                match_: FeatureRuleMatch {
                    request_kind: Some(RequestKind::WebSearch),
                    ..Default::default()
                },
                name: "WebSearch".into(),
                target: target("websearch"),
            },
            FeatureRule {
                enabled: false,
                id: "webfetch".into(),
                match_: FeatureRuleMatch {
                    request_kind: Some(RequestKind::WebFetch),
                    ..Default::default()
                },
                name: "WebFetch".into(),
                target: target("webfetch"),
            },
            FeatureRule {
                enabled: false,
                id: "classifier".into(),
                match_: FeatureRuleMatch {
                    request_kind: Some(RequestKind::Classifier),
                    ..Default::default()
                },
                name: "安全分类器".into(),
                target: target("classifier"),
            },
        ]
    }

    /// 内建三条按 canonical 顺序前置(纠正 name/match、保留 enabled/target,
    /// 缺失的补 canonical 停用态),自定义规则按原顺序排在其后。
    pub fn normalized(input: Vec<FeatureRule>) -> Vec<FeatureRule> {
        let mut output: Vec<FeatureRule> = canonical()
            .into_iter()
            .map(|canon| match input.iter().find(|r| r.id == canon.id) {
                Some(existing) => FeatureRule {
                    enabled: existing.enabled,
                    target: existing.target.clone(),
                    ..canon
                },
                None => canon,
            })
            .collect();
        output.extend(input.into_iter().filter(|r| !is_builtin(&r.id)));
        output
    }
}

#[cfg(test)]
mod tests {
    use super::{ContextMode, FeatureRuleTarget};
    use crate::model_name::ReasoningEffort;
    use serde_json::json;

    #[test]
    fn context_mode_strip_uses_stable_wire_value() {
        assert_eq!(
            serde_json::to_string(&ContextMode::Strip).unwrap(),
            r#""strip""#
        );
        assert_eq!(
            serde_json::from_str::<ContextMode>(r#""strip""#).unwrap(),
            ContextMode::Strip
        );
    }

    #[test]
    fn feature_rule_effort_is_optional_and_round_trips() {
        let inherited: FeatureRuleTarget = serde_json::from_value(json!({
            "model": "gpt-5.6-luna"
        }))
        .unwrap();
        assert_eq!(inherited.effort, None);

        let overridden: FeatureRuleTarget = serde_json::from_value(json!({
            "model": "gpt-5.6-luna",
            "effort": "xhigh"
        }))
        .unwrap();
        assert_eq!(overridden.effort, Some(ReasoningEffort::Xhigh));
        assert_eq!(serde_json::to_value(overridden).unwrap()["effort"], "xhigh");
    }
}
