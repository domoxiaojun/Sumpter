import test from 'node:test';
import assert from 'node:assert/strict';
import { createLocalID } from '../src/utils/helpers.js';
import { hasRoutableModel, endpointGroupModels, groupModels, groupRoutePreview, modelCatalogCategories, modelGroupPrefix, newModelGroup, pruneGroupReferences } from '../src/utils/modelGroups.js';

globalThis.window = { location: { search: '?mock=1' } };
const { fromWireConfig, toWireConfig } = await import('../src/services/api.js');

const config = () => ({ schemaVersion: 7, endpoints: ['a', 'b'].map((id) => ({
  id, name: id, enabled: true, protocol: 'auto', baseURL: `https://${id}.invalid`,
  mappings: [{ clientPattern: 'gpt-*', upstreamModel: '', thinking: 'adaptive', effort: 'high' }],
})), retry: { maxDeferredRounds: 0, max500Retries: 2, responseTimeoutSeconds: null }, featureRules: [] });

test('legacy group materialization preserves wildcards and leaves retries intact', () => {
  const c = config(); const before = structuredClone(c);
  const groups = groupModels(c);
  assert.deepEqual(groups[0].models, ['gpt-*']);
  assert.deepEqual(groups[0].bindings.map((b) => b.models), [['gpt-*'], ['gpt-*']]);
  assert.deepEqual(c, before);
  assert.deepEqual(groupModels({ ...c, modelGroups: [] }), []);
});

test('group priorities dominate entry priorities; selected/all and local overrides work', () => {
  const c = config();
  const groups = [
    { id: 'one', name: 'one', enabled: true, priority: 1, models: ['gpt-x', 'claude-x'], bindings: [
      { endpointID: 'a', enabled: true, priority: 2, models: null },
      { endpointID: 'b', enabled: true, priority: 3, models: ['gpt-x'], overrides: [{ model: 'gpt-x', priority: 0 }] },
    ] },
    { id: 'two', name: 'two', enabled: true, priority: 2, models: ['gpt-x'], bindings: [
      { endpointID: 'a', enabled: true, priority: 0, models: null },
    ] },
  ];
  assert.deepEqual(groupRoutePreview(c, groups, 'gpt-x').map((r) => `${r.groupID}/${r.endpointID}`), ['one/b', 'one/a', 'two/a']);
  assert.deepEqual(groupRoutePreview(c, groups, 'claude-x').map((r) => r.endpointID), []);
  assert.deepEqual(groupRoutePreview(c, groups, 'missing'), []);
});

test('save roundtrip retains groups, nil/all semantics, effort and original unlimited retries', () => {
  const c = config(); c.modelGroups = groupModels(c); c.modelGroups[0].bindings[0].models = null;
  c.modelGroups[0].bindings[0].overrides = [{ model: 'gpt-x', upstreamModel: 'private-gpt', priority: 1 }];
  const saved = toWireConfig(fromWireConfig({ config: c }).config);
  assert.deepEqual(saved.modelGroups, c.modelGroups);
  assert.deepEqual(saved.retry, c.retry);
  assert.equal(saved.endpoints[0].mappings[0].effort, 'high');
});

test('deleting endpoint/model prunes references while keeping global endpoint resources', () => {
  const c = config(); c.modelGroups = groupModels(c);
  c.featureRules = [{ target: { endpointID: 'b', model: 'gpt-x' } }];
  c.endpoints.pop(); pruneGroupReferences(c);
  assert.deepEqual(c.modelGroups[0].bindings.map((b) => b.endpointID), ['a']);
  assert.equal(c.featureRules[0].target.endpointID, undefined);
  c.modelGroups[0].models = []; pruneGroupReferences(c);
  assert.deepEqual(c.modelGroups[0].bindings[0].models, []);
  assert.equal(c.endpoints.length, 1);
});

test('compact group defaults and decorated model names match the engine', () => {
  const c = config();
  c.modelGroups = [{ id: 'main', models: ['gpt-x'], bindings: [{ endpointID: 'a' }] }];
  const groups = groupModels(c);
  assert.equal(groups[0].enabled, true);
  assert.equal(groups[0].name, 'main');
  assert.equal(groups[0].bindings[0].models, null);
  assert.equal(groupRoutePreview(c, groups, ' gpt-x ( ultra )[1m] ')[0].endpointID, 'a');
});

test('model categories are derived only from endpoint mappings', () => {
  const categories = modelCatalogCategories([
    { catalog: { models: [' GPT-5.6-sol ', 'claude_opus_5', 'plain'] }, mappings: [{ clientPattern: 'vendor:model' }] },
    { catalog: { models: ['gpt-5.6-sol'] }, mappings: [] },
  ]);
  assert.deepEqual(categories.map((category) => category.id), ['vendor']);
  assert.deepEqual(categories.find((category) => category.id === 'vendor').models, ['vendor:model']);
  assert.equal(modelGroupPrefix('plain'), '__other__');
  assert.equal(modelGroupPrefix('vendor:model'), 'vendor');
});

