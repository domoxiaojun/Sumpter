import { useCallback, useEffect, useRef, useState } from 'react';
import { useAppControls } from '../context/AppContext.jsx';
import { ModalBusyContext } from '../context/ModalContext.js';
import { Button } from './ui/button.jsx';
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from './ui/dialog.jsx';

const ACTION_VARIANT = {
  primary: 'default',
  secondary: 'outline',
  danger: 'destructive',
  destructive: 'destructive',
  ghost: 'ghost',
};

export function Modal() {
  const { activeModal, closeModal } = useAppControls();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const busyRef = useRef(false);
  const [contentBusy, setContentBusy] = useState(false);
  const contentBusyRef = useRef(false);
  const updateContentBusy = useCallback((value) => {
    contentBusyRef.current = value;
    setContentBusy(value);
  }, []);
  const dismiss = () => {
    if (!busyRef.current && !contentBusyRef.current) closeModal();
  };
  const activeRef = useRef(activeModal);
  activeRef.current = activeModal;

  useEffect(() => {
    busyRef.current = false;
    updateContentBusy(false);
    setBusy(false);
    setError('');
  }, [activeModal, updateContentBusy]);

  const { title, content, actions = [], maxWidth = '580px' } = activeModal || {};
  const isBusy = busy || contentBusy;

  return (
    <Dialog
      open={Boolean(activeModal)}
      onOpenChange={(open) => {
        if (!open) dismiss();
      }}
    >
      <DialogContent
        showCloseButton={!isBusy}
        aria-busy={isBusy || undefined}
        aria-labelledby="modal-title"
        style={{ maxWidth }}
        className="w-[calc(100%-2rem)] max-h-[88dvh] overflow-y-auto"
        onEscapeKeyDown={(event) => {
          if (busyRef.current || contentBusyRef.current) event.preventDefault();
        }}
        onInteractOutside={(event) => {
          if (busyRef.current || contentBusyRef.current) event.preventDefault();
        }}
      >
        <DialogHeader>
          <DialogTitle id="modal-title">{title}</DialogTitle>
        </DialogHeader>

        <div className="space-y-3 text-sm">
          <ModalBusyContext.Provider value={updateContentBusy}>
            {content}
          </ModalBusyContext.Provider>
          {error && (
            <p
              role="alert"
              className="rounded-lg border border-destructive/30 bg-destructive/10 px-3 py-2 text-xs text-destructive"
            >
              {error}
            </p>
          )}
        </div>

        {actions && actions.length > 0 && (
          <DialogFooter className="pt-2">
            {actions.map((act, idx) => (
              <Button
                key={idx}
                type="button"
                variant={ACTION_VARIANT[act.kind] || 'outline'}
                onClick={async () => {
                  if (busyRef.current || contentBusyRef.current) return;
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
                disabled={act.disabled || isBusy}
              >
                {act.label}
              </Button>
            ))}
          </DialogFooter>
        )}
      </DialogContent>
    </Dialog>
  );
}
