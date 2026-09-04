import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { MetricCard } from '../components/MetricCard.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { PaginationBar } from '../components/PaginationBar.jsx';
import { useRuntimeEventPage } from '../hooks/useRuntimeEventPage.js';
import { Icon } from '../utils/icons.jsx';
import { orderRuntimeEvents } from '../utils/runtimeEvents.js';
import { copyWithToast } from '../utils/clipboard.js';
import { api } from '../services/api.js';
import {
  formatNumber, formatTokenCount, formatDuration, formatTimestamp,
  eventModel, eventEndpoint, statusKind,
  eventDurationText, friendlyEventMessage, eventPurposeLabel,
  eventClientKindLabel, eventKindLabel, getRequestChain,
  eventOutcomeLabel, eventPhaseLabel, eventHttpStatusLabel, eventStatusDetailLabel,
  eventToolCalls, eventEndpointName, eventEndpointID,
  eventUpstreamHost, eventUpstreamModel, eventFeatureRuleID,
  eventRequestID, eventSessionID, eventUpstreamRequestID, eventMessage,
  eventFailureDetail, eventTimeoutMS, eventTTFBMS, eventDurationMS, eventOutcome,
  eventFailover, eventField, eventCodexMetadata, eventGrokMetadata, codexMetadataSummary, eventIsInFlight,
  codexMetadataJSON, codexMetadataField, grokMetadataJSON, grokMetadataField, grokMetadataSummary,
  eventFailureSummaryLabel, eventProtocolRouteLabel, eventResultKind,
  eventStreamTrace, eventToolCallsLabel, eventUpstreamStatusLabel,
  codexAgentPath, codexAgentRoleLabel, codexWorkspaceEntries, codexWorkspaceSummary,
  eventProjectContext, projectSourceLabel, eventCodexThreadClass,
  codexThreadClassLabel, attributionScopeLabel, codexHasRequestIdentity,
} from '../utils/helpers.js';

function traceField(value, camel, snake) {
  return value?.[camel] ?? value?.[snake];
}

