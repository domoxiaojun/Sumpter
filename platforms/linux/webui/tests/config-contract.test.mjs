import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

globalThis.window = { location: { search: '?mock=1' } };
const { mockConfig } = await import('../src/services/mockData.js');
const {
  api,
  fromWireConfig,
  toWireConfig,
} = await import('../src/services/api.js');
const {
  ENDPOINT_PROTOCOL_MODES,
  endpointProtocolLabel,
  isEndpointProtocol,
  isSourceFormat,
} = await import('../src/utils/protocols.js');
const {
  clientKindLabel,
  eventClientKindLabel,
  eventClientKindState,
  eventPurposeLabel,
  eventResultKind,
  eventHTTPStatusCode,
  eventStatus,
  eventHttpStatusLabel,
  eventOutcomeLabel,
  eventPhaseLabel,
  eventProtocolRouteLabel,
  eventStatusDetailLabel,
  eventFailureSummaryLabel,
  failureKindLabel,
  eventStreamTraceLabel,
  eventToolCallsLabel,
  eventUpstreamStatusLabel,
  eventDurationText,
  friendlyEventMessage,
  formatStreamTrace,
  statusKind,
  eventEndpoint,
  getRequestChain,
  codexMetadataSummary,
  codexMetadataJSON,
  eventCodexMetadata,
  eventGrokMetadata,
  grokMetadataSummary,
  grokMetadataJSON,
  grokMetadataField,
  codexHasRequestIdentity,
  codexAgentPath,
  codexAgentRoleLabel,
  codexWorkspaceEntries,
  codexWorkspaceSummary,
  normalizeTimestampMS,
  eventDurationMS,
  cacheHitRateValue,
  formatTokenCount,
} = await import('../src/utils/helpers.js');
const {
  aggregateEvents,
  codexDimensionValues,
  codexRows,
  protocolRouteRows,
} = await import('../src/utils/eventAnalytics.js');
const {
  orderRuntimeEvents,
  trimRuntimeEvents,
  upsertRuntimeEvent,
} = await import('../src/utils/runtimeEvents.js');
const { fetchRuntimeChanges } = await import('../src/utils/runtimeSync.js');
const providerEditorSource = await readFile(
  new URL('../src/pages/PrimaryProvidersPage.jsx', import.meta.url),
  'utf8',
);
const apiServiceSource = await readFile(
  new URL('../src/services/api.js', import.meta.url),
  'utf8',
);
const componentStyles = await readFile(
  new URL('../src/styles/components.css', import.meta.url),
  'utf8',
);

test('mock routing config mirrors all built-in Claude Code rules', () => {
  assert.deepEqual(
    mockConfig.featureRules.slice(0, 3).map((rule) => rule.match?.requestKind),
    ['websearch', 'webfetch', 'classifier'],
  );
});

test('v6 endpoint protocol contract exposes four modes and defaults mock entries to Auto', () => {
  assert.equal(mockConfig.schemaVersion, 6);
  assert.equal('inboundDialectPassthrough' in mockConfig.listener, false);
  assert.deepEqual(ENDPOINT_PROTOCOL_MODES.map((item) => item.value), ['auto', 'anthropic', 'openai', 'openai-responses']);
  assert.equal(mockConfig.endpoints[1].protocol, 'auto');
  for (const value of ENDPOINT_PROTOCOL_MODES.map((item) => item.value)) assert.equal(isEndpointProtocol(value), true);
  assert.equal(isSourceFormat('auto'), false);
  assert.equal(endpointProtocolLabel('auto'), '自动（三协议）');
});

test('Provider 新入口默认启用连接复用，编辑旧入口保留原值', () => {
  assert.match(
    providerEditorSource,
    /let keepAlive = isNew \? true : endpoint\?\.keepAlive === true;/,
  );
  assert.match(providerEditorSource, /新入口默认启用；可按入口关闭/);
});

test('Provider editor stores catalog mappings without a compatibility transform', () => {
  assert.match(providerEditorSource, /const upstream = readableCatalogModel\(raw\);/);
  assert.match(providerEditorSource, /to: upstream,/);
});

