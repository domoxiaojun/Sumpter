import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

globalThis.window = { location: { search: '?mock=1' } };

const {
  RUNTIME_EVENT_PAGE_SIZES,
  RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY,
  clampRuntimeEventPage,
  isRuntimeSnapshotError,
  normalizeRuntimeEventPage,
  normalizeRuntimeEventPageSize,
  persistRuntimeEventPageSize,
  readRuntimeEventPageSize,
  runtimeEventPageRange,
  runtimeEventPageWindow,
} = await import('../src/utils/runtimeAnalyticsV2.js');
const { api } = await import('../src/services/api.js');

test('event page size accepts the five wire-contract values and persists safely', () => {
  assert.deepEqual(RUNTIME_EVENT_PAGE_SIZES, [10, 25, 50, 100, 200]);
  assert.equal(normalizeRuntimeEventPageSize(), 10);
  assert.equal(normalizeRuntimeEventPageSize(25), 25);
  assert.equal(normalizeRuntimeEventPageSize('200'), 200);
  assert.equal(normalizeRuntimeEventPageSize(75), 10);

  const values = new Map();
  const storage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
  };
  assert.equal(persistRuntimeEventPageSize(100, storage), 100);
  assert.equal(values.get(RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY), '100');
  assert.equal(readRuntimeEventPageSize(storage), 100);
  values.set(RUNTIME_EVENT_PAGE_SIZE_STORAGE_KEY, '999');
  assert.equal(readRuntimeEventPageSize(storage), 10);
});

test('event page normalization derives bounded navigation without accepting a v1 cursor page', () => {
  assert.equal(normalizeRuntimeEventPage({ events: [], hasMore: false }), null);
  const page = normalizeRuntimeEventPage({
    apiVersion: 3,
    events: Array.from({ length: 25 }, (_, index) => ({ id: `event-${index}` })),
    page: 9,
    pageSize: 25,
    totalCount: 137,
    snapshotSeq: 500,
    historyGeneration: 3,
  });
  assert.equal(page.page, 6);
  assert.equal(page.totalPages, 6);
  assert.equal(page.hasNext, false);
  assert.equal(page.hasPrevious, true);
  assert.deepEqual(runtimeEventPageRange({ ...page, events: page.events.slice(0, 12) }), { from: 126, to: 137 });
  assert.equal(clampRuntimeEventPage(99, 6), 6);
  assert.deepEqual(runtimeEventPageWindow(5, 20), [1, 3, 4, 5, 6, 7, 20]);
});

test('mock v2 events use an exact stable snapshot and invalidate it after reset', async () => {
  const first = await api.getRuntimeEventPage({ page: 1, pageSize: 25, kind: 'client' });
  const normalizedFirst = normalizeRuntimeEventPage(first, { page: 1, pageSize: 25 });
  assert.ok(normalizedFirst);
  assert.equal(normalizedFirst.events.length, 25);
  assert.ok(normalizedFirst.totalCount > 25);
  assert.equal(normalizedFirst.hasPrevious, false);
  assert.equal(normalizedFirst.hasNext, true);
  assert.deepEqual(normalizedFirst.filters, { kind: 'client' });

  const second = await api.getRuntimeEventPage({
    page: 2,
    pageSize: 25,
    kind: 'client',
    snapshotSeq: normalizedFirst.snapshotSeq,
    historyGeneration: normalizedFirst.historyGeneration,
  });
  assert.equal(second.page, 2);
  assert.equal(second.totalCount, normalizedFirst.totalCount);
  assert.equal(second.snapshotSeq, normalizedFirst.snapshotSeq);
  assert.equal(new Set([...normalizedFirst.events, ...second.events].map((event) => event.id)).size, 50);

  const chain = await api.getRuntimeRequestChain(normalizedFirst.events[0].requestID);
  assert.ok(chain.events.length >= 1);
  assert.ok(chain.events.every((event) => event.requestID === normalizedFirst.events[0].requestID));

  await api.resetRuntime();
  await assert.rejects(
    api.getRuntimeEventPage({
      page: 2,
      pageSize: 25,
      kind: 'client',
      snapshotSeq: normalizedFirst.snapshotSeq,
      historyGeneration: normalizedFirst.historyGeneration,
    }),
    (error) => isRuntimeSnapshotError(error) && error.code === 'runtime_snapshot_expired',
  );
});

