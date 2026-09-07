import { configSaveMessage } from '../utils/configFeedback.js';
import { pruneGroupReferences } from '../utils/modelGroups.js';
import {
  mockConfig, mockSecretStatus, mockStatus, mockRuntime, mockRuntimeHistory,
  mockAnalytics, mockDiagnostics, mockAutostart,
} from './mockData.js';
import { clone } from '../utils/helpers.js';
import {
  isEndpointProtocol,
  isSourceFormat,
  normalizeEndpointProtocol,
} from '../utils/protocols.js';
import {
  RUNTIME_DEFAULT_PAGE_SIZE,
  RUNTIME_EVENT_PAGE_SIZES,
  runtimeRangeFilter,
} from '../utils/runtimeAnalyticsV2.js';

const isMock = new URLSearchParams(window.location.search).get('mock') === '1';

function mockPageSize(value, fallback) {
  const parsed = value == null || String(value).trim() === '' ? fallback : Number(value);
  if (!RUNTIME_EVENT_PAGE_SIZES.includes(parsed)) {
    const error = new Error('pageSize 只允许 10、25、50、100 或 200');
    error.status = 400;
    error.code = 'invalid_page_size';
    throw error;
  }
  return parsed;
}

let csrfToken = '';
let authState = Object.freeze({ authenticated: false, username: '', expiresAt: 0 });
const authListeners = new Set();

function setAuthState(value) {
  const next = Object.freeze({
    authenticated: Boolean(value?.authenticated),
    username: String(value?.username || ''),
    expiresAt: Number(value?.expiresAt || 0),
  });
  if (JSON.stringify(next) !== JSON.stringify(authState)) {
    authState = next;
    for (const listener of authListeners) listener(authState);
  }
  return authState;
}

function setAuthenticated(value) {
  csrfToken = String(value?.csrfToken || '');
  return setAuthState(value);
}

function markUnauthenticated() {
  csrfToken = '';
  return setAuthState({ authenticated: false, username: '', expiresAt: 0 });
}

export function getAuthState() { return authState; }
export function subscribeAuth(listener) {
  authListeners.add(listener);
  return () => authListeners.delete(listener);
}

function isWriteMethod(method) {
  return ['POST', 'PUT', 'PATCH', 'DELETE'].includes(String(method).toUpperCase());
}

function appendQueryValues(query, values = {}) {
  for (const [key, value] of Object.entries(values || {})) {
    if (value != null && String(value).trim() !== '') query.set(key, String(value));
  }
  return query;
}

function runtimeFilterValues(filters = {}) {
  return {
    kind: filters.kind,
    outcome: filters.outcome,
    clientKind: filters.clientKind,
    requestPurpose: filters.requestPurpose,
    requestID: filters.requestID,
    endpointID: filters.endpointID,
    model: filters.model,
    projectID: filters.projectID,
    // The v2 projection stores a stable projectID, while the UI's picker
    // intentionally exposes the redacted/project display name.  Send both
    // forms when available; daemons that understand only projectID keep the
    // stable-ID behavior and newer daemons can use the compatibility alias.
    project: filters.project,
    sessionID: filters.sessionID,
    failureKind: filters.failureKind,
    failurePhase: filters.failurePhase,
    from: filters.from,
    to: filters.to,
  };
}

const thinkingToUI = { disabled: 'disable', passthrough: 'passThrough', adaptive: 'adaptive' };
const thinkingToWire = { disable: 'disabled', passThrough: 'passthrough', adaptive: 'adaptive' };
const contextToUI = { standard: 'passThrough', oneMillion: 'oneMillion', strip: 'strip' };
const contextToWire = { passThrough: 'standard', oneMillion: 'oneMillion', strip: 'strip' };


function normalizeOptionalNumber(value) {
  if (value == null || (typeof value === 'string' && value.trim() === '')) return undefined;
  const number = Number(value);
  return Number.isFinite(number) ? number : undefined;
}

function normalizeCatalog(value) {
  const source = value && typeof value === 'object' ? value : {};
  const models = Array.isArray(source.models)
    ? [...new Set(source.models.map((model) => String(model ?? '').trim()).filter(Boolean))]
    : [];
  return {
    models,
    source: String(source.source ?? ''),
    status: String(source.status ?? ''),
    error: String(source.error ?? ''),
    updatedAt: String(source.updatedAt ?? ''),
  };
}

function normalizeMappingForUI(mapping) {
  const source = mapping || {};
  const {
    clientPattern: _clientPattern,
    upstreamModel: _upstreamModel,
    ...rest
  } = source;
  const timeout = normalizeOptionalNumber(source.failoverTimeoutSeconds);
  return {
    ...rest,
    from: String(source.from ?? source.clientPattern ?? '').trim(),
    to: String(source.to ?? source.upstreamModel ?? '').trim(),
    thinking: thinkingToUI[source.thinking] || source.thinking || 'disable',
    context: contextToUI[source.context] || source.context || 'passThrough',
    ...(timeout == null ? {} : { failoverTimeoutSeconds: timeout }),
  };
}

function normalizeProviderModelsResponse(value, endpointID) {
  const source = value || {};
  return {
    ...source,
    endpointID: String(source.endpointID ?? source.endpointId ?? endpointID ?? ''),
    models: Array.isArray(source.models)
      ? [...new Set(source.models.map((model) => String(model ?? '').trim()).filter(Boolean))]
      : [],
    source: String(source.source ?? ''),
    updatedAt: String(source.updatedAt ?? ''),
  };
}

function normalizeSecretStatus(value) {
  const source = value || {};
  const endpoints = Object.fromEntries(Object.entries(source.endpoints || {}).map(([id, status]) => [
    id,
    status?.apiKey || status || { configured: false, last4: '' },
  ]));
  return { ...source, endpoints };
}

function runtimeEventListItem(event, sequence) {
  return {
    seq: Number(event?.seq ?? sequence ?? 0),
    changeSeq: Number(event?.changeSeq ?? event?.seq ?? sequence ?? 0),
    id: event.id,
    timestamp: event.timestamp,
    kind: event.kind,
    phase: event.phase,
    outcome: event.outcome,
    statusCode: event.statusCode,
    requestID: event.requestID,
    sessionID: event.sessionID ?? event.codexMetadata?.sessionID,
    projectID: event.projectID,
    projectName: event.projectName ?? event.project,
    projectSource: event.projectSource ?? event.project_source,
    localUser: event.localUser ?? event.local_user,
    codexThreadClass: event.codexThreadClass ?? event.codex_thread_class,
    attributionScope: event.attributionScope ?? event.attribution_scope,
    // Keep the mock and older daemon projections compatible with the UI's
    // human-readable project filter while projectID remains the stable key.
    project: event.projectName ?? event.project,
    clientKind: event.clientKind,
    clientModel: event.clientModel,
    requestPurpose: event.requestPurpose,
    featureRuleID: event.featureRuleID,
    endpointID: event.endpointID,
    endpointName: event.endpointName,
    upstreamHost: event.upstreamHost,
    sourceFormat: event.sourceFormat,
    targetFormat: event.targetFormat,
    routeMode: event.routeMode,
    effectiveModel: event.effectiveModel,
    upstreamModel: event.upstreamModel,
    failureKind: event.failureKind,
    failurePhase: event.failurePhase,
    failureDetail: event.failureDetail,
    message: event.message,
    toolCalls: event.toolCalls,
    streamTrace: event.streamTrace,
    timeoutMS: event.timeoutMS,
    upstreamStatusCode: event.upstreamStatusCode,
    upstreamRequestID: event.upstreamRequestID,
    codexMetadata: event.codexMetadata,
    durationMS: event.durationMS,
    ttfbMS: event.ttfbMS,
    failover: Boolean(event.failover),
  };
}

function mockRuntimeEventItems(state) {
  return state.runtimeHistory
    .map((event, index) => runtimeEventListItem(event, state.runtimeHistory.length - index))
    .sort((left, right) => Number(right.seq || 0) - Number(left.seq || 0));
}

// 页面保留易懂的字段名，落盘前严格收敛到 Rust schema v7。
export function fromWireConfig(document) {
  if (!document) return document;
  const wire = clone(document);
  const config = wire.config || {};
  const schemaVersion = Number(config.schemaVersion);
  if (schemaVersion !== 7) {
    throw new TypeError(`仅支持 schema v7 配置，收到 v${config.schemaVersion ?? 'unknown'}`);
  }
  if (Object.prototype.hasOwnProperty.call(config, 'pools')) {
    throw new TypeError('服务端必须先将 Provider 池迁移为扁平 endpoints；WebUI 不执行旧池迁移');
  }
  if (config.listener) delete config.listener.inboundDialectPassthrough;
  for (const rule of config.featureRules || []) {
    if (rule.target && (rule.target.poolID != null || rule.target.poolId != null)) {
      throw new TypeError('schema v7 不允许 featureRules[].target.poolID');
    }
  }
  if (config.retry) delete config.retry.pinnedIPConcurrency;
  for (const endpoint of config.endpoints || []) {
      delete endpoint.pinnedIPs;
      delete endpoint.pinnedIP;
      delete endpoint.pinnedIPExclusive;
      const wireMappings = Array.isArray(endpoint.modelMappings)
        ? endpoint.modelMappings
        : (endpoint.mappings || []);
      endpoint.modelMappings = wireMappings.map(normalizeMappingForUI);

      endpoint.catalog = normalizeCatalog(endpoint.catalog);
      delete endpoint.searchDialect;
      endpoint.baseURL = endpoint.baseURL || '';
      endpoint.protocol = normalizeEndpointProtocol(endpoint.protocol, schemaVersion < 4 ? 'anthropic' : 'auto');
  }
  wire.secretStatus = normalizeSecretStatus(wire.secretStatus);
  return wire;
}

