import assert from 'node:assert/strict';
import test from 'node:test';
import { inboundTokenOptions, randomToken, validateInboundTokens } from '../src/utils/inboundTokens.js';

globalThis.window = { location: { search: '' } };
const { api, toWireConfig, fromWireConfig } = await import('../src/services/api.js');

test('named token secrets travel only in explicit secret updates', async t => {
  const entry = { id: 'work', name: '工作', enabled: true, token: 'synthetic-work-token' };
  const config = { schemaVersion: 7, listener: { authTokens: [entry] }, endpoints: [] };
  assert.equal(toWireConfig(config).listener.authTokens[0].token, '');
  assert.equal(entry.token, 'synthetic-work-token');
  let sent;
  t.mock.method(api, 'request', async (_, options) => {
    sent = JSON.parse(options.body);
    return { config: sent.config, secretStatus: { inboundTokens: { work: { configured: true, last4: 'oken' } } } };
  });
  const result = await api.saveConfig('generation', config, { inboundTokens: { work: entry.token } });
  assert.deepEqual(sent.secretUpdates.inboundTokens, { work: entry.token });
  assert.deepEqual(sent.secretUpdates.endpoints, {});
  assert.equal(sent.config.listener.authTokens[0].token, '');
  assert.equal(fromWireConfig(result).secretStatus.inboundTokens.work.last4, 'oken');
});

test('renaming and deleting tokens keep historical filter IDs and selection', () => {
  const facets = { inboundTokens: [{ value: 'work', name: '旧名称', count: 3 }, { value: 'deleted', name: '已删除', count: 2 }, { value: 'unrecorded', count: 5 }] };
  const rows = inboundTokenOptions({ listener: { authTokens: [{ id: 'work', name: '新名称', enabled: false }] } }, facets, 'missing');
  assert.equal(rows.find(row => row.value === 'work').label, '新名称（已停用）');
  assert.equal(rows.find(row => row.value === 'work').count, 3);
  assert.equal(rows.find(row => row.value === 'deleted').label, '已删除');
  assert.equal(rows.find(row => row.value === 'unrecorded').label, '未记录 Token');
  assert.equal(rows.find(row => row.value === 'missing').count, 0);
});

test('generated credentials and validation reject duplicates and whitespace', () => {
  const token = randomToken();
  assert.match(token, /^sk-sumpter-[a-f0-9]{64}$/);
  assert.notEqual(token, randomToken());
  const entry = { name: '工作电脑', token };
  assert.equal(validateInboundTokens([entry]), '');
  assert.ok(validateInboundTokens([entry, entry]));
  assert.ok(validateInboundTokens([{ ...entry, token: 'with whitespace token' }]));
  assert.ok(validateInboundTokens([{ ...entry, name: '' }]));
});
