import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { MetricCard } from '../components/MetricCard.jsx';
import { LiveSurfaceLight } from '../components/LiveSurfaceLight.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { PaginationBar } from '../components/PaginationBar.jsx';
import { useRuntimeEventPage } from '../hooks/useRuntimeEventPage.js';
import { Icon } from '../utils/icons.jsx';
import { orderRuntimeEvents, mergeRuntimeEvent } from '../utils/runtimeEvents.js';
import { EventInspector } from '../components/EventInspector.jsx';
import { CacheHitRate } from '../components/CacheHitRate.jsx';
import { eventAgentLabel, eventCacheLabel, eventHasObservedUsage, eventUsageLabel, eventUsage } from '../utils/eventPresentation.js';
import { copyWithToast } from '../utils/clipboard.js';
import { api } from '../services/api.js';
import {
  eventEndpointName, formatNumber, formatTokenCount, formatDuration, formatTimestamp, eventLogicalModel, eventEndpoint, statusKind, eventDurationText, friendlyEventMessage, eventPurposeLabel, eventClientKindLabel, eventKindLabel, getRequestChain, eventOutcomeLabel, eventPhaseLabel, eventHttpStatusLabel, eventHTTPStatusCode, eventRequestID, eventTTFBMS, eventOutcome, eventFailover, eventField, eventIsInFlight, eventFailureSummaryLabel, eventResultKind, eventProjectContext
} from '../utils/helpers.js';

function recentTokenTotals(events) {
  const usages = events
    .filter((event) => event.kind === 'client' && !eventIsInFlight(event) && eventResultKind(event) !== 'cancelled')
    .map(eventUsage)
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
    if (context.source === 'workspace_local' && context.localUser) {
      return `${context.name} 本地(${context.localUser})`;
    }
    const declared = context.source === 'client_declared' ? '（客户端声明）' : '';
    return `项目: ${context.name}${declared}`;
  }
  return context.label;
}

// 列表行只显示已记录的用途，让分流请求（如自动模式分类器）在列表里就能被认出来；
// 未记录时返回 null 不占位，缺失原因留给详情面板解释。
function recentEventPurposeLabel(event) {
  const raw = eventField(event, 'requestPurpose');
  const key = typeof raw === 'object' ? (raw?.kind || raw?.type || raw?.name) : raw;
  if (!key) return null;
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
  if (eventIsInFlight(event)) return '进行中';
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
  return [recentEventProjectSummary(event), eventKindLabel(event.kind), eventClientKindLabel(event),
    eventAgentLabel(event) ? `代理: ${eventAgentLabel(event)}` : null,
    recentEventPurposeLabel(event)].filter(Boolean).join(' · ');
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
  const project = recentEventProjectSummary(event);
  return (
    <span className={live ? 'telemetry-live-request' : 'telemetry-primary-cell'}>
      <span className={`mono-cell${live ? ' telemetry-live-time' : ' telemetry-event-time'}`}>
        {formatTimestamp(event.timestamp)}
      </span>
      {project && <span className="telemetry-event-meta" title={project}>{project}</span>}
      <span className={`telemetry-event-meta${live ? ' telemetry-live-request-summary' : ''}`} title={summary}>
        {[eventKindLabel(event.kind), eventClientKindLabel(event), eventAgentLabel(event), recentEventPurposeLabel(event)]
          .filter(Boolean).join(' · ')}
      </span>
    </span>
  );
}

function RecentEventRouteCell({ event, live = false }) {
  if (event.kind === 'notify') return <span>{event.hookEvent || '通知'}</span>;
  return (
    <span className={live ? 'telemetry-live-route' : 'telemetry-cell-stack telemetry-route-cell'}>
      <strong className="telemetry-event-model">{eventLogicalModel(event)}</strong>
      <span className="telemetry-route-meta">{eventEndpointName(event)}</span>
    </span>
  );
}

function RecentEventResultCell({ event, live = false }) {
  if (event.kind === 'notify') return <span className="telemetry-cell-stack">通知事件</span>;
  return (
    <span className={live ? 'telemetry-live-result' : 'telemetry-cell-stack telemetry-result-cell'}>
      <RecentEventOutcome event={event} />
      <span className="telemetry-result-detail" title={recentEventStatusDetail(event)}>
        {eventHTTPStatusCode(event) ? eventHttpStatusLabel(event) : 'HTTP —'}
      </span>
    </span>
  );
}

function RecentEventDurationCell({ event }) {
  if (event.kind === 'notify') return <span className="telemetry-duration-cell">—</span>;
  const slowTTFB = eventTTFBMS(event) >= 5000;
  return (
    <span className={`mono-cell telemetry-duration-cell${slowTTFB ? ' is-slow' : ''}`}
      title={slowTTFB ? '首字节 → 总耗时；首字节超过 5 秒，上游可能排队中' : '首字节 → 总耗时'}>
      {eventDurationText(event)}
    </span>
  );
}

