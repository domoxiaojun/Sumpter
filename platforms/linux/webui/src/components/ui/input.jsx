import { cn } from '../../lib/utils.js';

export function Input({ className, type = 'text', ...props }) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        'flex h-9 w-full rounded-lg border border-input bg-background/85 px-3 py-1.5 text-sm text-foreground shadow-2xs transition-[color,background-color,border-color,box-shadow] outline-none',
        'placeholder:text-muted-foreground/65 hover:border-foreground/20 focus-visible:border-ring focus-visible:bg-background focus-visible:ring-[3px] focus-visible:ring-ring/25',
        'aria-invalid:border-status-critical aria-invalid:ring-status-critical/20',
        'disabled:cursor-not-allowed disabled:opacity-50',
        className,
      )}
      {...props}
    />
  );
}
