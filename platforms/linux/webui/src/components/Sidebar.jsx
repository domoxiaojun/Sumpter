import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';
import donkeyHead from '../assets/donkey-logo.png';

export function Sidebar() {
  const { currentRoute, navigate, status } = useApp();
  const [open, setOpen] = useState(false);
  const [isMobile, setIsMobile] = useState(() => typeof window !== 'undefined' && window.matchMedia('(max-width: 900px)').matches);
  const menuButtonRef = useRef(null);
  const asideRef = useRef(null);

  const navGroups = [
    {
      title: '控制面板',
      items: [
        { id: 'run', title: '运行', icon: 'play' },
        { id: 'providers', title: 'Provider', icon: 'server' },
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

  const isRunning = status?.running;

  useEffect(() => {
    setOpen(false);
  }, [currentRoute]);

  useEffect(() => {
    if (typeof window === 'undefined') return undefined;
    const media = window.matchMedia('(max-width: 900px)');
    const onChange = (event) => {
      setIsMobile(event.matches);
      if (!event.matches) setOpen(false);
    };
    setIsMobile(media.matches);
    if (media.addEventListener) media.addEventListener('change', onChange);
    else media.addListener?.(onChange);
    return () => {
      if (media.removeEventListener) media.removeEventListener('change', onChange);
      else media.removeListener?.(onChange);
    };
  }, []);

  const closeMenu = useCallback(() => {
    setOpen(false);
    if (isMobile) requestAnimationFrame(() => menuButtonRef.current?.focus());
  }, [isMobile]);

  useEffect(() => {
    if (!isMobile || !open) return undefined;
    const aside = asideRef.current;
    const mainWrapper = document.querySelector('.main-wrapper');
    const previousInert = mainWrapper?.inert ?? false;
    const previousOverflow = document.body.style.overflow;
    if (mainWrapper) mainWrapper.inert = true;
    document.body.style.overflow = 'hidden';

    const focusableSelector = 'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
    const focusables = () => Array.from(aside?.querySelectorAll(focusableSelector) || []).filter((element) => element.getClientRects().length > 0);
    const frame = requestAnimationFrame(() => focusables()[0]?.focus());
    const onKeyDown = (event) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        closeMenu();
        return;
      }
      if (event.key !== 'Tab') return;
      const items = focusables();
      if (!items.length) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (event.shiftKey && document.activeElement === first) {
        event.preventDefault();
        last.focus();
      } else if (!event.shiftKey && document.activeElement === last) {
        event.preventDefault();
        first.focus();
      }
    };
    document.addEventListener('keydown', onKeyDown);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener('keydown', onKeyDown);
      if (mainWrapper) mainWrapper.inert = previousInert;
      document.body.style.overflow = previousOverflow;
    };
  }, [closeMenu, isMobile, open]);

  return (
    <>
      <button
        ref={menuButtonRef}
        type="button"
        className="mobile-menu-toggle"
        aria-label="打开主导航"
        aria-expanded={open}
        aria-controls="primary-navigation"
        onClick={() => setOpen((value) => !value)}
      >
        <Icon name="menu" size={20} />
      </button>
      {open && <button type="button" className="mobile-nav-backdrop" aria-label="关闭主导航" aria-hidden="true" tabIndex="-1" onClick={closeMenu} />}
      <aside
        ref={asideRef}
        id="primary-navigation"
        className={`glass-sidebar ${open ? 'mobile-open' : ''}`}
        aria-label="应用主导航"
        role={isMobile ? 'dialog' : undefined}
        aria-modal={isMobile && open ? 'true' : undefined}
        aria-hidden={isMobile && !open ? 'true' : undefined}
        inert={isMobile && !open ? true : undefined}
      >
      <div className="brand-section">
        <div className="brand-icon-wrap" aria-hidden="true">
          <img src={donkeyHead} alt="" />
        </div>
        <div className="brand-text-wrap">
          <span className="brand-title">Sumpter</span>
          <span className="brand-badge">DONKEY TECH</span>
        </div>
        <button type="button" className="mobile-nav-close" aria-label="关闭主导航" onClick={closeMenu}>
          <Icon name="close" size={18} />
        </button>
      </div>

      <nav className="sidebar-nav" aria-label="主路由">
        {navGroups.map((group) => (
          <div key={group.title} className="nav-group">
            <div className="nav-group-title">{group.title}</div>
            {group.items.map((item) => {
              const active = currentRoute === item.id;
              return (
                <button
                  key={item.id}
                  type="button"
                  className={`nav-link-btn ${active ? 'active' : ''}`}
                  onClick={() => { navigate(item.id); setOpen(false); }}
                  aria-current={active ? 'page' : undefined}
                >
                  <span className="nav-icon">
                    <Icon name={item.icon} size={18} />
                  </span>
                  <span>{item.title}</span>
                </button>
              );
            })}
          </div>
        ))}
      </nav>

      <div className="sidebar-footer">
        <nav className="sidebar-resource-links" aria-label="帮助与关于">
          {resourceItems.map((item) => {
            const active = currentRoute === item.id;
            return (
              <button
                key={item.id}
                type="button"
                className={`nav-link-btn ${active ? 'active' : ''}`}
                onClick={() => { navigate(item.id); setOpen(false); }}
                aria-current={active ? 'page' : undefined}
              >
                <span className="nav-icon"><Icon name={item.icon} size={18} /></span>
                <span>{item.title}</span>
              </button>
            );
          })}
        </nav>
        <div className="system-status-indicator">
          <span className={`pulse-dot ${isRunning ? 'status-good' : 'status-muted'}`} style={{ color: isRunning ? 'var(--status-good)' : 'var(--status-muted)' }} />
          <div className="system-status-text">
            <strong>
              {isRunning ? '代理运行中' : '代理已就绪'}
            </strong>
            <span>
              {status?.listener ? `${status.listener.host}:${status.listener.port}` : '127.0.0.1:57878'}
            </span>
          </div>
        </div>
      </div>
      </aside>
    </>
  );
}
