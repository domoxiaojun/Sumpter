import { useEffect, useRef } from 'react';

const FOCUSABLE = 'button:not([disabled]), a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

// Each dialog owns dismissal, so an in-flight save cannot be dismissed by a
// global Escape shortcut. Only the topmost dialog handles focus and keys.
export function useDialogFocus(open, { onClose, canClose = () => true, initialFocusRef } = {}) {
  const dialogRef = useRef(null);
  const options = useRef(null);
  options.current = { onClose, canClose, initialFocusRef };
  useEffect(() => {
    if (!open) return undefined;
    const dialog = dialogRef.current;
    if (!dialog) return undefined;
    const previousFocus = document.activeElement;
    const isTop = () => [...document.querySelectorAll('[role="dialog"][aria-modal="true"]')].at(-1) === dialog;
    const items = () => [...dialog.querySelectorAll(FOCUSABLE)].filter(node => node.getClientRects().length > 0);
    const focusFirst = () => (options.current.initialFocusRef?.current || items()[0] || dialog).focus();
    const frame = requestAnimationFrame(focusFirst);
    const onFocus = event => {
      if (isTop() && !dialog.contains(event.target)) focusFirst();
    };
    const onKey = event => {
      if (!isTop()) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        if (options.current.canClose()) options.current.onClose?.();
      } else if (event.key === 'Tab') {
        const targets = items();
        const first = targets[0] || dialog;
        const last = targets.at(-1) || dialog;
        if (!dialog.contains(document.activeElement) || document.activeElement === dialog
          || (event.shiftKey ? document.activeElement === first : document.activeElement === last)) {
          event.preventDefault();
          (event.shiftKey ? last : first).focus();
        }
      }
    };
    document.addEventListener('keydown', onKey, true);
    document.addEventListener('focusin', onFocus);
    return () => {
      cancelAnimationFrame(frame);
      document.removeEventListener('keydown', onKey, true);
      document.removeEventListener('focusin', onFocus);
      if (previousFocus?.isConnected) previousFocus.focus({ preventScroll: true });
    };
  }, [open]);
  return dialogRef;
}
