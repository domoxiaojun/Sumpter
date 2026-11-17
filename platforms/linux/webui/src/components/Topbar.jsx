import { ChevronDown, LogOut, Moon, Search, ShieldCheck, Sun } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';
import { Button } from './ui/button.jsx';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from './ui/dropdown-menu.jsx';
import { Kbd, KbdGroup } from './ui/kbd.jsx';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select.jsx';
import { Separator } from './ui/separator.jsx';
import { SidebarTrigger } from './ui/sidebar.jsx';
import { Switch } from './ui/switch.jsx';

const ROUTE_LABELS = {
  run: '运行',
  providers: '入口库',
  'model-groups': '模型组',
  routing: 'Claude Code 路由',
  security: '安全',
  statistics: '统计',
  diagnostics: '诊断',
  help: '帮助',
  about: '关于',
};

export function Topbar() {
  const {
    currentRoute, navigate, theme, toggleTheme, setIsCommandOpen, streamConnected,
    autoRefresh, setAutoRefresh, refreshIntervalOptions,
    runRefreshInterval, setRunRefreshInterval,
    statisticsRefreshInterval, setStatisticsRefreshInterval,
    auth, logout,
  } = useAppControls();

  const statisticsPage = currentRoute === 'statistics';
  const refreshInterval = statisticsPage ? statisticsRefreshInterval : runRefreshInterval;
  const setRefreshInterval = statisticsPage ? setStatisticsRefreshInterval : setRunRefreshInterval;
  const refreshLabel = statisticsPage ? '统计刷新' : '运行刷新';
  const username = auth?.username || 'Admin';

  return (
    <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center justify-between border-b border-border/75 bg-background/80 px-4 backdrop-blur-xl sm:px-6 lg:px-7">
      <div className="mx-auto flex w-full max-w-[1440px] items-center justify-between gap-2 md:gap-4">
        <div className="flex shrink-0 items-center gap-2.5">
          <SidebarTrigger aria-label="切换导航侧栏" />
          <Separator orientation="vertical" className="h-4 bg-border/80" />
          <div
            className={cn(
              'inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full border px-2.5 text-xs font-medium transition-colors',
              streamConnected
                ? 'border-status-good/25 bg-status-good/10 text-status-good'
                : 'border-border/80 bg-muted/65 text-muted-foreground',
            )}
            role="status"
            aria-live="polite"
            title={streamConnected ? '实时推流已连接' : '推流重连中...'}
          >
            <span
              className={cn(
                'size-1.5 shrink-0 rounded-full',
                streamConnected ? 'status-sonar-ring bg-status-good text-status-good' : 'bg-muted-foreground',
              )}
              aria-hidden="true"
            />
            <span className="hidden whitespace-nowrap md:inline">
              {streamConnected ? '实时推流' : '重连中…'}
            </span>
          </div>
          <span className="hidden items-center gap-1.5 truncate text-xs font-medium text-muted-foreground lg:inline-flex">
            <span className="text-border" aria-hidden="true">/</span>
            <span className="text-foreground/90">{ROUTE_LABELS[currentRoute] || '运行'}</span>
          </span>
        </div>

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => setIsCommandOpen(true)}
          title="打开快捷指令面板 (⌘K)"
          className="h-8 min-w-0 max-w-sm flex-1 shrink justify-between gap-2 rounded-xl border-border/80 bg-muted/45 px-3 text-muted-foreground shadow-none hover:border-border hover:bg-muted/75 hover:text-foreground"
        >
          <span className="flex items-center gap-2 truncate text-xs font-normal">
            <Search className="size-3.5 shrink-0 text-muted-foreground" />
            <span className="truncate">搜索页面或执行指令…</span>
          </span>
          <KbdGroup className="hidden sm:inline-flex">
            <Kbd>⌘</Kbd>
            <Kbd>K</Kbd>
          </KbdGroup>
        </Button>

        <div className="flex shrink-0 items-center gap-1.5 sm:gap-2">
          <div
            className="hidden h-8 items-center gap-2 rounded-xl border border-border/80 bg-muted/40 pl-2.5 pr-1 md:flex"
            title={`${refreshLabel}${autoRefresh ? '已开启' : '已关闭'}`}
          >
            <label className="flex cursor-pointer items-center gap-1.5 text-xs font-medium text-muted-foreground select-none hover:text-foreground">
              <Switch
                checked={autoRefresh}
                onCheckedChange={setAutoRefresh}
                aria-label={`${refreshLabel}自动刷新`}
              />
              <span>{refreshLabel}</span>
            </label>
            <Select
              value={String(refreshInterval)}
              onValueChange={(value) => setRefreshInterval(Number(value))}
              disabled={!autoRefresh}
            >
              <SelectTrigger
                size="sm"
                className="h-6 w-[66px] rounded-lg border-0 bg-card/90 px-2 text-xs font-mono tabular-nums shadow-2xs"
                aria-label={`${refreshLabel}间隔`}
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {refreshIntervalOptions.map((seconds) => (
                  <SelectItem key={seconds} value={String(seconds)}>
                    {seconds}秒
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <Button
            type="button"
            variant="outline"
            size="icon"
            className="size-8 rounded-xl border-border/80 bg-card/70 shadow-none"
            onClick={toggleTheme}
            title={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
            aria-label="切换主题"
          >
            {theme === 'dark' ? <Sun className="size-4" /> : <Moon className="size-4" />}
          </Button>

          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-8 gap-2 rounded-xl border-border/80 bg-card/70 pl-1.5 pr-2.5 shadow-none"
              >
                <span className="flex size-5 items-center justify-center rounded-full bg-primary/15 font-mono text-[11px] font-semibold text-primary">
                  {username.slice(0, 1).toUpperCase()}
                </span>
                <span className="hidden max-w-[100px] truncate text-xs font-medium sm:inline">{username}</span>
                <ChevronDown className="size-3 text-muted-foreground" aria-hidden="true" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-48">
              <div className="px-2.5 py-2 text-xs">
                <div className="truncate font-semibold text-foreground">{username}</div>
                <div className="text-[11px] text-muted-foreground">控制台管理员</div>
              </div>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => navigate('security')}>
                <ShieldCheck className="size-3.5" />
                安全与监听配置
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem variant="destructive" onSelect={logout}>
                <LogOut className="size-3.5 text-destructive" />
                退出登录
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
    </header>
  );
}
