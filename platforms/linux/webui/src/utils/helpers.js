// Helper utilities for formatters, models, request chains, and telemetry events
import { routeModeLabel, sourceFormatLabel } from './protocols.js';

export function formatNumber(value) {
  if (value === null || value === undefined || value === '') return '0';
  const num = Number(value);
  if (Number.isNaN(num)) return String(value);
  // Telemetry/request counts stay copyable/searchable: 1000. Token columns use
  // formatTokenCount below so large usage values remain quickly scannable.
  return String(num);
}

// Token 统计需要在大数字中保留可读的千位分隔符；普通请求计数继续使用
// formatNumber，避免改变已有的复制/筛选语义。按字符串分组可避免再次经过
// Number 时把大于安全整数的上游累计值四舍五入。
export function formatTokenCount(value) {
  if (value === null || value === undefined || value === '') return '0';
  const text = String(value).trim();
  const match = text.match(/^(-?)(\d+)(\.\d+)?$/);
  if (!match) return text;
  const grouped = match[2].replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return `${match[1]}${grouped}${match[3] || ''}`;
}

const DERIVED_USAGE_FIELDS = {
  uncachedInputTokens: ['inputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens'],
  processedInputTokens: ['inputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens'],
  processedTotalTokens: ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens'],
  totalTokens: ['inputTokens', 'outputTokens'],
};

// New analytics responses explicitly report how many upstream responses
// contained each usage field. This keeps a real zero distinct from a field
// the upstream never returned. Older daemon responses remain usable by
// falling back to own-property presence.
export function usageFieldPresent(row, field) {
  if (!row || typeof row !== 'object') return false;
  // observedRequests is the number of client rows with an upstream usage
  // object. It has no entry in usageFieldPresence; zero therefore means that
  // this dimension has no usage data rather than a real token value.
  if (field === 'observedRequests') {
    return Number(eventField(row, field)) > 0;
  }
  const presence = row.usageFieldPresence ?? row.usage_field_presence;
  if (presence && typeof presence === 'object' && !Array.isArray(presence)) {
    if (Object.prototype.hasOwnProperty.call(presence, field)) {
      return Number(presence[field]) > 0 || presence[field] === true;
    }
    const snakeField = camelToSnake(field);
    if (Object.prototype.hasOwnProperty.call(presence, snakeField)) {
      return Number(presence[snakeField]) > 0 || presence[snakeField] === true;
    }
    const dependencies = DERIVED_USAGE_FIELDS[field];
    if (dependencies) return dependencies.some((name) => usageFieldPresent(row, name));
  }
  return Object.prototype.hasOwnProperty.call(row, field)
    || Object.prototype.hasOwnProperty.call(row, camelToSnake(field));
}

export function usageDisplayValue(row, field) {
  if (!usageFieldPresent(row, field)) return '—';
  return formatTokenCount(eventField(row, field));
}

// A cache-read value is only meaningful against the protocol-normalized input
// total. OpenAI Chat/Responses report cached input as a subset of inputTokens;
// Anthropic reports cache reads/writes separately and the daemon folds those
// into processedInputTokens. Unknown accounting must stay visibly unknown
// instead of turning a plausible-looking number into a false percentage.
function observedUsageField(row, field) {
  if (!row || typeof row !== 'object') return false;
  const ownField = Object.prototype.hasOwnProperty.call(row, field)
    || Object.prototype.hasOwnProperty.call(row, camelToSnake(field));
  const presence = row.usageFieldPresence ?? row.usage_field_presence;
  if (!presence || typeof presence !== 'object' || Array.isArray(presence)) return ownField;

  const key = Object.prototype.hasOwnProperty.call(presence, field)
    ? field
    : camelToSnake(field);
  if (Object.prototype.hasOwnProperty.call(presence, key)) {
    return Number(presence[key]) > 0 || presence[key] === true;
  }

  // processedInputTokens is a derived wire field, so its presence counter is
  // represented by the raw input/cache counters rather than a separate key.
  if (field === 'processedInputTokens') {
    return ownField && ['inputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens']
      .some((dependency) => observedUsageField(row, dependency));
  }
  // A partial/older presence object may omit a raw field. Preserve the
  // compatibility behavior for that case while treating explicit zero
  // presence as absent above.
  return ownField;
}

function accountingSemantics(row) {
  const raw = eventField(row, 'tokenAccountingSemantics');
  const values = String(raw ?? '')
    .split(/[|,]/)
    .map((value) => value.trim().toLowerCase())
    .filter(Boolean);
  const unique = [...new Set(values)];
  if (!unique.length || unique.includes('unknown')) return 'unknown';
  if (unique.includes('mixed') || unique.length > 1) return 'mixed';
  return unique[0];
}

export function cacheHitRateValue(row) {
  const semantics = accountingSemantics(row);
  if (!['subset', 'independent', 'mixed'].includes(semantics)) return '—';
  if (!observedUsageField(row, 'inputTokens')) return '—';
  if (!observedUsageField(row, 'cacheReadInputTokens')) return '—';
  if (!observedUsageField(row, 'processedInputTokens')) return '—';

  const cacheRead = Number(eventField(row, 'cacheReadInputTokens'));
  const processedInput = Number(eventField(row, 'processedInputTokens'));
  if (!Number.isFinite(cacheRead) || !Number.isFinite(processedInput)
      || cacheRead < 0 || processedInput <= 0) return '—';
  // A malformed/partially migrated snapshot must not render an impossible
  // value above 100%. Keep this defensive bound identical to the macOS
  // presentation helper; valid runtime-store rows are already normalized.
  const ratio = Math.min(100, Math.max(0, (cacheRead / processedInput) * 100));
  return `${ratio.toFixed(1)}%`;
}

export function formatDuration(ms) {
  if (ms === null || ms === undefined || ms === '') return '-';
  const num = Number(ms);
  if (Number.isNaN(num) || num < 0) return '-';
  if (num < 1000) return `${Math.round(num)}ms`;
  if (num < 60_000) return `${(num / 1000).toFixed(1)}s`;
  if (num < 3600_000) {
    const mins = Math.floor(num / 60_000);
    const secs = Math.floor((num % 60_000) / 1000);
    return `${mins}m ${secs}s`;
  }
  const hours = Math.floor(num / 3600_000);
  const mins = Math.floor((num % 3600_000) / 60_000);
  return `${hours}h ${mins}m`;
}