export function toWireConfig(document) {
  const wire = clone(document || {});
  const config = wire.config || wire;
  if (Object.prototype.hasOwnProperty.call(config, 'pools')) {
    throw new TypeError('schema v7 不允许 pools；请先由服务端完成迁移');
  }
  pruneGroupReferences(config);
  delete config.warnings;
  if (config.retry) delete config.retry.pinnedIPConcurrency;
  for (const endpoint of config.endpoints || []) {
      delete endpoint.pinnedIPs;
      delete endpoint.pinnedIP;
      delete endpoint.pinnedIPExclusive;
      const rawProtocol = String(endpoint.protocol || '').trim();
      if (rawProtocol && !isEndpointProtocol(rawProtocol)) {
        throw new TypeError(`入口 ${endpoint.id || '(unknown)'} 的 protocol 非法: ${rawProtocol}`);
      }
      endpoint.protocol = rawProtocol || 'auto';
      const mappings = endpoint.modelMappings || endpoint.mappings || [];
      endpoint.mappings = mappings.map((mapping) => ({
        // UI aliases are authoritative after fromWireConfig.  Fall back to
        // wire names for callers that pass a raw Rust document directly.
        clientPattern: String(mapping.from ?? mapping.clientPattern ?? '').trim(),
        upstreamModel: String(mapping.to ?? mapping.upstreamModel ?? '').trim(),
        thinking: thinkingToWire[mapping.thinking] || 'disabled',
        context: contextToWire[mapping.context] || 'standard',
        ...(mapping.effort && mapping.effort !== 'auto' ? { effort: mapping.effort } : {}),
        ...(Array.isArray(mapping.capabilities) && mapping.capabilities.length > 0
          ? {
              capabilities: [...new Set(
                mapping.capabilities
                  .map((capability) => String(capability).trim().toLowerCase())
                  .filter(Boolean),
              )],
            }
          : {}),
        ...(normalizeOptionalNumber(mapping.failoverTimeoutSeconds) == null
          ? {}
          : { failoverTimeoutSeconds: normalizeOptionalNumber(mapping.failoverTimeoutSeconds) }),
      }));
      delete endpoint.modelMappings;
      delete endpoint.timeoutSeconds;
      delete endpoint.streamIdleTimeoutSeconds;
      delete endpoint.headers;
      delete endpoint.searchDialect;
      if (endpoint.catalog != null) {
        const catalog = normalizeCatalog(endpoint.catalog);
        if (catalog.models.length || catalog.source || catalog.status || catalog.error || catalog.updatedAt) {
          endpoint.catalog = catalog;
        } else {
          delete endpoint.catalog;
        }
      }
      endpoint.apiKey = '';
    }
  config.listener = config.listener || {};
  config.listener.authToken = '';
  delete config.listener.inboundDialectPassthrough;
  config.schemaVersion = 7;
  for (const rule of config.featureRules || []) {
    if (rule.target && (rule.target.poolID != null || rule.target.poolId != null)) {
      throw new TypeError('schema v7 不允许 featureRules[].target.poolID');
    }
    if (rule.target?.protocol && !isSourceFormat(rule.target.protocol)) {
      throw new TypeError(`分流规则 ${rule.id || '(unknown)'} 的目标协议非法: ${rule.target.protocol}`);
    }
    const kind = rule.match?.requestKind;
    if (!['websearch', 'webfetch', 'classifier'].includes(kind)) {
      if (rule.match) delete rule.match.requestKind;
    }
  }
  return document?.config ? config : wire;
}

class ApiService {
  constructor() {
    this.csrfToken = null;
    this.mockState = {
      config: clone(mockConfig),
      secretStatus: clone(mockSecretStatus),
      status: clone(mockStatus),
      runtime: clone(mockRuntime),
      runtimeHistory: clone(mockRuntimeHistory).map((event, index) => ({
        ...event,
        seq: mockRuntimeHistory.length - index,
        changeSeq: mockRuntimeHistory.length - index,
      })),
      runtimeAnalytics: clone(mockAnalytics),
      runtimeRetention: {
        revision: 1,
        maxAgeDays: null,
        storageLimitBytes: null,
      },
      runtimePricing: {
        apiVersion: 3,
        revision: 1,
        currency: 'USD',
        prices: [
          {
            id: 1,
            modelKey: 'claude-opus-5',
            effectiveFrom: 0,
            effectiveTo: null,
            inputPerMillionMicros: 15_000_000,
            outputPerMillionMicros: 75_000_000,
            cacheReadPerMillionMicros: 1_500_000,
            cacheCreationPerMillionMicros: 18_750_000,
          },
          {
            id: 2,
            modelKey: 'gpt-4o',
            effectiveFrom: 0,
            effectiveTo: null,
            inputPerMillionMicros: 2_500_000,
            outputPerMillionMicros: 10_000_000,
            cacheReadPerMillionMicros: 1_250_000,
            cacheCreationPerMillionMicros: null,
          },
        ],
      },
      deletedRuntimeSessions: new Set(),
      diagnostics: clone(mockDiagnostics),
      autostart: clone(mockAutostart),
      runtimeResetGeneration: 0,
      runtimeHistoryGeneration: 1,
      legacyRetentionDetected: true,
      generation: 'gen-local-001',
      session: { authenticated: true, username: 'admin', csrfToken: 'mock-csrf', expiresAt: Math.floor(Date.now() / 1000) + 86400 },
    };
  }

  async request(path, options = {}) {
    if (isMock) {
      return this.handleMock(path, options);
    }

    const method = String(options.method || 'GET').toUpperCase();
    const headers = {
      Accept: 'application/json',
      ...(isWriteMethod(method) ? { 'Content-Type': 'application/json' } : {}),
      ...(csrfToken ? { 'X-Sumpter-CSRF': csrfToken } : {}),
      ...options.headers,
    };

    // JSON body 写端点默认补空对象；runtime reset 也接受真正的空 body，
    // 但统一请求形状有利于旧 daemon/mock 兼容和契约测试。
    const body = isWriteMethod(method) && options.body == null ? '{}' : options.body;

    const response = await fetch(`/admin/api${path}`, {
      ...options,
      method,
      credentials: 'same-origin',
      headers,
      ...(body == null ? {} : { body }),
    });

    const csrfHeader = response.headers.get('X-Sumpter-CSRF');
    if (csrfHeader) csrfToken = csrfHeader;

    const text = await response.text();
    let data = {};
    if (text) {
      try { data = JSON.parse(text); } catch { data = {}; }
    }

    if (!response.ok) {
      if (response.status === 401) markUnauthenticated();
      const error = new Error(data.message || data.error || `HTTP ${response.status}`);
      error.status = response.status;
      error.code = data.error || data.code;
      throw error;
    }

    if (data?.authenticated) setAuthenticated(data);
    return data;
  }

