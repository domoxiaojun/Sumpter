//! 运行事件与内存统计快照。字段兼容旧 stats.json，当前持久化由 proxy 的 SQLite store 负责。
//! (ProxyEngine.swift)。
//!
//! 磁盘形状铁律(来自真实 stats.json):
//! - `timestamp` 是 **Apple reference date(2001-01-01T00:00:00Z)秒数** 的浮点;
//! - 可选字段 None 时省略键;`id` 为大写 UUID;键序字母序;
//! - `durationMS` / `statusCode` 为整数;`kind` 是自由字符串("client"/"upstream"/"notify")。

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::config::ProviderProtocol;
use crate::routing::{RequestPurpose, RouteMode};

/// Apple reference date 与 Unix 纪元的偏移秒数。
pub const APPLE_EPOCH_OFFSET_SECS: f64 = 978_307_200.0;

pub fn unix_to_apple_epoch(unix_secs: f64) -> f64 {
    unix_secs - APPLE_EPOCH_OFFSET_SECS
}

pub fn apple_to_unix_epoch(apple_secs: f64) -> f64 {
    apple_secs + APPLE_EPOCH_OFFSET_SECS
}

pub const KIND_CLIENT: &str = "client";
pub const KIND_UPSTREAM: &str = "upstream";
pub const KIND_NOTIFY: &str = "notify";

/// 每类事件的缓冲上限(Swift `maxRecentEventsPerKind`)。
pub const MAX_RECENT_EVENTS_PER_KIND: usize = 200;

/// 客户端取消的记账状态码:不计成功也不计失败。
pub const STATUS_CLIENT_DISCONNECTED: i64 = 499;

/// 事件消息词表(specs/spec-engine.md §5.1)。
///
/// `RuntimeEvent.message` 只放机器可读 token(或「token: 原因」形状),多 token 用 `"; "`
/// 连接;中文翻译全部在 UI 层(Swift `RuntimeEventPresentation`)。改词表必须同步
/// spec §5.1、Swift 映射与两边清单测试,否则 UI 摘要会退回原始串。
pub mod message_tokens {
    /// 多 token 连接符。
    pub const JOIN: &str = "; ";
    /// `pinned <ip>`:该次尝试走 IP 直连。
    pub const PINNED_PREFIX: &str = "pinned ";
    /// `bridge <openai|openai-responses>`:该请求经协议桥接。
    pub const BRIDGE_PREFIX: &str = "bridge ";

    /// 入站方言透传生效(后接 chat|responses):出站即客户端方言,两侧零转换。
    pub const PASSTHROUGH_PREFIX: &str = "passthrough ";
    /// `deferred_rounds <n>`:兼容历史 token；任一可重试故障跨轮后记录总轮数 n(>1)。
    pub const DEFERRED_ROUNDS_PREFIX: &str = "deferred_rounds ";
    /// 该请求无 tools、仅单条 user 消息,且未命中任何已知的 CC 内部请求指纹。
    ///
    /// 只陈述事实不猜用途:CC 主对话恒定带 tools,所以这个形状基本只可能是内部辅助请求。
    /// 命中即提示「指纹可能随 CC 升级失配」——否则失配会静默退化成「普通请求」无人察觉。
    pub const UNMATCHED_NO_TOOLS: &str = "unmatched_no_tools";
    /// 全轮耗尽后的最终失败(响应体里也是这个 error)。
    pub const UPSTREAM_RETRYABLE_STATUS: &str = "upstream_retryable_status";
    /// 整轮结束无任何可重试状态也无传输错误(如全部入口被守卫跳过)。
    pub const ALL_ENDPOINTS_FAILED: &str = "all endpoints failed";
    /// 入站鉴权失败(401)。
    pub const INBOUND_AUTH_REQUIRED: &str = "inbound_auth_required";
    /// openai 系入口 + 请求带 tools,被守卫跳过(400 upstream 事件)。
    pub const OPENAI_TOOLS_UNSUPPORTED: &str = "openai_tools_unsupported";
    /// 请求体解析失败(400 client 事件)。
    pub const BODY_NOT_JSON: &str = "body is not JSON";
    /// `/v1/messages` 请求体不满足 Anthropic 的最低结构要求。
    pub const ANTHROPIC_REQUEST_INVALID: &str = "anthropic request shape invalid";

    /// 入站 OpenAI 兼容层转换失败(后接原因,如 missing model)。
    pub const INBOUND_CONVERT_FAILED_PREFIX: &str = "inbound_convert_failed: ";
    pub const BODY_NOT_OBJECT: &str = "body is not an object";
    /// `stream interrupted: <原因>`:已回写响应头后的断流/吐字超时。
    pub const STREAM_INTERRUPTED_PREFIX: &str = "stream interrupted: ";
    /// `client_disconnected*`:客户端取消(与 499 状态码成对)。
    pub const CLIENT_DISCONNECTED_PREFIX: &str = "client_disconnected";

    pub fn pinned(ip: &str) -> String {
        format!("{PINNED_PREFIX}{ip}")
    }

    /// 组合 token:None/空串跳过;全空返回 None(事件不落 message 键)。
    pub fn join(parts: &[Option<String>]) -> Option<String> {
        let kept: Vec<&str> = parts
            .iter()
            .filter_map(|p| p.as_deref())
            .filter(|p| !p.is_empty())
            .collect();
        if kept.is_empty() {
            None
        } else {
            Some(kept.join(JOIN))
        }
    }
}

/// 事件阶段:进行中(已入流未结束)或已完成。新事件必须显式写入阶段；None
/// 仅用于升级前的 stats.json，并按已完成处理以保持兼容。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeEventPhase {
    #[serde(rename = "inFlight")]
    InFlight,
    #[serde(rename = "completed")]
    Completed,
}

/// 请求/尝试的最终结果。None 只用于 in-flight、非请求事件或旧 stats.json。
///
/// 不能仅靠 HTTP 状态判断最终结果：响应头已经是 200 后仍可能发生流中断，
/// 此时线上状态仍是 200，但最终结果必须记为 failed。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeEventOutcome {
    Succeeded,
    Failed,
    Cancelled,
}

/// 结构化失败分类。字符串值是 stats/admin API 的稳定契约。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFailureKind {
    ResponseTimeout,
    ConnectionFailed,
    InvalidResponse,
    UpstreamHttpStatus,
    StreamIdleTimeout,
    StreamInterrupted,
    UpstreamResponseIncomplete,
    UpstreamResponseFailed,
    EndpointsExhausted,
    ClientCancelled,
    ClientRequestRejected,
}

impl RuntimeFailureKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResponseTimeout => "response_timeout",
            Self::ConnectionFailed => "connection_failed",
            Self::InvalidResponse => "invalid_response",
            Self::UpstreamHttpStatus => "upstream_http_status",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::StreamInterrupted => "stream_interrupted",
            Self::UpstreamResponseIncomplete => "upstream_response_incomplete",
            Self::UpstreamResponseFailed => "upstream_response_failed",
            Self::EndpointsExhausted => "endpoints_exhausted",
            Self::ClientCancelled => "client_cancelled",
            Self::ClientRequestRejected => "client_request_rejected",
        }
    }
}

/// 入站客户端类型。字符串值是 stats/admin API 的稳定契约;判定只看入站
/// UA 与入站方言,不猜请求体内容。None = 升级前的旧事件,或请求在读到
/// 入站信息前就被拒。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    ClaudeCode,
    Codex,
    /// Grok Build CLI；其公开客户端产品名仍沿用 `grok-shell`。
    GrokBuild,
    /// 经 OpenAI 兼容层入站但 UA 不是已知客户端。
    OpenaiCompat,
    /// Anthropic 入站且 UA 不是已知客户端(含缺 UA)。
    Unknown,
}

impl ClientKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude_code",
            Self::Codex => "codex",
            Self::GrokBuild => "grok_build",
            Self::OpenaiCompat => "openai_compat",
            Self::Unknown => "unknown",
        }
    }

    /// UA 优先(claude-cli / codex / grok-shell 各有稳定产品名),UA 认不出再按
    /// `Originator` 产品标识判断，最后按入站方言兜底。`Originator` 是 Codex
    /// Desktop 在部分 WebRTC/Live 请求中唯一稳定的客户端身份；只看 UA 会把
    /// 这些请求错误记成 `openai_compat`。
    ///
    /// OpenAI 兼容层入站的未知 UA 记 openai_compat 而非 unknown ——「走哪个兼容层」
    /// 本身就是排障时最想知道的信息。
    pub fn detect(user_agent: Option<&str>, openai_inbound: bool) -> Self {
        Self::detect_with_originator(user_agent, None, openai_inbound)
    }

    /// Detect a client using both transport identity headers.  The originator
    /// value is deliberately treated as a product hint (not an authorization
    /// signal); it is bounded and normalized by the caller/header parser.
    pub fn detect_with_originator(
        user_agent: Option<&str>,
        originator: Option<&str>,
        openai_inbound: bool,
    ) -> Self {
        if let Some(ua) = user_agent {
            let ua = ua.trim();
            if ua.starts_with("claude-cli/") {
                return Self::ClaudeCode;
            }
            let lower = ua.to_ascii_lowercase();
            if lower.contains("codex") {
                return Self::Codex;
            }
            // Grok Build 的 CLI UA 是 `grok-shell/<version>`；分页器会在它前面再带
            // `grok-pager/...`，所以按 CPA 一样匹配 UA 中的 grok-shell 产品标识。
            if lower.contains("grok-shell") {
                return Self::GrokBuild;
            }
        }
        if let Some(originator) = originator {
            let lower = originator.trim().to_ascii_lowercase();
            // CPA accepts both the desktop product spelling and the CLI/TUI
            // originators.  Match a token boundary-ish substring so versioned
            // values such as `Codex Desktop/1.2` continue to work, while not
            // treating an arbitrary OpenAI originator as Codex.
            if lower.contains("codex desktop")
                || lower.contains("codex_cli_rs")
                || lower.contains("codex-tui")
            {
                return Self::Codex;
            }
        }
        if openai_inbound {
            Self::OpenaiCompat
        } else {
            Self::Unknown
        }
    }
}

/// 失败发生在哪个边界，用于区分“尚未收到响应头”和“200 已发出后断流”。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeFailurePhase {
    BeforeResponse,
    ResponseHeaders,
    ResponseStream,
}

impl RuntimeFailurePhase {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BeforeResponse => "before_response",
            Self::ResponseHeaders => "response_headers",
            Self::ResponseStream => "response_stream",
        }
    }
}

fn is_none<T>(v: &Option<T>) -> bool {
    v.is_none()
}

/// Codex turn metadata JSON 的防御性解析上限。超限只影响观测，不影响请求转发。
pub const CODEX_METADATA_MAX_JSON_BYTES: usize = 64 * 1024;
const CODEX_METADATA_MAX_ID_BYTES: usize = 128;
const CODEX_METADATA_MAX_LABEL_BYTES: usize = 128;
const CODEX_METADATA_MAX_AGENT_NAME_BYTES: usize = 256;
const CODEX_METADATA_MAX_PATH_BYTES: usize = 256;
const CODEX_METADATA_MAX_URL_BYTES: usize = 256;
// Runtime SQLite keeps a bounded source copy for local session/workspace
// attribution.  The source copy is deliberately separate from the compact
// display projection below; it never includes request credentials or bodies.
const CODEX_METADATA_MAX_SOURCE_ID_BYTES: usize = 256;
const CODEX_METADATA_MAX_SOURCE_LABEL_BYTES: usize = 512;
const CODEX_METADATA_MAX_SOURCE_PATH_BYTES: usize = 2048;
const CODEX_METADATA_MAX_SOURCE_WORKSPACES: usize = 32;
const CODEX_METADATA_MAX_WORKSPACES: usize = 32;
const CODEX_METADATA_MAX_REMOTE_URLS: usize = 32;
const CODEX_METADATA_MAX_NAMESPACES: usize = 64;
const CODEX_METADATA_MAX_FUNCTIONS_PER_NAMESPACE: usize = 128;
const CODEX_METADATA_MAX_FUNCTIONS_TOTAL: usize = 512;
const CODEX_METADATA_MAX_EXTRAS: usize = 16;
const CODEX_METADATA_MAX_EXTRA_KEY_BYTES: usize = 64;
const CODEX_METADATA_MAX_EXTRA_VALUE_BYTES: usize = 128;

