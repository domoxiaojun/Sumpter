// HTML 外壳契约:构建产物直接由 daemon `--web-root` 托管,不能依赖任何外部主机,
// 也不能引用 web-root 里不存在的额外文件(会 404)。favicon 因此内联为 data URI。
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';

const RUN_PAGE = new URL('../src/pages/RunPage.jsx', import.meta.url);
const STATS_PAGE = new URL('../src/pages/StatsPage.jsx', import.meta.url);
const ANALYTICS_WORKSPACE = new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url);
const DIAGNOSTICS_PAGE = new URL('../src/pages/DiagnosticsPage.jsx', import.meta.url);

const SHELLS = [
  ['webui/index.html', new URL('../index.html', import.meta.url)],
  ['web/index.html', new URL('../../web/index.html', import.meta.url)],
];

for (const [label, url] of SHELLS) {
  test(`${label} 声明内联 favicon`, () => {
    const html = readFileSync(url, 'utf8');
    const match = html.match(/<link\s+rel="icon"\s+href="([^"]+)"/);
    assert.ok(match, '必须有 rel="icon",否则浏览器请求 /favicon.ico 得到 404');
    assert.ok(
      match[1].startsWith('data:image/svg+xml,'),
      'favicon 必须内联为 data URI,不能指向额外文件或外部主机',
    );
    assert.ok(match[1].length < 8192, 'favicon data URI 不应过大');
  });

  test(`${label} 不含外部主机引用`, () => {
    const html = readFileSync(url, 'utf8');
    const hosts = html.match(/https?:\/\/(?!www\.w3\.org)[^"'\s>]+/g) || [];
    assert.deepEqual(hosts, [], `离线交付不得引用外部主机:${hosts.join(', ')}`);
  });
}

test('RunPage uses the shared inspector with independent collapsed groups', () => {
  const source = readFileSync(RUN_PAGE, 'utf8');
  const inspector = readFileSync(new URL('../src/components/EventInspector.jsx', import.meta.url), 'utf8');
  assert.match(source, /<EventInspector/);
  assert.match(inspector, /data-event-detail-tier="primary"/);
  assert.match(inspector, /<details[\s\S]*?<summary>/);
  assert.match(inspector, /open=\{open\}/);
  assert.match(inspector, /eventHttpTone\(event\)/);
  assert.match(inspector, /eventCacheLabel\(event\)/);
});

test('RunPage 分页控件位于事件列表上方', () => {
  const source = readFileSync(RUN_PAGE, 'utf8');
  const pager = source.indexOf('<PaginationBar');
  const eventContent = source.indexOf('className="event-page-content"');
  assert.ok(pager >= 0, '运行页必须保留分页控件');
  assert.ok(eventContent > pager, '分页控件必须在事件列表内容之前');
  assert.doesNotMatch(source, /加载更多.*1000|最多 1000 条/);
});

test('event identity fields are shared across clients and remain in expandable details', () => {
  const source = readFileSync(new URL('../src/components/EventInspector.jsx', import.meta.url), 'utf8');
  for (const label of ['会话来源', '父线程', '父回合', '根回合', 'Grok agent ID', '项目来源', '工作区']) assert.ok(source.includes(label));
  assert.match(source, /clientDeclared: event.clientDeclared/);
});

test('StatsPage shares the same inspector and unwraps the canonical detail envelope', () => {
  const source = readFileSync(STATS_PAGE, 'utf8');
  assert.match(source, /id="analytics-request-drilldown"/);
  assert.match(source, /<EventInspector/);
  assert.match(source, /runtimeEventDetail\?\.event\?\.id/);
});

test('StatsPage Token 展示统一术语并把缓存命中率并入缓存读取', () => {
  // The page owns filters and navigation; the canonical five-board workspace
  // owns the Token labels. Read both sources so this contract follows the
  // actual render boundary instead of forcing hidden copy into StatsPage.
  const source = `${readFileSync(STATS_PAGE, 'utf8')}\n${readFileSync(ANALYTICS_WORKSPACE, 'utf8')}`;
  for (const label of ['输入 Token', '输出 Token', '缓存读取', '缓存读取命中率', '缓存写入']) {
    assert.match(source, new RegExp(label));
  }
  assert.match(source, /命中率.*percent\(/);
  assert.doesNotMatch(source, /命中率（请求）|命中率（Token）|写入占比|缓存写入占比/);
  assert.doesNotMatch(source, /处理总量|处理输入|处理 Token/);
  assert.doesNotMatch(source, /title: ['"]缓存写['"]/);
  assert.doesNotMatch(source, /\bP(?:50|95|99)\b|百分位|percentile/i);
});

test('统计项目点击只局部过滤会话，不改变全局统计筛选', () => {
  const source = readFileSync(ANALYTICS_WORKSPACE, 'utf8');
  assert.match(source, /selectedProject/);
  assert.match(source, /projectID: localProject\.key/);
  assert.match(source, /kind === 'session'/);
  assert.match(source, /\['model', '模型使用情况'\]/);
  assert.match(source, /sessionID: localSession\.key/);
  assert.match(source, /onSessionSelect/);
  assert.match(source, /清除项目选择/);
  assert.match(source, /onRowClick=\{kind === 'project' \? onProjectSelect : kind === 'session' \? onSessionSelect : undefined\}/);
  assert.doesNotMatch(source, /onProjectSelect[\s\S]{0,400}setRuntimeAnalyticsFilters/);
});

test('统计项目局部选择同步顶部清除筛选状态', () => {
  const stats = readFileSync(STATS_PAGE, 'utf8');
  const workspace = readFileSync(ANALYTICS_WORKSPACE, 'utf8');
  assert.match(stats, /selectedProject\?\.key/);
  assert.match(stats, /selectedSession\?\.key/);
  assert.match(stats, /projectSelectionResetSignal/);
  assert.match(stats, /onProjectSelectionChange=\{handleProjectSelectionChange\}/);
  assert.match(stats, /onSessionSelectionChange=\{handleSessionSelectionChange\}/);
  assert.match(workspace, /onProjectSelectionChange\?\.\(next\)/);
  assert.match(workspace, /onSessionSelectionChange\?\.\(next\)/);
  assert.match(workspace, /projectSelectionResetSignal/);
});

test('统计表格内操作按钮不会被行键盘下钻拦截', () => {
  const table = readFileSync(new URL('../src/components/DataTable.jsx', import.meta.url), 'utf8');
  assert.match(table, /event\.target !== event\.currentTarget/);
});

test('DiagnosticsPage 详情内容按需生成且全量导出不经过前端内存', () => {
  const source = readFileSync(DIAGNOSTICS_PAGE, 'utf8');
  assert.match(source, /function RawBlock\(\{ title, value, valueFactory \}\)/);
  assert.doesNotMatch(source, /<details open>/, '原始块不能默认展开并创建大段 DOM');
  assert.doesNotMatch(source, /valueFactory=\{\(\) => JSON\.stringify\(captureDetail/, '详情渲染不能自动编码完整 JSON');
  assert.match(source, /captureDetailMatchesSelection/);
  assert.match(source, /单条导出仅针对当前选中/);
  assert.match(source, /下载全部捕获快照/);
  assert.match(source, /window\.confirm/);
  assert.match(source, /\/admin\/api\/diagnostic-capture\/export/);
  assert.doesNotMatch(source, /fetch\([^)]*diagnostic-capture\/export/, '全量导出不能走 fetch 聚合响应');
  assert.doesNotMatch(source, /selected\s*\|\|\s*capture/, '全量导出不能用选中详情拼接快照');
});
