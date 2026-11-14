import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  AlertTriangle,
  ArrowUpDown,
  Copy,
  Play,
  Square,
  Wrench,
} from 'lucide-react';
import { cn } from '../lib/utils.js';
import { createRequestChainLoader, mergeRequestChain } from '../utils/requestChain.js';
import { useApp, useRuntimePagePreference } from '../context/AppContext.jsx';
import { MetricCard } from '../components/MetricCard.jsx';
import { LiveSurfaceLight } from '../components/LiveSurfaceLight.jsx';
import { StatusBadge } from '../components/StatusBadge.jsx';
import { DataTable } from '../components/DataTable.jsx';
import { PaginationBar } from '../components/PaginationBar.jsx';
import { useRuntimeEventPage } from '../hooks/useRuntimeEventPage.js';
import { orderRuntimeEvents, mergeRuntimeEvent } from '../utils/runtimeEvents.js';
import { EventInspector } from '../components/EventInspector.jsx';
import { CacheHitRate } from '../components/CacheHitRate.jsx';
import { eventAgentLabel, eventCacheLabel, eventHasObservedUsage, eventUsageLabel } from '../utils/eventPresentation.js';
import { copyWithToast } from '../utils/clipboard.js';
import { api } from '../services/api.js';
import { RUN_OVERVIEW_RANGE, runOverviewQuery, runOverviewTotals } from '../utils/runOverview.js';
import { observeRuntimeDay, runtimeTodayBounds } from '../utils/runtimeAnalyticsV2.js';
import {
  requestKindLabel,
  eventEndpointName,
  formatNumber,
  formatTokenCount,
  formatDuration,
  formatTimestamp,
  eventLogicalModel,
  statusKind,
  eventDurationText,
  eventDisplayTiming,
  friendlyEventMessage,
  eventPurposeLabel,
  eventClientKindLabel,
  eventKindLabel,
  getRequestChain,
  eventOutcomeLabel,
  eventPhaseLabel,
  eventHttpStatusLabel,
  eventHTTPStatusCode,
  eventRequestID,
  eventOutcome,
  eventFailover,
  eventField,
  eventIsInFlight,
  eventFailureSummaryLabel,
  eventProjectContext,
  eventToolCalls,
  eventTransportLabel,
} from '../utils/helpers.js';
import { Badge } from '../components/ui/badge.jsx';
import { Button } from '../components/ui/button.jsx';
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '../components/ui/card.jsx';
import { ToggleGroup, ToggleGroupItem } from '../components/ui/toggle-group.jsx';

function overviewTokenDetail(totals) {
  if (!totals) return '正在读取今天的用量';
  if (totals.status === 'error') return '今天用量暂不可用';
  return totals.observed > 0
    ? `今天 · ${formatNumber(totals.observed)} 个请求有用量`
    : '今天暂无可用用量';
}

const overviewCount = (value) => (value == null ? '—' : formatNumber(value));

const OUTCOME_TEXT_COLOR = {
  good: 'text-status-good',
  warning: 'text-status-warning',
  critical: 'text-status-critical',
  live: 'text-primary',
  muted: 'text-muted-foreground',
};

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
  return [
    recentEventProjectSummary(event),
    eventKindLabel(event.kind),
    eventClientKindLabel(event),
    eventAgentLabel(event) ? `代理: ${eventAgentLabel(event)}` : null,
    recentEventPurposeLabel(event),
    event.sourceIP,
  ].filter(Boolean).join(' · ');
}

function RecentEventOutcome({ event, compact = false }) {
  const kind = statusKind(event);
  const isLive = eventIsInFlight(event) || kind === 'live';
  return (
    <span
      className={cn(
        'inline-flex items-center gap-1.5 font-semibold',
        compact ? 'text-[11px]' : 'text-xs',
        OUTCOME_TEXT_COLOR[kind] || 'text-muted-foreground',
      )}
    >
      {isLive && (
        <i className="telemetry-live-pill-dot size-1.5 shrink-0 rounded-full bg-current" aria-hidden="true" />
      )}
      {recentEventOutcomeLabel(event)}
    </span>
  );
}

