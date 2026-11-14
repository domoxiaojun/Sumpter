import {
  Activity,
  BarChart3,
  CircleHelp,
  GitBranch,
  Info,
  Layers,
  Server,
  ShieldCheck,
  Wrench,
} from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import donkeyHead from '../assets/donkey-logo.png';
import { cn } from '../lib/utils.js';
import {
  Sidebar as ShadcnSidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  useSidebar,
} from './ui/sidebar.jsx';

const ROUTE_ICONS = {
  run: Activity,
  providers: Server,
  'model-groups': Layers,
  routing: GitBranch,
  security: ShieldCheck,
  statistics: BarChart3,
  diagnostics: Wrench,
  help: CircleHelp,
  about: Info,
};

export function Sidebar() {
  const { currentRoute, navigate, status } = useAppControls();
  const { isMobile, setOpenMobile } = useSidebar();

  const navGroups = [
    {
      title: '控制面板',
      items: [
        { id: 'run', title: '运行', icon: 'play' },
        { id: 'providers', title: '入口库', icon: 'server' },
        { id: 'model-groups', title: '模型组', icon: 'route' },
        { id: 'routing', title: 'Claude Code 路由', icon: 'route' },
      ],
    },
    {
      title: '可观测性与安全',
      items: [
        { id: 'security', title: '安全', icon: 'shield' },
        { id: 'statistics', title: '统计', icon: 'chart' },
        { id: 'diagnostics', title: '诊断', icon: 'wrench' },
      ],
    },
  ];

  const resourceItems = [
    { id: 'help', title: '帮助', icon: 'help' },
    { id: 'about', title: '关于', icon: 'info' },
  ];

  const isRunning = Boolean(status?.running);
  const listenerText = status?.listener
    ? `${status.listener.host}:${status.listener.port}`
    : '127.0.0.1:57878';

  const handleSelect = (id) => {
    if (navigate(id) && isMobile) {
      setOpenMobile(false);
    }
  };

  return (
    <ShadcnSidebar id="primary-navigation" variant="inset" collapsible="icon" aria-label="应用主导航">
      <SidebarHeader className="pb-2">
        <SidebarMenu>
          <SidebarMenuItem>
            <button
              type="button"
              onClick={() => handleSelect('run')}
              aria-label="Sumpter · 运行"
              title="回到运行页"
              className="flex w-full cursor-pointer items-center gap-2.5 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-sidebar-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring"
            >
              <div className="flex size-8 shrink-0 items-center justify-center overflow-hidden rounded-lg border border-sidebar-border bg-background/80 p-1">
                <img src={donkeyHead} alt="" className="size-full object-contain" />
              </div>
              <div className="flex min-w-0 flex-col group-data-[collapsible=icon]:hidden">
                <span className="truncate text-sm font-semibold tracking-tight text-sidebar-foreground">
                  Sumpter
                </span>
                <span className="truncate font-mono text-[10px] tracking-wider text-muted-foreground">
                  DONKEY TECH
                </span>
              </div>
            </button>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        {navGroups.map((group) => (
          <SidebarGroup key={group.title}>
            <SidebarGroupLabel>{group.title}</SidebarGroupLabel>
            <SidebarMenu>
              {group.items.map((item) => {
                const NavIcon = ROUTE_ICONS[item.id] || Activity;
                const active = currentRoute === item.id;
                return (
                  <SidebarMenuItem key={item.id}>
                    <SidebarMenuButton
                      isActive={active}
                      aria-current={active ? 'page' : undefined}
                      onClick={() => handleSelect(item.id)}
                      tooltip={item.title}
                    >
                      <NavIcon />
                      <span className="truncate group-data-[collapsible=icon]:hidden">{item.title}</span>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                );
              })}
            </SidebarMenu>
          </SidebarGroup>
        ))}
      </SidebarContent>

      <SidebarFooter className="pt-2">
        <SidebarMenu aria-label="帮助与关于">
          {resourceItems.map((item) => {
            const NavIcon = ROUTE_ICONS[item.id] || CircleHelp;
            const active = currentRoute === item.id;
            return (
              <SidebarMenuItem key={item.id}>
                <SidebarMenuButton
                  isActive={active}
                  aria-current={active ? 'page' : undefined}
                  onClick={() => handleSelect(item.id)}
                  tooltip={item.title}
                >
                  <NavIcon />
                  <span className="truncate group-data-[collapsible=icon]:hidden">{item.title}</span>
                </SidebarMenuButton>
              </SidebarMenuItem>
            );
          })}
        </SidebarMenu>

        <div className="mt-1 rounded-xl border border-sidebar-border bg-background/60 p-2.5 group-data-[collapsible=icon]:hidden">
          <div className="flex items-center gap-2">
            <span
              className={cn(
                'size-2 shrink-0 rounded-full',
                isRunning ? 'bg-status-good animate-pulse' : 'bg-muted-foreground',
              )}
              aria-hidden="true"
            />
            <div className="min-w-0">
              <div className="truncate text-xs font-semibold text-sidebar-foreground">
                {isRunning ? '代理运行中' : '代理已就绪'}
              </div>
              <div className="truncate font-mono text-[11px] text-muted-foreground tabular-nums">
                {listenerText}
              </div>
            </div>
          </div>
        </div>
      </SidebarFooter>
    </ShadcnSidebar>
  );
}
