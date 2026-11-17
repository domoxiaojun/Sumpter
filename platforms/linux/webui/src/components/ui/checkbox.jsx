import { Check } from 'lucide-react';
import { cn } from '../../lib/utils.js';

export function Checkbox({ className, checked, disabled, onChange, onCheckedChange, ...props }) {
  return (
    <span
      data-slot="checkbox-wrapper"
      className={cn(
        'relative inline-flex size-4 shrink-0 items-center justify-center align-middle',
        disabled ? 'cursor-not-allowed opacity-50' : 'cursor-pointer',
        className,
      )}
    >
      <input
        type="checkbox"
        data-slot="checkbox"
        checked={checked}
        disabled={disabled}
        onChange={(event) => {
          onChange?.(event);
          onCheckedChange?.(event.target.checked);
        }}
        className={cn(
          'peer absolute inset-0 z-10 m-0 size-full opacity-0',
          disabled ? 'cursor-not-allowed' : 'cursor-pointer',
        )}
        {...props}
      />
      <span
        aria-hidden="true"
        className={cn(
          'flex size-4 items-center justify-center rounded-[5px] border border-input bg-background/90 text-primary-foreground shadow-2xs transition-all duration-150',
          'peer-hover:border-primary/60',
          'peer-checked:border-primary peer-checked:bg-primary peer-checked:shadow-xs',
          'peer-focus-visible:border-ring peer-focus-visible:ring-[3px] peer-focus-visible:ring-ring/30',
        )}
      >
        <Check
          className={cn(
            'size-3 stroke-[2.75] transition-transform duration-150',
            checked ? 'scale-100 opacity-100' : 'scale-50 opacity-0',
          )}
        />
      </span>
    </span>
  );
}
