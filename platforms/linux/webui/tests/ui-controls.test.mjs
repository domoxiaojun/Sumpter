import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import vm from 'node:vm';
import { createNavigationGuard } from '../src/utils/navigationGuard.js';
import { normalizeRuntimePagedResult, runtimeDimensionSortState } from '../src/utils/runtimeAnalyticsV2.js';

const read = path => readFile(new URL(path, import.meta.url), 'utf8');
const table = await read('../src/components/DataTable.jsx');
const workspace = await read('../src/components/AnalyticsV2Workspace.jsx');
const paginate = await read('../src/components/PaginationBar.jsx');
const rust = await read('../../../../crates/sumpter-runtime/src/runtime_query.rs');
const adapters = await Promise.all([
  '../../../../adapters/linux/sumpter-linux-adapter/src/admin_runtime.rs',
  '../../../../adapters/macos/sumpter-macos-adapter/src/admin.rs',
].map(read));
const clickBody = table.match(/onClick=\{\(\) => onSortChange\(([\s\S]*?)\)\}/)[1];
const clickSort = new Function('col', 'activeSort', 'sortDirection', 'onSortChange', `onSortChange(${clickBody})`);
const response = (sort, order = 'asc') => ({ rows: [{ key: 'model-a' }], totalCount: 25, page: 1, pageSize: 10, snapshotSeq: 8, historyGeneration: 1, sort, order });

