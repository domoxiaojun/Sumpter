import { ChevronRight, Copy, Monitor, X } from 'lucide-react';
import { cn } from '../lib/utils.js';
import { CacheHitRate } from './CacheHitRate.jsx';
import { RequestTrace } from './RequestTrace.jsx';
import { Badge } from './ui/badge.jsx';
import { Button } from './ui/button.jsx';
import {
  requestKindLabel, clientVariantLabel, agentRoleLabel, clientAttributionSummary, eventModel, eventEndpoint, eventEndpointName, eventDurationMS, eventProjectContext, eventClientKindLabel, eventKindLabel,
  eventHttpStatusLabel, eventOutcomeLabel, eventDurationText, friendlyEventMessage, eventFailureSummaryLabel,
  eventRequestID, eventSessionID, eventToolCalls, eventStreamTrace, formatTimestamp, formatDuration,
  eventTTFBMS, eventUpstreamStatusLabel, eventFailureDetail, eventPurposeLabel, eventProtocolRouteLabel,
  eventTransportLabel, eventIsInFlight, eventOutcome, statusKind, eventUpstreamRequestID, codexMetadataSummary,
} from '../utils/helpers.js';
import { eventDetailGroups, eventAgentLabel, eventCacheLabel, eventCacheRead, eventCacheStatusLabel, eventCacheTokenRatio, eventCacheReason, eventUsage, eventUsageLabel, eventHttpTone } from '../utils/eventPresentation.js';
import { formatOutputTokensPerSecond } from '../utils/outputSpeed.js';

function Field({ label, value, color }) {
  return (
    <div className="rounded-lg border border-border/60 bg-muted/20 px-3 py-2">
      <dt className="text-[11px] font-medium text-muted-foreground">{label}</dt>
      <dd
        className="mt-0.5 break-all text-xs font-medium text-foreground tabular-nums"
        style={color ? { color } : undefined}
      >
        {value ?? '—'}
      </dd>
    </div>
  );
}

function outcomeLabel(event) {
  if (event.kind === 'notify') return '不适用（通知事件）';
  if (eventIsInFlight(event)) return '进行中';
  if (event.failureKind === 'client_disconnected' && eventOutcome(event) === 'cancelled') return '连接中断';
  return ({ succeeded: '成功', failed: '失败', cancelled: '已取消' })[eventOutcome(event)] || '未上报';
}

function toneTextClass(tone) {
  if (tone === 'good') return 'text-status-good';
  if (tone === 'warning') return 'text-status-warning';
  if (tone === 'critical') return 'text-status-critical';
  if (tone === 'live') return 'text-primary';
  return 'text-muted-foreground';
}

function ChainWaterfallBar({ item, maxDurationMS }) {
  if (item.kind === 'notify') return null;
  const duration = Number(eventDurationMS(item));
  if (!Number.isFinite(duration) || duration <= 0) return null;
  const rawTTFB = eventTTFBMS(item);
  const ttfb = rawTTFB != null && Number.isFinite(Number(rawTTFB))
    ? Math.min(duration, Math.max(0, Number(rawTTFB)))
    : null;
  const scaleMax = Math.max(duration, Number(maxDurationMS) || 1);
  const totalPct = Math.min(100, Math.max(6, (duration / scaleMax) * 100));
  const ttfbShare = ttfb != null && duration > 0 ? Math.min(100, Math.max(0, (ttfb / duration) * 100)) : 0;
  return (
    <span
      className="flex flex-col gap-1 py-0.5"
      title={`首字节 ${formatDuration(ttfb)} / 总耗时 ${formatDuration(duration)}`}
    >
      <span className="flex h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <span className="flex h-full overflow-hidden rounded-full" style={{ width: `${totalPct}%` }}>
          {ttfb != null && (
            <span className="h-full bg-accent-cyan" style={{ width: `${ttfbShare}%` }} />
          )}
          <span className="h-full bg-primary" style={{ width: `${Math.max(0, 100 - ttfbShare)}%` }} />
        </span>
      </span>
      <small className="font-mono text-[10px] text-muted-foreground tabular-nums">
        {ttfb != null ? `TTFB ${formatDuration(ttfb)} → ${formatDuration(duration)}` : formatDuration(duration)}
      </small>
    </span>
  );
}

