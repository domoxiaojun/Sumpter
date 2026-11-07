import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { createRequestChainLoader, mergeRequestChain } from '../utils/requestChain.js';
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
import { eventAgentLabel, eventCacheLabel, eventHasObservedUsage, eventUsageLabel } from '../utils/eventPresentation.js';
import { copyWithToast } from '../utils/clipboard.js';
import { api } from '../services/api.js';
import { RUN_OVERVIEW_RANGE, runOverviewQuery, runOverviewTotals } from '../utils/runOverview.js';
import { observeRuntimeDay, runtimeTodayBounds } from '../utils/runtimeAnalyticsV2.js';
import { requestKindLabel, eventEndpointName, formatNumber, formatTokenCount, formatDuration, formatTimestamp, eventLogicalModel, statusKind, eventDurationText, eventDisplayTiming, friendlyEventMessage, eventPurposeLabel, eventClientKindLabel, eventKindLabel, getRequestChain, eventOutcomeLabel, eventPhaseLabel, eventHttpStatusLabel, eventHTTPStatusCode, eventRequestID, eventOutcome, eventFailover, eventField, eventIsInFlight, eventFailureSummaryLabel, eventProjectContext, eventToolCalls, eventTransportLabel } from '../utils/helpers.js';

function overviewTokenDetail(totals) {
  if (!totals) return '正在读取今天的用量';
  if (totals.status === 'error') return '今天用量暂不可用';
  return totals.observed > 0
    ? `今天 · ${formatNumber(totals.observed)} 个请求有用量`
    : '今天暂无可用用量';
}

const overviewCount = value => value == null ? '—' : formatNumber(value);

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
  const requestKind = eventField(event, 'requestKind');
  return requestKind != null ? requestKindLabel(requestKind) : eventPurposeLabel(event);
}

function recentEventOutcomeLabel(event) {
  if (String(event?.kind || '').toLowerCase() === 'notify' && !eventOutcome(event)) {
    return '不适用（通知事件）';
  }
  if (eventIsInFlight(event)) return '进行中';
  const outcome = eventOutcome(event);
  if (outcome === 'succeeded') return '成功';
  if (outcome === 'failed') return '失败';
  if (outcome === 'cancelled') return event.failureKind === 'client_disconnected' ? '连接中断' : '已取消';
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
    recentEventPurposeLabel(event), event.sourceIP].filter(Boolean).join(' · ');
}

function RecentEventOutcome({ event, compact = false }) {
  const kind = statusKind(event);
  const isLive = eventIsInFlight(event) || kind === 'live';
  return (
    <span className={`telemetry-outcome-text ${kind}${compact ? ' is-compact' : ''}`}>
      {isLive && <i className="telemetry-live-pill-dot" aria-hidden="true" />}
      {recentEventOutcomeLabel(event)}
    </span>
  );
}

function RecentEventRequestCell({ event, live = false }) {
  const summary = recentEventRequestSummary(event);
  const project = recentEventProjectSummary(event);
  const tools = event.kind === 'notify' ? [] : eventToolCalls(event);
  return (
    <span className={live ? 'telemetry-live-request' : 'telemetry-primary-cell'}>
      <span className="telemetry-event-head-row">
        <span className={`mono-cell${live ? ' telemetry-live-time' : ' telemetry-event-time'}`}>
          {formatTimestamp(event.timestamp)}
        </span>
        {project && <span className="telemetry-event-project-pill" title={project}>{project}</span>}
      </span>
      <span className={`telemetry-event-meta${live ? ' telemetry-live-request-summary' : ''}`} title={summary}>
        {[eventKindLabel(event.kind), eventClientKindLabel(event), eventAgentLabel(event), recentEventPurposeLabel(event), event.sourceIP]
          .filter(Boolean).join(' · ')}
      </span>
      {tools.length > 0 && (
        <span className="telemetry-event-meta telemetry-event-tools" title={`调用工具：${tools.join('、')}`}>
          <Icon name="wrench" size={11} aria-hidden="true" />
          {tools.join('、')}
        </span>
      )}
    </span>
  );
}

