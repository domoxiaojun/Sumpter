import { Icon } from '../utils/icons.jsx';
import { Card } from './ui/card.jsx';

export function MetricCard({ label, value, detail, icon, accent = 'var(--primary)' }) {
  return (
    <Card className="relative overflow-hidden py-4 transition-colors hover:border-border/80">
      <span
        className="pointer-events-none absolute inset-x-0 top-0 h-[2px] opacity-80"
        style={{ background: accent }}
        aria-hidden="true"
      />
      <div className="flex items-start justify-between gap-2 px-4">
        <div className="min-w-0 space-y-1">
          <div className="truncate text-xs font-medium text-muted-foreground">{label}</div>
          <div className="font-mono text-2xl font-semibold tracking-tight text-foreground tabular-nums">
            {value}
          </div>
          {detail && (
            <div className="truncate text-[11px] text-muted-foreground tabular-nums">
              {detail}
            </div>
          )}
        </div>
        {icon && (
          <div
            className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-muted/40"
            style={{ color: accent }}
            aria-hidden="true"
          >
            <Icon name={icon} size={16} aria-hidden="true" focusable="false" />
          </div>
        )}
      </div>
    </Card>
  );
}
