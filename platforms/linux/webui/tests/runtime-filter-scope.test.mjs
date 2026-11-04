import assert from 'node:assert/strict';
import test from 'node:test';
import {
  APPLE_EPOCH_OFFSET_SECONDS, observeRuntimeDay, runtimeAnalyticsFilters,
  runtimeRangeFilter, runtimeSnapshotFilters,
} from '../src/utils/runtimeAnalyticsV2.js';

globalThis.window = { location: { search: '' } };
const { api } = await import('../src/services/api.js');
const advanced = {
  clientKind: 'codex', requestKind: 'review', clientVariant: 'desktop', agentRole: 'subagent',
  agentName: 'reviewer', parentThreadID: 'parent', parentTurnID: 'turn', rootTurnID: 'root',
};

function fixedClock(t, value) {
  const original = Date;
  const timestamp = new original(value).getTime();
  globalThis.Date = class extends original {
    constructor(...args) { super(...(args.length ? args : [timestamp])); }
    static now() { return timestamp; }
  };
  t.after(() => { globalThis.Date = original; });
}

function timezone(t, value) {
  const previous = process.env.TZ;
  process.env.TZ = value;
  t.after(() => { if (previous === undefined) delete process.env.TZ; else process.env.TZ = previous; });
}

test('7d/30d bounds and every agent filter reach trends, dimensions, errors and both export requests', async t => {
  fixedClock(t, '2026-11-04T12:00:00+08:00');
  const paths = [];
  t.mock.method(api, 'request', async path => { paths.push(path); return {}; });
  const previousDocument = globalThis.document;
  globalThis.document = { createElement: () => ({ style: {}, click() { paths.push(this.href); }, remove() {} }) };
  t.after(() => { globalThis.document = previousDocument; });
  for (const [range, days] of [['7d', 7], ['30d', 30]]) {
    paths.length = 0;
    const filters = runtimeAnalyticsFilters(range, advanced);
    const from = Date.now() / 1000 - APPLE_EPOCH_OFFSET_SECONDS - days * 86400;
    assert.equal(filters.from, from);
    await api.getRuntimeTrends({ range, filters });
    await api.getRuntimeDimensions('project', { filters });
    await api.getRuntimeDimension('sessions', { filters });
    await api.getRuntimeErrors({ filters });
    await api.estimateRuntimeExport({ privacy: 'redacted', filters });
    await api.downloadRuntimeExport({ privacy: 'redacted', filters });
    assert.equal(paths.length, 6);
    for (const path of paths) {
      const query = new URL(path, 'http://test.invalid').searchParams;
      assert.equal(Number(query.get('from')), from, path);
      for (const [key, value] of Object.entries(advanced)) assert.equal(query.get(key), value, `${path}: ${key}`);
    }
  }
});

test('local today survives absent boundaries at all range-aware API entry points', async t => {
  timezone(t, 'Asia/Singapore');
  fixedClock(t, '2026-11-04T02:00:00+08:00');
  const paths = [];
  t.mock.method(api, 'request', async path => { paths.push(path); return {}; });
  const filters = { from: undefined, to: null, project: '' };
  await api.getRuntimeFacets('today', filters);
  await api.getRuntimeTrends({ range: 'today', filters });
  await api.getRuntimeAnalytics('today', filters);
  const from = new Date('2026-11-04T00:00:00+08:00').getTime() / 1000 - APPLE_EPOCH_OFFSET_SECONDS;
  for (const path of paths) assert.equal(Number(new URL(path, 'http://test.invalid').searchParams.get('from')), from);
  assert.deepEqual(runtimeAnalyticsFilters('today', { from: 0, to: 1 }), { from: 0, to: 1 });
  assert.deepEqual(runtimeRangeFilter('all'), {});
});

test('paging retains snapshot bounds while fresh refreshes advance the rolling range', () => {
  const initial = runtimeAnalyticsFilters('7d', advanced, new Date('2026-11-04T00:00:00Z'));
  const fresh = runtimeAnalyticsFilters('7d', advanced, new Date('2026-11-04T01:00:00Z'));
  assert.equal(fresh.from - initial.from, 3600);
  const page = runtimeSnapshotFilters({ ...fresh, projectID: 'project-1' }, { snapshotSeq: 100, filters: initial });
  assert.deepEqual(page, { ...initial, projectID: 'project-1' });
  assert.deepEqual(runtimeSnapshotFilters(fresh, { sort: 'last_seen' }), fresh);
});

test('mounted today rolls at local midnight, catches up on visibility, and cleans up timers', t => {
  timezone(t, 'Asia/Singapore');
  let now = new Date('2026-11-04T23:59:00+08:00');
  let timerID = 0;
  const timers = new Map();
  const days = [];
  const visibility = Object.assign(new EventTarget(), { hidden: false });
  const focus = new EventTarget();
  const stop = observeRuntimeDay(day => days.push(day), {
    now: () => now,
    schedule: (callback, delay) => { const id = ++timerID; timers.set(id, { callback, delay }); return id; },
    cancel: id => timers.delete(id), visibility, focus,
  });
  assert.equal(timers.values().next().value.delay, 60000);
  now = new Date('2026-11-05T00:00:00+08:00');
  timers.values().next().value.callback();
  assert.equal(days[1] - days[0], 86400);
  now = new Date('2026-11-06T09:00:00+08:00');
  visibility.dispatchEvent(new Event('visibilitychange'));
  assert.equal(days[2] - days[1], 86400);
  focus.dispatchEvent(new Event('focus'));
  assert.equal(days.length, 3);
  stop();
  assert.equal(timers.size, 0);
  visibility.dispatchEvent(new Event('visibilitychange'));
  assert.equal(timers.size, 0);
});

test('midnight scheduling follows 23-hour and 25-hour local days', t => {
  timezone(t, 'America/New_York');
  for (const [value, hours] of [['2026-03-08T00:00:00-05:00', 23], ['2026-11-01T00:00:00-04:00', 25]]) {
    let delay;
    const stop = observeRuntimeDay(() => {}, {
      now: () => new Date(value), schedule: (_, value) => { delay = value; return 1; },
      cancel() {}, visibility: null, focus: null,
    });
    assert.equal(delay, hours * 3600000);
    stop();
  }
});
