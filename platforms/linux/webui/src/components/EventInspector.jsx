import React from 'react';
import { CacheHitRate } from './CacheHitRate.jsx';
import {
  eventModel, eventEndpoint, eventEndpointName, eventDurationMS, eventProjectContext, eventClientKindLabel, eventKindLabel,
  eventHttpStatusLabel, eventOutcomeLabel, eventDurationText, friendlyEventMessage, eventFailureSummaryLabel,
  eventRequestID, eventSessionID, eventToolCalls, eventStreamTrace, formatTimestamp, formatDuration,
  eventTTFBMS, eventUpstreamStatusLabel, eventFailureDetail, eventPurposeLabel, eventProtocolRouteLabel,
  eventIsInFlight, eventOutcome, statusKind, eventUpstreamRequestID, codexMetadataSummary,
} from '../utils/helpers.js';
import { eventDetailGroups, eventAgentLabel, eventCacheLabel, eventCacheRead, eventCacheStatusLabel, eventCacheTokenRatio, eventCacheReason, eventUsage, eventUsageLabel, eventHttpTone } from '../utils/eventPresentation.js';

function Field({ label, value, color }) {
  return <div className="event-inspector-field"><dt>{label}</dt><dd style={color ? { color } : undefined}>{value ?? '—'}</dd></div>;
}

function outcomeLabel(event) {
  if (event.kind === 'notify') return '不适用（通知事件）';
  if (eventIsInFlight(event)) return '进行中';
  return ({ succeeded: '成功', failed: '失败', cancelled: '已取消' })[eventOutcome(event)] || '未上报';
}

