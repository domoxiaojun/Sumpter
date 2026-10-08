import { formatDuration, formatNumber } from './helpers.js';

export const initialConnectionsObservation = { value: null, loaded: false, error: null };

export function receiveConnectionsSummary(summary) {
  return { value: summary?.responsesWebSocketConnections ?? null, loaded: true, error: null };
}

export function failConnectionsSummary(previous, error) {
  return { ...previous, error: error || '读取连接汇总失败' };
}

// Keep the last displayed value (including unavailable/error state) while paused.
export function visibleConnectionsObservation(previous, current, autoRefresh) {
  return autoRefresh || !previous ? current : previous;
}

export function connectionsPresentation(observation) {
  const value = observation.value;
  return {
    message: value ? null : observation.error ? '连接数据暂不可用'
      : observation.loaded ? '当前版本未提供' : '正在读取连接数据…',
    error: observation.error ? `更新失败${value ? '，保留上次数据' : ''}` : null,
    rows: value ? [
      ['当前连接', formatNumber(value.total)],
      ['等待首条业务消息', formatNumber(value.awaitingFirstMessage)],
      ['其中 Guardian', formatNumber(value.guardianAwaitingFirstMessage)],
      ['连接上游中', formatNumber(value.connectingUpstream)],
      ['已进入转发', formatNumber(value.relaying)],
      ['最长首消息等待', value.oldestFirstMessageWaitMS == null ? '—' : formatDuration(value.oldestFirstMessageWaitMS)],
    ] : [],
  };
}
