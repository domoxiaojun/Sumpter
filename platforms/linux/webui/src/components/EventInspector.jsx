import React from 'react';
import {
  eventField, eventModel, eventEndpoint, eventEndpointName, eventDurationMS, eventProjectContext, eventClientKindLabel, eventKindLabel,
  eventHttpStatusLabel, eventOutcomeLabel, eventDurationText, friendlyEventMessage, eventFailureSummaryLabel,
  eventRequestID, eventSessionID, eventToolCalls, eventStreamTrace, formatTimestamp, formatDuration,
  eventTTFBMS, eventUpstreamStatusLabel, eventFailureDetail, eventPurposeLabel, eventProtocolRouteLabel,
} from '../utils/helpers.js';
import { eventDetailGroups, eventAgentLabel, eventCacheLabel, eventCacheRead, eventCacheTokenRatio, eventCacheReason, eventUsage, eventUsageLabel, eventHttpTone } from '../utils/eventPresentation.js';

function Field({ label, value, color }) {
  return <div className="event-inspector-field"><dt>{label}</dt><dd style={color ? { color } : undefined}>{value ?? '—'}</dd></div>;
}

export function EventInspector({ event, chain = [], onSelect, expanded, onToggle, onCopy, loading = false }) {
  const notify = event.kind === 'notify';
  const cache = eventCacheRead(event);
  const usage = eventUsage(event);
  const cacheRatio = eventCacheTokenRatio(event);
  const metadata = event.codexMetadata || {};
  const fields = (pairs) => <dl className="event-inspector-fields">{pairs.map(([label, value]) => <Field key={label} label={label} value={value} />)}</dl>;
  const attempts = chain.filter((item) => item.kind === 'upstream');
  const summary = {
    routing: `${eventEndpointName(event)}${attempts.length ? ` · ${attempts.length} 次上游尝试` : ''}`,
    usage: eventCacheLabel(event), identity: eventAgentLabel(event) || eventProjectContext(event).label,
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
      <div className="request-chain-list" role="region" aria-label="请求链事件列表">
        {chain.map((item) => <button key={item.id} type="button" className="event-chain-button" aria-pressed={item.id === event.id} onClick={() => onSelect(item.id)}>
          <strong>{eventKindLabel(item.kind)} · {eventEndpoint(item)}</strong>
          <span>{eventHttpStatusLabel(item)} · {eventOutcomeLabel(item)} · {eventCacheLabel(item)}</span>
        </button>)}
      </div>
    </>;
    if (key === 'usage') return fields([
      ['缓存读状态', eventCacheLabel(event)], ['缓存证据', eventCacheReason(event)],
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
  return <section className="event-inspector" aria-label="选中事件详情">
    <dl className="event-inspector-fields" data-event-detail-tier="primary">
      <Field label="时间 / 类型" value={`${formatTimestamp(event.timestamp, { date: true })} · ${eventKindLabel(event.kind)}`} />
      <Field label="客户端 / 项目" value={`${eventClientKindLabel(event)} · ${eventProjectContext(event).label}`} />
      {eventAgentLabel(event) && <Field label="代理" value={eventAgentLabel(event)} />}
      {notify && <Field label="Hook 类型" value={event.hookEvent} />}
      {!notify && <>
        <Field label="客户端模型 / 入口" value={`${event.clientModel || eventModel(event)} · ${eventEndpointName(event)}`} />
        <Field label={event.kind === 'upstream' ? '上游 HTTP' : '客户端 HTTP'} value={eventHttpStatusLabel(event)} color={eventHttpTone(event)} />
        <Field label="最终结果" value={eventOutcomeLabel(event)} color={event.outcome === 'failed' ? 'var(--status-critical)' : undefined} />
        <Field label="TTFB / 总耗时" value={`${formatDuration(eventTTFBMS(event))} / ${formatDuration(eventDurationMS(event))}`} />
        <Field label="缓存 / 用量" value={`${eventCacheLabel(event)} · ${eventUsageLabel(event)}`} />
      </>}
    </dl>
    <p className={`event-inspector-message${event.outcome === 'failed' ? ' is-error' : ''}`}>
      {event.failover ? '重试恢复 / 故障转移 · ' : ''}{friendlyEventMessage(event) || (event.outcome === 'failed' ? eventFailureSummaryLabel(event) : notify ? event.message : '')}
    </p>
    {eventDetailGroups.filter(([key]) => !notify || ['identity', 'tools', 'advanced'].includes(key)).map(([key, label]) => {
      const open = expanded?.[`${event.id}:${key}`] === true;
      return <details key={key} className="event-inspector-group" open={open} onToggle={(e) => onToggle(`${event.id}:${key}`, e.currentTarget.open)} data-event-detail-tier={key === 'advanced' ? 'advanced' : 'secondary'}>
        <summary><strong>{label}</strong><span>{summary[key]}</span></summary>
        {open && <div className="event-inspector-content">{loading ? <p role="status">正在加载详情…</p> : content(key)}</div>}
      </details>;
    })}
  </section>;
}
