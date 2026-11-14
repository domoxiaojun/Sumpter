import { Separator as SeparatorPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

export function Separator({ className, orientation = 'horizontal', decorative = true, ...props }) {
  return (
    <SeparatorPrimitive.Root
      data-slot="separator"
      decorative={decorative}
      orientation={orientation}
      className={cn(
        'shrink-0 bg-border data-horizontal:h-px data-horizontal:w-full data-vertical:h-full data-vertical:w-px',
        className,
      )}
      {...props}
    />
  );
}
