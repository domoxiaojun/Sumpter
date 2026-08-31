import React from 'react';
import { useApp } from '../context/AppContext.jsx';
import { Icon } from '../utils/icons.jsx';

export function Modal() {
  const { activeModal, closeModal } = useApp();

  if (!activeModal) return null;

  const { title, content, actions = [], maxWidth = '580px' } = activeModal;

  return (
    <div className="modal-overlay" onClick={closeModal}>
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
          <button type="button" className="btn-icon" onClick={closeModal} aria-label="关闭弹窗">
            <Icon name="close" size={16} />
          </button>
        </div>

        <div className="modal-body">{content}</div>

        {actions && actions.length > 0 && (
          <div className="modal-footer">
            {actions.map((act, idx) => (
              <button
                key={idx}
                type="button"
                className={`btn btn-${act.kind || 'secondary'}`}
                onClick={async () => {
                  if (act.onClick) {
                    const keepOpen = await act.onClick();
                    if (!keepOpen) closeModal();
                  } else {
                    closeModal();
                  }
                }}
                disabled={act.disabled}
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
