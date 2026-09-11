import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

globalThis.window = { location: { search: '?mock=1', assign() {} } };

const { api } = await import('../src/services/api.js');
const {
  RUNTIME_DEFAULT_PAGE_SIZE,
  RUNTIME_ANALYTICS_PAGE_SIZE_STORAGE_KEY,
  normalizeRuntimePagedResult,
  persistRuntimeAnalyticsPageSize,
  readRuntimeAnalyticsPageSize,
  runtimeCostAmount,
  runtimeCacheRate,
  runtimeCacheWriteRate,
  runtimeCacheRequestUnknownRequests,
  runtimePriceMicros,
  runtimeRatePercent,
} = await import('../src/utils/runtimeAnalyticsV2.js');

test('新环境所有可分页统计接口默认每页 10 条', async () => {
  assert.equal(RUNTIME_DEFAULT_PAGE_SIZE, 10);
  assert.equal((await api.getRuntimeErrors()).pageSize, RUNTIME_DEFAULT_PAGE_SIZE);
  assert.equal((await api.getRuntimeDimension('projects')).pageSize, RUNTIME_DEFAULT_PAGE_SIZE);
  assert.equal((await api.getRuntimeDimensions('endpoint')).pageSize, RUNTIME_DEFAULT_PAGE_SIZE);
});

test('按时间清理预览接口返回可确认的计数', async () => {
  const cutoff = Date.now() / 1000 - 978307200 - 86_400;
  const preview = await api.previewRuntimeCleanup({ olderThan: cutoff });
  assert.equal(typeof preview.deletableEvents, 'number');
  assert.equal(typeof preview.deletableRequests, 'number');
  assert.equal(typeof preview.remainingEvents, 'number');
});

test('统计 facets 为所有可选条件提供选项', async () => {
  const facets = await api.getRuntimeFacets('today');
  for (const key of ['clientKinds', 'endpoints', 'projects', 'sessions', 'models', 'requestPurposes', 'failureKinds', 'failurePhases']) {
    assert.ok(Array.isArray(facets.facets[key]), `${key} 应为数组`);
  }
  assert.ok(facets.facets.models.some((item) => item.value === 'claude-opus-5'));
  assert.ok(facets.facets.requestPurposes.some((item) => item.value === 'websearch'));
});

test('v3 分析接口使用稳定快照并支持项目分页', async () => {
  const trends = await api.getRuntimeTrends({ range: '24h' });
  assert.equal(trends.apiVersion, 3);
  assert.ok(trends.points.length > 4);
  assert.equal(runtimeRatePercent(trends.totals.tokens.cacheReadTokenRate), 43);

  const projects = await api.getRuntimeDimension('projects', { page: 2, pageSize: 25 });
  const normalized = normalizeRuntimePagedResult(projects, { page: 2, pageSize: 25 });
  assert.ok(normalized);
  assert.equal(normalized.page, 2);
  assert.equal(normalized.rows.length, 25);
  assert.equal(normalized.totalCount, 64);
  assert.equal(normalized.hasPrevious, true);
  assert.equal(typeof normalized.rows[0].cacheReadTokenRate, 'number');
  assert.equal(typeof normalized.rows[0].cacheReadRequestRate, 'number');

  const lastPage = await api.getRuntimeDimension('projects', { page: 7, pageSize: 10 });
  assert.equal(lastPage.page, 7);
  assert.equal(lastPage.pageSize, 10);
  assert.equal(lastPage.totalPages, 7);
  assert.equal(lastPage.rows.length, 4);
  assert.equal(lastPage.hasNext, false);
});

test('兼容 analytics tokenUsage 也返回可显示的缓存命中率', async () => {
  const analytics = await api.getRuntimeAnalytics('24h', {});
  assert.ok(analytics.tokenUsage);
  assert.equal(analytics.tokenUsage.cacheReadTokenRate, 121000 / 247400);
  assert.equal(analytics.tokenUsage.cacheReadRequestRate, 1);
  assert.equal(analytics.tokenUsage.cacheReadTokenEligibleRequests, 75);
});

