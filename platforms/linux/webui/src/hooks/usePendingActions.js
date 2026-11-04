import { useRef, useState } from 'react';

// The ref closes the gap before React renders a disabled button.
export function usePendingActions() {
  const inFlight = useRef(new Set());
  const [pending, setPending] = useState({});
  const run = async (key, action) => {
    if (inFlight.current.has(key)) return;
    inFlight.current.add(key);
    setPending(previous => ({ ...previous, [key]: true }));
    try { return await action(); }
    finally {
      inFlight.current.delete(key);
      setPending(previous => ({ ...previous, [key]: false }));
    }
  };
  return { pending, run };
}
