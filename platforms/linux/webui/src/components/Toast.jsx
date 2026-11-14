import { Activity, AlertTriangle, CheckCircle2, X } from 'lucide-react';
import { useAppControls } from '../context/AppContext.jsx';
import { cn } from '../lib/utils.js';

export function ToastContainer() {
  const { toasts, removeToast } = useAppControls();

  if (!toasts || toasts.length === 0) return null;

  return (
    <div
      className="fixed bottom-4 right-4 z-50 flex w-full max-w-sm flex-col gap-2 px-4 sm:px-0 pointer-events-none"
      aria-live="polite"
    >
      {toasts.map((toast) => {
        const ToastIcon = toast.kind === 'success'
          ? CheckCircle2
          : toast.kind === 'error'
            ? AlertTriangle
            : Activity;
        return (
          <div
            key={toast.id}
            role="status"
            className={cn(
              'pointer-events-auto flex items-center gap-2.5 rounded-xl border bg-popover/95 px-3.5 py-2.5 text-popover-foreground shadow-lg backdrop-blur-md animate-in fade-in slide-in-from-bottom-2',
              toast.kind === 'error' && 'border-status-critical/40',
              toast.kind === 'success' && 'border-status-good/35',
            )}
          >
            <ToastIcon
              className={cn(
                'size-4 shrink-0',
                toast.kind === 'success' && 'text-status-good',
                toast.kind === 'error' && 'text-status-critical',
                toast.kind !== 'success' && toast.kind !== 'error' && 'text-primary',
              )}
              aria-hidden="true"
            />
            <span className="flex-1 text-xs font-medium leading-relaxed text-foreground">
              {toast.message}
            </span>
            <button
              type="button"
              className="inline-flex size-6 shrink-0 cursor-pointer items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent hover:text-foreground"
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