  async handleMock(path, options) {
    await new Promise((r) => setTimeout(r, 120)); // Subtle network simulation
    const method = (options.method || 'GET').toUpperCase();

    if (path === '/status' && method === 'GET') {
      return this.mockState.status;
    }
    if (path === '/config' && method === 'GET') {
      return fromWireConfig({
        generation: this.mockState.generation,
        config: this.mockState.config,
        secretStatus: this.mockState.secretStatus,
      });
    }
    if (path === '/config' && method === 'PUT') {
      const body = JSON.parse(options.body || '{}');
      this.mockState.config = clone(body.config);
      this.mockState.generation = `gen-${Date.now()}`;
      return fromWireConfig({
        generation: this.mockState.generation,
        config: this.mockState.config,
        secretStatus: this.mockState.secretStatus,
      });
    }
    if (path === '/provider-models' && method === 'POST') {
      const body = JSON.parse(options.body || '{}');
      const endpointID = String(body.endpointID ?? body.endpointId ?? '');
      const endpoint = (this.mockState.config.endpoints || []).find((item) => item.id === endpointID);
      if (!endpoint) {
        const error = new Error(endpointID || '入口不存在');
        error.status = 404;
        error.code = 'endpoint_not_found';
        throw error;
      }
      const catalog = normalizeCatalog(endpoint.catalog);
      const models = catalog.models.length
        ? catalog.models
        : [`${endpointID}-model-a`, `${endpointID}-model-b`];
      return {
        endpointID,
        models,
        source: catalog.source || `${String(endpoint.baseURL || '').replace(/\/$/, '')}/v1/models`,
        updatedAt: catalog.updatedAt || String(Math.floor(Date.now() / 1000)),
      };
    }
    if ((path === '/runtime/summary' || path === '/runtime/summary?') && method === 'GET') {
      const counters = { ...this.mockState.runtime };
      delete counters.recentEvents;
      return {
        apiVersion: 1,
        storage: {
          backend: 'sqlite', state: 'ready', pendingEvents: 0,
          eventCount: this.mockState.runtimeHistory.length,
          dbBytes: 16384, walBytes: 0, lastCommitAt: Date.now() / 1000, lastError: null,
        },
        resetGeneration: this.mockState.runtimeResetGeneration,
        counters,
        latestEvent: this.mockState.runtime.recentEvents[0] || null,
      };
    }
    if (path.startsWith('/runtime/request-chain') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const requestID = String(query.get('requestID') || '').trim();
      if (!requestID) {
        const error = new Error('必须提供 requestID'); error.status = 400; error.code = 'request_id_required'; throw error;
      }
      const byID = new Map();
      const events = [
        ...mockRuntimeEventItems(this.mockState),
        ...this.mockState.runtime.recentEvents.map((event) => runtimeEventListItem(event, event.seq)),
      ];
      for (const event of events) {
        if (event.requestID !== requestID) continue;
        const previous = byID.get(event.id);
        if (!previous || Number(event.changeSeq || 0) >= Number(previous.changeSeq || 0)) byID.set(event.id, event);
      }
      return {
        apiVersion: 3,
        requestID,
        events: [...byID.values()].sort((left, right) => Number(left.seq || 0) - Number(right.seq || 0)),
        resetGeneration: this.mockState.runtimeResetGeneration,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
      };
    }
    if (path.startsWith('/runtime/events') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const id = path.match(/^\/runtime\/events\/([^?]+)$/)?.[1];
      if (id) {
        const decodedID = decodeURIComponent(id);
        const event = this.mockState.runtime.recentEvents.find((item) => item.id === decodedID)
          || this.mockState.runtimeHistory.find((item) => item.id === decodedID);
        if (!event) { const error = new Error('运行事件不存在'); error.status = 404; throw error; }
        return { seq: event.seq || 1, changeSeq: event.changeSeq || event.seq || 1, event };
      }
      if (query.get('view') === 'page') {
        const pageSize = mockPageSize(query.get('pageSize'), RUNTIME_DEFAULT_PAGE_SIZE);
        const requestedGeneration = query.get('historyGeneration');
        if (requestedGeneration != null
          && Number(requestedGeneration) !== Number(this.mockState.runtimeHistoryGeneration)) {
          const error = new Error('事件历史快照已失效'); error.status = 409; error.code = 'runtime_snapshot_expired'; throw error;
        }
        const allItems = mockRuntimeEventItems(this.mockState);
        const latestSeq = allItems.reduce((maximum, item) => Math.max(maximum, Number(item.seq || 0)), 0);
        const snapshotSeq = query.get('snapshotSeq') == null
          ? latestSeq
          : Math.max(0, Number(query.get('snapshotSeq')) || 0);
        const timestampBoundary = (value) => {
          const number = Number(value || 0);
          if (!number) return 0;
          return number < 100_000_000_000 ? number * 1000 : number;
        };
        const from = timestampBoundary(query.get('from'));
        const to = timestampBoundary(query.get('to'));
        const exactFilters = {
          kind: query.get('kind'), outcome: query.get('outcome'), clientKind: query.get('clientKind'),
          requestPurpose: query.get('requestPurpose'), requestID: query.get('requestID'),
          endpointID: query.get('endpointID'), projectID: query.get('projectID'),
          project: query.get('project') || query.get('projectName'),
          sessionID: query.get('sessionID'), failureKind: query.get('failureKind'),
          failurePhase: query.get('failurePhase'),
        };
        const model = String(query.get('model') || '').trim();
        const filtered = allItems
          .filter((item) => Number(item.seq || 0) <= snapshotSeq)
          .filter((item) => Object.entries(exactFilters).every(([key, value]) => !value || String(item[key] || '') === value))
          .filter((item) => !model || [item.clientModel, item.effectiveModel, item.upstreamModel].includes(model))
          .filter((item) => !from || Number(item.timestamp || 0) >= from)
          .filter((item) => !to || Number(item.timestamp || 0) <= to);
        const totalCount = filtered.length;
        const totalPages = totalCount ? Math.ceil(totalCount / pageSize) : 0;
        const requestedPage = Math.max(1, Number(query.get('page') || 1));
        const page = totalPages ? Math.min(requestedPage, totalPages) : 1;
        const offset = (page - 1) * pageSize;
        const events = filtered.slice(offset, offset + pageSize);
        return {
          apiVersion: 3,
          events,
          page,
          pageSize,
          totalCount,
          totalPages,
          snapshotSeq,
          historyGeneration: this.mockState.runtimeHistoryGeneration,
          resetGeneration: this.mockState.runtimeResetGeneration,
          hasNext: page < totalPages,
          hasPrevious: page > 1,
          nextCursor: page < totalPages ? events.at(-1)?.seq ?? null : null,
          previousCursor: page > 1 ? events.at(0)?.seq ?? null : null,
          filters: Object.fromEntries([
            ...Object.entries(exactFilters), ['model', model], ['from', query.get('from')], ['to', query.get('to')],
          ].filter(([, value]) => value)),
        };
      }
      const limit = Math.min(200, Math.max(1, Number(query.get('limit') || 10)));
      const items = this.mockState.runtime.recentEvents.map((event, index) => (
        runtimeEventListItem(event, this.mockState.runtime.recentEvents.length - index)
      ));
      const beforeSeq = Number(query.get('beforeSeq') || 0);
      const afterChangeSeq = Number(query.get('afterChangeSeq') || 0);
      const filtered = items
        .filter((item) => !beforeSeq || item.seq < beforeSeq)
        .filter((item) => !afterChangeSeq || item.changeSeq > afterChangeSeq)
        .filter((item) => !query.get('kind') || item.kind === query.get('kind'))
        .filter((item) => !query.get('requestID') || item.requestID === query.get('requestID'))
        .filter((item) => !query.get('outcome') || item.outcome === query.get('outcome'))
        .sort((left, right) => afterChangeSeq ? left.changeSeq - right.changeSeq : right.seq - left.seq);
      return {
        events: filtered.slice(0, limit),
        hasMore: filtered.length > limit,
        resetGeneration: this.mockState.runtimeResetGeneration,
        cursorValid: true,
      };
    }
    if (path.startsWith('/runtime/facets') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const analytics = clone(this.mockState.runtimeAnalytics);
      const selected = {
        clientKinds: String(query.get('clientKind') || '').trim(),
        endpoints: String(query.get('endpointID') || '').trim(),
        projects: String(query.get('project') || query.get('projectID') || '').trim(),
        sessions: String(query.get('sessionID') || '').trim(),
        models: String(query.get('model') || '').trim(),
        requestPurposes: String(query.get('requestPurpose') || '').trim(),
        failureKinds: String(query.get('failureKind') || '').trim(),
        failurePhases: String(query.get('failurePhase') || '').trim(),
      };
      const source = analytics?.facets || {};
      const withSelected = (key) => {
        const rows = Array.isArray(source[key]) ? source[key].map((row) => ({ ...row })) : [];
        if (selected[key] && !rows.some((row) => String(row.value || '') === selected[key])) {
          rows.unshift({ value: selected[key], count: 0 });
        }
        return rows;
      };
      return {
        apiVersion: 3,
        snapshotSeq: this.mockState.runtimeResetGeneration,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
        retainedFromSeq: 0,
        facets: {
          clientKinds: withSelected('clientKinds'),
          endpoints: withSelected('endpoints'),
          projects: withSelected('projects'),
          sessions: withSelected('sessions'),
          models: withSelected('models'),
          requestPurposes: withSelected('requestPurposes'),
          failureKinds: withSelected('failureKinds'),
          failurePhases: withSelected('failurePhases'),
        },
      };
    }
    if (path.startsWith('/runtime/analytics') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const analytics = clone(this.mockState.runtimeAnalytics);
      analytics.range = query.get('range') || '24h';
      const requestedFilters = {
        clientKind: String(query.get('clientKind') || '').trim(),
        endpointID: String(query.get('endpointID') || '').trim(),
        projectID: String(query.get('projectID') || '').trim(),
        project: String(query.get('project') || '').trim(),
        sessionID: String(query.get('sessionID') || '').trim(),
        model: String(query.get('model') || '').trim(),
        requestPurpose: String(query.get('requestPurpose') || '').trim(),
        outcome: String(query.get('outcome') || '').trim(),
        failureKind: String(query.get('failureKind') || '').trim(),
        failurePhase: String(query.get('failurePhase') || '').trim(),
      };
      // Mock deletion intentionally mutates the same aggregate object the UI
      // reads, so deleting a session is visibly reflected in Token usage and
      // facet counts instead of only returning {ok:true}.
      analytics.sessions = analytics.sessions.filter(
        (row) => !this.mockState.deletedRuntimeSessions.has(row.name),
      );
      analytics.facets.sessions = analytics.facets.sessions.filter(
        (row) => !this.mockState.deletedRuntimeSessions.has(row.value),
      );
      const allSessions = analytics.sessions;
      const allProjects = analytics.projects;
      const allEndpoints = analytics.endpoints;
      const projectIDFor = (row) => String(row?.projectID || `project-${row?.name || ''}`);
      const endpointMatches = (row) => (
        !requestedFilters.endpointID
        || String(row?.endpointID || '').trim() === requestedFilters.endpointID
        || (row?.endpointIDs || []).includes(requestedFilters.endpointID)
      );
      const sessionMatches = (row) => (
        (!requestedFilters.sessionID || row.name === requestedFilters.sessionID)
        && (!requestedFilters.projectID || (row.projectIDs || row.projects?.map((name) => `project-${name}`) || []).includes(requestedFilters.projectID))
        && (!requestedFilters.project || (row.projects || []).includes(requestedFilters.project))
        && (!requestedFilters.clientKind || (row.clientKinds || []).includes(requestedFilters.clientKind))
        && endpointMatches(row)
      );
      const projectMatches = (row) => {
        if (requestedFilters.projectID && projectIDFor(row) !== requestedFilters.projectID) return false;
        if (requestedFilters.project && row.name !== requestedFilters.project) return false;
        if (requestedFilters.clientKind && !(row.clientKinds || []).includes(requestedFilters.clientKind)) return false;
        if (!endpointMatches(row)) return false;
        if (requestedFilters.sessionID) {
          const session = allSessions.find((candidate) => candidate.name === requestedFilters.sessionID);
          if (!session || !(session.projects || []).includes(row.name)) return false;
        }
        return true;
      };

      // Each facet ignores its own active filter while respecting the other
      // two. Deriving sessions from filtered ranking rows would hide session
      // B after session A was selected.
      const facetRows = (rows, valueOf) => {
        const counts = new Map();
        for (const row of rows) {
          for (const value of valueOf(row)) {
            counts.set(value, (counts.get(value) || 0) + Number(row.attempts || 0));
          }
        }
        return [...counts.entries()].map(([value, count]) => ({ value, count }));
      };
      const sessionFacetRows = allSessions.filter((row) => (
        (!requestedFilters.projectID || (row.projectIDs || row.projects?.map((name) => `project-${name}`) || []).includes(requestedFilters.projectID))
        && (!requestedFilters.project || (row.projects || []).includes(requestedFilters.project))
        && (!requestedFilters.clientKind || (row.clientKinds || []).includes(requestedFilters.clientKind))
        && endpointMatches(row)
      ));
      const projectFacetRows = requestedFilters.sessionID
        ? allSessions
          .filter((row) => row.name === requestedFilters.sessionID)
          .flatMap((row) => (row.projects || [])
            .map((name) => ({ name, projectID: `project-${name}`, attempts: row.attempts }))
            .filter((project) => !requestedFilters.projectID || project.projectID === requestedFilters.projectID))
        : allProjects.filter((row) => (
          (!requestedFilters.projectID || projectIDFor(row) === requestedFilters.projectID)
          && (!requestedFilters.clientKind || (row.clientKinds || []).includes(requestedFilters.clientKind))
          && endpointMatches(row)
        ));
      const clientFacetRows = allSessions.filter((row) => (
        (!requestedFilters.projectID || (row.projectIDs || row.projects?.map((name) => `project-${name}`) || []).includes(requestedFilters.projectID))
        && (!requestedFilters.project || (row.projects || []).includes(requestedFilters.project))
        && (!requestedFilters.sessionID || row.name === requestedFilters.sessionID)
        && endpointMatches(row)
      ));
      const withSelected = (items, selected) => {
        const result = [...items];
        if (selected && !result.some((item) => item.value === selected)) result.push({ value: selected, count: 0 });
        return result;
      };
      analytics.facets = {
        clientKinds: withSelected(facetRows(clientFacetRows, (row) => row.clientKinds || []), requestedFilters.clientKind),
        projects: withSelected(facetRows(projectFacetRows, (row) => [row.name]), requestedFilters.project),
        sessions: withSelected(facetRows(sessionFacetRows, (row) => [row.name]), requestedFilters.sessionID),
        endpoints: withSelected(facetRows(allEndpoints, (row) => [row.endpointID]), requestedFilters.endpointID),
      };
      const allClientValues = new Set([
        ...(analytics.facets.clientKinds || []).map((row) => row.value),
        ...allSessions.flatMap((row) => row.clientKinds || []),
        ...allProjects.flatMap((row) => row.clientKinds || []),
      ]);
      for (const value of allClientValues) {
        if (!analytics.facets.clientKinds.some((item) => item.value === value)) {
          analytics.facets.clientKinds.push({ value, count: 0 });
        }
      }
      const warnings = [];
      if (requestedFilters.clientKind && !analytics.facets.clientKinds.some((row) => row.value === requestedFilters.clientKind)) {
        warnings.push(`客户端筛选值不存在：${requestedFilters.clientKind}`);
      }
      if (requestedFilters.project && !analytics.facets.projects.some((row) => row.value === requestedFilters.project)) {
        warnings.push(`项目筛选值不存在：${requestedFilters.project}`);
      }
      if (requestedFilters.sessionID && !analytics.facets.sessions.some((row) => row.value === requestedFilters.sessionID)) {
        warnings.push(`会话筛选值不存在：${requestedFilters.sessionID}`);
      }
      if (requestedFilters.endpointID && !analytics.facets.endpoints.some((row) => row.value === requestedFilters.endpointID)) {
        warnings.push(`入口筛选值不存在：${requestedFilters.endpointID}`);
      }
      analytics.projects = analytics.projects.filter(projectMatches);
      analytics.sessions = analytics.sessions.filter(sessionMatches);
      analytics.endpoints = analytics.endpoints.filter(endpointMatches);
      // Keep summary counters and token cards aligned with the selected
      // dimensions. The mock rows are intentionally explicit so this path
      // exercises the same AND semantics as the Rust analytics endpoint.
      if (requestedFilters.clientKind || requestedFilters.endpointID || requestedFilters.projectID || requestedFilters.project || requestedFilters.sessionID || requestedFilters.model || requestedFilters.requestPurpose || requestedFilters.outcome || requestedFilters.failureKind || requestedFilters.failurePhase) {
        // Session rows are the finest mock grain. Use them when a session is
        // selected (or when a client filter can be represented by sessions),
        // otherwise fall back to the project rows. This keeps the preview's
        // counters and token cards scoped together instead of leaving the
        // unfiltered totals on screen after a picker change.
        const sessionScope = analytics.sessions.filter(sessionMatches);
        const scopedRows = requestedFilters.sessionID || (!requestedFilters.project && requestedFilters.clientKind)
          ? sessionScope
          : requestedFilters.endpointID && !requestedFilters.project && !requestedFilters.projectID
            ? analytics.endpoints
            : analytics.projects.filter(projectMatches);
        const sum = (key) => scopedRows.reduce((total, row) => total + Number(row[key] || 0), 0);
        analytics.clientRequests = sum('attempts');
        analytics.clientSuccesses = sum('successes');
        analytics.clientFailures = sum('failures');
        analytics.clientCancelled = sum('cancelled');
        analytics.clientPending = sum('pending');
        analytics.failovers = sum('failovers');
        const completed = analytics.clientSuccesses + analytics.clientFailures + analytics.clientCancelled;
        analytics.clientSuccessRate = completed ? (analytics.clientSuccesses / completed) * 100 : null;
        const tokenKeys = ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens',
          'reasoningTokens', 'uncachedInputTokens', 'processedInputTokens', 'processedTotalTokens', 'totalTokens', 'observedRequests'];
        const presenceKeys = ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens', 'reasoningTokens'];
        analytics.tokenUsage = {
          ...analytics.tokenUsage,
          ...Object.fromEntries(tokenKeys.map((key) => [key, sum(key)])),
          usageFieldPresence: Object.fromEntries(presenceKeys.map((key) => [key, scopedRows.reduce((total, row) => total + Number(row.usageFieldPresence?.[key] || 0), 0)])),
          tokenAccountingSemantics: [...new Set(scopedRows.map((row) => row.tokenAccountingSemantics).filter(Boolean))].join(',') || 'unknown',
          tokenAccountingQuality: [...new Set(scopedRows.map((row) => row.tokenAccountingQuality).filter(Boolean))].join(',') || 'unknown',
        };
      }
      analytics.filtersApplied = true;
      analytics.filterWarning = warnings.length ? warnings.join('；') : null;
      analytics.appliedFilters = {
        clientKind: requestedFilters.clientKind,
        endpointID: requestedFilters.endpointID,
        project: requestedFilters.project,
        sessionID: requestedFilters.sessionID,
        ...(requestedFilters.model ? { model: requestedFilters.model } : {}),
        ...(requestedFilters.requestPurpose ? { requestPurpose: requestedFilters.requestPurpose } : {}),
        ...(requestedFilters.outcome ? { outcome: requestedFilters.outcome } : {}),
        ...(requestedFilters.failureKind ? { failureKind: requestedFilters.failureKind } : {}),
        ...(requestedFilters.failurePhase ? { failurePhase: requestedFilters.failurePhase } : {}),
        ...(requestedFilters.projectID ? { projectID: requestedFilters.projectID } : {}),
      };
      return analytics;
    }
    if (path.startsWith('/runtime/trends') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const range = query.get('range') || '24h';
      const trendFilters = {
        clientKind: String(query.get('clientKind') || '').trim(),
        endpointID: String(query.get('endpointID') || '').trim(),
        projectID: String(query.get('projectID') || '').trim(),
        project: String(query.get('project') || '').trim(),
        sessionID: String(query.get('sessionID') || '').trim(),
      };
      const endpointScale = {
        'ep-anthropic-direct': 0.50,
        'ep-azure-eastus': 0.35,
        'ep-openrouter-fast': 0.24,
        'ep-deepseek-backup': 0.12,
      }[trendFilters.endpointID] || 1;
      const filterScale = Math.max(0.08, endpointScale
        * (trendFilters.project || trendFilters.projectID ? 0.66 : 1)
        * (trendFilters.sessionID ? 0.42 : 1)
        * (trendFilters.clientKind ? 0.74 : 1));
      const bucketCount = range === '1h' ? 12 : range === '7d' ? 14 : range === '30d' ? 30 : 24;
      const bucketSeconds = range === '1h' ? 300 : range === '7d' ? 43_200 : range === '30d' ? 86_400 : 3_600;
      const appleNow = (Date.now() / 1000) - 978_307_200;
      const points = Array.from({ length: bucketCount }, (_, index) => {
        const requests = Math.max(0, Math.round((24 + ((index * 17) % 52)) * filterScale));
        const failures = index % 7 === 4 ? 6 : index % 5;
        const scaledFailures = Math.min(requests, Math.max(0, Math.round(failures * filterScale)));
        const cancelled = Math.min(Math.max(0, requests - scaledFailures), index % 9 === 0 ? Math.max(1, Math.round(filterScale)) : 0);
        const successes = Math.max(0, requests - scaledFailures - cancelled);
        const eligible = requests > 0 ? Math.floor(requests * 0.72) : 0;
        const cacheTokenRate = index % 8 === 7 ? null : 0.32 + ((index % 4) * 0.06);
        return {
          bucketStart: appleNow - ((bucketCount - index) * bucketSeconds),
          bucketEnd: appleNow - ((bucketCount - index - 1) * bucketSeconds),
          clientRequests: requests,
          clientSuccesses: successes,
          clientFailures: scaledFailures,
          clientCancelled: cancelled,
          failovers: index % 6 === 3 ? 3 : index % 2,
          tokens: {
            inputTokens: requests * 1_480,
            outputTokens: requests * 410,
            cacheReadInputTokens: cacheTokenRate == null ? 0 : Math.round(requests * 1_480 * cacheTokenRate),
            cacheCreationInputTokens: requests * 70,
            reasoningTokens: requests * 40,
            uncachedInputTokens: requests * 900,
            processedInputTokens: requests * 1_480,
            processedTotalTokens: requests * 1_890,
            observedRequests: requests,
            accountingKnownRequests: eligible,
            accountingUnknownRequests: requests - eligible,
            cacheReadReportedRequests: eligible,
            cacheReadHitRequests: Math.floor(eligible * 0.68),
            cacheReadTokenEligibleRequests: cacheTokenRate == null ? 0 : eligible,
            cacheReadTokenUnknownRequests: cacheTokenRate == null ? requests : requests - eligible,
            cacheReadTokenRate: cacheTokenRate,
            cacheReadRequestRate: cacheTokenRate == null ? null : 0.68,
            usageFieldPresence: {
              inputTokens: requests, outputTokens: requests,
              cacheReadInputTokens: cacheTokenRate == null ? 0 : eligible,
              cacheCreationInputTokens: eligible, reasoningTokens: Math.floor(requests / 3),
            },
          },
          ttfbMS: {
            observedRequests: requests,
            sumMS: requests * (460 + index * 6),
            averageMS: 460 + index * 6,
            thresholdBuckets: [
              { thresholdMS: 5000, exceededRequests: index % 5 },
              { thresholdMS: 15000, exceededRequests: index % 9 === 0 ? 1 : 0 },
            ],
          },
          durationMS: {
            observedRequests: requests,
            sumMS: requests * (1_420 + index * 18),
            averageMS: 1_420 + index * 18,
            thresholdBuckets: [
              { thresholdMS: 3000, exceededRequests: Math.min(requests, scaledFailures + (index % 4)) },
              { thresholdMS: 6000, exceededRequests: index % 6 === 2 ? 2 : index % 3 },
            ],
          },
          cost: {
            estimatedCostMicros: requests * 7_500,
            pricedRequests: Math.floor(requests * 0.78),
            unpricedRequests: Math.floor(requests * 0.12),
            unknownAccountingRequests: requests - Math.floor(requests * 0.78) - Math.floor(requests * 0.12),
            complete: false,
            currency: 'USD',
            priceVersion: 1,
          },
        };
      });
      const sum = (pick) => points.reduce((total, point) => total + Number(pick(point) || 0), 0);
      const last = points.at(-1);
      return {
        apiVersion: 3,
        rollupUsed: range !== '1h',
        granularity: range === '30d' ? 'day' : 'hour',
        from: points[0]?.bucketStart || appleNow,
        to: last?.bucketEnd || appleNow,
        snapshotSeq: this.mockState.runtimeHistory.length,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
        retainedFromSeq: 1,
        points,
        totals: {
          bucketStart: points[0]?.bucketStart || appleNow,
          bucketEnd: last?.bucketEnd || appleNow,
          clientRequests: sum((point) => point.clientRequests),
          clientSuccesses: sum((point) => point.clientSuccesses),
          clientFailures: sum((point) => point.clientFailures),
          clientCancelled: sum((point) => point.clientCancelled),
          failovers: sum((point) => point.failovers),
          tokens: {
            inputTokens: sum((point) => point.tokens.inputTokens),
            outputTokens: sum((point) => point.tokens.outputTokens),
            cacheReadInputTokens: sum((point) => point.tokens.cacheReadInputTokens),
            cacheCreationInputTokens: sum((point) => point.tokens.cacheCreationInputTokens),
            reasoningTokens: sum((point) => point.tokens.reasoningTokens),
            uncachedInputTokens: sum((point) => point.tokens.uncachedInputTokens),
            processedInputTokens: sum((point) => point.tokens.processedInputTokens),
            processedTotalTokens: sum((point) => point.tokens.processedTotalTokens),
            observedRequests: sum((point) => point.tokens.observedRequests),
            accountingKnownRequests: sum((point) => point.tokens.accountingKnownRequests),
            accountingUnknownRequests: sum((point) => point.tokens.accountingUnknownRequests),
            cacheReadReportedRequests: sum((point) => point.tokens.cacheReadReportedRequests),
            cacheReadHitRequests: sum((point) => point.tokens.cacheReadHitRequests),
            cacheReadTokenEligibleRequests: sum((point) => point.tokens.cacheReadTokenEligibleRequests),
            cacheReadTokenUnknownRequests: sum((point) => point.tokens.cacheReadTokenUnknownRequests),
            cacheReadTokenRate: 0.43,
            cacheReadRequestRate: 0.68,
            usageFieldPresence: {
              inputTokens: sum((point) => point.tokens.usageFieldPresence.inputTokens),
              outputTokens: sum((point) => point.tokens.usageFieldPresence.outputTokens),
              cacheReadInputTokens: sum((point) => point.tokens.usageFieldPresence.cacheReadInputTokens),
              cacheCreationInputTokens: sum((point) => point.tokens.usageFieldPresence.cacheCreationInputTokens),
              reasoningTokens: sum((point) => point.tokens.usageFieldPresence.reasoningTokens),
            },
          },
          ttfbMS: {
            observedRequests: sum((point) => point.ttfbMS.observedRequests),
            sumMS: sum((point) => point.ttfbMS.sumMS),
            averageMS: sum((point) => point.ttfbMS.sumMS) / Math.max(1, sum((point) => point.ttfbMS.observedRequests)),
            thresholdBuckets: [
              { thresholdMS: 5000, exceededRequests: sum((point) => point.ttfbMS.thresholdBuckets[0].exceededRequests) },
              { thresholdMS: 15000, exceededRequests: sum((point) => point.ttfbMS.thresholdBuckets[1].exceededRequests) },
            ],
          },
          durationMS: {
            observedRequests: sum((point) => point.durationMS.observedRequests),
            sumMS: sum((point) => point.durationMS.sumMS),
            averageMS: sum((point) => point.durationMS.sumMS) / Math.max(1, sum((point) => point.durationMS.observedRequests)),
            thresholdBuckets: [
              { thresholdMS: 3000, exceededRequests: sum((point) => point.durationMS.thresholdBuckets[0].exceededRequests) },
              { thresholdMS: 6000, exceededRequests: sum((point) => point.durationMS.thresholdBuckets[1].exceededRequests) },
            ],
          },
          cost: {
            estimatedCostMicros: sum((point) => point.cost.estimatedCostMicros),
            pricedRequests: sum((point) => point.cost.pricedRequests),
            unpricedRequests: sum((point) => point.cost.unpricedRequests),
            unknownAccountingRequests: sum((point) => point.cost.unknownAccountingRequests),
            complete: false,
            currency: 'USD',
            priceVersion: 1,
          },
        },
        thresholds: { ttfbMS: [5000, 15000], durationMS: [3000, 6000] },
        filters: trendFilters,
      };
    }
    if (path.startsWith('/runtime/errors') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const pageSize = mockPageSize(query.get('pageSize'), RUNTIME_DEFAULT_PAGE_SIZE);
      const requestedPage = Math.max(1, Math.trunc(Number(query.get('page') || 1) || 1));
      const errorEndpointID = String(query.get('endpointID') || '').trim();
      const groups = [
        ['timeout', 'before_headers', 'ep-anthropic-direct', 'Anthropic 官方直连', 'claude-opus-5', null, 18],
        ['upstream_status', 'response_headers', 'ep-openrouter-fast', 'OpenRouter 聚合加速', 'gpt-5.4', 429, 11],
        ['stream_interrupted', 'response_stream', 'ep-azure-eastus', 'Azure OpenAI 通道', 'gpt-4o', 200, 7],
        ['connect', 'connect', 'ep-deepseek-backup', 'DeepSeek 应急通道', 'deepseek-chat', null, 4],
      ].map(([failureKind, failurePhase, endpointID, endpointName, model, upstreamStatusCode, occurrences], index) => ({
        failureKind, failurePhase, endpointID, endpointName, model, upstreamStatusCode,
        occurrences, affectedRequests: Math.max(1, occurrences - 2), affectedSessions: Math.max(1, Math.floor(occurrences / 3)),
        recoveredAfterFailover: Math.floor(occurrences / 2), firstSeen: ((Date.now() / 1000) - 978_307_200) - ((index + 1) * 25_000),
        lastSeen: ((Date.now() / 1000) - 978_307_200) - (index * 1_800),
        sampleEventIDs: [`ev-error-${index + 1}`, `ev-error-${index + 1}-sample`],
      }));
      const filteredGroups = groups.filter((group) => !errorEndpointID || group.endpointID === errorEndpointID);
      const totalCount = filteredGroups.length;
      const totalPages = totalCount ? Math.ceil(totalCount / pageSize) : 0;
      const page = totalPages ? Math.min(requestedPage, totalPages) : 1;
      const offset = (page - 1) * pageSize;
      return {
        apiVersion: 3, groups: filteredGroups.slice(offset, offset + pageSize), page, pageSize, totalCount, totalPages,
        snapshotSeq: this.mockState.runtimeHistory.length, historyGeneration: this.mockState.runtimeHistoryGeneration,
        retainedFromSeq: 1, hasNext: page < totalPages, hasPrevious: page > 1 && totalPages > 0,
        filters: { endpointID: errorEndpointID },
      };
    }
    if ((path.startsWith('/runtime/projects') || path.startsWith('/runtime/sessions') || path.startsWith('/runtime/dimensions')) && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const kind = path.startsWith('/runtime/dimensions')
        ? String(query.get('kind') || '')
        : (path.startsWith('/runtime/projects') ? 'project' : 'session');
      const dimensionRows = {
        endpoint: this.mockState.runtimeAnalytics.endpoints,
        model: this.mockState.runtimeAnalytics.models,
        clientKind: this.mockState.runtimeAnalytics.clientKinds,
        purpose: this.mockState.runtimeAnalytics.requestPurposes,
        failureKind: this.mockState.runtimeAnalytics.failureKinds,
        failurePhase: this.mockState.runtimeAnalytics.failurePhases,
        protocol: this.mockState.runtimeAnalytics.protocolRoutes,
        streamTerminal: this.mockState.runtimeAnalytics.streamTerminals,
      };
      const sourceRows = kind === 'project'
        ? this.mockState.runtimeAnalytics.projects
        : kind === 'session'
          ? this.mockState.runtimeAnalytics.sessions
          : (dimensionRows[kind] || []).length
            ? dimensionRows[kind]
            : this.mockState.runtimeAnalytics.projects;
      const dimensionEndpointID = String(query.get('endpointID') || '').trim();
      const dimensionProjectID = String(query.get('projectID') || '').trim();
      const dimensionProject = String(query.get('project') || '').trim();
      const dimensionSessionID = String(query.get('sessionID') || '').trim();
      const endpointScopedRows = dimensionEndpointID
        ? sourceRows.filter((row) => String(row?.endpointID || '') === dimensionEndpointID
          || (row?.endpointIDs || []).includes(dimensionEndpointID))
        : sourceRows;
      const projectScopedRows = dimensionProjectID
        ? endpointScopedRows.filter((row) => (row?.projectIDs || []).includes(dimensionProjectID))
        : dimensionProject
          ? endpointScopedRows.filter((row) => (row?.projects || []).includes(dimensionProject))
          : endpointScopedRows;
      const effectiveSourceRows = dimensionSessionID
        ? projectScopedRows.filter((row) => (row?.sessionIDs || []).includes(dimensionSessionID))
        : projectScopedRows;
      const search = String(query.get('search') || '').trim().toLowerCase();
      const sort = query.get('sort') || 'last_seen';
      const order = query.get('order') === 'asc' ? 1 : -1;
      const pageSize = mockPageSize(query.get('pageSize'), RUNTIME_DEFAULT_PAGE_SIZE);
      const page = Math.max(1, Math.trunc(Number(query.get('page') || 1) || 1));
      const expandedLength = kind === 'project'
        ? (effectiveSourceRows.length ? 64 : 0)
        : kind === 'session'
          ? (effectiveSourceRows.length ? 148 : 0)
          : kind === 'model'
            ? effectiveSourceRows.length
          : (effectiveSourceRows.length ? Math.max(12, effectiveSourceRows.length) : 0);
      const expanded = Array.from({ length: expandedLength }, (_, index) => {
        const template = effectiveSourceRows[index % effectiveSourceRows.length];
        const name = index < sourceRows.length ? template.name : `${kind}-demo-${String(index + 1).padStart(3, '0')}`;
        // Dimension row keys are sent back as projectID/sessionID filters when
        // a user drills down. Keep the first real rows aligned with the
        // stable IDs used by the runtime wire contract; synthetic ordinal keys
        // make the mock appear empty after a perfectly valid row click.
        const stableKey = kind === 'project'
          ? template.projectID || template.name
          : kind === 'session'
            ? template.sessionID || template.name
            : kind === 'endpoint'
              ? template.endpointID || template.name
              : kind === 'model'
                ? template.modelKey || template.name
                : template[`${kind}ID`] || template.name;
        const key = index < sourceRows.length ? stableKey : `${kind}-demo-${String(index + 1).padStart(3, '0')}`;
        const requests = Math.max(1, Number(template.attempts || 1) - index * 3);
        const inputTokens = Number(template.inputTokens || 0);
        const cacheReadInputTokens = Number(template.cacheReadInputTokens || 0);
        const processedInputTokens = Number(template.processedInputTokens || 0);
        const cacheReadReportedRequests = Number(template.cacheReadReportedRequests ?? template.usageFieldPresence?.cacheReadInputTokens ?? 0);
        const cacheReadHitRequests = Number(template.cacheReadHitRequests ?? (cacheReadReportedRequests > 0 ? cacheReadReportedRequests : 0));
        const cacheReadTokenEligibleRequests = Number(template.cacheReadTokenEligibleRequests ?? (template.tokenAccountingSemantics && processedInputTokens > 0 ? cacheReadReportedRequests : 0));
        const cacheReadTokenUnknownRequests = Number(template.cacheReadTokenUnknownRequests ?? Math.max(0, Number(template.observedRequests || 0) - cacheReadTokenEligibleRequests));
        return {
          key,
          name,
          source: template.projectSource || (kind === 'project' ? 'workspace_local' : kind === 'session' ? 'session_id' : kind),
          requests,
          successes: Math.max(0, requests - (index % 5)), failures: index % 5, cancelled: index % 9 === 0 ? 1 : 0,
          failovers: index % 4, inputTokens, outputTokens: Number(template.outputTokens || 0),
          cacheReadInputTokens, cacheCreationInputTokens: Number(template.cacheCreationInputTokens || 0),
          processedInputTokens, processedTotalTokens: Number(template.processedTotalTokens || 0),
          cacheReadReportedRequests, cacheReadHitRequests,
          cacheReadTokenEligibleRequests, cacheReadTokenUnknownRequests,
          cacheReadTokenRate: template.cacheReadTokenRate ?? (cacheReadTokenEligibleRequests > 0 && processedInputTokens > 0 ? cacheReadInputTokens / processedInputTokens : null),
          cacheReadRequestRate: template.cacheReadRequestRate ?? (cacheReadReportedRequests > 0 ? cacheReadHitRequests / cacheReadReportedRequests : null),
          firstSeen: ((Date.now() / 1000) - 978_307_200) - (index + 1) * 86_400,
          lastSeen: ((Date.now() / 1000) - 978_307_200) - index * 3_600,
          averageDurationMS: Number(template.averageDurationMS || 0), averageTTFBMS: Number(template.averageTTFBMS || 0),
          relatedCount: kind === 'project' ? 2 + (index % 8) : 1 + (index % 3),
          workspacePaths: kind === 'project' ? (template.workspacePaths || [`.../.claude/${name}`]) : [],
          clientKinds: Array.isArray(template.clientKinds) ? template.clientKinds : [],
          tokenAccountingSemantics: template.tokenAccountingSemantics,
          tokenAccountingQuality: template.tokenAccountingQuality,
        };
      }).filter((row) => !search || row.name.toLowerCase().includes(search));
      const sortValues = {
        name: (row) => row.name,
        requests: (row) => row.requests,
        success_rate: (row) => {
          const completed = Number(row.successes || 0) + Number(row.failures || 0) + Number(row.cancelled || 0);
          return completed > 0 ? Number(row.successes || 0) / completed : -1;
        },
        failures: (row) => row.failures,
        input_tokens: (row) => row.inputTokens,
        output_tokens: (row) => row.outputTokens,
        cache_read: (row) => row.cacheReadInputTokens,
        cache_write: (row) => row.cacheCreationInputTokens,
        tokens: (row) => row.processedTotalTokens,
        average_duration: (row) => row.averageDurationMS,
        last_seen: (row) => row.lastSeen,
      };
      const pick = sortValues[sort] || sortValues.last_seen;
      expanded.sort((left, right) => {
        const a = pick(left); const b = pick(right);
        return (typeof a === 'string' ? a.localeCompare(b) : a - b) * order || left.key.localeCompare(right.key);
      });
      const totalCount = expanded.length;
      const totalPages = totalCount ? Math.ceil(totalCount / pageSize) : 0;
      const offset = (Math.min(page, totalPages || 1) - 1) * pageSize;
      return {
        apiVersion: 3, kind, rows: expanded.slice(offset, offset + pageSize), page: Math.min(page, totalPages || 1), pageSize,
        totalCount, totalPages, snapshotSeq: this.mockState.runtimeHistory.length,
        historyGeneration: this.mockState.runtimeHistoryGeneration, retainedFromSeq: 1,
        hasNext: page < totalPages, hasPrevious: page > 1, search: search || null, sort, order: order === 1 ? 'asc' : 'desc', filters: { endpointID: dimensionEndpointID },
      };
    }
    if (path === '/runtime/storage' && method === 'GET') {
      const retained = this.mockState.runtimeHistory.length;
      return {
        apiVersion: 3, backend: 'sqlite', schemaVersion: 3, projectionVersion: 3,
        projectionBackfillCursor: retained, projectionBackfillComplete: true, projectionIndexesReady: true,
        missingIndexes: [], hourlyRollupComplete: true, hourlyRollupMaxSeq: retained,
        hourlyRollupHistoryGeneration: this.mockState.runtimeHistoryGeneration, hourlyRollupFailed: false,
        hourlyRollupDirtyBuckets: 0, retainedEvents: retained, completedEvents: retained, inFlightEvents: 1,
        minSeq: 1, maxSeq: retained, earliestTimestamp: ((Date.now() / 1000) - 978_307_200) - 604_800,
        latestTimestamp: (Date.now() / 1000) - 978_307_200, retainedFromSeq: 1,
        historyGeneration: this.mockState.runtimeHistoryGeneration, resetGeneration: this.mockState.runtimeResetGeneration,
        userDeletedEvents: 12, userDeletedRequests: 6,
        payloadBytes: 1_843_200, databaseBytes: 4_194_304, liveBytes: 2_621_440, allocatedBytes: 4_194_304,
        freelistBytes: 524_288, walBytes: 131_072, pendingEvents: 2, pendingBytes: 2_048,
        retention: clone(this.mockState.runtimeRetention),
        legacyRetentionDetected: this.mockState.legacyRetentionDetected,
      };
    }
    if (path === '/runtime/retention' && method === 'GET') return clone(this.mockState.runtimeRetention);
    if (path === '/runtime/retention' && method === 'PUT') {
      const body = JSON.parse(options.body || '{}');
      if (Number(body.expectedRevision) !== Number(this.mockState.runtimeRetention.revision)) {
        const error = new Error('保留策略版本已更新'); error.status = 409; error.code = 'runtime_revision_conflict'; throw error;
      }
      const allowedKeys = new Set(['expectedRevision', 'maxAgeDays', 'storageLimitBytes']);
      if (Object.keys(body).some((key) => !allowedKeys.has(key))) {
        const error = new Error('存储设置包含已废弃字段');
        error.status = 400;
        error.code = 'invalid_json';
        throw error;
      }
      const maxAgeDays = body.maxAgeDays ?? null;
      if (maxAgeDays != null && (!Number.isSafeInteger(maxAgeDays) || maxAgeDays < 1)) {
        const error = new Error('maxAgeDays 必须为空或至少为 1 天');
        error.status = 400;
        error.code = 'invalid_retention';
        throw error;
      }
      const storageLimitBytes = body.storageLimitBytes ?? null;
      if (storageLimitBytes != null && (!Number.isSafeInteger(storageLimitBytes) || storageLimitBytes < 1024 * 1024)) {
        const error = new Error('storageLimitBytes 必须为空或至少为 1 MiB');
        error.status = 400;
        error.code = 'invalid_retention';
        throw error;
      }
      this.mockState.runtimeRetention = {
        ...this.mockState.runtimeRetention,
        revision: this.mockState.runtimeRetention.revision + 1,
        maxAgeDays,
        storageLimitBytes,
      };
      return clone(this.mockState.runtimeRetention);
    }
    if ((path === '/runtime/cleanup/preview' || path === '/runtime/cleanup') && method === 'POST') {
      const body = JSON.parse(options.body || '{}');
      const olderThan = Number(body.olderThan);
      if (!Number.isFinite(olderThan) || olderThan <= 0) {
        const error = new Error('olderThan 必须是大于 0 的有限时间戳'); error.status = 400; error.code = 'invalid_cleanup'; throw error;
      }
      // Mock history uses JavaScript milliseconds while the real wire contract
      // uses Apple reference-date seconds. Normalize only for this fixture.
      const toSeconds = (value) => Number(value) > 1e11 ? Number(value) / 1000 - 978307200 : Number(value);
      const selected = this.mockState.runtimeHistory.filter((event) => toSeconds(event.timestamp) < olderThan);
      const preview = {
        olderThan,
        deletableEvents: selected.length,
        deletableRequests: new Set(selected.filter((event) => event.kind === 'client').map((event) => event.requestID || event.id)).size,
        remainingEvents: this.mockState.runtimeHistory.length - selected.length,
      };
      if (path === '/runtime/cleanup/preview') return preview;
      this.mockState.runtimeHistory = this.mockState.runtimeHistory.filter((event) => !selected.includes(event));
      this.mockState.runtimeHistoryGeneration += selected.length ? 1 : 0;
      this.mockState.runtimeAnalytics = clone(mockAnalytics);
      return {
        ...preview,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
      };
    }
    if (path === '/runtime/pricing' && method === 'GET') return clone(this.mockState.runtimePricing);
    if (path === '/runtime/pricing' && method === 'PUT') {
      const body = JSON.parse(options.body || '{}');
      if (Number(body.expectedRevision) !== Number(this.mockState.runtimePricing.revision)) {
        const error = new Error('价格版本已更新'); error.status = 409; error.code = 'runtime_revision_conflict'; throw error;
      }
      this.mockState.runtimePricing = {
        apiVersion: 3,
        revision: this.mockState.runtimePricing.revision + 1,
        currency: body.currency,
        prices: (body.prices || []).map((price, index) => ({ ...price, id: index + 1 })),
      };
      return { revision: this.mockState.runtimePricing.revision, currency: body.currency, priceCount: body.prices.length };
    }
    if (path.startsWith('/runtime/export/estimate') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const scope = query.get('scope') || 'events';
      const rowCount = scope === 'events' ? this.mockState.runtimeHistory.length : scope === 'projects' ? 64 : 148;
      return {
        apiVersion: 3, scope, format: query.get('format') || 'jsonl', privacy: query.get('privacy') || 'stored',
        privacyScope: query.get('privacy') === 'stored'
          ? 'SQLite 已存的源事件与会话字段；不含诊断捕获正文、Headers 或凭据'
          : '标识符二次稳定脱敏；不含诊断捕获正文、Headers 或凭据',
        rowCount, estimatedBytes: rowCount * (query.get('format') === 'csv' ? 192 : 256),
        snapshotSeq: this.mockState.runtimeHistory.length, historyGeneration: this.mockState.runtimeHistoryGeneration,
        retainedFromSeq: 1,
      };
    }
    if (path.startsWith('/runtime/session/export') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const sessionID = String(query.get('sessionID') || '').trim();
      if (!sessionID) {
        const error = new Error('必须提供完整 sessionID'); error.status = 400; error.code = 'session_id_required'; throw error;
      }
      const session = this.mockState.runtimeAnalytics.sessions.find((row) => row.name === sessionID);
      if (!session || this.mockState.deletedRuntimeSessions.has(sessionID)) {
        const error = new Error('会话不存在'); error.status = 404; error.code = 'session_not_found'; throw error;
      }
      return {
        format: 'sumpter-session-export-v1',
        exportedAt: Date.now() / 1000,
        sessionID,
        projects: session.projects || [],
        clientKinds: session.clientKinds || [],
        eventCount: session.attempts,
        analytics: {
          clientRequests: session.attempts,
          clientSuccesses: session.successes,
          clientFailures: session.failures,
          clientCancelled: session.cancelled,
          clientPending: session.pending || 0,
          upstreamAttempts: 0,
          upstreamSuccesses: 0,
          upstreamFailures: 0,
          failovers: session.failovers,
          tokenUsage: {
            inputTokens: session.inputTokens,
            outputTokens: session.outputTokens,
            cacheReadInputTokens: session.cacheReadInputTokens,
            cacheCreationInputTokens: session.cacheCreationInputTokens,
            processedInputTokens: session.processedInputTokens,
            processedTotalTokens: session.processedTotalTokens,
            totalTokens: session.totalTokens,
            observedRequests: session.observedRequests,
          },
          endpoints: [],
        },
        events: [],
      };
    }
    if (path === '/runtime/projects/sticky-clear' && method === 'POST') {
      const body = JSON.parse(options.body || '{}');
      const projectID = String(body.projectID ?? body.project_id ?? '').trim();
      if (!projectID) {
        const error = new Error('必须提供 projectID'); error.status = 400; error.code = 'project_id_required'; throw error;
      }
      // Mock 把每个项目记为一条粘性归属,首次清除返回 1,之后幂等返回 0。
      if (!this.mockState.clearedProjectSticky) this.mockState.clearedProjectSticky = new Set();
      const cleared = this.mockState.clearedProjectSticky.has(projectID) ? 0 : 1;
      this.mockState.clearedProjectSticky.add(projectID);
      return { cleared, matched: cleared };
    }
    if (path.startsWith('/runtime/session') && method === 'DELETE') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      const sessionID = String(query.get('sessionID') || '').trim();
      if (!sessionID) {
        const error = new Error('必须提供完整 sessionID'); error.status = 400; error.code = 'session_id_required'; throw error;
      }
      if (sessionID === 'unidentified_session' && new URLSearchParams(path.split('?')[1] || '').get('confirmUnidentified') !== 'true') {
        const error = new Error('不能删除未识别会话；请输入 DELETE 并携带 confirmUnidentified=true'); error.status = 400; error.code = 'session_not_deletable'; throw error;
      }
      const session = this.mockState.runtimeAnalytics.sessions.find((row) => row.name === sessionID);
      if (!session || this.mockState.deletedRuntimeSessions.has(sessionID)) {
        const error = new Error('会话不存在'); error.status = 404; error.code = 'session_not_found'; throw error;
      }
      this.mockState.deletedRuntimeSessions.add(sessionID);
      this.mockState.runtime.recentEvents = this.mockState.runtime.recentEvents.filter((event) => (
        event.sessionID !== sessionID && event.codexMetadata?.sessionID !== sessionID
      ));
      this.mockState.runtimeHistory = this.mockState.runtimeHistory.filter((event) => (
        event.sessionID !== sessionID && event.codexMetadata?.sessionID !== sessionID
      ));
      this.mockState.runtimeHistoryGeneration += 1;
      const analytics = this.mockState.runtimeAnalytics;
      const subtract = (target, source, keys) => {
        keys.forEach((key) => {
          target[key] = Math.max(0, Number(target[key] || 0) - Number(source[key] || 0));
        });
        const completed = Number(target.successes || 0) + Number(target.failures || 0) + Number(target.cancelled || 0);
        target.pending = Math.max(0, Number(target.attempts || 0) - completed);
        target.successRate = completed ? (Number(target.successes || 0) / completed) * 100 : null;
      };
      const counterDeltas = {
        clientRequests: 'attempts', clientSuccesses: 'successes', clientFailures: 'failures',
        clientCancelled: 'cancelled', failovers: 'failovers',
      };
      Object.entries(counterDeltas).forEach(([targetKey, sourceKey]) => {
        analytics[targetKey] = Math.max(0, Number(analytics[targetKey] || 0) - Number(session[sourceKey] || 0));
        if (Object.prototype.hasOwnProperty.call(this.mockState.runtime, targetKey)) {
          this.mockState.runtime[targetKey] = Math.max(0, Number(this.mockState.runtime[targetKey] || 0) - Number(session[sourceKey] || 0));
        }
      });
      analytics.clientPending = Math.max(0, analytics.clientRequests - analytics.clientSuccesses - analytics.clientFailures - analytics.clientCancelled);
      analytics.clientSuccessRate = (analytics.clientSuccesses + analytics.clientFailures + analytics.clientCancelled)
        ? (analytics.clientSuccesses / (analytics.clientSuccesses + analytics.clientFailures + analytics.clientCancelled)) * 100 : null;
      (session.projects || []).forEach((projectName) => {
        const project = analytics.projects.find((row) => row.name === projectName);
        if (project) subtract(project, session, ['attempts', 'successes', 'failures', 'cancelled', 'failovers',
          'inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens', 'reasoningTokens',
          'uncachedInputTokens', 'processedInputTokens', 'processedTotalTokens', 'totalTokens', 'observedRequests']);
      });
      const usageKeys = ['inputTokens', 'outputTokens', 'cacheReadInputTokens', 'cacheCreationInputTokens',
        'reasoningTokens', 'uncachedInputTokens', 'processedInputTokens', 'processedTotalTokens', 'totalTokens', 'observedRequests'];
      usageKeys.forEach((key) => {
        analytics.tokenUsage[key] = Math.max(0, Number(analytics.tokenUsage[key] || 0) - Number(session[key] || 0));
      });
      analytics.facets.projects.forEach((facet) => {
        const project = analytics.projects.find((row) => row.name === facet.value);
        if (project && (session.projects || []).includes(facet.value)) facet.count = project.attempts;
      });
      (session.clientKinds || []).forEach((kind) => {
        const facet = analytics.facets.clientKinds.find((row) => row.value === kind);
        if (facet) facet.count = Math.max(0, facet.count - session.attempts);
      });
      return {
        resetGeneration: ++this.mockState.runtimeResetGeneration,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
        deletedEvents: session.attempts,
        deletedRequests: session.attempts,
      };
    }
    if (path === '/proxy/start' && method === 'POST') {
      this.mockState.status.running = true;
      this.mockState.status.health = { state: 'healthy', headline: '代理已启动并监听 127.0.0.1:57878' };
      return { running: true };
    }
    if (path === '/proxy/stop' && method === 'POST') {
      this.mockState.status.running = false;
      this.mockState.status.health = { state: 'stopped', headline: '代理已停止' };
      return { running: false };
    }
    if (path === '/runtime/reset' && method === 'POST') {
      this.mockState.runtimeResetGeneration += 1;
      this.mockState.runtimeHistoryGeneration += 1;
      this.mockState.runtime = {
        clientRequests: 0,
        clientSuccesses: 0,
        clientFailures: 0,
        upstreamAttempts: 0,
        upstreamSuccesses: 0,
        upstreamFailures: 0,
        failovers: 0,
        recentEvents: [],
      };
      this.mockState.runtimeHistory = [];
      this.mockState.runtimeAnalytics = clone(mockAnalytics);
      this.mockState.deletedRuntimeSessions.clear();
      return {
        reset: true,
        resetGeneration: this.mockState.runtimeResetGeneration,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
      };
    }
    if (path === '/runtime/recreate' && method === 'POST') {
      this.mockState.runtimeResetGeneration += 1;
      this.mockState.runtimeHistoryGeneration += 1;
      this.mockState.runtime = {
        clientRequests: 0,
        clientSuccesses: 0,
        clientFailures: 0,
        upstreamAttempts: 0,
        upstreamSuccesses: 0,
        upstreamFailures: 0,
        failovers: 0,
        recentEvents: [],
      };
      this.mockState.runtimeHistory = [];
      this.mockState.runtimeAnalytics = clone(mockAnalytics);
      this.mockState.deletedRuntimeSessions.clear();
      this.mockState.legacyRetentionDetected = false;
      return {
        reset: true,
        recreated: true,
        resetGeneration: this.mockState.runtimeResetGeneration,
        historyGeneration: this.mockState.runtimeHistoryGeneration,
      };
    }
    if (path === '/diagnostics' && method === 'GET') {
      // Keep the environment endpoint small as in the real daemon. Capture
      // metadata/details are served through their dedicated lazy-load routes.
      const { capture: _capture, captureDetails: _captureDetails, ...diagnostics } = this.mockState.diagnostics;
      return diagnostics;
    }
    if (path.startsWith('/diagnostic-capture/export') && method === 'GET') {
      const query = new URLSearchParams(path.split('?')[1] || '');
      return {
        started: true,
        scope: query.get('scope') || 'selected',
        format: query.get('format') || 'jsonl',
        privacy: query.get('privacy') || 'raw',
        recordCount: query.get('scope') === 'all' ? this.mockState.diagnostics.capture.recordCount : 1,
      };
    }
    if (path === '/diagnostic-capture' && method === 'GET') {
      const capture = this.mockState.diagnostics.capture;
      return {
        ...capture,
        records: capture.records.map(({ inboundBody: _inboundBody, ...record }) => record),
      };
    }
    if (path.startsWith('/diagnostic-capture/') && method === 'GET') {
      const requestID = decodeURIComponent(path.slice('/diagnostic-capture/'.length));
      const detail = this.mockState.diagnostics.captureDetails?.[requestID];
      if (!detail) {
        const error = new Error('未找到抓包请求'); error.status = 404; error.code = 'capture_not_found'; throw error;
      }
      return detail;
    }
    if (path === '/diagnostic-capture' && method === 'PUT') {
      const body = JSON.parse(options.body || '{}');
      const capture = this.mockState.diagnostics.capture;
      capture.enabled = Boolean(body.enabled);
      if (capture.enabled && Number.isSafeInteger(body.maxBytes) && body.maxBytes > 0) capture.maxBytes = body.maxBytes;
      capture.startedAt = capture.enabled ? Date.now() / 1000 : capture.startedAt;
      capture.stopReason = capture.enabled ? null : 'manual';
      return {
        ...capture,
        records: capture.records.map(({ inboundBody: _inboundBody, ...record }) => record),
      };
    }
    if (path === '/diagnostic-capture' && method === 'DELETE') {
      const capture = this.mockState.diagnostics.capture;
      capture.records = [];
      capture.capturedBytes = 0;
      capture.limitReached = false;
      capture.stopReason = null;
      this.mockState.diagnostics.captureDetails = {};
      return { cleared: true };
    }
    if (path === '/autostart' && method === 'GET') {
      return this.mockState.autostart;
    }
    if (path === '/autostart' && method === 'PUT') {
      const body = JSON.parse(options.body || '{}');
      this.mockState.autostart.enabled = Boolean(body.enabled);
      return this.mockState.autostart;
    }
    if (path.startsWith('/endpoint-secret') && method === 'GET') {
      return { endpointID: 'mock', apiKey: 'sk-mock-live-decrypted-key-9988', configured: true };
    }
    if (path === '/auth/session' && method === 'GET') {
      return this.mockState.session;
    }
    if (path === '/auth/login' && method === 'POST') {
      const body = JSON.parse(options.body || '{}');
      if (!body.username || !body.password) {
        const error = new Error('请输入用户名和密码'); error.status = 401; throw error;
      }
      return this.mockState.session;
    }
    if (path === '/auth/logout' && method === 'POST') {
      this.mockState.session = { authenticated: false, username: '', expiresAt: 0 };
      return this.mockState.session;
    }

    return { ok: true };
  }

  // High-level API calls
  getStatus(options = {}) { return this.request('/status', options); }
  getRuntimeSummary(options = {}) { return this.request('/runtime/summary', options); }
  getRuntimeEvents({ beforeSeq, afterChangeSeq, limit = 10, kind, requestID, outcome, from, to, signal } = {}) {
    const query = new URLSearchParams();
    if (beforeSeq != null) query.set('beforeSeq', String(beforeSeq));
    if (afterChangeSeq != null) query.set('afterChangeSeq', String(afterChangeSeq));
    if (limit != null) query.set('limit', String(Math.min(200, Math.max(1, limit))));
    if (kind) query.set('kind', kind);
    if (requestID) query.set('requestID', requestID);
    if (outcome) query.set('outcome', outcome);
    if (from) query.set('from', from);
    if (to) query.set('to', to);
    return this.request(`/runtime/events${query.toString() ? `?${query}` : ''}`, { signal });
  }
  getRuntimeEventPage({
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, snapshotSeq, historyGeneration,
    kind, outcome, clientKind, requestPurpose, requestID, endpointID,
    model, projectID, project, sessionID, failureKind, failurePhase, from, to, signal,
  } = {}) {
    const query = new URLSearchParams({
      view: 'page',
      page: String(Math.max(1, Number(page) || 1)),
      pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE),
    });
    const filters = {
      kind, outcome, clientKind, requestPurpose, requestID, endpointID,
      model, projectID, project, sessionID, failureKind, failurePhase, from, to,
    };
    if (snapshotSeq != null) query.set('snapshotSeq', String(snapshotSeq));
    if (historyGeneration != null) query.set('historyGeneration', String(historyGeneration));
    for (const [key, value] of Object.entries(filters)) {
      if (value != null && String(value).trim() !== '') query.set(key, String(value));
    }
    return this.request(`/runtime/events?${query.toString()}`, { signal });
  }
  getRuntimeEvent(id, options = {}) { return this.request(`/runtime/events/${encodeURIComponent(id)}`, options); }
  getRuntimeRequestChain(requestID, options = {}) {
    const query = new URLSearchParams({ requestID: String(requestID || '') });
    return this.request(`/runtime/request-chain?${query.toString()}`, options);
  }
  getRuntimeAnalytics(range = '24h', filters = {}, options = {}) {
    const query = new URLSearchParams({ range });
    if (filters.clientKind) query.set('clientKind', filters.clientKind);
    if (filters.endpointID) query.set('endpointID', filters.endpointID);
    if (filters.projectID) query.set('projectID', filters.projectID);
    if (filters.project) query.set('project', filters.project);
    if (filters.sessionID) query.set('sessionID', filters.sessionID);
    if (filters.model) query.set('model', filters.model);
    if (filters.requestPurpose) query.set('requestPurpose', filters.requestPurpose);
    if (filters.outcome) query.set('outcome', filters.outcome);
    if (filters.failureKind) query.set('failureKind', filters.failureKind);
    if (filters.failurePhase) query.set('failurePhase', filters.failurePhase);
    appendQueryValues(query, { ...runtimeRangeFilter(range), from: filters.from, to: filters.to });
    return this.request(`/runtime/analytics?${query.toString()}`, options);
  }
  getRuntimeTrends({ range = '24h', granularity = 'auto', snapshotSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ range, granularity });
    appendQueryValues(query, { snapshotSeq, historyGeneration, ...runtimeRangeFilter(range), ...runtimeFilterValues(filters) });
    return this.request(`/runtime/trends?${query.toString()}`, options);
  }
  getRuntimeErrors({ page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, snapshotSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE) });
    appendQueryValues(query, { snapshotSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/errors?${query.toString()}`, options);
  }
  getRuntimeDimension(kind, {
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, search, sort = 'last_seen', order = 'desc', snapshotSeq, historyGeneration, filters = {},
  } = {}, options = {}) {
    if (!['projects', 'sessions'].includes(kind)) throw new TypeError(`不支持的 runtime 维度: ${kind}`);
    const query = new URLSearchParams({ page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE), sort, order });
    appendQueryValues(query, { search, snapshotSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/${kind}?${query.toString()}`, options);
  }
  getRuntimeDimensions(kind, {
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, search, sort = 'last_seen', order = 'desc', snapshotSeq, historyGeneration, filters = {},
  } = {}, options = {}) {
    const allowed = ['endpoint', 'model', 'clientKind', 'purpose', 'failureKind', 'failurePhase', 'protocol', 'streamTerminal', 'project', 'session'];
    if (!allowed.includes(kind)) throw new TypeError(`不支持的 runtime 维度: ${kind}`);
    const query = new URLSearchParams({ kind, page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE), sort, order });
    appendQueryValues(query, { search, snapshotSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/dimensions?${query.toString()}`, options);
  }
  // One endpoint/transaction returns all picker dimensions.  The old
  // four-request implementation opened four SQLite connections and could
  // race itself during automatic refreshes.
  async getRuntimeFacets(range = '24h', filters = {}, options = {}) {
    const query = new URLSearchParams({ range: String(range || '24h') });
    appendQueryValues(query, { ...runtimeRangeFilter(range), ...runtimeFilterValues(filters) });
    const value = await this.request(`/runtime/facets?${query.toString()}`, options);
    const facets = value?.facets || {};
    return {
      ...value,
      apiVersion: Number(value?.apiVersion || 3),
      facets: {
        clientKinds: Array.isArray(facets.clientKinds) ? facets.clientKinds : [],
        endpoints: Array.isArray(facets.endpoints) ? facets.endpoints : [],
        projects: Array.isArray(facets.projects) ? facets.projects : [],
        sessions: Array.isArray(facets.sessions) ? facets.sessions : [],
        models: Array.isArray(facets.models) ? facets.models : [],
        requestPurposes: Array.isArray(facets.requestPurposes) ? facets.requestPurposes : [],
        failureKinds: Array.isArray(facets.failureKinds) ? facets.failureKinds : [],
        failurePhases: Array.isArray(facets.failurePhases) ? facets.failurePhases : [],
      },
    };
  }
  getRuntimeStorage(options = {}) { return this.request('/runtime/storage', options); }
  getRuntimeRetention(options = {}) { return this.request('/runtime/retention', options); }
  updateRuntimeRetention(payload, options = {}) {
    return this.request('/runtime/retention', {
      ...options,
      method: 'PUT',
      body: JSON.stringify(payload || {}),
    });
  }
  previewRuntimeCleanup(payload, options = {}) {
    return this.request('/runtime/cleanup/preview', {
      ...options,
      method: 'POST',
      body: JSON.stringify(payload || {}),
    });
  }
  cleanupRuntime(payload, options = {}) {
    return this.request('/runtime/cleanup', {
      ...options,
      method: 'POST',
      body: JSON.stringify(payload || {}),
    });
  }
  getRuntimePricing(options = {}) { return this.request('/runtime/pricing', options); }
  updateRuntimePricing(payload, options = {}) {
    return this.request('/runtime/pricing', {
      ...options,
      method: 'PUT',
      body: JSON.stringify(payload || {}),
    });
  }
  estimateRuntimeExport({ scope = 'events', format = 'jsonl', privacy = 'stored', confirmStored = false, snapshotSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ scope, format, privacy });
    if (confirmStored) query.set('confirmStored', 'true');
    appendQueryValues(query, { snapshotSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/export/estimate?${query.toString()}`, options);
  }
  downloadRuntimeExport({ scope = 'events', format = 'jsonl', privacy = 'stored', confirmStored = false, snapshotSeq, historyGeneration, filters = {} } = {}) {
    const query = new URLSearchParams({ scope, format, privacy });
    if (confirmStored) query.set('confirmStored', 'true');
    appendQueryValues(query, { snapshotSeq, historyGeneration, ...runtimeFilterValues(filters) });
    const path = `/runtime/export?${query.toString()}`;
    if (isMock) {
      return this.estimateRuntimeExport({ scope, format, privacy, confirmStored, snapshotSeq, historyGeneration, filters })
        .then((estimate) => ({ started: true, mock: true, estimate }));
    }
    // The server returns an attachment stream. A temporary same-origin link
    // lets the browser consume the bounded stream directly without replacing
    // the admin page; do not fetch it into a response string or construct a
    // giant Blob in the UI.
    if (typeof document !== 'undefined') {
      const anchor = document.createElement('a');
      anchor.href = `/admin/api${path}`;
      anchor.download = '';
      anchor.rel = 'noopener';
      anchor.style.display = 'none';
      document.body?.appendChild(anchor);
      anchor.click();
      anchor.remove();
    }
    return Promise.resolve({ started: true });
  }
  deleteRuntimeSession(sessionID, options = {}) {
    const query = new URLSearchParams({ sessionID: String(sessionID || '') });
    if (options.confirmUnidentified === true) query.set('confirmUnidentified', 'true');
    return this.request(`/runtime/session?${query.toString()}`, { ...options, method: 'DELETE' });
  }
  exportRuntimeSession(sessionID, options = {}) {
    const query = new URLSearchParams({ sessionID: String(sessionID || '') });
    return this.request(`/runtime/session/export?${query.toString()}`, options);
  }
  clearProjectSticky(projectID, options = {}) {
    return this.request('/runtime/projects/sticky-clear', {
      ...options,
      method: 'POST',
      body: JSON.stringify({ projectID: String(projectID || '') }),
    });
  }
  getConfig(options = {}) { return this.request('/config', options).then(fromWireConfig); }
  saveConfig(expectedGeneration, config, secretUpdates = {}) {
    return this.request('/config', {
      method: 'PUT',
      body: JSON.stringify({
        expectedGeneration,
        config: toWireConfig(config),
        secretUpdates: {
          inboundAuthToken: secretUpdates.inboundAuthToken,
          endpoints: Object.fromEntries(Object.entries(secretUpdates)
            .filter(([id]) => id !== 'inboundAuthToken')
            .map(([id, apiKey]) => [id, { apiKey }])),
        },
      }),
    }).then(fromWireConfig).catch((error) => {
      error.message = configSaveMessage(error.code, error.message);
      throw error;
    });
  }
  fetchProviderModels(endpointID, options = {}) {
    const id = String(endpointID ?? '').trim();
    return this.request('/provider-models', {
      ...options,
      method: 'POST',
      body: JSON.stringify({ endpointID: id }),
    }).then((value) => normalizeProviderModelsResponse(value, id));
  }
  // Kept as a small compatibility alias for callers that used the endpoint
  // name before the UI standardized on fetchProviderModels.
  providerModels(endpointID) { return this.fetchProviderModels(endpointID); }
  toggleProxy(start) {
    return this.request(start ? '/proxy/start' : '/proxy/stop', { method: 'POST' });
  }
  resetRuntime() { return this.request('/runtime/reset', { method: 'POST' }); }
  recreateRuntime() { return this.request('/runtime/recreate', { method: 'POST' }); }
  getDiagnostics(options = {}) { return this.request('/diagnostics', options); }
  getDiagnosticCapture(options = {}) { return this.request('/diagnostic-capture', options); }
  getDiagnosticCaptureDetail(requestID, options = {}) {
    return this.request(`/diagnostic-capture/${encodeURIComponent(requestID)}`, options);
  }
  downloadDiagnosticCapture({ scope = 'selected', format = 'jsonl', privacy = 'raw', confirmRaw = false, requestID = '' } = {}) {
    const query = new URLSearchParams({ scope, format, privacy });
    if (confirmRaw) query.set('confirmRaw', 'true');
    if (requestID) query.set('requestID', requestID);
    const path = `/diagnostic-capture/export?${query.toString()}`;
    if (isMock) return this.request(path);
    const anchor = document.createElement('a');
    anchor.href = `/admin/api${path}`;
    anchor.rel = 'noopener';
    anchor.download = `sumpter-diagnostic-${scope}-${privacy}-${Date.now()}.${format === 'jsonl' ? 'jsonl' : 'json'}`;
    document.body.appendChild(anchor);
    anchor.click();
    anchor.remove();
    return Promise.resolve({ started: true, path });
  }
  setDiagnosticCapture(enabled, maxBytes, options = {}) {
    return this.request('/diagnostic-capture', {
      ...options,
      method: 'PUT',
      body: JSON.stringify({ enabled, ...(maxBytes == null ? {} : { maxBytes }) }),
    });
  }
  clearDiagnosticCapture(options = {}) {
    return this.request('/diagnostic-capture', { ...options, method: 'DELETE' });
  }
  getAutostart() { return this.request('/autostart'); }
  setAutostart(enabled) {
    return this.request('/autostart', { method: 'PUT', body: JSON.stringify({ enabled }) });
  }
  getEndpointSecret(endpointID) {
    return this.request(`/endpoint-secret?endpointID=${encodeURIComponent(endpointID)}`);
  }
  getAuthSession() {
    return this.request('/auth/session').then((value) => {
      if (value?.authenticated) setAuthenticated(value); else markUnauthenticated();
      return value;
    });
  }
  login(username, password) {
    return this.request('/auth/login', { method: 'POST', body: JSON.stringify({ username, password }) })
      .then((value) => { if (value?.authenticated) setAuthenticated(value); return value; });
  }
  logout() {
    return this.request('/auth/logout', { method: 'POST' }).finally(markUnauthenticated);
  }
  changeCredentials(currentPassword, username, newPassword) {
    return this.request('/auth/credentials', {
      method: 'PUT',
      body: JSON.stringify({ currentPassword, username, newPassword }),
    });
  }
}