function RecentEventRequestCell({ event, live = false }) {
  const summary = recentEventRequestSummary(event);
  const project = recentEventProjectSummary(event);
  const tools = event.kind === 'notify' ? [] : eventToolCalls(event);
  const metaText = [
    eventKindLabel(event.kind),
    eventClientKindLabel(event),
    eventAgentLabel(event),
    recentEventPurposeLabel(event),
    event.sourceIP,
  ].filter(Boolean).join(' · ');

  return (
    <span className="flex min-w-0 flex-col gap-0.5 text-left">
      <span className="flex flex-wrap items-center gap-1.5">
        <span className={cn('font-mono text-xs font-medium text-foreground tabular-nums', live && 'text-primary')}>
          {formatTimestamp(event.timestamp)}
        </span>
        {project && (
          <Badge variant="outline" className="max-w-[180px] truncate px-1.5 py-0 text-[10px] font-normal" title={project}>
            {project}
          </Badge>
        )}
      </span>
      <span className="truncate text-[11px] text-muted-foreground" title={summary}>
        {metaText}
      </span>
      {tools.length > 0 && (
        <span
          className="telemetry-event-tools inline-flex w-fit max-w-full items-center gap-1 truncate whitespace-nowrap rounded border border-border/70 bg-muted/50 px-1.5 py-0.5 text-[10px] text-foreground/85"
          title={`调用工具：${tools.join('、')}`}
        >
          <Wrench className="size-2.5 shrink-0 text-muted-foreground" aria-hidden="true" />
          <span className="truncate whitespace-nowrap">{tools.join('、')}</span>
        </span>
      )}
    </span>
  );
}

function RecentEventRouteCell({ event }) {
  if (event.kind === 'notify') return <span className="text-xs text-muted-foreground">{event.hookEvent || '通知'}</span>;
  const logical = eventLogicalModel(event);
  const clientModel = String(eventField(event, 'clientModel', 'client_model') || '').trim();
  const isMapped = clientModel && logical !== '—' && clientModel !== logical;
  const attempt = Number(eventField(event, 'attemptNumber', 'attempt_number') || 0);
  const hasRetry = attempt > 1 || eventFailover(event);
  return (
    <span className="flex min-w-0 flex-col gap-0.5 text-left">
      <span className="flex flex-wrap items-center gap-1.5">
        <strong className="truncate font-mono text-xs font-semibold text-foreground">{logical}</strong>
        {isMapped && (
          <span
            className="truncate rounded border border-border/70 bg-muted/50 px-1 py-0 font-mono text-[10px] text-muted-foreground"
            title={`客户端模型 ${clientModel} → 路由模型 ${logical}`}
          >
            ← {clientModel}
          </span>
        )}
      </span>
      <span className="flex flex-wrap items-center gap-1.5 text-[11px] text-muted-foreground">
        <span className="truncate">{eventEndpointName(event)}</span>
        {hasRetry && (
          <Badge variant="outline" className="border-status-warning/40 bg-status-warning/10 px-1.5 py-0 text-[10px] text-status-warning">
            {attempt > 1 ? `重试 #${attempt}` : '故障转移'}
          </Badge>
        )}
      </span>
    </span>
  );
}

function RecentEventResultCell({ event }) {
  if (event.kind === 'notify') return <span className="text-xs text-muted-foreground">通知事件</span>;
  return (
    <span className="flex min-w-0 flex-col gap-0.5 text-left">
      <span className="flex flex-wrap items-center gap-1.5">
        <RecentEventOutcome event={event} />
        <span className="font-mono text-[11px] text-muted-foreground tabular-nums" title={recentEventStatusDetail(event)}>
          {eventHTTPStatusCode(event) ? eventHttpStatusLabel(event) : 'HTTP —'}
        </span>
      </span>
      <span className="telemetry-result-transport text-[11px] text-muted-foreground">{eventTransportLabel(event)}</span>
    </span>
  );
}

function RecentEventSpeed({ event }) {
  if (event.kind !== 'client' || eventIsInFlight(event)) return null;
  const raw = eventField(event, 'outputTokensPerSecond', 'output_tokens_per_second');
  const speed = raw == null ? NaN : Number(raw);
  const measurable = eventOutcome(event) === 'succeeded' && Number.isFinite(speed) && speed >= 0;
  return (
    <span
      className="rounded border border-border/70 bg-muted/40 px-1.5 py-0 font-mono text-[10px] text-muted-foreground tabular-nums"
      title={measurable ? '根据上游有效输出区间估算的速度' : '无法可靠测量输出速度'}
    >
      {measurable ? `${speed.toFixed(1)} tok/s` : '—'}
    </span>
  );
}