test('Linux Provider 入口拖拽覆盖整行且不劫持原生控件', () => {
  assert.match(providerEditorSource, /draggable: !disabled/);
  assert.match(providerEditorSource, /event\.target\.closest\?\.(?:\('button,input,select,textarea,a|\(PROVIDER_DRAG_INTERACTIVE_SELECTOR\))/);
  assert.match(providerEditorSource, /onDragOver: \(event\) =>/);
  assert.match(providerEditorSource, /onDragLeave: \(event\) =>/);
  assert.match(providerEditorSource, /getRowProps=\{providerRowProps\}/);
  assert.match(providerEditorSource, /createProviderDragImage/);
  assert.match(providerEditorSource, /拖动整行可调整入口顺序/);
  assert.match(providerEditorSource, /编辑此入口价格/);
  assert.match(providerEditorSource, /className="provider-reorder-cell"/);
  assert.match(componentStyles, /\.provider-reorder-cell\s*\{[\s\S]*?display:\s*flex;[\s\S]*?align-items:\s*center;/);
});

test('Provider 每行在编辑后提供入口级获取模型动作', () => {
  const actionStart = providerEditorSource.indexOf("title: '操作'");
  const actionEnd = providerEditorSource.indexOf('\n    },\n  ];', actionStart);
  assert.ok(actionStart >= 0 && actionEnd > actionStart, 'Provider 操作列必须存在');
  const actionSource = providerEditorSource.slice(actionStart, actionEnd);
  const editIndex = actionSource.indexOf('openEndpointEditor(row)');
  const fetchIndex = actionSource.indexOf('void handleFetchModels(row)');
  assert.ok(editIndex >= 0, '每行必须保留编辑入口动作');
  assert.ok(fetchIndex > editIndex, '获取模型必须紧跟在编辑动作之后');
  assert.match(actionSource, /type: 'action'/);
  assert.match(actionSource, /fetchingModelEndpointIDs\.has\(row\.id\)/);
  assert.match(actionSource, /读取中…/);
  assert.match(actionSource, /stopPropagation\(\)/);
});

test('DataTable 不用通用 display 覆盖渲染器布局，Provider 操作栏保持单行', () => {
  assert.doesNotMatch(
    componentStyles,
    /\.data-table td:not\([^\n]+\) > \*\s*\{[\s\S]*?-webkit-box/,
    '表格不能把所有直接子元素强制改成 -webkit-box',
  );
  assert.match(componentStyles, /:where\(\.data-table td > \*\)\s*\{/);
  assert.match(componentStyles, /\.provider-row-actions\s*\{[\s\S]*?flex-wrap: nowrap;[\s\S]*?max-width: none;/);
});

test('Grok Build client kind uses its product label', () => {
  assert.equal(clientKindLabel('grok_build'), 'Grok Build');
});

test('client attribution separates explicit unknown, legacy missing, and non-applicable events', () => {
  const explicitUnknown = { kind: 'client', clientKind: 'unknown' };
  const legacyClient = { kind: 'client', statusCode: 200 };
  const legacyUpstream = { kind: 'upstream', statusCode: 200 };
  const notification = { kind: 'notify', statusCode: 200 };
  assert.equal(eventClientKindState(explicitUnknown), 'explicit');
  assert.equal(eventClientKindLabel(explicitUnknown), '未知客户端');
  assert.equal(eventClientKindState(legacyClient), 'missing');
  assert.equal(eventClientKindLabel(legacyClient), '旧事件（未记录）');
  assert.equal(eventClientKindState(legacyUpstream), 'missing');
  assert.equal(eventClientKindLabel(legacyUpstream), '旧事件（未记录）');
  assert.equal(eventClientKindState(notification), 'not_applicable');
  assert.equal(eventClientKindLabel(notification), '不适用（通知事件）');
  assert.equal(eventResultKind(notification), null);
  assert.equal(eventStatus(notification), '不适用（通知事件）');
  assert.equal(eventHttpStatusLabel(notification), '不适用（通知事件）');
  assert.equal(eventOutcomeLabel(notification), '不适用（通知事件）');
  assert.equal(eventPhaseLabel(notification), '不适用（通知事件）');
  assert.equal(eventStatusDetailLabel(notification), '不适用（通知事件）');
  assert.equal(friendlyEventMessage(notification), '通知事件（不参与请求成败统计）');
  assert.equal(statusKind(notification), 'muted');
  assert.deepEqual(aggregateEvents([notification], () => 'all'), []);
  const misleadingNotification = { ...notification, outcome: 'succeeded', phase: 'completed' };
  assert.equal(eventResultKind(misleadingNotification), null);
  assert.equal(eventOutcomeLabel(misleadingNotification), '不适用（通知事件）');
});

test('request purpose separates explicit values, legacy missing, and notifications', () => {
  assert.equal(eventPurposeLabel({ kind: 'client', requestPurpose: 'websearch' }), 'WebSearch 搜索');
  assert.equal(eventPurposeLabel({ kind: 'client' }), '旧事件（未记录）');
  assert.equal(eventPurposeLabel({ kind: 'notify' }), '不适用（通知事件）');
});

test('legacy result fallback matches core status classification', () => {
  assert.equal(eventResultKind({ statusCode: 204 }), 'succeeded');
  assert.equal(eventResultKind({ statusCode: 302 }), 'succeeded');
  assert.equal(eventResultKind({ statusCode: 404 }), 'failed');
  assert.equal(eventResultKind({ statusCode: 499 }), 'cancelled');
  assert.equal(eventResultKind({ statusCode: 200, outcome: 'failed' }), 'failed');
  assert.equal(eventResultKind({ statusCode: 200, phase: 'inFlight' }), null);
  assert.equal(statusKind({ statusCode: 404 }), 'critical');
  assert.equal(eventOutcomeLabel({ statusCode: 204 }), '成功（旧事件推断）');
  assert.equal(eventOutcomeLabel({ statusCode: 404 }), '失败（旧事件推断）');
  assert.equal(eventOutcomeLabel({ statusCode: 499 }), '已取消（旧事件推断）');
  assert.equal(friendlyEventMessage({ statusCode: 204 }), '旧事件：按 HTTP 状态推断为成功');
  assert.equal(friendlyEventMessage({ statusCode: 404 }), '旧事件：按 HTTP 状态推断为失败');
});

test('legacy completed rows fall back to recorded upstream HTTP status consistently', () => {
  const succeeded = {
    kind: 'client', statusCode: 0, upstreamStatusCode: 200, phase: 'completed',
  };
  const failed = {
    kind: 'client', statusCode: 0, upstreamStatusCode: 503, phase: 'completed',
  };
  const cancelled = {
    kind: 'client', statusCode: 0, upstreamStatusCode: 499, phase: 'completed',
  };
  assert.equal(eventHTTPStatusCode(succeeded), 200);
  assert.equal(eventResultKind(succeeded), 'succeeded');
  assert.equal(eventOutcomeLabel(succeeded), '成功（最终结果未上报）');
  assert.equal(statusKind(succeeded), 'good');
  assert.equal(friendlyEventMessage(succeeded), '最终结果未上报（HTTP 状态仅供参考）');
  assert.equal(eventResultKind(failed), 'failed');
  assert.equal(eventOutcomeLabel(failed), '失败（最终结果未上报）');
  assert.equal(statusKind(failed), 'critical');
  assert.equal(friendlyEventMessage(failed), '最终结果未上报（HTTP 状态仅供参考）');
  assert.equal(eventResultKind(cancelled), 'cancelled');
  assert.equal(eventOutcomeLabel(cancelled), '已取消（最终结果未上报）');
  assert.equal(statusKind(cancelled), 'muted');
  assert.equal(friendlyEventMessage(cancelled), '最终结果未上报（HTTP 499 仅供参考）');
});

test('routing tokens use recorded outcome and stream trace for the user-facing summary', () => {
  assert.equal(
    friendlyEventMessage({
      kind: 'client', statusCode: 200, phase: 'inFlight', outcome: null,
      message: 'passthrough responses',
    }),
    '流式输出中',
  );
  assert.equal(
    friendlyEventMessage({
      kind: 'client', statusCode: 200, phase: 'completed', outcome: 'succeeded',
      message: 'passthrough responses',
    }),
    '请求成功',
  );
  assert.equal(
    friendlyEventMessage({
      kind: 'client', statusCode: 200, phase: 'completed', outcome: 'succeeded',
      message: 'passthrough responses', streamTrace: { terminalEvent: '[DONE]' },
    }),
    '流式输出完成',
  );
  assert.equal(
    friendlyEventMessage({
      kind: 'client', statusCode: 200, phase: 'completed', outcome: 'failed',
      message: 'passthrough responses', failureKind: 'upstream_response_failed',
    }),
    '上游响应协议失败',
  );
  assert.equal(
    friendlyEventMessage({
      kind: 'client', statusCode: 200, phase: 'completed', outcome: 'succeeded',
      message: 'bridge openai-responses; 命中路由规则',
      streamTrace: { terminalEvent: 'response.completed' },
    }),
    '流式输出完成',
  );
});

test('technical labels distinguish recorded facts from legacy or pre-route gaps', () => {
  assert.equal(failureKindLabel('client_request_rejected'), '客户端请求被代理拒绝');
  assert.equal(
    eventProtocolRouteLabel({ kind: 'client', statusCode: 401, phase: 'completed', outcome: 'failed' }),
    '未记录（路由前拒绝）',
  );
  assert.equal(
    eventProtocolRouteLabel({ kind: 'client', statusCode: 200 }),
    '未记录（旧事件）',
  );
  assert.equal(
    eventProtocolRouteLabel({ kind: 'client', sourceFormat: 'openai-responses', phase: 'completed', outcome: 'failed' }),
    'OpenAI Responses → 未记录 · 未记录',
  );
  assert.equal(eventFailureSummaryLabel({ kind: 'client', statusCode: 200 }), '不适用（请求成功）');
  assert.equal(eventFailureSummaryLabel({ kind: 'client', statusCode: 500 }), '未记录（旧事件 / 非结构化失败）');
  assert.equal(eventToolCallsLabel({ kind: 'client', statusCode: 200 }), '未记录（旧事件）');
  assert.equal(eventStreamTraceLabel({ kind: 'client', statusCode: 200 }), '未记录（旧事件）');
  assert.equal(eventUpstreamStatusLabel({ kind: 'upstream', statusCode: 200 }), 'HTTP 200（旧事件推断）');
});

test('runtime SSE updates replace an event in place instead of duplicating it', () => {
  const initial = [
    { id: 'newer', kind: 'client', phase: null, statusCode: 200 },
    { id: 'stream', kind: 'client', phase: 'inFlight', statusCode: 0 },
  ];
  const updated = upsertRuntimeEvent(initial, {
    id: 'stream', kind: 'client', phase: null, statusCode: 200, outcome: 'failed',
  });
  assert.equal(updated.length, 2);
  assert.equal(updated[1].id, 'stream');
  assert.equal(updated[1].outcome, 'failed');
});

test('runtime timestamps normalize Apple seconds, Unix seconds, and Unix milliseconds once', () => {
  const expected = Date.UTC(2026, 7, 23, 6, 17, 37);
  const unixSeconds = expected / 1000;
  const appleSeconds = unixSeconds - 978307200;
  assert.equal(normalizeTimestampMS(appleSeconds), expected);
  assert.equal(normalizeTimestampMS(unixSeconds), expected);
  assert.equal(normalizeTimestampMS(expected), expected);
  assert.equal(normalizeTimestampMS(String(unixSeconds)), expected);
});

test('request-chain sorting normalizes mixed timestamp dialects', () => {
  const expected = Date.UTC(2026, 7, 23, 6, 17, 37);
  const requestID = 'req-mixed-timestamps';
  const events = [
    { id: 'unix-seconds', requestID, timestamp: expected / 1000 - 1 },
    { id: 'apple-seconds', requestID, timestamp: expected / 1000 - 978307200 + 2 },
    { id: 'unix-milliseconds', requestID, timestamp: expected },
  ];
  assert.deepEqual(
    getRequestChain(events, 'apple-seconds').map((event) => event.id),
    ['apple-seconds', 'unix-milliseconds', 'unix-seconds'],
  );
});

test('in-flight event duration is local and completed duration stays server-owned', () => {
  const now = Date.now();
  const inFlight = { phase: 'inFlight', timestamp: now - 2100 };
  assert.ok(eventDurationMS(inFlight) >= 2000);
  assert.equal(eventDurationMS({ phase: 'completed', timestamp: now - 2100, durationMS: 37 }), 37);
});

test('runtime reconnect follows every change page without skipping updates', async () => {
  const calls = [];
  const pages = new Map([
    [10, { events: [{ id: 'a', seq: 7, changeSeq: 11 }, { id: 'b', seq: 8, changeSeq: 12 }], hasMore: true, resetGeneration: 4, cursorValid: true }],
    [12, { events: [{ id: 'a', seq: 7, changeSeq: 13 }], hasMore: false, resetGeneration: 4, cursorValid: true }],
  ]);
  const result = await fetchRuntimeChanges({
    afterChangeSeq: 10,
    resetGeneration: 4,
    fetchPage: async ({ afterChangeSeq }) => {
      calls.push(afterChangeSeq);
      return pages.get(afterChangeSeq);
    },
  });
  assert.equal(result.valid, true);
  assert.deepEqual(calls, [10, 12]);
  assert.deepEqual(result.changes.map((item) => item.changeSeq), [11, 12, 13]);
  assert.equal(result.lastChangeSeq, 13);
});

test('runtime reconnect requests a newest-page resync for invalid or stalled cursors', async () => {
  const invalid = await fetchRuntimeChanges({
    afterChangeSeq: 20,
    resetGeneration: 1,
    fetchPage: async () => ({ events: [], hasMore: false, resetGeneration: 2, cursorValid: true }),
  });
  assert.equal(invalid.valid, false);

  const stalled = await fetchRuntimeChanges({
    afterChangeSeq: 20,
    resetGeneration: 1,
    fetchPage: async () => ({ events: [], hasMore: true, resetGeneration: 1, cursorValid: true }),
  });
  assert.equal(stalled.valid, false);
});

test('runtime event retention keeps independent completed and in-flight quotas per kind', () => {
  const events = [
    { id: 'ci-1', kind: 'client', phase: 'inFlight' },
    { id: 'ci-2', kind: 'client', phase: 'inFlight' },
    { id: 'cc-1', kind: 'client' },
    { id: 'cc-2', kind: 'client' },
    { id: 'ui-1', kind: 'upstream', phase: 'inFlight' },
    { id: 'uc-1', kind: 'upstream' },
  ];
  assert.deepEqual(
    trimRuntimeEvents(events, 1).map((event) => event.id),
    ['ci-1', 'cc-1', 'ui-1', 'uc-1'],
  );
});

test('runtime event completion stays visible until a new event re-applies quotas', () => {
  const initial = Array.from({ length: 2 }, (_, index) => ({
    id: `live-${index}`, kind: 'client', phase: 'inFlight', statusCode: 0,
  }));
  const afterFirst = upsertRuntimeEvent(initial, {
    id: 'live-0', kind: 'client', phase: 'completed', statusCode: 200, outcome: 'succeeded',
  }, 1);
  assert.deepEqual(afterFirst.map((event) => event.id), ['live-0', 'live-1']);
  const completed = upsertRuntimeEvent(afterFirst, {
    id: 'live-1', kind: 'client', phase: 'completed', statusCode: 200, outcome: 'succeeded',
  }, 1);
  assert.deepEqual(completed.map((event) => event.id), ['live-0', 'live-1']);
  const afterInsert = upsertRuntimeEvent(completed, {
    id: 'fresh', kind: 'client', phase: 'completed', statusCode: 200, outcome: 'succeeded',
  }, 1);
  assert.deepEqual(afterInsert.map((event) => event.id), ['fresh']);
  assert.equal(eventStatus({ phase: 'in_flight', status_code: 200 }), 'HTTP 200 · 传输中');
  assert.equal(eventEndpoint({ endpoint_name: 'legacy', endpoint_id: 'ep-old', upstream_host: 'legacy.test' }), 'legacy @ legacy.test (ep-old)');
});

test('a long-running request keeps its terminal failure details after completion', () => {
  const completed = Array.from({ length: 200 }, (_, index) => ({
    id: `done-${index}`,
    kind: 'client',
    phase: 'completed',
    statusCode: 200,
    outcome: 'succeeded',
    timestamp: 1_000 - index,
  }));
  const initial = [
    ...completed,
    { id: 'long-live', kind: 'client', phase: 'inFlight', statusCode: 200, timestamp: 1 },
  ];
  const updated = upsertRuntimeEvent(initial, {
    id: 'long-live',
    kind: 'client',
    phase: 'completed',
    statusCode: 200,
    outcome: 'failed',
    failureKind: 'upstream_response_failed',
    toolCalls: ['functions.exec_command'],
    timestamp: 2_000,
  });
  assert.equal(updated.length, 201);
  assert.equal(updated.at(-1).id, 'long-live');
  assert.equal(updated.at(-1).outcome, 'failed');
  assert.deepEqual(updated.at(-1).toolCalls, ['functions.exec_command']);
});

test('runtime events are presented by final timestamp with stable ties', () => {
  const events = [
    { id: 'older', timestamp: 10 },
    { id: 'newer-a', timestamp: 20 },
    { id: 'newer-b', timestamp: 20 },
    { id: 'invalid', timestamp: null },
  ];
  assert.deepEqual(
    orderRuntimeEvents(events).map((event) => event.id),
    ['newer-a', 'newer-b', 'older', 'invalid'],
  );
  assert.deepEqual(
    orderRuntimeEvents(events, 'asc').map((event) => event.id),
    ['invalid', 'older', 'newer-a', 'newer-b'],
  );
});

test('stream trace includes terminal state and completed idle time', () => {
  assert.match(
    formatStreamTrace({ chunkCount: 3, lastChunkAtMS: 120, terminalEvent: null }, { durationMS: 500 }),
    /结束前空闲: 380ms · 未观察到协议终止$/,
  );
  assert.match(
    formatStreamTrace({ chunkCount: 1 }, { inFlight: true }),
    /等待协议终止$/,
  );
});

test('Codex metadata summary and full JSON tolerate legacy wire forms', () => {
  const event = { id: 'codex', codexMetadata: { isSubagent: true, subagentKind: 'thread_spawn', agentName: '/root/worker', threadID: 'thr-1', parentThreadID: 'parent-1', turnID: 'turn-2', requestKind: 'turn', workspaces: { '/w': { kind: 'local' } } } };
  assert.equal(codexMetadataSummary(event), '子代理 thread_spawn · 代理路径 /root/worker · 请求 turn · 线程 thr-1 · 回合 turn-2');
  assert.equal(codexAgentRoleLabel(event), '子代理 · thread_spawn');
  assert.equal(codexAgentPath(event), '/root/worker');
  assert.equal(eventCodexMetadata({ codex_metadata: { thread_id: 'legacy' } }).thread_id, 'legacy');
  assert.match(codexMetadataJSON(event), /"workspaces"/);
  assert.doesNotThrow(() => codexMetadataSummary({ id: 'old' }));
  assert.equal(codexMetadataSummary({ id: 'old', codex_metadata: { thread_id: 'legacy' } }), '主代理 · 线程 legacy');
});

test('Grok metadata summary exposes sampling headers and empty Codex OTel is not identity', () => {
  const event = {
    id: 'grok',
    clientKind: 'grok_build',
    grokMetadata: {
      sessionID: 'sess-1',
      convID: 'conv-1',
      clientIdentifier: 'grok-shell',
      clientVersion: '0.2.119',
      clientMode: 'interactive',
    },
    codexMetadata: { sources: ['headers'], redactedFields: ['traceparent'] },
  };
  assert.equal(eventGrokMetadata(event).sessionID, 'sess-1');
  assert.equal(grokMetadataField(eventGrokMetadata(event), 'convID', 'conv_id'), 'conv-1');
  assert.equal(
    grokMetadataSummary(event),
    'grok-shell · 0.2.119 · interactive · 会话 sess-1 · 对话 conv-1',
  );
  assert.match(grokMetadataJSON(event), /"sessionID"/);
  assert.equal(codexHasRequestIdentity(event.codexMetadata), false);
  assert.equal(codexHasRequestIdentity({ threadID: 'thr-1' }), true);
});

test('Codex workspace context prefers local project names and keeps remote as secondary metadata', () => {
  const metadata = {
    agentName: '/root',
    workspaces: {
      '/workspace/automode-proxy': {
        associatedRemoteURLs: { origin: 'https://github.com/example/sumpter.git' },
        latestGitCommitHash: '1234567890abcdef',
        hasChanges: true,
      },
      '/workspace/local-only': { hasChanges: false },
    },
  };
  assert.deepEqual(codexWorkspaceEntries(metadata), [
    {
      path: '/workspace/automode-proxy',
      projectName: 'automode-proxy',
      status: '有未提交改动',
      commit: '12345678',
      remote: 'github.com/example/sumpter',
    },
    {
      path: '/workspace/local-only',
      projectName: 'local-only',
      status: '工作区干净',
      commit: '',
      remote: '',
    },
  ]);
  assert.equal(codexWorkspaceSummary(metadata), 'automode-proxy · 有未提交改动 · 提交 12345678；local-only · 工作区干净');
  assert.equal(codexAgentRoleLabel(metadata), '主代理');
});

test('runtime event UI keeps HTTP 200 separate from a failed final outcome', () => {
  const event = {
    id: 'failed-after-200', requestID: 'req-1', kind: 'client',
    statusCode: 200, phase: 'completed', outcome: 'failed',
    failureKind: 'stream_interrupted', failurePhase: 'response_stream',
  };
  assert.equal(eventStatus(event), 'HTTP 200 · 失败');
  assert.equal(eventOutcomeLabel(event), '请求失败 (Failed)');
  assert.equal(eventStatusDetailLabel(event), 'HTTP 200 · 已完成 (Completed)');
  assert.equal(statusKind(event), 'critical');
});

test('runtime event status separates pending outcome from the in-flight phase', () => {
  const event = { statusCode: 200, phase: 'inFlight' };
  assert.equal(eventResultKind(event), null);
  assert.equal(eventOutcomeLabel(event), '待定 (Pending)');
  assert.equal(eventStatusDetailLabel(event), 'HTTP 200 · 进行中 (In Flight)');
  assert.equal(eventDurationText({ ...event, ttfbMS: 412 }), 'TTFB 412ms');
});

test('legacy in-flight rows use recorded upstream HTTP status instead of showing no headers', () => {
  const event = { kind: 'client', statusCode: 0, upstreamStatusCode: 200, phase: 'inFlight' };
  assert.equal(eventStatus(event), 'HTTP 200 · 传输中');
  assert.equal(eventHttpStatusLabel(event), 'HTTP 200');
  assert.equal(eventStatusDetailLabel(event), 'HTTP 200 · 进行中 (In Flight)');
});

test('in-flight duration is excluded from analytics averages', () => {
  const rows = aggregateEvents([
    { id: 'live', kind: 'client', statusCode: 200, phase: 'inFlight', ttfbMS: 8_000, durationMS: 10_000 },
    { id: 'done', kind: 'client', statusCode: 200, phase: 'completed', outcome: 'succeeded', ttfbMS: 400, durationMS: 1_200 },
  ], () => 'all');
  assert.equal(rows[0].avgTTFB, 400);
  assert.equal(rows[0].avgDuration, 1_200);
});

test('event analytics keeps HTTP 200 in-flight rows pending', () => {
  const row = aggregateEvents([{ id: 'live', statusCode: 200, phase: 'inFlight' }], () => 'all')[0];
  assert.equal(row.successes, 0);
  assert.equal(row.pending, 1);
  assert.equal(row.successRate, 0);
});

test('protocol route aggregation is a client-only secondary diagnostic', () => {
  const rows = protocolRouteRows([
    { id: 'native', kind: 'client', sourceFormat: 'openai-responses', targetFormat: 'openai-responses', routeMode: 'native', outcome: 'succeeded' },
    { id: 'translated', kind: 'client', source_format: 'openai-responses', target_format: 'anthropic', route_mode: 'translated', outcome: 'failed' },
    { id: 'attempt', kind: 'upstream', sourceFormat: 'openai-responses', targetFormat: 'anthropic', routeMode: 'translated' },
    { id: 'notify', kind: 'notify', statusCode: 200 },
  ]);
  assert.deepEqual(rows.map((row) => row.name), [
    'OpenAI Responses → Anthropic Messages · 桥接',
    'OpenAI Responses → OpenAI Responses · 原生',
  ]);
  assert.equal(rows.reduce((sum, row) => sum + row.attempts, 0), 2);
});

test('runtime event phase treats missing legacy phase as completed', () => {
  assert.equal(
    eventStatusDetailLabel({ statusCode: 200 }),
    'HTTP 200 · 已结束（旧事件未记录阶段）',
  );
  assert.equal(
    eventStatusDetailLabel({ statusCode: 200, outcome: 'succeeded' }),
    'HTTP 200 · 已完成 (Completed)',
  );
  assert.equal(
    eventStatusDetailLabel({ statusCode: 200, phase: 'inFlight' }),
    'HTTP 200 · 进行中 (In Flight)',
  );
});

test('event analytics recognizes failures, streams, tools, and Codex dimensions', () => {
  const events = [{
    id: 'failed-200', kind: 'client', statusCode: 200, outcome: 'failed',
    failureKind: 'upstream_response_failed', failurePhase: 'response_stream',
    featureRuleID: 'codex', effectiveModel: 'gpt-5',
    upstreamStatusCode: 200, toolCalls: ['functions.exec_command'],
    streamTrace: { chunkCount: 4, bytesReceived: 1024, maxChunkGapMS: 250, terminalEvent: 'response.failed' },
    codexMetadata: {
      requestKind: 'turn', isSubagent: true, subagentKind: 'thread_spawn',
      threadSource: 'collaboration', agentName: '/root/worker',
      workspaces: { '/workspace/demo': {} },
      toolNamespacesInfo: { functions: {} },
      compaction: { trigger: 'threshold', phase: 'complete' },
    },
  }];
  const row = aggregateEvents(events, () => 'all')[0];
  assert.equal(row.failures, 1);
  assert.equal(row.avgChunks, 4);
  assert.equal(row.maxChunkGapMS, 250);
  assert.equal(row.toolsList[0].name, 'functions.exec_command');
  assert.equal(row.terminalEventsList[0].name, 'response.failed');
  assert.deepEqual(codexDimensionValues(events[0].codexMetadata, 'workspace'), ['/workspace/demo']);
  assert.equal(codexRows(events, 'toolNamespace')[0].name, 'functions');
  assert.equal(codexRows(events, 'compaction')[0].name, 'threshold · complete');
});

test('runtime mock includes a representative HTTP 200 protocol failure', async () => {
  const runtime = await api.getRuntimeEvents({ limit: 200 });
  const item = runtime.events.find((candidate) => candidate.id === 'ev-codex-response-failed-200');
  const event = (await api.getRuntimeEvent(item.id)).event;
  assert.ok(event);
  assert.equal(event.statusCode, 200);
  assert.equal(event.outcome, 'failed');
  assert.equal(event.failureKind, 'upstream_response_failed');
  assert.equal(event.failurePhase, 'response_stream');
  assert.match(event.failureDetail, /response\.failed/);
  assert.equal(event.message, 'bridge openai-responses');
  assert.deepEqual(event.toolCalls, ['functions.exec_command', 'collaboration.spawn_agent']);
  assert.equal(Object.prototype.hasOwnProperty.call(event, 'poolID'), false);
  assert.equal(event.featureRuleID, 'builtin-codex-primary');
  assert.equal(event.endpointID, 'ep-openrouter-fast');
  assert.equal(event.upstreamHost, 'openrouter.ai');
  assert.equal(event.upstreamStatusCode, 200);
  assert.equal(event.upstreamRequestID, 'req_openai_failed_200_demo');
});

test('runtime analytics mock exposes token and project usage dimensions', async () => {
  const analytics = await api.getRuntimeAnalytics('24h');
  assert.equal(analytics.clientRequests, analytics.clientSuccesses + analytics.clientFailures + analytics.clientCancelled + analytics.clientPending);
  assert.equal(Math.round(analytics.clientSuccessRate * 100) / 100, Math.round((analytics.clientSuccesses / (analytics.clientSuccesses + analytics.clientFailures + analytics.clientCancelled) * 100) * 100) / 100);
  assert.equal(analytics.tokenUsage.totalTokens, 231000);
  assert.equal(analytics.tokenUsage.cacheReadInputTokens, 121000);
  assert.equal(analytics.tokenUsage.processedInputTokens, 247400);
  assert.equal(analytics.tokenUsage.processedTotalTokens, 296000);
  assert.equal(analytics.tokenUsage.reasoningTokens, 9200);
  assert.ok(analytics.endpoints.length > 0);
  assert.ok(analytics.endpoints.every((row) => row.processedTotalTokens > 0));
  assert.equal(analytics.endpoints.reduce((sum, row) => sum + row.attempts, 0), analytics.upstreamAttempts);
  assert.equal(analytics.endpoints.reduce((sum, row) => sum + row.processedTotalTokens, 0), analytics.tokenUsage.processedTotalTokens);
  assert.equal(analytics.endpoints[0].processedInputTokens, 141000, 'Anthropic mock 必须按独立缓存口径归一化输入');
  assert.equal(analytics.projects[0].name, 'automode-proxy');
  assert.equal(analytics.projects[0].totalTokens, 157000);
  assert.equal(analytics.sessions[0].name, 'session-demo-001');
  assert.deepEqual(analytics.sessions[0].projects, ['automode-proxy']);
  assert.deepEqual(analytics.sessions[0].clientKinds, ['codex']);
  assert.ok(analytics.facets.clientKinds.some((row) => row.value === 'codex'));
  assert.ok(analytics.facets.projects.some((row) => row.value === 'automode-proxy'));
  assert.ok(analytics.facets.sessions.some((row) => row.value === 'session-demo-001'));
});

test('cache hit rate uses protocol-normalized processed input and preserves unknowns', () => {
  const presence = {
    inputTokens: 1,
    outputTokens: 1,
    cacheReadInputTokens: 1,
    cacheCreationInputTokens: 1,
  };
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset',
    inputTokens: 431805163,
    cacheReadInputTokens: 384166656,
    processedInputTokens: 431805163,
    usageFieldPresence: { ...presence, cacheCreationInputTokens: 0 },
  }), '89.0%');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'independent',
    inputTokens: 12,
    cacheReadInputTokens: 4,
    cacheCreationInputTokens: 2,
    processedInputTokens: 18,
    usageFieldPresence: presence,
  }), '22.2%');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset,independent',
    cacheReadInputTokens: 64,
    processedInputTokens: 118,
    usageFieldPresence: { ...presence, inputTokens: 2 },
  }), '54.2%');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'unknown',
    cacheReadInputTokens: 4,
    processedInputTokens: 18,
    usageFieldPresence: presence,
  }), '—');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset',
    cacheReadInputTokens: 4,
    usageFieldPresence: presence,
  }), '—', 'processedInputTokens 缺失不能用零或旧公式伪造比例');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset',
    cacheReadInputTokens: 4,
    processedInputTokens: 18,
    usageFieldPresence: { ...presence, inputTokens: 0 },
  }), '—', 'inputTokens 缺失时分母不完整，不能伪造比例');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset',
    cacheReadInputTokens: 0,
    processedInputTokens: 10,
    usageFieldPresence: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 1, cacheCreationInputTokens: 0 },
  }), '0.0%');
  assert.equal(cacheHitRateValue({
    tokenAccountingSemantics: 'subset',
    cacheReadInputTokens: 12,
    processedInputTokens: 10,
    usageFieldPresence: { inputTokens: 1, outputTokens: 1, cacheReadInputTokens: 1 },
  }), '100.0%', '异常快照不能显示超过 100%');
});

