import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { RUN_OVERVIEW_RANGE, runOverviewQuery, runOverviewTotals } from '../src/utils/runOverview.js';
import { APPLE_EPOCH_OFFSET_SECONDS, runtimeCurrentRangeSnapshot, runtimeRangeFilter, runtimeSnapshotFilters } from '../src/utils/runtimeAnalyticsV2.js';
import { runtimeEventInRange } from '../src/utils/runtimeEvents.js';
import { formatTimestamp } from '../src/utils/helpers.js';

globalThis.window = { location: { search: '' } };
const { api } = await import('../src/services/api.js');
const now = new Date('2026-11-04T12:00:00+08:00');
const from = new Date('2026-11-04T00:00:00+08:00').getTime() / 1000 - APPLE_EPOCH_OFFSET_SECONDS;

function singapore(t) {
  const previous = process.env.TZ;
  process.env.TZ = 'Asia/Singapore';
  t.after(() => { if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous; });
}

test('home overview and event-page requests send the same local midnight', async t => {
  singapore(t);
  const paths = [];
  t.mock.method(api, 'request', async path => { paths.push(path); return {}; });
  const query = runOverviewQuery(now);
  assert.equal(query.range, 'today');
  assert.equal(query.granularity, 'hour');
  assert.equal(query.filters.from, from);
  await api.getRuntimeTrends(query);
  await api.getRuntimeEventPage({ kind: 'client', pageSize: 10, ...runtimeRangeFilter(RUN_OVERVIEW_RANGE, now) });
  for (const path of paths) assert.equal(Number(new URL(path, 'https://test.invalid').searchParams.get('from')), from);
  assert.equal(paths.length, 2);
});

test('history paging keeps today snapshots but drops yesterday watermarks at midnight', async t => {
  singapore(t);
  const paths = [];
  t.mock.method(api, 'request', async path => { paths.push(path); return {}; });
  const snapshot = { snapshotSeq: 42, filters: { from } };
  const later = new Date(now.getTime() + 3600_000);
  const sameDay = runtimeRangeFilter(RUN_OVERVIEW_RANGE, later);
  assert.equal(sameDay.from, from);
  assert.equal(runtimeCurrentRangeSnapshot(RUN_OVERVIEW_RANGE, snapshot, later), snapshot);
  await api.getRuntimeEventPage({ page: 2, snapshotSeq: 42, ...runtimeSnapshotFilters(sameDay, snapshot) });
  const tomorrow = new Date('2026-11-05T00:00:00+08:00');
  const current = runtimeCurrentRangeSnapshot(RUN_OVERVIEW_RANGE, snapshot, tomorrow);
  assert.equal(current, null);
  const fresh = runtimeRangeFilter(RUN_OVERVIEW_RANGE, tomorrow);
  await api.getRuntimeEventPage({ page: current ? 2 : 1, snapshotSeq: current?.snapshotSeq, ...runtimeSnapshotFilters(fresh, current) });
  assert.equal(Number(new URL(paths[0], 'https://test.invalid').searchParams.get('from')), from);
  const refreshed = new URL(paths[1], 'https://test.invalid').searchParams;
  assert.equal(Number(refreshed.get('from')), from + 86400);
  assert.equal(refreshed.get('page'), '1');
  assert.equal(refreshed.has('snapshotSeq'), false);
});

test('home request timestamps show only hours minutes and seconds', async t => {
  singapore(t);
  assert.equal(formatTimestamp(now.toISOString()), '12:00:00');
  assert.equal(formatTimestamp('2026-11-03T23:49:31+08:00'), '23:49:31');
  const source = await readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /smartDate:\s*true/);
});

test('cached and live events respect the inclusive date boundary across supported timestamp formats', () => {
  const cutoff = new Date((from + APPLE_EPOCH_OFFSET_SECONDS) * 1000);
  for (const phase of ['completed', 'inFlight']) {
    assert.equal(runtimeEventInRange({ timestamp: from - 0.001, phase }, { from }), false);
    for (const timestamp of [from, cutoff.toISOString(), cutoff.getTime(), cutoff.getTime() / 1000]) {
      assert.equal(runtimeEventInRange({ timestamp, phase }, { from }), true);
    }
  }
  assert.equal(runtimeEventInRange({ timestamp: from + 2 }, { from, to: from + 1 }), false);
  assert.equal(runtimeEventInRange({ timestamp: null }, { from }), false);
  assert.equal(runtimeEventInRange({ timestamp: 'invalid' }, { from }), false);
  assert.equal(runtimeEventInRange({ timestamp: from - 1 }, {}), true, 'unscoped callers keep their full history');
});

test('all overview counters come from one aggregate and missing values never become lifetime totals or zero', () => {
  const totals = runOverviewTotals({ totals: {
    clientRequests: 4, clientSuccesses: 2, clientFailures: 1, upstreamAttempts: 6, failovers: 2,
    tokens: { observedRequests: 3, inputTokens: 100, outputTokens: 20 },
  } });
  assert.deepEqual(totals, { input: 100, output: 20, observed: 3, clientRequests: 4, clientSuccesses: 2, clientFailures: 1, upstreamAttempts: 6, failovers: 2 });
  assert.equal(runOverviewTotals({ counters: { clientRequests: 99999 } }), null);
  const empty = runOverviewTotals({ totals: { clientRequests: 0, tokens: { observedRequests: 0, inputTokens: 0, outputTokens: 0 } } });
  assert.equal(empty.clientRequests, 0);
  assert.equal(empty.clientFailures, null);
  assert.equal(empty.upstreamAttempts, null);
});