test('Linux 缓存命中率兼容旧 tokenUsage，未知分母不伪装成 0%', async () => {
  assert.equal(runtimeCacheRate({
    tokenAccountingSemantics: 'independent',
    cacheReadInputTokens: 24320,
    processedInputTokens: 55065,
  }, 'token'), 24320 / 55065);
  assert.equal(runtimeCacheRate({
    cacheReadInputTokens: 24320,
    processedInputTokens: 55065,
    tokenAccountingSemantics: 'unknown',
  }, 'token'), null);
  assert.equal(runtimeCacheRate({
    cacheReadHitRequests: 3,
    cacheReadReportedRequests: 4,
  }, 'request'), 0.75);
  assert.equal(runtimeCacheRate({
    cacheReadTokenRate: null,
    processedInputTokens: 0,
    cacheReadInputTokens: 0,
  }, 'token'), null);
  assert.equal(runtimeCacheRate({ cacheHitRate: '44.2%' }, 'token'), 0.442);
  assert.equal(runtimeCacheRequestUnknownRequests({ observedRequests: 9, cacheReadReportedRequests: 4 }), 5);
  assert.equal(runtimeCacheRequestUnknownRequests({ observedRequests: 9 }), null);
});

test('缓存写入兼容字段只保留底层读取能力，不参与用户界面指标', () => {
  assert.equal(runtimeCacheWriteRate({
    cacheCreationTokenRate: 0.125,
  }), 0.125);
  assert.equal(runtimeCacheWriteRate({
    tokenAccountingSemantics: 'independent',
    cacheCreationInputTokens: 25,
    processedInputTokens: 200,
  }), null, '没有服务端明确写入分母时不能复用 processedInputTokens');
  assert.equal(runtimeCacheWriteRate({
    tokenAccountingSemantics: 'mixed',
    cacheCreationInputTokens: 25,
    processedInputTokens: 200,
  }), null);
  assert.equal(runtimeCacheWriteRate({
    tokenAccountingSemantics: 'subset',
    cacheCreationInputTokens: 25,
    processedInputTokens: 0,
  }), null);
});

test('事件历史 mock 的项目名称和稳定 ID 都可用于分页筛选', async () => {
  const all = await api.getRuntimeEventPage({ page: 1, pageSize: 25 });
  const sample = all.events.find((event) => event.projectName && event.projectID);
  assert.ok(sample);

  const byName = await api.getRuntimeEventPage({
    page: 1, pageSize: 25, project: sample.projectName,
  });
  assert.ok(byName.totalCount > 0);
  assert.ok(byName.events.every((event) => event.projectName === sample.projectName));

  const byID = await api.getRuntimeEventPage({
    page: 1, pageSize: 25, projectID: sample.projectID,
  });
  assert.ok(byID.totalCount > 0);
  assert.ok(byID.events.every((event) => event.projectID === sample.projectID));
});

test('项目维度 mock 返回客户端归因列表', async () => {
  const page = await api.getRuntimeDimensions('project', { page: 1, pageSize: 10 });
  assert.ok(page.rows.length > 0);
  assert.ok(page.rows.every((row) => Array.isArray(row.clientKinds)));
});

test('mock 维度行使用稳定 ID，点击下钻后仍能得到关联用量', async () => {
  const projects = await api.getRuntimeDimensions('project', { page: 1, pageSize: 10 });
  const project = projects.rows.find((row) => row.name === 'automode-proxy');
  assert.equal(project?.key, 'project-automode-proxy');
  const sessions = await api.getRuntimeDimensions('session', {
    page: 1,
    pageSize: 10,
    filters: { projectID: project.key },
  });
  assert.ok(sessions.totalCount > 0);
  const session = sessions.rows[0];
  assert.equal(session.key, 'session-demo-001');
  const models = await api.getRuntimeDimensions('model', {
    page: 1,
    pageSize: 10,
    filters: { sessionID: session.key },
  });
  assert.ok(models.totalCount > 0);
});

test('模型维度可按项目和会话分别查看 Token 用量', async () => {
  const all = await api.getRuntimeDimensions('model', { pageSize: 25 });
  assert.ok(all.rows.some((row) => row.name === 'claude-opus-5'));
  assert.ok(all.rows.every((row) => Number.isFinite(row.inputTokens)));

  const project = await api.getRuntimeDimensions('model', {
    pageSize: 25,
    filters: { projectID: 'project-unidentified_project' },
  });
  assert.deepEqual(project.rows.map((row) => row.name), ['gpt-4o']);

  const session = await api.getRuntimeDimensions('model', {
    pageSize: 25,
    filters: { sessionID: 'session-demo-001' },
  });
  assert.deepEqual(session.rows.map((row) => row.name), ['claude-opus-5', 'gpt-5.4']);
});

