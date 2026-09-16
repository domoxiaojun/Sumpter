import React, { useEffect, useRef, useState } from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

export function Modal() {
  const { activeModal, closeModal } = useApp();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const busyRef = useRef(false);
  const activeRef = useRef(activeModal);
  activeRef.current = activeModal;
  useEffect(() => {
    busyRef.current = false;
    setBusy(false);
    setError('');
  }, [activeModal]);

  if (!activeModal) return null;

  const { title, content, actions = [], maxWidth = '580px' } = activeModal;

  return (
    <div className="modal-overlay" onClick={() => { if (!busyRef.current) closeModal(); }}>
      <div
        className="modal-dialog"
        style={{ maxWidth }}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
      >
        <div className="modal-header">
          <h3 id="modal-title" style={{ fontSize: '1.1rem', fontWeight: 700, color: 'var(--text-primary)', margin: 0 }}>
            {title}
          </h3>
          <button type="button" className="btn-icon" onClick={closeModal} disabled={busy} aria-label="关闭弹窗">
            <Icon name="close" size={16} />
          </button>
        </div>

        <div className="modal-body">{content}{error && <p role="alert" className="form-hint">{error}</p>}</div>

        {actions && actions.length > 0 && (
          <div className="modal-footer">
            {actions.map((act, idx) => (
              <button
                key={idx}
                type="button"
                className={`btn btn-${act.kind || 'secondary'}`}
                onClick={async () => {
                  if (busyRef.current) return;
                  busyRef.current = true;
                  setBusy(true);
                  setError('');
                  try {
                    const keepOpen = act.onClick ? await act.onClick() : false;
                    if (!keepOpen && activeRef.current === activeModal) closeModal();
                  } catch (failure) {
                    if (activeRef.current === activeModal) setError(failure.message || '保存失败，草稿已保留');
                  } finally {
                    if (activeRef.current === activeModal) {
                      busyRef.current = false;
                      setBusy(false);
                    }
                  }
                }}
                disabled={act.disabled || busy}
              >
                {act.label}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