test('versioned family prefixes stay together without rewriting mapped model IDs', () => {
  const models = ['qwen-turbo', 'qwen3.6-plus', 'Qwen3.7-plus', 'qwen2.5', 'wan2.7-video'];
  const categories = modelCatalogCategories([{ modelMappings: models.map((from) => ({ from })) }]);
  assert.deepEqual(categories.map((c) => c.id), ['qwen', 'wan']);
  assert.equal(categories[0].models.length, 4);
  assert.ok(categories[0].models.includes('Qwen3.7-plus'));
  assert.equal(modelGroupPrefix('plain'), '__other__');
});

test('binding candidates use only the endpoint support intersected with group scope', () => {
  const endpoint = { catalog: { models: ['qwen3.6-plus', 'qwen3.7-plus', 'private-alias', 'out-of-group'] },
    modelMappings: [{ from: 'gpt-*' }, { from: 'custom' }] };
  const group = ['qwen*', 'gpt-x', 'custom', 'alias', 'claude-x'];
  const before = structuredClone({ endpoint, group });
  assert.deepEqual(endpointGroupModels(endpoint, group), ['custom', 'gpt-x']);
  assert.deepEqual({ endpoint, group }, before);
  assert.deepEqual(endpointGroupModels(undefined, group), []);
  assert.deepEqual(endpointGroupModels({ mappings: [] }, group), []);
  assert.deepEqual(endpointGroupModels({ mappings: [{ clientPattern: '*' }] }, ['qwen*', 'claude-x']), ['claude-x', 'qwen*']);
});

test('fetched-only catalogs never create candidates, including wildcard groups', () => {
  const endpoint = { catalog: { models: ['qwen3.6-plus', 'qwen3.7-plus'] }, mappings: [] };
  assert.deepEqual(modelCatalogCategories([endpoint]), []);
  assert.deepEqual(endpointGroupModels(endpoint, ['*']), []);
  endpoint.mappings.push({ clientPattern: 'qwen3.6-plus', upstreamModel: 'private-qwen' });
  assert.deepEqual(modelCatalogCategories([endpoint]).flatMap((c) => c.models), ['qwen3.6-plus']);
  assert.deepEqual(endpointGroupModels(endpoint, ['qwen*']), ['qwen3.6-plus']);
  endpoint.mappings = [{ clientPattern: 'qwen*' }];
  assert.deepEqual(modelCatalogCategories([endpoint]).flatMap((c) => c.models), ['qwen*']);
  assert.deepEqual(endpointGroupModels(endpoint, ['*']), ['qwen*']);
});

test('added client names and wildcards are deduplicated across endpoints', () => {
  const endpoints = [
    { mappings: [{ clientPattern: 'alias', upstreamModel: 'private-model' }, { clientPattern: 'gpt-*' }] },
    { modelMappings: [{ from: 'alias', to: 'another-upstream' }, { from: 'qwen3.7-plus' }] },
  ];
  assert.deepEqual(modelCatalogCategories(endpoints).flatMap((c) => c.models).sort(), ['alias', 'gpt-*', 'qwen3.7-plus']);
  assert.deepEqual(endpointGroupModels(endpoints[0], ['alias', 'qwen3.7-plus']), ['alias']);
});

test('new model groups get an id without crypto.randomUUID', () => {
  const cryptoObj = globalThis.crypto;
  const original = cryptoObj?.randomUUID;
  Object.defineProperty(cryptoObj, 'randomUUID', { configurable: true, writable: true, value: undefined });
  try {
    const group = newModelGroup();
    assert.match(group.id, /^group-[a-z0-9]+-[a-z0-9]+$/);
    assert.equal(group.name, '新模型组');
    assert.equal(group.enabled, true);
    assert.deepEqual(group.models, []);
    assert.deepEqual(group.bindings, []);
    assert.notEqual(createLocalID('group'), createLocalID('group'));
  } finally {
    Object.defineProperty(cryptoObj, 'randomUUID', { configurable: true, writable: true, value: original });
  }
});

test('all and stale bindings cannot restore removed endpoint mappings', () => {
  for (const models of [null, ['gpt-6-astra'], ['*'], []]) {
    const c = config();
    c.endpoints[0].mappings = [{ clientPattern: 'claude-*' }];
    c.endpoints[1].mappings = [{ clientPattern: 'gpt-6-astra' }];
    c.modelGroups = [{ id: 'main', models: ['*'], bindings: [
      { endpointID: 'a', models }, { endpointID: 'b', models: null },
    ] }];
    assert.deepEqual(groupRoutePreview(c, c.modelGroups, 'gpt-6-astra').map((e) => e.endpointID), ['b']);
    c.endpoints[1].mappings = [];
    c.endpoints[1].catalog = { models: ['gpt-6-astra'] };
    assert.deepEqual(groupRoutePreview(c, c.modelGroups, 'gpt-6-astra'), []);
    c.endpoints[0].mappings = [];
    assert.equal(hasRoutableModel(c), false);
    c.endpoints[1].mappings = [{ clientPattern: 'gpt-5.*' }];
    assert.equal(hasRoutableModel(c), true);
    assert.deepEqual(groupRoutePreview(c, c.modelGroups, 'gpt-6-astra'), []);
    assert.deepEqual(groupRoutePreview(c, c.modelGroups, 'gpt-5.5').map((e) => e.endpointID), ['b']);
  }
});
