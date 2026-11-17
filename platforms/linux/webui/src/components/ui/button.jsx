import { cva } from 'class-variance-authority';
import { Slot as SlotPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

const buttonVariants = cva(
  "inline-flex shrink-0 items-center justify-center gap-1.5 rounded-lg text-sm font-medium whitespace-nowrap transition-all duration-150 outline-none select-none cursor-pointer focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/30 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          'bg-primary text-primary-foreground shadow-[0_1px_2px_rgba(0,0,0,0.08),inset_0_1px_0_rgba(255,255,255,0.2)] hover:brightness-[1.06] hover:shadow-sm active:scale-[0.98]',
        destructive:
          'bg-destructive text-destructive-foreground shadow-[0_1px_2px_rgba(0,0,0,0.08),inset_0_1px_0_rgba(255,255,255,0.18)] hover:brightness-[1.06] active:scale-[0.98] focus-visible:ring-destructive/30',
        outline:
          'border border-border/90 bg-card/85 text-foreground shadow-2xs hover:bg-accent/75 hover:border-foreground/18 active:scale-[0.98]',
        secondary:
          'border border-border/70 bg-secondary/85 text-secondary-foreground shadow-2xs hover:bg-secondary hover:border-border active:scale-[0.98]',
        ghost:
          'text-muted-foreground hover:bg-accent/80 hover:text-foreground active:scale-[0.98]',
        link:
          'text-primary underline-offset-4 hover:underline',
      },
      size: {
        default: 'h-9 px-4 py-2 has-[>svg]:px-3',
        sm: 'h-8 gap-1.5 rounded-lg px-3 text-xs has-[>svg]:px-2.5',
        xs: 'h-7 gap-1 rounded-md px-2.5 text-xs',
        lg: 'h-10 rounded-xl px-5 has-[>svg]:px-4',
        icon: 'size-8 rounded-lg',
        'icon-sm': 'size-7 rounded-md',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
);

export function Button({ className, variant = 'default', size = 'default', asChild = false, ...props }) {
  const Comp = asChild ? SlotPrimitive.Root : 'button';
  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  );
}

export { buttonVariants };
