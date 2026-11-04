import { useState, useMemo, useEffect, useRef } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';
import { useDialogFocus } from '../hooks/useDialogFocus.js';

export function CommandPalette() {
  const {
    isCommandOpen, setIsCommandOpen, navigate, toggleTheme,
    toggleProxy, refreshCore, status,
  } = useAppControls();
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef(null);
  const dialogRef = useDialogFocus(isCommandOpen, { onClose: () => setIsCommandOpen(false), initialFocusRef: inputRef });

  const commands = useMemo(() => [
    { id: 'nav-run', title: '转到：运行监控', category: '页面跳转', icon: 'play', action: () => navigate('run') },
    { id: 'nav-model-groups', title: '转到：模型组', category: '页面跳转', icon: 'route', action: () => navigate('model-groups') },
    { id: 'nav-providers', title: '转到：入口库', category: '页面跳转', icon: 'server', action: () => navigate('providers') },
    { id: 'nav-routing', title: '转到：Claude Code 路由', category: '页面跳转', icon: 'route', action: () => navigate('routing') },
    { id: 'nav-security', title: '转到：安全与监听配置', category: '页面跳转', icon: 'shield', action: () => navigate('security') },
    { id: 'nav-stats', title: '转到：使用统计与分析', category: '页面跳转', icon: 'chart', action: () => navigate('statistics') },
    { id: 'nav-diag', title: '转到：系统诊断', category: '页面跳转', icon: 'wrench', action: () => navigate('diagnostics') },
    { id: 'nav-help', title: '转到：帮助', category: '页面跳转', icon: 'help', action: () => navigate('help') },
    { id: 'nav-about', title: '转到：关于', category: '页面跳转', icon: 'info', action: () => navigate('about') },
    { id: 'act-toggle-proxy', title: status?.running ? '停止代理服务' : '启动代理服务', category: '控制操作', icon: status?.running ? 'stop' : 'play', action: toggleProxy },
    { id: 'act-refresh', title: '刷新系统与配置数据', category: '控制操作', icon: 'refresh', action: refreshCore },
    { id: 'act-theme', title: '切换深色/浅色主题', category: '外观设置', icon: 'moon', action: toggleTheme },
  ], [navigate, toggleProxy, refreshCore, toggleTheme, status]);

  const filtered = useMemo(() => {
    if (!query.trim()) return commands;
    const q = query.toLowerCase();
    return commands.filter((c) => c.title.toLowerCase().includes(q) || c.category.toLowerCase().includes(q));
  }, [commands, query]);

  useEffect(() => {
    if (!isCommandOpen) return undefined;
    setQuery('');
    setSelectedIndex(0);
  }, [isCommandOpen]);

  const executeItem = (item) => {
    setIsCommandOpen(false);
    item.action();
  };

  const handleKeyDown = (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      if (filtered.length) setSelectedIndex((prev) => (prev + 1) % filtered.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      if (filtered.length) setSelectedIndex((prev) => (prev - 1 + filtered.length) % filtered.length);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (filtered[selectedIndex]) executeItem(filtered[selectedIndex]);
    }
  };

  if (!isCommandOpen) return null;

  return (
    <div className="modal-overlay" onClick={() => setIsCommandOpen(false)}>
      <div
        className="modal-dialog command-palette-dialog"
        ref={dialogRef}
        tabIndex={-1}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
      >
        <div className="command-palette-header">
          <Icon name="search" size={18} className="text-muted" />
          <input
            ref={inputRef}
            type="text"
            className="form-input command-palette-input"
            aria-label="搜索指令和页面"
            role="combobox"
            aria-expanded="true"
            aria-autocomplete="list"
            aria-controls="command-palette-results"
            aria-activedescendant={filtered[selectedIndex] ? `command-${filtered[selectedIndex].id}` : undefined}
            placeholder="输入指令或搜索页面 (如: 运行、Provider、启动代理)..."
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={handleKeyDown}
          />
          <button type="button" className="btn-icon" aria-label="关闭命令面板" onClick={() => setIsCommandOpen(false)}><Icon name="close" size={16} /></button>
        </div>

        <div className="command-palette-list" id="command-palette-results" role="listbox" aria-label="可用指令">
          {filtered.length === 0 ? (
            <div className="command-palette-empty">
              未找到匹配的指令或页面
            </div>
          ) : (
            filtered.map((item, idx) => {
              const isSelected = idx === selectedIndex;
              return (
                <div
                  key={item.id}
                  id={`command-${item.id}`}
                  role="option"
                  aria-selected={isSelected}
                  className={`command-palette-item${isSelected ? ' is-selected' : ''}`}
                  onClick={() => executeItem(item)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                >
                  <div className="command-palette-icon">
                    <Icon name={item.icon} size={15} />
                  </div>
                  <div className="command-palette-copy">
                    <span className="command-palette-title">{item.title}</span>
                    <span className="command-palette-category">{item.category}</span>
                  </div>
                  {isSelected && <Icon name="chevron" size={15} />}
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
