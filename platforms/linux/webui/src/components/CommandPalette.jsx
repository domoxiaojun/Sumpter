import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import {
  Activity,
  BarChart3,
  ChevronRight,
  CircleHelp,
  GitBranch,
  Info,
  Layers,
  Moon,
  Play,
  RefreshCw,
  Search,
  Server,
  ShieldCheck,
  Square,
  Sun,
  Wrench,
} from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';
import { Badge } from './ui/badge.jsx';
import { Dialog, DialogContent, DialogHeader, DialogTitle } from './ui/dialog.jsx';
import { InputGroup, InputGroupAddon, InputGroupInput } from './ui/input-group.jsx';

export function CommandPalette() {
  const {
    isCommandOpen, setIsCommandOpen, navigate, theme, toggleTheme,
    toggleProxy, refreshCore, status,
  } = useAppControls();
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef(null);
  const listRef = useRef(null);
  const selectedRef = useRef(null);

  const commands = useMemo(() => [
    { id: 'nav-run', title: '转到：运行监控', category: '页面跳转', IconComp: Activity, action: () => navigate('run') },
    { id: 'nav-model-groups', title: '转到：模型组', category: '页面跳转', IconComp: Layers, action: () => navigate('model-groups') },
    { id: 'nav-providers', title: '转到：入口库', category: '页面跳转', IconComp: Server, action: () => navigate('providers') },
    { id: 'nav-routing', title: '转到：Claude Code 路由', category: '页面跳转', IconComp: GitBranch, action: () => navigate('routing') },
    { id: 'nav-security', title: '转到：安全与监听配置', category: '页面跳转', IconComp: ShieldCheck, action: () => navigate('security') },
    { id: 'nav-stats', title: '转到：使用统计与分析', category: '页面跳转', IconComp: BarChart3, action: () => navigate('statistics') },
    { id: 'nav-diag', title: '转到：系统诊断', category: '页面跳转', IconComp: Wrench, action: () => navigate('diagnostics') },
    { id: 'nav-help', title: '转到：帮助', category: '页面跳转', IconComp: CircleHelp, action: () => navigate('help') },
    { id: 'nav-about', title: '转到：关于', category: '页面跳转', IconComp: Info, action: () => navigate('about') },
    { id: 'act-toggle-proxy', title: status?.running ? '停止代理服务' : '启动代理服务', category: '控制操作', IconComp: status?.running ? Square : Play, action: toggleProxy },
    { id: 'act-refresh', title: '刷新系统与配置数据', category: '控制操作', IconComp: RefreshCw, action: refreshCore },
    { id: 'act-theme', title: '切换深色/浅色主题', category: '外观设置', IconComp: theme === 'dark' ? Sun : Moon, action: toggleTheme },
  ], [navigate, toggleProxy, refreshCore, toggleTheme, status, theme]);

  const filtered = useMemo(() => {
    if (!query.trim()) return commands;
    const q = query.toLowerCase();
    return commands.filter((c) => c.title.toLowerCase().includes(q) || c.category.toLowerCase().includes(q));
  }, [commands, query]);

  useEffect(() => {
    if (!isCommandOpen) return undefined;
    setQuery('');
    setSelectedIndex(0);
    const timer = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(timer);
  }, [isCommandOpen]);

  const executeItem = (item) => {
    setIsCommandOpen(false);
    item.action();
  };

  const safeIndex = filtered.length ? Math.min(selectedIndex, filtered.length - 1) : 0;
  const selectedID = filtered[safeIndex]?.id;

  useLayoutEffect(() => {
    if (!isCommandOpen || !listRef.current || !selectedRef.current) return;
    // 只滚动指令列表，避免 scrollIntoView 同时挪动弹窗或背景页面。
    const list = listRef.current;
    const viewport = list.getBoundingClientRect();
    const option = selectedRef.current.getBoundingClientRect();
    if (option.top < viewport.top) list.scrollTop += option.top - viewport.top;
    else if (option.bottom > viewport.bottom) list.scrollTop += option.bottom - viewport.bottom;
  }, [isCommandOpen, selectedID, query]);

  const handleKeyDown = (e) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (filtered.length) setSelectedIndex((safeIndex + 1) % filtered.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (filtered.length) setSelectedIndex((safeIndex - 1 + filtered.length) % filtered.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (filtered[safeIndex]) executeItem(filtered[safeIndex]);
    }
  };

  return (
    <Dialog open={isCommandOpen} onOpenChange={setIsCommandOpen}>
      <DialogContent className="flex max-h-[calc(100dvh-2rem)] flex-col gap-0 overflow-hidden p-0 sm:max-w-lg" aria-label="命令面板">
        <DialogHeader className="sr-only">
          <DialogTitle>命令面板</DialogTitle>
        </DialogHeader>
        <div className="shrink-0 border-b px-3 py-2.5 pr-10">
          <InputGroup className="border-0 shadow-none focus-within:ring-0">
            <InputGroupAddon>
              <Search className="size-4 text-muted-foreground" />
            </InputGroupAddon>
            <InputGroupInput
              ref={inputRef}
              type="text"
              aria-label="搜索指令和页面"
              role="combobox"
              aria-expanded="true"
              aria-autocomplete="list"
              aria-controls="command-palette-results"
              aria-activedescendant={filtered[safeIndex] ? `command-${filtered[safeIndex].id}` : undefined}
              placeholder="输入指令或搜索页面 (如: 运行、Provider、启动代理)..."
              value={query}
              onChange={(e) => {
                setQuery(e.target.value);
                setSelectedIndex(0);
              }}
              onKeyDown={handleKeyDown}
            />
          </InputGroup>
        </div>

        <div
          ref={listRef}
          id="command-palette-results"
          role="listbox"
          aria-label="可用指令"
          className="min-h-0 max-h-80 overflow-y-auto p-1.5"
        >
          {filtered.length === 0 ? (
            <div className="px-3 py-8 text-center text-xs text-muted-foreground">
              未找到匹配的指令或页面
            </div>
          ) : (
            filtered.map((item, idx) => {
              const isSelected = idx === safeIndex;
              const ItemIcon = item.IconComp;
              return (
                <button
                  key={item.id}
                  id={`command-${item.id}`}
                  ref={isSelected ? selectedRef : null}
                  type="button"
                  tabIndex={-1}
                  role="option"
                  aria-selected={isSelected}
                  className={cn(
                    'flex w-full cursor-pointer items-center justify-between gap-3 rounded-lg px-3 py-2 text-left text-xs transition-colors',
                    isSelected
                      ? 'bg-accent text-accent-foreground'
                      : 'text-foreground hover:bg-accent/60',
                  )}
                  onClick={() => executeItem(item)}
                  onPointerMove={(event) => {
                    if (event.pointerType === 'mouse') setSelectedIndex(idx);
                  }}
                >
                  <span className="flex min-w-0 items-center gap-2.5">
                    <span className="flex size-6 shrink-0 items-center justify-center rounded-md border bg-background/80 text-muted-foreground">
                      <ItemIcon className="size-3.5" />
                    </span>
                    <span className="truncate font-medium">{item.title}</span>
                  </span>
                  <span className="flex shrink-0 items-center gap-1.5">
                    <Badge variant="outline" className="text-[10px] font-normal">
                      {item.category}
                    </Badge>
                    {isSelected && <ChevronRight className="size-3.5 text-muted-foreground" />}
                  </span>
                </button>
              );
            })
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