test('错误聚合分页与存储/策略接口不把不支持能力拖垮主面板', async () => {
  const errors = await api.getRuntimeErrors({ page: 1, pageSize: 25 });
  assert.equal(errors.apiVersion, 3);
  assert.ok(errors.groups.length > 0);
  const storage = await api.getRuntimeStorage();
  assert.equal(storage.backend, 'sqlite');
  const retention = await api.getRuntimeRetention();
  assert.equal(retention.maxAgeDays, null, '最大保存天数默认关闭');
  assert.equal(retention.storageLimitBytes, null, '存储上限默认关闭');
  assert.equal(Object.hasOwn(retention, 'maxEvents'), false, '不再输出旧自动清理字段');
  await assert.rejects(
    api.updateRuntimeRetention({
      expectedRevision: retention.revision,
      maxEvents: 10000,
    }),
    (error) => error?.code === 'invalid_json' && error?.status === 400,
  );
  const saved = await api.updateRuntimeRetention({
    expectedRevision: retention.revision,
    maxAgeDays: 30,
    storageLimitBytes: 1024 * 1024 * 1024,
  });
  assert.equal(saved.revision, retention.revision + 1);
  assert.equal(saved.maxAgeDays, 30);
  assert.equal(saved.storageLimitBytes, 1024 * 1024 * 1024);

  const disabledTime = await api.updateRuntimeRetention({
    expectedRevision: saved.revision,
    maxAgeDays: null,
    storageLimitBytes: saved.storageLimitBytes,
  });
  assert.equal(disabledTime.maxAgeDays, null);
  assert.equal(disabledTime.storageLimitBytes, saved.storageLimitBytes);
});

test('导出先估算后触发 attachment 流，stored 需要显式确认', async () => {
  const estimate = await api.estimateRuntimeExport({ scope: 'events', format: 'jsonl', privacy: 'redacted' });
  assert.ok(estimate.rowCount > 0);
  assert.ok(estimate.snapshotSeq > 0);
  const started = await api.downloadRuntimeExport({
    scope: 'events', format: 'jsonl', privacy: 'redacted',
    snapshotSeq: estimate.snapshotSeq, historyGeneration: estimate.historyGeneration,
  });
  assert.equal(started.started, true);
});

test('价格与页大小转换保持整数微货币和本地持久化', () => {
  assert.equal(runtimePriceMicros('15.25'), 15_250_000);
  assert.equal(runtimeCostAmount(15_250_000), 15.25);
  const values = new Map();
  const storage = { getItem: (key) => values.get(key) ?? null, setItem: (key, value) => values.set(key, value) };
  assert.equal(persistRuntimeAnalyticsPageSize(100, storage), 100);
  assert.equal(values.get(RUNTIME_ANALYTICS_PAGE_SIZE_STORAGE_KEY), '100');
  assert.equal(readRuntimeAnalyticsPageSize(storage), 100);
});