function RecentEventDurationCell({ event }) {
  if (event.kind === 'notify') return <span className="font-mono text-xs text-muted-foreground">—</span>;
  const live = eventIsInFlight(event);
  const { ttfbMS, durationMS } = eventDisplayTiming(event);
  const slowTTFB = ttfbMS >= 5000;
  const totalMS = Number.isFinite(durationMS) && durationMS > 0
    ? durationMS
    : (Number.isFinite(ttfbMS) && ttfbMS > 0 ? ttfbMS : 0);
  const ttfbPart = Number.isFinite(ttfbMS) && ttfbMS > 0 && totalMS > 0
    ? Math.min(100, Math.max(10, Math.round((ttfbMS / totalMS) * 100)))
    : 0;
  const streamPart = totalMS > 0 ? Math.max(0, 100 - ttfbPart) : 0;
  return (
    <span
      className="flex flex-col gap-1 font-mono text-xs tabular-nums"
      title={slowTTFB ? '首字节 → 总耗时；首字节超过 5 秒，上游可能排队中' : '首字节 → 总耗时'}
    >
      <span className="flex flex-wrap items-center gap-1.5">
        <span className={cn(slowTTFB && 'text-status-warning font-semibold', live && 'text-primary font-semibold')}>
          {eventDurationText(event)}
        </span>
        <RecentEventSpeed event={event} />
      </span>
      {!live && totalMS > 0 && (
        <span className="flex h-1 w-28 overflow-hidden rounded-full bg-muted" aria-hidden="true">
          {ttfbPart > 0 && (
            <i
              className={cn('h-full', slowTTFB ? 'bg-status-warning' : 'bg-accent-cyan')}
              style={{ width: `${ttfbPart}%` }}
            />
          )}
          {streamPart > 0 && <i className="h-full bg-primary/80" style={{ width: `${streamPart}%` }} />}
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
    <div
      className="grid gap-3 border-y bg-muted/15 px-4 py-3 sm:grid-cols-2 xl:grid-cols-4"
      role="region"
      aria-label="本页事件遥测摘要"
    >
      <div className="space-y-1">
        <div className="flex items-baseline justify-between gap-2 text-xs">
          <span className="text-muted-foreground">本页请求健康度</span>
          <strong className="font-mono font-semibold text-foreground tabular-nums">{successRate}</strong>
        </div>
        <div className="flex h-1.5 w-full overflow-hidden rounded-full bg-muted" aria-hidden="true">
          {goodPct > 0 && <i className="h-full bg-status-good" style={{ width: `${goodPct}%` }} />}
          {failPct > 0 && <i className="h-full bg-status-critical" style={{ width: `${failPct}%` }} />}
          {warnPct > 0 && <i className="h-full bg-status-warning" style={{ width: `${warnPct}%` }} />}
        </div>
        <div className="flex items-center justify-between text-[11px] text-muted-foreground tabular-nums">
          <span>成功 {formatNumber(succeeded)}</span>
          <span>失败 {formatNumber(failed)} · 取消 {formatNumber(cancelled)}</span>
        </div>
      </div>

      <div className="space-y-1">
        <div className="flex items-baseline justify-between gap-2 text-xs">
          <span className="text-muted-foreground">本页首字节均值</span>
          <strong className="font-mono font-semibold text-foreground tabular-nums">
            {ttfbCount > 0 ? formatDuration(ttfbSum / ttfbCount) : '—'}
          </strong>
        </div>
        <div className="flex items-center justify-between text-[11px] text-muted-foreground tabular-nums">
          <span>蓝青段代表首字节响应</span>
          <span>{formatNumber(ttfbCount)} 条已采样</span>
        </div>
      </div>

      <div className="space-y-1">
        <div className="flex items-baseline justify-between gap-2 text-xs">
          <span className="text-muted-foreground">本页完成耗时均值</span>
          <strong className="font-mono font-semibold text-foreground tabular-nums">
            {durationCount > 0 ? formatDuration(durationSum / durationCount) : '—'}
          </strong>
        </div>
        <div className="flex items-center justify-between text-[11px] text-muted-foreground tabular-nums">
          <span>紫绿段代表流式生成</span>
          <span>{formatNumber(durationCount)} 条已采样</span>
        </div>
      </div>

      <div className="space-y-1">
        <div className="flex items-baseline justify-between gap-2 text-xs">
          <span className="text-muted-foreground">本页 Token 与切换</span>
          <strong className="font-mono font-semibold text-foreground tabular-nums">
            {tokenTotal > 0 ? formatTokenCount(tokenTotal) : '—'}
          </strong>
        </div>
        <div className="flex items-center justify-between text-[11px] text-muted-foreground tabular-nums">
          <span>本页输入+输出累计</span>
          <span>故障转移 {formatNumber(failoverCount)} 次</span>
        </div>
      </div>
    </div>
  );
}

function RecentEventUsageCell({ event }) {
  if (event.kind === 'notify') return <span className="font-mono text-xs text-muted-foreground">—</span>;
  const observed = eventHasObservedUsage(event);
  const live = eventIsInFlight(event);
  return (
    <span
      className={cn('flex flex-col gap-0.5 text-left text-xs', !observed && 'text-muted-foreground')}
      title={
        live
          ? '仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。'
          : '输入、输出和缓存读取均来自上游报告；— 表示未报告，不代表 0。'
      }
    >
      <span className="font-mono text-xs font-medium text-foreground tabular-nums">{eventUsageLabel(event)}</span>
      <span className="flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground tabular-nums">
        <span>{live && !observed ? '上游尚未返回' : eventCacheLabel(event)}</span>
        {(!live || observed) && <CacheHitRate event={event} />}
        {live && observed && (
          <small className="rounded bg-primary/15 px-1 py-0 text-[10px] font-medium text-primary">暂计</small>
        )}
      </span>
    </span>
  );
}

function RecentEventMessageCell({ event, live = false }) {
  const isLive = live || eventIsInFlight(event);
  const friendly = friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : '');
  const failover = eventFailover(event);
  const failed = eventOutcome(event) === 'failed';
  return (
    <span
      className={cn(
        'line-clamp-2 text-left text-xs leading-relaxed',
        failover ? 'text-status-warning' : failed ? 'text-status-critical' : 'text-muted-foreground',
      )}
      title={friendly || undefined}
    >
      {failover && <strong className="font-semibold">故障转移 · </strong>}
      {friendly || (isLive ? '请求进行中，等待最终结果' : '-')}
    </span>
  );
}

function LiveEventList({ events, selectedEventID, onSelect }) {
  if (!events?.length) return null;
  return (
    <section
      className="telemetry-live-group relative overflow-hidden border-b bg-primary/5"
      aria-label={`${formatNumber(events.length)} 个进行中请求`}
    >
      <LiveSurfaceLight />
      {/* 标题与「上游暂计」并进列头行:计数落在「请求」列,暂计提示落在「Token 用量」列。 */}
      <div className="relative z-10 grid grid-cols-[220px_180px_104px_156px_220px_minmax(220px,1fr)] items-center border-b bg-background/50 px-3.5 py-2 text-[11px] font-medium text-muted-foreground backdrop-blur-xs">
        <span className="telemetry-live-heading-cell inline-flex items-center gap-1.5 font-semibold text-foreground">
          <i className="size-1.5 rounded-full bg-primary" aria-hidden="true" />
          进行中 · {formatNumber(events.length)}
        </span>
        <span aria-hidden="true">模型 / 路由</span>
        <span aria-hidden="true">结果</span>
        <span aria-hidden="true">首字节 → 总耗时</span>
        <span aria-hidden="true">
          Token 用量 <small className="ml-1 text-[10px] font-normal text-primary" title="仅显示上游已报告的用量，可能滞后；请求完成后以最终用量为准。">上游暂计</small>
        </span>
        <span aria-hidden="true">事件状态</span>
      </div>
      <div className="relative z-10 max-h-[360px] divide-y overflow-y-auto" role="list" aria-label="进行中请求列表">
        {events.map((event) => {
          const selected = selectedEventID === event.id;
          return (
            <div key={event.id} role="listitem">
              <button
                type="button"
                className={cn(
                  'runtime-row-enter grid w-full min-w-[960px] cursor-pointer grid-cols-[220px_180px_104px_156px_220px_minmax(220px,1fr)] items-center px-3.5 py-2.5 text-left transition-colors hover:bg-primary/10 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary',
                  selected && 'bg-primary/15',
                )}
                aria-pressed={selected}
                onClick={() => onSelect(event.id)}
              >
                <RecentEventRequestCell event={event} live />
                <RecentEventRouteCell event={event} />
                <RecentEventResultCell event={event} />
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
      <div className="p-4 text-center text-xs text-muted-foreground">
        {loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}
      </div>
    );
  }

  return (
    <div
      className={cn('relative space-y-2 p-3', live && 'overflow-hidden border-b bg-primary/5')}
      data-live={live}
      role="list"
      aria-label={live ? '进行中请求列表' : '请求事件列表'}
    >
      {live && <LiveSurfaceLight />}
      {events.map((event) => {
        const projectContext = eventProjectContext(event);
        const outcome = eventOutcome(event);
        const friendly = friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : '');
        const selected = selectedEventID === event.id;
        return (
          <article key={event.id} role="listitem" className="relative z-10">
            <div
              className={cn(
                'cursor-pointer space-y-2 rounded-xl border bg-card p-3 transition-colors hover:bg-muted/40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring',
                selected && 'border-primary bg-primary/10',
              )}
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
              <div className="telemetry-mobile-heading flex flex-wrap items-center justify-between gap-2 text-xs">
                <span className="font-mono text-muted-foreground tabular-nums">{formatTimestamp(event.timestamp)}</span>
                <RecentEventOutcome event={event} compact />
                <span
                  className={cn(
                    'flex items-center gap-1.5 font-mono text-xs tabular-nums',
                    (live || eventIsInFlight(event)) && 'text-primary',
                    outcome === 'failed' && 'text-status-critical',
                  )}
                  title={eventDisplayTiming(event).ttfbMS >= 5000 ? '首字节 → 总耗时；首字节超过 5 秒，上游可能排队中' : '首字节 → 总耗时'}
                >
                  <span>{eventDurationText(event)}</span>
                  <RecentEventSpeed event={event} />
                </span>
              </div>
              <div className="telemetry-mobile-route flex flex-wrap items-baseline justify-between gap-2">
                <strong className="font-mono text-xs font-semibold text-foreground">
                  {event.kind === 'notify' ? (event.hookEvent || '通知') : eventLogicalModel(event)}
                </strong>
                {event.kind !== 'notify' && (
                  <small className="text-[11px] text-muted-foreground">{eventEndpointName(event)}</small>
                )}
              </div>
              <div className="telemetry-mobile-status flex flex-wrap items-center justify-between gap-2 text-[11px] text-muted-foreground">
                <span>{recentEventStatusDetail(event)}</span>
                {event.kind !== 'notify' && <RecentEventUsageCell event={event} />}
              </div>
              <div className="space-y-1 text-[11px] text-muted-foreground">
                <span className="telemetry-mobile-request-summary block truncate" title={recentEventRequestSummary(event)}>
                  {recentEventRequestSummary(event) || projectContext.label}
                </span>
                {friendly && (
                  <p
                    className={cn(
                      'rounded-md border px-2 py-1 text-[11px]',
                      outcome === 'failed'
                        ? 'border-status-critical/35 bg-status-critical/10 text-status-critical'
                        : eventFailover(event)
                          ? 'border-status-warning/35 bg-status-warning/10 text-status-warning'
                          : 'border-border/60 bg-muted/30 text-foreground/90',
                    )}
                  >
                    <span className="mr-1 font-medium text-muted-foreground">事件状态</span>
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
    autoRefresh, runRefreshInterval, pageVisible,
    loadRuntimeEvent, addToast,
  } = useApp();
  const [eventFilter, setEventFilter] = useRuntimePagePreference('run.kind', 'client'); // 'client' | 'upstream' | 'all'
  const [eventOutcomeFilter, setEventOutcomeFilter] = useRuntimePagePreference('run.outcome', ''); // '' | 'succeeded' | 'failed' | 'cancelled'
  const [eventSort, setEventSort] = useRuntimePagePreference('run.sort', 'desc'); // 'desc' = 最新优先, 'asc' = 最早优先
  const [selectedEventID, setSelectedEventID] = useState(null);
  const toggleSelectEvent = useCallback((id) => {
    setSelectedEventID((previous) => (previous === id ? null : id));
  }, []);
  const [expandedGroups, setExpandedGroups] = useState({});
  const toggleGroup = (key, open) => setExpandedGroups((previous) => (previous[key] === open ? previous : { ...previous, [key]: open }));
  const [requestChainSnapshot, setRequestChainSnapshot] = useState(null);
  const requestChainEvents = requestChainSnapshot?.events || [];
  const chainLoader = useRef(null);
  const refreshEnabled = useRef(autoRefresh);
  refreshEnabled.current = autoRefresh;
  const [eventPageDirection, setEventPageDirection] = useState('none');
  const [, setDurationTick] = useState(0);

  const isRunning = Boolean(status?.running);
  const recentEvents = runtime?.recentEvents || [];
  const [overview, setOverview] = useState(null);
  const [localDay, setLocalDay] = useState(() => runtimeTodayBounds().from);
  useEffect(() => observeRuntimeDay(setLocalDay), []);
  useEffect(() => {
    setSelectedEventID(null);
    setRequestChainSnapshot(null);
  }, [localDay]);
  const notifySnapshotRecovered = useCallback((message) => addToast(message, 'info'), [addToast]);
  const eventHistory = useRuntimeEventPage({
    range: RUN_OVERVIEW_RANGE,
    enabled: pageVisible,
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
    if (!pageVisible) return undefined;
    let cancelled = false;
    let pending = false;
    const controller = new AbortController();
    const load = () => {
      if (document.hidden || pending) return;
      pending = true;
      api.getRuntimeTrends(runOverviewQuery(), { signal: controller.signal }).then((series) => {
        if (cancelled) return;
        const totals = runOverviewTotals(series);
        setOverview(totals ? { status: 'ready', ...totals } : { status: 'error' });
      }).catch(() => {
        if (!cancelled && !controller.signal.aborted) {
          setOverview((current) => (current?.status === 'ready' ? current : { status: 'error' }));
        }
      }).finally(() => {
        pending = false;
      });
    };
    load();
    if (!autoRefresh) return () => { cancelled = true; controller.abort(); };
    const timer = window.setInterval(load, Math.max(15, Number(runRefreshInterval) || 15) * 1000);
    return () => {
      cancelled = true;
      controller.abort();
      window.clearInterval(timer);
    };
  }, [pageVisible, autoRefresh, runRefreshInterval, runtime?.resetGeneration, localDay]);

  useEffect(() => {
    if (!pageVisible || !recentEvents.some((event) => eventIsInFlight(event))) return undefined;
    const timer = window.setInterval(() => setDurationTick((value) => value + 1), 1000);
    return () => window.clearInterval(timer);
  }, [recentEvents, pageVisible]);

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
    for (const event of [...recentEvents, ...eventHistory.events]) {
      byID.set(event.id, mergeRuntimeEvent(byID.get(event.id), event));
    }
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
    for (const event of [...requestChainEvents, ...baseEventContext]) {
      byID.set(event.id, mergeRuntimeEvent(byID.get(event.id), event));
    }
    const eventContext = [...byID.values()];
    const chain = getRequestChain(eventContext, currentSelectedID);
    if (runtimeEventDetail?.event?.id !== currentSelectedID) return chain;
    return chain.map((event) => (event.id === currentSelectedID ? runtimeEventDetail.event : event));
  }, [baseEventContext, currentSelectedID, requestChainEvents, runtimeEventDetail]);

  const selectedRequestID = selectedEvent ? eventRequestID(selectedEvent) : '';
  const selectedChangeSeq = Math.max(
    Number(baseEventContext.find((event) => event.id === currentSelectedID)?.changeSeq || 0),
    Number(requestChainEvents.find((event) => event.id === currentSelectedID)?.changeSeq || 0),
  );

  useEffect(() => {
    setRequestChainSnapshot(null);
    if (!pageVisible || !selectedRequestID || selectedRequestID === '-') return undefined;
    const loader = createRequestChainLoader({
      read: (signal) => api.getRuntimeRequestChain(selectedRequestID, { signal }),
      onValue: (value) => setRequestChainSnapshot((previous) => (
        mergeRequestChain(value, previous?.requestID === selectedRequestID ? previous.events : [])
      )),
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
  }, [selectedRequestID, runtime?.resetGeneration, pageVisible]);

  useEffect(() => { chainLoader.current?.synchronize(); }, [autoRefresh]);
  useEffect(() => {
    if (!autoRefresh || document.hidden || !selectedRequestID) return;
    const relevant = baseEventContext.filter((e) => eventRequestID(e) === selectedRequestID);
    setRequestChainSnapshot((previous) => (
      previous?.requestID === selectedRequestID ? mergeRequestChain(previous, relevant) : previous
    ));
    chainLoader.current?.refresh();
  }, [autoRefresh, baseEventContext, selectedRequestID]);

  useEffect(() => {
    if (!currentSelectedID || !pageVisible) {
      loadRuntimeEvent(null);
      return;
    }
    loadRuntimeEvent(currentSelectedID, selectedChangeSeq).catch((error) => {
      addToast(`加载事件详情失败: ${error.message}`, 'error');
    });
  }, [addToast, currentSelectedID, selectedChangeSeq, loadRuntimeEvent, pageVisible]);

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
    <div className="space-y-6">
      {/* Page Header */}
      <div className="flex flex-wrap items-center justify-between gap-4">
        <div className="space-y-1">
          <h1 className="flex flex-wrap items-center gap-2.5 text-2xl font-semibold tracking-tight text-foreground">
            <Play className="size-5 text-primary" aria-hidden="true" />
            <span>运行</span>
            <StatusBadge status={status} />
          </h1>
          <p className="text-xs text-muted-foreground">启动状态、监听地址、统计和最近事件。</p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            type="button"
            variant={isRunning ? 'destructive' : 'default'}
            size="sm"
            onClick={toggleProxy}
            className="gap-1.5"
          >
            {isRunning ? <Square className="size-3.5" /> : <Play className="size-3.5" />}
            <span>{isRunning ? '停止运行' : '启动运行'}</span>
          </Button>
        </div>
      </div>

      {/* Last Error Banner if present */}
      {status?.lastError && (
        <div
          role="alert"
          className="flex items-center gap-2.5 rounded-xl border border-status-critical/40 bg-status-critical/10 px-4 py-3 text-xs text-status-critical"
        >
          <AlertTriangle className="size-4 shrink-0" aria-hidden="true" />
          <div>
            <strong className="font-semibold">最近错误告警：</strong> {status.lastError}
          </div>
        </div>
      )}

      {/* Hero Status & Listener Summary */}
      <Card className="run-hero-grid-single">
        <CardHeader className="flex flex-row flex-wrap items-start justify-between gap-3 space-y-0 pb-3">
          <div className="space-y-1">
            <div className="font-mono text-[11px] font-medium uppercase tracking-wider text-primary">
              服务监听状态
            </div>
            <CardDescription className="text-sm font-medium text-foreground">
              {status?.health?.headline || (isRunning ? '代理服务正常响应客户端请求' : '服务当前处于休眠停止状态')}
            </CardDescription>
          </div>
          <Badge variant="outline" className="font-mono text-xs tabular-nums">
            v{status?.version || '0.2.0'} · {status?.generation || '-'}
          </Badge>
        </CardHeader>
        <CardContent>
          <dl className="grid gap-3 border-t pt-4 sm:grid-cols-2 lg:grid-cols-5">
            <div className="space-y-1 sm:col-span-2 lg:col-span-1">
              <dt className="text-[11px] font-medium text-muted-foreground">HTTP 监听地址</dt>
              <dd className="flex items-center gap-1.5">
                <span className="font-mono text-xs font-semibold text-foreground tabular-nums">
                  {status?.listener ? `${status.listener.host}:${status.listener.port}` : '127.0.0.1:57878'}
                </span>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-6 gap-1 px-1.5 text-[11px]"
                  onClick={() => copyText(`http://${status?.listener?.host || '127.0.0.1'}:${status?.listener?.port || 57878}`, '监听地址')}
                  title="复制监听地址"
                >
                  <Copy className="size-3" />
                  <span>复制</span>
                </Button>
              </dd>
              {status?.listener?.httpsPort != null && (
                <div className="pt-1">
                  <dt className="text-[11px] font-medium text-muted-foreground">HTTPS 监听地址</dt>
                  <dd className="mt-0.5 flex items-center gap-1.5">
                    <span className="font-mono text-xs font-semibold text-foreground tabular-nums">
                      {status.listener.host}:{status.listener.httpsPort}
                    </span>
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      className="size-6"
                      title="复制 HTTPS 地址"
                      aria-label="复制 HTTPS 地址"
                      onClick={() => copyText(`https://${status.listener.host.includes(':') ? `[${status.listener.host}]` : status.listener.host}:${status.listener.httpsPort}`, 'HTTPS 地址')}
                    >
                      <Copy className="size-3" />
                    </Button>
                  </dd>
                </div>
              )}
            </div>
            <div className="space-y-1">
              <dt className="text-[11px] font-medium text-muted-foreground">配置版本</dt>
              <dd className="font-mono text-xs font-semibold text-foreground">v{config?.schemaVersion || 7}</dd>
            </div>
            <div className="space-y-1">
              <dt className="text-[11px] font-medium text-muted-foreground">认证状态</dt>
              <dd
                className={cn(
                  'text-xs font-medium',
                  secretStatus?.inboundAuthToken?.configured ? 'text-status-good' : 'text-foreground',
                )}
              >
                {secretStatus?.inboundAuthToken?.configured
                  ? `已启用 · 尾号 ${secretStatus.inboundAuthToken.last4}`
                  : '未启用 · 本机免密'}
              </dd>
            </div>
            <div className="space-y-1">
              <dt className="text-[11px] font-medium text-muted-foreground">运行时长</dt>
              <dd className="font-mono text-xs font-medium text-foreground tabular-nums">
                {status?.uptimeSeconds ? formatDuration(status.uptimeSeconds * 1000) : '-'}
              </dd>
            </div>
            <div className="space-y-1">
              <dt className="text-[11px] font-medium text-muted-foreground">调度策略</dt>
              <dd className="text-xs font-medium text-foreground">优先级 + 会话粘性绑定</dd>
            </div>
          </dl>
        </CardContent>
      </Card>

      {/* Today's usage cards and current routing inventory (strict order preserved) */}
      <div className="run-metric-grid grid gap-3 sm:grid-cols-2 lg:grid-cols-5" aria-label="今天用量与当前入口">
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
      <Card className="run-events-panel overflow-hidden py-0">
        <CardHeader className="flex flex-col gap-3 border-b py-4 lg:flex-row lg:items-center lg:justify-between">
          <div className="space-y-1">
            <CardTitle className="flex items-center gap-2 text-base font-semibold">
              <Activity className="size-4 text-primary" aria-hidden="true" />
              <span>今日事件与遥测日志</span>
            </CardTitle>
            <CardDescription className="text-xs">
              客户端请求和上游尝试通过请求 ID 关联；点击一行后查看完整链路。HTTP 状态与最终结果分开显示，200 后的协议中断不会被误报为成功。
            </CardDescription>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={eventFilter}
              onValueChange={(value) => {
                if (!value) return;
                setSelectedEventID(null);
                setEventPageDirection('replace');
                setEventFilter(value);
              }}
              aria-label="事件类型筛选"
            >
              <ToggleGroupItem value="client">客户端</ToggleGroupItem>
              <ToggleGroupItem value="upstream">上游</ToggleGroupItem>
              <ToggleGroupItem value="all">全部</ToggleGroupItem>
            </ToggleGroup>

            <ToggleGroup
              type="single"
              variant="outline"
              size="sm"
              value={eventOutcomeFilter || 'all-outcomes'}
              onValueChange={(value) => {
                if (!value) return;
                setSelectedEventID(null);
                setEventPageDirection('replace');
                setEventOutcomeFilter(value === 'all-outcomes' ? '' : value);
              }}
              aria-label="事件结果状态筛选"
            >
              <ToggleGroupItem value="all-outcomes">全部状态</ToggleGroupItem>
              <ToggleGroupItem value="succeeded">成功</ToggleGroupItem>
              <ToggleGroupItem value="failed">失败</ToggleGroupItem>
              <ToggleGroupItem value="cancelled">已取消</ToggleGroupItem>
            </ToggleGroup>

            {eventHistory.mode !== 'page' && (
              <span className="text-xs text-muted-foreground tabular-nums">
                {formatNumber(visibleEvents.length)} 条持久事件
              </span>
            )}

            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-xs"
              onClick={() => setEventSort((current) => (current === 'desc' ? 'asc' : 'desc'))}
              aria-label={`切换当前页事件时间排序，当前为${eventSort === 'desc' ? '最新优先' : '最早优先'}`}
              title="按当前页事件时间稳定排序"
            >
              <ArrowUpDown className="size-3.5" aria-hidden="true" />
              <span>{eventHistory.mode === 'page' ? '本页' : ''}{eventSort === 'desc' ? '最新优先' : '最早优先'}</span>
            </Button>
          </div>
        </CardHeader>

        <PageTelemetryStrip events={visibleEvents} />

        <div>
          {eventHistory.error && (
            <div
              role="alert"
              className="flex flex-wrap items-center justify-between gap-2 border-b bg-status-critical/10 px-4 py-2.5 text-xs text-status-critical"
            >
              <span>读取事件历史失败：{eventHistory.error}</span>
              <Button type="button" variant="outline" size="sm" className="h-7 text-xs" onClick={eventHistory.retry}>
                重试
              </Button>
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
              {liveEvents.length > 0 && (
                <div className="telemetry-live-section">
                  <div className="telemetry-desktop-table hidden md:block">
                    <LiveEventList events={liveEvents} selectedEventID={selectedEvent?.id} onSelect={toggleSelectEvent} />
                  </div>
                  <div className="telemetry-live-mobile md:hidden">
                    <div className="flex items-center justify-between border-b bg-primary/10 px-4 py-2 text-xs font-medium">
                      <span className="inline-flex items-center gap-1.5 text-foreground">
                        <i className="size-1.5 rounded-full bg-primary animate-pulse" aria-hidden="true" />
                        进行中 · {formatNumber(liveEvents.length)}
                      </span>
                      <small className="text-[11px] text-primary">上游暂计</small>
                    </div>
                    <MobileEventList
                      events={liveEvents}
                      selectedEventID={selectedEvent?.id}
                      onSelect={toggleSelectEvent}
                      live
                    />
                  </div>
                </div>
              )}

              <section className="telemetry-history-group" aria-label="历史事件">
                <div className="telemetry-desktop-table hidden md:block">
                  <DataTable
                    className="telemetry-table"
                    columns={columns}
                    data={visibleEvents}
                    keyField="id"
                    ariaLabel={eventHistory.mode === 'page' ? 'SQLite 持久事件分页列表' : '最近请求事件列表'}
                    tableMinWidth="960px"
                    sortKey="timestamp"
                    sortDirection={eventSort}
                    onSortChange={(_key, direction) => setEventSort(direction)}
                    onRowClick={(row) => toggleSelectEvent(row.id)}
                    activeRowKey={selectedEvent?.id}
                    emptyText={eventHistory.loading ? '正在读取事件历史…' : '当前筛选条件下暂无请求事件'}
                  />
                </div>
                <div className="telemetry-mobile-list md:hidden">
                  <MobileEventList
                    events={visibleEvents}
                    selectedEventID={selectedEvent?.id}
                    onSelect={toggleSelectEvent}
                    loading={eventHistory.loading}
                  />
                </div>
              </section>

              {!selectedEvent && (visibleEvents.length > 0 || liveEvents.length > 0) && (
                <div
                  className="border-t bg-muted/15 px-4 py-2 text-center text-[11px] text-muted-foreground"
                  role="status"
                >
                  选择一条事件查看请求链路与诊断详情
                </div>
              )}

              {eventHistory.mode === 'page' && visibleEvents.length >= 15 && eventHistory.totalPages > 1 ? (
                <div className="border-t">
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
            <div className="border-t bg-muted/20 px-4 py-2 text-xs text-muted-foreground" role="status">
              当前 daemon 尚未提供全量快照分页，暂时显示最近事件；升级 daemon 后可选择每页 10 / 25 / 50 / 100 / 200 条。
            </div>
          ) : null}
        </div>

        {selectedEvent && (
          <EventInspector
            event={selectedEvent}
            chain={requestChain}
            chainTruncated={requestChainSnapshot?.truncated}
            onSelect={setSelectedEventID}
            onClose={() => setSelectedEventID(null)}
            requestSummary={recentEventRequestSummary(selectedEvent)}
            expanded={expandedGroups}
            onToggle={toggleGroup}
            onCopy={copyText}
            loading={runtimeEventDetail?.event?.id !== selectedEvent.id && selectedEvent.detailsOmitted === true}
          />
        )}
      </Card>
    </div>
  );
}
