import React, { useState, useMemo, useEffect, useRef } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

export function CommandPalette() {
  const {
    isCommandOpen, setIsCommandOpen, navigate, toggleTheme,
    toggleProxy, refreshCore, status,
  } = useApp();
  const [query, setQuery] = useState('');
  const [selectedIndex, setSelectedIndex] = useState(0);
  const inputRef = useRef(null);

  const commands = useMemo(() => [
    { id: 'nav-run', title: '转到：运行监控', category: '页面跳转', icon: 'play', action: () => navigate('run') },
    { id: 'nav-providers', title: '转到：Provider 管理', category: '页面跳转', icon: 'server', action: () => navigate('providers') },
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
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [isCommandOpen]);

  const executeItem = (item) => {
    setIsCommandOpen(false);
    item.action();
  };

  const handleKeyDown = (e) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setSelectedIndex((prev) => (prev + 1) % filtered.length);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setSelectedIndex((prev) => (prev - 1 + filtered.length) % filtered.length);
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
        style={{ maxWidth: '620px', borderRadius: 'var(--radius-lg)' }}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="命令面板"
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: '12px', padding: '16px 20px', borderBottom: '1px solid var(--border-subtle)' }}>
          <Icon name="search" size={20} className="text-muted" />
          <input
            ref={inputRef}
            type="text"
            className="form-input"
            style={{ border: 'none', background: 'transparent', fontSize: '1.05rem', padding: 0, boxShadow: 'none' }}
            placeholder="输入指令或搜索页面 (如: 运行、Provider、启动代理)..."
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setSelectedIndex(0);
            }}
            onKeyDown={handleKeyDown}
          />
          <kbd className="kbd-badge">ESC</kbd>
        </div>

        <div style={{ maxHeight: '380px', overflowY: 'auto', padding: '8px' }}>
          {filtered.length === 0 ? (
            <div style={{ padding: '32px 16px', textAlign: 'center', color: 'var(--text-muted)' }}>
              未找到匹配的指令或页面
            </div>
          ) : (
            filtered.map((item, idx) => {
              const isSelected = idx === selectedIndex;
              return (
                <div
                  key={item.id}
                  onClick={() => executeItem(item)}
                  onMouseEnter={() => setSelectedIndex(idx)}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: '12px',
                    padding: '10px 14px',
                    borderRadius: 'var(--radius-md)',
                    background: isSelected ? 'var(--bg-active)' : 'transparent',
                    color: isSelected ? 'var(--primary)' : 'var(--text-primary)',
                    cursor: 'pointer',
                    transition: 'all var(--ease-fast)',
                  }}
                >
                  <div
                    style={{
                      width: '32px',
                      height: '32px',
                      borderRadius: 'var(--radius-sm)',
                      background: 'var(--bg-surface-elevated)',
                      border: '1px solid var(--border-subtle)',
                      display: 'grid',
                      placeItems: 'center',
                    }}
                  >
                    <Icon name={item.icon} size={16} />
                  </div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: '2px', flex: 1 }}>
                    <span style={{ fontSize: '0.92rem', fontWeight: 600 }}>{item.title}</span>
                    <span style={{ fontSize: '0.75rem', color: 'var(--text-muted)' }}>{item.category}</span>
                  </div>
                  {isSelected && <Icon name="chevron" size={16} />}
                </div>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}
