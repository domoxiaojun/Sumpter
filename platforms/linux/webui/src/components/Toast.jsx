import React from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

export function ToastContainer() {
  const { toasts, removeToast } = useApp();

  if (!toasts || toasts.length === 0) return null;

  return (
    <div className="toast-container" aria-live="polite">
      {toasts.map((toast) => {
        const iconName = toast.kind === 'success' ? 'check' : toast.kind === 'error' ? 'warning' : 'activity';
        return (
          <div key={toast.id} className={`toast-item ${toast.kind}`}>
            <span style={{ color: toast.kind === 'success' ? 'var(--status-good)' : toast.kind === 'error' ? 'var(--status-critical)' : 'var(--primary)' }}>
              <Icon name={iconName} size={18} />
            </span>
            <span style={{ fontSize: '0.88rem', color: 'var(--text-primary)', flex: 1 }}>{toast.message}</span>
            <button
              type="button"
              className="btn-icon"
              style={{ width: '24px', height: '24px', border: 'none', background: 'transparent' }}
              onClick={() => removeToast(toast.id)}
            >
              <Icon name="close" size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