test('every sortable usage column round-trips through both adapters and Rust serialization', () => {
  const columns = workspace.slice(workspace.indexOf('const identityColumn'), workspace.indexOf('const costColumns'));
  const keys = [...new Set([...columns.matchAll(/key: '([^']+)'[^\n]*sortable: true/g)].map(match => match[1]))];
  assert.equal(keys.length, 11);
  const variants = new Map();
  const body = rust.match(/pub enum DimensionSort \{([\s\S]*?)\n\}/)[1];
  for (const match of body.matchAll(/(?:#\[serde\(rename = "([^"]+)"\)\]\s*)?(\w+),/g)) {
    variants.set(match[2], match[1] || match[2].replace(/[A-Z]/g, (letter, index) => `${index ? '_' : ''}${letter.toLowerCase()}`));
  }
  for (const adapter of adapters) {
    const parser = adapter.slice(adapter.indexOf('let sort = match query.sort'), adapter.indexOf('let order = match query.order'));
    for (const key of keys) {
      const entry = [...parser.matchAll(/((?:"[^"]+"\s*(?:\|\s*)?)+)=>\s*\{?\s*DimensionSort::(\w+)/g)]
        .find(match => match[1].includes(`"${key}"`));
      assert.ok(entry, `adapter accepts ${key}`);
      assert.equal(variants.get(entry[2]), key, `${key} must serialize back to the table key`);
    }
  }
});

test('cache headers reverse after legacy responses and preserve paging sort', () => {
  for (const key of ['cache_read', 'cache_write']) {
    let page = normalizeRuntimePagedResult(response('last_seen', 'desc'));
    const orders = [];
    for (let count = 0; count < 3; count++) {
      clickSort({ key }, page.sort === key, page.order, (sort, order) => {
        orders.push(order);
        page = normalizeRuntimePagedResult(response(`${sort}_tokens`, order));
      });
      assert.equal(page.sort, key);
      assert.deepEqual(runtimeDimensionSortState(page), { sort: key, order: page.order });
    }
    assert.deepEqual(orders, ['asc', 'desc', 'asc']);
  }
});

function sourceFunction(name, globals = {}) {
  const start = workspace.indexOf(`function ${name}(`);
  const end = workspace.indexOf('\n}', start) + 2;
  return vm.runInNewContext(`(${workspace.slice(start, end)})`, globals);
}

test('unknown cache rates stay unknown while measured zero remains 0.0%', () => {
  const percent = sourceFunction('percent');
  for (const value of [null, undefined, '', NaN]) assert.equal(percent(value), '—');
  assert.equal(percent(0), '0.0%');
  assert.equal(percent(0.375), '37.5%');
});

test('background refresh skips an in-flight sort and resumes with the completed selection', async () => {
  const begin = sourceFunction('beginLatestRequest', { AbortController, safeNumber: value => Number(value) || 0 });
  const reference = { current: null };
  const first = begin(reference);
  const sort = begin(reference);
  assert.equal(first.controller.signal.aborted, true);
  first.finish();
  assert.equal(reference.current.pending, true);
  const calls = [];
  const context = {
    useCallback: callback => callback,
    dimensionRequestRefs: { current: { model: reference } },
    dimensions: { model: { ...response('cache_read', 'desc'), page: 2 } },
    dimensionSearch: { model: 'model' }, pageSize: 10,
    loadDimension: (...args) => calls.push(args),
  };
  const body = workspace.slice(workspace.indexOf('  const refreshDimensions ='), workspace.indexOf('  const loadStorage ='));
  vm.runInNewContext(`${body}\nglobalThis.refresh = refreshDimensions;`, context);
  await context.refresh(['model']);
  assert.equal(calls.length, 0);
  assert.equal(sort.controller.signal.aborted, false);
  sort.finish();
  await context.refresh(['model']);
  assert.equal(calls.length, 1);
  assert.equal(calls[0][1], 2);
  assert.equal(calls[0][4].sort, 'cache_read');
  assert.equal(calls[0][4].order, 'desc');
  assert.equal(calls[0][4].snapshotSeq, undefined);
});

test('snapshot recovery preserves selected sort and clears the recovered error', async () => {
  const start = workspace.indexOf('  const loadDimension =');
  const end = workspace.indexOf('  const loadDimensions =', start);
  const requests = [];
  let dimensions = { model: response('cache_read', 'desc') };
  let errors = {};
  const context = {
    useCallback: callback => callback, dimensions, pageSize: 10, dimensionSearch: {},
    selectedProject: null, selectedSession: null, activeFilters: { model: 'chosen' },
    dimensionRequestRefs: { current: { model: { current: {} } } },
    beginLatestRequest: () => ({ controller: new AbortController(), isCurrent: () => true, finish() {} }),
    setDimensionLoading() {}, setDimensionError: update => { errors = update(errors); },
    setDimensions: update => { dimensions = update(dimensions); },
    runtimeDimensionSortState, normalizeRuntimePagedResult,
    runtimeSnapshotFilters: filters => filters, sameJSON: () => false,
    isAbortError: () => false, isRuntimeSnapshotError: error => error.code === 'runtime_snapshot_expired',
    dimensionLabel: value => value,
    api: { getRuntimeDimensions: async (_kind, query) => {
      requests.push(query);
      if (requests.length === 1) throw Object.assign(new Error('expired'), { code: 'runtime_snapshot_expired' });
      return response('cache_read_tokens', 'desc');
    } },
  };
  vm.runInNewContext(`${workspace.slice(start, end)}\nglobalThis.load = loadDimension;`, context);
  await context.load('model', 2);
  assert.equal(requests.length, 2);
  assert.equal(requests[1].sort, 'cache_read');
  assert.equal(requests[1].order, 'desc');
  assert.equal(requests[1].snapshotSeq, undefined);
  assert.equal(requests[1].filters.model, 'chosen');
  assert.equal(dimensions.model.sort, 'cache_read');
  assert.equal(errors.model, null);
});

test('pagination prevents submits while loading and when only one page exists', () => {
  const start = paginate.indexOf('  const submitPage =');
  const end = paginate.indexOf('\n  return (', start);
  for (const canNavigate of [false, true]) {
    const changes = [];
    const context = { canNavigate, pageDraft: '2', currentPage: 1, pageCount: 3,
      setPageDraft() {}, onPageChange: page => changes.push(page) };
    vm.runInNewContext(`${paginate.slice(start, end)}\nglobalThis.submit = submitPage;`, context);
    context.submit({ preventDefault() {} });
    assert.deepEqual(changes, canNavigate ? [2] : []);
  }
  assert.match(paginate, /const canNavigate = !loading && Boolean\(onPageChange\) && normalizedTotalPages > 1/);
});

test('navigation cancellation keeps guards active; busy saves cannot be discarded', () => {
  let dirty = false; let saving = false; let prompts = 0; let busyNotices = 0; let accept = false;
  const guard = createNavigationGuard({ confirmDiscard: () => { prompts++; return accept; }, notifyBusy: () => { busyNotices++; } });
  const unregister = guard.register(() => ({ dirty, saving }));
  assert.equal(guard.canLeave(), true);
  dirty = true;
  assert.equal(guard.canLeave(), false);
  assert.equal(guard.shouldBlockUnload(), true);
  accept = true;
  assert.equal(guard.canLeave(), true);
  saving = true;
  assert.equal(guard.canLeave(), false);
  assert.equal(prompts, 2);
  assert.equal(busyNotices, 1);
  unregister();
  assert.equal(guard.shouldBlockUnload(), false);
});

test('action locks prevent same-turn duplicate writes and release after failures', async () => {
  const hook = await read('../src/hooks/usePendingActions.js');
  const context = { useRef: value => ({ current: value }), useState: () => [{}, () => {}] };
  vm.runInNewContext(hook.replace(/^import .*\n/, '').replace('export function', 'function') + '\nglobalThis.actions = usePendingActions();', context);
  let calls = 0; let finish;
  const pending = context.actions.run('config', () => { calls++; return new Promise(resolve => { finish = resolve; }); });
  await context.actions.run('config', () => { calls++; });
  assert.equal(calls, 1);
  await context.actions.run('admin', () => { calls++; });
  assert.equal(calls, 2);
  finish(); await pending;
  await assert.rejects(context.actions.run('config', () => { throw new Error('failed'); }), /failed/);
  await context.actions.run('config', () => { calls++; });
  assert.equal(calls, 3);
});
