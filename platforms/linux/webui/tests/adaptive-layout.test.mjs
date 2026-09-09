import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

const [
  routingSource,
  providerSource,
  runSource,
  statisticsSource,
  analyticsSource,
  componentStyles,
] = await Promise.all([
  readFile(new URL('../src/pages/RoutingPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/PrimaryProvidersPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/RunPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/pages/StatsPage.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8'),
  readFile(new URL('../src/styles/components.css', import.meta.url), 'utf8'),
]);

test('Routing and Provider switch from desktop tables to task-focused phone cards', () => {
  assert.match(routingSource, /className="routing-desktop-table"/);
  assert.match(routingSource, /className="routing-mobile-list"/);
  assert.match(routingSource, /responsive-data-card routing-mobile-card/);
  assert.match(routingSource, /目标 Provider 与模型/);
  assert.match(providerSource, /className="provider-desktop-table"/);
  assert.match(providerSource, /className="provider-mobile-list"/);
  assert.match(providerSource, /provider-mobile-primary-actions/);
  assert.match(providerSource, /排序、模型与删除/);
  assert.match(componentStyles, /@media \(max-width: 600px\)[\s\S]*?\.routing-desktop-table,[\s\S]*?\.provider-desktop-table,[\s\S]*?\.telemetry-desktop-table\s*\{\s*display: none;/);
  assert.match(componentStyles, /\.routing-mobile-list,[\s\S]*?\.provider-mobile-list,[\s\S]*?\.responsive-data-card-list\.telemetry-mobile-list\s*\{[\s\S]*?display: flex;/);
});

test('Run uses readable phone event cards and a bounded sticky desktop table', () => {
  assert.match(runSource, /function MobileEventList/);
  assert.match(runSource, /className="telemetry-desktop-table"/);
  assert.match(runSource, /className="responsive-data-card-list telemetry-mobile-list"/);
  assert.match(runSource, /className="telemetry-mobile-heading"/);
  assert.match(runSource, /className="telemetry-mobile-route"/);
  assert.match(runSource, /className="telemetry-mobile-request-summary"/);
  assert.match(runSource, /className="telemetry-mobile-status"/);
  assert.match(runSource, /eventKindLabel\(event\.kind\)/);
  assert.match(componentStyles, /\.responsive-data-card-list\.telemetry-mobile-list\s*\{\s*display: none;/);
  assert.match(componentStyles, /@media \(min-width: 601px\)[\s\S]*?\.table-container\.telemetry-table\s*\{[\s\S]*?overflow-y: auto;/);
  assert.match(componentStyles, /\.telemetry-table \.data-table thead\s*\{[\s\S]*?position: sticky;/);
});

test('Statistics keeps common filters visible and advanced selections discoverable', () => {
  assert.match(statisticsSource, /<details className="analytics-v3-advanced-filters">/);
  assert.match(statisticsSource, /advancedFilterLabels.join/);
  assert.match(statisticsSource, /id="analytics-v3-filter-grid" className="analytics-v3-filter-grid"/);
  assert.match(statisticsSource, /默认显示全部数据/);
  const order = ['<span>入口</span>', '<span>项目</span>', '<span>会话</span>', '<span>客户端</span>', '<span>模型</span>', '<span>最终结果</span>', '<span>用途</span>', '<span>失败类型</span>', '<span>失败阶段</span>'];
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
  assert.match(componentStyles, /\.analytics-v3-filter-grid\s*\{[\s\S]*?padding: 0;/);
  assert.doesNotMatch(componentStyles, /\.analytics-v3-filter-grid:not\(\.is-open\) > :not\(\.analytics-v3-range\)\s*\{\s*display: none;/);
  assert.match(componentStyles, /\.analytics-v3-segmented\s*\{[\s\S]*?grid-template-columns: repeat\(4, minmax\(0, 1fr\)\);/);
  assert.match(analyticsSource, /data-storage-management/);
  assert.match(analyticsSource, /运行统计存储/);
  assert.match(analyticsSource, /<details className="runtime-v2-storage-technical-details">/);
  assert.match(componentStyles, /\.runtime-v2-storage-management\s*\{[\s\S]*?grid-template-columns: auto minmax\(0, 1fr\) auto;/);
  assert.match(componentStyles, /\.modal-dialog\.runtime-v2-storage-dialog\s*\{[\s\S]*?max-width: min\(760px, 100%\);/);
  assert.match(componentStyles, /@media \(max-width: 700px\)[\s\S]*?\.runtime-v2-storage-management\s*\{[\s\S]*?grid-template-columns: auto minmax\(0, 1fr\);/);
});

test('compact topbar stays one row and canonical page labels match navigation', () => {
  assert.match(componentStyles, /@media \(max-width: 1199px\)[\s\S]*?\.glass-topbar\s*\{[\s\S]*?height: 64px;[\s\S]*?flex-wrap: nowrap;/);
  assert.match(componentStyles, /@media \(max-width: 900px\)[\s\S]*?\.glass-topbar\s*\{[\s\S]*?padding-left: 64px;/);
  assert.match(runSource, /<span>运行<\/span>/);
  assert.match(providerSource, /<span>入口库<\/span>/);
  assert.match(statisticsSource, /<span>统计<\/span>/);
});
