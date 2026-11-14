import { LogOut, Moon, Search, ShieldCheck, Sun } from 'lucide-react';
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
    <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center justify-between gap-2 border-b bg-background/85 px-3 backdrop-blur-md md:gap-3 md:px-6">
      <div className="flex shrink-0 items-center gap-2">
        <SidebarTrigger aria-label="切换导航侧栏" />
        <Separator orientation="vertical" className="h-4" />
        <div
          className={cn(
            'inline-flex shrink-0 items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-medium transition-colors',
            streamConnected
              ? 'border-status-good/30 bg-status-good/10 text-status-good'
              : 'border-border bg-muted/60 text-muted-foreground',
          )}
          role="status"
          aria-live="polite"
          title={streamConnected ? '实时推流已连接' : '推流重连中...'}
        >
          <span
            className={cn(
              'size-1.5 shrink-0 rounded-full',
              streamConnected ? 'bg-status-good animate-pulse' : 'bg-muted-foreground',
            )}
            aria-hidden="true"
          />
          <span className="hidden whitespace-nowrap md:inline">
            {streamConnected ? '实时推流已连接' : '推流重连中...'}
          </span>
        </div>
        <span className="hidden truncate text-xs text-muted-foreground xl:inline">
          / {ROUTE_LABELS[currentRoute] || '运行'}
        </span>
      </div>

      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={() => setIsCommandOpen(true)}
        title="打开快捷指令面板 (⌘K)"
        className="h-8 min-w-0 flex-1 shrink max-w-xs justify-between gap-2 text-muted-foreground hover:text-foreground"
      >
        <span className="flex items-center gap-2 truncate text-xs font-normal">
          <Search className="size-3.5 shrink-0" />
          <span className="truncate">搜索或执行指令...</span>
        </span>
        <KbdGroup className="hidden sm:inline-flex">
          <Kbd>⌘</Kbd>
          <Kbd>K</Kbd>
        </KbdGroup>
      </Button>

      <div className="flex shrink-0 items-center gap-1.5 md:gap-2">
        <div
          className="hidden items-center gap-2 rounded-lg border bg-muted/30 px-2.5 py-1 md:flex"
          title={`${refreshLabel}${autoRefresh ? '已开启' : '已关闭'}`}
        >
          <label className="flex cursor-pointer items-center gap-1.5 text-xs text-muted-foreground">
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
              className="h-6 w-[68px] border-0 bg-background/80 px-2 text-xs shadow-none"
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
          size="icon-sm"
          onClick={toggleTheme}
          title={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
          aria-label="切换主题"
        >
          {theme === 'dark' ? <Sun className="size-4" /> : <Moon className="size-4" />}
        </Button>

        <div className="flex items-center gap-1">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button type="button" variant="ghost" size="sm" className="h-8 gap-2 px-2">
                <span className="flex size-6 items-center justify-center rounded-full bg-primary/15 font-mono text-xs font-semibold text-primary">
                  {username.slice(0, 1).toUpperCase()}
                </span>
                <span className="hidden text-xs font-medium sm:inline">{username}</span>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              <div className="px-2 py-1.5 text-xs">
                <div className="font-medium text-foreground">{username}</div>
                <div className="text-[11px] text-muted-foreground">控制台管理员</div>
              </div>
              <DropdownMenuSeparator />
              <DropdownMenuItem onSelect={() => navigate('security')}>
                <ShieldCheck className="size-3.5" />
                安全与监听配置
              </DropdownMenuItem>
              <DropdownMenuItem onSelect={logout}>
                <LogOut className="size-3.5" />
                退出登录
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>

          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            onClick={logout}
            title="退出登录"
            aria-label="退出登录"
            className="text-muted-foreground hover:text-foreground"
          >
            <LogOut className="size-4" />
          </Button>
        </div>
      </div>
    </header>
  );
}
