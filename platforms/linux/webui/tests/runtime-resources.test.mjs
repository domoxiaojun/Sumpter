import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import { BoundedLRUCache, createPageCacheLifecycle, runtimePageActive } from '../src/utils/runtimeResources.js';
import { statisticsDateBounds, statisticsRangeIsLive, nextDown, runtimeWireRange } from '../src/utils/statisticsDateRange.js';
import { runtimeAnalyticsFilters, runtimeSnapshotFilters } from '../src/utils/runtimeAnalyticsV2.js';

test('日期范围使用两端共享边界，覆盖单日、跨月、闰日、夏令时及负 Apple 时间戳', async () => {
  const fixtures = JSON.parse(await readFile(new URL('../../../../tests/fixtures/statistics-date-ranges.json', import.meta.url)));
  const originalTZ = process.env.TZ;
  try {
    for (const row of fixtures) {
      process.env.TZ = row.zone;
      const actual = statisticsDateBounds(row.startDay, row.endDay, new Date(row.now));
      assert.deepEqual(actual, { from: row.from, to: row.to }, `${row.zone} ${row.startDay}`);
      assert.ok(actual.to < row.nextMidnight);
      assert.equal(JSON.parse(JSON.stringify(actual)).to, actual.to);
      assert.deepEqual(runtimeAnalyticsFilters('custom', { customStartDay: row.startDay, customEndDay: row.endDay }, new Date(row.now)), actual);
    }
  } finally {
    if (originalTZ == null) delete process.env.TZ; else process.env.TZ = originalTZ;
  }
});

test('日期草稿拒绝无效、倒序及未来日期；历史不自动刷新，导出保留快照', () => {
  const now = new Date(2026, 10, 13, 12);
  for (const [start, end] of [['', '2026-11-12'], ['2026-02-30', '2026-03-01'], ['2026-11-13', '2026-11-12'], ['2026-11-13', '2026-11-14']]) {
    assert.throws(() => statisticsDateBounds(start, end, now));
  }
  assert.equal(statisticsRangeIsLive('custom', { customEndDay: '2026-11-12' }, now), false);
  assert.equal(statisticsRangeIsLive('custom', { customEndDay: '2026-11-13' }, now), true);
  assert.equal(runtimeWireRange('custom'), 'all');
  assert.equal(runtimeWireRange('7d'), '7d');
  const saved = { snapshotSeq: 12, filters: { from: 100, to: nextDown(200) } };
  assert.deepEqual(runtimeSnapshotFilters({ from: 300, to: 400, model: 'kept' }, saved), { model: 'kept', ...saved.filters });
});

test('LRU 同时遵守条数和字节预算，替换、触碰和超大详情不会累积', () => {
  const cache = new BoundedLRUCache(2, 10);
  cache.set('a', { id: 'a' }, 4).set('b', { id: 'b' }, 4);
  cache.get('a');
  cache.set('c', { id: 'c' }, 4);
  assert.equal(cache.get('b'), undefined);
  cache.set('a', { id: 'updated' }, 9);
  assert.equal(cache.size, 1);
  assert.equal(cache.cost, 9);
  cache.set('large', { complete: true }, 11);
  assert.equal(cache.size, 1);
  for (let index = 0; index < 1000; index++) cache.set(index, { index }, 3);
  assert.ok(cache.size <= 2 && cache.cost <= 10);
  cache.clear();
  assert.equal(cache.cost, 0);
});

