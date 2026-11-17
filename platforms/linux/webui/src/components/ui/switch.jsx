import { Switch as SwitchPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

export function Switch({ className, ...props }) {
  return (
    <SwitchPrimitive.Root
      data-slot="switch"
      className={cn(
        'peer inline-flex h-5 w-9 shrink-0 cursor-pointer items-center rounded-full border border-transparent shadow-inner transition-colors duration-200 outline-none',
        'data-[state=checked]:bg-primary data-[state=unchecked]:bg-input/90 hover:data-[state=unchecked]:bg-muted-foreground/35',
        'focus-visible:ring-[3px] focus-visible:ring-ring/30',
        'disabled:cursor-not-allowed disabled:opacity-50',
        className,
      )}
      {...props}
    >
      <SwitchPrimitive.Thumb
        data-slot="switch-thumb"
        className={cn(
          'pointer-events-none block size-4 rounded-full bg-white shadow-sm ring-0 transition-transform duration-200 ease-[cubic-bezier(0.16,1,0.3,1)]',
          'data-[state=checked]:translate-x-4 data-[state=unchecked]:translate-x-0.5',
        )}
      />
    </SwitchPrimitive.Root>
  );
}
