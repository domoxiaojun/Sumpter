import { Label as LabelPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

export function Label({ className, ...props }) {
  return (
    <LabelPrimitive.Root
      data-slot="label"
      className={cn(
        'text-xs font-medium leading-none text-foreground/90 select-none peer-disabled:cursor-not-allowed peer-disabled:opacity-70',
        className,
      )}
      {...props}
    />
  );
}