function RecentEventRouteCell({ event, live = false }) {
  if (event.kind === 'notify') return <span>{event.hookEvent || '通知'}</span>;
  const logical = eventLogicalModel(event);
  const clientModel = String(eventField(event, 'clientModel', 'client_model') || '').trim();
  const isMapped = clientModel && logical !== '—' && clientModel !== logical;
  const attempt = Number(eventField(event, 'attemptNumber', 'attempt_number') || 0);
  const hasRetry = attempt > 1 || eventFailover(event);
  return (
    <span className={live ? 'telemetry-live-route' : 'telemetry-cell-stack telemetry-route-cell'}>
      <span className="telemetry-route-model-line">
        <strong className="telemetry-event-model">{logical}</strong>
        {isMapped && (
          <span className="telemetry-route-mapped-badge mono-cell" title={`客户端模型 ${clientModel} → 路由模型 ${logical}`}>
            ← {clientModel}
          </span>
        )}
      </span>
      <span className="telemetry-route-meta">
        <span>{eventEndpointName(event)}</span>
        {hasRetry && (
          <span className="telemetry-route-retry-badge">
            {attempt > 1 ? `重试 #${attempt}` : '故障转移'}
          </span>
        )}
      </span>
    </span>
  );
}

function RecentEventResultCell({ event, live = false }) {
  if (event.kind === 'notify') return <span className="telemetry-cell-stack">通知事件</span>;
  return (
    <span className={live ? 'telemetry-live-result' : 'telemetry-cell-stack telemetry-result-cell'}>
      <span className="telemetry-result-primary-row">
        <RecentEventOutcome event={event} />
        <span className="telemetry-result-detail mono-cell" title={recentEventStatusDetail(event)}>
          {eventHTTPStatusCode(event) ? eventHttpStatusLabel(event) : 'HTTP —'}
        </span>
      </span>
      <span className="telemetry-result-detail telemetry-result-transport">{eventTransportLabel(event)}</span>
    </span>
  );
}

function RecentEventSpeed({ event }) {
  if (event.kind !== 'client' || eventIsInFlight(event)) return null;
  const raw = eventField(event, 'outputTokensPerSecond', 'output_tokens_per_second');
  const speed = raw == null ? NaN : Number(raw);
  const measurable = eventOutcome(event) === 'succeeded' && Number.isFinite(speed) && speed >= 0;
  return <span className="telemetry-speed-pill" title={measurable ? '根据上游有效输出区间估算的速度' : '无法可靠测量输出速度'}>{measurable ? `${speed.toFixed(1)} tok/s` : '—'}</span>;
}

function RecentEventDurationCell({ event }) {
  if (event.kind === 'notify') return <span className="telemetry-duration-cell">—</span>;
  const live = eventIsInFlight(event);
  const { ttfbMS, durationMS } = eventDisplayTiming(event);
  const slowTTFB = ttfbMS >= 5000;
  const totalMS = Number.isFinite(durationMS) && durationMS > 0 ? durationMS : (Number.isFinite(ttfbMS) && ttfbMS > 0 ? ttfbMS : 0);
  const ttfbPart = Number.isFinite(ttfbMS) && ttfbMS > 0 && totalMS > 0
    ? Math.min(100, Math.max(10, Math.round((ttfbMS / totalMS) * 100)))
    : 0;
  const streamPart = totalMS > 0 ? Math.max(0, 100 - ttfbPart) : 0;
  return (
    <span className="mono-cell telemetry-duration-cell telemetry-inline-latency"
      title={slowTTFB ? '首字节 → 总耗时；首字节超过 5 秒，上游可能排队中' : '首字节 → 总耗时'}>
      <span className="telemetry-inline-latency-top">
        <span className={`telemetry-inline-latency-nums${slowTTFB ? ' is-slow' : ''}${live ? ' is-live' : ''}`}>{eventDurationText(event)}</span>
        <RecentEventSpeed event={event} />
      </span>
      {!live && totalMS > 0 && (
        <span className="telemetry-latency-track" aria-hidden="true">
          {ttfbPart > 0 && <i className={`telemetry-latency-ttfb${slowTTFB ? ' is-slow' : ''}`} style={{ width: `${ttfbPart}%` }} />}
          {streamPart > 0 && <i className="telemetry-latency-stream" style={{ width: `${streamPart}%` }} />}
        </span>
      )}
    </span>
  );
}

