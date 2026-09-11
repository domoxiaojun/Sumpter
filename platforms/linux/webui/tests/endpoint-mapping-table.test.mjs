import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

// 入口库的模型映射改为就地编辑 + 批量操作,不再每条映射进弹窗。
test('endpoint mapping table edits rows inline with bulk selection and a single save', async () => {
  const page = await readFile(new URL('../src/pages/PrimaryProvidersPage.jsx', import.meta.url), 'utf8');
  assert.match(page, /function EndpointMappingTable\(/);
  assert.doesNotMatch(page, /openMappingEditor|handleDeleteMapping|MappingThinkingControls/, '逐条弹窗编辑/删除应已移除');

  const table = page.slice(page.indexOf('function EndpointMappingTable('), page.indexOf('function EndpointPricingEditor('));
  assert.match(table, /aria-label="全选当前可见映射"/);
  assert.match(table, /className="mapping-bulk-bar"/);
  assert.match(table, /删除选中/);
  assert.match(table, /aria-label="批量设置 Thinking"/);
  assert.match(table, /disabled=\{!dirty \|\| saving \|\| invalidCount > 0\}/, '只有有改动且无错误时才能保存');
  assert.match(table, /target\.modelMappings = mappings;\s*delete target\.mappings;/, '保存要整表覆盖并清掉旧别名字段');
  assert.match(table, /与另一行的客户端模型重复/, '需要重复客户端模型校验');
  // 未知字段的保留逻辑在表格组件上方的 mappingDraftRow / mappingFromDraftRow 里。
  const helpers = page.slice(page.indexOf('function mappingDraftRow('), page.indexOf('function EndpointMappingTable('));
  assert.match(helpers, /_extra: Object\.fromEntries/, '未知映射字段要保留');
  assert.match(helpers, /\.\.\.row\._extra,/, '保存时要把未知字段带回');
});

test('mapping table keeps future mapping fields through the draft roundtrip', async () => {
  const page = await readFile(new URL('../src/pages/PrimaryProvidersPage.jsx', import.meta.url), 'utf8');
  assert.match(page, /MAPPING_EDITABLE_FIELDS = \['from', 'to', 'clientPattern', 'upstreamModel', 'thinking', 'effort', 'context', 'failoverTimeoutSeconds'\]/);
  const css = await readFile(new URL('../src/styles/components.css', import.meta.url), 'utf8');
  assert.match(css, /\.mapping-grid-row\.is-invalid/);
  assert.match(css, /@media \(max-width: 900px\) \{[\s\S]*\.mapping-grid-head \{ display: none; \}/);
});