export const api = new ApiService();

export function openEventStream({ onRuntimeChange, onRuntimeEvent, onConfigReloaded, onConfigMigrated, onStatsReset, onProxyState, onStatus } = {}) {
  if (isMock) {
    let timer = null;
    onStatus?.('open');
    return { close() { if (timer) clearInterval(timer); onStatus?.('closed'); } };
  }
  let closed = false;
  let source = null;
  let reconnectTimer = null;
  let retryDelay = 1000;
  const parse = (event, callback) => {
    if (closed || typeof event?.data !== 'string') return;
    try { callback?.(JSON.parse(event.data)); } catch { /* ignore malformed frames */ }
  };
  const schedule = () => {
    if (closed) return;
    onStatus?.('retrying', { delayMS: retryDelay });
    reconnectTimer = setTimeout(() => { reconnectTimer = null; connect(); }, retryDelay);
    retryDelay = Math.min(retryDelay * 2, 15000);
  };
  const connect = () => {
    if (closed) return;
    onStatus?.('connecting');
    source = new EventSource('/admin/api/events', { withCredentials: true });
    const runtime = (event) => parse(event, onRuntimeChange || onRuntimeEvent);
    source.addEventListener('runtime-change', runtime);
    source.addEventListener('config-reloaded', (event) => parse(event, onConfigReloaded));
    source.addEventListener('config_reloaded', (event) => parse(event, onConfigReloaded));
    source.addEventListener('config-migrated', (event) => parse(event, onConfigMigrated));
    source.addEventListener('config_migrated', (event) => parse(event, onConfigMigrated));
    source.addEventListener('stats-reset', (event) => parse(event, onStatsReset));
    source.addEventListener('stats_reset', (event) => parse(event, onStatsReset));
    source.addEventListener('proxy-state', (event) => parse(event, onProxyState));
    source.addEventListener('proxy_state', (event) => parse(event, onProxyState));
    source.onmessage = runtime;
    source.onopen = () => { retryDelay = 1000; onStatus?.('open'); };
    source.onerror = async () => {
      if (closed) return;
      source?.close(); source = null;
      try {
        const session = await api.getAuthSession();
        if (!session?.authenticated) return onStatus?.('auth-required');
        schedule();
      } catch (error) {
        if (error.status === 401) { markUnauthenticated(); onStatus?.('auth-required'); }
        else schedule();
      }
    };
  };
  connect();
  return { close() { closed = true; if (reconnectTimer) clearTimeout(reconnectTimer); source?.close(); source = null; onStatus?.('closed'); } };
}