function PageTelemetryStrip({ events }) {
  if (!events?.length) return null;
  const nonNotify = events.filter((event) => String(event?.kind || '').toLowerCase() !== 'notify');
  if (!nonNotify.length) return null;
  let succeeded = 0;
  let failed = 0;
  let cancelled = 0;
  let ttfbSum = 0;
  let ttfbCount = 0;
  let durationSum = 0;
  let durationCount = 0;
  let tokenTotal = 0;
  let failoverCount = 0;
  for (const event of nonNotify) {
    const outcome = eventOutcome(event);
    if (outcome === 'succeeded') succeeded += 1;
    else if (outcome === 'failed') failed += 1;
    else if (outcome === 'cancelled') cancelled += 1;
    const timing = eventDisplayTiming(event);
    if (Number.isFinite(timing.ttfbMS) && timing.ttfbMS >= 0) {
      ttfbSum += timing.ttfbMS;
      ttfbCount += 1;
    }
    if (Number.isFinite(timing.durationMS) && timing.durationMS >= 0) {
      durationSum += timing.durationMS;
      durationCount += 1;
    }
    const input = Number(eventField(event, 'inputTokens', 'input_tokens') || 0);
    const output = Number(eventField(event, 'outputTokens', 'output_tokens') || 0);
    if (Number.isFinite(input) && input > 0) tokenTotal += input;
    if (Number.isFinite(output) && output > 0) tokenTotal += output;
    if (eventFailover(event)) failoverCount += 1;
  }
  const completed = succeeded + failed + cancelled;
  const successRate = completed > 0 ? `${((succeeded / completed) * 100).toFixed(1)}%` : '—';
  const goodPct = completed > 0 ? Math.round((succeeded / completed) * 100) : 0;
  const failPct = completed > 0 ? Math.round((failed / completed) * 100) : 0;
  const warnPct = completed > 0 ? Math.max(0, 100 - goodPct - failPct) : 0;
  return (
    <div className="telemetry-overview-strip" role="region" aria-label="本页事件遥测摘要">
      <div className="telemetry-overview-item">
        <div className="telemetry-overview-head">
          <span className="telemetry-overview-label">本页请求健康度</span>
          <strong className="mono-cell telemetry-overview-value">{successRate}</strong>
        </div>
        <div className="telemetry-health-bar" aria-hidden="true">
          {goodPct > 0 && <i className="telemetry-health-seg-good" style={{ width: `${goodPct}%` }} />}
          {failPct > 0 && <i className="telemetry-health-seg-fail" style={{ width: `${failPct}%` }} />}
          {warnPct > 0 && <i className="telemetry-health-seg-warn" style={{ width: `${warnPct}%` }} />}
        </div>
        <div className="telemetry-overview-sub">
          <span>成功 {formatNumber(succeeded)}</span>
          <span>失败 {formatNumber(failed)} · 取消 {formatNumber(cancelled)}</span>
        </div>
      </div>
      <div className="telemetry-overview-item">
        <div className="telemetry-overview-head">
          <span className="telemetry-overview-label">本页首字节均值</span>
          <strong className="mono-cell telemetry-overview-value">
            {ttfbCount > 0 ? formatDuration(ttfbSum / ttfbCount) : '—'}
          </strong>
        </div>
        <div className="telemetry-overview-sub">
          <span>蓝青段代表首字节响应</span>
          <span>{formatNumber(ttfbCount)} 条已采样</span>
        </div>
      </div>
      <div className="telemetry-overview-item">
        <div className="telemetry-overview-head">
          <span className="telemetry-overview-label">本页完成耗时均值</span>
          <strong className="mono-cell telemetry-overview-value">
            {durationCount > 0 ? formatDuration(durationSum / durationCount) : '—'}
          </strong>
        </div>
        <div className="telemetry-overview-sub">
          <span>紫绿段代表流式生成</span>
          <span>{formatNumber(durationCount)} 条已采样</span>
        </div>
      </div>
      <div className="telemetry-overview-item">
        <div className="telemetry-overview-head">
          <span className="telemetry-overview-label">本页 Token 与切换</span>
          <strong className="mono-cell telemetry-overview-value">
            {tokenTotal > 0 ? formatTokenCount(tokenTotal) : '—'}
          </strong>
        </div>
        <div className="telemetry-overview-sub">
          <span>本页输入+输出累计</span>
          <span>故障转移 {formatNumber(failoverCount)} 次</span>
        </div>
      </div>
    </div>
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
  const isLive = live || eventIsInFlight(event);
  const friendly = friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : '');
  return (
    <span
      className={`${isLive ? 'telemetry-live-message' : 'telemetry-event-message'}${eventFailover(event) ? ' is-warning' : eventOutcome(event) === 'failed' ? ' is-error' : ''}`}
      title={friendly || undefined}
    >
      {eventFailover(event) && <strong>故障转移 · </strong>}
      {friendly || (isLive ? '请求进行中，等待最终结果' : '-')}
    </span>
  );
}