/// Codex 工作区摘要。map 的 key 是折叠后的本地路径，不保存完整用户目录。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexWorkspaceMetadata {
    #[serde(
        rename = "associatedRemoteURLs",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub associated_remote_urls: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub latest_git_commit_hash: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub has_changes: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexToolSourceMetadata {
    pub kind: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub server_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexToolFunctionMetadata {
    #[serde(default, skip_serializing_if = "is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub direct: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub code_mode_name: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub deferred: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub source: Option<CodexToolSourceMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexToolNamespaceMetadata {
    #[serde(default, skip_serializing_if = "is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: BTreeMap<String, CodexToolFunctionMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexCompactionMetadata {
    #[serde(default, skip_serializing_if = "is_none")]
    pub trigger: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub implementation: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub strategy: Option<String>,
}

/// 客户端自己声明的项目归因，来自入站 `X-Sumpter-*` header。
///
/// 用途是给 Claude Code 之类**不上行 workspace 结构**的客户端补项目维度：CC 的
/// `cwd`/`workspace.project_dir` 只存在于 statusLine/hook 的 stdin JSON，不进请求体也不进
/// header，代理在 HTTP 层看不到，所以只能由用户通过 `ANTHROPIC_CUSTOM_HEADERS` 主动带上。
///
/// 与 [`CodexMetadata`] 的关键区别是**可信度**：这里的值是客户端自称的，不是客户端结构化采集
/// 的，因此归因时排在 Codex workspace 之后，并在 analytics 里标成独立来源。三个 header 都在
/// 出站黑名单里，读完即剥离，不会外泄给上游。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientDeclaredMetadata {
    /// 项目名。裸名字或路径皆可，归因时按路径末段取。
    #[serde(default, skip_serializing_if = "is_none")]
    pub project: Option<String>,
    /// 工作区路径，已按 [`sanitize_workspace_path`] 脱敏到尾两段，不含完整绝对路径。
    #[serde(default, skip_serializing_if = "is_none")]
    pub workspace: Option<String>,
    /// Git remote，已去掉凭据与 query/fragment。
    #[serde(rename = "gitRemote", default, skip_serializing_if = "is_none")]
    pub git_remote: Option<String>,
    /// 有界源项目名，仅用于本地 SQLite 复盘；显示/统计仍使用 `project`。
    #[serde(rename = "sourceProject", default, skip_serializing_if = "is_none")]
    pub source_project: Option<String>,
    /// 有界源工作区路径，仅用于本地 SQLite 复盘；不把原始 Git remote 凭据复制进来。
    #[serde(rename = "sourceWorkspace", default, skip_serializing_if = "is_none")]
    pub source_workspace: Option<String>,
}

impl ClientDeclaredMetadata {
    /// 从入站 header 解析客户端声明的项目归因。三个值全缺时返回 `None`，
    /// 让未配置的用户与历史事件保持紧凑 wire 形状。
    ///
    /// 畸形输入（控制字符、超长、空值）一律丢弃该字段而不是报错——归因是观测能力，
    /// 不能影响请求转发。
    pub fn from_headers(headers: &[(String, String)]) -> Option<Self> {
        // 一次性 state：这些脱敏函数的 flags 服务于 Codex 观测口径，客户端声明不复用它的
        // redacted/conflict 报告，所以就地丢弃。
        let mut state = CodexMetadataParseState::default();

        let project_raw = Self::clean_header(headers, "x-sumpter-project", &mut state);
        let workspace_raw = Self::clean_header(headers, "x-sumpter-workspace", &mut state);
        let project = project_raw
            .as_deref()
            .and_then(|raw| bounded_nonempty(raw, CODEX_METADATA_MAX_LABEL_BYTES, &mut state));
        let workspace = workspace_raw
            .as_deref()
            .map(|raw| sanitize_workspace_path(raw, &mut state));
        let source_project = project_raw.as_deref().and_then(|raw| {
            source_bounded_nonempty(raw, CODEX_METADATA_MAX_SOURCE_LABEL_BYTES, &mut state)
        });
        let source_workspace = workspace_raw.as_deref().and_then(|raw| {
            source_bounded_nonempty(raw, CODEX_METADATA_MAX_SOURCE_PATH_BYTES, &mut state)
        });
        let git_remote = Self::clean_header(headers, "x-sumpter-git-remote", &mut state)
            .and_then(|raw| sanitize_remote_url(&raw, &mut state));

        if project.is_none()
            && workspace.is_none()
            && git_remote.is_none()
            && source_project.is_none()
            && source_workspace.is_none()
        {
            return None;
        }
        Some(Self {
            project,
            workspace,
            git_remote,
            source_project,
            source_workspace,
        })
    }

    /// 取 header 并拒掉空值与含控制字符的值（与 analytics 侧 session id 同口径）。
    fn clean_header(
        headers: &[(String, String)],
        name: &str,
        state: &mut CodexMetadataParseState,
    ) -> Option<String> {
        let value = first_header_value(headers, name, state)?;
        let trimmed = value.trim();
        if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
            return None;
        }
        Some(trimmed.to_string())
    }
}

/// Codex 0.148 Responses 的安全观测形状。
///
/// canonical 来源是请求体 `client_metadata["x-codex-turn-metadata"]`；flat body 与
/// HTTP header 只作兼容回退。结构不保存鉴权、prompt、turn state 或 tracing 原值；
/// `source*` 字段只保留有界的 installation/workspace 原值，供本地 SQLite 复盘归属。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexMetadata {
    /// 稳定 SHA-256 短指纹，不是安装 ID 原文。
    #[serde(rename = "installationID", default, skip_serializing_if = "is_none")]
    pub installation_id: Option<String>,
    /// 本地复盘用的有界源 installation ID；兼容展示字段仍保留短指纹。
    #[serde(
        rename = "sourceInstallationID",
        default,
        skip_serializing_if = "is_none"
    )]
    pub source_installation_id: Option<String>,
    #[serde(rename = "sessionID", default, skip_serializing_if = "is_none")]
    pub session_id: Option<String>,
    #[serde(rename = "threadID", default, skip_serializing_if = "is_none")]
    pub thread_id: Option<String>,
    /// Codex multi-agent 路径，例如 `/root` 或 `/root/worker`。
    #[serde(default, skip_serializing_if = "is_none")]
    pub agent_name: Option<String>,
    #[serde(rename = "turnID", default, skip_serializing_if = "is_none")]
    pub turn_id: Option<String>,
    #[serde(rename = "windowID", default, skip_serializing_if = "is_none")]
    pub window_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub request_kind: Option<String>,
    #[serde(
        rename = "forkedFromThreadID",
        default,
        skip_serializing_if = "is_none"
    )]
    pub forked_from_thread_id: Option<String>,
    #[serde(rename = "parentThreadID", default, skip_serializing_if = "is_none")]
    pub parent_thread_id: Option<String>,
    #[serde(rename = "parentTurnID", default, skip_serializing_if = "is_none")]
    pub parent_turn_id: Option<String>,
    #[serde(rename = "rootTurnID", default, skip_serializing_if = "is_none")]
    pub root_turn_id: Option<String>,
    /// `x-openai-subagent` 的兼容 header 值，例如 thread spawn 的 `collab_spawn`。
    #[serde(default, skip_serializing_if = "is_none")]
    pub subagent_header: Option<String>,
    /// canonical 值；thread spawn 对应 `thread_spawn`，与 header 值刻意不同。
    #[serde(default, skip_serializing_if = "is_none")]
    pub subagent_kind: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub thread_source: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub sandbox: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub auto_review_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub node_repl_auto_review_required: Option<bool>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub node_repl_disabled: Option<bool>,
    #[serde(
        rename = "turnStartedAtUnixMS",
        default,
        skip_serializing_if = "is_none"
    )]
    pub turn_started_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub workspaces: BTreeMap<String, CodexWorkspaceMetadata>,
    /// 本地复盘用的有界源工作区路径；统计投影仍只使用 `workspaces` 的脱敏键。
    #[serde(
        rename = "sourceWorkspacePaths",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub source_workspace_paths: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tool_namespaces_info: BTreeMap<String, CodexToolNamespaceMetadata>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub compaction: Option<CodexCompactionMetadata>,
    /// 通过 Codex 自身 key/value 约束且不属于保留字段的扩展元数据。
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extras: BTreeMap<String, String>,
    /// 其余安全 transport 投影。
    #[serde(default, skip_serializing_if = "is_none")]
    pub originator: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub beta_features: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub memgen_request: Option<String>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub responses_lite: Option<String>,
    #[serde(
        rename = "wsStreamRequestStartMS",
        default,
        skip_serializing_if = "is_none"
    )]
    pub ws_stream_request_start_ms: Option<i64>,
    /// 实际观察到的来源，按解析优先级排列。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<String>,
    /// 只记录被主动脱敏/忽略的字段名，不记录对应原值。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redacted_fields: Vec<String>,
    #[serde(default)]
    pub malformed: bool,
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub has_conflicts: bool,
    /// `field:ignoredSource`；canonical 值总是优先，冲突列表不含原值。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conflicts: Vec<String>,
    #[serde(default)]
    pub is_subagent: bool,
    /// 仅在有独立 subagent 证据且缺 authoritative parent 时，才从 fork 关系推断。
    #[serde(rename = "parentThreadIDInferred", default)]
    pub parent_thread_id_inferred: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct CodexMetadataCandidate {
    installation_id: Option<String>,
    source_installation_id: Option<String>,
    session_id: Option<String>,
    thread_id: Option<String>,
    agent_name: Option<String>,
    turn_id: Option<String>,
    window_id: Option<String>,
    request_kind: Option<String>,
    forked_from_thread_id: Option<String>,
    parent_thread_id: Option<String>,
    parent_turn_id: Option<String>,
    root_turn_id: Option<String>,
    subagent_header: Option<String>,
    subagent_kind: Option<String>,
    thread_source: Option<String>,
    sandbox: Option<String>,
    sandbox_mode: Option<String>,
    auto_review_enabled: Option<bool>,
    node_repl_auto_review_required: Option<bool>,
    node_repl_disabled: Option<bool>,
    turn_started_at_unix_ms: Option<i64>,
    workspaces: Option<BTreeMap<String, CodexWorkspaceMetadata>>,
    source_workspace_paths: Option<Vec<String>>,
    tool_namespaces_info: Option<BTreeMap<String, CodexToolNamespaceMetadata>>,
    compaction: Option<CodexCompactionMetadata>,
    extras: BTreeMap<String, String>,
    originator: Option<String>,
    beta_features: Option<String>,
    memgen_request: Option<String>,
    responses_lite: Option<String>,
    ws_stream_request_start_ms: Option<i64>,
}

impl CodexMetadataCandidate {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Default)]
struct CodexMetadataParseState {
    observed: bool,
    malformed: bool,
    truncated: bool,
    sources: Vec<String>,
    redacted_fields: Vec<String>,
    conflicts: Vec<String>,
}

impl CodexMetadataParseState {
    fn source(&mut self, source: &str) {
        self.observed = true;
        push_unique(&mut self.sources, source.to_string());
    }

    fn redact(&mut self, field: &str) {
        self.observed = true;
        push_unique(&mut self.redacted_fields, field.to_string());
    }

    fn conflict(&mut self, field: &str, ignored_source: &str) {
        self.observed = true;
        push_unique(&mut self.conflicts, format!("{field}:{ignored_source}"));
    }
}

impl CodexMetadata {
    /// 从 Responses 请求体与原始 header 中解析 Codex 元数据。
    ///
    /// 优先级：canonical body > flat body > canonical header > direct headers >
    /// `thread-id`/`x-client-request-id` 与 `session-id` identity fallback。
    /// 任意畸形/超限输入只会设置 flags，调用者应继续原样转发请求。
    pub fn from_request(headers: &[(String, String)], body: Option<&Value>) -> Option<Self> {
        let mut state = CodexMetadataParseState::default();
        let mut merged = CodexMetadataCandidate::default();

        if let Some(client_metadata) = body
            .and_then(Value::as_object)
            .and_then(|body| body.get("client_metadata"))
        {
            match client_metadata {
                Value::Object(object) => {
                    record_redacted_client_metadata(object, &mut state);
                    if let Some(canonical) = object.get("x-codex-turn-metadata") {
                        state.source("bodyCanonical");
                        if let Some(candidate) = parse_canonical_value(canonical, &mut state) {
                            merge_candidate(&mut merged, candidate, "bodyCanonical", &mut state);
                        }
                    }
                    let flat = parse_flat_client_metadata(object, &mut state);
                    merge_candidate(&mut merged, flat, "bodyFlat", &mut state);
                }
                Value::Null => {}
                _ => {
                    state.observed = true;
                    state.malformed = true;
                }
            }
        }

        if let Some(raw) = first_header_value(headers, "x-codex-turn-metadata", &mut state) {
            state.source("headerCanonical");
            if let Some(candidate) = parse_metadata_json(&raw, &mut state) {
                merge_candidate(&mut merged, candidate, "headerCanonical", &mut state);
            }
        }

        record_redacted_headers(headers, &mut state);
        let direct = parse_direct_headers(headers, &mut state);
        merge_candidate(&mut merged, direct, "headers", &mut state);
        let fallback = parse_identity_fallback(headers, &mut state);
        merge_candidate(&mut merged, fallback, "identityFallback", &mut state);

        if !state.observed && merged.is_empty() {
            return None;
        }

        let is_subagent = merged.subagent_header.is_some()
            || merged.subagent_kind.is_some()
            || merged.thread_source.as_deref().is_some_and(|source| {
                source.eq_ignore_ascii_case("subagent")
                    || source.eq_ignore_ascii_case("memory_consolidation")
            });
        let mut parent_thread_id = merged.parent_thread_id;
        let mut parent_thread_id_inferred = false;
        if parent_thread_id.is_none()
            && is_subagent
            && let Some(forked_from_thread_id) = merged.forked_from_thread_id.as_ref()
        {
            parent_thread_id = Some(forked_from_thread_id.clone());
            parent_thread_id_inferred = true;
        }

        Some(Self {
            installation_id: merged.installation_id,
            source_installation_id: merged.source_installation_id,
            session_id: merged.session_id,
            thread_id: merged.thread_id,
            agent_name: merged.agent_name,
            turn_id: merged.turn_id,
            window_id: merged.window_id,
            request_kind: merged.request_kind,
            forked_from_thread_id: merged.forked_from_thread_id,
            parent_thread_id,
            parent_turn_id: merged.parent_turn_id,
            root_turn_id: merged.root_turn_id,
            subagent_header: merged.subagent_header,
            subagent_kind: merged.subagent_kind,
            thread_source: merged.thread_source,
            sandbox: merged.sandbox,
            sandbox_mode: merged.sandbox_mode,
            auto_review_enabled: merged.auto_review_enabled,
            node_repl_auto_review_required: merged.node_repl_auto_review_required,
            node_repl_disabled: merged.node_repl_disabled,
            turn_started_at_unix_ms: merged.turn_started_at_unix_ms,
            workspaces: merged.workspaces.unwrap_or_default(),
            source_workspace_paths: merged.source_workspace_paths.unwrap_or_default(),
            tool_namespaces_info: merged.tool_namespaces_info.unwrap_or_default(),
            compaction: merged.compaction,
            extras: merged.extras,
            originator: merged.originator,
            beta_features: merged.beta_features,
            memgen_request: merged.memgen_request,
            responses_lite: merged.responses_lite,
            ws_stream_request_start_ms: merged.ws_stream_request_start_ms,
            sources: state.sources,
            redacted_fields: state.redacted_fields,
            malformed: state.malformed,
            truncated: state.truncated,
            has_conflicts: !state.conflicts.is_empty(),
            conflicts: state.conflicts,
            is_subagent,
            parent_thread_id_inferred,
        })
    }
}

