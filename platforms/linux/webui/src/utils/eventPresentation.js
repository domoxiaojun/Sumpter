import { eventField, eventStreamTrace, formatTokenCount } from './helpers.js';

export const eventDetailGroups = [
  ['routing', '路由与重试'], ['usage', '用量与缓存'], ['identity', '会话与代理'],
  ['response', '请求与响应'], ['tools', '工具与通知'], ['advanced', '高级诊断'],
];

const cacheLabels = { hit: '已命中', miss: '未命中', pending: '等待上游数据', unknown: '未知', not_applicable: '不适用' };
const cacheReasons = {
  unreported: '上游未报告', not_observed: '未观测到用量', unsupported_transport: '此传输未采集用量',
  unknown_applicability: '适用性未确定', observation_truncated: '观测不完整', invalid_value: '无效数值',
  conflicting_evidence: '上游证据冲突', insufficient_evidence: '证据不足',
};

export function eventCacheRead(event) {
  return event?.cacheRead ?? { state: event?.kind === 'notify' ? 'not_applicable' : 'unknown', readTokens: null, finality: 'unknown', reason: null };
}

export function eventCacheLabel(event) {
  const cache = eventCacheRead(event);
  const count = cache.readTokens;
  return `缓存${cacheLabels[cache.state] ?? '未知'}${count != null && count > 0 ? ` ${formatTokenCount(count)}` : ''}`;
}

export function eventCacheReason(event) {
  const reason = eventCacheRead(event).reason;
  return cacheReasons[reason] || reason || '—';
}

export function eventUsage(event) {
  return event?.usageSummary ?? eventStreamTrace(event)?.usage ?? null;
}

export function eventUsageLabel(event) {
  const usage = eventUsage(event);
  const count = (value) => value == null ? '—' : formatTokenCount(value);
  return `输入 ${count(usage?.inputTokens)} · 输出 ${count(usage?.outputTokens)}`;
}

export function eventCacheTokenRatio(event) {
  const cache = eventCacheRead(event);
  const usage = eventUsage(event);
  if (!['hit', 'miss'].includes(cache.state) || cache.finality !== 'confirmed' || cache.readTokens == null || usage?.inputTokens == null) return null;
  const format = event.targetFormat ?? event.sourceFormat;
  let input = usage.inputTokens;
  if (format === 'anthropic') {
    if (usage.cacheCreationInputTokens == null) return null;
    input += cache.readTokens + usage.cacheCreationInputTokens;
  } else if (!['openai', 'openai-responses', 'gemini'].includes(format)) return null;
  return input > 0 && cache.readTokens >= 0 && cache.readTokens <= input ? cache.readTokens / input : null;
}

export function eventHttpTone(event) {
  const status = Number(eventField(event, 'statusCode', 'status_code'));
  if (status === 101) return 'var(--primary)';
  if (status >= 200 && status < 300) return 'var(--status-good)';
  if (status >= 300 && status < 400) return 'var(--status-warning)';
  if (status >= 400) return 'var(--status-critical)';
  return 'var(--text-secondary)';
}

export function eventAgentLabel(event) {
  const role = event?.agentRole;
  const labels = { root: '主代理', subagent: '子代理', guardian: 'Guardian', review: '审查', memory: '记忆任务', title: '标题任务', automation: '自动任务', system: '系统', ambient: '后台任务', unknown: '身份未确定' };
  return [event?.agentName, role && role !== 'unknown' ? (labels[role] || role) : null].filter(Boolean).join(' · ');
}
