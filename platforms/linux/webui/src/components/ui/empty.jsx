import { cn } from '../../lib/utils.js';

export function Empty({ className, ...props }) {
  return (
    <div
      data-slot="empty"
      className={cn(
        'flex flex-col items-center justify-center gap-3 rounded-xl border border-dashed border-border bg-muted/20 px-6 py-12 text-center',
        className,
      )}
      {...props}
    />
  );
}

export function EmptyMedia({ className, ...props }) {
  return (
    <div
      data-slot="empty-media"
      className={cn(
        'flex size-10 items-center justify-center rounded-lg border border-border bg-card text-muted-foreground shadow-xs [&_svg]:size-5',
        className,
      )}
      {...props}
    />
  );
}

export function EmptyTitle({ className, ...props }) {
  return (
    <div
      data-slot="empty-title"
      className={cn('text-sm font-medium text-foreground', className)}
      {...props}
    />
  );
}

export function EmptyDescription({ className, ...props }) {
  return (
    <p
      data-slot="empty-description"
      className={cn('max-w-sm text-xs text-muted-foreground leading-relaxed', className)}
      {...props}
    />
  );
}