test('token usage values use comma grouping without changing ordinary counters', () => {
  assert.equal(formatTokenCount(431805163), '431,805,163');
  assert.equal(formatTokenCount('1575726'), '1,575,726');
  assert.equal(formatTokenCount(-12000), '-12,000');
  assert.equal(formatTokenCount('—'), '—');
});

test('runtime analytics request forwards client, endpoint, project, and session filters', () => {
  assert.match(apiServiceSource, /query\.set\('clientKind', filters\.clientKind\)/);
  assert.match(apiServiceSource, /query\.set\('endpointID', filters\.endpointID\)/);
  assert.match(apiServiceSource, /query\.set\('project', filters\.project\)/);
  assert.match(apiServiceSource, /query\.set\('sessionID', filters\.sessionID\)/);
});

test('runtime analytics mock applies AND filters and scopes token usage', async () => {
  const filtered = await api.getRuntimeAnalytics('24h', {
    clientKind: 'codex',
    endpointID: 'ep-anthropic-direct',
    project: 'automode-proxy',
    sessionID: 'session-demo-001',
  });
  assert.equal(filtered.filtersApplied, true);
  assert.deepEqual(filtered.appliedFilters, {
    clientKind: 'codex', endpointID: 'ep-anthropic-direct', project: 'automode-proxy', sessionID: 'session-demo-001',
  });
  assert.deepEqual(filtered.projects.map((row) => row.name), ['automode-proxy']);
  assert.deepEqual(filtered.sessions.map((row) => row.name), ['session-demo-001']);
  assert.equal(filtered.tokenUsage.inputTokens, 58400);
  assert.equal(filtered.tokenUsage.usageFieldPresence.inputTokens, 36);
});