test('event table keeps pagination outside horizontal scrolling and exposes sortable headers', async () => {
  const [runPageSource, hookSource, tableSource] = await Promise.all([
    readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/hooks/useRuntimeEventPage.js', import.meta.url), 'utf8'),
    readFile(new URL('../src/components/DataTable.jsx', import.meta.url), 'utf8'),
  ]);
  assert.ok(
    runPageSource.indexOf('<PaginationBar') < runPageSource.indexOf('className="event-page-content"'),
    '分页控件应位于事件列表上方',
  );
  assert.match(runPageSource, /<LiveEventList/);
  assert.match(runPageSource, /data=\{visibleEvents\}/);
  assert.match(hookSource, /persistedEvents/);
  assert.match(hookSource, /eventIsInFlight\(event\)/);
  assert.match(hookSource, /autoRefreshTimerRef/);
  assert.match(hookSource, /setReloadGeneration/);
  assert.doesNotMatch(hookSource, /reloadLatest|pendingNewCount/);
  assert.doesNotMatch(runPageSource, /载入最新/);
  assert.doesNotMatch(runPageSource, /containerStyle=\{\{\s*overflowY:/);
  assert.match(tableSource, /aria-sort=\{ariaSort\}/);
  assert.match(tableSource, /data-table-sort-button/);
  assert.match(tableSource, /tabIndex=\{0\}/);
  assert.match(runPageSource, /<PaginationBar[\s\S]*?onPageSizeChange/);
  assert.match(runPageSource, /title: '结果'/);
  assert.match(runPageSource, /title: '说明'/);
  assert.match(runPageSource, /recentEventRequestSummary/);
  assert.match(runPageSource, /代理: \$\{codexAgentRoleLabel\(metadata\)\}/);
  assert.match(runPageSource, /路径: \$\{codexAgentPath\(metadata\)\}/);
  assert.match(runPageSource, /key === 'standard'\) return '主请求'/);
  const paginationSource = await readFile(new URL('../src/components/PaginationBar.jsx', import.meta.url), 'utf8');
  assert.match(paginationSource, /pagination-page-jump/);
  assert.match(paginationSource, /onPageChange\?\.\(target\)/);
  assert.match(runPageSource, /event-page-stage/);
  assert.match(runPageSource, /data-page-direction=\{eventPageDirection\}/);
  assert.match(runPageSource, /event-page-loading/);
  assert.match(paginationSource, /data-loading=\{loading \? 'true' : 'false'\}/);
  assert.match(tableSource, /getRowProps/);
  assert.match(tableSource, /beforeTable = null/);
  assert.match(tableSource, /\{beforeTable\}/);
  assert.match(runPageSource, /className="telemetry-live-mobile"/);
  assert.match(runPageSource, /beforeTable=\{liveEvents\?\.length \? \(/);
});

test('analytics facet tabs stay inside the board on phone widths', async () => {
  const componentStyles = await readFile(
    new URL('../src/styles/components.css', import.meta.url),
    'utf8',
  );
  assert.match(componentStyles, /@media \(max-width: 600px\)[\s\S]*?\.runtime-v2-section-tabs\s*\{[\s\S]*?display: grid;/);
  assert.match(componentStyles, /\.runtime-v2-section-tabs\s*\{[\s\S]*?grid-template-columns: repeat\(6, minmax\(0, 1fr\)\);[\s\S]*?overflow: visible;/);
  assert.match(componentStyles, /\.runtime-v2-section-tabs button:nth-child\(-n \+ 3\)\s*\{\s*grid-column: span 2;/);
  assert.match(componentStyles, /\.runtime-v2-section-tabs button:nth-child\(n \+ 4\)\s*\{\s*grid-column: span 3;/);
  assert.match(componentStyles, /\.runtime-v2-section-tabs button\s*\{[\s\S]*?min-width: 0;[\s\S]*?white-space: normal;/);
});

test('Run page keeps Provider inventory on its dedicated page', async () => {
  const runPageSource = await readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8');
  assert.match(runPageSource, /run-hero-grid-single/);
  assert.doesNotMatch(runPageSource, /Provider 概览/);
  assert.doesNotMatch(runPageSource, /run-provider-list/);
  assert.doesNotMatch(runPageSource, /run-provider-row/);
});

test('Run page places input/output Token cards before the routing counters', async () => {
  const runPageSource = await readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8');
  const input = runPageSource.indexOf('label="输入 Token"');
  const output = runPageSource.indexOf('label="输出 Token"');
  const provider = runPageSource.indexOf('label="Provider 候选"');
  assert.ok(input >= 0 && output > input && provider > output);
  assert.match(runPageSource, /function recentTokenTotals\(events\)/);
  assert.match(runPageSource, /暂无可用用量/);
  for (const label of ['近 5 次成功率', '当前进行中', '近 5 次首响应', '近 5 次平均耗时']) {
    assert.doesNotMatch(runPageSource, new RegExp(label));
  }
  assert.doesNotMatch(runPageSource, /run-pulse-grid/);
});

test('Run page recent-event mobile route matches macOS wording without Provider prefix', async () => {
  const runPageSource = await readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8');
  assert.match(runPageSource, /<small>\{eventEndpoint\(event\)\}<\/small>/);
  assert.doesNotMatch(runPageSource, /<small>Provider · \{eventEndpoint\(event\)\}<\/small>/);
});

test('runtime page motion is directional, compositor-friendly, and reduced-motion safe', async () => {
  const [componentStyles, animationStyles] = await Promise.all([
    readFile(new URL('../src/styles/components.css', import.meta.url), 'utf8'),
    readFile(new URL('../src/styles/animations.css', import.meta.url), 'utf8'),
  ]);
  assert.match(componentStyles, /\.event-page-content[\s\S]*?will-change: opacity, transform/);
  assert.match(componentStyles, /data-page-direction='forward'[\s\S]*?translate3d\(-12px/);
  assert.match(componentStyles, /data-page-direction='backward'[\s\S]*?translate3d\(12px/);
  assert.match(animationStyles, /@keyframes runtime-page-enter-forward/);
  assert.match(animationStyles, /@keyframes runtime-page-enter-backward/);
  assert.match(animationStyles, /prefers-reduced-motion[\s\S]*?event-page-stage-loading\[data-page-direction\]/);
});

test('live events share one visible ambient layer for the in-flight group', async () => {
  const [runPageSource, componentStyles, animationStyles] = await Promise.all([
    readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/styles/components.css', import.meta.url), 'utf8'),
    readFile(new URL('../src/styles/animations.css', import.meta.url), 'utf8'),
  ]);
  assert.match(runPageSource, /<section className="telemetry-live-group"/);
  assert.match(runPageSource, /aria-label=\{`\$\{formatNumber\(events\.length\)\} 个进行中请求`\}/);
  assert.match(runPageSource, /ambient-deco telemetry-live-ambient/);
  const ambientIndex = runPageSource.indexOf('<span className="ambient-deco telemetry-live-ambient"');
  const eventIndex = runPageSource.indexOf('className={`telemetry-live-event');
  assert.ok(ambientIndex >= 0 && ambientIndex < eventIndex, 'ambient layer must be outside the event rows');
  assert.match(runPageSource, /className="ambient-base"/);
  assert.match(runPageSource, /className="ambient-halo"/);
  assert.match(runPageSource, /className="ambient-flow"/);
  assert.match(runPageSource, /function RecentEventRequestCell/);
  assert.match(runPageSource, /function RecentEventRouteCell/);
  assert.match(runPageSource, /function RecentEventResultCell/);
  assert.match(runPageSource, /function RecentEventMessageCell/);
  assert.match(runPageSource, /telemetry-outcome-text/);
  for (const column of ['telemetry-live-request', 'telemetry-live-route', 'telemetry-live-result', 'telemetry-live-message']) {
    assert.match(runPageSource, new RegExp(column));
  }
  assert.doesNotMatch(runPageSource, /telemetry-live-group-heading|telemetry-live-group-status|telemetry-live-group-dot/);
  assert.doesNotMatch(runPageSource, /<svg[\s\S]*?telemetry-live-event-glow|linearGradient|telemetry-live-event-glow-wave/);
  assert.match(componentStyles, /\.telemetry-live-group[\s\S]*?border: 0[\s\S]*?background: transparent/);
  assert.match(componentStyles, /\.telemetry-live-ambient[\s\S]*?pointer-events: none[\s\S]*?mix-blend-mode: normal/);
  assert.match(componentStyles, /\.telemetry-live-ambient \.ambient-base[\s\S]*?border-radius: 42% 58% 52% 48%[\s\S]*?filter: blur\(20px\)[\s\S]*?opacity: 0\.22/);
  assert.match(componentStyles, /\.telemetry-live-ambient \.ambient-halo[\s\S]*?border-radius: 56% 44% 48% 52%[\s\S]*?filter: blur\(26px\)[\s\S]*?opacity: 0\.13/);
  assert.match(componentStyles, /\.telemetry-live-ambient \.ambient-flow[\s\S]*?width: 64%[\s\S]*?filter: blur\(18px\)[\s\S]*?opacity: 0\.26/);
  assert.match(componentStyles, /\.telemetry-live-ambient \.ambient-flow[\s\S]*?linear-gradient\(90deg/);
  assert.match(animationStyles, /@keyframes telemetry-ambient-base[\s\S]*?translate3d\(1%, -0\.4%, 0\) scale\(1\.018\)/);
  assert.match(animationStyles, /@keyframes telemetry-ambient-halo[\s\S]*?translate3d\(-0\.8%, -0\.5%, 0\) scale\(1\.014\)/);
  assert.match(animationStyles, /@keyframes telemetry-ambient-flow[\s\S]*?translate3d\(140%, 0\.5%, 0\) scale\(1\.004\)/);
  assert.match(animationStyles, /@keyframes telemetry-live-status-pulse[\s\S]*?transform: scale\(0\.86\)/);
  assert.match(animationStyles, /prefers-reduced-motion[\s\S]*?ambient-deco\.telemetry-live-ambient/);
  assert.doesNotMatch(componentStyles, /telemetry-live-event-glow|conic-gradient/);
  assert.doesNotMatch(animationStyles, /runtime-live-glow|runtime-spectrum-spin/);
  assert.doesNotMatch(runPageSource, /telemetry-live-strip/);
  assert.doesNotMatch(runPageSource, /流式输出中/);
});
