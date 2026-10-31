import { mergeRuntimeEvent } from './runtimeEvents.js';

export function mergeRequestChain(snapshot, updates = [], requestID = snapshot?.requestID) {
  const byID = new Map();
  for (const event of [...(snapshot?.events || []), ...updates]) {
    if (event.requestID !== requestID) continue;
    byID.set(event.id, mergeRuntimeEvent(byID.get(event.id), event));
  }
  const events = [...byID.values()].sort((a, b) => Number(a.seq || 0) - Number(b.seq || 0));
  return { ...snapshot, requestID, events: events.slice(0, 512), truncated: Boolean(snapshot?.truncated || events.length > 512) };
}

export function chainInFlight(chain) {
  return chain?.events?.some(e => e.kind === 'client' && ['inFlight', 'in_flight'].includes(e.phase)) || false;
}

// One read at a time. Continuous SSE changes cannot postpone reads forever,
// and a slow read is allowed to finish instead of being aborted every second.
export function createRequestChainLoader({ read, onValue, enabled, interval = 1000,
  schedule = setTimeout, unschedule = clearTimeout, now = Date.now }) {
  let disposed = false;
  let active = true;
  let timer;
  let inFlight;
  let lastRead = -Infinity;
  function refresh(force = false) {
    if (disposed || inFlight || (!force && (!active || !enabled()))) return;
    if (timer != null) return;
    const delay = force ? 0 : Math.max(0, interval - (now() - lastRead));
    timer = schedule(() => { timer = undefined; void load(force); }, delay);
  }
  async function load(force) {
    if (disposed || inFlight || (!force && !enabled())) return;
    const controller = new AbortController();
    inFlight = controller;
    lastRead = now();
    try {
      const chain = await read(controller.signal);
      if (disposed || controller.signal.aborted || (!force && !enabled())) return;
      active = chainInFlight(chain);
      onValue(chain);
    } catch (error) {
      // Missing endpoints/auth stop polling; temporary read failures can recover.
      if ([401, 403, 404].includes(error?.status)) active = false;
    } finally {
      inFlight = undefined;
      if (!disposed && active && enabled()) {
        timer = schedule(() => { timer = undefined; void load(false); }, interval);
      }
    }
  }
  return {
    refresh,
    synchronize() {
      if (!enabled()) { if (timer != null) unschedule(timer); timer = undefined; }
      else refresh();
    },
    dispose() {
      disposed = true;
      if (timer != null) unschedule(timer);
      inFlight?.abort();
    },
  };
}