/// Stable classification of Codex thread sources for analytics and display.
///
/// `ThreadSource::Feature(String)` is intentionally open-ended in Codex, so
/// the raw `threadSource` remains the detail value while this enum provides a
/// bounded dimension.  This classification is observational only; it must not
/// affect routing, authorization, or billing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexThreadClass {
    User,
    Ambient,
    System,
    Title,
    Automation,
    AutomatedReview,
    GuardianReview,
    MemoryConsolidation,
    Subagent,
    Feature,
    Unknown,
}

impl CodexThreadClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Ambient => "ambient",
            Self::System => "system",
            Self::Title => "title",
            Self::Automation => "automation",
            Self::AutomatedReview => "automated_review",
            Self::GuardianReview => "guardian_review",
            Self::MemoryConsolidation => "memory_consolidation",
            Self::Subagent => "subagent",
            Self::Feature => "feature",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether an event can be safely attributed to a project dimension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodexAttributionScope {
    Project,
    InternalFeature,
    Unknown,
}

impl CodexAttributionScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::InternalFeature => "internal_feature",
            Self::Unknown => "unknown",
        }
    }
}

/// Classify the raw Codex thread source without treating arbitrary feature
/// names as project identities.
pub fn codex_thread_class(metadata: Option<&CodexMetadata>) -> CodexThreadClass {
    let Some(source) = metadata.and_then(|metadata| metadata.thread_source.as_deref()) else {
        return CodexThreadClass::Unknown;
    };
    let source = source.trim();
    if source.is_empty() {
        return CodexThreadClass::Unknown;
    }
    match source {
        "user" => CodexThreadClass::User,
        "system" => CodexThreadClass::System,
        "title" => CodexThreadClass::Title,
        "automation" => CodexThreadClass::Automation,
        "automated_review" => CodexThreadClass::AutomatedReview,
        "guardian_review" => CodexThreadClass::GuardianReview,
        "memory_consolidation" => CodexThreadClass::MemoryConsolidation,
        "subagent" => CodexThreadClass::Subagent,
        value if value.starts_with("ambient") => CodexThreadClass::Ambient,
        _ => CodexThreadClass::Feature,
    }
}

/// Derive attribution scope independently from the thread class.  A backend
/// feature with an explicit workspace is still attributable to that project;
/// an ambient feature without workspace evidence is kept in a separate
/// internal scope instead of inflating `unidentified_project`.
pub fn codex_attribution_scope(
    metadata: Option<&CodexMetadata>,
    declared: Option<&ClientDeclaredMetadata>,
) -> CodexAttributionScope {
    let has_workspace = metadata.is_some_and(|metadata| !metadata.workspaces.is_empty());
    let has_declared_project = declared.is_some_and(|declared| {
        declared
            .project
            .as_deref()
            .or(declared.workspace.as_deref())
            .or(declared.git_remote.as_deref())
            .is_some_and(|value| !value.trim().is_empty())
    });
    if has_workspace || has_declared_project {
        return CodexAttributionScope::Project;
    }
    match codex_thread_class(metadata) {
        CodexThreadClass::Ambient
        | CodexThreadClass::System
        | CodexThreadClass::Title
        | CodexThreadClass::Automation
        | CodexThreadClass::AutomatedReview
        | CodexThreadClass::GuardianReview
        | CodexThreadClass::MemoryConsolidation
        | CodexThreadClass::Subagent => CodexAttributionScope::InternalFeature,
        CodexThreadClass::User | CodexThreadClass::Feature | CodexThreadClass::Unknown => {
            CodexAttributionScope::Unknown
        }
    }
}

fn parse_canonical_value(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<CodexMetadataCandidate> {
    match value {
        Value::String(raw) => parse_metadata_json(raw, state),
        // 接受中间件错误解包后的对象以保住观测字段，同时明确标记非标准 wire。
        Value::Object(object) => {
            state.malformed = true;
            Some(parse_metadata_object(object, state))
        }
        Value::Null => None,
        _ => {
            state.malformed = true;
            None
        }
    }
}

fn parse_metadata_json(
    raw: &str,
    state: &mut CodexMetadataParseState,
) -> Option<CodexMetadataCandidate> {
    if raw.len() > CODEX_METADATA_MAX_JSON_BYTES {
        state.truncated = true;
        return None;
    }
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(object)) => Some(parse_metadata_object(&object, state)),
        Ok(_) | Err(_) => {
            state.malformed = true;
            None
        }
    }
}

