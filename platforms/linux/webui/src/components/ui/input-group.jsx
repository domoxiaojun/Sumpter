import { cva } from 'class-variance-authority';
import { cn } from '../../lib/utils.js';
import { Input } from './input.jsx';

export function InputGroup({ className, ...props }) {
  return (
    <div
      data-slot="input-group"
      role="group"
      className={cn(
        'group/input-group relative flex h-9 w-full items-center rounded-lg border border-input bg-background/85 shadow-2xs transition-[color,background-color,border-color,box-shadow] outline-none hover:border-foreground/20',
        'has-[[data-slot=input-group-input]:focus-visible]:border-ring has-[[data-slot=input-group-input]:focus-visible]:bg-background has-[[data-slot=input-group-input]:focus-visible]:ring-[3px] has-[[data-slot=input-group-input]:focus-visible]:ring-ring/25',
        className,
      )}
      {...props}
    />
  );
}

const addonVariants = cva(
  'flex h-full items-center justify-center gap-1.5 text-muted-foreground select-none [&_svg]:pointer-events-none [&_svg:not([class*="size-"])]:size-3.5',
  {
    variants: {
      align: {
        'inline-start': 'order-first pl-3 pr-1.5',
        'inline-end': 'order-last pr-2.5 pl-1.5',
      },
    },
    defaultVariants: {
      align: 'inline-start',
    },
  },
);

export function InputGroupAddon({ className, align = 'inline-start', ...props }) {
  return (
    <div
      data-slot="input-group-addon"
      data-align={align}
      className={cn(addonVariants({ align }), className)}
      {...props}
    />
  );
}

export function InputGroupInput({ className, ...props }) {
  return (
    <Input
      data-slot="input-group-input"
      className={cn(
        'h-full flex-1 rounded-none border-0 bg-transparent px-1.5 shadow-none hover:border-0 focus-visible:ring-0 focus-visible:border-0 focus-visible:bg-transparent',
        className,
      )}
      {...props}
    />
  );
}
