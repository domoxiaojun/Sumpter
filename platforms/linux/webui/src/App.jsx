import { lazy, Suspense, useEffect } from 'react';
import { useAppControls } from './context/AppContext.jsx';
import { Sidebar } from './components/Sidebar.jsx';
import { Topbar } from './components/Topbar.jsx';
import { CommandPalette } from './components/CommandPalette.jsx';
import { Modal } from './components/Modal.jsx';
import { ToastContainer } from './components/Toast.jsx';
import { ErrorBoundary } from './components/ErrorBoundary.jsx';
import { SidebarInset, SidebarProvider } from './components/ui/sidebar.jsx';
import { TooltipProvider } from './components/ui/tooltip.jsx';
import { LoginPage } from './pages/LoginPage.jsx';

const RunPage = lazy(() => import('./pages/RunPage.jsx').then((module) => ({ default: module.RunPage })));
const PrimaryProvidersPage = lazy(() => import('./pages/PrimaryProvidersPage.jsx').then((module) => ({ default: module.PrimaryProvidersPage })));
const ModelGroupsPage = lazy(() => import('./pages/ModelGroupsPage.jsx').then((module) => ({ default: module.ModelGroupsPage })));
const RoutingPage = lazy(() => import('./pages/RoutingPage.jsx').then((module) => ({ default: module.RoutingPage })));
const SecurityPage = lazy(() => import('./pages/SecurityPage.jsx').then((module) => ({ default: module.SecurityPage })));
const StatsPage = lazy(() => import('./pages/StatsPage.jsx').then((module) => ({ default: module.StatsPage })));
const DiagnosticsPage = lazy(() => import('./pages/DiagnosticsPage.jsx').then((module) => ({ default: module.DiagnosticsPage })));
const HelpPage = lazy(() => import('./pages/HelpPage.jsx').then((module) => ({ default: module.HelpPage })));
const AboutPage = lazy(() => import('./pages/AboutPage.jsx').then((module) => ({ default: module.AboutPage })));

function LoadingPill({ label }) {
  return (
    <div className="flex min-h-[45vh] items-center justify-center">
      <div className="surface-card inline-flex items-center gap-2.5 rounded-full border border-border/80 bg-card/90 px-4 py-2 text-xs font-medium text-muted-foreground backdrop-blur-sm">
        <span className="status-sonar-ring size-2 rounded-full bg-primary text-primary" aria-hidden="true" />
        <span>{label}</span>
      </div>
    </div>
  );
}

export function AppContent() {
  const { currentRoute, runtimePagesRetained, isLoading, auth, authLoading, theme } = useAppControls();

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark');
  }, [theme]);

  useEffect(() => {
    if (!auth.authenticated) return undefined;
    const frame = requestAnimationFrame(() => document.getElementById('page')?.focus({ preventScroll: true }));
    return () => cancelAnimationFrame(frame);
  }, [auth.authenticated, currentRoute]);

  if (authLoading) {
    return (
      <div className="ambient-canvas-glow flex min-h-dvh items-center justify-center bg-background">
        <span
          className="status-sonar-ring size-2.5 rounded-full bg-primary text-primary"
          role="status"
          aria-label="正在检查登录状态"
        />
      </div>
    );
  }
  if (!auth.authenticated) return <LoginPage />;

  const renderPage = () => {
    if (isLoading) {
      return <LoadingPill label="正在连接代理控制平面…" />;
    }

    if (!runtimePagesRetained && ['run', 'statistics'].includes(currentRoute)) return null;
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
    <TooltipProvider>
      <SidebarProvider>
        <a className="skip-link" href="#page">跳转到主要内容</a>
        <Sidebar />
        <SidebarInset className="min-w-0">
          <Topbar />
          <div
            className="flex-1 px-4 py-5 outline-none sm:px-6 sm:py-6 lg:px-7"
            data-route={currentRoute}
            id="page"
            tabIndex="-1"
          >
            <div className="mx-auto w-full max-w-[1440px]">
              <ErrorBoundary resetKey={currentRoute}>
                <Suspense fallback={<LoadingPill label="正在加载页面…" />}>
                  <div key={currentRoute} className="page-stage-enter">
                    {renderPage()}
                  </div>
                </Suspense>
              </ErrorBoundary>
            </div>
          </div>
        </SidebarInset>
        <CommandPalette />
        <Modal />
        <ToastContainer />
      </SidebarProvider>
    </TooltipProvider>
  );
}