function RecentEventUsageCell({ event }) {
  if (event.kind === 'notify') return <span className="telemetry-usage-cell is-empty">—</span>;
  const observed = eventHasObservedUsage(event);
  const live = eventIsInFlight(event);
  return (
    <span className={`telemetry-usage-cell${observed ? '' : ' is-empty'}`}
      title={live ? '仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。' : '输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。'}>
      <span className="telemetry-usage-tokens">{eventUsageLabel(event)}</span>
      <span className="telemetry-usage-secondary">
        <span>{live && !observed ? '上游尚未返回' : eventCacheLabel(event)}</span>
        {(!live || observed) && <CacheHitRate event={event} />}
        {live && observed && <small className="telemetry-usage-provisional">暂计</small>}
      </span>
    </span>
  );
}

function RecentEventMessageCell({ event, live = false }) {
  const friendly = friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : '');
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

function LiveEventList({ events, selectedEventID, onSelect }) {
  if (!events?.length) return null;
  return (
    <section className="telemetry-live-group" aria-label={`${formatNumber(events.length)} 个进行中请求`}>
      <LiveSurfaceLight />
      <div className="telemetry-live-heading">
        <span><i aria-hidden="true" />进行中 · {formatNumber(events.length)}</span>
        <small>用量为上游暂计</small>
      </div>
      <div className="telemetry-live-columns" aria-hidden="true">
        {['请求', '模型 / 路由', '结果', '首字节 → 总耗时', 'Token 用量', '事件状态'].map((label) => <span key={label}>{label}</span>)}
      </div>
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
                <RecentEventDurationCell event={event} />
                <RecentEventUsageCell event={event} />
                <RecentEventMessageCell event={event} live />
              </button>
            </div>
          );
        })}
      </div>
    </section>
  );
}

