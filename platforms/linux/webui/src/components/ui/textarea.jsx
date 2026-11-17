import { cn } from '../../lib/utils.js';

export function Textarea({ className, ...props }) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        'flex min-h-[76px] w-full rounded-lg border border-input bg-background/85 px-3 py-2 text-xs leading-relaxed text-foreground shadow-2xs transition-[color,background-color,border-color,box-shadow] outline-none',
        'placeholder:text-muted-foreground/70 hover:border-foreground/20 focus-visible:border-ring focus-visible:bg-background focus-visible:ring-[3px] focus-visible:ring-ring/25',
        'disabled:cursor-not-allowed disabled:opacity-50',
        className,
      )}
      {...props}
    />
  );
}