test('runtime analytics mock keeps the full compatible session facet after selecting one session', async () => {
  const filtered = await api.getRuntimeAnalytics('24h', {
    project: 'automode-proxy',
    sessionID: 'session-demo-001',
  });
  assert.deepEqual(
    filtered.facets.sessions.map((item) => item.value),
    ['session-demo-001', 'unidentified_session'],
  );
  assert.equal(filtered.facets.sessions.find((item) => item.value === 'session-demo-001').count, 36);
});

test('runtime mock covers main and subagent Codex metadata with nested wire fields', async () => {
  const runtime = await api.getRuntimeEvents({ limit: 200 });
  const mainItem = runtime.events.find((item) => item.id === 'ev-codex-response-failed-200');
  const subagentItem = runtime.events.find((item) => item.id === 'ev-websearch-1');
  const main = (await api.getRuntimeEvent(mainItem.id)).event;
  const subagent = (await api.getRuntimeEvent(subagentItem.id)).event;
  assert.equal(main.codexMetadata.isSubagent, false);
  assert.equal(main.codexMetadata.agentName, '/root');
  assert.equal(codexMetadataSummary(main), '主代理 · 代理路径 /root · 请求 turn · 线程 thread-main-0001 · 回合 turn-main-0041');
  assert.equal(subagent.codexMetadata.isSubagent, true);
  assert.equal(subagent.codexMetadata.agentName, '/root/linux_final_tests');
  assert.equal(subagent.codexMetadata.originator, 'codex_cli_rs');
  assert.equal(
    subagent.codexMetadata.workspaces['/workspace/demo'].associatedRemoteURLs.origin,
    'https://github.com/example/demo.git',
  );
  assert.equal(
    subagent.codexMetadata.toolNamespacesInfo.functions.functions.rg.source.kind,
    'built_in',
  );
});