function MobileEventList({ events, selectedEventID, onSelect, loading, live = false }) {
  if (!events?.length) {
    if (live) return null;
    return (
      <div className="responsive-data-card-list telemetry-mobile-list">
        <div className="responsive-card-empty">{loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}</div>
      </div>
    );
  }

  return (
    <div className="responsive-data-card-list telemetry-mobile-list" data-live={live} role="list" aria-label={live ? '进行中请求列表' : '请求事件列表'}>
      {live && <LiveSurfaceLight />}
      {events.map((event) => {
        const projectContext = eventProjectContext(event);
        const outcome = eventOutcome(event);
        const slowTTFB = eventTTFBMS(event) >= 5000;
        const friendly = friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : '');
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
                title="首字节 → 总耗时"
              >
                {eventDurationText(event)}
              </span>
            </div>
            <div className="telemetry-mobile-route">
              <strong className="mono-cell">{event.kind === 'notify' ? (event.hookEvent || '通知') : eventLogicalModel(event)}</strong>
              {event.kind !== 'notify' && <small>{eventEndpointName(event)}</small>}
            </div>
            <div className="telemetry-mobile-usage">
              <span className="telemetry-mobile-status">{recentEventStatusDetail(event)}</span>
              {event.kind !== 'notify' && <RecentEventUsageCell event={event} />}
            </div>
            <div className="telemetry-mobile-context">
              <span className="telemetry-mobile-request-summary" title={recentEventRequestSummary(event)}>
                {recentEventRequestSummary(event) || projectContext.label}
              </span>
              {friendly && (
                <p className={`telemetry-mobile-message${outcome === 'failed' ? ' is-error' : eventFailover(event) ? ' is-warning' : ''}`}>
                  <span className="telemetry-state-label">事件状态</span>
                  {eventFailover(event) && <strong>故障转移 · </strong>}
                  {friendly}
                </p>
              )}
            </div>
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
  const [expandedGroups, setExpandedGroups] = useState({});
  const toggleGroup = (key, open) => setExpandedGroups((previous) => previous[key] === open ? previous : { ...previous, [key]: open });
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
    for (const event of [...recentEvents, ...eventHistory.events]) byID.set(event.id, mergeRuntimeEvent(byID.get(event.id), event));
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
  const selectedChangeSeq = Number(baseEventContext.find((event) => event.id === currentSelectedID)?.changeSeq || 0);
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
    loadRuntimeEvent(currentSelectedID, selectedChangeSeq).catch((error) => {
      addToast(`加载事件详情失败: ${error.message}`, 'error');
    });
  }, [addToast, currentSelectedID, selectedChangeSeq, loadRuntimeEvent]);

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
      width: '220px',
      minWidth: '220px',
      sortable: true,
      render: (row) => <RecentEventRequestCell event={row} />,
    },
    {
      key: 'model',
      title: '模型 / 路由',
      type: 'text',
      width: '180px',
      minWidth: '180px',
      render: (row) => <RecentEventRouteCell event={row} />,
    },
    {
      key: 'outcome',
      title: '结果',
      type: 'status',
      width: '104px',
      minWidth: '104px',
      render: (row) => <RecentEventResultCell event={row} />,
    },
    {
      key: 'duration',
      title: '首字节 → 总耗时',
      type: 'text',
      width: '156px',
      minWidth: '156px',
      render: (row) => <RecentEventDurationCell event={row} />,
    },
    {
      key: 'usage',
      title: 'Token 用量',
      type: 'text',
      width: '220px',
      minWidth: '220px',
      render: (row) => <RecentEventUsageCell event={row} />,
    },
    {
      key: 'message',
      title: '事件状态',
      type: 'text',
      width: '220px',
      minWidth: '220px',
      render: (row) => <RecentEventMessageCell event={row} />,
    },
  ];

  return (
      <div className="page-stack run-page">
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
          <div className="run-detail-grid">
            <div>
              <span>配置版本：</span>
              <strong>v{config?.schemaVersion || 7}</strong>
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
      <div className="run-metric-grid">
        <MetricCard
          label="输入 Token"
          value={tokenTotals.input == null ? '—' : formatTokenCount(tokenTotals.input)}
          detail={tokenTotals.observed ? `最近事件内累计 · ${formatNumber(tokenTotals.observed)} 个请求有用量` : '暂无可用用量'}
          icon="file-down"
          accent="var(--primary)"
        />
        <MetricCard
          label="输出 Token"
          value={tokenTotals.output == null ? '—' : formatTokenCount(tokenTotals.output)}
          detail={tokenTotals.observed ? `最近事件内累计 · ${formatNumber(tokenTotals.observed)} 个请求有用量` : '暂无可用用量'}
          icon="file-up"
          accent="var(--status-warning)"
        />
        <MetricCard
          label="可调度入口"
          value={formatNumber(status?.endpoints ?? config?.endpoints?.length ?? 0)}
          detail="按优先级形成 Provider 候选序列"
          icon="git-fork"
          accent="var(--primary)"
        />
        <MetricCard
          label="客户端请求"
          value={formatNumber(runtime?.clientRequests || 0)}
          detail={`端到端 · 成功 ${formatNumber(runtime?.clientSuccesses || 0)} / 失败 ${formatNumber(runtime?.clientFailures || 0)}`}
          icon="move-diagonal"
          accent="var(--primary)"
        />
        <MetricCard
          label="上游尝试"
          value={formatNumber(runtime?.upstreamAttempts || 0)}
          detail={`含重试 · 故障转移 ${formatNumber(runtime?.failovers || 0)}`}
          icon="network"
          accent="var(--primary)"
        />
      </div>

      {/* Real-time Streaming Telemetry Event Log */}
      <div className="glass-panel run-events-panel">
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
            {eventHistory.mode !== 'page' && <span className="panel-hint">
              {formatNumber(visibleEvents.length)} 条持久事件
            </span>}
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
            {eventHistory.mode === 'page' ? (
              <PaginationBar
                compact
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
            ) : null}
            <div className="event-page-content">
              {liveEvents.length > 0 && <div className="telemetry-live-section">
                <div className="telemetry-live-desktop">
                  <LiveEventList events={liveEvents} selectedEventID={selectedEvent?.id} onSelect={setSelectedEventID} />
                </div>
                <div className="telemetry-live-mobile">
                  <div className="telemetry-live-heading">
                    <span><i aria-hidden="true" />进行中 · {formatNumber(liveEvents.length)}</span>
                    <small>用量为上游暂计</small>
                  </div>
                <MobileEventList
                  events={liveEvents}
                  selectedEventID={selectedEvent?.id}
                  onSelect={setSelectedEventID}
                  live
                />
                </div>
              </div>}
              <section className="telemetry-history-group" aria-label="历史事件">
              <div className="telemetry-desktop-table">
                <DataTable
                  className="telemetry-table"
                  columns={columns}
                  data={visibleEvents}
                  keyField="id"
                  ariaLabel={eventHistory.mode === 'page' ? 'SQLite 持久事件分页列表' : '最近请求事件列表'}
                  tableMinWidth="1100px"
                  sortKey="timestamp"
                  sortDirection={eventSort}
                  onSortChange={(_key, direction) => setEventSort(direction)}
                  onRowClick={(row) => setSelectedEventID(row.id)}
                  activeRowKey={selectedEvent?.id}
                  emptyText={eventHistory.loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}
                />
              </div>
              <MobileEventList
                events={visibleEvents}
                selectedEventID={selectedEvent?.id}
                onSelect={setSelectedEventID}
                loading={eventHistory.loading}
              />
              </section>
              {!selectedEvent && (visibleEvents.length > 0 || liveEvents.length > 0) && (
                <div className="telemetry-selection-hint" role="status">选择一条事件查看请求链路与诊断详情</div>
              )}
            </div>

          </div>
          {eventHistory.mode === 'legacy' ? (
            <div className="event-page-compatibility" role="status">
              当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。
            </div>
          ) : null}
        </div>

        {selectedEvent && <EventInspector
          event={selectedEvent} chain={requestChain} onSelect={setSelectedEventID}
          requestSummary={recentEventRequestSummary(selectedEvent)}
          expanded={expandedGroups} onToggle={toggleGroup} onCopy={copyText}
          loading={runtimeEventDetail?.event?.id !== selectedEvent.id && selectedEvent.detailsOmitted === true}
        />}

      </div>
    </div>
  );
}
