import { createContext, useCallback, useContext, useEffect, useState } from 'react';
import { PanelLeft } from 'lucide-react';
import { cn } from '../../lib/utils.js';
import { Button } from './button.jsx';
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from './sheet.jsx';
import { Tooltip, TooltipContent, TooltipTrigger } from './tooltip.jsx';

const SidebarContext = createContext(null);

export function useSidebar() {
  const ctx = useContext(SidebarContext);
  if (!ctx) throw new Error('useSidebar must be used within SidebarProvider');
  return ctx;
}

export function SidebarProvider({ defaultOpen = true, style, className, children, ...props }) {
  const [open, setOpen] = useState(defaultOpen);
  const [openMobile, setOpenMobile] = useState(false);
  const [isMobile, setIsMobile] = useState(() => (typeof window !== 'undefined' ? window.innerWidth < 768 : false));

  useEffect(() => {
    const mq = window.matchMedia('(max-width: 767px)');
    const onChange = () => setIsMobile(mq.matches);
    onChange();
    mq.addEventListener('change', onChange);
    return () => mq.removeEventListener('change', onChange);
  }, []);

  const toggleSidebar = useCallback(() => {
    if (isMobile) setOpenMobile((v) => !v);
    else setOpen((v) => !v);
  }, [isMobile]);

  useEffect(() => {
    const onKeyDown = (e) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'b') {
        e.preventDefault();
        toggleSidebar();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [toggleSidebar]);

  const state = open ? 'expanded' : 'collapsed';

  return (
    <SidebarContext.Provider value={{ state, open, setOpen, isMobile, openMobile, setOpenMobile, toggleSidebar }}>
      <div
        data-slot="sidebar-wrapper"
        style={{
          '--sidebar-width': '16rem',
          '--sidebar-width-icon': '3.5rem',
          ...style,
        }}
        className={cn(
          'group/sidebar-wrapper flex min-h-svh w-full bg-sidebar text-sidebar-foreground',
          className,
        )}
        {...props}
      >
        {children}
      </div>
    </SidebarContext.Provider>
  );
}

export function Sidebar({ side = 'left', variant = 'inset', collapsible = 'icon', className, children, ...props }) {
  const { isMobile, state, openMobile, setOpenMobile } = useSidebar();

  if (isMobile) {
    return (
      <Sheet open={openMobile} onOpenChange={setOpenMobile}>
        <SheetContent side={side} className="w-[17rem] bg-sidebar p-0 text-sidebar-foreground">
          <SheetHeader className="sr-only">
            <SheetTitle>导航菜单</SheetTitle>
            <SheetDescription>切换控制台功能页</SheetDescription>
          </SheetHeader>
          <div className="flex h-full w-full flex-col">{children}</div>
        </SheetContent>
      </Sheet>
    );
  }

  return (
    <div
      className="group peer hidden md:block text-sidebar-foreground"
      data-state={state}
      data-collapsible={state === 'collapsed' ? collapsible : ''}
      data-variant={variant}
      data-side={side}
      data-slot="sidebar"
    >
      <div
        data-slot="sidebar-gap"
        className={cn(
          'relative w-[var(--sidebar-width)] bg-transparent transition-[width] duration-200 ease-linear',
          'group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+1rem)]',
        )}
      />
      <div
        data-slot="sidebar-container"
        className={cn(
          'fixed inset-y-0 left-0 z-10 hidden h-svh w-[var(--sidebar-width)] transition-[left,right,width] duration-200 ease-linear md:flex',
          variant === 'inset' ? 'p-2.5 pr-0' : 'border-r border-sidebar-border',
          'group-data-[collapsible=icon]:w-[calc(var(--sidebar-width-icon)+1rem)]',
          className,
        )}
        {...props}
      >
        <div
          data-sidebar="sidebar"
          className="flex h-full w-full flex-col rounded-xl bg-sidebar"
        >
          {children}
        </div>
      </div>
    </div>
  );
}

export function SidebarInset({ className, ...props }) {
  return (
    <main
      data-slot="sidebar-inset"
      className={cn(
        'relative flex w-full flex-1 flex-col bg-background',
        'md:peer-data-[variant=inset]:m-2.5 md:peer-data-[variant=inset]:rounded-xl md:peer-data-[variant=inset]:border md:peer-data-[variant=inset]:border-border/80 md:peer-data-[variant=inset]:shadow-xs',
        className,
      )}
      {...props}
    />
  );
}

export function SidebarTrigger({ className, onClick, ...props }) {
  const { toggleSidebar } = useSidebar();
  return (
    <Button
      data-slot="sidebar-trigger"
      variant="ghost"
      size="icon-sm"
      className={cn('size-7', className)}
      onClick={(e) => {
        onClick?.(e);
        toggleSidebar();
      }}
      {...props}
    >
      <PanelLeft className="size-4" />
      <span className="sr-only">切换侧边栏</span>
    </Button>
  );
}

export function SidebarHeader({ className, ...props }) {
  return <div data-slot="sidebar-header" className={cn('flex flex-col gap-2 p-2.5', className)} {...props} />;
}

export function SidebarFooter({ className, ...props }) {
  return <div data-slot="sidebar-footer" className={cn('mt-auto flex flex-col gap-2 p-2.5', className)} {...props} />;
}

export function SidebarContent({ className, ...props }) {
  return <div data-slot="sidebar-content" className={cn('flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-2.5', className)} {...props} />;
}

export function SidebarGroup({ className, ...props }) {
  return <div data-slot="sidebar-group" className={cn('flex w-full flex-col gap-1', className)} {...props} />;
}

export function SidebarGroupLabel({ className, ...props }) {
  return (
    <div
      data-slot="sidebar-group-label"
      className={cn(
        'px-2 py-1 text-[11px] font-medium text-muted-foreground/80 transition-opacity duration-200',
        'group-data-[collapsible=icon]:opacity-0 group-data-[collapsible=icon]:h-0 group-data-[collapsible=icon]:overflow-hidden group-data-[collapsible=icon]:py-0',
        className,
      )}
      {...props}
    />
  );
}

export function SidebarMenu({ className, ...props }) {
  return <ul data-slot="sidebar-menu" className={cn('flex w-full flex-col gap-1', className)} {...props} />;
}

export function SidebarMenuItem({ className, ...props }) {
  return <li data-slot="sidebar-menu-item" className={cn('relative', className)} {...props} />;
}

export function SidebarMenuButton({ isActive = false, tooltip, className, children, ...props }) {
  const { state, isMobile } = useSidebar();
  const button = (
    <button
      type="button"
      data-slot="sidebar-menu-button"
      data-active={isActive}
      aria-label={tooltip || undefined}
      className={cn(
        'flex w-full items-center gap-2.5 overflow-hidden rounded-lg px-2.5 py-2 text-left text-[13px] font-medium transition-colors outline-none cursor-pointer',
        'text-sidebar-foreground/80 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-2 focus-visible:ring-sidebar-ring',
        'data-[active=true]:bg-sidebar-accent data-[active=true]:text-sidebar-accent-foreground data-[active=true]:font-semibold',
        'group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-2 [&_svg]:size-4 [&_svg]:shrink-0',
        className,
      )}
      {...props}
    >
      {children}
    </button>
  );

  if (!tooltip || state !== 'collapsed' || isMobile) return button;
  return (
    <Tooltip>
      <TooltipTrigger asChild>{button}</TooltipTrigger>
      <TooltipContent side="right" align="center">{tooltip}</TooltipContent>
    </Tooltip>
  );
}