function LiveEventList({ events, selectedEventID, onSelect }) {
  if (!events?.length) return null;
  return (
    <section className="telemetry-live-group" aria-label={`${formatNumber(events.length)} 个进行中请求`}>
      <LiveSurfaceLight />
      {/* 标题与「上游暂计」并进列头行:计数落在「请求」列,暂计提示落在「Token 用量」列。 */}
      <div className="telemetry-live-columns">
        <span className="telemetry-live-heading-cell"><i aria-hidden="true" />进行中 · {formatNumber(events.length)}</span>
        <span aria-hidden="true">模型 / 路由</span>
        <span aria-hidden="true">结果</span>
        <span aria-hidden="true">首字节 → 总耗时</span>
        <span aria-hidden="true">Token 用量 <small title="仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。">上游暂计</small></span>
        <span aria-hidden="true">事件状态</span>
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
                className={`mono-cell telemetry-mobile-duration${live || eventIsInFlight(event) ? ' is-live' : ''}`}
                style={{ color: outcome === 'failed' ? 'var(--status-critical)' : undefined }}
                title={eventDisplayTiming(event).ttfbMS >= 5000 ? '首字节 → 总耗时；首字节超过 5 秒，上游可能排队中' : '首字节 → 总耗时'}
              >
                <span>{eventDurationText(event)}</span>
                <RecentEventSpeed event={event} />
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
    autoRefresh, runRefreshInterval,
    loadRuntimeEvent, addToast,
  } = useApp();
  const [eventFilter, setEventFilter] = useState('client'); // 'client' | 'upstream' | 'all'
  const [eventOutcomeFilter, setEventOutcomeFilter] = useState(''); // '' | 'succeeded' | 'failed' | 'cancelled'
  const [eventSort, setEventSort] = useState('desc'); // 'desc' = 最新优先, 'asc' = 最早优先
  const [selectedEventID, setSelectedEventID] = useState(null);
  const toggleSelectEvent = useCallback((id) => {
    setSelectedEventID((previous) => (previous === id ? null : id));
  }, []);
  const [expandedGroups, setExpandedGroups] = useState({});
  const toggleGroup = (key, open) => setExpandedGroups((previous) => previous[key] === open ? previous : { ...previous, [key]: open });
  const [requestChainSnapshot, setRequestChainSnapshot] = useState(null);
  const requestChainEvents = requestChainSnapshot?.events || [];
  const chainLoader = useRef(null);
  const refreshEnabled = useRef(autoRefresh);
  refreshEnabled.current = autoRefresh;
  const [eventPageDirection, setEventPageDirection] = useState('none');
  const [, setDurationTick] = useState(0);

  const isRunning = status?.running;
  const recentEvents = runtime?.recentEvents || [];
  const [overview, setOverview] = useState(null);
  const [localDay, setLocalDay] = useState(() => runtimeTodayBounds().from);
  useEffect(() => observeRuntimeDay(setLocalDay), []);
  useEffect(() => { setSelectedEventID(null); setRequestChainSnapshot(null); }, [localDay]);
  const notifySnapshotRecovered = useCallback((message) => addToast(message, 'info'), [addToast]);
  const eventHistory = useRuntimeEventPage({
    range: RUN_OVERVIEW_RANGE,
    scopeKey: localDay,
    recentEvents,
    autoRefresh,
    refreshInterval: runRefreshInterval,
    eventKind: eventFilter === 'all' ? '' : eventFilter,
    outcome: eventOutcomeFilter,
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
    let cancelled = false;
    setOverview(null);
    const load = () => {
      if (document.hidden) return;
      api.getRuntimeTrends(runOverviewQuery()).then((series) => {
        if (cancelled) return;
        const totals = runOverviewTotals(series);
        setOverview(totals ? { status: 'ready', ...totals } : { status: 'error' });
      }).catch(() => {
        if (!cancelled) {
          setOverview((current) => current?.status === 'ready' ? current : { status: 'error' });
        }
      });
    };
    load();
    if (!autoRefresh) return () => { cancelled = true; };
    const timer = window.setInterval(load, Math.max(15, Number(runRefreshInterval) || 15) * 1000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [autoRefresh, runRefreshInterval, runtime?.resetGeneration, localDay]);

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
    : baseEventContext.find((e) => e.id === currentSelectedID)
      || requestChainEvents.find((e) => e.id === currentSelectedID)
      || visibleEvents.find((e) => e.id === currentSelectedID));

  // Request chain for selected event
  const requestChain = useMemo(() => {
    const byID = new Map();
    for (const event of [...requestChainEvents, ...baseEventContext]) byID.set(event.id, mergeRuntimeEvent(byID.get(event.id), event));
    const eventContext = [...byID.values()];
    const chain = getRequestChain(eventContext, currentSelectedID);
    if (runtimeEventDetail?.event?.id !== currentSelectedID) return chain;
    return chain.map((event) => event.id === currentSelectedID ? runtimeEventDetail.event : event);
  }, [baseEventContext, currentSelectedID, requestChainEvents, runtimeEventDetail]);

  const selectedRequestID = selectedEvent ? eventRequestID(selectedEvent) : '';
  const selectedChangeSeq = Math.max(
    Number(baseEventContext.find(event => event.id === currentSelectedID)?.changeSeq || 0),
    Number(requestChainEvents.find(event => event.id === currentSelectedID)?.changeSeq || 0),
  );
  useEffect(() => {
    setRequestChainSnapshot(null);
    if (!selectedRequestID || selectedRequestID === '-') return undefined;
    const loader = createRequestChainLoader({
      read: signal => api.getRuntimeRequestChain(selectedRequestID, { signal }),
      onValue: value => setRequestChainSnapshot(previous => mergeRequestChain(value, previous?.requestID === selectedRequestID ? previous.events : [])),
      enabled: () => refreshEnabled.current && !document.hidden,
    });
    chainLoader.current = loader;
    const visibility = () => loader.synchronize();
    document.addEventListener('visibilitychange', visibility);
    loader.refresh(true);
    return () => {
      loader.dispose();
      if (chainLoader.current === loader) chainLoader.current = null;
      document.removeEventListener('visibilitychange', visibility);
    };
  }, [selectedRequestID, runtime?.resetGeneration]);

  useEffect(() => { chainLoader.current?.synchronize(); }, [autoRefresh]);
  useEffect(() => {
    if (!autoRefresh || document.hidden || !selectedRequestID) return;
    const relevant = baseEventContext.filter(e => eventRequestID(e) === selectedRequestID);
    setRequestChainSnapshot(previous => previous?.requestID === selectedRequestID ? mergeRequestChain(previous, relevant) : previous);
    chainLoader.current?.refresh();
  }, [autoRefresh, baseEventContext, selectedRequestID]);

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
          <div className="run-hero-head">
            <div className="run-hero-heading">
              <span className="run-hero-kicker">服务监听状态</span>
              <p className="run-hero-description">
                {status?.health?.headline || (isRunning ? '代理服务正常响应客户端请求' : '服务当前处于休眠停止状态')}
              </p>
            </div>
            <span className="mono-cell run-hero-version">
              v{status?.version || '0.2.0'} · {status?.generation || '-'}
            </span>
          </div>

          {/* Detailed Info Matrix matching macOS */}
          <dl className="run-detail-grid">
            <div className="run-detail-listener">
              <dt>监听地址</dt>
              <dd className="run-listener-row">
                <span className="mono-cell run-listener-address">
                  {status?.listener ? `${status.listener.host}:${status.listener.port}` : '127.0.0.1:57878'}
                </span>
                <button
                  type="button"
                  className="btn btn-ghost btn-compact run-copy-address"
                  onClick={() => copyText(`http://${status?.listener?.host || '127.0.0.1'}:${status?.listener?.port || 57878}`, '监听地址')}
                  title="复制监听地址"
                >
                  <Icon name="copy" size={14} />
                  <span>复制</span>
                </button>
              </dd>
            </div>
            <div>
              <dt>配置版本</dt>
              <dd>v{config?.schemaVersion || 7}</dd>
            </div>
            <div>
              <dt>认证状态</dt>
              <dd className={secretStatus?.inboundAuthToken?.configured ? 'run-detail-good' : undefined}>
                {secretStatus?.inboundAuthToken?.configured ? `已启用 · 尾号 ${secretStatus.inboundAuthToken.last4}` : '未启用 · 本机免密'}
              </dd>
            </div>
            <div>
              <dt>运行时长</dt>
              <dd className="mono-cell">
                {status?.uptimeSeconds ? formatDuration(status.uptimeSeconds * 1000) : '-'}
              </dd>
            </div>
            <div>
              <dt>调度策略</dt>
              <dd>优先级 + 会话粘性绑定</dd>
            </div>
          </dl>
        </div>

      </div>

      {/* Today's usage cards and current routing inventory */}
      <div className="run-metric-grid" aria-label="今天用量与当前入口">
        <MetricCard
          label="输入 Token"
          value={overview?.status === 'ready' && overview.observed > 0 && overview.input != null ? formatTokenCount(overview.input) : '—'}
          detail={overviewTokenDetail(overview)}
          icon="file-down"
          accent="var(--primary)"
        />
        <MetricCard
          label="输出 Token"
          value={overview?.status === 'ready' && overview.observed > 0 && overview.output != null ? formatTokenCount(overview.output) : '—'}
          detail={overviewTokenDetail(overview)}
          icon="file-up"
          accent="var(--status-warning)"
        />
        <MetricCard
          label="可调度入口"
          value={formatNumber(status?.endpoints ?? config?.endpoints?.length ?? 0)}
          detail="按优先级形成 Provider 候选序列"
          icon="git-fork"
          accent="var(--accent-cyan)"
        />
        <MetricCard
          label="客户端请求"
          value={overviewCount(overview?.clientRequests)}
          detail={`今天 · 成功 ${overviewCount(overview?.clientSuccesses)} / 失败 ${overviewCount(overview?.clientFailures)}`}
          icon="move-diagonal"
          accent="var(--status-good)"
        />
        <MetricCard
          label="上游尝试"
          value={overviewCount(overview?.upstreamAttempts)}
          detail={`今天 · 含重试 · 故障转移 ${overviewCount(overview?.failovers)}`}
          icon="network"
          accent="var(--accent-indigo)"
        />
      </div>


      {/* Real-time Streaming Telemetry Event Log */}
      <div className="glass-panel run-events-panel">
        <div className="panel-header">
          <div className="panel-title-group">
            <div className="panel-title">
              <Icon name="activity" size={18} style={{ color: 'var(--primary)' }} />
              <span>今日事件与遥测日志</span>
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
            <div className="telemetry-filter-group telemetry-outcome-filter-group" role="group" aria-label="事件结果状态筛选">
              {[
                { id: '', label: '全部状态' },
                { id: 'succeeded', label: '成功' },
                { id: 'failed', label: '失败' },
                { id: 'cancelled', label: '已取消' },
              ].map((tab) => (
                <button
                  key={tab.id || 'all-outcomes'}
                  type="button"
                  className={`telemetry-filter-button${eventOutcomeFilter === tab.id ? ' active' : ''}`}
                  onClick={() => {
                    setSelectedEventID(null);
                    setEventPageDirection('replace');
                    setEventOutcomeFilter(tab.id);
                  }}
                  aria-pressed={eventOutcomeFilter === tab.id}
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

        <PageTelemetryStrip events={visibleEvents} />

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
                  <LiveEventList events={liveEvents} selectedEventID={selectedEvent?.id} onSelect={toggleSelectEvent} />
                </div>
                <div className="telemetry-live-mobile">
                  <div className="telemetry-live-heading">
                    <span><i aria-hidden="true" />进行中 · {formatNumber(liveEvents.length)}</span>
                    <small>上游暂计</small>
                  </div>
                <MobileEventList
                  events={liveEvents}
                  selectedEventID={selectedEvent?.id}
                  onSelect={toggleSelectEvent}
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
                  onRowClick={(row) => toggleSelectEvent(row.id)}
                  activeRowKey={selectedEvent?.id}
                  emptyText={eventHistory.loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}
                />
              </div>
              <MobileEventList
                events={visibleEvents}
                selectedEventID={selectedEvent?.id}
                onSelect={toggleSelectEvent}
                loading={eventHistory.loading}
              />
              </section>
              {!selectedEvent && (visibleEvents.length > 0 || liveEvents.length > 0) && (
                <div className="telemetry-selection-hint" role="status">选择一条事件查看请求链路与诊断详情</div>
              )}
              {eventHistory.mode === 'page' && visibleEvents.length >= 15 && eventHistory.totalPages > 1 ? (
                <div className="telemetry-bottom-pagination">
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
                    ariaLabel="持久事件底部分页"
                  />
                </div>
              ) : null}
            </div>

          </div>
          {eventHistory.mode === 'legacy' ? (
            <div className="event-page-compatibility" role="status">
              当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。
            </div>
          ) : null}
        </div>

        {selectedEvent && <EventInspector
          event={selectedEvent} chain={requestChain} chainTruncated={requestChainSnapshot?.truncated} onSelect={setSelectedEventID}
          onClose={() => setSelectedEventID(null)}
          requestSummary={recentEventRequestSummary(selectedEvent)}
          expanded={expandedGroups} onToggle={toggleGroup} onCopy={copyText}
          loading={runtimeEventDetail?.event?.id !== selectedEvent.id && selectedEvent.detailsOmitted === true}
        />}

      </div>
    </div>
  );
}
