import { lazy, Suspense, useEffect } from 'react';
import { useAppControls } from './context/AppContext.jsx';
import { Sidebar } from './components/Sidebar.jsx';
import { Topbar } from './components/Topbar.jsx';
import { CommandPalette } from './components/CommandPalette.jsx';
import { Modal } from './components/Modal.jsx';
import { ToastContainer } from './components/Toast.jsx';

const RunPage = lazy(() => import('./pages/RunPage.jsx').then((module) => ({ default: module.RunPage })));
const PrimaryProvidersPage = lazy(() => import('./pages/PrimaryProvidersPage.jsx').then((module) => ({ default: module.PrimaryProvidersPage })));
const ModelGroupsPage = lazy(() => import('./pages/ModelGroupsPage.jsx').then((module) => ({ default: module.ModelGroupsPage })));
const RoutingPage = lazy(() => import('./pages/RoutingPage.jsx').then((module) => ({ default: module.RoutingPage })));
const SecurityPage = lazy(() => import('./pages/SecurityPage.jsx').then((module) => ({ default: module.SecurityPage })));
const StatsPage = lazy(() => import('./pages/StatsPage.jsx').then((module) => ({ default: module.StatsPage })));
const DiagnosticsPage = lazy(() => import('./pages/DiagnosticsPage.jsx').then((module) => ({ default: module.DiagnosticsPage })));
const HelpPage = lazy(() => import('./pages/HelpPage.jsx').then((module) => ({ default: module.HelpPage })));
const AboutPage = lazy(() => import('./pages/AboutPage.jsx').then((module) => ({ default: module.AboutPage })));
import { LoginPage } from './pages/LoginPage.jsx';

export function AppContent() {
  const { currentRoute, isLoading, auth, authLoading } = useAppControls();

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
      case 'model-groups':
        return <ModelGroupsPage />;
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
        <main className="content-body" data-route={currentRoute} id="page" tabIndex="-1">
          <Suspense fallback={<div className="app-loading-state"><span className="pulse-dot" /><span>正在加载页面…</span></div>}>{renderPage()}</Suspense>
        </main>
      </div>

      <CommandPalette />
      <Modal />
      <ToastContainer />
    </div>
  );
}
