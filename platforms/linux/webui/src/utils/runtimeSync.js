import { upsertRuntimeEvent } from './runtimeEvents.js';

export const RUNTIME_API_VERSION = 1;

function pageGenerationMatches(page, expectedResetGeneration) {
  return page?.resetGeneration == null
    || expectedResetGeneration == null
    || Number(page.resetGeneration) === Number(expectedResetGeneration);
}

export async function fetchRuntimeChanges({
  fetchPage,
  afterChangeSeq,
  resetGeneration,
  limit = 200,
}) {
  let cursor = Number(afterChangeSeq || 0);
  const changes = [];

  while (true) {
    const page = await fetchPage({ afterChangeSeq: cursor, limit });
    if (page?.cursorValid === false || !pageGenerationMatches(page, resetGeneration)) {
      return { valid: false, changes: [], resetGeneration: page?.resetGeneration };
    }

    const pageEvents = Array.isArray(page?.events) ? page.events : [];
    changes.push(...pageEvents);
    const nextCursor = pageEvents.reduce(
      (latest, item) => Math.max(latest, Number(item?.changeSeq || 0)),
      cursor,
    );

    if (!page?.hasMore) {
      return {
        valid: true,
        changes,
        lastChangeSeq: nextCursor,
        resetGeneration: page?.resetGeneration ?? resetGeneration,
      };
    }
    // A full page must advance the cursor. Otherwise another request would
    // loop forever, so fall back to a newest-page resync.
    if (!pageEvents.length || nextCursor <= cursor) {
      return { valid: false, changes: [], resetGeneration: page?.resetGeneration };
    }
    cursor = nextCursor;
  }
}

export function mergeRuntimeListItems(events = [], changes = []) {
  let merged = events;
  for (const item of changes) merged = upsertRuntimeEvent(merged, item);
  return merged;
}

// Cheap change detector for the statistics workspace. Only fields that can
// change the visible result participate; object identity and SQLite internals
// must not turn a quiet polling tick into a React tree update.
export function runtimeSummaryRevision(summary) {
  const storage = summary?.storage || {};
  const latest = summary?.latestEvent || {};
  return JSON.stringify({
    resetGeneration: summary?.resetGeneration ?? 0,
    historyGeneration: summary?.historyGeneration ?? storage?.historyGeneration ?? 0,
    counters: summary?.counters || {},
    latestEvent: {
      id: latest.id || null,
      seq: latest.seq ?? null,
      changeSeq: latest.changeSeq ?? null,
      phase: latest.phase || null,
      outcome: latest.outcome || null,
      statusCode: latest.statusCode ?? null,
      timestamp: latest.timestamp ?? null,
    },
    storage: {
      state: storage.state || null,
      eventCount: storage.eventCount ?? storage.event_count ?? null,
      completedEventCount: storage.completedEventCount ?? storage.completed_event_count ?? null,
      inFlightEventCount: storage.inFlightEventCount ?? storage.in_flight_event_count ?? null,
      pendingEvents: storage.pendingEvents ?? storage.pending_events ?? null,
      pendingBytes: storage.pendingBytes ?? storage.pending_bytes ?? null,
      lastError: storage.lastError ?? storage.last_error ?? null,
      startupIssue: summary?.startupIssue || null,
    },
  });
}