test('隐藏立即取消，60 秒才释放；重开撤销旧计时器，清理不累积', () => {
  let clock = 0; let nextID = 0; let aborted = 0; let released = 0;
  const timers = new Map();
  const lifecycle = createPageCacheLifecycle({
    onVisibility() {}, onRetain() {}, cancelRequests: () => aborted++, release: () => released++,
    schedule: (callback, delay) => { const id = ++nextID; timers.set(id, { at: clock + delay, callback }); return id; },
    cancel: id => timers.delete(id),
  });
  const advance = ms => { clock += ms; for (const [id, timer] of timers) if (timer.at <= clock) { timers.delete(id); timer.callback(); } };
  lifecycle.update(false);
  assert.equal(aborted, 1);
  advance(59_999); assert.equal(released, 0);
  lifecycle.update(true); advance(1); assert.equal(released, 0);
  for (let index = 0; index < 5; index++) {
    lifecycle.update(false); advance(60_000); lifecycle.update(true);
  }
  assert.equal(released, 5);
  lifecycle.dispose(); assert.equal(timers.size, 0);
  assert.equal(runtimePageActive('run', false), false);
  assert.equal(runtimePageActive('security', true), false);
  assert.equal(runtimePageActive('statistics', true), true);
});

test('实际详情加载器切换选择会中止旧请求，关闭选择不报告取消错误', async () => {
  const source = await readFile(new URL('../src/context/AppContext.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('  const loadRuntimeEvent =');
  const end = source.indexOf('  const deleteRuntimeSession', start);
  const requests = [];
  let displayed;
  const context = {
    AbortController, useCallback: callback => callback, runtimeDataActive: () => true,
    eventDetailGenerationRef: { current: 0 }, eventDetailAbortRef: { current: null },
    eventDetailCacheRef: { current: new BoundedLRUCache() }, setRuntimeEventDetail: value => { displayed = value; },
    api: { getRuntimeEvent: (id, { signal }) => new Promise((resolve, reject) => {
      requests.push({ id, signal, resolve });
      signal.addEventListener('abort', () => reject(Object.assign(new Error('cancelled'), { name: 'AbortError' })));
    }) },
  };
  vm.runInNewContext(`${source.slice(start, end)}\nglobalThis.load = loadRuntimeEvent;`, context);
  const first = context.load('first');
  const second = context.load('second');
  assert.equal(requests[0].signal.aborted, true);
  requests[1].resolve({ changeSeq: 2, event: { id: 'second' } });
  await Promise.all([first, second]);
  assert.equal(displayed.event.id, 'second');
  const third = context.load('third');
  await context.load(null); await third;
  assert.equal(displayed, null);
  assert.equal(requests[2].signal.aborted, true);
});

test('状态读取共用单个请求，非运行页面不会查询运行摘要或事件', async () => {
  const source = await readFile(new URL('../src/context/AppContext.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('  const readStatus =');
  const end = source.indexOf('  const refreshRuntimeAggregates =', start);
  const requests = [];
  let resolveStatus;
  const context = {
    AbortController, Promise, queueMicrotask,
    useCallback: callback => callback,
    window: { setTimeout, clearTimeout }, document: { hidden: false },
    auth: { authenticated: true }, currentRouteRef: { current: 'security' }, configDocRef: { current: {} },
    lastChangeSeqRef: { current: 0 },
    statusRequestRef: { current: null }, coreAbortRef: { current: null }, aggregateAbortRef: { current: null },
    corePendingRef: { current: false }, coreRefreshGenerationRef: { current: 0 }, aggregateRefreshGenerationRef: { current: 0 },
    analyticsFacetsRevisionRef: { current: '' },
    sameJSON: () => false, requireStatusRuntimeApiV1() {}, requireRuntimeApiV1() {},
    addToast() {}, fetchAnalyticsFacets() {}, showMigrationNotice() {},
    setConfigDoc() {}, setStatus() {}, setLastError() {}, setIsLoading() {},
    api: {
      getConfig: async () => { requests.push('config'); return {}; },
      getStatus: () => { requests.push('status'); return new Promise(resolve => { resolveStatus = resolve; }); },
      getRuntimeSummary: async () => { throw new Error('unexpected summary'); },
      getRuntimeEvents: async () => { throw new Error('unexpected events'); },
    },
  };
  vm.runInNewContext(`${source.slice(start, end)}\nglobalThis.read = readStatus; globalThis.refresh = refreshCore;`, context);
  const first = context.read();
  const second = context.read();
  assert.equal(first, second);
  const core = context.refresh();
  resolveStatus({ running: true });
  await Promise.all([first, second, core]);
  assert.deepEqual(requests, ['status', 'config']);
  assert.equal(context.statusRequestRef.current, null);
  assert.equal(context.coreAbortRef.current, null);
});

test('统计自动刷新只更新首页，已翻页的表格保持原有快照', async () => {
  const source = await readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('  const refreshDimensions =');
  const end = source.indexOf('  const loadStorage =', start);
  const requests = [];
  const context = {
    useCallback: callback => callback,
    dimensionSearch: {}, pageSize: 20,
    dimensions: { endpoint: { page: 1 }, project: { page: 2, snapshotSeq: 123 } },
    dimensionRequestRefs: { current: { endpoint: { current: {} }, project: { current: {} } } },
    loadDimension: async (...args) => requests.push(args),
  };
  vm.runInNewContext(`${source.slice(start, end)}\nglobalThis.refresh = refreshDimensions;`, context);
  await context.refresh(['endpoint', 'project']);
  assert.equal(requests.length, 1);
  assert.equal(requests[0][0], 'endpoint');
  assert.equal(context.dimensions.project.snapshotSeq, 123);
});

test('导出估算在替换或离开页面时取消，取消不显示错误', async () => {
  const source = await readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('function ExportPanel(');
  const end = source.indexOf('\n  return (\n', start);
  const cleanups = []; const errors = []; const requests = [];
  let stateIndex = 0;
  const context = {
    AbortController, runtimeFilterValues: value => value,
    isAbortError: error => error?.name === 'AbortError',
    useRef: current => ({ current }),
    useState: initial => {
      const index = stateIndex++;
      return [index === 2 ? 'redacted' : initial, value => { if (index === 6) errors.push(value); }];
    },
    useEffect: effect => cleanups.push(effect()),
    api: { estimateRuntimeExport: (_, { signal }) => new Promise((resolve, reject) => {
      requests.push({ signal, resolve });
      signal.addEventListener('abort', () => reject(Object.assign(new Error('cancelled'), { name: 'AbortError' })));
    }) },
  };
  vm.runInNewContext(`${source.slice(start, end)}\nglobalThis.estimate = loadEstimate;\n}\nExportPanel({ pageVisible: true });`, context);
  const first = context.estimate();
  const second = context.estimate();
  assert.equal(requests[0].signal.aborted, true);
  cleanups.forEach(cleanup => cleanup?.());
  assert.equal(requests[1].signal.aborted, true);
  await Promise.all([first, second]);
  assert.equal(errors.filter(Boolean).length, 0);
});

test('自定义日期沿用 all/from/to 协议，趋势、facets、错误和导出范围一致', async () => {
  globalThis.window = { location: { search: '?mock=1' } };
  const { api } = await import('../src/services/api.js');
  const previous = api.request;
  const paths = [];
  api.request = async path => { paths.push(new URL(path, 'http://localhost')); return { facets: {} }; };
  const filters = { from: 100, to: nextDown(200), model: 'kept' };
  try {
    await api.getRuntimeTrends({ range: 'custom', filters });
    await api.getRuntimeFacets('custom', filters);
    await api.getRuntimeAnalytics('custom', filters);
    await api.getRuntimeErrors({ filters });
    await api.estimateRuntimeExport({ filters });
    for (const path of paths) {
      assert.equal(path.searchParams.get('from'), String(filters.from));
      assert.equal(path.searchParams.get('to'), String(filters.to));
      assert.equal(path.searchParams.get('model'), 'kept');
      assert.notEqual(path.searchParams.get('range'), 'custom');
    }
    for (const path of paths.slice(0, 3)) assert.equal(path.searchParams.get('range'), 'all');
  } finally { api.request = previous; }
});
