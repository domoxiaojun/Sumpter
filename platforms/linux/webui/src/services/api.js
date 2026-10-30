import { configSaveMessage } from '../utils/configFeedback.js';
import { normalizeUserAgentSettings } from '../utils/userAgent.js';
import { pruneGroupReferences } from '../utils/modelGroups.js';

import { clone } from '../utils/helpers.js';
import {
  isEndpointProtocol,
  isSourceFormat,
  normalizeEndpointProtocol,
} from '../utils/protocols.js';
import { RUNTIME_DEFAULT_PAGE_SIZE, runtimeRangeFilter } from '../utils/runtimeAnalyticsV2.js';

const isMock = new URLSearchParams(window.location.search).get('mock') === '1';

export function normalizeResolveIP(value) {
  return String(value ?? '').trim();
}

export function isValidResolveIP(value) {
  const candidate = normalizeResolveIP(value);
  if (!candidate) return true;
  const octets = candidate.split('.');
  if (octets.length === 4 && octets.every((octet) => /^\d+$/.test(octet))) {
    return octets.every((octet) => Number(octet) >= 0 && Number(octet) <= 255);
  }
  if (!candidate.includes(':') || candidate.includes('[') || candidate.includes(']')) return false;
  try {
    new URL(`http://[${candidate}]/`);
    return true;
  } catch {
    return false;
  }
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
    clientVariant: filters.clientVariant,
    agentRole: filters.agentRole,
    agentName: filters.agentName,
    parentThreadID: filters.parentThreadID,
    parentTurnID: filters.parentTurnID,
    rootTurnID: filters.rootTurnID,
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
    attemptedAt: String(source.attemptedAt ?? ''),
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
      endpoint.capabilities = [...new Set(
        (Array.isArray(endpoint.capabilities) ? endpoint.capabilities : [])
          .map((capability) => String(capability).trim().toLowerCase())
          .filter(Boolean),
      )];
      delete endpoint.searchDialect;
      endpoint.baseURL = endpoint.baseURL || '';
      endpoint.resolveIP = normalizeResolveIP(endpoint.resolveIP);
      if (endpoint.resolveIP && !isValidResolveIP(endpoint.resolveIP)) {
        throw new TypeError(`入口 ${endpoint.id || '(unknown)'} 的 resolveIP 不是合法 IPv4/IPv6 地址`);
      }
      if (!endpoint.resolveIP) delete endpoint.resolveIP;
      endpoint.protocol = normalizeEndpointProtocol(endpoint.protocol, schemaVersion < 4 ? 'anthropic' : 'auto');
      const normalizedUserAgent = normalizeUserAgentSettings(endpoint.userAgent);
      if (Object.keys(normalizedUserAgent).length > 0) endpoint.userAgent = normalizedUserAgent;
      else delete endpoint.userAgent;
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
      endpoint.resolveIP = normalizeResolveIP(endpoint.resolveIP);
      if (endpoint.resolveIP && !isValidResolveIP(endpoint.resolveIP)) {
        throw new TypeError(`入口 ${endpoint.id || '(unknown)'} 的 resolveIP 不是合法 IPv4/IPv6 地址`);
      }
      if (!endpoint.resolveIP) delete endpoint.resolveIP;
      const endpointCapabilities = [...new Set(
        (Array.isArray(endpoint.capabilities) ? endpoint.capabilities : [])
          .map((capability) => String(capability).trim().toLowerCase())
          .filter(Boolean),
      )];
      if (endpointCapabilities.length > 0) endpoint.capabilities = endpointCapabilities;
      else delete endpoint.capabilities;
      const normalizedUserAgent = normalizeUserAgentSettings(endpoint.userAgent);
      if (Object.keys(normalizedUserAgent).length > 0) endpoint.userAgent = normalizedUserAgent;
      else delete endpoint.userAgent;
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
        if (catalog.models.length || catalog.source || catalog.status || catalog.error || catalog.attemptedAt || catalog.updatedAt) {
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

// Shared validation also runs in the editor before attempting a save.

class ApiService {


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
    this.mockApiPromise ||= import('./mockApi.js').then(({ createMockApi }) =>
      createMockApi({ fromWireConfig, normalizeCatalog }));
    return (await this.mockApiPromise).handleMock(path, options);
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
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, snapshotSeq, snapshotChangeSeq, historyGeneration,
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
    if (snapshotChangeSeq != null) query.set('snapshotChangeSeq', String(snapshotChangeSeq));
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
  getRuntimeAnalytics(range = '24h', filters = {}, { diagnosticsOnly = false, ...options } = {}) {
    const query = new URLSearchParams({ range });
    if (diagnosticsOnly) query.set('view', 'diagnostics');
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
  getRuntimeTrends({ range = '24h', granularity = 'auto', snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ range, granularity });
    appendQueryValues(query, { snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeRangeFilter(range), ...runtimeFilterValues(filters) });
    return this.request(`/runtime/trends?${query.toString()}`, options);
  }
  getRuntimeErrors({ page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE) });
    appendQueryValues(query, { snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/errors?${query.toString()}`, options);
  }
  getRuntimeDimension(kind, {
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, search, sort = 'last_seen', order = 'desc', snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {},
  } = {}, options = {}) {
    if (!['projects', 'sessions'].includes(kind)) throw new TypeError(`不支持的 runtime 维度: ${kind}`);
    const query = new URLSearchParams({ page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE), sort, order });
    appendQueryValues(query, { search, snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/${kind}?${query.toString()}`, options);
  }
  getRuntimeDimensions(kind, {
    page = 1, pageSize = RUNTIME_DEFAULT_PAGE_SIZE, search, sort = 'last_seen', order = 'desc', snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {},
  } = {}, options = {}) {
    const allowed = ['endpoint', 'model', 'clientKind', 'purpose', 'failureKind', 'failurePhase', 'protocol', 'streamTerminal', 'project', 'session', 'clientVariant', 'agentRole', 'agentName', 'parentThread', 'parentTurn', 'rootTurn'];
    if (!allowed.includes(kind)) throw new TypeError(`不支持的 runtime 维度: ${kind}`);
    const query = new URLSearchParams({ kind, page: String(Math.max(1, Number(page) || 1)), pageSize: String(Number(pageSize) || RUNTIME_DEFAULT_PAGE_SIZE), sort, order });
    appendQueryValues(query, { search, snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeFilterValues(filters) });
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
  estimateRuntimeExport({ scope = 'events', format = 'jsonl', privacy = 'stored', confirmStored = false, snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {} } = {}, options = {}) {
    const query = new URLSearchParams({ scope, format, privacy });
    if (confirmStored) query.set('confirmStored', 'true');
    appendQueryValues(query, { snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeFilterValues(filters) });
    return this.request(`/runtime/export/estimate?${query.toString()}`, options);
  }
  downloadRuntimeExport({ scope = 'events', format = 'jsonl', privacy = 'stored', confirmStored = false, snapshotSeq, snapshotChangeSeq, historyGeneration, filters = {} } = {}) {
    const query = new URLSearchParams({ scope, format, privacy });
    if (confirmStored) query.set('confirmStored', 'true');
    appendQueryValues(query, { snapshotSeq, snapshotChangeSeq, historyGeneration, ...runtimeFilterValues(filters) });
    const path = `/runtime/export?${query.toString()}`;
    if (isMock) {
      return this.estimateRuntimeExport({ scope, format, privacy, confirmStored, snapshotSeq, snapshotChangeSeq, historyGeneration, filters })
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
  clearSessionSticky(sessionID, options = {}) {
    return this.request('/runtime/sessions/sticky-clear', {
      ...options,
      method: 'POST',
      body: JSON.stringify({ sessionID: String(sessionID || '') }),
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
  getModelCatalogStatus(options = {}) { return this.request('/model-catalog/status', options); }
  refreshModelCatalog() {
    return this.request('/model-catalog/status', { method: 'POST' });
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

export function openEventStream({ onRuntimeChange, onRuntimeEvent, onConfigReloaded, onConfigMigrated, onStatsReset, onProxyState, onModelCatalogUpdated, onModelMetadataUpdated, onStatus } = {}) {
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
    source.addEventListener('model-catalog-updated', (event) => parse(event, onModelCatalogUpdated));
    source.addEventListener('model-metadata-updated', (event) => parse(event, onModelMetadataUpdated));
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
