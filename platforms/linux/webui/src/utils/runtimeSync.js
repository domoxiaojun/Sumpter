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
