import { normalizeTimestampMS } from './helpers.js';

const DEFAULT_PER_KIND_LIMIT = 200;

const omittedDetailFields = ['codexMetadata', 'clientDeclared', 'grokMetadata', 'failureDetail', 'message', 'streamTrace', 'toolCalls', 'timeoutMS', 'upstreamHost', 'upstreamRequestID'];

export function mergeRuntimeEvent(previous, incoming) {
  if (!previous || previous.id !== incoming?.id) return incoming;
  if (Number(previous.changeSeq || 0) > Number(incoming.changeSeq || 0) && Number(incoming.changeSeq || 0) > 0) return previous;
  const merged = { ...previous, ...incoming };
  if (incoming.detailsOmitted === true) {
    for (const key of omittedDetailFields) {
      if (incoming[key] == null && previous[key] != null) merged[key] = previous[key];
    }
  } else if ('streamTrace' in incoming) {
    merged.usageSummary = incoming.streamTrace?.usage ?? null;
  }
  return merged;
}

function eventPhase(event) {
  return event?.phase === 'in_flight' ? 'inFlight' : event?.phase;
}

function eventKind(event) {
  return String(event?.kind || 'unknown');
}

export function trimRuntimeEvents(events, perKindLimit = DEFAULT_PER_KIND_LIMIT) {
  const completedCounts = new Map();
  const inFlightCounts = new Map();

  return events.filter((event) => {
    const kind = eventKind(event);
    const counts = eventPhase(event) === 'inFlight' ? inFlightCounts : completedCounts;
    const count = counts.get(kind) || 0;
    if (count >= perKindLimit) return false;
    counts.set(kind, count + 1);
    return true;
  });
}

export function upsertRuntimeEvent(events = [], event, perKindLimit = DEFAULT_PER_KIND_LIMIT) {
  if (!event || !event.id) return events;
  const next = [...events];
  const existingIndex = next.findIndex((item) => item?.id === event.id);
  if (existingIndex >= 0) {
    next[existingIndex] = mergeRuntimeEvent(next[existingIndex], event);
    // Match the Rust/macOS contract: a terminal update must never make the
    // row disappear before its failure/tool/stream details can be inspected.
    // Upserts do not grow the array; the next new event re-applies quotas.
    return next;
  }
  next.unshift(event);
  return trimRuntimeEvents(next, perKindLimit);
}

function eventTimestamp(event) {
  const parsed = normalizeTimestampMS(event?.timestamp);
  return Number.isNaN(parsed) ? 0 : parsed;
}

// Existing rows keep their storage position during in-flight -> completed
// updates. Sort only for presentation so a completed request moves to the top
// according to its final timestamp while equal timestamps remain stable.
export function orderRuntimeEvents(events = [], direction = 'desc') {
  const multiplier = direction === 'asc' ? 1 : -1;
  return events
    .map((event, index) => ({ event, index }))
    .sort((left, right) => {
      const byTimestamp = multiplier * (eventTimestamp(left.event) - eventTimestamp(right.event));
      return byTimestamp || left.index - right.index;
    })
    .map(({ event }) => event);
}
