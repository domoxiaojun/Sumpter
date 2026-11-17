import { cn } from '../../lib/utils.js';

export function Table({ className, ...props }) {
  return (
    <div className="relative w-full overflow-x-auto">
      <table
        data-slot="table"
        className={cn('w-full caption-bottom text-sm border-collapse', className)}
        {...props}
      />
    </div>
  );
}

export function TableHeader({ className, ...props }) {
  return (
    <thead
      data-slot="table-header"
      className={cn('[&_tr]:border-b border-border/80 bg-muted/45', className)}
      {...props}
    />
  );
}

export function TableBody({ className, ...props }) {
  return (
    <tbody
      data-slot="table-body"
      className={cn('[&_tr:last-child]:border-0', className)}
      {...props}
    />
  );
}

export function TableRow({ className, ...props }) {
  return (
    <tr
      data-slot="table-row"
      className={cn(
        'border-b border-border/65 transition-colors hover:bg-muted/40',
        className,
      )}
      {...props}
    />
  );
}

export function TableHead({ className, ...props }) {
  return (
    <th
      data-slot="table-head"
      className={cn(
        'h-10 px-4 text-left align-middle text-[11px] font-semibold tracking-wide text-muted-foreground whitespace-nowrap first:pl-5 last:pr-5',
        className,
      )}
      {...props}
    />
  );
}

export function TableCell({ className, ...props }) {
  return (
    <td
      data-slot="table-cell"
      className={cn('px-4 py-2.5 align-middle text-xs first:pl-5 last:pr-5', className)}
      {...props}
    />
  );
}
