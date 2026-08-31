import React, { useEffect } from 'react';
import { useApp } from './context/AppContext.jsx';
import { Sidebar } from './components/Sidebar.jsx';
import { Topbar } from './components/Topbar.jsx';
import { CommandPalette } from './components/CommandPalette.jsx';
import { Modal } from './components/Modal.jsx';
import { ToastContainer } from './components/Toast.jsx';

import { RunPage } from './pages/RunPage.jsx';
import { PrimaryProvidersPage } from './pages/PrimaryProvidersPage.jsx';
import { RoutingPage } from './pages/RoutingPage.jsx';
import { SecurityPage } from './pages/SecurityPage.jsx';
import { StatsPage } from './pages/StatsPage.jsx';
import { DiagnosticsPage } from './pages/DiagnosticsPage.jsx';
import { HelpPage } from './pages/HelpPage.jsx';
import { AboutPage } from './pages/AboutPage.jsx';
import { LoginPage } from './pages/LoginPage.jsx';

export function AppContent() {
  const { currentRoute, isLoading, auth, authLoading } = useApp();

  useEffect(() => {
    if (!auth.authenticated) return undefined;
    const frame = requestAnimationFrame(() => document.getElementById('page')?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [auth.authenticated, currentRoute]);

  if (authLoading) return <div className="login-shell"><div className="pulse-dot" aria-label="正在检查登录状态" /></div>;
  if (!auth.authenticated) return <LoginPage />;

  const renderPage = () => {
    if (isLoading) {
      return (
        <div className="app-loading-state">
          <div className="pulse-dot" />
          <span>正在连接代理控制平面...</span>
        </div>
      );
    }

    switch (currentRoute) {
      case 'run':
        return <RunPage />;
      case 'providers':
      case 'providers-primary':
        return <PrimaryProvidersPage />;
      case 'routing':
        return <RoutingPage />;
      case 'security':
        return <SecurityPage />;
      case 'statistics':
        return <StatsPage />;
      case 'diagnostics':
        return <DiagnosticsPage />;
      case 'help':
        return <HelpPage />;
      case 'about':
        return <AboutPage />;
      default:
        return <RunPage />;
    }
  };

  return (
    <div className="app-container">
      <a className="skip-link" href="#page">跳转到主要内容</a>
      <Sidebar />
      <div className="main-wrapper">
        <Topbar />
        <main className="content-body" id="page" tabIndex="-1">
          {renderPage()}
        </main>
      </div>

      <CommandPalette />
      <Modal />
      <ToastContainer />
    </div>
  );
}