export function formatTimestamp(ts, { date = false } = {}) {
  if (!ts) return '-';
  const time = normalizeTimestampMS(ts);
  if (Number.isNaN(time) || time <= 0) return String(ts);
  const d = new Date(time);
  const pad = (n) => String(n).padStart(2, '0');
  const timePart = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  if (!date) return timePart;
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${timePart}`;
}

// Runtime timestamps are seen in three numeric forms: Apple reference
// seconds (seconds after 2001-01-01), Unix seconds, and Unix milliseconds.
// Convert exactly once before applying the browser's local timezone. Values
// below 1e9 are the current Apple-seconds range; current Unix seconds are
// above it, while milliseconds are several orders larger.
export function normalizeTimestampMS(ts) {
  const numeric = typeof ts === 'number'
    ? ts
    : (typeof ts === 'string' && /^\s*\d+(?:\.\d+)?\s*$/.test(ts) ? Number(ts) : null);
  if (numeric != null) {
    if (!Number.isFinite(numeric)) return Number.NaN;
    if (numeric < 1_000_000_000) return (numeric + 978307200) * 1000;
    if (numeric < 10_000_000_000) return numeric * 1000;
    return numeric;
  }
  return new Date(ts).getTime();
}

export function cleanText(value) {
  return typeof value === 'string' ? value.trim() : (value ? String(value).trim() : '');
}

function camelToSnake(name) {
  return String(name)
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .replace(/([a-z\d])([A-Z])/g, '$1_$2')
    .toLowerCase();
}

// Runtime events are persisted as camelCase, but older stats snapshots and
// third-party callers may still use the Rust field names. Keep the UI read
// path tolerant without changing the wire contract.
export function eventField(event, camelName, snakeName = camelToSnake(camelName)) {
  if (!event) return undefined;
  return event[camelName] ?? event[snakeName];
}

export function eventStatusCode(event) {
  const value = eventField(event, 'statusCode', 'status_code');
  if (value === null || value === undefined || value === '') return null;
  const number = Number(value);
  return Number.isNaN(number) ? value : number;
}

export function eventIsInFlight(event) {
  const phase = cleanText(eventField(event, 'phase'));
  return phase === 'inFlight' || phase === 'in_flight';
}

export function eventOutcome(event) {
  return cleanText(eventField(event, 'outcome')) || null;
}

export function eventPhase(event) {
  const phase = cleanText(eventField(event, 'phase', 'phase'));
  if (phase === 'in_flight') return 'inFlight';
  return phase || null;
}

export function eventFailover(event) {
  const value = eventField(event, 'failover', 'failover');
  if (typeof value === 'string') return value.toLowerCase() === 'true' || value === '1';
  return Boolean(value);
}

export function eventToolCalls(event) {
  const value = eventField(event, 'toolCalls', 'tool_calls');
  return Array.isArray(value) ? value.map(cleanText).filter(Boolean) : parseList(value);
}

export function eventToolCallsLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const calls = eventToolCalls(event);
  if (calls.length) return calls.join('、');
  if (eventIsInFlight(event)) return '尚未观察到工具调用';
  if (!eventPhase(event) && !eventOutcome(event)) return '未记录（旧事件）';
  return '未观察到工具调用';
}

export function eventStreamTrace(event) {
  return eventField(event, 'streamTrace', 'stream_trace') || null;
}

export function eventStreamTraceLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const trace = eventStreamTrace(event);
  if (trace) {
    return formatStreamTrace(trace, {
      durationMS: eventDurationMS(event),
      inFlight: eventIsInFlight(event),
    });
  }
  if (eventIsInFlight(event)) return '等待流诊断信息';
  if (!eventPhase(event) && !eventOutcome(event)) return '未记录（旧事件）';
  return '无流诊断信息（未进入流式阶段）';
}

export function eventEndpointName(event) {
  return cleanText(eventField(event, 'endpointName', 'endpoint_name')) || '-';
}

export function eventEndpointID(event) {
  return cleanText(eventField(event, 'endpointID', 'endpoint_id')) || '-';
}

export function eventUpstreamHost(event) {
  return cleanText(eventField(event, 'upstreamHost', 'upstream_host')) || '-';
}

export function eventUpstreamModel(event) {
  return cleanText(eventField(event, 'upstreamModel', 'upstream_model')) || '-';
}

export function eventFeatureRuleID(event) {
  return cleanText(eventField(event, 'featureRuleID', 'feature_rule_id')) || '-';
}

export function eventRequestID(event) {
  return cleanText(eventField(event, 'requestID', 'request_id')) || '-';
}

export function eventSessionID(event) {
  return cleanText(eventField(event, 'sessionID', 'session_id')) || '-';
}

export function eventUpstreamRequestID(event) {
  return cleanText(eventField(event, 'upstreamRequestID', 'upstream_request_id')) || '-';
}

export function eventMessage(event) {
  return cleanText(eventField(event, 'message')) || '';
}

export function eventCodexMetadata(event) {
  const value = eventField(event, 'codexMetadata', 'codex_metadata');
  return value && typeof value === 'object' ? value : null;
}

export function eventGrokMetadata(event) {
  const value = eventField(event, 'grokMetadata', 'grok_metadata');
  return value && typeof value === 'object' ? value : null;
}

function grokMetadataValue(metadataOrEvent) {
  const isEvent = metadataOrEvent && typeof metadataOrEvent === 'object'
    && (Object.prototype.hasOwnProperty.call(metadataOrEvent, 'id')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'kind')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'statusCode')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'status_code'));
  return isEvent ? eventGrokMetadata(metadataOrEvent) : metadataOrEvent;
}

export function grokMetadataField(metadata, camelName, snakeName = camelName) {
  if (!metadata || typeof metadata !== 'object') return undefined;
  return metadata[camelName] ?? metadata[snakeName] ?? metadata[camelToSnake(camelName)];
}

export function grokMetadataJSON(metadataOrEvent) {
  const metadata = grokMetadataValue(metadataOrEvent);
  return metadata && typeof metadata === 'object' ? JSON.stringify(metadata, null, 2) : '';
}

export function grokMetadataSummary(metadataOrEvent) {
  const metadata = grokMetadataValue(metadataOrEvent);
  if (!metadata || typeof metadata !== 'object') return null;
  const identifier = cleanText(grokMetadataField(metadata, 'clientIdentifier', 'client_identifier'));
  const version = cleanText(grokMetadataField(metadata, 'clientVersion', 'client_version'));
  const mode = cleanText(grokMetadataField(metadata, 'clientMode', 'client_mode'));
  const sessionID = cleanText(grokMetadataField(metadata, 'sessionID', 'session_id'));
  const convID = cleanText(grokMetadataField(metadata, 'convID', 'conv_id'));
  const parts = [];
  if (identifier) parts.push(identifier);
  if (version) parts.push(version);
  if (mode) parts.push(mode);
  if (sessionID) parts.push(`会话 ${sessionID}`);
  if (convID) parts.push(`对话 ${convID}`);
  return parts.join(' · ') || 'Grok 客户端';
}

export function codexHasRequestIdentity(metadata) {
  if (!metadata || typeof metadata !== 'object') return false;
  const workspaces = codexMetadataField(metadata, 'workspaces');
  return Boolean(
    codexMetadataField(metadata, 'installationID', 'installation_id')
    || codexMetadataField(metadata, 'sourceInstallationID', 'source_installation_id')
    || codexMetadataField(metadata, 'sessionID', 'session_id')
    || codexMetadataField(metadata, 'threadID', 'thread_id')
    || codexMetadataField(metadata, 'agentName', 'agent_name')
    || codexMetadataField(metadata, 'turnID', 'turn_id')
    || codexMetadataField(metadata, 'windowID', 'window_id')
    || codexMetadataField(metadata, 'requestKind', 'request_kind')
    || codexMetadataField(metadata, 'forkedFromThreadID', 'forked_from_thread_id')
    || codexMetadataField(metadata, 'parentThreadID', 'parent_thread_id')
    || codexMetadataField(metadata, 'parentTurnID', 'parent_turn_id')
    || codexMetadataField(metadata, 'rootTurnID', 'root_turn_id')
    || codexMetadataField(metadata, 'subagentHeader', 'subagent_header')
    || codexMetadataField(metadata, 'subagentKind', 'subagent_kind')
    || codexMetadataField(metadata, 'threadSource', 'thread_source')
    || codexMetadataField(metadata, 'sandbox')
    || codexMetadataField(metadata, 'sandboxMode', 'sandbox_mode')
    || codexMetadataField(metadata, 'originator')
    || metadata.isSubagent
    || metadata.is_subagent
    || (workspaces && typeof workspaces === 'object' && Object.keys(workspaces).length)
  );
}

function codexMetadataValue(metadataOrEvent) {
  const isEvent = metadataOrEvent && typeof metadataOrEvent === 'object'
    && (Object.prototype.hasOwnProperty.call(metadataOrEvent, 'id')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'kind')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'statusCode')
      || Object.prototype.hasOwnProperty.call(metadataOrEvent, 'status_code'));
  return isEvent ? eventCodexMetadata(metadataOrEvent) : metadataOrEvent;
}

function workspaceProjectName(path, workspace) {
  const pathName = cleanText(path).split(/[\\/]/).filter(Boolean).pop();
  if (pathName) return pathName;

  const remoteValues = workspace && typeof workspace === 'object'
    ? Object.values(workspace.associatedRemoteURLs ?? workspace.associated_remote_urls ?? {})
    : [];
  const remote = cleanText(remoteValues[0]);
  const remotePath = remote
    .replace(/[?#].*$/, '')
    .replace(/\/+$/, '')
    .replace(/\.git$/, '')
    .replace(/^git@[^:]+:/, '');
  const remoteName = remotePath.split('/').filter(Boolean).pop();
  if (remoteName) return remoteName;
  return '未命名工作区';
}

function workspaceRemoteLabel(workspace) {
  if (!workspace || typeof workspace !== 'object') return '';
  const remote = cleanText(Object.values(
    workspace.associatedRemoteURLs ?? workspace.associated_remote_urls ?? {},
  )[0]);
  if (!remote) return '';
  const normalized = remote.replace(/^git@([^:]+):/, '$1/');
  try {
    const url = new URL(normalized.includes('://') ? normalized : `https://${normalized}`);
    return `${url.hostname}${url.pathname.replace(/\/+$/, '').replace(/\.git$/, '')}`;
  } catch {
    return normalized.replace(/^https?:\/\//, '').replace(/\.git$/, '');
  }
}

