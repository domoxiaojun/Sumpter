import { cva } from 'class-variance-authority';
import { cn } from '../../lib/utils.js';

const badgeVariants = cva(
  'inline-flex items-center justify-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[11px] font-medium leading-4 whitespace-nowrap transition-colors',
  {
    variants: {
      variant: {
        default: 'border-primary/25 bg-primary/10 text-primary',
        secondary: 'border-border/75 bg-muted/80 text-muted-foreground',
        outline: 'border-border/85 bg-background/60 text-foreground/90',
        success: 'border-status-good/25 bg-status-good/10 text-status-good',
        warning: 'border-status-warning/28 bg-status-warning/10 text-status-warning',
        destructive: 'border-status-critical/28 bg-status-critical/10 text-status-critical',
        info: 'border-accent-cyan/28 bg-accent-cyan/10 text-accent-cyan',
      },
    },
    defaultVariants: {
      variant: 'secondary',
    },
  },
);

export function Badge({ className, variant, ...props }) {
  return (
    <span
      data-slot="badge"
      className={cn(badgeVariants({ variant }), className)}
      {...props}
    />
  );
}
