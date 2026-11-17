import { cva } from 'class-variance-authority';
import { Toggle as TogglePrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

export const toggleVariants = cva(
  "inline-flex items-center justify-center gap-1.5 rounded-lg text-xs font-medium whitespace-nowrap transition-all duration-150 outline-none cursor-pointer text-muted-foreground hover:text-foreground disabled:pointer-events-none disabled:opacity-50 data-[state=on]:bg-card data-[state=on]:text-foreground data-[state=on]:font-semibold data-[state=on]:shadow-xs focus-visible:ring-2 focus-visible:ring-ring/40 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-3.5",
  {
    variants: {
      variant: {
        default: 'bg-transparent hover:bg-accent/60',
        outline: 'bg-transparent hover:bg-background/60',
      },
      size: {
        default: 'h-7 px-3 min-w-8',
        sm: 'h-6 px-2.5 min-w-7 text-xs',
        lg: 'h-8 px-3.5 min-w-9',
      },
    },
    defaultVariants: {
      variant: 'default',
      size: 'default',
    },
  },
);

export function Toggle({ className, variant, size, ...props }) {
  return (
    <TogglePrimitive.Root
      data-slot="toggle"
      className={cn(toggleVariants({ variant, size, className }))}
      {...props}
    />
  );
}
