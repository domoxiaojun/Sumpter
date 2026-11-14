import { cn } from '../lib/utils.js';
import { Badge } from './ui/badge.jsx';

const TONE_MAP = {
  good: {
    pill: 'border-status-good/30 bg-status-good/10 text-status-good',
    dot: 'bg-status-good animate-pulse',
  },
  warning: {
    pill: 'border-status-warning/35 bg-status-warning/10 text-status-warning',
    dot: 'bg-status-warning animate-pulse',
  },
  critical: {
    pill: 'border-status-critical/35 bg-status-critical/10 text-status-critical',
    dot: 'bg-status-critical',
  },
  live: {
    pill: 'border-primary/35 bg-primary/10 text-primary',
    dot: 'bg-primary animate-pulse',
  },
  muted: {
    pill: 'border-border bg-muted/60 text-muted-foreground',
    dot: 'bg-muted-foreground/70',
  },
};

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

  const tone = TONE_MAP[resolvedKind || 'muted'] || TONE_MAP.muted;

  return (
    <Badge
      variant="outline"
      className={cn('inline-flex items-center gap-1.5 rounded-full px-2.5 py-0.5 text-xs font-medium', tone.pill)}
    >
      <span className={cn('size-1.5 shrink-0 rounded-full', tone.dot)} aria-hidden="true" />
      <span>{resolvedText}</span>
    </Badge>
  );
}
