import assert from 'node:assert/strict';
import test from 'node:test';
import { buildInboundTokenWrite, inboundTokenDraft, inboundTokenOptions, randomToken, validateInboundTokens } from '../src/utils/inboundTokens.js';

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

test('default token shares the editor while rename and disable preserve its credential', () => {
  const doc = { generation: 'current', config: { listener: { authToken: '', authTokenName: '默认 Token', authTokens: [{ id: 'work', name: '工作', enabled: true, token: '' }] } }, secretStatus: { inboundAuthToken: { configured: true, last4: '1234' } } };
  const entries = inboundTokenDraft(doc);
  assert.deepEqual(entries.map(entry => entry.id), ['legacy', 'work']);
  assert.equal(entries[0].token, null);
  entries[0].name = '第一台电脑'; entries[0].enabled = false;
  let write = buildInboundTokenWrite(doc, entries);
  assert.equal(write.config.listener.authTokenName, '第一台电脑');
  assert.equal(write.config.listener.authTokenEnabled, false);
  assert.equal(Object.hasOwn(write.secrets, 'inboundAuthToken'), false);
  assert.deepEqual(write.secrets.inboundTokens, {});
  assert.equal(write.opensAccess, false);
  assert.equal(doc.config.listener.authTokenName, '默认 Token');
  entries[0].token = 'changed-default-token';
  write = buildInboundTokenWrite(doc, entries);
  assert.equal(write.secrets.inboundAuthToken, 'changed-default-token');
  assert.equal(write.config.listener.authTokens.some(entry => entry.id === 'legacy'), false);
  assert.equal(buildInboundTokenWrite(doc, entries.slice(1)).secrets.inboundAuthToken, '');
  assert.equal(buildInboundTokenWrite(doc, []).opensAccess, true);
  entries[0].token = '';
  assert.throws(() => buildInboundTokenWrite(doc, entries), /Token 不能为空/);
});

test('secret reveal is an explicit uncached POST scoped to the displayed generation', async t => {
  const calls = [];
  t.mock.method(api, 'request', async (path, options) => { calls.push({ path, options }); return { tokenID: 'legacy', token: 'synthetic' }; });
  await api.getInboundTokenSecret('legacy', 'current');
  assert.equal(calls[0].path, '/inbound-token-secret');
  assert.equal(calls[0].options.method, 'POST');
  assert.equal(calls[0].options.cache, 'no-store');
  assert.deepEqual(JSON.parse(calls[0].options.body), { tokenID: 'legacy', expectedGeneration: 'current' });
});
