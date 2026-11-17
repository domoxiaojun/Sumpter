import { cn } from '../../lib/utils.js';

export function PageHeader({
  icon: IconComp,
  iconClassName,
  title,
  badge,
  description,
  actions,
  className,
}) {
  return (
    <header
      data-slot="page-header"
      className={cn(
        'flex flex-col gap-3.5 sm:flex-row sm:items-center sm:justify-between',
        className,
      )}
    >
      <div className="flex min-w-0 items-start gap-3.5 sm:items-center">
        {IconComp && (
          <div
            aria-hidden="true"
            className={cn(
              'mt-0.5 flex size-10 shrink-0 items-center justify-center rounded-xl border border-primary/20 bg-gradient-to-br from-primary/15 via-primary/8 to-transparent text-primary shadow-2xs sm:mt-0',
              iconClassName,
            )}
          >
            <IconComp className="size-[18px]" />
          </div>
        )}
        <div className="min-w-0 space-y-1">
          <h1 className="flex flex-wrap items-center gap-2.5 text-xl font-semibold tracking-tight text-foreground sm:text-[22px] sm:leading-7">
            {typeof title === 'string' ? <span>{title}</span> : title}
            {badge}
          </h1>
          {description && (
            <p className="text-xs leading-relaxed text-muted-foreground">
              {description}
            </p>
          )}
        </div>
      </div>
      {actions && (
        <div className="flex shrink-0 flex-wrap items-center gap-2">
          {actions}
        </div>
      )}
    </header>
  );
}
