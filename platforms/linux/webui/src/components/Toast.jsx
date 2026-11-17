import { Activity, AlertTriangle, CheckCircle2, Info, X } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';

export function ToastContainer() {
  const { toasts, removeToast } = useAppControls();

  if (!toasts || toasts.length === 0) return null;

  return (
    <div
      className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-full max-w-sm flex-col gap-2.5 px-4 sm:px-0"
      aria-live="polite"
    >
      {toasts.map((toast) => {
        const ToastIcon = toast.kind === 'success'
          ? CheckCircle2
          : toast.kind === 'error' || toast.kind === 'warning'
            ? AlertTriangle
            : toast.kind === 'info'
              ? Info
              : Activity;
        return (
          <div
            key={toast.id}
            role="status"
            className={cn(
              'surface-popover pointer-events-auto flex items-center gap-3 rounded-2xl border border-border/85 bg-popover/95 px-3.5 py-3 text-popover-foreground backdrop-blur-xl animate-in fade-in slide-in-from-bottom-2 duration-200',
              toast.kind === 'error' && 'border-status-critical/35',
              toast.kind === 'warning' && 'border-status-warning/35',
              toast.kind === 'success' && 'border-status-good/30',
            )}
          >
            <span
              className={cn(
                'flex size-7 shrink-0 items-center justify-center rounded-xl border',
                toast.kind === 'success' && 'border-status-good/25 bg-status-good/12 text-status-good',
                toast.kind === 'error' && 'border-status-critical/25 bg-status-critical/12 text-status-critical',
                toast.kind === 'warning' && 'border-status-warning/25 bg-status-warning/12 text-status-warning',
                !['success', 'error', 'warning'].includes(toast.kind) && 'border-primary/25 bg-primary/12 text-primary',
              )}
              aria-hidden="true"
            >
              <ToastIcon className="size-3.5" />
            </span>
            <span className="flex-1 text-xs font-medium leading-relaxed text-foreground">
              {toast.message}
            </span>
            <button
              type="button"
              className="inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
              aria-label="关闭通知"
              onClick={() => removeToast(toast.id)}
            >
              <X className="size-3.5" />
            </button>
          </div>
        );
      })}
    </div>
  );
}
