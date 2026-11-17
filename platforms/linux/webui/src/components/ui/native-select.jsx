import { ChevronDown } from 'lucide-react';
import { cn } from '../../lib/utils.js';

export function NativeSelect({
  className,
  wrapperClassName,
  size = 'default',
  disabled,
  children,
  ...props
}) {
  return (
    <span
      data-slot="native-select-wrapper"
      className={cn(
        'relative inline-flex w-full items-center',
        disabled && 'opacity-55',
        wrapperClassName,
      )}
    >
      <select
        data-slot="native-select"
        data-size={size}
        disabled={disabled}
        className={cn(
          'w-full appearance-none rounded-lg border border-input bg-background/85 pr-8 pl-3 text-xs text-foreground shadow-2xs transition-[color,background-color,border-color,box-shadow] outline-none cursor-pointer',
          size === 'sm' ? 'h-8 py-1 pl-2.5 pr-7 text-xs' : 'h-9 py-1.5',
          'hover:border-foreground/20 hover:bg-background',
          'focus-visible:border-ring focus-visible:bg-background focus-visible:ring-[3px] focus-visible:ring-ring/25',
          'disabled:pointer-events-none disabled:cursor-not-allowed',
          className,
        )}
        {...props}
      >
        {children}
      </select>
      <ChevronDown
        aria-hidden="true"
        className={cn(
          'pointer-events-none absolute text-muted-foreground transition-colors',
          size === 'sm' ? 'right-2 size-3.5' : 'right-2.5 size-3.5',
        )}
      />
    </span>
  );
}