test('runtime event UI exposes endpoint identity and request-chain grouping', () => {
  const client = { id: 'client-1', requestID: 'req-2', kind: 'client', endpointName: '主入口', endpointID: 'ep-1', upstreamHost: 'api.example.test' };
  const upstream = { id: 'upstream-1', requestID: 'req-2', kind: 'upstream', endpointID: 'ep-1' };
  assert.equal(eventEndpoint(client), '主入口 @ api.example.test (ep-1)');
  assert.deepEqual(getRequestChain([upstream, client], 'client-1').map((event) => event.id), ['upstream-1', 'client-1']);
});

test('v6 UI config removes legacy fixed IP fields while adapting mappings', () => {
  const ui = fromWireConfig({ generation: 'g1', config: { schemaVersion: 6, listener: { authToken: '', host: '127.0.0.1', port: 57878 }, retry: { pinnedIPConcurrency: 3 }, featureRules: [], endpoints: [{ id: 'ep', name: '入口', baseURL: 'https://example.invalid', apiKey: '', protocol: 'openai', enabled: true, pinnedIPs: ['203.0.113.1'], pinnedIPExclusive: true, mappings: [{ clientPattern: 'gpt-*', upstreamModel: 'gpt-5', thinking: 'disabled', context: 'strip' }] }] }, secretStatus: { endpoints: { ep: { apiKey: { configured: true, last4: '1234' } } } } });
  assert.equal('pools' in ui.config, false);
  assert.equal(ui.config.endpoints[0].modelMappings[0].from, 'gpt-*');
  assert.equal('pinnedIPs' in ui.config.endpoints[0], false);
  assert.equal('pinnedIPExclusive' in ui.config.endpoints[0], false);
  assert.equal('pinnedIPConcurrency' in ui.config.retry, false);
  assert.equal(ui.secretStatus.endpoints.ep.configured, true);
});

test('v6 retry settings preserve HTTP 500 failover and retry delay fields', () => {
  const wire = toWireConfig({
    schemaVersion: 6,
    listener: {},
    retry: { max500Retries: 4, failoverOn500: false, retryDelaySeconds: 2.5 },
    featureRules: [],
    endpoints: [],
  });
  assert.equal(wire.retry.max500Retries, 4);
  assert.equal(wire.retry.failoverOn500, false);
  assert.equal(wire.retry.retryDelaySeconds, 2.5);
});

test('save adapter emits only Rust v6 endpoint and routing fields', () => {
  const wire = toWireConfig({ schemaVersion: 6, listener: { authToken: 'redacted', host: '127.0.0.1', port: 57878 }, retry: {}, featureRules: [{ id: 'custom', name: 'custom', enabled: true, match: { requestKind: 'session_title', modelEquals: 'x' }, target: { endpointID: 'ep', model: 'x', effort: 'high' } }], endpoints: [{ id: 'ep', name: '入口', baseURL: 'https://example.invalid', apiKey: 'redacted', protocol: 'openai', enabled: true, pinnedIP: '203.0.113.1', pinnedIPExclusive: true, timeoutSeconds: 30, headers: { x: 'y' }, modelMappings: [{ from: 'gpt-*', to: 'gpt-5', thinking: 'disable', context: 'strip' }] }] });
  const endpoint = wire.endpoints[0];
  assert.equal('pinnedIPs' in endpoint, false);
  assert.equal('pinnedIP' in endpoint, false);
  assert.equal('pinnedIPExclusive' in endpoint, false);
  assert.deepEqual(endpoint.mappings[0], { clientPattern: 'gpt-*', upstreamModel: 'gpt-5', thinking: 'disabled', context: 'strip' });
  assert.equal(endpoint.apiKey, '');
  assert.equal('modelMappings' in endpoint, false);
  assert.equal('timeoutSeconds' in endpoint, false);
  assert.equal('headers' in endpoint, false);
  assert.equal('requestKind' in wire.featureRules[0].match, false);
  assert.equal(wire.featureRules[0].target.effort, 'high');
  assert.equal(wire.schemaVersion, 6);
  assert.equal('inboundDialectPassthrough' in wire.listener, false);
  assert.equal(endpoint.protocol, 'openai');
  assert.equal('pools' in wire, false);
});

test('legacy pool-level model rules are handled by the server migration', () => {
  assert.throws(() => toWireConfig({ schemaVersion: 5, listener: {}, pools: [] }), /不允许 pools/);
  const wire = toWireConfig({ schemaVersion: 6, listener: {}, endpoints: [
    { id: 'empty', protocol: 'auto', mappings: [] },
    { id: 'explicit', protocol: 'auto', mappings: [{ from: 'gpt-*', to: 'gpt-5', thinking: 'disable', context: 'passThrough' }] },
  ] });
  assert.deepEqual(wire.endpoints[0].mappings, []);
  assert.deepEqual(wire.endpoints[1].mappings, [{
    clientPattern: 'gpt-*', upstreamModel: 'gpt-5', thinking: 'disabled', context: 'standard',
  }]);
});

test('legacy schema is rejected by the UI adapter and server owns migration', () => {
  assert.throws(() => fromWireConfig({ config: {
    schemaVersion: 3,
    listener: { inboundDialectPassthrough: true },
    pools: [{ endpoints: [{ id: 'a', protocol: 'anthropic' }] }],
  } }), /仅支持 schema v6/);
  const ui = fromWireConfig({ config: {
    schemaVersion: 6,
    listener: {},
    endpoints: [{ id: 'a', protocol: 'auto' }, { id: 'b', protocol: 'auto' }],
  } });
  assert.deepEqual(ui.config.endpoints.map((endpoint) => endpoint.protocol), ['auto', 'auto']);
});

test('save adapter rejects unknown endpoint modes and Auto feature targets', () => {
  assert.throws(
    () => toWireConfig({ listener: {}, endpoints: [{ id: 'bad', protocol: 'guess' }] }),
    /protocol 非法/,
  );
  assert.throws(
    () => toWireConfig({
      listener: {}, endpoints: [],
      featureRules: [{ id: 'bad-rule', target: { model: 'x', protocol: 'auto' } }],
    }),
    /目标协议非法/,
  );
});