export function codexWorkspaceEntries(metadataOrEvent) {
  const metadata = codexMetadataValue(metadataOrEvent);
  const workspaces = codexMetadataField(metadata, 'workspaces');
  if (!workspaces || typeof workspaces !== 'object' || Array.isArray(workspaces)) return [];
  return Object.entries(workspaces).map(([path, workspace]) => {
    const value = workspace && typeof workspace === 'object' ? workspace : {};
    const commit = cleanText(value.latestGitCommitHash ?? value.latest_git_commit_hash);
    const hasChanges = value.hasChanges ?? value.has_changes;
    const status = hasChanges === true
      ? '有未提交改动'
      : hasChanges === false
        ? '工作区干净'
        : '状态未记录';
    return {
      path,
      projectName: workspaceProjectName(path, value),
      status,
      commit: commit ? commit.slice(0, 8) : '',
      remote: workspaceRemoteLabel(value),
    };
  });
}

export function codexWorkspaceSummary(metadataOrEvent) {
  const entries = codexWorkspaceEntries(metadataOrEvent);
  if (!entries.length) return '未记录项目 / 工作区';
  return entries.map((entry) => [
    entry.projectName,
    entry.status,
    entry.commit ? `提交 ${entry.commit}` : null,
  ].filter(Boolean).join(' · ')).join('；');
}

// Claude Code 项目归因未配置的判定。与 macOS 侧 SumpterCore/ClaudeAttributionHint
// 保持同一口径:只看分析数据,不去读 shell 配置或 settings.json —— 用户可能用
// direnv / per-project settings 配好了,读那些只会误判。
//
// 成立条件:存在未识别项目行,且确实由 Claude Code 贡献。一旦出现任何
// client_declared 行就闭嘴 —— 说明已经配好了,剩下的未识别行是配置生效前的历史
// 事件,继续提示只是噪音。
export const CC_ATTRIBUTION_HINT = {
  title: 'Claude Code 项目归因未配置',
  message: '这些 Claude Code 请求没有项目归因。Linux 常见是浏览器在本机、daemon 在远程服务器；wrapper 必须装在 Claude Code 实际运行的那台机器，让它随请求带上项目名。会话统计不受影响。',
  command: './cc-project-attribution.sh status\n./cc-project-attribution.sh install',
  hint: '从远程 WebUI 复制后，先 SSH/进入 Claude Code 主机，再在脚本所在目录执行；没有本地脚本时可从 Sumpter listener Base URL 下载。若 CC 与 daemon 不同机，确认 ANTHROPIC_BASE_URL 指向 daemon 的可达地址；装完要新开终端。',
};

// 安全页面的完整引导。与统计页的小提示卡(CC_ATTRIBUTION_HINT)分开:那里是"发现症状后的
// 一句话 + 命令",这里是"从零配完"的全流程。两侧 UI 共用这份文案 —— macOS 侧的等价物在
// SumpterCore/ClaudeAttributionHint.guide,关键串由两边的测试各自钉死。
export const CC_ATTRIBUTION_GUIDE = {
  title: 'Claude Code 项目归因',
  subtitle: '让统计能按项目区分 Claude Code 请求。只影响项目维度，会话维度零配置就有。',
  statusLabels: {
    configured: '已生效',
    unconfigured: '未配置',
    unknown: '暂无法判定',
  },
  statusDetails: {
    configured: '统计里已出现「客户端声明」来源的项目行，归因链路是通的。',
    unconfigured: '有 Claude Code 请求落进「未识别项目」，且没有任何一行来自客户端声明。',
    unknown: '当前时间窗口内没有 Claude Code 流量，或分析数据还没取到 —— 无法判定。',
  },
  why: 'Claude Code 不把工作目录放进请求（cwd / project_dir 只给本机 statusLine 和 hook 用），所以默认所有 CC 请求都堆在「未识别项目」里。要分项目，就让 CC 把项目名随请求带上：Sumpter认 X-Sumpter-Project / X-Sumpter-Workspace / X-Sumpter-Git-Remote 三个入站 header，读完即从出站剥离。会话维度不受影响 —— CC 无条件发 X-Claude-Code-Session-Id。',
  whereToRun: '先分清三台机器：WebUI/浏览器所在设备只负责打开管理页；daemon 所在 Linux 主机负责接收请求和保存统计；Claude Code 所在主机才需要安装 wrapper。配置命令必须在跑 CC 的主机执行，不是打开 WebUI 或运行 daemon 的机器。若 CC 与 daemon 不同机，先 SSH/进入 CC 主机，并确认 ANTHROPIC_BASE_URL 指向 daemon 的可达地址；daemon 只监听 127.0.0.1 时要使用 SSH 隧道或安全内网地址，不要直接暴露无认证监听。每台跑 CC 的机器各配一次。',
  steps: [
    {
      title: '只读体检',
      command: './cc-project-attribution.sh status',
      note: '在 Claude Code 实际运行主机执行；远程 WebUI 场景先 SSH 登录该主机，再 cd 到脚本所在目录。看当前 shell、要改哪个 rc 文件、有没有 settings.json 覆盖陷阱，不改任何文件。',
    },
    {
      title: '安装 wrapper',
      command: './cc-project-attribution.sh install',
      note: '仍在 Claude Code 主机执行，不能装到只运行 daemon 的服务器上。想先预演就加 --dry-run；装前自动给 rc 打时间戳备份，改动是一段带标记的 source 块，可精确移除。',
    },
    {
      title: '新开终端验证',
      command: '',
      note: 'wrapper 是 shell 函数，只对之后启动的 shell 生效。在同一台 CC 主机新开终端，进任意项目发一条消息；回本页看状态变成「已生效」。若 daemon 在另一台机器，不要在 daemon 主机重复安装，只需保证 Base URL 可达。',
    },
  ],
  remoteMachines: [
    { label: 'WebUI / 浏览器', detail: '只负责打开管理页，可以在本地电脑或任意跳板机；不会执行配置，也不决定归因归属。' },
    { label: 'daemon / Linux 服务', detail: '接收 Claude Code 请求并写入统计。它可以是远程服务器或容器；这里通常不安装 CC wrapper。' },
    { label: 'Claude Code 运行主机', detail: '真正启动 claude 进程的机器。wrapper 脚本、shell 环境、项目目录和 Base URL 都以这台机器为准。' },
  ],
  remoteScenarios: [
    { title: 'CC 与 daemon 同机', detail: '在这台 Linux 主机执行 status/install；Claude Code 可使用 http://127.0.0.1:57878 访问 daemon。' },
    { title: '本地电脑调用远程 daemon', detail: '在运行 Claude Code 的本地电脑安装 wrapper，不要只在远程 daemon 服务器安装；若本地没有脚本，从 Sumpter listener Base URL 的 /__sumpter/cc-project-attribution.sh 下载。ANTHROPIC_BASE_URL 指向远程 daemon 的安全可达地址。' },
    { title: '远程开发机调用另一台 daemon', detail: '先 SSH/进入远程开发机，再执行配置器；让该机的 ANTHROPIC_BASE_URL 指向 daemon。若 daemon 仅监听 127.0.0.1，使用 SSH 隧道、反向代理或安全内网，不要裸露无认证端口。' },
  ],
  remoteDownload: {
    command: "SUMPTER_LISTENER_BASE_URL='http://192.168.1.20:57878'\nSUMPTER_LISTENER_BASE_URL=\"\${SUMPTER_LISTENER_BASE_URL%/}\"\ncurl --fail --location \"\$SUMPTER_LISTENER_BASE_URL/__sumpter/cc-project-attribution.sh\" -o /tmp/cc-project-attribution.sh\nbash /tmp/cc-project-attribution.sh install",
    note: 'Base URL 是 Sumpter Linux listener 地址，可以是局域网 host:port 或转发该路径的 Nginx HTTPS 地址，不是发布镜像地址。若 listener.authToken 非空，给 curl 加 Authorization: Bearer；Nginx 对外提供时建议（跨机器时应）开启非空 authToken。脚本只在 Claude Code 客户端本地执行。',
  },
  remoteChecks: [
    { command: 'hostname', note: '确认当前终端确实是 Claude Code 将要运行的主机。' },
    { command: 'whoami', note: '确认 wrapper 会写入启动 Claude Code 的那个用户，而不是 root 或另一账号。' },
    { command: 'pwd', note: '确认当前目录是要归因的项目目录；wrapper 按启动时目录生成项目名。' },
    { command: 'printf \'%s\\n\' "$ANTHROPIC_BASE_URL"', note: '确认 Claude Code 访问的是 daemon 的可达地址；跨机时不要误留 127.0.0.1。' },
  ],
  platformMatrix: [
    { label: '配置器位置', macos: 'App 内 Resources/（源码构建则在 macos/scripts/）', linux: '部署包解包后 scripts/（安装后 /opt/sumpter/scripts/）' },
    { label: '默认 shell', macos: '通常 zsh → ~/.zshrc', linux: '视发行版，zsh 或 bash 都常见' },
    { label: 'bash 用哪个 rc', macos: '~/.bash_profile（登录 shell 不读 .bashrc）', linux: '~/.bashrc' },
    { label: 'CC 与 daemon', macos: '通常同机', linux: 'CC 常在别的机器上连远程 daemon' },
    { label: 'fish', macos: 'fish-snippet 手动粘贴（未实测）', linux: '同左' },
  ],
  pitfalls: [
    {
      title: '值必须是纯 ASCII',
      detail: 'CC 见到含非 ASCII 的 ANTHROPIC_CUSTOM_HEADERS 会直接报错退出 —— 不是归因缺失，是整个会话起不来。中文目录名会让 claude 在该项目完全不可用。配置器已内置 ASCII 守卫，跳过不安全的值而不是硬塞。',
    },
    {
      title: '别写进 settings.json 的 env',
      detail: '那里的值会覆盖进程环境变量，且不做插值（$PWD、${CLAUDE_PROJECT_DIR} 全部字面传出）。一旦写死，shell wrapper 永久失效，且只能固定一个项目名。配置器检出该键会拒绝安装。',
    },
    {
      title: '进程级，启动时读一次',
      detail: '归因的是「启动 CC 时所在的项目」，会话内 cd 不更新。--print / SDK / CI 这些非交互场景不走 shell 函数，需要自行显式设环境变量。',
    },
  ],
  rollback: [
    { command: './cc-project-attribution.sh restore', note: '还原 rc 到装前（取最新备份，并先把当前 rc 另存为 .sumpter-prerestore-*）' },
    { command: './cc-project-attribution.sh uninstall', note: '移除 wrapper，保留备份' },
  ],
  privacy: 'header 会被代理从出站剥离，上游中转站看不到。但同一请求的 body 本来就带工作目录绝对路径、CLAUDE.md 全文和 git status —— 配这三个 header 不增不减外泄面，只决定能否按项目统计。',
};