test('统计页和 v2 导出不再在前端拼接完整 JSON Blob', async () => {
  const [stats, workspace, apiSource] = await Promise.all([
    readFile(new URL('../src/pages/StatsPage.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/services/api.js', import.meta.url), 'utf8'),
  ]);
  assert.doesNotMatch(stats, /new Blob\(/);
  assert.doesNotMatch(workspace, /new Blob\(/);
  assert.match(workspace, /estimateRuntimeExport/);
  assert.match(apiSource, /document\.createElement\('a'\)/);
  assert.doesNotMatch(apiSource, /window\.location\.assign/);
  assert.match(workspace, /onSearchSubmit/);
  assert.match(workspace, /AbortController/);
  assert.match(workspace, /自动轮换/);
  assert.match(workspace, /滚动 24 小时/);
  assert.match(workspace, /手动清理统计/);
  assert.doesNotMatch(workspace, /停用自动清理/);
  assert.doesNotMatch(workspace, /onClearError/);
  assert.match(workspace, /缓存写入/);
  assert.match(workspace, /data-storage-management/);
  assert.match(workspace, /存储设置…/);
  assert.match(workspace, /运行统计存储设置/);
  assert.match(workspace, /role="dialog"/);
  assert.match(workspace, /runtime-v2-storage-technical-details/);
  assert.match(workspace, /state === 'degraded'/);
  assert.match(workspace, /storage\?\.hourlyRollupFailed === true/);
  assert.match(workspace, /missingIndexes\.length > 0/);
  assert.match(workspace, /重置并新建数据库/);
  assert.match(workspace, /存储上限/);
  assert.match(workspace, /自动轮换最旧的已完成请求/);
  const storageCardStart = workspace.indexOf('function StorageManagementCard');
  const storagePanelStart = workspace.indexOf('function StoragePanel');
  assert.ok(storageCardStart >= 0 && storagePanelStart > storageCardStart);
  const storageCard = workspace.slice(storageCardStart, storagePanelStart);
  assert.match(storageCard, /const effectiveBytes = storage\?\.liveBytes \?\? databaseBytes/);
  assert.match(storageCard, /<small>有效占用<\/small>/);
  assert.match(storageCard, /<small>保留事件<\/small>/);
  assert.match(storageCard, /仅手动清理/);
  assert.doesNotMatch(storageCard, /<small>存储上限<\/small>/);
  assert.doesNotMatch(storageCard, /<small>数据库占用<\/small>/);
  const storagePanel = workspace.slice(storagePanelStart);
  assert.match(storagePanel, /runtime-v2-storage-limit-mb/);
  assert.match(storagePanel, /runtime-v2-storage-max-age-days/);
  assert.match(storagePanel, /最大保存天数/);
  assert.match(storagePanel, /<label htmlFor="runtime-v2-storage-limit-mb">容量上限<\/label>/);
  assert.match(storagePanel, /runtime-v2-storage-dialog-context/);
  assert.match(storagePanel, /轮换不会立即缩小数据库文件/);
  assert.doesNotMatch(workspace, /自动保留 10,000 条事件/);
  assert.match(workspace, /保存保留策略/);
  assert.doesNotMatch(workspace, /命中率（请求）|命中率（Token）|写入占比|缓存写入占比/);
  assert.doesNotMatch(workspace, /缓存写入（缓存命中率）/);
});

test('统计维度排序接受语义字段并保留正反方向', async () => {
  const asc = await api.getRuntimeDimensions('project', {
    page: 1, pageSize: 10, sort: 'success_rate', order: 'asc',
  });
  const desc = await api.getRuntimeDimensions('project', {
    page: 1, pageSize: 10, sort: 'cache_read', order: 'desc',
  });
  assert.equal(asc.sort, 'success_rate');
  assert.equal(asc.order, 'asc');
  assert.equal(desc.sort, 'cache_read');
  assert.equal(desc.order, 'desc');
  assert.equal(asc.rows.length, 10);
  assert.equal(desc.rows.length, 10);
});

test('统计页自动刷新会重新锚定使用明细快照，而不是沿用首个快照', async () => {
  const workspace = await readFile(new URL('../src/components/AnalyticsV2Workspace.jsx', import.meta.url), 'utf8');
  // The dimension loader defaults to the previous page's snapshotSeq so
  // paging/sorting stay consistent. A timer tick must not reuse that anchor,
  // otherwise the usage tables stay pinned while the trend totals move.
  assert.match(workspace, /const refreshDimensions = useCallback/);
  const refreshStart = workspace.indexOf('const refreshDimensions = useCallback');
  const refreshEnd = workspace.indexOf('}, [', refreshStart);
  const refreshBody = workspace.slice(refreshStart, refreshEnd);
  assert.match(refreshBody, /sort: current\.sort, order: current\.order/);
  assert.doesNotMatch(refreshBody, /snapshotSeq/);
  const signalStart = workspace.indexOf('if (!analyticsRefreshSignal || analyticsRefreshSignal === lastAnalyticsRefreshSignal.current) return;');
  const signalEnd = workspace.indexOf('[analyticsRefreshSignal]', signalStart);
  const signalEffect = workspace.slice(signalStart, signalEnd);
  assert.match(signalEffect, /section === 'overview'\) refreshDimensions\(/);
  assert.match(signalEffect, /section === 'tokens'\) refreshDimensions\(/);
  assert.doesNotMatch(signalEffect, /loadDimensions\(/);
});
