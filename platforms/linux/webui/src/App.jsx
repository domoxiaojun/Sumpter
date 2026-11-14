import { lazy, Suspense, useEffect } from 'react';
import { useAppControls } from './context/AppContext.jsx';
import { Sidebar } from './components/Sidebar.jsx';
import { Topbar } from './components/Topbar.jsx';
import { CommandPalette } from './components/CommandPalette.jsx';
import { Modal } from './components/Modal.jsx';
import { ToastContainer } from './components/Toast.jsx';
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
      <div className="flex min-h-dvh items-center justify-center bg-background">
        <span
          className="size-2.5 rounded-full bg-primary animate-pulse"
          role="status"
          aria-label="正在检查登录状态"
        />
      </div>
    );
  }
  if (!auth.authenticated) return <LoginPage />;

  const renderPage = () => {
    if (isLoading) {
      return (
        <div className="flex min-h-[50vh] items-center justify-center gap-2.5 text-sm text-muted-foreground">
          <span className="size-2 rounded-full bg-primary animate-pulse" aria-hidden="true" />
          <span>正在连接代理控制平面...</span>
        </div>
      );
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
          <main
            className="flex-1 p-4 outline-none md:p-6"
            data-route={currentRoute}
            id="page"
            tabIndex="-1"
          >
            <div className="mx-auto w-full max-w-[1440px]">
              <Suspense
                fallback={
                  <div className="flex min-h-[40vh] items-center justify-center gap-2.5 text-sm text-muted-foreground">
                    <span className="size-2 rounded-full bg-primary animate-pulse" aria-hidden="true" />
                    <span>正在加载页面…</span>
                  </div>
                }
              >
                {renderPage()}
              </Suspense>
            </div>
          </main>
        </SidebarInset>
        <CommandPalette />
        <Modal />
        <ToastContainer />
      </SidebarProvider>
    </TooltipProvider>
  );
}
