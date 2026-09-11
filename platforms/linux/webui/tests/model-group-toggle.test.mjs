import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

// 入口的「组内启用」开关必须在折叠头部,不能藏在展开后的设置区里。
test('binding enable toggle lives in the collapsed summary, not the expanded editor', async () => {
  const page = await readFile(new URL('../src/pages/ModelGroupsPage.jsx', import.meta.url), 'utf8');
  const start = page.indexOf('<summary className="model-group-binding-summary">');
  const summary = page.slice(start, page.indexOf('</summary>', start));
  assert.match(summary, /className="binding-state-toggle"/);
  assert.match(summary, /type="checkbox"[^>]*checked=\{b\.enabled\}/);
  assert.match(summary, /onClick=\{\(event\) => event\.stopPropagation\(\)\}/, 'summary 内的开关不能触发展开/收起');

  const editor = await readFile(new URL('../src/components/ModelGroupBindingEditor.jsx', import.meta.url), 'utf8');
  assert.doesNotMatch(editor, /在此组中启用|binding-enabled/, '展开区不再重复放开关');
});