// 三态判定的内核。安全页面的引导要区分「已配好」「确实没配」「说不清」——统计页那个
// Bool 只需要知道该不该提示,不够用。两者共用同一内核,避免两处口径漂移。
//
// rows: 项目维度行(v3 DimensionRow,只有 name/source/requests,没有 clientKinds);
// clientKindFacets: analytics.facets.clientKinds,用来确认这批流量里确实有 CC。
// 分成两个入参是因为 v3 的项目行不带客户端类型 —— 只看项目行会把 Codex 的未识别
// 行也算成"CC 没配",只看 facets 又不知道未识别的是谁,两者必须合起来判。
export const CC_ATTRIBUTION_STATE = {
  configured: 'configured',
  unconfigured: 'unconfigured',
  unknown: 'unknown',
};

function rowHasClientKind(row, kind) {
  const kinds = Array.isArray(row?.clientKinds) ? row.clientKinds.map(cleanText) : [];
  return kinds.includes(kind);
}

export function ccAttributionState(rows, clientKindFacets) {
  const list = Array.isArray(rows) ? rows : [];
  // 纯项目名声明仍是 client_declared；wrapper 带工作区后升格 workspace_local。
  if (list.some((row) => cleanText(row?.source ?? row?.projectSource) === 'client_declared')) {
    return CC_ATTRIBUTION_STATE.configured;
  }
  if (list.some((row) => (
    cleanText(row?.source ?? row?.projectSource) === 'workspace_local'
      && rowHasClientKind(row, 'claude_code')
  ))) {
    return CC_ATTRIBUTION_STATE.configured;
  }
  const facets = Array.isArray(clientKindFacets) ? clientKindFacets : [];
  const facetsHaveClaudeCode = facets.some(
    (item) => cleanText(item?.value) === 'claude_code' && Number(item?.count ?? 0) > 0,
  );
  const unidentified = list.filter((row) => {
    const name = cleanText(row?.name);
    const source = cleanText(row?.source ?? row?.projectSource);
    const requests = Number(row?.requests ?? row?.attempts ?? 0);
    return requests > 0
      && (name === 'unidentified_project' || source === 'missing_workspace_metadata');
  });
  // 行自带 clientKinds(analytics.projects 的行)时以行为准 —— 比全局 facets 精确:
  // 未归因的那行全是 Codex 时不能说 CC 没配。v3 分页行不带 kinds,才回退看 facets。
  // 这也让两侧口径对齐:macOS 的 ClaudeAttributionHint 一直是按行内 kinds 判的。
  const hasUnattributedClaudeCode = unidentified.some((row) => {
    const kinds = Array.isArray(row?.clientKinds) ? row.clientKinds.map(cleanText) : null;
    return kinds ? kinds.includes('claude_code') : facetsHaveClaudeCode;
  });
  // 没有未归因的 CC 请求 → 说不清(窗口内没有 CC 流量,或都已归因),不能说"没配"。
  return hasUnattributedClaudeCode
    ? CC_ATTRIBUTION_STATE.unconfigured
    : CC_ATTRIBUTION_STATE.unknown;
}

export function shouldPromptCCAttribution(rows, clientKindFacets) {
  return ccAttributionState(rows, clientKindFacets) === CC_ATTRIBUTION_STATE.unconfigured;
}

export function grokAttributionState(rows, clientKindFacets) {
  const list = Array.isArray(rows) ? rows : [];
  if (list.some((row) => (
    (cleanText(row?.source ?? row?.projectSource) === 'client_declared'
      || cleanText(row?.source ?? row?.projectSource) === 'workspace_local')
    && rowHasClientKind(row, 'grok_build')
  ))) {
    return CC_ATTRIBUTION_STATE.configured;
  }
  const facets = Array.isArray(clientKindFacets) ? clientKindFacets : [];
  const facetsHaveGrok = facets.some(
    (item) => cleanText(item?.value) === 'grok_build' && Number(item?.count ?? 0) > 0,
  );
  const unidentified = list.filter((row) => {
    const name = cleanText(row?.name);
    const source = cleanText(row?.source ?? row?.projectSource);
    const requests = Number(row?.requests ?? row?.attempts ?? 0);
    return requests > 0
      && (name === 'unidentified_project' || source === 'missing_workspace_metadata');
  });
  const hasUnattributedGrok = unidentified.some((row) => {
    const kinds = Array.isArray(row?.clientKinds) ? row.clientKinds.map(cleanText) : null;
    return kinds ? kinds.includes('grok_build') : facetsHaveGrok;
  });
  return hasUnattributedGrok
    ? CC_ATTRIBUTION_STATE.unconfigured
    : CC_ATTRIBUTION_STATE.unknown;
}