test('provider-models API posts endpointID and normalizes the catalog response', async () => {
  const result = await api.fetchProviderModels('ep-azure-eastus');
  assert.equal(result.endpointID, 'ep-azure-eastus');
  assert.deepEqual(result.models, ['gpt-4o', 'gpt-4o-mini']);
  assert.match(result.source, /eastus\.openai\.azure\.com\/.*models$/);
  assert.match(result.updatedAt, /^\d+$/);
});

test('v6 flat endpoints preserve provider order, mappings, and endpoint fields', () => {
  const wireDocument = {
    schemaVersion: 6,
    listener: { authToken: '', host: '127.0.0.1', port: 57878 },
    retry: {},
    featureRules: [],
    endpoints: [{
      id: 'ep-primary', name: '主入口', baseURL: 'https://primary.invalid', protocol: 'openai', enabled: true,
      priority: 0, mappings: [],
    }, {
      id: 'ep-fallback', name: '备用', baseURL: 'https://provider.invalid', protocol: 'openai', enabled: true,
      priority: 10, keepAlive: true, searchDialect: 'openai',
      stickyGroup: 'shared-fallback',
      catalog: {
        models: ['model-a', 'model-a', ' model-b '], source: 'https://provider.invalid/v1/models',
        status: '已获取', error: '', updatedAt: '1720000000',
      },
      mappings: [{
        clientPattern: 'claude-*', upstreamModel: 'provider-model', thinking: 'passthrough',
        context: 'oneMillion', failoverTimeoutSeconds: 7.5,
      }],
    }],
  };
  const ui = fromWireConfig({ config: wireDocument, secretStatus: { endpoints: {} } });
  assert.equal('pools' in ui.config, false);
  assert.deepEqual(ui.config.endpoints.map((endpoint) => endpoint.id), ['ep-primary', 'ep-fallback']);
  assert.equal(ui.config.endpoints[1].priority, 10);
  const endpoint = ui.config.endpoints[1];
  assert.equal('pinnedIPs' in endpoint, false);
  assert.equal('pinnedIP' in endpoint, false);
  assert.equal('pinnedIPExclusive' in endpoint, false);
  assert.equal(endpoint.stickyGroup, 'shared-fallback');
  assert.equal(endpoint.keepAlive, true);
  assert.deepEqual(endpoint.catalog.models, ['model-a', 'model-b']);
  assert.equal(endpoint.modelMappings[0].thinking, 'passThrough');
  assert.equal(endpoint.modelMappings[0].context, 'oneMillion');
  assert.equal(endpoint.modelMappings[0].failoverTimeoutSeconds, 7.5);

  const roundTrip = toWireConfig(ui.config);
  const saved = roundTrip.endpoints[1];
  assert.equal('pinnedIPs' in saved, false);
  assert.equal('pinnedIP' in saved, false);
  assert.deepEqual(saved.catalog.models, ['model-a', 'model-b']);
  assert.equal(saved.catalog.status, '已获取');
  assert.equal(saved.keepAlive, true);
  assert.equal(saved.searchDialect, undefined);
  assert.equal('pinnedIPExclusive' in saved, false);
  assert.equal(saved.stickyGroup, 'shared-fallback');
  assert.deepEqual(saved.mappings[0], {
    clientPattern: 'claude-*', upstreamModel: 'provider-model', thinking: 'passthrough',
    context: 'oneMillion', failoverTimeoutSeconds: 7.5,
  });
  assert.equal('pools' in roundTrip, false);
});

test('legacy fixed IP aliases are ignored and never written back', () => {
  const legacy = fromWireConfig({ config: {
    schemaVersion: 6,
    retry: { pinnedIPConcurrency: 3 },
    endpoints: [{ id: 'legacy', pinnedIP: '203.0.113.3', pinnedIPs: ['203.0.113.4'], pinnedIPExclusive: true }],
  } });
  assert.equal('pinnedIP' in legacy.config.endpoints[0], false);
  assert.equal('pinnedIPs' in legacy.config.endpoints[0], false);
  assert.equal('pinnedIPExclusive' in legacy.config.endpoints[0], false);
  const cleared = toWireConfig({ schemaVersion: 6, retry: { pinnedIPConcurrency: 1 }, endpoints: [{ id: 'clear', pinnedIP: '203.0.113.99', pinnedIPs: ['203.0.113.1'] }] });
  assert.equal('pinnedIP' in cleared.endpoints[0], false);
  assert.equal('pinnedIPs' in cleared.endpoints[0], false);
  assert.equal('pinnedIPConcurrency' in cleared.retry, false);
});

test('editing UI mapping aliases does not drop thinking, context, or failover timeout', () => {
  const ui = fromWireConfig({ config: {
    schemaVersion: 6,
    endpoints: [{ id: 'ep', mappings: [{
      clientPattern: 'client-model', upstreamModel: 'upstream-model', thinking: 'adaptive',
      context: 'strip', failoverTimeoutSeconds: 11,
    }] }],
  } });
  const mapping = ui.config.endpoints[0].modelMappings[0];
  mapping.from = 'edited-client';
  mapping.to = 'edited-upstream';
  const saved = toWireConfig(ui.config).endpoints[0].mappings[0];
  assert.deepEqual(saved, {
    clientPattern: 'edited-client', upstreamModel: 'edited-upstream', thinking: 'adaptive',
    context: 'strip', failoverTimeoutSeconds: 11,
  });
});

test('diagnostics mock mirrors the listener object returned by the Rust admin API', async () => {
  const diagnostics = await api.getDiagnostics();
  assert.deepEqual(diagnostics.adminListener, { host: '127.0.0.1', port: 57879 });
  assert.deepEqual(diagnostics.proxyListener, { host: '127.0.0.1', port: 57878 });
});

test('diagnostics request forwards AbortSignal for cancellable refreshes', async () => {
  const controller = new AbortController();
  let captured;
  const original = api.request.bind(api);
  api.request = async (path, options) => {
    captured = { path, options };
    return {};
  };
  try {
    await api.getDiagnostics({ signal: controller.signal });
  } finally {
    api.request = original;
  }
  assert.equal(captured.path, '/diagnostics');
  assert.equal(captured.options.signal, controller.signal);
});

test('diagnostic capture uses an index first and loads plaintext by request ID', async () => {
  const index = await api.getDiagnosticCapture();
  assert.ok(Array.isArray(index.records));
  assert.equal(index.recordCount, index.records.length);
  assert.equal(index.indexTruncated, false);
  assert.equal(Object.prototype.hasOwnProperty.call(index.records[0] || {}, 'inboundBody'), false);
  const detail = await api.getDiagnosticCaptureDetail(index.records[0].requestID);
  assert.equal(detail.requestID, index.records[0].requestID);
  assert.equal(typeof detail.inboundBody, 'string');
});

test('诊断捕获详情用大写 ID 键并带客户端声明的项目归因', async () => {
  const { clientDeclaredProject } = await import('../src/utils/helpers.js');
  const index = await api.getDiagnosticCapture();
  const detail = await api.getDiagnosticCaptureDetail(index.records[0].requestID);

  // 详情端点直接序列化 Rust 的 DiagnosticRequestCapture,serde 的 camelCase 规则会把
  // request_id 写成 requestId。索引记录(手写 json!)一直是大写口径,详情必须一致:
  // 否则下载文件名会带 undefined,尝试块的入口/出站 URL 也读不出来。
  assert.ok(Object.prototype.hasOwnProperty.call(detail, 'requestID'), '详情缺大写键 requestID');
  // 池概念只剩事件 wire 与路由内部:捕获记录里那个恒为 primary 的展示残留已摘掉,
  // 索引与详情都不该再出现它。
  for (const stale of ['requestId', 'featureRuleId', 'poolID', 'poolId']) {
    assert.equal(
      Object.prototype.hasOwnProperty.call(detail, stale),
      false,
      `详情残留小写键 ${stale}`,
    );
  }
  assert.equal(Object.prototype.hasOwnProperty.call(index.records[0], 'poolID'), false, '索引残留池字段');
  const attempt = detail.attempts[0];
  assert.ok(attempt.endpointID, '尝试缺 endpointID');
  assert.ok(attempt.outboundURL, '尝试缺 outboundURL');
  for (const stale of ['endpointId', 'outboundUrl', 'pinnedIp']) {
    assert.equal(
      Object.prototype.hasOwnProperty.call(attempt, stale),
      false,
      `尝试残留小写键 ${stale}`,
    );
  }

  // 诊断页「项目」行的数据来源就是捕获记录里的客户端声明。
  const project = clientDeclaredProject(detail);
  assert.equal(project?.name, 'automode-proxy');
  assert.equal(project?.workspace, '.../.claude/automode-proxy');
});

test('diagnostic capture detail forwards AbortSignal and URL-encodes request ID', async () => {
  const controller = new AbortController();
  let captured;
  const original = api.request.bind(api);
  api.request = async (path, options) => {
    captured = { path, options };
    return { requestID: 'REQ / 1' };
  };
  try {
    await api.getDiagnosticCaptureDetail('REQ / 1', { signal: controller.signal });
  } finally {
    api.request = original;
  }
  assert.equal(captured.path, '/diagnostic-capture/REQ%20%2F%201');
  assert.equal(captured.options.signal, controller.signal);
});

