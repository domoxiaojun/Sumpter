import assert from 'node:assert/strict';
import test from 'node:test';
import { reconcileInFlightEvents } from '../src/utils/runtimeSync.js';
import { upsertRuntimeEvent } from '../src/utils/runtimeEvents.js';

test('all 250 live requests survive a refresh followed by incremental events', () => {
  const live = Array.from({ length: 250 }, (_, id) => ({ id: String(id), kind: 'client', phase: 'inFlight' }));
  const refreshed = reconcileInFlightEvents([], live);
  const updated = upsertRuntimeEvent(refreshed, { id: 'new', kind: 'client', phase: 'completed' });
  assert.equal(updated.filter((event) => event.phase === 'inFlight').length, 250);
  assert.ok(updated.some((event) => event.id === '249'));
});

test('refresh restores a long retry outside the newest ten events', () => {
  const page = Array.from({ length: 10 }, (_, id) => ({ id: String(id), phase: 'completed' }));
  const live = { id: 'long-retry', phase: 'inFlight' };
  assert.deepEqual(reconcileInFlightEvents(page, [live]), [live, ...page]);
  assert.deepEqual(reconcileInFlightEvents([live, ...page], []), page);
});

test('a completed page event wins over an earlier live snapshot, and old servers remain compatible', () => {
  const live = { id: 'request', phase: 'inFlight' };
  const completed = { id: 'request', phase: 'completed' };
  assert.deepEqual(reconcileInFlightEvents([completed], [live]), [completed]);
  assert.deepEqual(reconcileInFlightEvents([live], undefined), [live]);
});