fn parse_metadata_object(
    object: &Map<String, Value>,
    state: &mut CodexMetadataParseState,
) -> CodexMetadataCandidate {
    let mut metadata = CodexMetadataCandidate {
        installation_id: object_installation_id(object, "installation_id", state),
        source_installation_id: object_source_string(
            object,
            "installation_id",
            CODEX_METADATA_MAX_SOURCE_ID_BYTES,
            state,
        ),
        session_id: object_string(object, "session_id", CODEX_METADATA_MAX_ID_BYTES, state),
        thread_id: object_string(object, "thread_id", CODEX_METADATA_MAX_ID_BYTES, state),
        agent_name: object_string(
            object,
            "agent_name",
            CODEX_METADATA_MAX_AGENT_NAME_BYTES,
            state,
        ),
        turn_id: object_string(object, "turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        window_id: object_string(object, "window_id", CODEX_METADATA_MAX_ID_BYTES, state),
        request_kind: object_string(
            object,
            "request_kind",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        forked_from_thread_id: object_string(
            object,
            "forked_from_thread_id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        parent_thread_id: object_string(
            object,
            "parent_thread_id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        parent_turn_id: object_string(object, "parent_turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        root_turn_id: object_string(object, "root_turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        subagent_kind: object_string(
            object,
            "subagent_kind",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        thread_source: object_string(
            object,
            "thread_source",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        sandbox: object_string(object, "sandbox", CODEX_METADATA_MAX_LABEL_BYTES, state),
        sandbox_mode: object_string(
            object,
            "sandbox_mode",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        auto_review_enabled: object_bool(object, "auto_review_enabled", state),
        node_repl_auto_review_required: object_bool(
            object,
            "node_repl_auto_review_required",
            state,
        ),
        node_repl_disabled: object_bool(object, "node_repl_disabled", state),
        turn_started_at_unix_ms: object_i64(object, "turn_started_at_unix_ms", state),
        workspaces: object
            .get("workspaces")
            .and_then(|value| parse_workspaces(value, state)),
        source_workspace_paths: object
            .get("workspaces")
            .and_then(|value| source_workspace_paths(value, state)),
        tool_namespaces_info: object
            .get("tool_namespaces_info")
            .and_then(|value| parse_tool_namespaces(value, state)),
        compaction: object
            .get("compaction")
            .and_then(|value| parse_compaction(value, state)),
        ..CodexMetadataCandidate::default()
    };

    for (key, value) in object {
        if is_known_canonical_key(key) {
            continue;
        }
        if is_sensitive_metadata_key(key) {
            state.redact(key);
            continue;
        }
        let Some(raw) = value.as_str() else {
            // 未知结构化字段留给未来版本，不误当作字符串 extra。
            continue;
        };
        if metadata.extras.len() >= CODEX_METADATA_MAX_EXTRAS {
            state.truncated = true;
            continue;
        }
        if !valid_extra_key(key) || key.len() > CODEX_METADATA_MAX_EXTRA_KEY_BYTES {
            state.malformed = true;
            continue;
        }
        if let Some(value) = bounded_nonempty(raw, CODEX_METADATA_MAX_EXTRA_VALUE_BYTES, state) {
            metadata.extras.insert(key.clone(), value);
        }
    }
    metadata
}

fn parse_flat_client_metadata(
    object: &Map<String, Value>,
    state: &mut CodexMetadataParseState,
) -> CodexMetadataCandidate {
    let mut metadata = CodexMetadataCandidate {
        installation_id: object_installation_id(object, "x-codex-installation-id", state),
        source_installation_id: object_source_string(
            object,
            "x-codex-installation-id",
            CODEX_METADATA_MAX_SOURCE_ID_BYTES,
            state,
        ),
        session_id: object_string(object, "session_id", CODEX_METADATA_MAX_ID_BYTES, state),
        thread_id: object_string(object, "thread_id", CODEX_METADATA_MAX_ID_BYTES, state),
        agent_name: object_string(
            object,
            "agent_name",
            CODEX_METADATA_MAX_AGENT_NAME_BYTES,
            state,
        ),
        turn_id: object_string(object, "turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        window_id: object_string(
            object,
            "x-codex-window-id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        parent_thread_id: object_string(
            object,
            "x-codex-parent-thread-id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        parent_turn_id: object_string(object, "parent_turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        root_turn_id: object_string(object, "root_turn_id", CODEX_METADATA_MAX_ID_BYTES, state),
        subagent_header: object_string(
            object,
            "x-openai-subagent",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        originator: object_string(object, "originator", CODEX_METADATA_MAX_LABEL_BYTES, state),
        beta_features: object_string(
            object,
            "x-codex-beta-features",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        memgen_request: object_string(
            object,
            "x-openai-memgen-request",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        ws_stream_request_start_ms: object_i64(object, "x-codex-ws-stream-request-start-ms", state),
        ..CodexMetadataCandidate::default()
    };
    let direct_lite = object_string(
        object,
        "x-openai-internal-codex-responses-lite",
        CODEX_METADATA_MAX_LABEL_BYTES,
        state,
    );
    let websocket_lite = object_string(
        object,
        "ws_request_header_x_openai_internal_codex_responses_lite",
        CODEX_METADATA_MAX_LABEL_BYTES,
        state,
    );
    metadata.responses_lite = direct_lite.or_else(|| websocket_lite.clone());
    if metadata.responses_lite.is_some()
        && websocket_lite.is_some()
        && metadata.responses_lite != websocket_lite
    {
        state.conflict("responsesLite", "bodyFlatWebSocketProjection");
    }
    metadata
}

fn parse_direct_headers(
    headers: &[(String, String)],
    state: &mut CodexMetadataParseState,
) -> CodexMetadataCandidate {
    CodexMetadataCandidate {
        installation_id: header_installation_id(headers, "x-codex-installation-id", state),
        source_installation_id: header_source_string(
            headers,
            "x-codex-installation-id",
            CODEX_METADATA_MAX_SOURCE_ID_BYTES,
            state,
        ),
        window_id: header_string(
            headers,
            "x-codex-window-id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        parent_thread_id: header_string(
            headers,
            "x-codex-parent-thread-id",
            CODEX_METADATA_MAX_ID_BYTES,
            state,
        ),
        subagent_header: header_string(
            headers,
            "x-openai-subagent",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        originator: header_string(headers, "originator", CODEX_METADATA_MAX_LABEL_BYTES, state),
        beta_features: header_string(
            headers,
            "x-codex-beta-features",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        memgen_request: header_string(
            headers,
            "x-openai-memgen-request",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        responses_lite: header_string(
            headers,
            "x-openai-internal-codex-responses-lite",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        ws_stream_request_start_ms: header_i64(
            headers,
            "x-codex-ws-stream-request-start-ms",
            state,
        ),
        ..CodexMetadataCandidate::default()
    }
}

fn parse_identity_fallback(
    headers: &[(String, String)],
    state: &mut CodexMetadataParseState,
) -> CodexMetadataCandidate {
    let session_dash = header_string(headers, "session-id", CODEX_METADATA_MAX_ID_BYTES, state);
    let session_underscore =
        header_string(headers, "session_id", CODEX_METADATA_MAX_ID_BYTES, state);
    if session_dash.is_some() && session_underscore.is_some() && session_dash != session_underscore
    {
        state.conflict("sessionID", "session_id");
    }

    let thread_dash = header_string(headers, "thread-id", CODEX_METADATA_MAX_ID_BYTES, state);
    let thread_underscore = header_string(headers, "thread_id", CODEX_METADATA_MAX_ID_BYTES, state);
    let client_request = header_string(
        headers,
        "x-client-request-id",
        CODEX_METADATA_MAX_ID_BYTES,
        state,
    );
    let thread_id = thread_dash
        .clone()
        .or_else(|| thread_underscore.clone())
        .or_else(|| client_request.clone());
    for (source, value) in [
        ("thread_id", thread_underscore.as_ref()),
        ("x-client-request-id", client_request.as_ref()),
    ] {
        if thread_id
            .as_ref()
            .is_some_and(|preferred| value.is_some_and(|fallback| fallback != preferred))
        {
            state.conflict("threadID", source);
        }
    }

    let turn_dash = header_string(headers, "turn-id", CODEX_METADATA_MAX_ID_BYTES, state);
    let turn_underscore = header_string(headers, "turn_id", CODEX_METADATA_MAX_ID_BYTES, state);
    if turn_dash.is_some() && turn_underscore.is_some() && turn_dash != turn_underscore {
        state.conflict("turnID", "turn_id");
    }

    CodexMetadataCandidate {
        session_id: session_dash.or(session_underscore),
        thread_id,
        turn_id: turn_dash.or(turn_underscore),
        ..CodexMetadataCandidate::default()
    }
}

fn merge_candidate(
    target: &mut CodexMetadataCandidate,
    source: CodexMetadataCandidate,
    source_name: &str,
    state: &mut CodexMetadataParseState,
) {
    if source.is_empty() {
        return;
    }
    state.source(source_name);
    macro_rules! merge {
        ($field:ident, $wire:literal) => {
            merge_field(&mut target.$field, source.$field, $wire, source_name, state);
        };
    }
    merge!(installation_id, "installationID");
    merge!(source_installation_id, "sourceInstallationID");
    merge!(session_id, "sessionID");
    merge!(thread_id, "threadID");
    merge!(agent_name, "agentName");
    merge!(turn_id, "turnID");
    merge!(window_id, "windowID");
    merge!(request_kind, "requestKind");
    merge!(forked_from_thread_id, "forkedFromThreadID");
    merge!(parent_thread_id, "parentThreadID");
    merge!(parent_turn_id, "parentTurnID");
    merge!(root_turn_id, "rootTurnID");
    merge!(subagent_header, "subagentHeader");
    merge!(subagent_kind, "subagentKind");
    merge!(thread_source, "threadSource");
    merge!(sandbox, "sandbox");
    merge!(sandbox_mode, "sandboxMode");
    merge!(auto_review_enabled, "autoReviewEnabled");
    merge!(node_repl_auto_review_required, "nodeReplAutoReviewRequired");
    merge!(node_repl_disabled, "nodeReplDisabled");
    merge!(turn_started_at_unix_ms, "turnStartedAtUnixMS");
    merge!(workspaces, "workspaces");
    merge!(source_workspace_paths, "sourceWorkspacePaths");
    merge!(tool_namespaces_info, "toolNamespacesInfo");
    merge!(compaction, "compaction");
    merge!(originator, "originator");
    merge!(beta_features, "betaFeatures");
    merge!(memgen_request, "memgenRequest");
    merge!(responses_lite, "responsesLite");
    merge!(ws_stream_request_start_ms, "wsStreamRequestStartMS");

    for (key, value) in source.extras {
        match target.extras.get(&key) {
            Some(existing) if existing != &value => {
                state.conflict(&format!("extras.{key}"), source_name);
            }
            Some(_) => {}
            None => {
                target.extras.insert(key, value);
            }
        }
    }
}

fn merge_field<T: PartialEq>(
    target: &mut Option<T>,
    source: Option<T>,
    field: &str,
    source_name: &str,
    state: &mut CodexMetadataParseState,
) {
    let Some(source) = source else {
        return;
    };
    match target {
        Some(preferred) if preferred != &source => state.conflict(field, source_name),
        Some(_) => {}
        None => *target = Some(source),
    }
}

fn object_installation_id(
    object: &Map<String, Value>,
    key: &str,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        state.malformed = true;
        return None;
    };
    installation_fingerprint(raw, state)
}

/// Read a bounded source value without applying the display/privacy projection.
/// The caller has already selected a metadata field (never a prompt/body or
/// credential field), and control characters are rejected before persistence.
fn object_source_string(
    object: &Map<String, Value>,
    key: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        state.malformed = true;
        return None;
    };
    source_bounded_nonempty(raw, max_bytes, state)
}

fn object_string(
    object: &Map<String, Value>,
    key: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    let Some(raw) = value.as_str() else {
        state.malformed = true;
        return None;
    };
    bounded_nonempty(raw, max_bytes, state)
}

fn object_bool(
    object: &Map<String, Value>,
    key: &str,
    state: &mut CodexMetadataParseState,
) -> Option<bool> {
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    value.as_bool().or_else(|| {
        state.malformed = true;
        None
    })
}

fn object_i64(
    object: &Map<String, Value>,
    key: &str,
    state: &mut CodexMetadataParseState,
) -> Option<i64> {
    let value = object.get(key)?;
    if value.is_null() {
        return None;
    }
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(raw) = value.as_str() {
        return raw.parse::<i64>().ok().or_else(|| {
            state.malformed = true;
            None
        });
    }
    state.malformed = true;
    None
}

fn parse_workspaces(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<BTreeMap<String, CodexWorkspaceMetadata>> {
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        state.malformed = true;
        return None;
    };
    let mut workspaces = BTreeMap::new();
    for (index, (path, value)) in object.iter().enumerate() {
        if index >= CODEX_METADATA_MAX_WORKSPACES {
            state.truncated = true;
            break;
        }
        let Some(workspace) = value.as_object() else {
            state.malformed = true;
            continue;
        };
        let mut remotes = BTreeMap::new();
        if let Some(value) = workspace.get("associated_remote_urls") {
            if let Some(object) = value.as_object() {
                for (remote_index, (name, value)) in object.iter().enumerate() {
                    if remote_index >= CODEX_METADATA_MAX_REMOTE_URLS {
                        state.truncated = true;
                        break;
                    }
                    let Some(raw_url) = value.as_str() else {
                        state.malformed = true;
                        continue;
                    };
                    let Some(name) = bounded_nonempty(name, CODEX_METADATA_MAX_LABEL_BYTES, state)
                    else {
                        continue;
                    };
                    if let Some(url) = sanitize_remote_url(raw_url, state) {
                        remotes.insert(name, url);
                    }
                }
            } else if !value.is_null() {
                state.malformed = true;
            }
        }
        let metadata = CodexWorkspaceMetadata {
            associated_remote_urls: remotes,
            latest_git_commit_hash: object_string(
                workspace,
                "latest_git_commit_hash",
                CODEX_METADATA_MAX_ID_BYTES,
                state,
            ),
            has_changes: object_bool(workspace, "has_changes", state),
        };
        let key = unique_map_key(
            &workspaces,
            sanitize_workspace_path(path, state),
            CODEX_METADATA_MAX_PATH_BYTES,
            state,
        );
        workspaces.insert(key, metadata);
    }
    Some(workspaces)
}

fn parse_tool_namespaces(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<BTreeMap<String, CodexToolNamespaceMetadata>> {
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        state.malformed = true;
        return None;
    };
    let mut namespaces = BTreeMap::new();
    let mut total_functions = 0usize;
    for (index, (effective_name, value)) in object.iter().enumerate() {
        if index >= CODEX_METADATA_MAX_NAMESPACES {
            state.truncated = true;
            break;
        }
        let Some(namespace) = value.as_object() else {
            state.malformed = true;
            continue;
        };
        let mut functions = BTreeMap::new();
        if let Some(value) = namespace.get("functions") {
            if let Some(object) = value.as_object() {
                for (function_index, (effective_function_name, value)) in object.iter().enumerate()
                {
                    if function_index >= CODEX_METADATA_MAX_FUNCTIONS_PER_NAMESPACE
                        || total_functions >= CODEX_METADATA_MAX_FUNCTIONS_TOTAL
                    {
                        state.truncated = true;
                        break;
                    }
                    let Some(function) = value.as_object() else {
                        state.malformed = true;
                        continue;
                    };
                    let source = function
                        .get("source")
                        .and_then(|value| parse_tool_source(value, state));
                    let metadata = CodexToolFunctionMetadata {
                        name: object_string(
                            function,
                            "name",
                            CODEX_METADATA_MAX_LABEL_BYTES,
                            state,
                        ),
                        direct: object_bool(function, "direct", state),
                        code_mode_name: object_string(
                            function,
                            "code_mode_name",
                            CODEX_METADATA_MAX_LABEL_BYTES,
                            state,
                        ),
                        deferred: object_bool(function, "deferred", state),
                        source,
                    };
                    let Some(key) = bounded_nonempty(
                        effective_function_name,
                        CODEX_METADATA_MAX_LABEL_BYTES,
                        state,
                    ) else {
                        continue;
                    };
                    functions.insert(key, metadata);
                    total_functions += 1;
                }
            } else if !value.is_null() {
                state.malformed = true;
            }
        }
        let metadata = CodexToolNamespaceMetadata {
            name: object_string(namespace, "name", CODEX_METADATA_MAX_LABEL_BYTES, state),
            functions,
        };
        let Some(key) = bounded_nonempty(effective_name, CODEX_METADATA_MAX_LABEL_BYTES, state)
        else {
            continue;
        };
        namespaces.insert(key, metadata);
    }
    Some(namespaces)
}

fn parse_tool_source(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<CodexToolSourceMetadata> {
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        state.malformed = true;
        return None;
    };
    let Some(kind) = object_string(object, "kind", CODEX_METADATA_MAX_LABEL_BYTES, state) else {
        state.malformed = true;
        return None;
    };
    Some(CodexToolSourceMetadata {
        kind,
        server_name: object_string(object, "server_name", CODEX_METADATA_MAX_LABEL_BYTES, state),
    })
}

fn parse_compaction(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<CodexCompactionMetadata> {
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        state.malformed = true;
        return None;
    };
    Some(CodexCompactionMetadata {
        trigger: object_string(object, "trigger", CODEX_METADATA_MAX_LABEL_BYTES, state),
        reason: object_string(object, "reason", CODEX_METADATA_MAX_LABEL_BYTES, state),
        implementation: object_string(
            object,
            "implementation",
            CODEX_METADATA_MAX_LABEL_BYTES,
            state,
        ),
        phase: object_string(object, "phase", CODEX_METADATA_MAX_LABEL_BYTES, state),
        strategy: object_string(object, "strategy", CODEX_METADATA_MAX_LABEL_BYTES, state),
    })
}

fn first_header_value(
    headers: &[(String, String)],
    name: &str,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let mut values = headers
        .iter()
        .filter(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.trim());
    let first = values.next()?.to_string();
    if values.any(|value| value != first) {
        state.conflict(name, "duplicateHeader");
    }
    Some(first)
}

fn header_string(
    headers: &[(String, String)],
    name: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    first_header_value(headers, name, state)
        .and_then(|value| bounded_nonempty(&value, max_bytes, state))
}

fn header_installation_id(
    headers: &[(String, String)],
    name: &str,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    first_header_value(headers, name, state)
        .and_then(|value| installation_fingerprint(&value, state))
}

fn header_source_string(
    headers: &[(String, String)],
    name: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    first_header_value(headers, name, state)
        .and_then(|value| source_bounded_nonempty(&value, max_bytes, state))
}

fn header_i64(
    headers: &[(String, String)],
    name: &str,
    state: &mut CodexMetadataParseState,
) -> Option<i64> {
    let value = first_header_value(headers, name, state)?;
    value.parse::<i64>().ok().or_else(|| {
        state.malformed = true;
        None
    })
}

fn installation_fingerprint(raw: &str, state: &mut CodexMetadataParseState) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.len() > CODEX_METADATA_MAX_ID_BYTES {
        state.truncated = true;
    }
    state.redact("installationID");
    let digest = Sha256::digest(raw.as_bytes());
    let mut fingerprint = String::from("sha256:");
    for byte in digest.iter().take(8) {
        let _ = write!(&mut fingerprint, "{byte:02x}");
    }
    Some(fingerprint)
}

fn source_bounded_nonempty(
    raw: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    bounded_nonempty(raw, max_bytes, state)
}

/// Keep only workspace path keys from Codex's bounded metadata map.  Workspace
/// details (commit/remotes) remain in the compact projection; in particular,
/// remote URLs are never copied as source values because they may contain
/// credentials.
fn source_workspace_paths(
    value: &Value,
    state: &mut CodexMetadataParseState,
) -> Option<Vec<String>> {
    if value.is_null() {
        return None;
    }
    let Some(object) = value.as_object() else {
        state.malformed = true;
        return None;
    };
    let mut paths = Vec::new();
    for (index, path) in object.keys().enumerate() {
        if index >= CODEX_METADATA_MAX_SOURCE_WORKSPACES {
            state.truncated = true;
            break;
        }
        if let Some(path) =
            source_bounded_nonempty(path, CODEX_METADATA_MAX_SOURCE_PATH_BYTES, state)
            && !paths.contains(&path)
        {
            paths.push(path);
        }
    }
    Some(paths)
}

fn bounded_nonempty(
    raw: &str,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if raw.len() <= max_bytes {
        return Some(raw.to_string());
    }
    state.truncated = true;
    let mut end = max_bytes;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    Some(raw[..end].to_string())
}

fn sanitize_workspace_path(raw: &str, state: &mut CodexMetadataParseState) -> String {
    let normalized = raw.replace('\\', "/");
    let parts: Vec<&str> = normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    let display = match parts.as_slice() {
        [] => "workspace".to_string(),
        [only] => (*only).to_string(),
        [parent, child] => format!("{parent}/{child}"),
        _ => format!(".../{}/{}", parts[parts.len() - 2], parts[parts.len() - 1]),
    };
    if display != raw {
        state.redact("workspaces.path");
    }
    bounded_nonempty(&display, CODEX_METADATA_MAX_PATH_BYTES, state)
        .unwrap_or_else(|| "workspace".to_string())
}

fn sanitize_remote_url(raw: &str, state: &mut CodexMetadataParseState) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let cut = [trimmed.find('?'), trimmed.find('#')]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(trimmed.len());
    let without_suffix = &trimmed[..cut];
    let sanitized = if let Some(scheme_end) = without_suffix.find("://") {
        let scheme = &without_suffix[..scheme_end + 3];
        let rest = &without_suffix[scheme_end + 3..];
        let authority_end = rest.find('/').unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let host = authority
            .rsplit_once('@')
            .map_or(authority, |(_, host)| host);
        format!("{scheme}{host}{}", &rest[authority_end..])
    } else if let Some((_, suffix)) = without_suffix.rsplit_once('@') {
        suffix.to_string()
    } else {
        without_suffix.to_string()
    };
    if sanitized != trimmed {
        state.redact("workspaces.remoteURL");
    }
    bounded_nonempty(&sanitized, CODEX_METADATA_MAX_URL_BYTES, state)
}

fn unique_map_key<T>(
    map: &BTreeMap<String, T>,
    base: String,
    max_bytes: usize,
    state: &mut CodexMetadataParseState,
) -> String {
    if !map.contains_key(&base) {
        return base;
    }
    for suffix in 2..=CODEX_METADATA_MAX_WORKSPACES + 1 {
        let marker = format!("#{suffix}");
        let budget = max_bytes.saturating_sub(marker.len());
        let prefix = bounded_nonempty(&base, budget, state).unwrap_or_else(|| "workspace".into());
        let candidate = format!("{prefix}{marker}");
        if !map.contains_key(&candidate) {
            return candidate;
        }
    }
    state.truncated = true;
    format!("workspace#{}", map.len() + 1)
}

fn valid_extra_key(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_alphabetic())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
}

fn is_known_canonical_key(key: &str) -> bool {
    matches!(
        key,
        "installation_id"
            | "session_id"
            | "thread_id"
            | "agent_name"
            | "turn_id"
            | "window_id"
            | "request_kind"
            | "forked_from_thread_id"
            | "parent_thread_id"
            | "parent_turn_id"
            | "root_turn_id"
            | "subagent_kind"
            | "thread_source"
            | "sandbox"
            | "sandbox_mode"
            | "auto_review_enabled"
            | "node_repl_auto_review_required"
            | "node_repl_disabled"
            | "workspaces"
            | "tool_namespaces_info"
            | "turn_started_at_unix_ms"
            | "compaction"
            | "code_mode_tool_names"
    )
}

fn is_sensitive_metadata_key(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization"
            | "x-api-key"
            | "api_key"
            | "cookie"
            | "set-cookie"
            | "x-oai-attestation"
            | "x-codex-routing-hint"
            | "x-codex-turn-state"
            | "traceparent"
            | "tracestate"
            | "ws_request_header_traceparent"
            | "ws_request_header_tracestate"
            | "prompt"
            | "instructions"
            | "input"
            | "output"
            | "body"
    ) || lower.contains("api_key")
        || lower.contains("authorization")
        || lower.contains("cookie")
        || lower.contains("attestation")
        || lower.contains("routing_hint")
        || lower.contains("turn_state")
}

fn record_redacted_client_metadata(
    object: &Map<String, Value>,
    state: &mut CodexMetadataParseState,
) {
    for key in object.keys() {
        if is_sensitive_metadata_key(key) {
            state.redact(key);
        }
    }
}

fn record_redacted_headers(headers: &[(String, String)], state: &mut CodexMetadataParseState) {
    for (name, _) in headers {
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "x-oai-attestation"
                | "x-codex-routing-hint"
                | "x-codex-turn-state"
                | "traceparent"
                | "tracestate"
        ) {
            state.redact(&name.to_ascii_lowercase());
        }
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

/// 一次响应流的脱敏观测摘要。不保存 prompt、响应正文、请求头或鉴权信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ResponseUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
    /// OpenAI Responses/Chat 的 output_tokens_details.reasoning_tokens。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamTrace {
    #[serde(rename = "chunkCount", default, skip_serializing_if = "is_none")]
    pub chunk_count: Option<u64>,
    #[serde(rename = "bytesReceived", default, skip_serializing_if = "is_none")]
    pub bytes_received: Option<u64>,
    #[serde(rename = "maxChunkGapMS", default, skip_serializing_if = "is_none")]
    pub max_chunk_gap_ms: Option<i64>,
    #[serde(rename = "lastChunkAtMS", default, skip_serializing_if = "is_none")]
    pub last_chunk_at_ms: Option<i64>,
    #[serde(rename = "terminalEvent", default, skip_serializing_if = "is_none")]
    pub terminal_event: Option<String>,
    /// 上游协议公开的 token 计数；不含正文或价格推断。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ResponseUsage>,
    /// Anthropic `stop_reason` 或 OpenAI `finish_reason` 的有界值。
    #[serde(rename = "stopReason", default, skip_serializing_if = "is_none")]
    pub stop_reason: Option<String>,
    /// 脱敏的原生 WebSocket 连接摘要。帧正文、关闭 reason、SDP 和凭据永不写入。
    #[serde(rename = "websocketTrace", default, skip_serializing_if = "is_none")]
    pub websocket_trace: Option<WebSocketTrace>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WebSocketTrace {
    /// Upstream handshake status (normally 101; an HTTP rejection is kept as
    /// the received status when the connection never became a relay).
    #[serde(default, skip_serializing_if = "is_none")]
    pub handshake_status: Option<i64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub bytes_sent: Option<u64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub bytes_received: Option<u64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub client_message_count: Option<u64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub upstream_message_count: Option<u64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub close_code: Option<i64>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub attempt_count: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticHeader {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticChunk {
    #[serde(rename = "atMS")]
    pub at_ms: i64,
    pub bytes: u64,
    pub data: String,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAttemptCapture {
    pub id: String,
    #[serde(rename = "endpointID", alias = "endpointId")]
    pub endpoint_id: String,
    pub endpoint_name: String,
    pub protocol: String,
    #[serde(rename = "sourceFormat", default, skip_serializing_if = "is_none")]
    pub source_format: Option<ProviderProtocol>,
    #[serde(rename = "targetFormat", default, skip_serializing_if = "is_none")]
    pub target_format: Option<ProviderProtocol>,
    #[serde(rename = "routeMode", default, skip_serializing_if = "is_none")]
    pub route_mode: Option<RouteMode>,
    #[serde(rename = "pinnedIP", alias = "pinnedIp")]
    pub pinned_ip: Option<String>,
    #[serde(rename = "startedAtMS")]
    pub started_at_ms: i64,
    pub outbound_method: String,
    #[serde(rename = "outboundURL", alias = "outboundUrl")]
    pub outbound_url: String,
    pub outbound_headers: Vec<DiagnosticHeader>,
    pub outbound_body: String,
    pub outbound_body_bytes: u64,
    #[serde(default)]
    pub outbound_body_truncated: bool,
    pub response_status: Option<u16>,
    pub response_headers: Vec<DiagnosticHeader>,
    pub upstream_chunks: Vec<DiagnosticChunk>,
    pub error: Option<String>,
    #[serde(rename = "completedAtMS")]
    pub completed_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticRequestCapture {
    // ID 结尾字段必须显式 rename:详情端点直接序列化这个结构体,而 WebUI/macOS 读的是
    // 大写口径(engine.rs 手写的索引记录也是)。alias 保住旧 diagnostic_capture.json。
    #[serde(rename = "requestID", alias = "requestId")]
    pub request_id: String,
    pub timestamp: f64,
    pub method: String,
    pub path: String,
    pub inbound_headers: Vec<DiagnosticHeader>,
    pub inbound_body: String,
    pub inbound_body_bytes: u64,
    #[serde(default)]
    pub inbound_body_truncated: bool,
    pub client_kind: ClientKind,
    pub request_purpose: RequestPurpose,
    pub client_model: String,
    pub effective_model: String,
    #[serde(rename = "featureRuleID", alias = "featureRuleId")]
    pub feature_rule_id: Option<String>,
    /// 客户端 `X-Sumpter-*` 声明的项目归因(与事件里同一份,已脱敏、有界)。捕获里带上,
    /// 排障时不必再去 inbound_headers 里翻这三个 header。Codex 的结构化 workspace 不复制
    /// 进来:它在 inbound_body 的 client_metadata 里,体积不可控。
    #[serde(rename = "clientDeclared", default, skip_serializing_if = "is_none")]
    pub client_declared: Option<ClientDeclaredMetadata>,
    #[serde(rename = "sourceFormat", default, skip_serializing_if = "is_none")]
    pub source_format: Option<ProviderProtocol>,
    #[serde(rename = "targetFormat", default, skip_serializing_if = "is_none")]
    pub target_format: Option<ProviderProtocol>,
    #[serde(rename = "routeMode", default, skip_serializing_if = "is_none")]
    pub route_mode: Option<RouteMode>,
    pub attempts: Vec<DiagnosticAttemptCapture>,
    pub client_chunks: Vec<DiagnosticChunk>,
    #[serde(rename = "completedAtMS")]
    pub completed_at_ms: Option<i64>,
    pub status_code: Option<i64>,
    pub outcome: Option<RuntimeEventOutcome>,
    pub failure_kind: Option<RuntimeFailureKind>,
    pub failure_detail: Option<String>,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticCaptureSnapshot {
    pub enabled: bool,
    pub started_at: Option<f64>,
    pub max_bytes: usize,
    pub captured_bytes: usize,
    #[serde(default)]
    pub limit_reached: bool,
    #[serde(default, skip_serializing_if = "is_none")]
    pub stop_reason: Option<String>,
    pub records: Vec<DiagnosticRequestCapture>,
}

impl Default for DiagnosticCaptureSnapshot {
    fn default() -> Self {
        Self {
            enabled: false,
            started_at: None,
            max_bytes: 512 * 1024 * 1024,
            captured_bytes: 0,
            limit_reached: false,
            stop_reason: None,
            records: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    /// 入站客户端类型(client/upstream 请求事件都会记录)。None 表示旧 stats.json
    /// 或没有入站客户端的 notify；显式 Unknown 才表示无法识别的 Anthropic 客户端。
    #[serde(rename = "clientKind", default, skip_serializing_if = "is_none")]
    pub client_kind: Option<ClientKind>,
    /// Codex 入站线程/回合/子代理元数据；旧 stats.json 没有时保持 None。
    #[serde(rename = "codexMetadata", default, skip_serializing_if = "is_none")]
    pub codex_metadata: Option<CodexMetadata>,
    /// 客户端自己用 `X-Sumpter-*` header 声明的项目归因；未配置或旧事件为 None。
    /// 可信度低于 `codex_metadata`，归因时只作兜底。
    #[serde(rename = "clientDeclared", default, skip_serializing_if = "is_none")]
    pub client_declared: Option<ClientDeclaredMetadata>,
    #[serde(rename = "clientModel", default, skip_serializing_if = "is_none")]
    pub client_model: Option<String>,
    /// 入站路径确定的真实协议；None 仅表示旧 stats 或尚未完成路由定型。
    #[serde(rename = "sourceFormat", default, skip_serializing_if = "is_none")]
    pub source_format: Option<ProviderProtocol>,
    /// 本次请求实际选择的真实出站协议；绝不会写入 Auto。
    #[serde(rename = "targetFormat", default, skip_serializing_if = "is_none")]
    pub target_format: Option<ProviderProtocol>,
    /// 本次请求走原生适配还是协议桥接。
    #[serde(rename = "routeMode", default, skip_serializing_if = "is_none")]
    pub route_mode: Option<RouteMode>,
    #[serde(rename = "durationMS", default)]
    pub duration_ms: i64,
    /// 路由定型后的**逻辑模型**(分流规则 target.model / 映射前的客户端模型),
    /// 与 `upstream_model`(实际发给上游的模型名,可能是中转站私有别名)分工:
    /// 前者是「这次请求按哪个模型路由」,后者是「线上真正发出去的串」。
    /// None = 来自升级前的 stats.json,或事件在路由定型前就被拒。
    #[serde(rename = "effectiveModel", default, skip_serializing_if = "is_none")]
    pub effective_model: Option<String>,
    #[serde(rename = "endpointID", default, skip_serializing_if = "is_none")]
    pub endpoint_id: Option<String>,
    #[serde(rename = "endpointName", default, skip_serializing_if = "is_none")]
    pub endpoint_name: Option<String>,
    #[serde(default)]
    pub failover: bool,
    #[serde(rename = "featureRuleID", default, skip_serializing_if = "is_none")]
    pub feature_rule_id: Option<String>,
    /// 有界、去 URL/凭据后的技术详情；面向详情页与排障，不作为分类依据。
    #[serde(rename = "failureDetail", default, skip_serializing_if = "is_none")]
    pub failure_detail: Option<String>,
    #[serde(rename = "failureKind", default, skip_serializing_if = "is_none")]
    pub failure_kind: Option<RuntimeFailureKind>,
    #[serde(rename = "failurePhase", default, skip_serializing_if = "is_none")]
    pub failure_phase: Option<RuntimeFailurePhase>,
    /// 大写 UUID 串;upsert 键。
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "is_none")]
    pub message: Option<String>,
    /// 本轮流中实际观察到的工具调用名称；None 表示旧事件或尚未观察到调用。
    #[serde(rename = "toolCalls", default, skip_serializing_if = "is_none")]
    pub tool_calls: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub outcome: Option<RuntimeEventOutcome>,
    #[serde(default, skip_serializing_if = "is_none")]
    pub phase: Option<RuntimeEventPhase>,
    // Pool selection remains an internal routing/configuration concern. Keep
    // accepting the legacy key on read, but never emit it in runtime events;
    // older persisted JSON remains forward-readable without leaking this
    // deprecated presentation field into Admin/diagnostic responses.
    #[serde(rename = "poolID", default, skip_serializing)]
    pub pool_id: Option<String>,
    /// None 仅表示事件来自升级前的 stats.json,或事件本身不是模型请求。
    #[serde(rename = "requestPurpose", default, skip_serializing_if = "is_none")]
    pub request_purpose: Option<RequestPurpose>,
    /// 一次客户端请求及其全部上游尝试共享；与每行唯一的 `id` 分工。
    #[serde(rename = "requestID", default, skip_serializing_if = "is_none")]
    pub request_id: Option<String>,
    /// 入站 HTTP method。只保存规范化后的短 token，不含 header/body。
    #[serde(rename = "requestMethod", default, skip_serializing_if = "is_none")]
    pub request_method: Option<String>,
    /// 入站 URL path；刻意去掉 query，避免把临时 token 或用户参数写入统计。
    #[serde(rename = "requestPath", default, skip_serializing_if = "is_none")]
    pub request_path: Option<String>,
    /// 路径与 Live/Realtime 身份判定得到的稳定意图标签，例如 `responses`、
    /// `live`、`video`。它描述选路面，不替代 requestPurpose 的业务用途。
    #[serde(rename = "routeIntent", default, skip_serializing_if = "is_none")]
    pub route_intent: Option<String>,
    /// 客户端提供的稳定会话标识（例如 Claude Code session header）。
    /// 只保存有界、无控制字符的标识，不保存会话正文。
    #[serde(rename = "sessionID", default, skip_serializing_if = "is_none")]
    pub session_id: Option<String>,
    /// 0 = 进行中且尚未拿到响应头;收到响应头后进行中事件保留真实 HTTP 状态;
    /// 499 = 客户端取消。
    #[serde(rename = "statusCode", default)]
    pub status_code: i64,
    /// Apple reference date 秒数。
    #[serde(default)]
    pub timestamp: f64,
    /// 首字节耗时(ms):到上游响应头 accepted 为止。
    /// None = 从未 accepted(规划/鉴权失败、全轮耗尽)或来自升级前的 stats.json。
    ///
    /// **两类事件口径不同**:client 事件是客户端视角的总等待(含 failover 与跨轮
    /// 重跑的全部时间);upstream 事件只是该次尝试自身的响应头延迟。
    #[serde(rename = "ttfbMS", default, skip_serializing_if = "is_none")]
    pub ttfb_ms: Option<i64>,
    /// 请求级流诊断；旧事件没有该字段时保持 None。
    #[serde(rename = "streamTrace", default, skip_serializing_if = "is_none")]
    pub stream_trace: Option<StreamTrace>,
    /// 实际生效的超时阈值。response_timeout 是全局与映射级阈值的较小值；
    /// stream_idle_timeout 是响应头之后的块间空闲阈值。
    #[serde(rename = "timeoutMS", default, skip_serializing_if = "is_none")]
    pub timeout_ms: Option<i64>,
    #[serde(rename = "upstreamHost", default, skip_serializing_if = "is_none")]
    pub upstream_host: Option<String>,
    /// **实际发给上游的模型名**(经入口映射,可能是中转站私有别名)。
    /// 逻辑模型见 `effective_model`。
    #[serde(rename = "upstreamModel", default, skip_serializing_if = "is_none")]
    pub upstream_model: Option<String>,
    /// 实际收到的上游 HTTP 状态。None 表示连接/超时发生在响应头之前。
    #[serde(
        rename = "upstreamStatusCode",
        default,
        skip_serializing_if = "is_none"
    )]
    pub upstream_status_code: Option<i64>,
    /// 上游返回的请求追踪 ID（仅采纳安全的响应头白名单）。
    #[serde(rename = "upstreamRequestID", default, skip_serializing_if = "is_none")]
    pub upstream_request_id: Option<String>,
}

impl RuntimeEvent {
    pub fn is_in_flight(&self) -> bool {
        self.phase == Some(RuntimeEventPhase::InFlight)
    }

    /// 新事件按 outcome 判定；旧 stats.json 回退到既有状态码口径。
    pub fn is_succeeded(&self) -> bool {
        if self.is_in_flight() {
            return false;
        }
        self.outcome.map_or_else(
            || (200..=399).contains(&self.status_code),
            |v| v == RuntimeEventOutcome::Succeeded,
        )
    }

    pub fn is_failed(&self) -> bool {
        if self.is_in_flight() {
            return false;
        }
        self.outcome.map_or_else(
            || {
                self.status_code != STATUS_CLIENT_DISCONNECTED
                    && !(200..=399).contains(&self.status_code)
            },
            |v| v == RuntimeEventOutcome::Failed,
        )
    }

    pub fn is_cancelled(&self) -> bool {
        self.outcome
            .map_or(self.status_code == STATUS_CLIENT_DISCONNECTED, |v| {
                v == RuntimeEventOutcome::Cancelled
            })
    }

    /// 展示用:按类型过滤并按时间降序;时间戳相同保持原数组顺序(插入序,新的在前)。
    /// 引擎的 upsert 原地更新不移动行位置,排序统一放展示层。
    pub fn ordered<'a>(
        events: &'a [RuntimeEvent],
        kind_filter: Option<&str>,
    ) -> Vec<&'a RuntimeEvent> {
        let mut filtered: Vec<&RuntimeEvent> = events
            .iter()
            .filter(|e| kind_filter.is_none_or(|k| e.kind == k))
            .collect();
        // sort_by 是稳定排序:同 timestamp 保持原序。
        filtered.sort_by(|a, b| {
            b.timestamp
                .partial_cmp(&a.timestamp)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        filtered
    }

    /// 按事件类型各留最近 `per_kind_limit` 条(入参新→旧,返回保持原顺序)。
    /// in-flight 事件优先保留(还要被原地更新),但单独计数、同样封顶——
    /// 否则永不收尾的僵尸行会无限堆积。
    pub fn trimmed(events: Vec<RuntimeEvent>, per_kind_limit: usize) -> Vec<RuntimeEvent> {
        if events.len() <= per_kind_limit {
            return events;
        }
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut in_flight_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        events
            .into_iter()
            .filter(|event| {
                let bucket = if event.is_in_flight() {
                    &mut in_flight_counts
                } else {
                    &mut counts
                };
                let count = bucket.entry(event.kind.clone()).or_insert(0);
                if *count < per_kind_limit {
                    *count += 1;
                    true
                } else {
                    false
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    #[serde(rename = "clientFailures", default)]
    pub client_failures: i64,
    #[serde(rename = "clientRequests", default)]
    pub client_requests: i64,
    #[serde(rename = "clientSuccesses", default)]
    pub client_successes: i64,
    #[serde(default)]
    pub failovers: i64,
    #[serde(rename = "recentEvents", default)]
    pub recent_events: Vec<RuntimeEvent>,
    #[serde(rename = "upstreamAttempts", default)]
    pub upstream_attempts: i64,
    #[serde(rename = "upstreamFailures", default)]
    pub upstream_failures: i64,
    #[serde(rename = "upstreamSuccesses", default)]
    pub upstream_successes: i64,
}

impl RuntimeSnapshot {
    /// 插入或原地更新事件:同 id 原地覆盖(数组位置不动,保住 in-flight→完成的行位置);
    /// 新事件插到最前,并按 per-kind 上限裁剪。**计数不在这里**——in-flight 插入不计数,
    /// 完成时由引擎按口径计数一次。
    pub fn upsert_event(&mut self, event: RuntimeEvent) {
        if let Some(existing) = self.recent_events.iter_mut().find(|e| e.id == event.id) {
            *existing = event;
            return;
        }
        self.recent_events.insert(0, event);
        let events = std::mem::take(&mut self.recent_events);
        self.recent_events = RuntimeEvent::trimmed(events, MAX_RECENT_EVENTS_PER_KIND);
    }

    /// 供旧 stats.json 离线兼容读取使用：丢弃全部 in-flight 残迹。进程重启后这些请求已不可能继续，
    /// 但仅凭已经收到的 HTTP 状态无法推断最终结果；保留并改成 completed 会把
    /// `HTTP 200 + outcome=None` 误报为成功。
    pub fn normalize_loaded(&mut self) {
        self.recent_events.retain(|event| !event.is_in_flight());
    }

    pub fn from_json(data: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(data)
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 诊断捕获**详情**是 Admin API 直接序列化这个结构体的结果(索引记录反而是
    /// `engine.rs` 手写的 `json!`,那边一直是 `requestID`/`featureRuleID` 大写口径)。
    /// 一旦 ID 结尾字段落到 serde 的 camelCase 规则上就会变成 `requestId`/`endpointId`,
    /// 而 WebUI 读 `record.requestID`、`attempt.endpointID`/`attempt.outboundURL`,
    /// macOS 侧那三个字段还是非可选的——整条详情会直接解码失败。
    /// 旧 `diagnostic_capture.json` 写的是小写形状,alias 必须保住它能读回来,
    /// 否则启动恢复失败会让 daemon 本次运行拒写原文件。
    #[test]
    fn diagnostic_capture_detail_wire_keeps_uppercase_id_keys() {
        let legacy = serde_json::json!({
            "requestId": "REQ-1",
            "timestamp": 1.0,
            "method": "POST",
            "path": "/v1/messages",
            "inboundHeaders": [],
            "inboundBody": "{}",
            "inboundBodyBytes": 2,
            "clientKind": "claude_code",
            "requestPurpose": "standard",
            "clientModel": "claude-opus-5",
            "effectiveModel": "claude-opus-5",
            // 旧捕获文件里的池字段:字段已删,反序列化必须当未知字段忽略而不是报错。
            "poolId": "primary",
            "featureRuleId": "rule-1",
            "attempts": [{
                "id": "A-1",
                "endpointId": "ep-1",
                "endpointName": "up",
                "protocol": "anthropic",
                "pinnedIp": "203.0.113.7",
                "startedAtMS": 1,
                "outboundMethod": "POST",
                "outboundUrl": "https://up.invalid/v1/messages",
                "outboundHeaders": [],
                "outboundBody": "{}",
                "outboundBodyBytes": 2,
                "responseStatus": 200,
                "responseHeaders": [],
                "upstreamChunks": [],
                "error": null,
                "completedAtMS": 5,
            }],
            "clientChunks": [],
            "completedAtMS": 6,
            "statusCode": 200,
            "outcome": "succeeded",
            "failureKind": null,
            "failureDetail": null,
        });
        let record: DiagnosticRequestCapture =
            serde_json::from_value(legacy).expect("旧小写形状必须仍能读回");
        let wire = serde_json::to_value(&record).expect("序列化");

        assert_eq!(wire["requestID"], "REQ-1");
        assert_eq!(wire["featureRuleID"], "rule-1");
        // 池概念只剩事件 wire 与路由内部;捕获记录里那个恒为 primary 的展示残留已摘掉。
        for pool in ["poolID", "poolId"] {
            assert!(wire.get(pool).is_none(), "捕获详情残留池字段 {pool}");
        }
        assert_eq!(wire["attempts"][0]["endpointID"], "ep-1");
        assert_eq!(wire["attempts"][0]["pinnedIP"], "203.0.113.7");
        assert_eq!(
            wire["attempts"][0]["outboundURL"],
            "https://up.invalid/v1/messages"
        );
        for stale in ["requestId", "featureRuleId"] {
            assert!(wire.get(stale).is_none(), "详情 wire 残留小写键 {stale}");
        }
        for stale in ["endpointId", "outboundUrl", "pinnedIp"] {
            assert!(
                wire["attempts"][0].get(stale).is_none(),
                "尝试 wire 残留小写键 {stale}"
            );
        }
    }

    fn event(id: &str, kind: &str, ts: f64) -> RuntimeEvent {
        RuntimeEvent {
            client_kind: None,
            codex_metadata: None,
            client_declared: None,
            client_model: None,
            source_format: None,
            target_format: None,
            route_mode: None,
            duration_ms: 0,
            effective_model: None,
            endpoint_id: None,
            endpoint_name: None,
            failover: false,
            feature_rule_id: None,
            failure_detail: None,
            failure_kind: None,
            failure_phase: None,
            id: id.into(),
            kind: kind.into(),
            message: None,
            tool_calls: None,
            outcome: None,
            phase: None,
            pool_id: None,
            request_purpose: None,
            request_id: None,
            request_method: None,
            request_path: None,
            route_intent: None,
            session_id: None,
            status_code: 200,
            timestamp: ts,
            ttfb_ms: None,
            stream_trace: None,
            timeout_ms: None,
            upstream_host: None,
            upstream_model: None,
            upstream_request_id: None,
            upstream_status_code: None,
        }
    }

    fn in_flight(id: &str, kind: &str, ts: f64) -> RuntimeEvent {
        RuntimeEvent {
            phase: Some(RuntimeEventPhase::InFlight),
            status_code: 0,
            ..event(id, kind, ts)
        }
    }

    #[test]
    fn upsert_updates_in_place_without_moving() {
        let mut snapshot = RuntimeSnapshot::default();
        snapshot.upsert_event(event("a", KIND_CLIENT, 1.0));
        snapshot.upsert_event(in_flight("b", KIND_UPSTREAM, 2.0));
        snapshot.upsert_event(event("c", KIND_CLIENT, 3.0));
        assert_eq!(snapshot.recent_events[1].id, "b");

        // 原地更新 b:位置不动、内容替换。
        snapshot.upsert_event(RuntimeEvent {
            phase: None,
            status_code: 200,
            timestamp: 9.0,
            ..event("b", KIND_UPSTREAM, 9.0)
        });
        assert_eq!(snapshot.recent_events.len(), 3);
        assert_eq!(snapshot.recent_events[1].id, "b");
        assert_eq!(snapshot.recent_events[1].status_code, 200);
        assert!(!snapshot.recent_events[1].is_in_flight());
    }

    #[test]
    fn trim_keeps_per_kind_quota_and_in_flight_extra() {
        let mut events = Vec::new();
        // 新→旧:300 条 upstream、3 条 client、2 条 in-flight upstream。
        for i in 0..2 {
            events.push(in_flight(
                &format!("f{i}"),
                KIND_UPSTREAM,
                1000.0 - i as f64,
            ));
        }
        for i in 0..300 {
            events.push(event(&format!("u{i}"), KIND_UPSTREAM, 900.0 - i as f64));
        }
        for i in 0..3 {
            events.push(event(&format!("c{i}"), KIND_CLIENT, 500.0 - i as f64));
        }
        let trimmed = RuntimeEvent::trimmed(events, 200);
        let upstream_completed = trimmed
            .iter()
            .filter(|e| e.kind == KIND_UPSTREAM && !e.is_in_flight())
            .count();
        let upstream_in_flight = trimmed
            .iter()
            .filter(|e| e.kind == KIND_UPSTREAM && e.is_in_flight())
            .count();
        let clients = trimmed.iter().filter(|e| e.kind == KIND_CLIENT).count();
        assert_eq!(upstream_completed, 200); // 完成配额
        assert_eq!(upstream_in_flight, 2); // in-flight 豁免且独立配额
        assert_eq!(clients, 3); // client 不被 upstream 挤掉
        // 保留的是最新的(数组前端)。
        assert!(trimmed.iter().any(|e| e.id == "u0"));
        assert!(!trimmed.iter().any(|e| e.id == "u299"));
    }

    #[test]
    fn ordered_filters_and_sorts_stable() {
        let events = vec![
            event("a", KIND_CLIENT, 5.0),
            event("b", KIND_UPSTREAM, 7.0),
            event("c", KIND_CLIENT, 7.0),
            event("d", KIND_CLIENT, 6.0),
        ];
        let ordered = RuntimeEvent::ordered(&events, Some(KIND_CLIENT));
        let ids: Vec<&str> = ordered.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "d", "a"]);
        let all = RuntimeEvent::ordered(&events, None);
        let ids: Vec<&str> = all.iter().map(|e| e.id.as_str()).collect();
        // 同戳 7.0:保持原数组顺序 b 在 c 前。
        assert_eq!(ids, vec!["b", "c", "d", "a"]);
    }

    #[test]
    fn normalize_loaded_drops_all_stale_in_flight_events() {
        let mut snapshot = RuntimeSnapshot {
            recent_events: vec![
                in_flight("dead", KIND_CLIENT, 1.0),
                RuntimeEvent {
                    status_code: 200,
                    ..in_flight("accepted-client", KIND_CLIENT, 2.0)
                },
                RuntimeEvent {
                    status_code: 503,
                    ..in_flight("accepted-upstream", KIND_UPSTREAM, 2.5)
                },
                event("done", KIND_CLIENT, 3.0),
            ],
            ..Default::default()
        };
        snapshot.normalize_loaded();
        assert_eq!(snapshot.recent_events.len(), 1);
        assert_eq!(snapshot.recent_events[0].id, "done");
        assert!(!snapshot.recent_events[0].is_in_flight());
    }

    #[test]
    fn stats_json_roundtrip_matches_swift_shape() {
        // 真实 stats.json 的事件片段(Swift 写出的形状)。
        let source = r#"{
  "clientFailures" : 7,
  "clientRequests" : 76,
  "clientSuccesses" : 68,
  "failovers" : 0,
  "recentEvents" : [
    {
      "durationMS" : 9581,
      "endpointID" : "main-a",
      "endpointName" : "A",
      "failover" : false,
      "id" : "EED36CA5-7D07-4F67-8307-F0E810E6730F",
      "kind" : "upstream",
      "phase" : "inFlight",
      "poolID" : "primary",
      "requestPurpose" : "standard",
      "statusCode" : 200,
      "timestamp" : 807287975.215088,
      "ttfbMS" : 812,
      "upstreamHost" : "example.com",
      "upstreamModel" : "claude-fable-5"
    }
  ],
  "upstreamAttempts" : 80,
  "upstreamFailures" : 9,
  "upstreamSuccesses" : 71
}"#;
        let snapshot = RuntimeSnapshot::from_json(source).expect("decode swift stats");
        assert_eq!(snapshot.client_requests, 76);
        let ev = &snapshot.recent_events[0];
        assert_eq!(ev.duration_ms, 9581);
        // 首字节远早于流结束:这正是 ttfbMS 存在的意义(单看 durationMS 分不清
        // 「上游卡住」和「正常长输出」)。
        assert_eq!(ev.ttfb_ms, Some(812));
        assert!(ev.is_in_flight());
        assert_eq!(
            ev.request_purpose,
            Some(crate::routing::RequestPurpose::Standard)
        );
        assert!((apple_to_unix_epoch(ev.timestamp) - 1_785_595_175.215_088).abs() < 1.0);

        // 重编码后的值树保持兼容形状。旧文件里的 poolID 仍可读，但它是
        // 已废弃的展示字段，新 runtime JSON 不再重新写出。
        let reencoded = snapshot.to_json_pretty().unwrap();
        let original: serde_json::Value = serde_json::from_str(source).unwrap();
        let ours: serde_json::Value = serde_json::from_str(&reencoded).unwrap();
        assert_eq!(original["recentEvents"][0]["poolID"], "primary");
        assert!(ours["recentEvents"][0].get("poolID").is_none());
        let mut expected = original;
        expected["recentEvents"][0]
            .as_object_mut()
            .expect("legacy event object")
            .remove("poolID");
        assert_eq!(expected, ours);

        // 新事件即使内部仍保留旧路由值，也不得把它泄漏到公开 wire。
        let mut current = event("new-event", KIND_CLIENT, 2.0);
        current.pool_id = Some("primary".into());
        let current_json = serde_json::to_value(&current).unwrap();
        assert!(current_json.get("poolID").is_none());

        // 缺 requestPurpose / phase / ttfbMS 的旧事件正常往返,且不凭空产出新键
        // ——老版 app 写出的 stats.json 必须原样读回,否则回退一次就丢数据。
        let legacy = r#"{"recentEvents":[{"durationMS":5,"failover":false,"id":"X","kind":"client","statusCode":200,"timestamp":1.5}]}"#;
        let snapshot = RuntimeSnapshot::from_json(legacy).unwrap();
        assert_eq!(snapshot.recent_events[0].request_purpose, None);
        assert_eq!(snapshot.recent_events[0].outcome, None);
        assert_eq!(snapshot.recent_events[0].failure_kind, None);
        assert_eq!(snapshot.recent_events[0].request_id, None);
        assert_eq!(snapshot.recent_events[0].phase, None);
        assert_eq!(snapshot.recent_events[0].ttfb_ms, None);
        assert_eq!(snapshot.recent_events[0].effective_model, None);
        assert_eq!(snapshot.recent_events[0].client_kind, None);
        assert!(!snapshot.recent_events[0].is_in_flight());
        assert!(!snapshot.to_json_pretty().unwrap().contains("ttfbMS"));
        assert!(
            !snapshot
                .to_json_pretty()
                .unwrap()
                .contains("effectiveModel"),
            "老 stats.json 不得凭空长出 effectiveModel 键"
        );
        assert!(
            !snapshot.to_json_pretty().unwrap().contains("clientKind"),
            "老 stats.json 不得凭空长出 clientKind 键"
        );
    }

    #[test]
    fn client_kind_detect_prefers_user_agent_over_dialect() {
        // CC 的 UA 产品名稳定,版本号随升级变化,只认前缀。
        assert_eq!(
            ClientKind::detect(Some("claude-cli/2.1.220 (external, cli)"), false),
            ClientKind::ClaudeCode
        );
        // 即使经兼容层入站,UA 说是 CC 就是 CC。
        assert_eq!(
            ClientKind::detect(Some("claude-cli/1.0.0"), true),
            ClientKind::ClaudeCode
        );
        // Codex 的 UA 形态多变(codex_cli_rs / Codex.app 等),大小写不敏感地认子串。
        assert_eq!(
            ClientKind::detect(Some("codex_cli_rs/0.5.0"), true),
            ClientKind::Codex
        );
        assert_eq!(
            ClientKind::detect(Some("Codex/151.0 Desktop"), false),
            ClientKind::Codex
        );
        // Grok Build 对外仍使用 grok-shell 产品名；pager 形态也会保留该段。
        assert_eq!(
            ClientKind::detect(Some("grok-shell/0.2.119 (macos; aarch64)"), true),
            ClientKind::GrokBuild
        );
        assert_eq!(
            ClientKind::detect(
                Some("grok-pager/0.2.119 grok-shell/0.2.119 (macos; aarch64)"),
                true
            ),
            ClientKind::GrokBuild
        );
        // 未知 UA:按入站方言兜底,缺 UA 同理。
        assert_eq!(
            ClientKind::detect(Some("curl/8.7.1"), true),
            ClientKind::OpenaiCompat
        );
        assert_eq!(
            ClientKind::detect(Some("curl/8.7.1"), false),
            ClientKind::Unknown
        );
        assert_eq!(ClientKind::detect(None, true), ClientKind::OpenaiCompat);
        assert_eq!(ClientKind::detect(None, false), ClientKind::Unknown);
    }

    #[test]
    fn client_kind_detects_codex_originator_when_user_agent_is_generic_or_missing() {
        assert_eq!(
            ClientKind::detect_with_originator(Some("Mozilla/5.0"), Some("Codex Desktop"), true,),
            ClientKind::Codex
        );
        assert_eq!(
            ClientKind::detect_with_originator(None, Some("Codex Desktop/1.2"), true),
            ClientKind::Codex
        );
        assert_eq!(
            ClientKind::detect_with_originator(Some("codex_cli_rs/1.0"), Some("other"), true,),
            ClientKind::Codex
        );
        // UA remains authoritative when it identifies another first-party
        // client; an untrusted Originator must not override it.
        assert_eq!(
            ClientKind::detect_with_originator(
                Some("claude-cli/2.1.0"),
                Some("Codex Desktop"),
                true,
            ),
            ClientKind::ClaudeCode
        );
    }

    #[test]
    fn client_kind_wire_values_are_stable() {
        // 字符串值是 stats/admin API 契约,Swift 侧按同名解码。
        for (kind, wire) in [
            (ClientKind::ClaudeCode, "claude_code"),
            (ClientKind::Codex, "codex"),
            (ClientKind::GrokBuild, "grok_build"),
            (ClientKind::OpenaiCompat, "openai_compat"),
            (ClientKind::Unknown, "unknown"),
        ] {
            assert_eq!(kind.as_str(), wire);
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(wire));
            assert_eq!(
                serde_json::from_value::<ClientKind>(serde_json::json!(wire)).unwrap(),
                kind
            );
        }
    }

    #[test]
    fn explicit_unknown_client_is_distinct_from_legacy_missing_client_kind() {
        let legacy = event("legacy", KIND_CLIENT, 1.0);
        let legacy_json = serde_json::to_value(&legacy).unwrap();
        assert_eq!(legacy.client_kind, None);
        assert!(legacy_json.get("clientKind").is_none());

        let unknown = RuntimeEvent {
            client_kind: Some(ClientKind::Unknown),
            ..event("unknown", KIND_CLIENT, 2.0)
        };
        assert_eq!(
            serde_json::to_value(&unknown).unwrap()["clientKind"],
            serde_json::json!("unknown")
        );
    }

    #[test]
    fn structured_failure_roundtrips_without_changing_legacy_inference() {
        assert_eq!(
            RuntimeFailureKind::ClientRequestRejected.as_str(),
            "client_request_rejected"
        );
        assert_eq!(
            serde_json::to_value(RuntimeFailureKind::ClientRequestRejected).unwrap(),
            serde_json::json!("client_request_rejected")
        );

        let mut current = event("REQUEST-1", KIND_CLIENT, 2.0);
        current.status_code = 200;
        current.outcome = Some(RuntimeEventOutcome::Failed);
        current.failure_kind = Some(RuntimeFailureKind::StreamInterrupted);
        current.failure_phase = Some(RuntimeFailurePhase::ResponseStream);
        current.failure_detail = Some("connection reset by peer".into());
        current.request_id = Some("REQUEST-1".into());
        current.upstream_status_code = Some(200);
        assert!(current.is_failed());
        assert!(!current.is_succeeded());

        let encoded = serde_json::to_string(&current).unwrap();
        let decoded: RuntimeEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, current);

        let legacy_success = event("legacy-ok", KIND_CLIENT, 1.0);
        let mut legacy_cancelled = event("legacy-cancel", KIND_CLIENT, 1.0);
        legacy_cancelled.status_code = STATUS_CLIENT_DISCONNECTED;
        assert!(legacy_success.is_succeeded());
        assert!(legacy_cancelled.is_cancelled());
        assert!(!legacy_cancelled.is_failed());

        let mut in_flight_with_headers = event("streaming", KIND_CLIENT, 3.0);
        in_flight_with_headers.status_code = 200;
        in_flight_with_headers.phase = Some(RuntimeEventPhase::InFlight);
        assert!(!in_flight_with_headers.is_succeeded());
        assert!(!in_flight_with_headers.is_failed());
    }

    #[test]
    fn stream_trace_roundtrips_with_camel_case_wire_names() {
        let mut current = event("trace", KIND_CLIENT, 1.0);
        current.stream_trace = Some(StreamTrace {
            chunk_count: Some(3),
            bytes_received: Some(128),
            max_chunk_gap_ms: Some(60_000),
            last_chunk_at_ms: Some(60_100),
            terminal_event: None,
            usage: Some(ResponseUsage {
                input_tokens: Some(10),
                output_tokens: Some(42),
                ..ResponseUsage::default()
            }),
            stop_reason: Some("end_turn".into()),
            websocket_trace: None,
        });
        let encoded = serde_json::to_value(&current).unwrap();
        assert_eq!(encoded["streamTrace"]["chunkCount"], 3);
        assert_eq!(encoded["streamTrace"]["maxChunkGapMS"], 60_000);
        assert_eq!(encoded["streamTrace"]["usage"]["inputTokens"], 10);
        assert_eq!(encoded["streamTrace"]["stopReason"], "end_turn");
        assert!(
            !encoded["streamTrace"]
                .as_object()
                .unwrap()
                .contains_key("terminalEvent")
        );
        assert_eq!(
            serde_json::from_value::<RuntimeEvent>(encoded).unwrap(),
            current
        );
    }

    #[test]
    fn runtime_protocol_observability_roundtrips_and_legacy_events_omit_it() {
        let legacy = event("legacy-protocol", KIND_CLIENT, 1.0);
        let legacy_json = serde_json::to_value(&legacy).unwrap();
        assert!(legacy_json.get("sourceFormat").is_none());
        assert!(legacy_json.get("targetFormat").is_none());
        assert!(legacy_json.get("routeMode").is_none());

        let current = RuntimeEvent {
            source_format: Some(ProviderProtocol::OpenAIResponses),
            target_format: Some(ProviderProtocol::Anthropic),
            route_mode: Some(RouteMode::Translated),
            ..event("protocol", KIND_UPSTREAM, 2.0)
        };
        let encoded = serde_json::to_value(&current).unwrap();
        assert_eq!(encoded["sourceFormat"], "openai-responses");
        assert_eq!(encoded["targetFormat"], "anthropic");
        assert_eq!(encoded["routeMode"], "translated");
        assert_eq!(
            serde_json::from_value::<RuntimeEvent>(encoded).unwrap(),
            current
        );
    }

    #[test]
    fn client_declared_metadata_sanitizes_paths_and_rejects_unusable_values() {
        let headers = vec![
            ("X-Sumpter-Project".into(), "  automode-proxy  ".into()),
            (
                "x-sumpter-workspace".into(),
                "/Users/kkl/.claude/automode-proxy".into(),
            ),
            (
                "x-sumpter-git-remote".into(),
                "https://user:token@github.com/domoxiaojun/sumpter.git?ref=main".into(),
            ),
        ];
        let declared = ClientDeclaredMetadata::from_headers(&headers).expect("declared");
        // header 名大小写不敏感,值 trim。
        assert_eq!(declared.project.as_deref(), Some("automode-proxy"));
        // 绝对路径只留脱敏尾两段,不暴露完整本机路径。
        assert_eq!(
            declared.workspace.as_deref(),
            Some(".../.claude/automode-proxy")
        );
        // 凭据与 query 都被剥掉。
        assert_eq!(
            declared.git_remote.as_deref(),
            Some("https://github.com/domoxiaojun/sumpter.git")
        );

        // 空值、纯空白、含控制字符一律丢弃该字段。
        let bad = vec![
            ("x-sumpter-project".into(), "   ".into()),
            ("x-sumpter-workspace".into(), "ok\u{7}bell".into()),
            ("x-sumpter-git-remote".into(), String::new()),
        ];
        assert!(ClientDeclaredMetadata::from_headers(&bad).is_none());

        // 三个 header 全缺 -> None,保持未配置用户与旧事件的紧凑 wire 形状。
        assert!(ClientDeclaredMetadata::from_headers(&[]).is_none());

        // 超长 project 被截断而不是整体丢弃。
        let long = vec![("x-sumpter-project".into(), "p".repeat(4096))];
        let truncated = ClientDeclaredMetadata::from_headers(&long).expect("declared");
        assert_eq!(
            truncated.project.as_deref().map(str::len),
            Some(CODEX_METADATA_MAX_LABEL_BYTES)
        );
    }

    #[test]
    fn client_declared_metadata_is_omitted_from_the_wire_when_absent() {
        let mut event = event("E1", KIND_CLIENT, 1.0);
        let json = serde_json::to_value(&event).expect("encode");
        assert!(
            json.get("clientDeclared").is_none(),
            "缺省不落键,旧事件 round-trip 形状不变"
        );

        event.client_declared = Some(ClientDeclaredMetadata {
            project: Some("demo".into()),
            workspace: None,
            git_remote: None,
            source_project: None,
            source_workspace: None,
        });
        let json = serde_json::to_value(&event).expect("encode");
        assert_eq!(
            json["clientDeclared"],
            serde_json::json!({"project": "demo"}),
            "只落非空字段,camelCase 键名"
        );
        let back: RuntimeEvent = serde_json::from_value(json).expect("decode");
        assert_eq!(back.client_declared, event.client_declared);
    }

    #[test]
    fn codex_metadata_parser_covers_sources_precedence_and_safe_boundaries() {
        let headers = vec![
            ("x-codex-installation-id".into(), "install-secret".into()),
            ("x-openai-subagent".into(), "collab_spawn".into()),
            ("x-codex-parent-thread-id".into(), "parent-header".into()),
            ("x-codex-turn-metadata".into(), r#"{"thread_id":"thread-header","agent_name":"/root/header","turn_id":"turn-header","parent_thread_id":"parent-canonical","subagent_kind":"thread_spawn"}"#.into()),
            ("authorization".into(), "Bearer do-not-store".into()),
        ];
        let body = serde_json::json!({
            "client_metadata": {
                "x-codex-turn-metadata": r#"{"thread_id":"thread-body","agent_name":"/root/worker","turn_id":"turn-body","workspaces":{"/Users/kkl/Documents/automode-proxy":{"latest_git_commit_hash":"abc"}}}"#,
                "agent_name":"/root/flat",
                "thread_id":"thread-flat",
                "prompt":"do-not-store"
            }
        });
        let metadata = CodexMetadata::from_request(&headers, Some(&body)).unwrap();
        assert_eq!(metadata.thread_id.as_deref(), Some("thread-body"));
        assert_eq!(metadata.agent_name.as_deref(), Some("/root/worker"));
        assert!(
            metadata
                .conflicts
                .iter()
                .any(|value| value == "agentName:bodyFlat")
        );
        assert!(!metadata.extras.contains_key("agent_name"));
        assert!(
            metadata
                .conflicts
                .iter()
                .any(|value| value == "agentName:headerCanonical")
        );
        assert_eq!(
            metadata.parent_thread_id.as_deref(),
            Some("parent-canonical")
        );
        assert_eq!(metadata.subagent_header.as_deref(), Some("collab_spawn"));
        assert!(metadata.is_subagent);
        assert!(!metadata.malformed);
        assert_ne!(metadata.installation_id.as_deref(), Some("install-secret"));
        assert_eq!(
            metadata.source_installation_id.as_deref(),
            Some("install-secret")
        );
        assert!(metadata.redacted_fields.iter().any(|v| v == "prompt"));
        assert_eq!(
            metadata.source_workspace_paths,
            vec!["/Users/kkl/Documents/automode-proxy".to_string()]
        );
        let encoded = serde_json::to_string(&metadata).unwrap();
        assert!(encoded.contains(r#""agentName":"/root/worker""#));
        assert!(!encoded.contains("do-not-store"));
        assert!(encoded.contains("sourceInstallationID"));

        let malformed =
            CodexMetadata::from_request(&[("x-codex-turn-metadata".into(), "{".into())], None)
                .unwrap();
        assert!(malformed.malformed);
        let oversized = "x".repeat(CODEX_METADATA_MAX_JSON_BYTES + 1);
        let oversized =
            CodexMetadata::from_request(&[("x-codex-turn-metadata".into(), oversized)], None)
                .unwrap();
        assert!(!oversized.malformed);
        assert!(oversized.truncated);

        let long_agent_name = format!("/root/{}", "worker_".repeat(60));
        let bounded = CodexMetadata::from_request(
            &[],
            Some(&serde_json::json!({
                "client_metadata": {"agent_name": long_agent_name}
            })),
        )
        .unwrap();
        assert_eq!(bounded.agent_name.as_deref().unwrap().len(), 256);
        assert!(bounded.truncated);
    }

    #[test]
    fn codex_thread_class_and_attribution_scope_are_independent() {
        let ambient = CodexMetadata {
            thread_source: Some("ambient_suggestion_safety".into()),
            ..Default::default()
        };
        assert_eq!(
            codex_thread_class(Some(&ambient)),
            CodexThreadClass::Ambient
        );
        assert_eq!(
            codex_attribution_scope(Some(&ambient), None),
            CodexAttributionScope::InternalFeature
        );

        let mut attached = ambient.clone();
        attached
            .workspaces
            .insert("/workspace/demo".into(), CodexWorkspaceMetadata::default());
        assert_eq!(
            codex_attribution_scope(Some(&attached), None),
            CodexAttributionScope::Project
        );
        assert_eq!(
            codex_thread_class(Some(&attached)),
            CodexThreadClass::Ambient
        );

        let feature = CodexMetadata {
            thread_source: Some("custom_feature".into()),
            ..Default::default()
        };
        assert_eq!(
            codex_thread_class(Some(&feature)),
            CodexThreadClass::Feature
        );
        assert_eq!(
            codex_attribution_scope(Some(&feature), None),
            CodexAttributionScope::Unknown
        );
    }

    #[test]
    fn codex_metadata_parser_infers_parent_only_for_subagent_and_old_stats_roundtrip() {
        let metadata = CodexMetadata::from_request(
            &[("x-openai-subagent".into(), "collab_spawn".into()), ("x-codex-window-id".into(), "thread-child:7".into())],
            Some(&serde_json::json!({"client_metadata":{"x-codex-turn-metadata":r#"{"forked_from_thread_id":"thread-parent"}"#}})),
        ).unwrap();
        assert_eq!(metadata.parent_thread_id.as_deref(), Some("thread-parent"));
        assert!(metadata.parent_thread_id_inferred);
        let legacy = RuntimeSnapshot::from_json(r#"{"recentEvents":[{"durationMS":1,"failover":false,"id":"old","kind":"client","statusCode":200,"timestamp":1.0}]}"#).unwrap();
        assert!(legacy.recent_events[0].codex_metadata.is_none());
        assert!(!legacy.to_json_pretty().unwrap().contains("codexMetadata"));
    }
}
