import { Icon } from '../utils/icons.jsx';
import { Card } from './ui/card.jsx';

export function MetricCard({ label, value, detail, icon, accent = 'var(--primary)' }) {
  return (
    <Card
      className="surface-card-interactive group/metric relative overflow-hidden py-4"
      style={{ '--metric-accent': accent }}
    >
      <span
        className="pointer-events-none absolute -top-10 -right-10 size-28 rounded-full opacity-20 blur-2xl transition-opacity duration-300 group-hover/metric:opacity-35"
        style={{ background: accent }}
        aria-hidden="true"
      />
      <div className="relative z-10 flex items-start justify-between gap-3 px-5">
        <div className="min-w-0 space-y-1.5">
          <div className="truncate text-xs font-medium text-muted-foreground">{label}</div>
          <div className="font-mono text-[22px] leading-7 font-semibold tracking-tight text-foreground tabular-nums">
            {value}
          </div>
          {detail && (
            <div className="truncate text-[11px] leading-4 text-muted-foreground tabular-nums" title={detail}>
              {detail}
            </div>
          )}
        </div>
        {icon && (
          <div
            className="flex size-9 shrink-0 items-center justify-center rounded-xl border border-border/75 bg-background/80 shadow-2xs transition-transform duration-200 group-hover/metric:scale-105"
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
