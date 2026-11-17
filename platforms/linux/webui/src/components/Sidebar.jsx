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
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <button
              type="button"
              onClick={() => handleSelect('run')}
              aria-label="Sumpter · 运行"
              title="回到运行页"
              className="group/brand flex w-full cursor-pointer items-center gap-3 rounded-xl px-2 py-2 text-left transition-all duration-150 hover:bg-sidebar-accent/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-sidebar-ring/50 group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-1"
            >
              <div className="flex size-9 shrink-0 items-center justify-center overflow-hidden rounded-xl border border-sidebar-border/90 bg-card p-1 shadow-2xs transition-transform duration-200 group-hover/brand:scale-[1.03]">
                <img src={donkeyHead} alt="" className="size-full object-contain" />
              </div>
              <div className="flex min-w-0 flex-col gap-0.5 group-data-[collapsible=icon]:hidden">
                <span className="truncate text-[14px] font-semibold leading-tight tracking-tight text-sidebar-foreground">
                  Sumpter
                </span>
                <span className="truncate font-mono text-[10px] font-medium tracking-[0.14em] text-muted-foreground">
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

      <SidebarFooter>
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

        <div className="mt-1 rounded-xl border border-sidebar-border/85 bg-card/75 px-3 py-2.5 shadow-2xs backdrop-blur-xs group-data-[collapsible=icon]:hidden">
          <div className="flex items-center gap-2.5">
            <span
              className={cn(
                'size-2 shrink-0 rounded-full',
                isRunning
                  ? 'status-sonar-ring bg-status-good text-status-good'
                  : 'bg-muted-foreground/70',
              )}
              aria-hidden="true"
            />
            <div className="min-w-0 flex-1 space-y-0.5">
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-xs font-semibold text-sidebar-foreground">
                  {isRunning ? '代理运行中' : '代理已就绪'}
                </span>
                {status?.version && (
                  <span className="font-mono text-[10px] text-muted-foreground tabular-nums">
                    v{status.version}
                  </span>
                )}
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
