import assert from 'node:assert/strict';
import test from 'node:test';
import { requestKindLabel, clientVariantLabel, agentRoleLabel, clientAttributionSummary, eventPurposeLabel, purposeLabel } from '../src/utils/helpers.js';
import { mergeRuntimeEvent } from '../src/utils/runtimeEvents.js';

test('new attribution labels preserve the difference between unknown and historical missing values', () => {
  for (const label of [requestKindLabel, clientVariantLabel, agentRoleLabel]) {
    assert.equal(label(undefined), '未记录');
    assert.equal(label('unrecorded'), '未记录');
    assert.equal(label('unknown'), '未识别');
  }
  assert.equal(requestKindLabel('memory'), '记忆任务');
  assert.equal(eventPurposeLabel({ requestKind: 'memory', requestPurpose: 'standard' }), purposeLabel('standard'));
  assert.equal(clientVariantLabel('vscode'), 'VS Code');
  assert.equal(agentRoleLabel('subagent'), '子代理');
});

test('evidence and conflicts survive list/detail merges', () => {
  const clientAttribution = { version: 1, evidence: [{ field: 'requestKind', source: 'native', rule: 'deepseek-compact-v1', value: 'compaction' }], conflicts: ['requestKind'], issues: [] };
  const event = mergeRuntimeEvent({ id: 'r', requestKind: 'compaction', clientAttribution }, { id: 'r', detailsOmitted: true });
  assert.equal(event.requestKind, 'compaction');
  assert.match(clientAttributionSummary(event.clientAttribution), /原生标记/);
  assert.match(clientAttributionSummary(event.clientAttribution), /信号冲突/);
  assert.equal(clientAttributionSummary(null), '未记录');
});

test('request-kind filters reach pagination, analytics, facets, dimensions and exports', async () => {
  globalThis.window = { location: { search: '?mock=1', assign() {} } };
  const { api } = await import('../src/services/api.js');
  const original = api.request;
  const paths = [];
  api.request = async (path) => { paths.push(path); return { facets: { requestKinds: [{ value: 'memory', count: 2 }], clientVariants: [{ value: 'sdk', count: 2 }] } }; };
  try {
    await api.getRuntimeEventPage({ requestKind: 'memory' });
    await api.getRuntimeAnalytics('all', { requestKind: 'memory' });
    const facets = await api.getRuntimeFacets('all', { requestKind: 'memory' });
    assert.equal(facets.facets.requestKinds[0].value, 'memory');
    assert.equal(facets.facets.clientVariants[0].value, 'sdk');
    await api.getRuntimeDimensions('requestKind', { filters: { requestKind: 'memory' } });
    await api.estimateRuntimeExport({ filters: { requestKind: 'memory' } });
    for (const path of paths) assert.equal(new URL(path, 'https://example.invalid').searchParams.get('requestKind'), 'memory', path);
  } finally { api.request = original; }
});
