import { cva } from 'class-variance-authority';
import { cn } from '../../lib/utils.js';

const badgeVariants = cva(
  'inline-flex items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-[11px] font-medium whitespace-nowrap transition-colors',
  {
    variants: {
      variant: {
        default: 'border-primary/30 bg-accent text-accent-foreground',
        secondary: 'border-border bg-secondary text-muted-foreground',
        outline: 'border-border text-foreground bg-transparent',
        success: 'border-status-good/25 bg-status-good/10 text-status-good',
        warning: 'border-status-warning/25 bg-status-warning/10 text-status-warning',
        destructive: 'border-status-critical/25 bg-status-critical/10 text-status-critical',
        info: 'border-accent-cyan/25 bg-accent-cyan/10 text-accent-cyan',
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