function RequestChain({ event, chain, onSelect }) {
  const items = chain.length ? chain : [event];
  const primary = items.find((item) => item.kind === 'client' || item.kind === 'notify');
  // The API/context is newest first; number attempts in execution order.
  const attempts = items.filter((item) => item.kind === 'upstream').reverse();
  const traceItem = (item, number) => {
    const upstream = item.kind === 'upstream';
    const requestID = eventRequestID(item);
    const upstreamID = eventUpstreamRequestID(item);
    const tools = eventToolCalls(item).join('、');
    const diagnostics = [
      !upstream && requestID !== '-' ? `请求=${requestID}` : null,
      upstream ? eventDurationText(item) : eventEndpointName(item),
      upstream && item.endpointID ? `入口 ID=${item.endpointID}` : null,
      upstreamID && upstreamID !== '-' ? `上游请求=${upstreamID}` : null,
      tools ? `工具=${tools}` : null,
      !upstream ? codexMetadataSummary(item) : null,
    ].filter(Boolean).join(' · ');
    return <button key={item.id} type="button"
      className={`event-chain-button${upstream ? ' is-attempt' : ''}`}
      aria-label={upstream ? `查看第 ${number} 次上游尝试详情` : `查看${eventKindLabel(item.kind)}请求详情`}
      aria-pressed={item.id === event.id} onClick={() => onSelect(item.id)}
      style={{ '--chain-tone': `var(--status-${statusKind(item)})` }}>
      {upstream && <span className="event-chain-number" aria-hidden="true">{number}</span>}
      <span className="event-chain-content">
        <strong className="event-chain-title">
          {!upstream && <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" aria-hidden="true"><rect x="3" y="4" width="18" height="13" rx="2" /><path d="M8 21h8M12 17v4" /></svg>}
          {upstream ? eventEndpointName(item) : eventClientKindLabel(item)}
        </strong>
        <span className={`telemetry-outcome-text ${statusKind(item)}`}>{outcomeLabel(item)}</span>
        {item.kind !== 'notify' && <>
          <span className="event-chain-secondary">{eventHttpStatusLabel(item)}</span>
          <span className="event-chain-usage">{eventUsageLabel(item)}</span>
          <span className="event-chain-secondary event-cache-inline">{eventCacheLabel(item)} <CacheHitRate event={item} />{eventCacheRead(item).finality === 'provisional' ? ' · 暂计' : ''}</span>
          <span className="event-chain-secondary">{eventModel(item)}</span>
        </>}
        <span className="event-chain-diagnostics">{diagnostics}</span>
        {upstream && <span className="event-chain-message">{friendlyEventMessage(item) || (eventOutcome(item) === 'failed' ? eventFailureSummaryLabel(item) : '')}</span>}
        {upstream && eventFailureDetail(item) && <span className="event-chain-diagnostics">{eventFailureDetail(item)}</span>}
      </span>
    </button>;
  };
  return <aside className="request-chain-pane" aria-label="请求链">
    <div className="request-chain-header"><h3>请求链</h3><span>{attempts.length} 次上游尝试</span></div>
    <div className="request-chain-list" role="region" aria-label="请求链事件列表">
      {primary && traceItem(primary)}
      {attempts.map((item, index) => traceItem(item, index + 1))}
      {!attempts.length && <p className="request-chain-empty">尚未产生上游尝试</p>}
    </div>
  </aside>;
}

export function EventInspector({ event, chain = [], requestSummary, onSelect, expanded, onToggle, onCopy, loading = false }) {
  const notify = event.kind === 'notify';
  const cache = eventCacheRead(event);
  const usage = eventUsage(event);
  const cacheRatio = eventCacheTokenRatio(event);
  const metadata = event.codexMetadata || {};
  const fields = (pairs) => <dl className="event-inspector-fields">{pairs.map(([label, value]) => <Field key={label} label={label} value={value} />)}</dl>;
  const attempts = chain.filter((item) => item.kind === 'upstream');
  const summary = {
    routing: `${eventEndpointName(event)} · ${attempts.length} 次上游尝试`,
    usage: eventCacheLabel(event), identity: eventAgentLabel(event) || '会话、父子关系与归因来源',
    response: `${event.requestMethod || '—'} · ${eventHttpStatusLabel(event)}`,
    tools: eventToolCalls(event).length ? `${eventToolCalls(event).length} 种工具` : (event.hookEvent || '按需查看'),
    advanced: '元数据、流观测与原始事件',
  };
  const content = (key) => {
    if (key === 'routing') return <>
      {fields([
        ['客户端模型', event.clientModel], ['逻辑模型', event.effectiveModel], ['上游模型', event.upstreamModel],
        ['入口', eventEndpoint(event)], ['模型组', event.modelGroupName || event.modelGroupID],
        ['命中规则', event.featureRuleID], ['用途', eventPurposeLabel(event)],
      ])}
    </>;
    if (key === 'usage') return fields([
      ['缓存读状态', eventCacheStatusLabel(event)], ['缓存证据', eventCacheReason(event)],
      ['缓存读计数', ({ confirmed: '已确认', provisional: '暂定，可能增加', unknown: '未知' })[cache.finality] || '未知'],
      ['缓存读占比', cacheRatio == null ? '—' : `${(cacheRatio * 100).toFixed(1)}%`],
      ['输入 token', usage?.inputTokens], ['输出 token', usage?.outputTokens],
      ['缓存读 token', usage?.cacheReadInputTokens], ['缓存写 token', usage?.cacheCreationInputTokens],
      ['推理 token（输出子集）', usage?.reasoningTokens],
    ]);
    if (key === 'identity') return fields([
      ['代理', eventAgentLabel(event) || '未确定'], ['客户端形态', event.clientVariant],
      ['请求 ID', eventRequestID(event)], ['会话 ID', eventSessionID(event)], ['会话来源', event.sessionSource],
      ['线程 ID', metadata.threadID], ['回合 ID', metadata.turnID],
      ['父线程', event.parentThreadId ?? metadata.parentThreadID], ['父回合', event.parentTurnId ?? metadata.parentTurnID],
      ['根回合', event.rootTurnId ?? metadata.rootTurnID], ['Grok agent ID', event.grokMetadata?.agentID],
      ['项目来源', eventProjectContext(event).source], ['工作区', event.clientDeclared?.workspace],
    ]);
    if (key === 'response') return fields([
      ['方法', event.requestMethod], ['路径', event.requestPath], ['协议', eventProtocolRouteLabel(event)],
      ['本层 HTTP', eventHttpStatusLabel(event)], ['直接上游 HTTP', eventUpstreamStatusLabel(event)],
      ['最终结果', eventOutcomeLabel(event)], ['失败类型 / 阶段', eventFailureSummaryLabel(event)],
      ['停止原因', eventStreamTrace(event)?.stopReason], ['超时阈值', event.timeoutMS == null ? '—' : formatDuration(event.timeoutMS)],
      ['错误详情', eventFailureDetail(event)],
    ]);
    if (key === 'tools') return fields([
      ['工具名称（去重）', eventToolCalls(event).join('、') || '未观测到'],
      ['工具观测截断', eventStreamTrace(event)?.toolCallsTruncated ? '是' : '否'],
      ['Hook 类型', event.hookEvent], ['通知文案', notify ? event.message : null],
    ]);
    return <>
      <button type="button" className="btn btn-secondary" onClick={() => onCopy(JSON.stringify(event, null, 2), '完整事件 JSON')}>复制完整事件 JSON</button>
      {fields([['上游追踪 ID', event.upstreamRequestID], ['上游主机', event.upstreamHost], ['入口 ID', event.endpointID], ['事件 ID', event.id]])}
      <pre className="technical-pre event-inspector-json">{JSON.stringify({
        codexMetadata: event.codexMetadata, grokMetadata: event.grokMetadata, clientDeclared: event.clientDeclared,
        streamTrace: event.streamTrace, message: event.message,
      }, null, 2)}</pre>
    </>;
  };
  return <div className="request-detail-grid">
    <RequestChain event={event} chain={chain} onSelect={onSelect} />
    <section className="event-inspector" aria-label="选中事件详情">
    <h3>选中事件</h3>
    <dl className="event-inspector-fields" data-event-detail-tier="primary">
      <Field label="时间 / 类型" value={`${formatTimestamp(event.timestamp, { date: true })} · ${eventKindLabel(event.kind)}`} />
      <Field label="客户端 / 项目" value={requestSummary || `${eventClientKindLabel(event)} · ${eventProjectContext(event).label}`} />
      {notify && <Field label="Hook 类型" value={event.hookEvent} />}
      {!notify && <>
        <Field label="客户端模型 / 入口" value={`${event.clientModel || eventModel(event)} · ${eventEndpointName(event)}`} />
        <Field label={event.kind === 'upstream' ? '上游 HTTP' : '客户端 HTTP'} value={eventHttpStatusLabel(event)} color={eventHttpTone(event)} />
        <Field label="最终结果" value={outcomeLabel(event)} color={`var(--status-${statusKind(event)})`} />
        <Field label="TTFB / 总耗时" value={`${formatDuration(eventTTFBMS(event))} / ${formatDuration(eventDurationMS(event))}`} />
        <Field label="缓存 / 用量" value={<><span className="event-cache-inline">{eventCacheLabel(event)} <CacheHitRate event={event} /></span> · {eventUsageLabel(event)}</>} />
      </>}
    </dl>
    <p className={`event-inspector-message${event.outcome === 'failed' ? ' is-error' : ''}`}>
      {event.failover ? '重试恢复 / 故障转移 · ' : ''}{friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : notify ? event.message : '')}
    </p>
    {eventDetailGroups.filter(([key]) => !notify || ['identity', 'tools', 'advanced'].includes(key)).map(([key, label]) => {
      const open = expanded?.[`${event.id}:${key}`] === true;
      return <details key={key} className="event-inspector-group" open={open} onToggle={(e) => onToggle(`${event.id}:${key}`, e.currentTarget.open)} data-event-detail-tier={key === 'advanced' ? 'advanced' : 'secondary'}>
        <summary><svg className="event-inspector-chevron" width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><path d="m6 3 5 5-5 5" /></svg><span className="event-inspector-group-label"><strong>{label}</strong><small>{summary[key]}</small></span></summary>
        {open && <div className="event-inspector-content">{loading ? <p role="status">正在加载详情…</p> : content(key)}</div>}
      </details>;
    })}
    </section>
  </div>;
}