function RequestChain({ event, chain, onSelect, truncated }) {
  const items = chain.length ? chain : [event];
  const primary = items.find((item) => item.kind === 'client' || item.kind === 'notify');
  // The API/context is newest first; number attempts in execution order.
  const attempts = items.filter((item) => item.kind === 'upstream').reverse();
  const maxDurationMS = Math.max(1, ...items.map((item) => Number(eventDurationMS(item) || 0)));

  const traceItem = (item, number) => {
    const upstream = item.kind === 'upstream';
    const requestID = eventRequestID(item);
    const upstreamID = eventUpstreamRequestID(item);
    const tools = eventToolCalls(item).join('、');
    const diagnostics = [
      !upstream && requestID !== '-' ? `请求=${requestID}` : null,
      upstream ? eventDurationText(item) : eventEndpointName(item),
      item.kind !== 'notify' ? `传输=${eventTransportLabel(item)}` : null,
      upstream && item.endpointID ? `入口 ID=${item.endpointID}` : null,
      upstreamID && upstreamID !== '-' ? `上游请求=${upstreamID}` : null,
      tools ? `工具=${tools}` : null,
      !upstream ? codexMetadataSummary(item) : null,
    ].filter(Boolean).join(' · ');
    const selected = item.id === event.id;
    const tone = statusKind(item);
    return (
      <button
        key={item.id}
        type="button"
        className={cn(
          'flex w-full cursor-pointer items-start gap-2.5 rounded-xl border p-3 text-left transition-colors',
          selected
            ? 'border-primary/50 bg-primary/10 shadow-xs'
            : 'border-border/70 bg-card hover:bg-muted/40',
        )}
        aria-label={upstream ? `查看第 ${number} 次上游尝试详情` : `查看${eventKindLabel(item.kind)}请求详情`}
        aria-pressed={selected}
        onClick={() => onSelect(item.id)}
      >
        {upstream && (
          <span
            className="mt-0.5 flex size-5 shrink-0 items-center justify-center rounded-full border font-mono text-[11px] font-semibold text-foreground"
            style={{ borderColor: `var(--status-${tone})` }}
            aria-hidden="true"
          >
            {number}
          </span>
        )}
        <span className="flex min-w-0 flex-1 flex-col gap-1">
          <span className="flex items-center justify-between gap-2">
            <strong className="flex min-w-0 items-center gap-1.5 truncate text-xs font-semibold text-foreground">
              {!upstream && <Monitor className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />}
              <span className="truncate">{upstream ? eventEndpointName(item) : eventClientKindLabel(item)}</span>
            </strong>
            <span className={cn('inline-flex shrink-0 items-center gap-1 text-[11px] font-semibold', toneTextClass(tone))}>
              {eventIsInFlight(item) && (
                <i className="telemetry-live-pill-dot size-1.5 rounded-full bg-current" aria-hidden="true" />
              )}
              {outcomeLabel(item)}
            </span>
          </span>
          {item.kind !== 'notify' && (
            <>
              <ChainWaterfallBar item={item} maxDurationMS={maxDurationMS} />
              <span className="font-mono text-[11px] text-muted-foreground">
                {eventHttpStatusLabel(item)} · {eventModel(item)}
              </span>
              <span className="font-mono text-[11px] text-foreground/90 tabular-nums">
                {eventUsageLabel(item)}
              </span>
              <span className="inline-flex flex-wrap items-center gap-1 text-[11px] text-muted-foreground">
                <span>{eventCacheLabel(item)}</span>
                <CacheHitRate event={item} />
                {eventCacheRead(item).finality === 'provisional' ? <span>· 暂计</span> : null}
              </span>
            </>
          )}
          <span className="break-all font-mono text-[10px] text-muted-foreground/85">{diagnostics}</span>
          {upstream && (friendlyEventMessage(item) || (eventOutcome(item) === 'failed' ? eventFailureSummaryLabel(item) : '')) && (
            <span className="text-[11px] text-foreground/90">
              {friendlyEventMessage(item) || eventFailureSummaryLabel(item)}
            </span>
          )}
          {upstream && eventFailureDetail(item) && (
            <span className="break-all font-mono text-[10px] text-status-critical">
              {eventFailureDetail(item)}
            </span>
          )}
        </span>
      </button>
    );
  };

  return (
    <aside className="space-y-2.5 lg:border-r lg:pr-5" aria-label="请求链">
      <div className="flex items-center justify-between gap-2">
        <h3 className="text-xs font-semibold text-foreground">请求链</h3>
        <Badge variant="outline" className="text-[10px] font-normal tabular-nums">
          {truncated ? '至少 ' : ''}{attempts.length} 次上游尝试{truncated ? '（已截断）' : ''}
        </Badge>
      </div>
      <div className="space-y-2" role="region" aria-label="请求链事件列表">
        {primary && traceItem(primary)}
        {attempts.map((item, index) => traceItem(item, index + 1))}
        {!attempts.length && (
          <p className="rounded-lg border border-dashed px-3 py-4 text-center text-xs text-muted-foreground">
            尚未产生上游尝试
          </p>
        )}
      </div>
    </aside>
  );
}

