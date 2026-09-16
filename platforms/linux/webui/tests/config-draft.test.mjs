import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';
import test from 'node:test';
import { configDraftWrite, requireDraftGeneration } from '../src/utils/configDraft.js';
import { completedEventsAfterSnapshot, mergeCompletedPage } from '../src/utils/runtimeEventSnapshot.js';
import { normalizeRuntimeEventPage, normalizeRuntimePagedResult } from '../src/utils/runtimeAnalyticsV2.js';

const original = { generation: 'g1', config: { endpoints: [{ id: 'a', name: '原名称' }], retry: { max500Retries: 1 } } };
const latest = { generation: 'g2', config: { endpoints: [{ id: 'a', name: '原名称' }, { id: 'b' }], retry: { max500Retries: 3 } } };

test('完整草稿必须保留打开编辑时的版本且不能以最新版本覆写', async () => {
  const source = await readFile(new URL('../src/context/AppContext.jsx', import.meta.url), 'utf8');
  const start = source.indexOf('  const saveConfig = useCallback(');
  const body = source.slice(start, source.indexOf('\n  const login =', start));
  let sent;
  const state = {
    useCallback: (value) => value, clone: structuredClone, configDraftWrite, requireDraftGeneration,
    configDocRef: { current: latest }, configSaveQueueRef: { current: Promise.resolve() },
    setConfigDoc: () => {}, addToast: () => {},
    api: {
      getConfig: async () => structuredClone(latest),
      saveConfig: async (generation, config) => {
        sent = { generation, config };
        if (generation !== latest.generation) throw new Error('generation_conflict');
        return { generation: 'g3', config };
      },
    },
  };
  vm.runInNewContext(body + '\nglobalThis.save = saveConfig;', state);
  const draft = structuredClone(original.config);
  draft.endpoints[0].name = '草稿';
  await assert.rejects(state.save(draft, {}, original), /generation_conflict/);
  assert.equal(sent.generation, 'g1');
  assert.equal(draft.endpoints[0].name, '草稿');
  assert.equal(state.configDocRef.current, latest);
  assert.throws(() => state.save(draft), /缺少编辑基线/);
  const saved = await state.save((config) => { config.endpoints[0].enabled = false; return config; });
  assert.equal(saved.config.endpoints.length, 2);
  assert.equal(saved.config.retry.max500Retries, 3);
  await assert.rejects(state.save((config) => config, {}, original), (error) => error.code === 'generation_conflict');
});

test('晚完成的旧 seq 请求按 changeSeq 刷新并保留在首页，旧页不变', () => {
  const a = { id: 'a', seq: 10, changeSeq: 100, kind: 'client', phase: 'completed', outcome: 'succeeded' };
  const b = { id: 'b', seq: 11, changeSeq: 50, kind: 'client', phase: 'completed', outcome: 'succeeded' };
  const snapshot = { snapshotSeq: 11, snapshotChangeSeq: 50, events: [b] };
  const additions = completedEventsAfterSnapshot([a, b], snapshot, 'client');
  assert.deepEqual(additions, [a]);
  assert.deepEqual(mergeCompletedPage(snapshot.events, additions, 10).map((event) => event.id), ['b', 'a']);
  assert.deepEqual(snapshot.events, [b]);
  assert.equal(mergeCompletedPage(snapshot.events, additions, 1).length, 1);
  assert.deepEqual(completedEventsAfterSnapshot([a, b], { ...snapshot, snapshotChangeSeq: 100 }, 'client'), []);
});

test('快照规范化保留完成水位并兼容旧 daemon', () => {
  const wire = { events: [], rows: [], totalCount: 0, snapshotSeq: 7, snapshotChangeSeq: 90, historyGeneration: 1 };
  assert.equal(normalizeRuntimeEventPage(wire).snapshotChangeSeq, 90);
  assert.equal(normalizeRuntimePagedResult(wire).snapshotChangeSeq, 90);
  assert.equal(normalizeRuntimeEventPage({ ...wire, snapshotChangeSeq: undefined }).snapshotChangeSeq, null);
});

test('所有分页和导出 API 都传递完成水位', async () => {
  globalThis.window = { location: { search: '' } };
  const { api } = await import('../src/services/api.js');
  const captured = [];
  const previous = globalThis.fetch;
  globalThis.fetch = async (url) => { captured.push(url); return { ok: true, headers: new Headers(), text: async () => '{}' }; };
  try {
    const anchor = { snapshotSeq: 10, snapshotChangeSeq: 80, historyGeneration: 3 };
    await api.getRuntimeEventPage(anchor);
    await api.getRuntimeTrends(anchor);
    await api.getRuntimeErrors(anchor);
    await api.getRuntimeDimension('projects', anchor);
    await api.getRuntimeDimensions('model', anchor);
    await api.estimateRuntimeExport(anchor);
    assert.equal(captured.length, 6);
    for (const url of captured) assert.equal(new URL(url, 'http://localhost').searchParams.get('snapshotChangeSeq'), '80');
    let href;
    globalThis.document = { createElement: () => ({ set href(value) { href = value; }, style: {}, click() {}, remove() {} }), body: { appendChild() {} } };
    await api.downloadRuntimeExport(anchor);
    assert.equal(new URL(href, 'http://localhost').searchParams.get('snapshotChangeSeq'), '80');
  } finally { globalThis.fetch = previous; delete globalThis.document; }
});
