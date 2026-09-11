import { createLocalID } from './helpers.js';

export const cleanGroupModel = (value) => String(value ?? '').trim()
  .replace(/\[[^\]]*\]\s*$/, '').trim()
  .replace(/\s*\(\s*(?:none|auto|minimal|low|medium|high|xhigh|max|ultra)\s*\)$/i, '').trim();

export const modelGroupPrefix = (value) => {
  const model = cleanGroupModel(value);
  const match = model.match(/^([^\-_:\/]+)[\-_:\/]/);
  const token = (match?.[1] || model).toLowerCase();
  const family = token.replace(/\d+(?:\.\d+)*$/, '');
  return family && (match || family !== token) ? family : '__other__';
};

export function modelCatalogCategories(endpoints = []) {
  const byKey = new Map();
  for (const endpoint of endpoints) {
    const mappings = (endpoint.modelMappings || endpoint.mappings || []).map((mapping) => mapping.from ?? mapping.clientPattern);
    for (const raw of mappings) {
      const model = cleanGroupModel(raw);
      if (!model) continue;
      const key = model.toLowerCase();
      if (!byKey.has(key)) byKey.set(key, model);
    }
  }
  return categoriesForModels([...byKey.values()]);
}

export function categoriesForModels(models = []) {
  const grouped = new Map();
  for (const model of [...new Set(models.map(cleanGroupModel).filter(Boolean))]) {
    const id = modelGroupPrefix(model);
    if (!grouped.has(id)) grouped.set(id, []);
    grouped.get(id).push(model);
  }
  return [...grouped.entries()]
    .map(([id, models]) => ({ id, title: id === '__other__' ? '其他' : id, models: models.sort((a, b) => a.localeCompare(b)) }))
    .sort((a, b) => a.id === '__other__' ? 1 : b.id === '__other__' ? -1 : a.title.localeCompare(b.title));
}

export function endpointGroupModels(endpoint, groupModels = []) {
  if (!endpoint) return [];
  const supported = modelCatalogCategories([endpoint]).flatMap((category) => category.models);
  return [...new Set([...groupModels, ...supported].map(cleanGroupModel))]
    .filter((model) => model && groupModels.some((pattern) => modelMatches(pattern, model))
      && supported.some((pattern) => modelMatches(pattern, model)))
    .sort();
}

export function modelMatches(pattern, model) {
  pattern = cleanGroupModel(pattern); model = cleanGroupModel(model);
  return !!pattern && (pattern === model
    || (pattern.endsWith('*') && model.startsWith(pattern.slice(0, -1))));
}

export function newModelGroup() {
  return {
    id: createLocalID('group'),
    name: '新模型组',
    enabled: true,
    priority: 0,
    schedulingStrategy: 'priority',
    models: [],
    bindings: [],
  };
}

export function groupModels(config) {
  if (Array.isArray(config?.modelGroups)) return structuredClone(config.modelGroups).map((g) => ({
    ...g, name: g.name?.trim() || g.id, enabled: g.enabled ?? true, priority: g.priority ?? 0,
    schedulingStrategy: g.schedulingStrategy || 'priority',
    models: (g.models || []).map(cleanGroupModel),
    bindings: (g.bindings || []).map((b) => ({
      ...b, enabled: b.enabled ?? true, priority: b.priority ?? 0,
      models: b.models == null ? null : b.models.map(cleanGroupModel), overrides: b.overrides || [],
    })),
  }));
  const endpoints = config?.endpoints || [];
  if (!endpoints.length) return [];
  const patterns = (e) => [...new Set((e.modelMappings || e.mappings || [])
    .map((m) => cleanGroupModel(m.from ?? m.clientPattern)).filter(Boolean))];
  return [{ id: 'default', name: '默认模型组', enabled: true, priority: 0, schedulingStrategy: 'priority',
    models: [...new Set(endpoints.flatMap(patterns))],
    bindings: endpoints.map((e) => ({ endpointID: e.id, enabled: true,
      priority: e.priority || 0, models: patterns(e), overrides: [] })) }];
}

// Materialize legacy groups before adding the endpoint, so a later migration
// cannot silently enroll the new library entry in the default group.
export function addEndpointToLibrary(config, endpoint) {
  if (config.modelGroups == null) config.modelGroups = groupModels(config);
  config.endpoints = [...(config.endpoints || []), endpoint];
}

export function hasRoutableModel(config) {
  if (!Array.isArray(config?.modelGroups)) {
    return (config?.endpoints || []).some((e) => e.enabled !== false
      && (e.modelMappings || e.mappings || []).length > 0);
  }
  return config.modelGroups.some((g) => g.enabled !== false && (g.bindings || []).some((b) => {
    const endpoint = (config.endpoints || []).find((e) => e.id === b.endpointID && e.enabled !== false);
    return b.enabled !== false && endpointGroupModels(endpoint, g.models || []).some((pattern) =>
      b.models == null || b.models.some((selected) => modelMatches(pattern, selected) || modelMatches(selected, pattern)));
  }));
}

export function pruneGroupReferences(config) {
  const ids = new Set((config.endpoints || []).map((e) => e.id));
  for (const group of config.modelGroups || []) {
    group.bindings = (group.bindings || []).filter((b) => ids.has(b.endpointID));
    for (const binding of group.bindings) {
      if (binding.models != null) binding.models = binding.models.filter((m) => (group.models || []).some((p) => modelMatches(p, m)));
      binding.overrides = (binding.overrides || []).filter((o) => (group.models || []).some((p) => modelMatches(p, o.model))
        && (binding.models == null || binding.models.some((p) => modelMatches(p, o.model))));
    }
  }
  for (const rule of config.featureRules || []) {
    if (rule.target?.endpointID && !ids.has(rule.target.endpointID)) delete rule.target.endpointID;
  }
  return config;
}

export function groupRoutePreview(config, groups, model) {
  model = cleanGroupModel(model);
  const result = [];
  const endpoints = new Map((config.endpoints || []).map((e) => [e.id, e]));
  for (const group of groups.filter((g) => g.enabled !== false && (g.models || []).some((p) => modelMatches(p, model)))
    .toSorted((a, b) => (a.priority ?? 0) - (b.priority ?? 0))) {
    const bindings = (group.bindings || []).filter((b) => b.enabled !== false && endpoints.get(b.endpointID)?.enabled !== false
      && endpoints.has(b.endpointID) && (b.models == null || b.models.some((p) => modelMatches(p, model)))
      && (endpoints.get(b.endpointID).modelMappings || endpoints.get(b.endpointID).mappings || [])
        .some((mapping) => modelMatches(mapping.from ?? mapping.clientPattern, model)));
    const priority = (b) => b.overrides?.find((o) => cleanGroupModel(o.model) === model)?.priority ?? b.priority ?? 0;
    for (const b of bindings.toSorted((a, b) => priority(a) - priority(b))) {
      result.push({ groupID: group.id, group: group.name || group.id, endpointID: b.endpointID,
        endpoint: endpoints.get(b.endpointID).name || b.endpointID, priority: priority(b) });
    }
  }
  return result;
}