test('diagnostic capture write response remains an index without plaintext body', async () => {
  const index = await api.setDiagnosticCapture(false);
  assert.ok(Array.isArray(index.records));
  assert.equal(index.recordCount, index.records.length);
  assert.equal(Object.prototype.hasOwnProperty.call(index.records[0] || {}, 'inboundBody'), false);
});

test('runtime mock session export and delete update the visible aggregate', async () => {
  const before = await api.getRuntimeAnalytics('24h');
  const exported = await api.exportRuntimeSession('session-demo-001');
  assert.equal(exported.format, 'sumpter-session-export-v1');
  assert.equal(exported.sessionID, 'session-demo-001');
  const mutation = await api.deleteRuntimeSession('session-demo-001');
  assert.equal(mutation.deletedRequests, 36);
  const after = await api.getRuntimeAnalytics('24h');
  assert.equal(after.sessions.some((row) => row.name === 'session-demo-001'), false);
  assert.equal(after.clientRequests, before.clientRequests - mutation.deletedRequests);
  const summary = await api.getRuntimeSummary();
  assert.equal(summary.latestEvent?.codexMetadata?.sessionID, undefined);
  await assert.rejects(() => api.exportRuntimeSession('session-demo-001'), (error) => error.status === 404);
});

test('客户端声明的项目归因与 Codex workspace 同形状且来源可区分', async () => {
  const { eventProjectContext, clientDeclaredProject, projectSourceLabel, localUserFromWorkspacePath } =
    await import('../src/utils/helpers.js');

  // mock 的项目行必须和 Rust analytics 同形状：脱敏尾段 workspacePaths + 独立来源词。
  const analytics = await api.getRuntimeAnalytics('24h');
  const declaredRow = analytics.projects.find((row) => row.projectSource === 'client_declared');
  assert.ok(declaredRow, 'mock 缺少 client_declared 项目行');
  assert.ok(Array.isArray(declaredRow.workspacePaths));
  assert.ok(
    declaredRow.workspacePaths.every((path) => !path.startsWith('/')),
    '工作区只能是脱敏尾段，不得是完整绝对路径',
  );
  assert.equal(projectSourceLabel('client_declared'), '客户端声明');

  // 无 Codex workspace 时读客户端声明，来源标 client_declared。
  const declaredEvent = {
    kind: 'client',
    clientDeclared: { project: 'demo', workspace: '.../claude/demo', gitRemote: 'https://github.com/o/r.git' },
  };
  const context = eventProjectContext(declaredEvent);
  assert.equal(context.applicable, true);
  assert.equal(context.name, 'demo');
  assert.equal(context.source, 'workspace_local');

  // Codex 结构化 workspace 必须优先，声明值不得把来源降级。
  const bothEvent = {
    kind: 'client',
    codexMetadata: { workspaces: { '/Users/x/projects/codex-demo': {} } },
    clientDeclared: { project: 'declared-loser' },
  };
  assert.equal(eventProjectContext(bothEvent).source, 'workspace_local');
  assert.equal(eventProjectContext(bothEvent).localUser, 'x');
  assert.equal(eventProjectContext(bothEvent).label, 'codex-demo 本地(x)');
  assert.equal(projectSourceLabel('workspace_local', 'kkl'), '本地(kkl)');
  assert.equal(localUserFromWorkspacePath('/Users/kkl/Documents/claude/sumpter'), 'kkl');
  assert.equal(localUserFromWorkspacePath('.../claude/sumpter'), '');

  const codexEvent = {
    kind: 'client',
    codexMetadata: {
      workspaces: { '.../claude/sumpter': {} },
      sourceWorkspacePaths: ['/Users/kkl/Documents/claude/sumpter'],
    },
  };
  assert.equal(eventProjectContext(codexEvent).source, 'workspace_local');
  assert.equal(eventProjectContext(codexEvent).localUser, 'kkl');
  assert.equal(eventProjectContext(codexEvent).label, 'sumpter 本地(kkl)');

  // 两者都没有仍归入未识别，且不为空对象误报。
  assert.equal(clientDeclaredProject({ kind: 'client', clientDeclared: {} }), null);
  assert.equal(eventProjectContext({ kind: 'client' }).name, 'unidentified_project');

  // upstream 事件不做项目归因。
  assert.equal(eventProjectContext({ kind: 'upstream', clientDeclared: { project: 'x' } }).applicable, false);
});

// 回归:分页列表走服务端投影快路径,不带 codexMetadata / clientDeclared,只带算好的
// projectName + projectSource。前端若只按那两个字段推导,翻页拿到的行会全部显示
// 「未识别项目」——而 SSE 推送的同一批事件是好的(那条路径带完整字段),表现为
// 「详情识别到项目,列表仍显示未识别」。
test('列表投影行只带 projectName/projectSource 时仍能正确归因', async () => {
  const { eventProjectContext } = await import('../src/utils/helpers.js');

  const projectedRow = {
    kind: 'client',
    projectName: 'automode-proxy',
    projectSource: 'client_declared',
    // 投影快路径刻意不带这两个,不能因此判成未识别
    codexMetadata: undefined,
    clientDeclared: undefined,
  };
  const context = eventProjectContext(projectedRow);
  assert.equal(context.applicable, true);
  assert.equal(context.name, 'automode-proxy');
  assert.equal(context.source, 'client_declared');
  assert.match(context.label, /客户端声明/);

  // 投影里的合成桶名要翻成中文再展示。
  const unidentified = eventProjectContext({
    kind: 'client',
    projectName: 'unidentified_project',
    projectSource: 'missing_workspace_metadata',
  });
  assert.equal(unidentified.name, 'unidentified_project');
  assert.match(unidentified.label, /未识别项目/);

  // 服务端投影优先于客户端自行推导,避免两边算出不同结果。
  const conflicting = eventProjectContext({
    kind: 'client',
    projectName: 'server-wins',
    projectSource: 'workspace_local',
    clientDeclared: { project: 'client-loses' },
  });
  assert.equal(conflicting.name, 'server-wins');
  assert.equal(conflicting.source, 'workspace_local');

  const localNamed = eventProjectContext({
    kind: 'client',
    projectName: 'sumpter',
    projectSource: 'workspace_local',
    localUser: 'kkl',
  });
  assert.equal(localNamed.label, 'sumpter 本地(kkl)');
});

test('统一 runtime dimensions endpoint accepts every v3 dimension kind', async () => {
  const kinds = ['endpoint', 'model', 'clientKind', 'purpose', 'failureKind', 'failurePhase', 'protocol', 'streamTerminal', 'project', 'session'];
  for (const kind of kinds) {
    const page = await api.getRuntimeDimensions(kind, { page: 1, pageSize: 10 });
    assert.equal(page.apiVersion, 3);
    assert.equal(page.kind, kind);
    assert.ok(Number.isInteger(page.totalCount));
    assert.ok(Array.isArray(page.rows));
  }
  assert.throws(
    () => api.getRuntimeDimensions('pool'),
    (error) => error instanceof TypeError,
  );
});

// CC 归因提示的判定必须与 macOS 侧 ClaudeAttributionHint 同口径:只看分析数据,
// 不读 shell 配置。v3 项目行不带 clientKinds,所以要配合 facets 才能确认这批
// 未识别流量确实来自 Claude Code——否则 Codex 的未识别行会误催用户配 CC。
test('CC 归因提示判定:项目行与 clientKinds facets 合起来判', async () => {
  const { shouldPromptCCAttribution, CC_ATTRIBUTION_HINT } =
    await import('../src/utils/helpers.js');

  const ccFacets = [{ value: 'claude_code', count: 5 }];
  const codexFacets = [{ value: 'codex', count: 5 }];
  const unidentified = [{ name: 'unidentified_project', requests: 3 }];

  assert.equal(shouldPromptCCAttribution(unidentified, ccFacets), true);

  // 只有 Codex 的未识别行不该催用户配 CC。
  assert.equal(shouldPromptCCAttribution(unidentified, codexFacets), false);

  // 已经配好(出现 client_declared 行)后闭嘴,剩下的未识别行是历史事件。
  assert.equal(
    shouldPromptCCAttribution(
      [...unidentified, { name: 'automode-proxy', source: 'client_declared', requests: 1 }],
      ccFacets,
    ),
    false,
  );

  // 全部已归因、空数据、0 请求的筛选残留都不提示。
  assert.equal(shouldPromptCCAttribution([{ name: 'p', source: 'workspace_local', requests: 3 }], ccFacets), false);
  assert.equal(shouldPromptCCAttribution([], ccFacets), false);
  assert.equal(shouldPromptCCAttribution([{ name: 'unidentified_project', requests: 0 }], ccFacets), false);

  // 非数组入参不得抛异常(analytics 未加载时就是 undefined)。
  assert.equal(shouldPromptCCAttribution(undefined, undefined), false);

  // 文案里必须给出可复制的命令,否则提示等于没用。
  assert.match(CC_ATTRIBUTION_HINT.command, /cc-project-attribution\.sh install/);
});