function formatTraceBytes(value) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes)) return '—';
  if (bytes < 1024) return `${formatNumber(bytes)} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
}

function recentTokenTotals(events) {
  const usages = events
    .filter((event) => event.kind === 'client' && !eventIsInFlight(event) && eventResultKind(event) !== 'cancelled')
    .map((event) => eventStreamTrace(event)?.usage)
    .filter((usage) => usage && typeof usage === 'object');
  const sum = (field, snakeField) => {
    const values = usages
      .map((usage) => usage[field] ?? usage[snakeField])
      .map(Number)
      .filter((value) => Number.isFinite(value) && value >= 0);
    return values.length ? values.reduce((total, value) => total + value, 0) : null;
  };
  return { input: sum('inputTokens', 'input_tokens'), output: sum('outputTokens', 'output_tokens'), observed: usages.length };
}

// macOS 的最近事件行把请求归属和 Codex 上下文收敛成一条可换行摘要，
// 让项目、客户端、代理身份、用途和工具路径在第一眼就能被读到。
// 分页投影没有完整元数据时，只显示服务端已确认的字段；详情面板仍保留
// 完整诊断字段，不在列表里臆造缺失信息。
function recentEventProjectSummary(event) {
  const context = eventProjectContext(event);
  if (!context?.applicable) return null;
  if (context.name && context.name !== 'unidentified_project') {
    const declared = context.source === 'client_declared' ? '（客户端声明）' : '';
    return `项目: ${context.name}${declared}`;
  }
  return context.label;
}

function recentEventPurposeLabel(event) {
  const raw = eventField(event, 'requestPurpose');
  const key = typeof raw === 'object' ? (raw?.kind || raw?.type || raw?.name) : raw;
  // RuntimeEventDisplay on macOS uses these shorter labels for the recent
  // events table. Analytics keeps its own, more explanatory labels.
  if (key === 'standard') return '主请求';
  if (key === 'websearch') return 'WebSearch';
  if (key === 'webfetch') return 'WebFetch';
  return eventPurposeLabel(event);
}

function recentEventOutcomeLabel(event) {
  if (String(event?.kind || '').toLowerCase() === 'notify' && !eventOutcome(event)) {
    return '不适用（通知事件）';
  }
  if (eventIsInFlight(event)) return '传输中';
  const outcome = eventOutcome(event);
  if (outcome === 'succeeded') return '成功';
  if (outcome === 'failed') return '失败';
  if (outcome === 'cancelled') return '已取消';
  return eventOutcomeLabel(event).replace(/\s*\([^)]*\)\s*/g, '');
}

function recentEventPhaseLabel(event) {
  if (String(event?.kind || '').toLowerCase() === 'notify' && !eventOutcome(event)) {
    return '不适用（通知事件）';
  }
  if (eventIsInFlight(event)) return '进行中';
  const phase = eventField(event, 'phase');
  if (phase === 'completed' || eventOutcome(event)) return '已完成';
  return eventPhaseLabel(event).replace(/\s*\([^)]*\)\s*/g, '');
}

function recentEventStatusDetail(event) {
  if (String(event?.kind || '').toLowerCase() === 'notify' && !eventOutcome(event)) {
    return '不适用（通知事件）';
  }
  return `${eventHttpStatusLabel(event)} · ${recentEventPhaseLabel(event)}`;
}

function recentEventRequestSummary(event) {
  const metadata = eventCodexMetadata(event);
  const grok = eventGrokMetadata(event);
  const grokClient = eventField(event, 'clientKind', 'client_kind') === 'grok_build';
  const showCodexAgent = metadata && !(grokClient && !codexHasRequestIdentity(metadata));
  const tools = eventToolCalls(event);
  return [
    recentEventProjectSummary(event),
    eventKindLabel(event.kind),
    eventClientKindLabel(event),
    grok ? `Grok: ${grokMetadataSummary(grok)}` : null,
    showCodexAgent ? `代理: ${codexAgentRoleLabel(metadata)}` : null,
    recentEventPurposeLabel(event),
    tools.length ? `工具: ${tools.join('、')}` : null,
    showCodexAgent ? `路径: ${codexAgentPath(metadata)}` : null,
    eventCodexThreadClass(event) ? `功能线程: ${codexThreadClassLabel(eventCodexThreadClass(event))}` : null,
    eventField(event, 'attributionScope', 'attribution_scope') === 'internal_feature'
      ? `归因范围: ${attributionScopeLabel('internal_feature')}` : null,
  ].filter(Boolean).join(' · ');
}

function RecentEventOutcome({ event, compact = false }) {
  return (
    <span className={`telemetry-outcome-text ${statusKind(event)}${compact ? ' is-compact' : ''}`}>
      {recentEventOutcomeLabel(event)}
    </span>
  );
}

function RecentEventRequestCell({ event, live = false }) {
  const summary = recentEventRequestSummary(event);
  return (
    <span className={live ? 'telemetry-live-request' : 'telemetry-primary-cell'}>
      <span className={`mono-cell${live ? ' telemetry-live-time' : ' telemetry-event-time'}`}>
        {formatTimestamp(event.timestamp)}
      </span>
      <span className={`telemetry-event-meta${live ? ' telemetry-live-request-summary' : ''}`} title={summary}>
        {summary || '-'}
      </span>
    </span>
  );
}

function RecentEventRouteCell({ event, live = false }) {
  return (
    <span className={live ? 'telemetry-live-route' : 'telemetry-cell-stack telemetry-route-cell'}>
      <strong className="telemetry-event-model">{eventModel(event)}</strong>
      <span className="telemetry-route-meta">{eventEndpoint(event)}</span>
    </span>
  );
}

function RecentEventResultCell({ event, live = false }) {
  const slowTTFB = eventTTFBMS(event) >= 5000;
  return (
    <span className={live ? 'telemetry-live-result' : 'telemetry-cell-stack telemetry-result-cell'}>
      <RecentEventOutcome event={event} />
      <span className="telemetry-result-detail">{recentEventStatusDetail(event)}</span>
      <span
        className={`mono-cell telemetry-result-duration${slowTTFB ? ' is-slow' : ''}`}
        title={slowTTFB ? '首字节超过 5 秒，上游可能排队中' : undefined}
      >
        {eventDurationText(event)}
      </span>
    </span>
  );
}

function RecentEventMessageCell({ event, live = false }) {
  const friendly = friendlyEventMessage(event);
  return (
    <span
      className={`${live ? 'telemetry-live-message' : 'telemetry-event-message'}${eventFailover(event) ? ' is-warning' : eventOutcome(event) === 'failed' ? ' is-error' : ''}`}
      title={friendly || undefined}
    >
      {eventFailover(event) && <strong>故障转移 · </strong>}
      {friendly || (live ? '请求进行中，等待最终结果' : '-')}
    </span>
  );
}

function StreamTraceRows({ event }) {
  const trace = eventStreamTrace(event);
  if (!trace) {
    return <div className="runtime-stream-trace-empty">{eventIsInFlight(event) ? '等待流诊断信息' : '无流诊断信息（未进入流式阶段或旧事件未记录）'}</div>;
  }
  const usage = trace.usage && typeof trace.usage === 'object' ? trace.usage : null;
  const duration = eventDurationMS(event);
  const lastChunkAtMS = traceField(trace, 'lastChunkAtMS', 'last_chunk_at_ms');
  const tokenParts = usage ? [
    ['输入', traceField(usage, 'inputTokens', 'input_tokens')],
    ['输出', traceField(usage, 'outputTokens', 'output_tokens')],
    ['缓存读', traceField(usage, 'cacheReadInputTokens', 'cache_read_input_tokens')],
    ['缓存写', traceField(usage, 'cacheCreationInputTokens', 'cache_creation_input_tokens')],
    ['推理', traceField(usage, 'reasoningTokens', 'reasoning_tokens')],
  ].filter(([, value]) => value != null).map(([label, value]) => `${label} ${formatNumber(value)}`) : [];
  const rows = [
    ['终止事件', traceField(trace, 'terminalEvent', 'terminal_event') || (eventIsInFlight(event) ? '等待协议终止' : '未观察到终止')],
    ['停止原因', traceField(trace, 'stopReason', 'stop_reason') || '—'],
    ['Token usage', tokenParts.join(' · ') || '—'],
    ['Chunk 数', traceField(trace, 'chunkCount', 'chunk_count') ?? '—'],
    ['接收字节', formatTraceBytes(traceField(trace, 'bytesReceived', 'bytes_received'))],
    ['最大 Chunk 间隔', traceField(trace, 'maxChunkGapMS', 'max_chunk_gap_ms') == null ? '—' : formatDuration(traceField(trace, 'maxChunkGapMS', 'max_chunk_gap_ms'))],
    ['最后 Chunk', lastChunkAtMS == null ? '—' : formatDuration(lastChunkAtMS)],
    ['结束前空闲', lastChunkAtMS == null || duration == null || eventIsInFlight(event) ? '—' : formatDuration(Math.max(0, duration - lastChunkAtMS))],
  ];
  return <div className="runtime-stream-trace-grid">{rows.map(([label, value]) => <div key={label}><span>{label}</span><strong className="mono-cell">{value}</strong></div>)}</div>;
}

const CODEX_METADATA_LABELS = {
  installationID: 'Installation ID', sessionID: 'Session ID', threadID: 'Thread ID',
  sourceInstallationID: '源 installation ID',
  agentName: '代理路径 (agentName)', turnID: '回合 ID', windowID: '窗口 ID', requestKind: '请求类型',
  forkedFromThreadID: '派生自线程 ID', parentThreadID: '父线程 ID',
  parentTurnID: '父回合 ID', rootTurnID: '根回合 ID', subagentHeader: '子代理 Header',
  subagentKind: '子代理类型', threadSource: '线程来源', sandbox: '沙箱',
  sandboxMode: '沙箱模式', autoReviewEnabled: '自动审查',
  nodeReplAutoReviewRequired: 'Node REPL 需要审查', nodeReplDisabled: 'Node REPL 已禁用',
  turnStartedAtUnixMS: '回合开始时间（Unix ms）', originator: '来源客户端',
  betaFeatures: 'Beta 特性', memgenRequest: 'Memgen 请求',
  responsesLite: 'Responses Lite',
  wsStreamRequestStartMS: 'WebSocket 请求开始（ms）', malformed: '格式异常', truncated: '已截断',
  hasConflicts: '字段冲突', isSubagent: '是否子代理', parentThreadIDInferred: '父线程是否推断',
};

const GROK_METADATA_LABELS = {
  sessionID: '会话 ID', convID: '对话 ID', requestID: 'Grok 请求 ID',
  agentID: 'Agent ID', turnIndex: '回合序号', transientRetry: '瞬时重试',
  modelOverride: '模型覆盖', clientIdentifier: '客户端标识',
  clientVersion: '客户端版本', clientMode: '客户端模式',
  deploymentID: '部署 ID', userID: '账号 ID', userAgent: 'User-Agent',
  compactionsRemaining: '剩余压缩次数', compactionAt: '压缩阈值',
  doomLoopCheck: 'Doom loop 窗口', exactRepetitionCheck: '精确重复检测',
};

function GrokMetadataDetails({ metadata, onCopy }) {
  if (!metadata) return null;
  const scalarEntries = Object.entries(GROK_METADATA_LABELS)
    .map(([camel, label]) => [label, grokMetadataField(metadata, camel)])
    .filter(([, value]) => value !== undefined && value !== null && value !== '');
  if (!scalarEntries.length) return null;
  return (
    <details className="codex-metadata-details">
      <summary style={{ cursor: 'pointer', color: 'var(--text-primary)', fontWeight: 600 }}>Grok 客户端 / 会话全部元数据</summary>
      <div className="grid-3col codex-metadata-grid" style={{ marginTop: '8px', gap: '8px', fontSize: '0.78rem' }}>
        {scalarEntries.map(([label, value]) => <div key={label}><span style={{ color: 'var(--text-muted)' }}>{label}：</span><span className="mono-cell">{String(value)}</span></div>)}
      </div>
      <details className="codex-raw-details">
        <summary>完整 Grok 元数据 JSON</summary>
        <pre className="mono-cell technical-pre" style={{ color: 'var(--text-secondary)', fontSize: '0.72rem' }}>
          {grokMetadataJSON(metadata)}
        </pre>
      </details>
      <button type="button" className="btn btn-ghost" style={{ marginTop: '8px', padding: '4px 8px', fontSize: '0.75rem' }} onClick={() => onCopy(grokMetadataJSON(metadata), 'Grok 元数据 JSON')}>
        <Icon name="copy" size={12} /> 复制完整 Grok 元数据 JSON
      </button>
    </details>
  );
}

function CodexMetadataDetails({ metadata, onCopy }) {
  if (!metadata) return null;
  const scalarEntries = Object.entries(CODEX_METADATA_LABELS)
    .map(([camel, label]) => [label, codexMetadataField(metadata, camel)])
    .filter(([, value]) => value !== undefined && value !== null && value !== '');
  const renderJSONSection = (label, value) => {
    if (!value || typeof value !== 'object' || Object.keys(value).length === 0) return null;
    return (
      <div key={label} style={{ marginTop: '8px' }}>
        <span style={{ color: 'var(--text-muted)' }}>{label}：</span>
        <pre className="mono-cell technical-pre" style={{ color: 'var(--text-secondary)', fontSize: '0.72rem' }}>
          {JSON.stringify(value, null, 2)}
        </pre>
      </div>
    );
  };
  return (
    <details className="codex-metadata-details">
      <summary style={{ cursor: 'pointer', color: 'var(--text-primary)', fontWeight: 600 }}>Codex 回合 / 代理全部元数据</summary>
      <div className="grid-3col codex-metadata-grid" style={{ marginTop: '8px', gap: '8px', fontSize: '0.78rem' }}>
        {scalarEntries.map(([label, value]) => <div key={label}><span style={{ color: 'var(--text-muted)' }}>{label}：</span><span className="mono-cell">{typeof value === 'boolean' ? (value ? 'true' : 'false') : String(value)}</span></div>)}
      </div>
      {renderJSONSection('工作区完整字段', codexMetadataField(metadata, 'workspaces'))}
      {renderJSONSection('源工作区路径', codexMetadataField(metadata, 'sourceWorkspacePaths', 'source_workspace_paths'))}
      {renderJSONSection('工具命名空间', codexMetadataField(metadata, 'toolNamespacesInfo', 'tool_namespaces_info'))}
      {renderJSONSection('上下文压缩', codexMetadataField(metadata, 'compaction'))}
      {renderJSONSection('扩展字段', codexMetadataField(metadata, 'extras'))}
      {renderJSONSection('元数据来源', codexMetadataField(metadata, 'sources'))}
      {renderJSONSection('已脱敏字段', codexMetadataField(metadata, 'redactedFields', 'redacted_fields'))}
      {renderJSONSection('冲突字段', codexMetadataField(metadata, 'conflicts'))}
      <details className="codex-raw-details">
        <summary>完整源元数据 JSON</summary>
        <pre className="mono-cell technical-pre" style={{ color: 'var(--text-secondary)', fontSize: '0.72rem' }}>
          {codexMetadataJSON(metadata)}
        </pre>
      </details>
      <button type="button" className="btn btn-ghost" style={{ marginTop: '8px', padding: '4px 8px', fontSize: '0.75rem' }} onClick={() => onCopy(codexMetadataJSON(metadata), 'Codex 元数据 JSON')}>
        <Icon name="copy" size={12} /> 复制完整源元数据 JSON
      </button>
    </details>
  );
}

function LiveEventList({ events, selectedEventID, onSelect }) {
  if (!events?.length) return null;
  return (
    <section className="telemetry-live-group" aria-label={`${formatNumber(events.length)} 个进行中请求`}>
      <span className="ambient-deco telemetry-live-ambient" aria-hidden="true">
        <span className="ambient-base" />
        <span className="ambient-halo" />
        <span className="ambient-flow" />
      </span>
      <div className="telemetry-live-list" role="list" aria-label="进行中请求列表">
        {events.map((event) => {
          const selected = selectedEventID === event.id;
          return (
            <div key={event.id} role="listitem">
              <button
                type="button"
                className={`telemetry-live-event runtime-row-enter${selected ? ' active' : ''}`}
                aria-pressed={selected}
                onClick={() => onSelect(event.id)}
              >
                <RecentEventRequestCell event={event} live />
                <RecentEventRouteCell event={event} live />
                <RecentEventResultCell event={event} live />
                <RecentEventMessageCell event={event} live />
              </button>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function MobileEventList({ events, selectedEventID, onSelect, loading }) {
  if (!events?.length) {
    return (
      <div className="responsive-data-card-list telemetry-mobile-list">
        <div className="responsive-card-empty">{loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}</div>
      </div>
    );
  }

  return (
    <div className="responsive-data-card-list telemetry-mobile-list" role="list" aria-label="请求事件列表">
      {events.map((event) => {
        const projectContext = eventProjectContext(event);
        const outcome = eventOutcome(event);
        const slowTTFB = eventTTFBMS(event) >= 5000;
        const friendly = friendlyEventMessage(event);
        const selected = selectedEventID === event.id;
        return (
          <article key={event.id} role="listitem">
            <div
              className={`responsive-data-card telemetry-mobile-card${selected ? ' is-selected' : ''}`}
              role="button"
              tabIndex={0}
              aria-pressed={selected}
              onClick={() => onSelect(event.id)}
              onKeyDown={(keyboardEvent) => {
                if (keyboardEvent.key === 'Enter' || keyboardEvent.key === ' ') {
                  keyboardEvent.preventDefault();
                  onSelect(event.id);
                }
              }}
            >
            <div className="telemetry-mobile-heading">
              <span className="mono-cell telemetry-mobile-time">{formatTimestamp(event.timestamp)}</span>
              <RecentEventOutcome event={event} compact />
              <span
                className="mono-cell telemetry-mobile-duration"
                style={{ color: slowTTFB ? 'var(--status-warning)' : outcome === 'failed' ? 'var(--status-critical)' : undefined }}
                title={recentEventStatusDetail(event)}
              >
                {eventDurationText(event)}
              </span>
            </div>
            <div className="telemetry-mobile-route">
              <strong className="mono-cell">{eventModel(event)}</strong>
              <small>{eventEndpoint(event)}</small>
            </div>
            <div className="telemetry-mobile-context">
              <span className="telemetry-mobile-request-summary" title={recentEventRequestSummary(event)}>
                {recentEventRequestSummary(event) || projectContext.label}
              </span>
              <span className="telemetry-mobile-status">{recentEventStatusDetail(event)}</span>
              {friendly && (
                <p className={`telemetry-mobile-message${outcome === 'failed' ? ' is-error' : eventFailover(event) ? ' is-warning' : ''}`}>
                  {eventFailover(event) && <strong>故障转移 · </strong>}
                  {friendly}
                </p>
              )}
            </div>
            <span className="responsive-data-card-drill-hint">点击查看请求链路与诊断详情</span>
            </div>
          </article>
        );
      })}
    </div>
  );
}

export function RunPage() {
  const {
    status, runtime, runtimeEventDetail, config, secretStatus, toggleProxy,
    loadRuntimeEvent, addToast,
  } = useApp();
  const [eventFilter, setEventFilter] = useState('client'); // 'client' | 'upstream' | 'all'
  const [eventSort, setEventSort] = useState('desc'); // 'desc' = 最新优先, 'asc' = 最早优先
  const [selectedEventID, setSelectedEventID] = useState(null);
  const [requestChainEvents, setRequestChainEvents] = useState([]);
  const [eventPageDirection, setEventPageDirection] = useState('none');
  const [, setDurationTick] = useState(0);

  const isRunning = status?.running;
  const recentEvents = runtime?.recentEvents || [];
  const tokenTotals = useMemo(() => recentTokenTotals(recentEvents), [recentEvents]);
  const notifySnapshotRecovered = useCallback((message) => addToast(message, 'info'), [addToast]);
  const eventHistory = useRuntimeEventPage({
    recentEvents,
    eventKind: eventFilter === 'all' ? '' : eventFilter,
    resetGeneration: runtime?.resetGeneration,
    onRecovered: notifySnapshotRecovered,
  });

  // Keep the page transition directional while the request is in flight, then
  // clear it so background SSE refreshes do not replay a navigation animation.
  useEffect(() => {
    if (eventHistory.loading || eventPageDirection === 'none') return undefined;
    const timer = window.setTimeout(() => setEventPageDirection('none'), 280);
    return () => window.clearTimeout(timer);
  }, [eventHistory.loading, eventPageDirection]);

  useEffect(() => {
    if (!recentEvents.some((event) => eventIsInFlight(event))) return undefined;
    const timer = window.setInterval(() => setDurationTick((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [recentEvents]);

  const liveEvents = useMemo(() => (
    orderRuntimeEvents(eventHistory.liveEvents || [], eventSort)
  ), [eventHistory.liveEvents, eventSort]);
  const visibleEvents = useMemo(() => (
    orderRuntimeEvents(
      eventHistory.persistedEvents
        || eventHistory.events.filter((event) => !eventIsInFlight(event)),
      eventSort,
    )
  ), [eventHistory.events, eventHistory.persistedEvents, eventSort]);

  const baseEventContext = useMemo(() => {
    const byID = new Map();
    for (const event of [...recentEvents, ...eventHistory.events]) byID.set(event.id, event);
    return [...byID.values()];
  }, [eventHistory.events, recentEvents]);

  // Keep the event list readable on first load. Details and the request chain
  // are an explicit drill-down, so opening Run must not silently expand the
  // first row and trigger an extra request-chain read.
  const currentSelectedID = selectedEventID;
  const selectedEvent = currentSelectedID && (runtimeEventDetail?.event?.id === currentSelectedID
    ? runtimeEventDetail.event
    : requestChainEvents.find((e) => e.id === currentSelectedID)
      || baseEventContext.find((e) => e.id === currentSelectedID)
      || visibleEvents.find((e) => e.id === currentSelectedID));
  const selectedCodexMetadata = eventCodexMetadata(selectedEvent);
  const selectedGrokMetadata = eventGrokMetadata(selectedEvent);
  const selectedCodexHasIdentity = codexHasRequestIdentity(selectedCodexMetadata);
  const selectedClientDeclared = eventField(selectedEvent, 'clientDeclared', 'client_declared') || {};
  const selectedSourceProject = String(selectedClientDeclared.sourceProject ?? selectedClientDeclared.source_project ?? '').trim();
  const selectedSourceWorkspace = String(selectedClientDeclared.sourceWorkspace ?? selectedClientDeclared.source_workspace ?? '').trim();
  const selectedProjectContext = eventProjectContext(selectedEvent);
  const selectedResult = eventResultKind(selectedEvent);

  // Request chain for selected event
  const requestChain = useMemo(() => {
    const byID = new Map();
    for (const event of [...baseEventContext, ...requestChainEvents]) byID.set(event.id, event);
    const eventContext = [...byID.values()];
    const chain = getRequestChain(eventContext, currentSelectedID);
    if (runtimeEventDetail?.event?.id !== currentSelectedID) return chain;
    return chain.map((event) => event.id === currentSelectedID ? runtimeEventDetail.event : event);
  }, [baseEventContext, currentSelectedID, requestChainEvents, runtimeEventDetail]);

  const selectedRequestID = selectedEvent ? eventRequestID(selectedEvent) : '';
  useEffect(() => {
    setRequestChainEvents([]);
    if (!selectedRequestID || selectedRequestID === '-') return undefined;
    const controller = new AbortController();
    api.getRuntimeRequestChain(selectedRequestID, { signal: controller.signal })
      .then((value) => {
        if (Array.isArray(value?.events)) setRequestChainEvents(value.events);
      })
      .catch(() => {
        // Older daemons do not expose this endpoint; page/detail data remains usable.
      });
    return () => controller.abort();
  }, [selectedRequestID]);

  useEffect(() => {
    if (!currentSelectedID) {
      loadRuntimeEvent(null);
      return;
    }
    loadRuntimeEvent(currentSelectedID).catch((error) => {
      addToast(`加载事件详情失败: ${error.message}`, 'error');
    });
  }, [addToast, currentSelectedID, loadRuntimeEvent]);

  const copyText = (text, label = '内容') => copyWithToast(text, label, addToast);
  const changeEventPage = (nextPage) => {
    setSelectedEventID(null);
    const target = Number(nextPage);
    if (Number.isFinite(target) && target !== eventHistory.page) {
      setEventPageDirection(target > eventHistory.page ? 'forward' : 'backward');
    }
    eventHistory.setPage(nextPage);
  };
  const changeEventPageSize = (nextPageSize) => {
    setSelectedEventID(null);
    setEventPageDirection('replace');
    eventHistory.setPageSize(nextPageSize);
  };

  const columns = [
    {
      key: 'timestamp',
      title: '请求',
      type: 'time',
      // Keep the request summary wide enough for the macOS-style project /
      // client / agent context before it wraps or clamps.
      width: '280px',
      minWidth: '240px',
      sortable: true,
      render: (row) => <RecentEventRequestCell event={row} />,
    },
    {
      key: 'model',
      title: '模型 / 路由',
      type: 'text',
      width: '260px',
      minWidth: '220px',
      render: (row) => <RecentEventRouteCell event={row} />,
    },
    {
      key: 'outcome',
      title: '结果',
      type: 'status',
      width: '220px',
      minWidth: '190px',
      render: (row) => <RecentEventResultCell event={row} />,
    },
    {
      key: 'message',
      title: '说明',
      type: 'text',
      width: '280px',
      minWidth: '240px',
      render: (row) => <RecentEventMessageCell event={row} />,
    },
  ];

  return (
      <div className="page-stack">
      {/* Page Header */}
      <div className="page-header">
        <div className="page-title-group">
          <h1 className="page-title">
            <Icon name="play" size={24} style={{ color: 'var(--primary)' }} />
            <span>运行</span>
            <StatusBadge status={status} />
          </h1>
          <p className="page-subtitle">启动状态、监听地址、统计和最近事件。</p>
        </div>
        <div className="page-actions">
          <button
            type="button"
            className={`btn ${isRunning ? 'btn-danger' : 'btn-primary'}`}
            onClick={toggleProxy}
          >
            <Icon name={isRunning ? 'stop' : 'play'} size={16} />
            <span>{isRunning ? '停止运行' : '启动运行'}</span>
          </button>
        </div>
      </div>

      {/* Last Error Banner if present */}
      {status?.lastError && (
        <div
            className="run-error-banner"
        >
          <Icon name="warning" size={20} />
          <div>
            <strong>最近错误告警：</strong> {status.lastError}
          </div>
        </div>
      )}

      {/* Hero Status & Routing Summary */}
      <div className="run-hero-grid run-hero-grid-single">
        {/* Status Hero */}
        <div className="glass-panel run-hero-panel">
          <div className="panel-toolbar">
            <span className="run-hero-kicker">
              服务监听状态
            </span>
            <span className="mono-cell run-hero-version">
              v{status?.version || '0.2.0'} · Generation: {status?.generation || '-'}
            </span>
          </div>

          <div className="run-listener-row">
            <div className="mono-cell run-listener-address">
              {status?.listener ? `${status.listener.host}:${status.listener.port}` : '127.0.0.1:57878'}
            </div>
            <button
              type="button"
              className="btn btn-ghost run-copy-address"
              onClick={() => copyText(`http://${status?.listener?.host || '127.0.0.1'}:${status?.listener?.port || 57878}`, '监听地址')}
            >
              <Icon name="copy" size={14} />
              <span>复制地址</span>
            </button>
          </div>

          <p className="run-hero-description">
            {status?.health?.headline || (isRunning ? '代理服务正常响应客户端请求' : '服务当前处于休眠停止状态')}
          </p>

          {/* Detailed Info Matrix matching macOS */}
          <div className="grid-2col run-detail-grid">
            <div>
              <span>配置版本：</span>
              <strong>v{config?.schemaVersion || 6}</strong>
            </div>
            <div>
              <span>认证状态：</span>
              <strong className={secretStatus?.inboundAuthToken?.configured ? 'run-detail-good' : undefined}>
                {secretStatus?.inboundAuthToken?.configured ? `已启用 (尾号 ${secretStatus.inboundAuthToken.last4})` : '未启用 (本机免密)'}
              </strong>
            </div>
            <div>
              <span>运行时长：</span>
              <strong className="mono-cell">
                {status?.uptimeSeconds ? formatDuration(status.uptimeSeconds * 1000) : '-'}
              </strong>
            </div>
            <div>
              <span>调度策略：</span>
              <strong>优先级 + 会话粘性绑定</strong>
            </div>
          </div>
        </div>

      </div>

      {/* Primary cumulative and recent usage cards */}
      <div className="grid-4col run-metric-grid">
        <MetricCard
          label="输入 Token"
          value={tokenTotals.input == null ? '—' : formatTokenCount(tokenTotals.input)}
          detail={tokenTotals.observed ? `最近事件内累计 · ${formatNumber(tokenTotals.observed)} 个请求有用量` : '暂无可用用量'}
          icon="chart"
          accent="var(--accent-indigo)"
        />
        <MetricCard
          label="输出 Token"
          value={tokenTotals.output == null ? '—' : formatTokenCount(tokenTotals.output)}
          detail={tokenTotals.observed ? `最近事件内累计 · ${formatNumber(tokenTotals.observed)} 个请求有用量` : '暂无可用用量'}
          icon="sparkles"
          accent="var(--accent-purple)"
        />
        <MetricCard
          label="Provider 候选"
          value={formatNumber(status?.providers ?? config?.endpoints?.length ?? 0)}
          detail="按优先级形成候选序列"
          icon="route"
          accent="var(--primary)"
        />
        <MetricCard
          label="Endpoints 上游入口"
          value={formatNumber(status?.endpoints ?? config?.endpoints?.length ?? 0)}
          detail="可独立配置协议与模型映射"
          icon="server"
          accent="var(--accent-indigo)"
        />
        <MetricCard
          label="客户端请求"
          value={formatNumber(runtime?.clientRequests || 0)}
          detail={`端到端 · 成功 ${formatNumber(runtime?.clientSuccesses || 0)} / 失败 ${formatNumber(runtime?.clientFailures || 0)}`}
          icon="activity"
          accent="var(--status-good)"
        />
        <MetricCard
          label="上游尝试"
          value={formatNumber(runtime?.upstreamAttempts || 0)}
          detail={`含重试 · 故障转移 ${formatNumber(runtime?.failovers || 0)} 次`}
          icon="route"
          accent="var(--accent-purple)"
        />
      </div>

      {/* Real-time Streaming Telemetry Event Log */}
      <div className="glass-panel">
        <div className="panel-header">
          <div className="panel-title-group">
            <div className="panel-title">
              <Icon name="activity" size={18} style={{ color: 'var(--primary)' }} />
              <span>最近事件与遥测日志</span>
            </div>
            <span className="panel-hint">
              客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。
            </span>
          </div>

          <div className="telemetry-toolbar">
            <div className="telemetry-filter-group" role="group" aria-label="事件类型筛选">
              {[
                { id: 'client', label: '客户端' },
                { id: 'upstream', label: '上游' },
                { id: 'all', label: '全部' },
              ].map((tab) => (
                <button
                  key={tab.id}
                  type="button"
                  className={`telemetry-filter-button${eventFilter === tab.id ? ' active' : ''}`}
                  onClick={() => {
                    setSelectedEventID(null);
                    setEventPageDirection('replace');
                    setEventFilter(tab.id);
                  }}
                  aria-pressed={eventFilter === tab.id}
                >
                  {tab.label}
                </button>
              ))}
            </div>
            <span className="mono-cell" style={{ fontSize: '0.78rem', color: 'var(--text-muted)' }}>
              {eventHistory.mode === 'page'
                ? `第 ${formatNumber(eventHistory.page)} 页 · 每页 ${formatNumber(eventHistory.pageSize)} · 共 ${formatNumber(eventHistory.totalCount)} 条`
                : `${formatNumber(visibleEvents.length)} 条持久事件${liveEvents.length ? ` · ${formatNumber(liveEvents.length)} 条进行中` : ''}`}
            </span>
            <button
              type="button"
              className="btn btn-ghost telemetry-sort-button"
              onClick={() => {
                setEventSort((current) => (current === 'desc' ? 'asc' : 'desc'));
              }}
              aria-label={`切换当前页事件时间排序，当前为${eventSort === 'desc' ? '最新优先' : '最早优先'}`}
              title="按当前页事件时间稳定排序"
            >
              <Icon name="sort" size={14} />
              <span>{eventHistory.mode === 'page' ? '本页' : ''}{eventSort === 'desc' ? '最新优先' : '最早优先'}</span>
            </button>
          </div>
        </div>

        <div className="panel-body" style={{ padding: 0 }}>
          {eventHistory.error && (
            <div className="event-page-error" role="alert">
              <span>读取事件历史失败：{eventHistory.error}</span>
              <button type="button" className="btn btn-secondary" onClick={eventHistory.retry}>重试</button>
            </div>
          )}
          <div
            className={`event-page-stage event-page-stage-${eventHistory.loading ? 'loading' : 'ready'}`}
            data-page={eventHistory.page}
            data-page-direction={eventPageDirection}
            aria-busy={eventHistory.loading}
          >
            <div className="event-page-content">
              <div className="telemetry-live-mobile">
                <LiveEventList
                  events={liveEvents}
                  selectedEventID={selectedEvent?.id}
                  onSelect={setSelectedEventID}
                />
              </div>
              <div className="telemetry-desktop-table">
                <DataTable
                  className="telemetry-table"
                  columns={columns}
                  data={visibleEvents}
                  keyField="id"
                  ariaLabel={eventHistory.mode === 'page' ? 'SQLite 持久事件分页列表' : '最近请求事件列表'}
                  tableMinWidth="1040px"
                  sortKey="timestamp"
                  sortDirection={eventSort}
                  onSortChange={(_key, direction) => setEventSort(direction)}
                  onRowClick={(row) => setSelectedEventID(row.id)}
                  activeRowKey={selectedEvent?.id}
                  beforeTable={liveEvents?.length ? (
                    <LiveEventList
                      events={liveEvents}
                      selectedEventID={selectedEvent?.id}
                      onSelect={setSelectedEventID}
                    />
                  ) : null}
                  emptyText={eventHistory.loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}
                />
              </div>
              <MobileEventList
                events={visibleEvents}
                selectedEventID={selectedEvent?.id}
                onSelect={setSelectedEventID}
                loading={eventHistory.loading}
              />
              {!selectedEvent && (visibleEvents.length > 0 || liveEvents.length > 0) && (
                <div className="telemetry-selection-hint" role="status">选择一条事件查看请求链路与诊断详情</div>
              )}
            </div>
            {eventHistory.loading && (
              <div className="event-page-loading" role="status" aria-live="polite">
                <span className="loading-dot" aria-hidden="true" />
                <span>正在读取第 {formatNumber(eventHistory.page)} 页…</span>
              </div>
            )}
          </div>
          {eventHistory.mode === 'page' ? (
            <PaginationBar
              page={eventHistory.page}
              pageSize={eventHistory.pageSize}
              totalCount={eventHistory.totalCount}
              totalPages={eventHistory.totalPages}
              itemCount={visibleEvents.length}
              liveItemCount={eventHistory.liveEventCount}
              loading={eventHistory.loading}
              onPageChange={changeEventPage}
              onPageSizeChange={changeEventPageSize}
              ariaLabel="持久事件分页"
            />
          ) : eventHistory.mode === 'legacy' ? (
            <div className="event-page-compatibility" role="status">
              当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。
            </div>
          ) : null}
        </div>

        {/* Master-Detail Request Chain & Full Technical Inspector (macOS Parity) */}
        {selectedEvent && (
          <div
            className="request-detail-grid"
            style={{
              padding: 'clamp(16px, 4vw, 24px)',
              borderTop: '1px solid var(--border-subtle)',
              background: 'var(--bg-surface-glass)',
              display: 'grid',
              gap: 'clamp(16px, 4vw, 24px)',
            }}
          >
            {/* Left: Request Chain Visualization */}
            <div className="request-chain-pane" style={{ display: 'flex', flexDirection: 'column', gap: '12px' }}>
              <div className="request-chain-header">
                <strong style={{ fontSize: '0.95rem', color: 'var(--text-primary)' }}>完整请求链路 (Trace)</strong>
                <span className="mono-cell" style={{ fontSize: '0.75rem', color: 'var(--text-muted)' }}>
                  {requestChain.filter((e) => e.kind === 'upstream').length} 次上游尝试
                </span>
              </div>

              <div style={{ display: 'flex', flexDirection: 'column', gap: '8px' }}>
                {requestChain.map((ev, idx) => {
                  const isCur = ev.id === selectedEvent.id;
                  const isClient = ev.kind === 'client';
                  const upstreamIndex = requestChain
                    .slice(idx + 1)
                    .filter((candidate) => candidate.kind === 'upstream').length + 1;
                  const tools = eventToolCalls(ev);
                  return (
                    <div
                      key={ev.id}
                      onClick={() => setSelectedEventID(ev.id)}
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' || event.key === ' ') {
                          event.preventDefault();
                          setSelectedEventID(ev.id);
                        }
                      }}
                      role="button"
                      tabIndex={0}
                      aria-pressed={isCur}
                      aria-label={`查看${isClient ? '客户端请求' : `第 ${upstreamIndex} 次上游尝试`}详情`}
                      style={{
                        padding: '10px 12px',
                        borderRadius: 'var(--radius-md)',
                        background: isCur ? 'var(--bg-active)' : 'var(--bg-surface-elevated)',
                        border: `1px solid ${isCur ? 'var(--primary)' : 'var(--border-subtle)'}`,
                        cursor: 'pointer',
                        display: 'flex',
                        flexDirection: 'column',
                        gap: '4px',
                      }}
                    >
                      <div className="request-chain-card-header">
                        <span style={{ fontSize: '0.78rem', fontWeight: 700, color: isClient ? 'var(--primary)' : 'var(--accent-purple)' }}>
                          {isClient ? '端到端客户端请求' : `上游尝试 #${upstreamIndex}`}
                        </span>
                        <StatusBadge
                          text={eventOutcomeLabel(ev)}
                          kind={statusKind(ev)}
                        />
                      </div>
                      <div className="mono-cell" style={{ fontSize: '0.75rem', color: 'var(--text-primary)', fontWeight: 600 }}>
                        {eventEndpoint(ev)}
                      </div>
                      <div style={{ fontSize: '0.72rem', color: 'var(--text-muted)' }}>
                        {eventStatusDetailLabel(ev)} · 耗时: {eventDurationText(ev)} · {eventModel(ev)}
                      </div>
                      <div style={{ fontSize: '0.72rem', color: eventOutcome(ev) === 'failed' ? 'var(--status-critical)' : 'var(--text-muted)' }}>
                        {eventFailover(ev) ? '故障转移 · ' : ''}{eventEndpointID(ev)} · {eventUpstreamRequestID(ev) !== '-' ? `上游请求 ${eventUpstreamRequestID(ev)}` : '上游请求 ID -'}
                        {tools.length ? ` · 工具 ${tools.join(', ')}` : ''}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>

            {/* Right: Progressive event details: core first, forensic fields on demand. */}
            <div className="event-inspector" style={{ display: 'flex', flexDirection: 'column', gap: '14px' }}>
              <div className="event-inspector-header">
                <strong style={{ fontSize: '0.95rem', color: 'var(--text-primary)' }}>
                  选中事件详情
                </strong>
                <button
                  type="button"
                  className="btn btn-ghost"
                  style={{ padding: '2px 8px', fontSize: '0.78rem' }}
                  onClick={() => copyText(JSON.stringify(selectedEvent, null, 2), '完整事件 JSON')}
                >
                  <Icon name="copy" size={13} />
                  <span>复制 JSON</span>
                </button>
              </div>

              <div className="grid-3col event-core-summary" data-event-detail-tier="primary" style={{ fontSize: '0.82rem', gap: '10px' }}>
                <div><span style={{ color: 'var(--text-muted)' }}>时间：</span><span className="mono-cell">{formatTimestamp(selectedEvent.timestamp, { date: true })}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>事件类型：</span><span className="mono-cell">{eventKindLabel(selectedEvent.kind)}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>入站客户端：</span><strong style={{ color: 'var(--text-primary)' }}>{eventClientKindLabel(selectedEvent)}</strong></div>
                <div><span style={{ color: 'var(--text-muted)' }}>请求用途：</span><strong style={{ color: 'var(--primary)' }}>{eventPurposeLabel(selectedEvent)}</strong></div>
                {eventCodexThreadClass(selectedEvent) && <div><span style={{ color: 'var(--text-muted)' }}>功能线程：</span><strong>{codexThreadClassLabel(eventCodexThreadClass(selectedEvent))}</strong></div>}
                {eventField(selectedEvent, 'attributionScope', 'attribution_scope') && <div><span style={{ color: 'var(--text-muted)' }}>归因范围：</span><strong>{attributionScopeLabel(eventField(selectedEvent, 'attributionScope', 'attribution_scope'))}</strong></div>}
                <div><span style={{ color: 'var(--text-muted)' }}>生命周期：</span><span className="mono-cell">{eventPhaseLabel(selectedEvent)}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>客户端模型：</span><span className="mono-cell">{eventField(selectedEvent, 'clientModel', 'client_model') || '-'}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>路由/逻辑模型：</span><span className="mono-cell" style={{ color: 'var(--status-good)' }}>{eventField(selectedEvent, 'effectiveModel', 'effective_model') || '-'}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>实际上游模型：</span><span className="mono-cell">{eventUpstreamModel(selectedEvent)}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>入口名称：</span><span>{eventEndpointName(selectedEvent)}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>HTTP 状态：</span><strong className="mono-cell" style={{ color: selectedResult === 'failed' ? 'var(--status-critical)' : 'var(--text-primary)' }}>{eventHttpStatusLabel(selectedEvent)}</strong></div>
                <div><span style={{ color: 'var(--text-muted)' }}>最终结果：</span><strong style={{ color: selectedResult === 'failed' ? 'var(--status-critical)' : selectedResult === 'succeeded' ? 'var(--status-good)' : 'var(--text-secondary)' }}>{eventOutcomeLabel(selectedEvent)}</strong></div>
                <div><span style={{ color: 'var(--text-muted)' }}>故障转移：</span><strong style={{ color: eventFailover(selectedEvent) ? 'var(--status-warning)' : 'var(--text-secondary)' }}>{eventFailover(selectedEvent) ? '已发生' : '未发生'}</strong></div>
                <div><span style={{ color: 'var(--text-muted)' }}>首字节延迟 (TTFB)：</span><span className="mono-cell">{formatDuration(eventTTFBMS(selectedEvent))}</span></div>
                <div><span style={{ color: 'var(--text-muted)' }}>总耗时：</span><span className="mono-cell">{formatDuration(eventDurationMS(selectedEvent))}</span></div>
              </div>

              <div className="event-project-context" data-event-detail-tier="primary">
                {selectedGrokMetadata && (
                  <>
                    {grokMetadataField(selectedGrokMetadata, 'clientIdentifier', 'client_identifier') && (
                      <div>
                        <span>Grok 客户端</span>
                        <strong className="mono-cell">{[
                          grokMetadataField(selectedGrokMetadata, 'clientIdentifier', 'client_identifier'),
                          grokMetadataField(selectedGrokMetadata, 'clientVersion', 'client_version'),
                          grokMetadataField(selectedGrokMetadata, 'clientMode', 'client_mode'),
                        ].filter(Boolean).join(' · ')}</strong>
                      </div>
                    )}
                    {grokMetadataField(selectedGrokMetadata, 'sessionID', 'session_id') && (
                      <div>
                        <span>Grok 会话</span>
                        <strong className="mono-cell">{grokMetadataField(selectedGrokMetadata, 'sessionID', 'session_id')}</strong>
                      </div>
                    )}
                    {grokMetadataField(selectedGrokMetadata, 'convID', 'conv_id') && (
                      <div>
                        <span>Grok 对话</span>
                        <strong className="mono-cell">{grokMetadataField(selectedGrokMetadata, 'convID', 'conv_id')}</strong>
                      </div>
                    )}
                  </>
                )}
                {selectedCodexHasIdentity && (
                  <>
                    <div>
                      <span>代理身份</span>
                      <strong>{codexAgentRoleLabel(selectedCodexMetadata)}</strong>
                    </div>
                    <div>
                      <span>代理路径</span>
                      <strong className="mono-cell">{codexAgentPath(selectedCodexMetadata)}</strong>
                    </div>
                  </>
                )}
                <div>
                  <span>项目 / 工作区</span>
                  <strong>{selectedProjectContext.label}</strong>
                </div>
                <div>
                  <span>项目来源</span>
                  <strong>{selectedProjectContext.applicable ? projectSourceLabel(selectedProjectContext.source) : '不适用'}</strong>
                </div>
                {selectedSourceProject && (
                  <div>
                    <span>源项目</span>
                    <strong className="mono-cell">{selectedSourceProject}</strong>
                  </div>
                )}
                {selectedSourceWorkspace && (
                  <div>
                    <span>源工作区</span>
                    <strong className="mono-cell">{selectedSourceWorkspace}</strong>
                  </div>
                )}
                {selectedCodexHasIdentity && (
                  <div className="event-project-context-meta">
                    {codexWorkspaceEntries(selectedCodexMetadata).map((workspace) => (
                      <span key={workspace.path} className="mono-cell">
                        {workspace.remote || workspace.path}
                      </span>
                    ))}
                  </div>
                )}
              </div>

              <details className="event-secondary-details" data-event-detail-tier="secondary">
                <summary>路由与协议详情</summary>
                <div className="grid-3col event-secondary-grid" style={{ fontSize: '0.82rem', gap: '10px' }}>
                  <div><span style={{ color: 'var(--text-muted)' }}>命中特征规则：</span><span className="mono-cell">{eventFeatureRuleID(selectedEvent)}</span></div>
                  <div><span style={{ color: 'var(--text-muted)' }}>上游 Host：</span><span className="mono-cell">{eventUpstreamHost(selectedEvent)}</span></div>
                  <div><span style={{ color: 'var(--text-muted)' }}>上游 HTTP 状态：</span><span className="mono-cell">{eventUpstreamStatusLabel(selectedEvent)}</span></div>
                  <div>
                    <span style={{ color: 'var(--text-muted)' }}>协议路由：</span>
                    <span className="mono-cell" style={{ color: 'var(--text-secondary)' }}>{eventProtocolRouteLabel(selectedEvent)}</span>
                  </div>
                </div>
              </details>

              <details className="event-diagnostics-details" data-event-detail-tier="diagnostics">
                <summary>失败、工具与流诊断</summary>
                <div className="event-technical-details" style={{ display: 'flex', flexDirection: 'column', gap: '8px', paddingTop: '10px', fontSize: '0.82rem' }}>
                  <div>
                    <span style={{ color: 'var(--text-muted)' }}>失败原因 / 阶段：</span>
                    <span className="technical-inline-value" style={{ color: selectedResult === 'failed' ? 'var(--status-critical)' : 'var(--text-secondary)', fontWeight: selectedResult === 'failed' ? 600 : 400 }}>{eventFailureSummaryLabel(selectedEvent)}</span>
                  </div>
                  <div>
                    <span style={{ color: 'var(--text-muted)' }}>实际工具调用：</span>
                    <span className="mono-cell" style={{ color: 'var(--accent-cyan)' }}>{eventToolCallsLabel(selectedEvent)}</span>
                  </div>

                  <div>
                    <span className="event-technical-section-label">流诊断信息</span>
                    <StreamTraceRows event={selectedEvent} />
                  </div>

                  {eventFailureDetail(selectedEvent) && (
                    <div>
                      <span style={{ color: 'var(--text-muted)' }}>技术详情：</span>
                      <pre
                        className="mono-cell technical-pre"
                        style={{
                          color: 'var(--status-critical)',
                          fontSize: '0.78rem',
                        }}
                      >
                        {eventFailureDetail(selectedEvent)}
                      </pre>
                    </div>
                  )}

                  <div>
                    <span style={{ color: 'var(--text-muted)' }}>原始引擎消息：</span>
                    <span className="technical-inline-value" style={{ color: 'var(--text-secondary)' }}>{eventMessage(selectedEvent) || '无'}</span>
                  </div>

                  {eventRequestID(selectedEvent) !== '-' && (
                    <div className="request-id-row">
                      <span style={{ color: 'var(--text-muted)' }}>Request ID：</span>
                      <span className="mono-cell">{eventRequestID(selectedEvent)}</span>
                      <button
                        type="button"
                        className="btn btn-ghost"
                        style={{ padding: '2px 6px', fontSize: '0.72rem' }}
                        onClick={() => copyText(eventRequestID(selectedEvent), 'Request ID')}
                        aria-label="复制 Request ID"
                      >
                        <Icon name="copy" size={11} />
                      </button>
                    </div>
                  )}
                  {eventSessionID(selectedEvent) !== '-' && (
                    <div><span style={{ color: 'var(--text-muted)' }}>会话 ID：</span><span className="mono-cell">{eventSessionID(selectedEvent)}</span></div>
                  )}
                  <div><span style={{ color: 'var(--text-muted)' }}>事件 ID：</span><span className="mono-cell">{selectedEvent.id}</span></div>
                  <div><span style={{ color: 'var(--text-muted)' }}>入口 ID：</span><span className="mono-cell">{eventEndpointID(selectedEvent)}</span></div>
                  <div><span style={{ color: 'var(--text-muted)' }}>上游请求 ID：</span><span className="mono-cell">{eventUpstreamRequestID(selectedEvent)}</span></div>
                  <div><span style={{ color: 'var(--text-muted)' }}>超时阈值：</span><span className="mono-cell">{eventTimeoutMS(selectedEvent) ? formatDuration(eventTimeoutMS(selectedEvent)) : '默认 / 未上报'}</span></div>
                </div>
              </details>

              {selectedGrokMetadata && (
                <GrokMetadataDetails metadata={selectedGrokMetadata} onCopy={copyText} />
              )}
              {selectedCodexHasIdentity && (
                <CodexMetadataDetails metadata={selectedCodexMetadata} onCopy={copyText} />
              )}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
