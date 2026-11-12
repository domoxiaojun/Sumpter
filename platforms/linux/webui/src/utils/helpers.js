// Helper utilities for formatters, models, request chains, and telemetry events
import { CLIENT_CATALOG } from './clientRegistry.js';
import { routeModeLabel, sourceFormatLabel } from './protocols.js';

// randomUUID is missing on plain HTTP LAN origins (not a secure context).
export function createLocalID(prefix = 'id') {
  const randomUUID = globalThis.crypto?.randomUUID?.bind(globalThis.crypto);
  if (typeof randomUUID === 'function') {
    try {
      return randomUUID();
    } catch {
      // Fall through when the runtime exposes the method but rejects the call.
    }
  }
  const token = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
  return `${prefix}-${token}`;
}

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

export function formatTimestamp(ts, { date = false, smartDate = false } = {}) {
  if (!ts) return '-';
  const time = normalizeTimestampMS(ts);
  if (Number.isNaN(time) || time <= 0) return String(ts);
  const d = new Date(time);
  const pad = (n) => String(n).padStart(2, '0');
  const timePart = `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  if (date) return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${timePart}`;
  if (smartDate) {
    const now = new Date();
    const sameDay = d.getFullYear() === now.getFullYear()
      && d.getMonth() === now.getMonth()
      && d.getDate() === now.getDate();
    if (!sameDay) return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${timePart}`;
  }
  return timePart;
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
  return '工具调用未记录';
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
  return '流诊断信息未记录';
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
  return cleanText(codexMetadataField(metadata, 'subagentKind', 'subagent_kind'))
    || cleanText(codexMetadataField(metadata, 'subagentHeader', 'subagent_header'));
}

function codexIsGuardian(metadata) {
  return codexSubagentKind(metadata) === 'guardian'
    || codexMetadataField(metadata, 'threadSource', 'thread_source') === 'guardian_review';
}

function codexInternalRole(metadata) {
  const source = codexMetadataField(metadata, 'threadSource', 'thread_source');
  const kind = codexSubagentKind(metadata);
  if (codexIsGuardian(metadata)) return 'Guardian 安全审查';
  if (source === 'memory_consolidation' || kind === 'memory_consolidation'
    || codexMetadataField(metadata, 'requestKind', 'request_kind') === 'memory'
    || codexMetadataField(metadata, 'memgenRequest', 'memgen_request') === 'true') return '记忆任务';
  if (source === 'automated_review' || kind === 'review') return '自动审查';
  return null;
}

export function codexRequestKindLabel(kind) {
  return ({prewarm:'连接预热',turn:'正式回合',compaction:'上下文压缩',memory:'记忆提取',unknown:'用途未确定'})[kind] || '用途未确定';
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
  if (codexInternalRole(metadata)) return codexInternalRole(metadata);
  if (codexHasSubagentEvidence(metadata)) return `子代理${kind ? ` · ${kind}` : ''}`;
  if (codexMetadataField(metadata, 'agentName', 'agent_name')
    || codexMetadataField(metadata, 'threadID', 'thread_id')
    || codexMetadataField(metadata, 'turnID', 'turn_id')) {
    return codexMetadataField(metadata, 'threadSource', 'thread_source') === 'user' ? '主代理' : '未发现子代理证据';
  }
  return '代理身份未确定';
}

function codexAgentRoleSummary(metadata) {
  if (codexInternalRole(metadata)) return codexInternalRole(metadata);
  if (codexHasSubagentEvidence(metadata)) {
    const kind = codexSubagentKind(metadata);
    return `子代理${kind ? ` ${kind}` : ''}`;
  }
  return codexMetadataField(metadata, 'agentName', 'agent_name')
    || codexMetadataField(metadata, 'threadID', 'thread_id')
    || codexMetadataField(metadata, 'turnID', 'turn_id')
    ? (codexMetadataField(metadata, 'threadSource', 'thread_source') === 'user' ? '主代理' : '未发现子代理证据')
    : '未发现子代理证据';
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
  if (requestKind) parts.push(codexRequestKindLabel(requestKind));
  const history = metadata.historyInitialization ?? metadata.extras?.history_initialization;
  if (history) parts.push(`历史初始化 ${history}`);
  if (typeof metadata.analyticsEnabled === 'boolean') parts.push(`客户端分析 ${metadata.analyticsEnabled ? '开启' : '关闭'}`);
  if (metadata.mcpAttribution) parts.push(`MCP 归因 ${metadata.mcpAttribution.status} · 来源 ${metadata.mcpAttribution.sourceCount}`);
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

// Completed client rows and output speed use the final upstream attempt's
// timeline. In-flight rows keep the client clock so the total continues to
// advance while the request is open.
export function eventDisplayTiming(event) {
  const upstreamTTFB = eventField(event, 'upstreamTTFBMS', 'upstream_ttfb_ms');
  const upstreamDuration = eventField(event, 'upstreamDurationMS', 'upstream_duration_ms');
  if (cleanText(event?.kind).toLowerCase() === 'client'
      && !eventIsInFlight(event) && upstreamTTFB != null && upstreamDuration != null) {
    return { ttfbMS: upstreamTTFB, durationMS: upstreamDuration };
  }
  return { ttfbMS: eventTTFBMS(event), durationMS: eventDurationMS(event) };
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

export function eventLogicalModel(event) {
  return cleanText(eventField(event, 'effectiveModel', 'effective_model')) || '—';
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
  return '请求用途未记录';
}

export function requestKindLabel(value) {
  if (value == null || value === '' || value === 'unrecorded') return '未记录';
  return ({ turn: '生成回合', prewarm: '预热', compaction: '上下文压缩', session_title: '会话标题', memory: '记忆任务', review: '审查', websearch: '搜索', webfetch: '网页抓取', classifier: '安全分类', token_count: 'Token 计数', image_generation: '图片生成', image_edit: '图片编辑', alpha_search: '独立搜索', realtime: '实时会话', unknown: '未识别' })[value] || '未识别';
}

export function clientVariantLabel(value) {
  if (value == null || value === '' || value === 'unrecorded') return '未记录';
  return ({ cli: 'CLI', tui: 'TUI', desktop: '桌面', ide: 'IDE', vscode: 'VS Code', sdk: 'SDK', web: '网页', backend: '服务端', exec: '非交互执行', unknown: '未识别' })[value] || value;
}

export function agentRoleLabel(value) {
  if (value == null || value === '' || value === 'unrecorded') return '未记录';
  return ({ root: '主代理', subagent: '子代理', guardian: 'Guardian', review: '审查', memory: '记忆任务', title: '标题任务', automation: '自动任务', system: '系统', ambient: '后台任务', unknown: '未识别' })[value] || value;
}

export function clientAttributionSummary(attribution) {
  if (!attribution) return '未记录';
  const sources = { declared: '客户端声明', native: '原生标记', endpoint: '专用接口', user_agent: '产品 UA', fingerprint: '固定指纹', token_binding: 'Token 绑定', configured_ua: '配置别名', trusted_relay: '可信中继' };
  const fields = { clientKind: '客户端', clientVariant: '形态', agentRole: '角色', requestKind: '请求类型' };
  const evidence = (attribution.evidence || []).map((e) => `${fields[e.field] || e.field}：${sources[e.source] || e.source} / ${e.rule} → ${e.value}`);
  const identity=attribution.identity;
  if(identity) for(const [key,label] of [['rawProduct','原始产品'],['product','产品'],['reportedAs','统计归属'],['host','宿主'],['engine','引擎'],['relaySource','中继'],['agentId','子代理ID'],['parentAgentId','父代理ID']]) if(identity[key]) evidence.push(label+'：'+identity[key]);
  if (attribution.conflicts?.length) evidence.push(`信号冲突：${attribution.conflicts.map((f) => fields[f] || f).join('、')}`);
  if (attribution.issues?.length) evidence.push(`已忽略无效字段：${attribution.issues.join('、')}`);
  return evidence.join('；') || '未识别（缺少有效依据）';
}

export const CLIENT_KIND_LABELS = Object.freeze({
  ...Object.fromEntries(CLIENT_CATALOG.map(c => [c.id,c.name])),
  openai_compat: 'OpenAI 兼容客户端',
  unknown: '未知客户端',
});

export function clientKindLabel(kind) {
  return CLIENT_KIND_LABELS[cleanText(kind)] || (cleanText(kind) ? cleanText(kind) : '未知客户端');
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
  if (state === 'missing') return '客户端未记录';
  return clientKindLabel(eventField(event, 'clientKind'));
}

export function eventResultKind(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return null;
  // A streaming response can already have HTTP 200 headers while its final
  // protocol outcome is still unknown. Keep it pending until completion.
  if (eventIsInFlight(event)) return null;
  const outcome = eventOutcome(event);
  if (outcome === 'succeeded' || outcome === 'failed' || outcome === 'cancelled') return outcome;
  return null;
}

export function eventKindLabel(kind) {
  return ({ client: '客户端', upstream: '上游', notify: '通知' })[kind] || String(kind || '-');
}

export function failureKindLabel(kind) {
  const map = {
    response_timeout: '响应超时 (Response Timeout)',
    response_wait_timeout: '代理响应等待超时',
    no_response_retry_exhausted: '连续未收到响应，已停止重试',
    connection_failed: '连接失败 (Connection Failed)',
    invalid_response: '上游响应格式异常 (Invalid Response)',
    upstream_http_status: '上游返回 HTTP 错误状态',
    stream_idle_timeout: '流式空闲超时 (Stream Idle Timeout)',
    stream_interrupted: '流传输中途断开 (Stream Interrupted)',
    upstream_response_incomplete: '上游响应流未完整结束',
    upstream_response_failed: '上游响应协议失败',
    endpoints_exhausted: '所有可用入口全部耗尽',
    local_admission_rejected: '本地请求队列已满或排队超时',
    client_cancelled: '客户端断开或取消 (Client Cancelled)',
    client_disconnected: '客户端连接已关闭 (Client Disconnected)',
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
  const { ttfbMS: ttfb, durationMS: duration } = eventDisplayTiming(event);
  if (eventIsInFlight(event)) {
    if (ttfb != null) {
      // Keep TTFB fixed while the locally calculated elapsed duration advances.
      // Header-timestamped upstream rows can briefly have an elapsed time below TTFB.
      const total = duration != null && duration >= ttfb ? formatDuration(duration) : '进行中...';
      return `${formatDuration(ttfb)} → ${total}`;
    }
    return duration != null ? `${formatDuration(duration)} · 等待首字节...` : '等待首字节...';
  }
  if (ttfb != null && duration != null && duration >= ttfb) {
    return `${formatDuration(ttfb)} → ${formatDuration(duration)}`;
  }
  return formatDuration(duration ?? ttfb);
}

export function eventHTTPStatusCode(event) {
  return eventStatusCode(event);
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
  if (outcome === 'cancelled') return event.failureKind === 'client_disconnected' ? '连接中断（原因未确定）' : '客户端断开或取消 (Cancelled)';
  if (outcome === 'failed') return '最终结果失败 (Failed)';
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
  return phase || '阶段未记录';
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
  return '未收到响应头 / 未记录';
}

export function eventProtocolRouteLabel(event) {
  if (cleanText(event?.kind).toLowerCase() === 'notify') return '不适用（通知事件）';
  const source = eventField(event, 'sourceFormat', 'source_format');
  const target = eventField(event, 'targetFormat', 'target_format');
  const mode = eventField(event, 'routeMode', 'route_mode');
  if (!source && !target && !mode) {
    if (eventIsInFlight(event)) return '尚未记录（路由未定型）';
    return '未记录';
  }
  return `${source ? sourceFormatLabel(source) : '未记录'} → ${target ? sourceFormatLabel(target) : '未记录'} · ${mode ? routeModeLabel(mode) : '未记录'}`;
}

export function eventTransportLabel(event) {
  const transport = String(eventField(event, 'transport') || '').toLowerCase();
  if (transport === 'sse') return 'SSE';
  if (transport === 'websocket' || transport === 'web_socket') return 'WebSocket';
  if (transport === 'http') return 'HTTP';
  return '未记录';
}

export function eventFailureSummaryLabel(event) {
  const kind = eventFailureKind(event);
  const phase = eventFailurePhase(event);
  if (kind) {
    return `${failureKindLabel(kind)}${phase ? ` · ${failurePhaseLabel(phase)}` : ''}`;
  }
  const result = eventResultKind(event);
  if (result === 'failed') return '未记录结构化失败信息';
  if (eventIsInFlight(event)) return '尚无失败信息（进行中）';
  if (result === 'cancelled') return '不适用（请求已取消）';
  if (result === 'succeeded') return '不适用（请求成功）';
  return '未记录';
}

export function formatWebSocketTrace(trace) {
  if (!trace) return null;
  const parts = [];
  const stage = trace.stage;
  const stages = { awaiting_first_message: '等待首条业务消息', upstream_handshake: '上游握手', relay: '消息转发' };
  if (stage) parts.push(`WebSocket 阶段: ${stages[stage] || stage}`);
  const handshake = trace.clientHandshakeStatus ?? trace.client_handshake_status;
  if (handshake != null) parts.push(`客户端握手: HTTP ${handshake}`);
  const upstreamHandshake = trace.handshakeStatus ?? trace.handshake_status;
  if (upstreamHandshake != null) parts.push(`上游握手: HTTP ${upstreamHandshake}`);
  const wait = trace.firstMessageWaitMS ?? trace.first_message_wait_ms;
  if (wait != null) parts.push(`首消息等待: ${formatDuration(wait)}`);
  const side = trace.closedBy ?? trace.closed_by;
  const sides = { client: '客户端侧连接（可能经过反代）', upstream: '上游侧连接', server: '代理服务', relay_error: '转发过程' };
  if (side) parts.push(`关闭位置: ${sides[side] || side}`);
  const code = trace.clientCloseCode ?? trace.client_close_code;
  if (code != null) parts.push(`客户端关闭码: ${code}`);
  const upstreamCode = trace.upstreamCloseCode ?? trace.upstream_close_code;
  if (upstreamCode != null) parts.push(`上游关闭码: ${upstreamCode}`);
  const abnormal = trace.abnormalClose ?? trace.abnormal_close;
  if (abnormal != null) parts.push(abnormal ? '异常关闭' : '正常关闭');
  const attempts = trace.attemptCount ?? trace.attempt_count;
  if (attempts != null) parts.push(`上游尝试: ${attempts}`);
  const error = trace.relayError ?? trace.relay_error;
  if (error) parts.push(`关闭记录: ${error}`);
  const transportError = trace.transportErrorKind ?? trace.transport_error_kind;
  if (transportError) parts.push(`传输错误类型: ${transportError}`);
  const lastEvent = trace.lastEventType ?? trace.last_event_type;
  if (lastEvent) parts.push(`最近上游事件: ${lastEvent}`);
  const idleTimeout = trace.idleTimeoutMS ?? trace.idle_timeout_ms;
  if (idleTimeout != null) parts.push(`空闲截止: ${formatDuration(idleTimeout)}`);
  const requests = trace.requests;
  if (requests) {
    parts.push('逻辑请求: ' + Object.entries(requests.counts || {}).map(([kind,count]) => `${codexRequestKindLabel(kind)} ${count}`).join(' / '));
    parts.push(`明细省略 ${requests.dropped} · 未观测帧 ${requests.unobservedFrames} · 未关联事件 ${requests.unassociatedEvents}`);
    parts.push(...(requests.recent || []).map(r => `#${r.sequence} ${codexRequestKindLabel(r.kind)} · turn=${r.turnId || '—'} · response=${r.responseId || '—'} · ${r.terminal || '未观察到终态'}`));
  }

  return parts.join(' · ');
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
  const websocket = trace.websocketTrace ?? trace.websocket_trace;
  if (websocket) return formatWebSocketTrace(websocket);
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

export function websocketFailureMessage(trace) {
  const side = trace.closedBy ?? trace.closed_by;
  const sides = { client: '客户端侧连接', upstream: '上游侧连接', server: '代理服务' };
  const kind = trace.transportErrorKind ?? trace.transport_error_kind;
  const reasons = { eof_without_close: '未完成关闭握手即断开', unexpected_eof: '传输意外结束', connection_reset: '连接被重置', connection_aborted: '连接被中止', broken_pipe: '写入时连接已断开', protocol_error: 'WebSocket 协议错误', tls_error: 'TLS 连接错误', timed_out: '传输超时', abnormal_close: '异常关闭码' };
  if (!kind) return 'WebSocket 连接异常结束（不代表每次响应均失败）';
  const code = side === 'client' ? trace.clientCloseCode : trace.upstreamCloseCode;
  const last = trace.lastEventType ?? trace.last_event_type;
  return `${sides[side] || 'WebSocket 连接'}：${reasons[kind] || kind}${code == null ? '' : `（${code}）`}${last ? ` · 最近事件 ${last}` : ''}（连接结果不代表每次响应结果）`;
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
  if (phase === 'inFlight' || eventIsInFlight(event)) {
    const ws = eventStreamTrace(event)?.websocketTrace;
    if (ws) return ({ awaiting_first_message: 'WebSocket 等待首条业务消息', upstream_handshake: 'WebSocket 连接上游中', relay: 'WebSocket 连接转发中（不代表模型正在生成）' })[ws.stage] || 'WebSocket 连接中';
    if (!statusCode) return '等待响应';
    return eventStreamTrace(event) ? '流式输出中' : '接收响应中';
  }
  if (outcome === 'cancelled') {
    if (event.failureKind === 'client_disconnected') return '连接中断（原因未确定）';
    const trace = eventStreamTrace(event)?.websocketTrace ?? eventStreamTrace(event)?.websocket_trace;
    if (trace?.stage === 'awaiting_first_message' && message === 'websocket_unused_prewarm_connection_closed') return '预热连接已关闭（未发送业务请求）';
    if (trace?.stage === 'awaiting_first_message' && message === 'websocket_unused_guardian_connection_closed') {
      return 'Guardian 未使用连接已关闭（预热阶段，未发送业务请求）';
    }
    return (trace?.closedBy ?? trace?.closed_by) === 'client'
      ? '客户端侧 WebSocket 连接关闭（可能经过反代）'
      : '客户端断开或取消请求';
  }
  const websocket = eventStreamTrace(event)?.websocketTrace ?? eventStreamTrace(event)?.websocket_trace;
  if (websocket?.stage === 'awaiting_first_message' && outcome === 'failed') {
    if ((websocket.relayError ?? websocket.relay_error) === 'server_shutdown') return '代理停止，WebSocket 连接已结束';
    return failureKind === 'client_request_rejected'
      ? 'WebSocket 首条业务消息格式错误'
      : '首条业务消息前 WebSocket 异常断开';
  }
  if (websocket?.stage === 'relay') {
    if ((websocket.relayError ?? websocket.relay_error) === 'server_shutdown') return '代理停止，WebSocket 连接已结束';
    if (failureKind === 'stream_idle_timeout') return 'WebSocket 双向帧空闲超时';
    if (outcome === 'failed') return websocketFailureMessage(websocket);
    if (outcome === 'succeeded') return websocket.abnormalClose || websocket.transportErrorKind
      ? `响应已完成；${websocketFailureMessage(websocket)}` : 'WebSocket 连接正常结束';
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
  if (failureDetail) return failureDetail;
  return '最终结果未上报';
}

export function statusKind(statusCodeOrEvent, inFlight, outcome) {
  const isEvent = statusCodeOrEvent && typeof statusCodeOrEvent === 'object';
  if (isEvent && cleanText(statusCodeOrEvent.kind).toLowerCase() === 'notify') return 'muted';
  const statusCode = isEvent ? eventHTTPStatusCode(statusCodeOrEvent) : statusCodeOrEvent;
  const live = isEvent ? eventIsInFlight(statusCodeOrEvent) : inFlight;
  const finalOutcome = isEvent ? eventOutcome(statusCodeOrEvent) : outcome;
  if (live) return 'live';
  if (finalOutcome === 'failed') return 'critical';
  if (finalOutcome === 'succeeded') return 'good';
  if (finalOutcome === 'cancelled') return 'muted';
  if (isEvent) return 'muted';
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