export function EventInspector({
  event,
  chain = [],
  chainTruncated = false,
  requestSummary,
  onSelect,
  onClose,
  expanded,
  onToggle,
  onCopy,
  loading = false,
}) {
  const notify = event.kind === 'notify';
  const cache = eventCacheRead(event);
  const usage = eventUsage(event);
  const cacheRatio = eventCacheTokenRatio(event);
  const metadata = event.codexMetadata || {};
  const fields = (pairs) => (
    <dl className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3">
      {pairs.map(([label, value]) => <Field key={label} label={label} value={value} />)}
    </dl>
  );
  const attempts = chain.filter((item) => item.kind === 'upstream');
  const summary = {
    routing: `${eventEndpointName(event)} · ${attempts.length} 次上游尝试`,
    usage: eventCacheLabel(event),
    identity: eventAgentLabel(event) || '会话、父子关系与归因来源',
    response: `${event.requestMethod || '—'} · ${eventHttpStatusLabel(event)}`,
    tools: eventToolCalls(event).length ? `${eventToolCalls(event).length} 种工具` : (event.hookEvent || '按需查看'),
    advanced: '元数据、流观测与原始事件',
  };

  const content = (key) => {
    if (key === 'routing') {
      return (
        <div className="space-y-3">
          {fields([
            ['客户端模型', event.clientModel],
            ['逻辑模型', event.effectiveModel],
            ['上游模型', event.upstreamModel],
            ['入口', eventEndpoint(event)],
            ['模型组', event.modelGroupName || event.modelGroupID],
            ['命中规则', event.featureRuleID],
            ['用途', eventPurposeLabel(event)],
          ])}
          <RequestTrace trace={event.requestTrace || chain.find((item) => item.kind === 'client')?.requestTrace} />
        </div>
      );
    }
    if (key === 'usage') {
      return fields([
        ['缓存读状态', eventCacheStatusLabel(event)],
        ['缓存证据', eventCacheReason(event)],
        ['缓存读计数', ({ confirmed: '已确认', provisional: '暂定，可能增加', unknown: '未知' })[cache.finality] || '未知'],
        ['缓存读占比', cacheRatio == null ? '—' : `${(cacheRatio * 100).toFixed(1)}%`],
        ['输入 token', usage?.inputTokens],
        ['输出 token', usage?.outputTokens],
        ['输出速度', formatOutputTokensPerSecond(event.outputTokensPerSecond)],
        ['缓存读 token', usage?.cacheReadInputTokens],
        ['缓存写 token', usage?.cacheCreationInputTokens],
        ['推理 token（输出子集）', usage?.reasoningTokens],
      ]);
    }
    if (key === 'identity') {
      return fields([
        ['代理', eventAgentLabel(event) || agentRoleLabel(event.agentRole)],
        ['客户端形态', clientVariantLabel(event.clientVariant)],
        ['请求类型', requestKindLabel(event.requestKind)],
        ['归因依据', clientAttributionSummary(event.clientAttribution)],
        ['请求 ID', eventRequestID(event)],
        ['会话 ID', eventSessionID(event)],
        ['会话来源', event.sessionSource],
        ['线程 ID', metadata.threadID],
        ['回合 ID', metadata.turnID],
        ['父线程', event.parentThreadId ?? metadata.parentThreadID],
        ['父回合', event.parentTurnId ?? metadata.parentTurnID],
        ['根回合', event.rootTurnId ?? metadata.rootTurnID],
        ['Grok agent ID', event.grokMetadata?.agentID],
        ['项目来源', eventProjectContext(event).source],
        ['工作区', event.clientDeclared?.workspace],
      ]);
    }
    if (key === 'response') {
      return fields([
        ['方法', event.requestMethod],
        ['路径', event.requestPath],
        ['传输', eventTransportLabel(event)],
        ['协议', eventProtocolRouteLabel(event)],
        ['本层 HTTP', eventHttpStatusLabel(event)],
        ['直接上游 HTTP', eventUpstreamStatusLabel(event)],
        ['最终结果', outcomeLabel(event)],
        ['失败类型 / 阶段', eventFailureSummaryLabel(event)],
        ['停止原因', eventStreamTrace(event)?.stopReason],
        ['超时阈值', event.timeoutMS == null ? '—' : formatDuration(event.timeoutMS)],
        ['错误详情', eventFailureDetail(event)],
      ]);
    }
    if (key === 'tools') {
      return fields([
        ['工具名称（去重）', eventToolCalls(event).join('、') || '未观测到'],
        ['工具观测截断', eventStreamTrace(event)?.toolCallsTruncated ? '是' : '否'],
        ['Hook 类型', event.hookEvent],
        ['通知文案', notify ? event.message : null],
      ]);
    }
    return (
      <div className="space-y-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="gap-1.5 text-xs"
          onClick={() => onCopy(JSON.stringify(event, null, 2), '完整事件 JSON')}
        >
          <Copy className="size-3.5" />
          复制完整事件 JSON
        </Button>
        {fields([
          ['上游追踪 ID', event.upstreamRequestID],
          ['上游主机', event.upstreamHost],
          ['入口 ID', event.endpointID],
          ['事件 ID', event.id],
        ])}
        <pre className="max-h-72 overflow-auto rounded-lg border bg-muted/40 p-3 font-mono text-[11px] leading-relaxed text-foreground">
          {JSON.stringify({
            clientAttribution: event.clientAttribution,
            requestKind: event.requestKind,
            codexMetadata: event.codexMetadata,
            grokMetadata: event.grokMetadata,
            clientDeclared: event.clientDeclared,
            streamTrace: event.streamTrace,
            message: event.message,
          }, null, 2)}
        </pre>
      </div>
    );
  };

  return (
    <div className="grid gap-5 border-t bg-muted/10 p-4 md:p-5 lg:grid-cols-[320px_minmax(0,1fr)]">
      <RequestChain event={event} chain={chain} onSelect={onSelect} truncated={chainTruncated} />
      <section className="min-w-0 space-y-4" aria-label="选中事件详情">
        <div className="flex items-center justify-between gap-2">
          <h3 className="text-sm font-semibold text-foreground">选中事件</h3>
          {onClose && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 gap-1 px-2.5 text-xs"
              onClick={onClose}
              aria-label="收起选中事件详情"
            >
              <X className="size-3.5" />
              <span>收起详情</span>
            </Button>
          )}
        </div>

        <dl className="grid gap-2 sm:grid-cols-2 xl:grid-cols-3" data-event-detail-tier="primary">
          <Field
            label="时间 / 类型"
            value={`${formatTimestamp(event.timestamp, { date: true })} · ${eventKindLabel(event.kind)}`}
          />
          <Field
            label="客户端 / 项目"
            value={requestSummary || `${eventClientKindLabel(event)} · ${eventProjectContext(event).label}`}
          />
          <Field label="请求源 IP" value={event.sourceIP} />
          {notify && <Field label="Hook 类型" value={event.hookEvent} />}
          {!notify && (
            <>
              <Field label="客户端模型 / 入口" value={`${event.clientModel || eventModel(event)} · ${eventEndpointName(event)}`} />
              <Field
                label={event.kind === 'upstream' ? '上游 HTTP' : '客户端 HTTP'}
                value={eventHttpStatusLabel(event)}
                color={eventHttpTone(event)}
              />
              <Field
                label="最终结果"
                value={outcomeLabel(event)}
                color={`var(--status-${statusKind(event)})`}
              />
              <Field
                label="TTFB / 总耗时"
                value={`${formatDuration(eventTTFBMS(event))} / ${formatDuration(eventDurationMS(event))}`}
              />
              {event.kind === 'client' && (event.upstreamTTFBMS != null || event.upstreamDurationMS != null) && (
                <Field
                  label="上游 TTFB / 总耗时"
                  value={`${formatDuration(event.upstreamTTFBMS)} / ${formatDuration(event.upstreamDurationMS)}`}
                />
              )}
              <Field
                label="缓存 / 用量"
                value={
                  <span className="inline-flex flex-wrap items-center gap-1">
                    <span>{eventCacheLabel(event)}</span>
                    <CacheHitRate event={event} />
                    <span>· {eventUsageLabel(event)}</span>
                  </span>
                }
              />
            </>
          )}
        </dl>

        {(friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : notify ? event.message : '')) && (
          <p
            className={cn(
              'rounded-lg border px-3 py-2 text-xs leading-relaxed',
              event.outcome === 'failed'
                ? 'border-status-critical/35 bg-status-critical/10 text-status-critical'
                : 'border-border/70 bg-muted/30 text-foreground/90',
            )}
          >
            {event.failover ? '重试恢复 / 故障转移 · ' : ''}
            {friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : notify ? event.message : '')}
          </p>
        )}

        <div className="space-y-2">
          {eventDetailGroups
            .filter(([key]) => !notify || ['identity', 'tools', 'advanced'].includes(key))
            .map(([key, label]) => {
              const open = expanded?.[`${event.id}:${key}`] === true;
              return (
                <details
                  key={key}
                  className="group rounded-xl border bg-card/60"
                  open={open}
                  onToggle={(e) => onToggle(`${event.id}:${key}`, e.currentTarget.open)}
                  data-event-detail-tier={key === 'advanced' ? 'advanced' : 'secondary'}
                >
                  <summary className="flex cursor-pointer list-none items-center gap-2 px-3.5 py-2.5 text-xs select-none hover:bg-muted/40">
                    <ChevronRight className="size-3.5 shrink-0 text-muted-foreground transition-transform group-open:rotate-90" aria-hidden="true" />
                    <span className="flex min-w-0 flex-1 flex-wrap items-baseline justify-between gap-2">
                      <strong className="font-semibold text-foreground">{label}</strong>
                      <small className="truncate font-normal text-muted-foreground">{summary[key]}</small>
                    </span>
                  </summary>
                  {open && (
                    <div className="border-t px-3.5 py-3">
                      {loading ? <p role="status" className="text-xs text-muted-foreground">正在加载详情…</p> : content(key)}
                    </div>
                  )}
                </details>
              );
            })}
        </div>
      </section>
    </div>
  );
}