export const GROK_ATTRIBUTION_GUIDE = {
  title: 'Grok Build 项目归因',
  subtitle: '让统计能按项目区分 Grok Build 请求。只影响项目维度。',
  statusLabels: CC_ATTRIBUTION_GUIDE.statusLabels,
  statusDetails: {
    configured: '统计里已出现 Grok Build 的本地项目行，归因链路是通的。',
    unconfigured: '有 Grok Build 请求落进「未识别项目」，且没有任何一行来自 Grok 的工作区声明。',
    unknown: '当前时间窗口内没有 Grok Build 流量，或分析数据还没取到 —— 无法判定。',
  },
  whereToRun: '配置必须在启动 grok 的那台机器执行，不是只运行 sidecar/daemon 的机器。每台跑 Grok Build 的主机各装一次；装完要新开终端。',
  steps: [
    {
      title: '只读体检',
      command: './grok-project-attribution.sh status',
      note: '在运行 grok 的主机执行。看当前 shell、要改哪个 rc、GROK_CONFIG_PATH 会不会挡住 overlay。',
    },
    {
      title: '安装 wrapper',
      command: './grok-project-attribution.sh install',
      note: '仍在 grok 主机执行。想先预演加 --dry-run。装前自动备份 rc。',
    },
    {
      title: '新开终端验证',
      command: '',
      note: 'wrapper 是 grok() 函数。新开终端后再启动 grok，进项目发一条消息，回本页看状态变成「已生效」。',
    },
  ],
  remoteDownload: {
    command: "SUMPTER_LISTENER_BASE_URL='http://192.168.1.20:57878'\nSUMPTER_LISTENER_BASE_URL=\"${SUMPTER_LISTENER_BASE_URL%/}\"\ncurl --fail --location \"$SUMPTER_LISTENER_BASE_URL/__sumpter/grok-project-attribution.sh\" -o /tmp/grok-project-attribution.sh\nbash /tmp/grok-project-attribution.sh install",
    note: 'Base URL 是 Sumpter Linux listener 地址。脚本只在 Grok Build 客户端本地执行。',
  },
  rollback: [
    { command: './grok-project-attribution.sh restore', note: '还原 rc 到装前' },
    { command: './grok-project-attribution.sh uninstall', note: '移除 wrapper，保留备份' },
  ],
};

// 投影里的合成桶名是机器可读的,展示前翻成中文,与 StatsPage 的 projectLabel 一致。
export function projectLabelText(value) {
  const text = cleanText(value);
  if (text === 'unidentified_project') return '未识别项目';
  if (text === 'multiple_workspaces') return '多工作区（未拆分）';
  return text;
}

export function localUserFromWorkspacePath(path) {
  const parts = cleanText(path).replace(/\\/g, '/').split('/').filter(Boolean);
  for (let index = 0; index < parts.length - 1; index += 1) {
    const home = parts[index];
    const user = parts[index + 1];
    if (!/^(users|home)$/i.test(home)) continue;
    if (/^(shared|public)$/i.test(user)) continue;
    if (user.length > 32) continue;
    if (!/^[A-Za-z0-9._-]+$/.test(user)) continue;
    return user;
  }
  return '';
}

function localUserFromCodexMetadata(metadata) {
  if (!metadata || typeof metadata !== 'object') return '';
  const sourcePaths = metadata.sourceWorkspacePaths ?? metadata.source_workspace_paths ?? [];
  const workspaceKeys = Object.keys(codexMetadataField(metadata, 'workspaces') || {});
  const paths = [...(Array.isArray(sourcePaths) ? sourcePaths : []), ...workspaceKeys];
  for (const path of paths) {
    const user = localUserFromWorkspacePath(path);
    if (user) return user;
  }
  return '';
}

export function projectSourceLabel(source, localUser) {
  const user = cleanText(localUser);
  if (cleanText(source) === 'workspace_local' && user) {
    return `本地(${user})`;
  }
  const labels = {
    workspace_local: '本地项目',
    client_declared: '客户端声明',
    workspace_remote_fallback: '远程仓库回退识别',
    missing_workspace_metadata: '来源未记录',
    multiple_workspaces: '多工作区（未拆分）',
    workspace_unidentified: '来源未识别',
    internal_feature: '后台功能',
    mixed: '混合项目来源',
  };
  return labels[cleanText(source)] || '来源未记录';
}

// 客户端用 X-Sumpter-* 声明的项目归因。daemon 已做脱敏与有界处理，这里只读不再加工；
// 显示名优先用 project，其次工作区尾段，最后 git remote。
export function clientDeclaredProject(event) {
  const declared = event?.clientDeclared ?? event?.client_declared;
  if (!declared || typeof declared !== 'object') return null;
  const project = cleanText(declared.project);
  const workspace = cleanText(declared.workspace);
  const remote = cleanText(declared.gitRemote ?? declared.git_remote);
  const name = project || workspace || remote;
  if (!name) return null;
  const detail = [workspace, remote].filter((value) => value && value !== name);
  return {
    name,
    project,
    workspace,
    remote,
    label: detail.length ? `${name}（${detail.join(' · ')}）` : name,
  };
}

// Project attribution applies only to the inbound client request. Upstream
// retry and notification rows must not be presented as if they independently
// carried a project identity.
export function eventProjectContext(event) {
  const kind = cleanText(event?.kind).toLowerCase();
  if (kind === 'notify') {
    return { applicable: false, name: '', source: '', label: '不适用（通知事件）' };
  }
  if (kind === 'upstream') {
    return { applicable: false, name: '', source: '', label: '不适用（上游尝试，请查看客户端请求）' };
  }
  if (kind && kind !== 'client') {
    return { applicable: false, name: '', source: '', label: '不适用' };
  }

  const attributionScope = cleanText(eventField(event, 'attributionScope', 'attribution_scope'));
  if (attributionScope === 'internal_feature') {
    const thread = eventCodexThreadClass(event);
    return {
      applicable: true,
      name: 'internal_feature',
      source: 'internal_feature',
      label: `后台功能${thread ? ` · ${codexThreadClassLabel(thread)}` : ''}`,
    };
  }

  // 分页列表走服务端投影快路径,不带 codexMetadata / clientDeclared,只带算好的
  // projectName + projectSource。有它们就直接用——优先级已由服务端统一决定;
  // 否则(SSE 推送、单事件详情、旧 daemon)按下面的完整字段自行推导。
  const metadata = eventCodexMetadata(event);
  const projectedName = cleanText(event?.projectName);
  const projectedSource = cleanText(event?.projectSource);
  const projectedLocalUser = cleanText(event?.localUser ?? event?.local_user)
    || localUserFromCodexMetadata(metadata);
  if (projectedName) {
    const sourceLabel = projectSourceLabel(projectedSource, projectedLocalUser);
    const nameLabel = projectLabelText(projectedName);
    const compact = projectedSource === 'workspace_local' && projectedLocalUser
      ? `${nameLabel} 本地(${projectedLocalUser})`
      : `${nameLabel} · ${sourceLabel}`;
    return {
      applicable: true,
      name: projectedName,
      source: projectedSource || 'missing_workspace_metadata',
      localUser: projectedLocalUser,
      label: compact,
    };
  }

  const entries = codexWorkspaceEntries(metadata);
  if (!metadata || !entries.length) {
    const declared = clientDeclaredProject(event);
    if (declared) {
      const user = cleanText(event?.localUser ?? event?.local_user ?? event?.clientDeclared?.user);
      const source = declared.workspace
        ? 'workspace_local'
        : declared.remote
          ? 'workspace_remote_fallback'
          : 'client_declared';
      const sourceLabel = projectSourceLabel(source, user);
      const compact = source === 'workspace_local' && user
        ? `${declared.name} 本地(${user})`
        : `${declared.label} · ${sourceLabel}`;
      return {
        applicable: true,
        name: declared.name,
        source,
        localUser: user,
        label: compact,
      };
    }
    return {
      applicable: true,
      name: 'unidentified_project',
      source: 'missing_workspace_metadata',
      label: '未识别项目 · 来源未记录',
    };
  }
  if (entries.length > 1) {
    return {
      applicable: true,
      name: 'multiple_workspaces',
      source: 'multiple_workspaces',
      label: `${entries.map((entry) => entry.projectName).join(' + ')} · 多工作区（未拆分）`,
    };
  }

  const entry = entries[0];
  const rawWorkspaces = codexMetadataField(metadata, 'workspaces');
  const [path, workspace] = Object.entries(rawWorkspaces || {})[0] || ['', {}];
  const remotes = workspace && typeof workspace === 'object'
    ? Object.values(workspace.associatedRemoteURLs ?? workspace.associated_remote_urls ?? {})
    : [];
  const source = cleanText(path)
    ? 'workspace_local'
    : remotes.some((value) => cleanText(value))
      ? 'workspace_remote_fallback'
      : 'workspace_unidentified';
  const user = cleanText(event?.localUser ?? event?.local_user) || localUserFromCodexMetadata(metadata);
  const sourceLabel = projectSourceLabel(source, user);
  const compact = source === 'workspace_local' && user
    ? `${entry.projectName} 本地(${user})`
    : `${entry.projectName} · ${sourceLabel}`;
  return {
    applicable: true,
    name: entry.projectName,
    source,
    localUser: user,
    label: compact,
  };
}

