import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const [
  routingSource,
  providerSource,
  runSource,
  statisticsSource,
  analyticsSource,
  dataTableSource,
  topbarSource,
  shadcnStyles,
] = await Promise.all([
  readFile(new URL('../src/pages/RoutingPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/PrimaryProvidersPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/StatsPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/components/DataTable.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/components/Topbar.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/styles/shadcn.css', import.meta.url), 'utf8'),
]);

test('Routing and Provider switch from desktop tables to task-focused phone cards', () => {
  assert.match(routingSource, /className="[^"]*routing-desktop-table[^"]*hidden md:block/);
  assert.match(routingSource, /className="[^"]*routing-mobile-list[^"]*md:hidden/);
  assert.match(routingSource, /目标 Provider 与模型/);
  assert.match(providerSource, /className="[^"]*provider-desktop-table[^"]*hidden md:block/);
  assert.match(providerSource, /className="[^"]*provider-mobile-list[^"]*md:hidden/);
  assert.match(providerSource, /provider-mobile-primary-actions/);
  assert.match(providerSource, /排序、模型与删除/);
});

test('Run uses readable phone event cards and a bounded sticky desktop table', () => {
  assert.match(runSource, /function MobileEventList/);
  assert.match(runSource, /className="[^"]*telemetry-desktop-table[^"]*hidden md:block/);
  assert.match(runSource, /className="[^"]*telemetry-mobile-list[^"]*md:hidden/);
  assert.match(runSource, /className="[^"]*telemetry-mobile-heading/);
  assert.match(runSource, /className="[^"]*telemetry-mobile-route/);
  assert.match(runSource, /className="[^"]*telemetry-mobile-request-summary/);
  assert.match(runSource, /className="[^"]*telemetry-mobile-status/);
  assert.match(runSource, /eventKindLabel\(event\.kind\)/);
  assert.match(dataTableSource, /<thead className="[^"]*sticky top-0/);
  assert.match(dataTableSource, /scrollHeight && 'overflow-y-auto'/);
  assert.match(runSource, /max-h-\[360px\][\s\S]*?overflow-y-auto/);
});

test('Statistics keeps common filters visible and advanced selections discoverable', () => {
  assert.match(statisticsSource, /<details className="analytics-v3-advanced-filters\b/);
  assert.match(statisticsSource, /advancedFilterLabels\.join/);
  assert.match(statisticsSource, /id="analytics-v3-filter-grid"[\s\S]*?className="analytics-v3-filter-grid\b/);
  assert.match(statisticsSource, /按所选时间范围展示数据/);
  const order = ['>入口</span>', '>项目</span>', '>会话</span>', '>客户端</span>', '>模型</span>', '>最终结果</span>', '>用途</span>', '>失败类型</span>', '>失败阶段</span>'];
  let previous = -1;
  for (const marker of order) {
    const index = statisticsSource.indexOf(marker);
    assert.ok(index > previous, `筛选条件顺序不正确：${marker}`);
    previous = index;
  }
  assert.doesNotMatch(statisticsSource, /FilterTextInput/);
  assert.match(statisticsSource, /models: facetOptions\(runtimeFacets, 'models'/);
  assert.match(statisticsSource, /requestPurposes: facetOptions\(runtimeFacets, 'requestPurposes'/);
  assert.match(statisticsSource, /failureKinds: facetOptions\(runtimeFacets, 'failureKinds'/);
  assert.match(statisticsSource, /failurePhases: facetOptions\(runtimeFacets, 'failurePhases'/);
  assert.doesNotMatch(shadcnStyles, /\.analytics-v3-filter-grid:not\(\.is-open\) > :not\(\.analytics-v3-range\)\s*\{\s*display: none;/);
  assert.match(statisticsSource, /RANGE_OPTIONS\.map/);
  assert.match(analyticsSource, /data-storage-management/);
  assert.match(analyticsSource, /运行统计存储/);
  assert.match(analyticsSource, /<details className="runtime-v2-storage-technical-details\b/);
  assert.match(analyticsSource, /lg:flex-row lg:items-start lg:justify-between/);
  assert.match(analyticsSource, /max-h-\[88dvh\] w-full max-w-3xl/);
});

test('compact topbar stays one row and canonical page labels match navigation', () => {
  assert.match(topbarSource, /<header className="sticky top-0 z-20 flex h-14 shrink-0 items-center justify-between/);
  assert.match(runSource, /<span>运行<\/span>/);
  assert.match(providerSource, /<span>入口库<\/span>/);
  assert.match(statisticsSource, /<span>统计<\/span>/);
});
