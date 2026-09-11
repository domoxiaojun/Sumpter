import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

// 运行页第一列要直接显示本次调用的工具:列表投影已带 toolCalls,
// 前端不能再把它当成“详情省略字段”从旧值里补。
test('recent event request cell renders tool calls from the list projection', async () => {
  const page = await readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8');
  const cell = page.slice(page.indexOf('function RecentEventRequestCell'), page.indexOf('function RecentEventRouteCell'));
  assert.match(cell, /eventToolCalls\(event\)/, '第一列必须读取 toolCalls');
  assert.match(cell, /telemetry-event-tools/, '工具行需要独立样式挂钩');
  assert.match(cell, /event\.kind === 'notify' \? \[\]/, '通知事件不显示工具行');

  const runtimeEvents = await readFile(new URL('../src/utils/runtimeEvents.js', import.meta.url), 'utf8');
  const omitted = runtimeEvents.match(/const omittedDetailFields = \[([^\]]*)\]/)[1];
  assert.doesNotMatch(omitted, /toolCalls/, '分页列表现在自带 toolCalls,不应再被视为省略字段');

  const css = await readFile(new URL('../src/styles/components.css', import.meta.url), 'utf8');
  assert.match(css, /\.telemetry-event-tools \{[^}]*white-space: nowrap/);
});