function codexSubagentKind(metadata) {
  return cleanText(codexMetadataField(metadata, 'subagentKind', 'subagent_kind'));
}

function codexHasSubagentEvidence(metadata) {
  const kind = codexSubagentKind(metadata);
  const header = cleanText(codexMetadataField(metadata, 'subagentHeader', 'subagent_header'));
  const source = cleanText(codexMetadataField(metadata, 'threadSource', 'thread_source')).toLowerCase();
  const flag = codexMetadataField(metadata, 'isSubagent', 'is_subagent');
  const isSubagent = typeof flag === 'string'
    ? ['true', '1', 'yes'].includes(flag.toLowerCase())
    : Boolean(flag);
  return isSubagent || Boolean(kind) || Boolean(header)
    || source === 'subagent' || source === 'memory_consolidation';
}

export function codexAgentPath(metadataOrEvent) {
  const metadata = codexMetadataValue(metadataOrEvent);
  return cleanText(codexMetadataField(metadata, 'agentName', 'agent_name')) || '未记录代理路径';
}

const CODEX_THREAD_CLASS_LABELS = {
  user: '普通用户回合',
  ambient: 'ambient 后台功能',
  system: '系统线程',
  title: '标题生成',
  automation: '自动化',
  automated_review: '自动审查',
  guardian_review: 'Guardian 审查',
  memory_consolidation: '记忆整理',
  subagent: '子代理',
  feature: 'Codex 功能线程',
  unknown: '线程类型未记录',
};

export function eventCodexThreadClass(event) {
  return cleanText(eventField(event, 'codexThreadClass', 'codex_thread_class'))
    || cleanText(codexMetadataField(eventCodexMetadata(event), 'threadSource', 'thread_source'))
    || null;
}

export function codexThreadClassLabel(value) {
  const key = cleanText(value);
  if (!key) return '线程类型未记录';
  if (CODEX_THREAD_CLASS_LABELS[key]) return CODEX_THREAD_CLASS_LABELS[key];
  if (key.startsWith('ambient')) return `ambient 后台功能（${key}）`;
  return key;
}

export function attributionScopeLabel(value) {
  switch (cleanText(value)) {
    case 'project': return '项目请求';
    case 'internal_feature': return '后台功能';
    case 'unknown': return '归因范围未确定';
    default: return '归因范围未记录';
  }
}

export function codexAgentRoleLabel(metadataOrEvent) {
  const metadata = codexMetadataValue(metadataOrEvent);
  if (!metadata || typeof metadata !== 'object') return '未记录代理身份';
  const kind = codexSubagentKind(metadata);
  if (codexHasSubagentEvidence(metadata)) return `子代理${kind ? ` · ${kind}` : ''}`;
  if (codexMetadataField(metadata, 'agentName', 'agent_name')
    || codexMetadataField(metadata, 'threadID', 'thread_id')
    || codexMetadataField(metadata, 'turnID', 'turn_id')) {
    return '主代理';
  }
  return '代理身份未确定';
}

function codexAgentRoleSummary(metadata) {
  if (codexHasSubagentEvidence(metadata)) {
    const kind = codexSubagentKind(metadata);
    return `子代理${kind ? ` ${kind}` : ''}`;
  }
  return '主代理';
}

export function codexMetadataSummary(metadataOrEvent) {
  const metadata = codexMetadataValue(metadataOrEvent);
  if (!metadata || typeof metadata !== 'object') return null;
  const threadID = cleanText(metadata.threadID ?? metadata.thread_id);
  const agentName = cleanText(metadata.agentName ?? metadata.agent_name);
  const turnID = cleanText(metadata.turnID ?? metadata.turn_id);
  const requestKind = cleanText(metadata.requestKind ?? metadata.request_kind);
  const parts = [];
  parts.push(codexAgentRoleSummary(metadata));
  if (agentName) parts.push(`代理路径 ${agentName}`);
  if (requestKind) parts.push(`请求 ${requestKind}`);
  if (threadID) parts.push(`线程 ${threadID}`);
  if (turnID) parts.push(`回合 ${turnID}`);
  return parts.join(' · ') || 'Codex 元数据';
}

export function codexMetadataJSON(metadataOrEvent) {
  const metadata = codexMetadataValue(metadataOrEvent);
  return metadata && typeof metadata === 'object' ? JSON.stringify(metadata, null, 2) : '';
}

export function codexMetadataField(metadata, camelName, snakeName = camelName) {
  if (!metadata || typeof metadata !== 'object') return undefined;
  return metadata[camelName] ?? metadata[snakeName] ?? metadata[camelToSnake(camelName)];
}

export function codexMetadataHas(metadataOrEvent) {
  return Boolean(eventCodexMetadata(metadataOrEvent));
}

export function eventFailureDetail(event) {
  return cleanText(eventField(event, 'failureDetail', 'failure_detail')) || '';
}

export function eventFailureKind(event) {
  return cleanText(eventField(event, 'failureKind', 'failure_kind')) || null;
}

export function eventFailurePhase(event) {
  return cleanText(eventField(event, 'failurePhase', 'failure_phase')) || null;
}

export function eventTimeoutMS(event) {
  return eventField(event, 'timeoutMS', 'timeout_ms');
}

export function eventTTFBMS(event) {
  return eventField(event, 'ttfbMS', 'ttfb_ms');
}

export function eventDurationMS(event) {
  if (!event) return null;
  if (eventIsInFlight(event)) {
    const startedAt = normalizeTimestampMS(eventField(event, 'timestamp'));
    if (Number.isFinite(startedAt) && startedAt > 0) return Math.max(0, Date.now() - startedAt);
  }
  return eventField(event, 'durationMS', 'duration_ms');
}

export function isLoopback(host) {
  const h = cleanText(host).toLowerCase();
  return h === '127.0.0.1' || h === 'localhost' || h === '::1';
}

export function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

export function deepEqual(a, b) {
  return JSON.stringify(a) === JSON.stringify(b);
}

export function parseList(value) {
  if (Array.isArray(value)) return value.map(cleanText).filter(Boolean);
  if (!value) return [];
  return String(value).split(/[,\n]+/).map(cleanText).filter(Boolean);
}

export function eventModel(event) {
  const client = cleanText(eventField(event, 'clientModel', 'client_model'));
  const effective = cleanText(eventField(event, 'effectiveModel', 'effective_model'));
  const parts = [];
  if (client) parts.push(client);
  if (effective && effective !== client) parts.push(effective);
  if (parts.length) return parts.join(' → ');
  return eventUpstreamModel(event);
}

export function eventEndpoint(event) {
  const name = eventEndpointName(event);
  const endpointID = eventEndpointID(event);
  const host = eventUpstreamHost(event);
  const hasName = name !== '-';
  const hasEndpointID = endpointID !== '-';
  const hasHost = host !== '-';
  if (hasName && hasHost) return `${name} @ ${host}${hasEndpointID ? ` (${endpointID})` : ''}`;
  if (hasName && hasEndpointID) return `${name} (${endpointID})`;
  if (hasEndpointID && hasHost) return `${endpointID} @ ${host}`;
  const url = cleanText(event?.baseURL || event?.base_url);
  if (hasName && url) return `${name} (${url})`;
  return hasName ? name : hasEndpointID ? endpointID : hasHost ? host : url || '-';
}

export function purposeLabel(value) {
  const key = typeof value === 'object' ? (value?.kind || value?.type || value?.name) : value;
  const map = {
    normal: '普通请求',
    standard: '主对话',
    session_title: '会话标题生成',
    websearch: 'WebSearch 搜索',
    webSearch: 'WebSearch 搜索',
    webfetch: 'WebFetch 抓取',
    webFetch: 'WebFetch 抓取',
    classifier: '安全分类器',
    image_generation: '图片生成',
    image_edit: '图片编辑',
    alpha_search: 'Codex 独立搜索',
    token_count: 'Token 计数',
    compact: '上下文压缩',
    title: '会话标题生成',
    unknown: '未识别用途',
  };
  return map[key] || cleanText(key) || '-';
}

export function eventPurposeLabel(event) {
  const purpose = eventField(event, 'requestPurpose');
  if (cleanText(purpose)) return purposeLabel(purpose);
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  return '旧事件（未记录）';
}

