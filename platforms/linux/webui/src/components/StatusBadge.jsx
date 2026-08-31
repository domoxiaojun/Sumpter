import React from 'react';

export function StatusBadge({ status, text, kind }) {
  let resolvedKind = kind;
  let resolvedText = text;

  if (status) {
    const running = Boolean(status.running);
    const health = status.health?.state;
    if (!running) {
      resolvedKind = 'muted';
      resolvedText = resolvedText || '已停止';
    } else if (health === 'healthy') {
      resolvedKind = 'good';
      resolvedText = resolvedText || '运行正常';
    } else if (health === 'degraded') {
      resolvedKind = 'warning';
      resolvedText = resolvedText || '运行降级';
    } else if (health === 'down') {
      resolvedKind = 'critical';
      resolvedText = resolvedText || '上游故障';
    } else {
      resolvedKind = 'live';
      resolvedText = resolvedText || '运行中';
    }
  }

  return (
    <span className={`status-pill ${resolvedKind || 'muted'}`}>
      <span className="pulse-dot" />
      <span>{resolvedText}</span>
    </span>
  );
}
