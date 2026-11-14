import { cn } from '../../lib/utils.js';

export function Kbd({ className, ...props }) {
  return (
    <kbd
      data-slot="kbd"
      className={cn(
        'pointer-events-none inline-flex h-5 w-fit min-w-5 items-center justify-center gap-1 rounded-xs border border-border bg-muted px-1.5 font-mono text-[10px] font-medium leading-none text-muted-foreground select-none',
        className,
      )}
      {...props}
    />
  );
}

export function KbdGroup({ className, ...props }) {
  return (
    <span
      data-slot="kbd-group"
      className={cn('inline-flex items-center gap-1', className)}
      {...props}
    />
  );
}