export function clientKindLabel(kind) {
  const map = {
    claude_code: 'Claude Code',
    codex: 'Codex',
    grok_build: 'Grok Build',
    openai_compat: 'OpenAI 兼容客户端',
    unknown: '未知客户端',
  };
  return map[cleanText(kind)] || (cleanText(kind) ? cleanText(kind) : '未知客户端');
}

export function projectClientKindsLabel(kinds) {
  const values = Array.isArray(kinds)
    ? [...new Set(kinds.map(cleanText).filter(Boolean))]
    : [];
  return values.length
    ? values.map(clientKindLabel).join(' · ')
    : '客户端未记录';
}

// `clientKind=unknown` is an explicit parser result.  A missing field means
// the event predates client attribution (or was rejected before attribution).
// Current upstream attempt events inherit the request's client kind; notify
// events are the only rows where this field is inherently not applicable.
// Keep these states distinct so a large legacy bucket is not presented as an
// active unknown client.
export function eventClientKindState(event) {
  if (!event) return 'missing';
  const kind = eventField(event, 'clientKind');
  const hasField = Object.prototype.hasOwnProperty.call(event, 'clientKind')
    || Object.prototype.hasOwnProperty.call(event, 'client_kind');
  const eventKind = cleanText(event.kind).toLowerCase();
  if (eventKind === 'notify' && !cleanText(kind)) {
    return 'not_applicable';
  }
  return hasField && cleanText(kind) ? 'explicit' : 'missing';
}

export function eventClientKindLabel(event) {
  const state = eventClientKindState(event);
  if (state === 'not_applicable') return '不适用（通知事件）';
  if (state === 'missing') return '旧事件（未记录）';
  return clientKindLabel(eventField(event, 'clientKind'));
}

export function eventResultKind(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return null;
  const outcome = eventOutcome(event);
  if (outcome === 'succeeded' || outcome === 'failed' || outcome === 'cancelled') return outcome;
  // A streaming response can already have HTTP 200 headers while its final
  // protocol outcome is still unknown. Keep it pending until completion.
  if (eventIsInFlight(event)) return null;
  const statusCode = eventHTTPStatusCode(event);
  if (statusCode === 499) return 'cancelled';
  if (statusCode !== null && statusCode >= 200 && statusCode < 400) return 'succeeded';
  if (statusCode !== null && statusCode >= 400) return 'failed';
  return null;
}

export function eventKindLabel(kind) {
  return ({ client: '客户端', upstream: '上游', notify: '通知' })[kind] || String(kind || '-');
}

export function failureKindLabel(kind) {
  const map = {
    response_timeout: '响应超时 (Response Timeout)',
    connection_failed: '连接失败 (Connection Failed)',
    invalid_response: '上游响应格式异常 (Invalid Response)',
    upstream_http_status: '上游返回 HTTP 错误状态',
    stream_idle_timeout: '流式空闲超时 (Stream Idle Timeout)',
    stream_interrupted: '流传输中途断开 (Stream Interrupted)',
    upstream_response_incomplete: '上游响应流未完整结束',
    upstream_response_failed: '上游响应协议失败',
    endpoints_exhausted: '所有可用入口全部耗尽',
    client_cancelled: '客户端主动取消 (Client Cancelled)',
    client_request_rejected: '客户端请求被代理拒绝',
  };
  return map[kind] || cleanText(kind) || '-';
}

export function failurePhaseLabel(phase) {
  const map = {
    before_response: '建立连接 / 请求发送前',
    response_headers: '接收响应头阶段',
    response_stream: '流式传输输出阶段',
  };
  return map[phase] || cleanText(phase) || '-';
}

export function eventDurationText(event) {
  const ttfb = eventTTFBMS(event);
  const duration = eventDurationMS(event);
  if (eventIsInFlight(event)) {
    return ttfb != null ? `TTFB ${formatDuration(ttfb)}` : '等待首字节...';
  }
  if (ttfb != null && duration != null && duration >= ttfb) {
    return `${formatDuration(ttfb)} → ${formatDuration(duration)}`;
  }
  return formatDuration(duration ?? ttfb);
}

export function eventHTTPStatusCode(event) {
  const statusCode = eventStatusCode(event);
  if (statusCode !== null && statusCode !== 0) return statusCode;
  const upstreamStatus = eventField(event, 'upstreamStatusCode', 'upstream_status_code');
  if (upstreamStatus === null || upstreamStatus === undefined || upstreamStatus === '') return statusCode;
  const normalized = Number(upstreamStatus);
  return Number.isNaN(normalized) || normalized === 0 ? statusCode : normalized;
}

export function eventStatus(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const phase = eventPhase(event);
  const outcome = eventOutcome(event);
  const statusCode = eventHTTPStatusCode(event);
  const hasStatus = statusCode !== null && statusCode !== 0;
  const status = hasStatus ? `HTTP ${statusCode}` : 'HTTP 未收到';
  if (phase === 'inFlight' || eventIsInFlight(event)) return `${status} · 传输中`;
  if (outcome === 'failed') return `${status} · 失败`;
  if (outcome === 'cancelled') return `${status} · 已取消`;
  if (outcome === 'succeeded') return `${status} · 成功`;
  if (hasStatus) return status;
  return phase === 'completed' ? '已完成 · 结果未上报' : '已结束';
}

export function eventOutcomeLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const outcome = eventOutcome(event);
  if (eventIsInFlight(event)) return '待定 (Pending)';
  if (outcome === 'succeeded') return '成功 (Succeeded)';
  if (outcome === 'cancelled') return '客户端主动取消 (Cancelled)';
  if (outcome === 'failed') return '请求失败 (Failed)';
  const inferred = eventResultKind(event);
  // A completed event with a missing outcome is a field gap, not necessarily
  // a legacy row. Only rows missing both phase and outcome may use the HTTP
  // status fallback wording; this keeps a current 200/failed protocol result
  // and a completed-but-unreported result visibly distinct.
  const phase = eventPhase(event);
  const resultSuffix = phase === 'completed' ? '（最终结果未上报）' : '（旧事件推断）';
  if (inferred === 'succeeded') return `成功${resultSuffix}`;
  if (inferred === 'cancelled') return `已取消${resultSuffix}`;
  if (inferred === 'failed') return `失败${resultSuffix}`;
  return '未上报 (Unknown)';
}

export function eventPhaseLabel(phaseOrEvent) {
  const isEvent = phaseOrEvent && typeof phaseOrEvent === 'object';
  if (isEvent && cleanText(phaseOrEvent.kind).toLowerCase() === 'notify') {
    return '不适用（通知事件）';
  }
  const phase = isEvent ? eventPhase(phaseOrEvent) : eventPhase({ phase: phaseOrEvent });
  const map = {
    inFlight: '进行中 (In Flight)',
    completed: '已完成 (Completed)',
  };
  if (map[phase]) return map[phase];
  // Newer event snapshots always carry a final outcome, but older engines
  // omitted phase on completion.  Do not call those current events legacy.
  if (isEvent && eventOutcome(phaseOrEvent)) return '已完成 (Completed)';
  return phase || '已结束（旧事件未记录阶段）';
}

export function eventHttpStatusLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const statusCode = eventHTTPStatusCode(event);
  return statusCode === null || statusCode === 0 ? '未收到响应头' : `HTTP ${statusCode}`;
}

// Keep HTTP transport, final outcome, and lifecycle phase as three distinct
// concepts. The result badge renders the outcome; this line renders the other two.
export function eventStatusDetailLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const httpStatus = eventHttpStatusLabel(event);
  return `${httpStatus} · ${eventPhaseLabel(event)}`;
}

export function eventUpstreamStatusLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const explicit = eventField(event, 'upstreamStatusCode', 'upstream_status_code');
  if (explicit !== null && explicit !== undefined && explicit !== '') return `HTTP ${explicit}`;
  const legacyStatus = eventStatusCode(event);
  if (cleanText(event?.kind).toLowerCase() === 'upstream'
    && !eventPhase(event) && !eventOutcome(event) && legacyStatus) {
    return `HTTP ${legacyStatus}（旧事件推断）`;
  }
  return '未收到响应头 / 未记录';
}

export function eventProtocolRouteLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const source = eventField(event, 'sourceFormat', 'source_format');
  const target = eventField(event, 'targetFormat', 'target_format');
  const mode = eventField(event, 'routeMode', 'route_mode');
  if (!source && !target && !mode) {
    if (eventIsInFlight(event)) return '尚未记录（路由未定型）';
    if (!eventPhase(event) && !eventOutcome(event)) return '未记录（旧事件）';
    return '未记录（路由前拒绝）';
  }
  return `${source ? sourceFormatLabel(source) : '未记录'} → ${target ? sourceFormatLabel(target) : '未记录'} · ${mode ? routeModeLabel(mode) : '未记录'}`;
}

