export const BUILTIN_RULE_IDS = new Set(['websearch', 'webfetch', 'classifier']);

export const isBuiltinRule = (rule) => BUILTIN_RULE_IDS.has(String(rule?.id || '').toLowerCase());

export function routeModelChoices(endpoints, endpointID, catalog) {
  const scoped = endpointID ? endpoints.filter((endpoint) => endpoint.id === endpointID) : endpoints.filter((endpoint) => endpoint.enabled !== false);
  const names = scoped.flatMap((endpoint) => [
    ...(endpointID ? (catalog?.models ?? endpoint.catalog?.models ?? []) : []),
    // Rule targets are logical models: the engine still applies endpoint
    // mappings. A pinned endpoint also accepts concrete catalog models.
    ...(endpoint.modelMappings || endpoint.mappings || []).map((mapping) => mapping.from ?? mapping.clientPattern),
  ]);
  return [...new Set(names.map((name) => String(name ?? '').trim()).filter((name) => name && !name.includes('*')))].sort();
}

export function routeCatalogKey(endpoint) {
  return JSON.stringify([endpoint.id, endpoint.baseURL, endpoint.protocol, endpoint.headers, endpoint.pinnedIP, endpoint.pinnedIPExclusive, endpoint.userAgent]);
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
  const name = String(draft.name ?? '').trim();
  const model = String(draft.model ?? '').trim();
  if (!name || !model) throw new Error('请填写规则名称并选择目标承接模型');
  if (draft.endpointID && !config.endpoints?.some((endpoint) => endpoint.id === draft.endpointID)) {
    throw new Error('所选入口已不存在，请重新选择');
  }
  const endpoint = config.endpoints?.find((item) => item.id === draft.endpointID);
  const catalog = endpoint && discovered?.key === routeCatalogKey(endpoint) ? discovered.catalog : undefined;
  if (!routeModelChoices(config.endpoints || [], draft.endpointID, catalog).includes(model)) {
    throw new Error('请从当前入口的模型列表中选择目标承接模型');
  }
  const rules = config.featureRules ||= [];
  const existing = rule ? rules.find((item) => item.id === rule.id) : null;
  if (rule && !existing) throw new Error('规则已不存在，请关闭后重新编辑');
  const next = existing || { id: draft.id, enabled: true };
  if (!isBuiltinRule(rule)) {
    next.name = name;
    next.match = { ...(next.match || {}) };
    for (const field of ['requestKind', 'toolTypePrefix', 'modelEquals', 'systemContains', 'messagesContain']) {
      const value = String(draft[field] ?? '').trim();
      if (value) next.match[field] = value; else delete next.match[field];
    }
  }
  next.target = { ...(next.target || {}), model };
  for (const field of ['endpointID', 'protocol', 'effort']) {
    if (draft[field]) next.target[field] = draft[field]; else delete next.target[field];
  }
  if (!existing) rules.push(next);
  if (endpoint && discovered?.key === routeCatalogKey(endpoint)) {
    endpoint.catalog = { ...(endpoint.catalog || {}), ...discovered.catalog };
  }
  return config;
}
