import React from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

export function Topbar() {
  const {
    currentRoute, theme, toggleTheme, setIsCommandOpen, streamConnected,
    autoRefresh, setAutoRefresh, refreshIntervalOptions,
    runRefreshInterval, setRunRefreshInterval,
    statisticsRefreshInterval, setStatisticsRefreshInterval,
    auth, logout,
  } = useApp();
  const statisticsPage = currentRoute === 'statistics';
  const refreshInterval = statisticsPage ? statisticsRefreshInterval : runRefreshInterval;
  const setRefreshInterval = statisticsPage ? setStatisticsRefreshInterval : setRunRefreshInterval;
  const refreshLabel = statisticsPage ? '统计刷新' : '运行刷新';

  return (
    <header className="glass-topbar">
      <div className="topbar-left">
        <div className={`status-pill ${streamConnected ? 'good' : 'muted'}`} role="status" aria-live="polite">
          <span className="pulse-dot" />
          <span>{streamConnected ? '实时推流已连接' : '推流重连中...'}</span>
        </div>
      </div>

      <div className="topbar-center">
        <button
          type="button"
          className="command-search-trigger"
          onClick={() => setIsCommandOpen(true)}
          title="打开快捷指令面板 (⌘K)"
        >
          <Icon name="search" size={15} />
          <span>搜索或执行指令...</span>
          <kbd className="kbd-badge">⌘K</kbd>
        </button>
      </div>

      <div className="topbar-right">
        {/* Auto refresh dropdown */}
        <div className="topbar-auto-refresh">
          <label
            className="switch-control topbar-refresh-control"
            title={`${refreshLabel}${autoRefresh ? '已开启' : '已关闭'}`}
          >
            <input
              type="checkbox"
              className="switch-input"
              checked={autoRefresh}
              onChange={(e) => setAutoRefresh(e.target.checked)}
              aria-label={`${refreshLabel}自动刷新`}
            />
            <span className="switch-track" aria-hidden="true"><span className="switch-thumb" /></span>
            <span className="topbar-refresh-label">{refreshLabel}</span>
          </label>
          <select
            className="form-select topbar-refresh-select"
            value={refreshInterval}
            onChange={(e) => setRefreshInterval(Number(e.target.value))}
            disabled={!autoRefresh}
            aria-label={`${refreshLabel}间隔`}
          >
            {refreshIntervalOptions.map((seconds) => (
              <option key={seconds} value={seconds}>{seconds}秒</option>
            ))}
          </select>
        </div>

        {/* Theme Toggle Button */}
        <button
          type="button"
          className="btn-icon topbar-theme-button"
          onClick={toggleTheme}
          title={theme === 'dark' ? '切换为亮色主题' : '切换为暗色主题'}
          aria-label="切换主题"
        >
          <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={18} />
        </button>

        {/* User Account */}
        <div className="topbar-account">
          <div className="topbar-avatar">
            {(auth?.username || 'A').slice(0, 1).toUpperCase()}
          </div>
          <span className="topbar-username">{auth?.username || 'Admin'}</span>
          <button type="button" className="btn-icon" onClick={logout} title="退出登录" aria-label="退出登录"><Icon name="logout" size={16} /></button>
        </div>
      </div>
    </header>
  );
}
