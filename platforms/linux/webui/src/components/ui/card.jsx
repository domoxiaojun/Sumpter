import { cn } from '../../lib/utils.js';

export function Card({ className, ...props }) {
  return (
    <div
      data-slot="card"
      className={cn(
        'surface-card group/card flex flex-col gap-5 rounded-2xl border border-border/85 bg-card py-5 text-card-foreground',
        className,
      )}
      {...props}
    />
  );
}

export function CardHeader({ className, ...props }) {
  return (
    <div
      data-slot="card-header"
      className={cn(
        'grid auto-rows-min items-start gap-1.5 px-5 has-data-[slot=card-action]:grid-cols-[1fr_auto] [.border-b]:pb-4',
        className,
      )}
      {...props}
    />
  );
}

export function CardTitle({ className, ...props }) {
  return (
    <div
      data-slot="card-title"
      className={cn('text-sm font-semibold leading-snug tracking-tight text-foreground', className)}
      {...props}
    />
  );
}

export function CardDescription({ className, ...props }) {
  return (
    <div
      data-slot="card-description"
      className={cn('text-xs text-muted-foreground leading-relaxed', className)}
      {...props}
    />
  );
}

export function CardAction({ className, ...props }) {
  return (
    <div
      data-slot="card-action"
      className={cn('col-start-2 row-span-2 row-start-1 self-center justify-self-end', className)}
      {...props}
    />
  );
}

export function CardContent({ className, ...props }) {
  return (
    <div
      data-slot="card-content"
      className={cn('px-5', className)}
      {...props}
    />
  );
}

export function CardFooter({ className, ...props }) {
  return (
    <div
      data-slot="card-footer"
      className={cn('flex items-center px-5 [.border-t]:pt-4', className)}
      {...props}
    />
  );
}
