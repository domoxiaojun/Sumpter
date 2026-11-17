import { Tabs as TabsPrimitive } from 'radix-ui';
import { cn } from '../../lib/utils.js';

export function Tabs({ className, ...props }) {
  return (
    <TabsPrimitive.Root
      data-slot="tabs"
      className={cn('flex flex-col gap-3.5', className)}
      {...props}
    />
  );
}

export function TabsList({ className, ...props }) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        'inline-flex h-9 w-fit items-center justify-center gap-1 rounded-xl border border-border/75 bg-muted/65 p-1 text-muted-foreground',
        className,
      )}
      {...props}
    />
  );
}

export function TabsTrigger({ className, ...props }) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        'inline-flex h-full items-center justify-center gap-1.5 whitespace-nowrap rounded-lg px-3 py-1 text-xs font-medium transition-all duration-150 cursor-pointer outline-none',
        'data-[state=active]:bg-card data-[state=active]:text-foreground data-[state=active]:font-semibold data-[state=active]:shadow-xs',
        'hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/40',
        className,
      )}
      {...props}
    />
  );
}

export function TabsContent({ className, ...props }) {
  return (
    <TabsPrimitive.Content
      data-slot="tabs-content"
      className={cn('outline-none', className)}
      {...props}
    />
  );
}