test('Grok 归因提示判定:未识别 grok_build 行才提示', async () => {
  const { grokAttributionState, GROK_ATTRIBUTION_GUIDE, CC_ATTRIBUTION_STATE } =
    await import('../src/utils/helpers.js');
  assert.equal(
    grokAttributionState(
      [{ name: 'unidentified_project', requests: 3, clientKinds: ['grok_build'] }],
    ),
    CC_ATTRIBUTION_STATE.unconfigured,
  );
  assert.equal(
    grokAttributionState(
      [{ name: 'sumpter', source: 'workspace_local', requests: 3, clientKinds: ['grok_build'] }],
    ),
    CC_ATTRIBUTION_STATE.configured,
  );
  assert.equal(
    grokAttributionState(
      [{ name: 'unidentified_project', requests: 3, clientKinds: ['codex'] }],
    ),
    CC_ATTRIBUTION_STATE.unknown,
  );
  assert.match(GROK_ATTRIBUTION_GUIDE.steps[1].command, /grok-project-attribution\.sh install/);
});

test('项目行副标题显示对应客户端且不把项目来源冒充客户端', async () => {
  const { projectClientKindsLabel } = await import('../src/utils/helpers.js');
  assert.equal(projectClientKindsLabel(['codex']), 'Codex');
  assert.equal(projectClientKindsLabel(['claude_code']), 'Claude Code');
  assert.equal(
    projectClientKindsLabel(['codex', 'openai_compat', 'codex']),
    'Codex · OpenAI 兼容客户端',
  );
  assert.equal(projectClientKindsLabel([]), '客户端未记录');
});

test('CC 归因三态判定:configured / unconfigured / unknown', async () => {
  const { ccAttributionState, CC_ATTRIBUTION_STATE, shouldPromptCCAttribution } =
    await import('../src/utils/helpers.js');
  const ccFacets = [{ value: 'claude_code', count: 950 }];
  const codexFacets = [{ value: 'codex', count: 12 }];

  // 出现任何 client_declared 行就是配好了,哪怕旁边还留着配置生效前的未识别行。
  assert.equal(
    ccAttributionState(
      [
        { name: 'unidentified_project', requests: 5 },
        { name: 'automode-proxy', source: 'client_declared', requests: 3 },
      ],
      ccFacets,
    ),
    CC_ATTRIBUTION_STATE.configured,
  );

  // 有 CC 流量 + 未识别项目行 = 确实没配。列表投影行用 projectSource 别名,同样要认。
  assert.equal(
    ccAttributionState([{ name: 'unidentified_project', requests: 5 }], ccFacets),
    CC_ATTRIBUTION_STATE.unconfigured,
  );
  assert.equal(
    ccAttributionState(
      [{ name: 'p', projectSource: 'missing_workspace_metadata', requests: 2 }],
      ccFacets,
    ),
    CC_ATTRIBUTION_STATE.unconfigured,
  );

  // unknown 的两种成因:窗口内没有 CC 流量;有 CC 但一行未识别都没有(窗口边界)。
  assert.equal(
    ccAttributionState([{ name: 'unidentified_project', requests: 5 }], codexFacets),
    CC_ATTRIBUTION_STATE.unknown,
  );
  assert.equal(
    ccAttributionState([{ name: 'p', source: 'workspace_local', requests: 3 }], ccFacets),
    CC_ATTRIBUTION_STATE.unknown,
  );
  // 安全页面可能在分析数据到达前就渲染:不能抛,也不能谎报"没配"。
  assert.equal(ccAttributionState(undefined, undefined), CC_ATTRIBUTION_STATE.unknown);

  // 行自带 clientKinds 时以行为准(analytics.projects 就是这种行):未归因的那行全是
  // Codex 时不能说 CC 没配,即使全局 facets 里有 CC 流量。与 macOS 侧同口径。
  assert.equal(
    ccAttributionState(
      [{ name: 'unidentified_project', attempts: 40, clientKinds: ['codex'] }],
      ccFacets,
    ),
    CC_ATTRIBUTION_STATE.unknown,
  );
  assert.equal(
    ccAttributionState(
      [{ name: 'unidentified_project', attempts: 40, clientKinds: ['claude_code', 'codex'] }],
      ccFacets,
    ),
    CC_ATTRIBUTION_STATE.unconfigured,
  );

  // Bool 提示只是 unconfigured 的投影 —— 统计页行为不许因为加了三态而改变。
  for (const [rows, facets] of [
    [[{ name: 'unidentified_project', requests: 5 }], ccFacets],
    [[{ name: 'automode-proxy', source: 'client_declared', requests: 3 }], ccFacets],
    [[{ name: 'unidentified_project', requests: 5 }], codexFacets],
    [[], ccFacets],
    [undefined, undefined],
  ]) {
    assert.equal(
      shouldPromptCCAttribution(rows, facets),
      ccAttributionState(rows, facets) === CC_ATTRIBUTION_STATE.unconfigured,
    );
  }
});

test('安全页面归因引导:三态文案齐全、命令可复制、三个陷阱都在', async () => {
  const { CC_ATTRIBUTION_GUIDE, CC_ATTRIBUTION_HINT, CC_ATTRIBUTION_STATE } = await import('../src/utils/helpers.js');

  // 三态都要有徽标与解释,否则某个状态下 UI 会渲染出空白。
  for (const state of Object.values(CC_ATTRIBUTION_STATE)) {
    assert.ok(CC_ATTRIBUTION_GUIDE.statusLabels[state], `缺 statusLabels.${state}`);
    assert.ok(CC_ATTRIBUTION_GUIDE.statusDetails[state], `缺 statusDetails.${state}`);
  }

  // 少一条命令,用户就配不完或退不回来,所以逐条钉死而不是只数个数。
  const stepCommands = CC_ATTRIBUTION_GUIDE.steps.map((step) => step.command);
  assert.ok(stepCommands.includes('./cc-project-attribution.sh status'));
  assert.ok(stepCommands.includes('./cc-project-attribution.sh install'));
  const rollback = CC_ATTRIBUTION_GUIDE.rollback.map((item) => item.command);
  assert.ok(rollback.includes('./cc-project-attribution.sh restore'));
  assert.ok(rollback.includes('./cc-project-attribution.sh uninstall'));

  // 「新开终端」那步没有命令但必须有说明 —— 它正是最容易被跳过的一步。
  const commandless = CC_ATTRIBUTION_GUIDE.steps.filter((step) => !step.command);
  assert.equal(commandless.length, 1);
  assert.ok(commandless.every((step) => step.title && step.note));

  // 三个实测陷阱:漏掉任一条会让用户以为配置失败,或直接把 CC 弄到起不来。
  assert.equal(CC_ATTRIBUTION_GUIDE.pitfalls.length, 3);
  const pitfallTitles = CC_ATTRIBUTION_GUIDE.pitfalls.map((item) => item.title).join('|');
  assert.match(pitfallTitles, /ASCII/);
  assert.match(pitfallTitles, /settings\.json/);
  assert.match(pitfallTitles, /进程级/);
  assert.ok(CC_ATTRIBUTION_GUIDE.pitfalls.every((item) => item.detail));

  // 平台对照两列都要填满 —— 这张表两个产品的用户都会看。
  assert.ok(CC_ATTRIBUTION_GUIDE.platformMatrix.length > 0);
  assert.ok(
    CC_ATTRIBUTION_GUIDE.platformMatrix.every((row) => row.label && row.macos && row.linux),
  );

  // 最容易搞错的两点:在哪台机器配;以及"header 被剥离"不等于"什么都不外泄"。
  assert.match(CC_ATTRIBUTION_GUIDE.whereToRun, /daemon/);
  assert.match(CC_ATTRIBUTION_GUIDE.whereToRun, /WebUI/);
  assert.match(CC_ATTRIBUTION_GUIDE.whereToRun, /SSH/);
  assert.match(CC_ATTRIBUTION_GUIDE.whereToRun, /ANTHROPIC_BASE_URL/);
  assert.match(CC_ATTRIBUTION_GUIDE.privacy, /CLAUDE\.md/);

  // Linux 默认按远程部署理解:三台机器、三种拓扑和执行前身份检查都必须有，
  // 否则用户很容易把 wrapper 装到 daemon 主机或 root 用户下。
  assert.equal(CC_ATTRIBUTION_GUIDE.remoteMachines.length, 3);
  assert.ok(CC_ATTRIBUTION_GUIDE.remoteMachines.every((item) => item.label && item.detail));
  assert.equal(CC_ATTRIBUTION_GUIDE.remoteScenarios.length, 3);
  assert.ok(CC_ATTRIBUTION_GUIDE.remoteScenarios.every((item) => item.title && item.detail));
  assert.match(CC_ATTRIBUTION_GUIDE.remoteDownload.command, /__sumpter\/cc-project-attribution\.sh/);
  assert.match(CC_ATTRIBUTION_GUIDE.remoteDownload.command, /192\.168\.1\.20:57878/);
  assert.match(CC_ATTRIBUTION_GUIDE.remoteDownload.note, /Nginx/);
  assert.match(CC_ATTRIBUTION_GUIDE.remoteDownload.note, /发布镜像/);
  assert.deepEqual(
    CC_ATTRIBUTION_GUIDE.remoteChecks.map((item) => item.command),
    ['hostname', 'whoami', 'pwd', 'printf \'%s\\n\' "$ANTHROPIC_BASE_URL"'],
  );
  assert.match(CC_ATTRIBUTION_HINT.message, /实际运行/);
  assert.match(CC_ATTRIBUTION_HINT.hint, /SSH/);
});