export function eventFailureSummaryLabel(event) {
  const kind = eventFailureKind(event);
  const phase = eventFailurePhase(event);
  if (kind) {
    return `${failureKindLabel(kind)}${phase ? ` · ${failurePhaseLabel(phase)}` : ''}`;
  }
  const result = eventResultKind(event);
  if (result === 'failed') return '未记录（旧事件 / 非结构化失败）';
  if (eventIsInFlight(event)) return '尚无失败信息（进行中）';
  if (result === 'cancelled') return '不适用（请求已取消）';
  if (result === 'succeeded') return '不适用（请求成功）';
  return '未记录';
}

export function formatStreamTrace(trace, { durationMS = null, inFlight = false } = {}) {
  if (!trace) return null;
  const parts = [];
  const chunkCount = trace.chunkCount ?? trace.chunk_count;
  const bytesReceived = trace.bytesReceived ?? trace.bytes_received;
  const maxChunkGapMS = trace.maxChunkGapMS ?? trace.max_chunk_gap_ms;
  const lastChunkAtMS = trace.lastChunkAtMS ?? trace.last_chunk_at_ms;
  const terminalEvent = trace.terminalEvent ?? trace.terminal_event;
  const stopReason = trace.stopReason ?? trace.stop_reason;
  const usage = trace.usage;
  if (chunkCount != null) parts.push(`接收 Chunk 数: ${chunkCount}`);
  if (bytesReceived != null) parts.push(`累计字节: ${(bytesReceived / 1024).toFixed(1)} KB`);
  if (maxChunkGapMS != null) parts.push(`最大间隔: ${formatDuration(maxChunkGapMS)}`);
  if (lastChunkAtMS != null) parts.push(`最后 Chunk: ${formatDuration(lastChunkAtMS)}`);
  if (lastChunkAtMS != null && durationMS != null && !inFlight) {
    parts.push(`结束前空闲: ${formatDuration(Math.max(0, durationMS - lastChunkAtMS))}`);
  }
  if (stopReason) parts.push(`停止原因: ${stopReason}`);
  if (usage && typeof usage === 'object') {
    const input = usage.inputTokens ?? usage.input_tokens;
    const output = usage.outputTokens ?? usage.output_tokens;
    const cacheRead = usage.cacheReadInputTokens ?? usage.cache_read_input_tokens;
    const cacheWrite = usage.cacheCreationInputTokens ?? usage.cache_creation_input_tokens;
    const reasoning = usage.reasoningTokens ?? usage.reasoning_tokens;
    const tokens = [
      input != null ? `输入 ${formatTokenCount(input)}` : null,
      output != null ? `输出 ${formatTokenCount(output)}` : null,
      cacheRead != null ? `缓存读 ${formatTokenCount(cacheRead)}` : null,
      cacheWrite != null ? `缓存写 ${formatTokenCount(cacheWrite)}` : null,
      reasoning != null ? `推理 ${formatTokenCount(reasoning)}` : null,
    ].filter(Boolean);
    if (tokens.length) parts.push(`Token: ${tokens.join('、')}`);
  }
  parts.push(terminalEvent ? `终止事件: ${terminalEvent}` : (inFlight ? '等待协议终止' : '未观察到协议终止'));
  return parts.join(' · ');
}

export function friendlyEventMessage(event) {
  if (!event) return '';
  if (cleanText(event.kind).toLowerCase() === 'notify') {
    return eventMessage(event) || '通知事件（不参与请求成败统计）';
  }
  const phase = eventPhase(event);
  const outcome = eventOutcome(event);
  const statusCode = eventHTTPStatusCode(event);
  const failureKind = eventFailureKind(event);
  const message = eventMessage(event);
  const failureDetail = eventFailureDetail(event);
  if (phase === 'inFlight' || eventIsInFlight(event)) return '流式输出中';
  if (statusCode === 499 || outcome === 'cancelled') {
    if (!outcome && phase === 'completed') {
      return '最终结果未上报（HTTP 499 仅供参考）';
    }
    return '客户端主动断开连接 / 取消请求 (499)';
  }
  // A successful request is described by its recorded lifecycle result. The
  // engine message may contain routing tokens (passthrough/bridge) or other
  // implementation notes; those remain available in the diagnostic detail
  // and must not replace the user-facing success state.
  if (outcome === 'succeeded') {
    return eventStreamTrace(event) ? '流式输出完成' : '请求成功';
  }

  const parts = [];
  if (eventFailover(event)) parts.push('本次上游额度耗尽/故障，已自动平滑故障转移');
  if (failureKind) parts.push(failureKindLabel(failureKind));
  if (message) {
    for (const segment of message.split(';').map(cleanText).filter(Boolean)) {
      if (/^(?:passthrough|bridge)\s+/.test(segment)) {
        // This is a routing token, not a user-facing outcome. The protocol
        // path and raw engine message remain in the diagnostic detail.
        continue;
      } else if (segment === 'unmatched_no_tools') {
        parts.push('未观察到工具调用，且请求用途未识别');
      } else if (/^deferred_rounds\s+\d+$/.test(segment)) {
        parts.push(`跨轮重试 ${segment.split(/\s+/).at(-1)} 轮`);
      } else {
        parts.push(segment);
      }
    }
  }

  if (parts.length) return parts.join('；');
  if (outcome === 'failed' && failureDetail) {
    return `请求失败（最终结果为 Failed）：${failureDetail}`;
  }
  if (outcome === 'failed') return '请求失败（最终结果为 Failed）';
  const inferred = eventResultKind(event);
  if (!outcome && inferred === 'succeeded') {
    return eventPhase(event) === 'completed'
      ? '最终结果未上报（HTTP 状态仅供参考）'
      : '旧事件：按 HTTP 状态推断为成功';
  }
  if (!outcome && inferred === 'cancelled') {
    return eventPhase(event) === 'completed'
      ? '最终结果未上报（HTTP 499 仅供参考）'
      : '旧事件：按 HTTP 499 推断为客户端取消';
  }
  if (!outcome && inferred === 'failed') {
    return eventPhase(event) === 'completed'
      ? '最终结果未上报（HTTP 状态仅供参考）'
      : '旧事件：按 HTTP 状态推断为失败';
  }
  if (failureDetail) return failureDetail;
  return '已结束';
}

export function statusKind(statusCodeOrEvent, inFlight, outcome) {
  const isEvent = statusCodeOrEvent && typeof statusCodeOrEvent === 'object';
  if (isEvent && cleanText(statusCodeOrEvent.kind).toLowerCase() === 'notify') return 'muted';
  const statusCode = isEvent ? eventHTTPStatusCode(statusCodeOrEvent) : statusCodeOrEvent;
  const live = isEvent ? eventIsInFlight(statusCodeOrEvent) : inFlight;
  const finalOutcome = isEvent ? eventOutcome(statusCodeOrEvent) : outcome;
  if (live) return 'live';
  if (finalOutcome === 'failed') return 'critical';
  if (statusCode === 499 || finalOutcome === 'cancelled') return 'muted';
  if (statusCode >= 200 && statusCode < 400 && finalOutcome !== 'failed') return 'good';
  // Match the core outcome fallback: every non-499 HTTP 4xx/5xx is a failed
  // request.  Warning is reserved for degraded-but-not-final states elsewhere.
  if (statusCode >= 400) return 'critical';
  return 'muted';
}

// Build Request Chain (Client + all upstream retry / failover attempts for same requestID)
export function getRequestChain(events = [], selectedID) {
  if (!selectedID || !events.length) return [];
  const selected = events.find((e) => e.id === selectedID);
  if (!selected) return [];

  const groupID = eventRequestID(selected) !== '-' ? eventRequestID(selected) : selected.id;
  return events
    .filter((e) => (eventRequestID(e) !== '-' ? eventRequestID(e) : e.id) === groupID)
    .sort((a, b) => {
      const ta = normalizeTimestampMS(a.timestamp);
      const tb = normalizeTimestampMS(b.timestamp);
      return (Number.isFinite(tb) ? tb : 0) - (Number.isFinite(ta) ? ta : 0);
    });
}
