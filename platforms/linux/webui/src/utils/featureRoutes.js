export const BUILTIN_RULE_IDS = new Set(['websearch', 'webfetch', 'classifier']);

export const isBuiltinRule = (rule) => BUILTIN_RULE_IDS.has(String(rule?.id || '').toLowerCase());

export function routeModelChoices(endpoints, endpointID, catalog) {
  const scoped = endpointID ? endpoints.filter((endpoint) => endpoint.id === endpointID) : endpoints.filter((endpoint) => endpoint.enabled !== false);
  const names = scoped.flatMap((endpoint) => [
    ...(endpointID ? (catalog?.models ?? endpoint.catalog?.models ?? []) : []),
    ...(endpoint.modelMappings || endpoint.mappings || []).map((mapping) => mapping.from ?? mapping.clientPattern),
  ]);
  return [...new Set(names.map((name) => String(name ?? '').trim()).filter((name) => name && !name.includes('*')))].sort();
}

export function routeCatalogKey(endpoint) {
  return JSON.stringify([endpoint.id, endpoint.baseURL, endpoint.protocol, endpoint.headers, endpoint.pinnedIP, endpoint.pinnedIPExclusive]);
}

// Cache only successful discoveries. Aborted/outdated probes must never become
// the next editor's suggestions. Catalog reads do not trigger config hot reloads.
export function createRouteCatalogLoader(fetchModels, { maxAge = 300_000, now = Date.now } = {}) {
  const cache = new Map();
  return {
    invalidate(endpoint) { cache.delete(routeCatalogKey(endpoint)); },
    peek(endpoint) {
      const entry = cache.get(routeCatalogKey(endpoint));
      return entry && now() - entry.at < maxAge ? entry.catalog : null;
    },
    async load(endpoint, { signal, force = false } = {}) {
      signal?.throwIfAborted();
      const cached = this.peek(endpoint);
      if (cached && !force) return cached;
      const result = await fetchModels(endpoint.id, { signal });
      signal?.throwIfAborted();
      if (result.endpointID !== endpoint.id) throw new Error('模型列表与所选入口不一致，请重试');
      const catalog = { models: result.models, source: result.source || 'api', status: '已获取', error: '', updatedAt: result.updatedAt };
      cache.set(routeCatalogKey(endpoint), { catalog, at: now() });
      return catalog;
    },
  };
}

export function saveFeatureRule(config, rule, draft, discovered = null) {
  if (!draft.name.trim() || !draft.model.trim()) throw new Error('请完整填写规则名称与目标承接模型');
  if (draft.endpointID && !config.endpoints?.some((endpoint) => endpoint.id === draft.endpointID)) {
    throw new Error('所选入口已不存在，请重新选择');
  }
  const rules = config.featureRules ||= [];
  const existing = rule ? rules.find((item) => item.id === rule.id) : null;
  if (rule && !existing) throw new Error('规则已不存在，请关闭后重新编辑');
  const next = existing || { id: draft.id, enabled: true };
  if (!isBuiltinRule(rule)) {
    next.name = draft.name.trim();
    next.match = { ...(next.match || {}) };
    for (const field of ['requestKind', 'toolTypePrefix', 'modelEquals', 'systemContains', 'messagesContain']) {
      const value = draft[field].trim();
      if (value) next.match[field] = value; else delete next.match[field];
    }
  }
  next.target = { ...(next.target || {}), model: draft.model.trim() };
  for (const field of ['endpointID', 'protocol', 'effort']) {
    if (draft[field]) next.target[field] = draft[field]; else delete next.target[field];
  }
  if (!existing) rules.push(next);
  const endpoint = config.endpoints?.find((item) => item.id === draft.endpointID);
  if (endpoint && discovered?.key === routeCatalogKey(endpoint)) {
    endpoint.catalog = { ...(endpoint.catalog || {}), ...discovered.catalog };
  }
  return config;
}
