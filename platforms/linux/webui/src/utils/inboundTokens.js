export function randomToken(prefix = 'sk-sumpter-') {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return prefix + Array.from(bytes, byte => byte.toString(16).padStart(2, '0')).join('');
}

export function inboundTokenOptions(config, facets, selected = '', legacyConfigured = false) {
  const rows = new Map((facets?.inboundTokens || []).map(row => [row.value, { ...row, label: row.name || row.value }]));
  for (const entry of config?.listener?.authTokens || []) {
    rows.set(entry.id, { ...(rows.get(entry.id) || { value: entry.id, count: 0 }), label: entry.name + (entry.enabled ? '' : '（已停用）') });
  }
  if (rows.has('legacy') || selected === 'legacy' || legacyConfigured) {
    rows.set('legacy', { ...(rows.get('legacy') || { value: 'legacy', count: 0 }), label: (config?.listener?.authTokenName || '默认 Token') + (config?.listener?.authTokenEnabled === false ? '（已停用）' : '') });
  }
  if (rows.has('unrecorded')) rows.get('unrecorded').label = '未记录 Token';
  if (selected && !rows.has(selected)) rows.set(selected, { value: selected, label: selected, count: 0 });
  return [...rows.values()];
}

export function validateInboundTokens(entries) {
  if (entries.length > 128) return '入站 Token 最多 128 个';
  const secrets = new Set();
  for (const entry of entries) {
    if (!entry.name.trim() || new TextEncoder().encode(entry.name.trim()).length > 128 || /[\u0000-\u001f\u007f]/.test(entry.name)) return 'Token 名称不能为空、含控制字符或超过 128 字节';
    if (entry.token && !/^[!-~]{16,512}$/.test(entry.token)) return 'Token 必须为 16–512 个可见 ASCII 字符，不能包含空格';
    if (entry.token && secrets.has(entry.token)) return 'Token 凭据不能重复';
    if (entry.token) secrets.add(entry.token);
  }
  return '';
}

// null means unchanged; revealed secrets never enter this draft.
export function inboundTokenDraft(document) {
  const listener = document?.config?.listener || {};
  const legacy = document?.secretStatus?.inboundAuthToken?.configured
    ? [{ id: 'legacy', name: listener.authTokenName || '默认 Token', enabled: listener.authTokenEnabled !== false, token: null }]
    : [];
  return [...legacy, ...(listener.authTokens || []).map(entry => ({ id: entry.id, name: entry.name, enabled: entry.enabled !== false, token: null }))];
}

export function buildInboundTokenWrite(document, entries) {
  const config = JSON.parse(JSON.stringify(document.config));
  const error = validateInboundTokens(entries.filter(entry => entry.id !== 'legacy'));
  if (error) throw new Error(error);
  if (entries.some(entry => !entry.name.trim() || new TextEncoder().encode(entry.name.trim()).length > 128 || /[\u0000-\u001f\u007f-\u009f]/.test(entry.name))) throw new Error('Token 名称不能为空、含控制字符或超过 128 字节');
  if (entries.some(entry => entry.token !== null && !entry.token)) throw new Error('Token 不能为空；要移除凭据，请使用删除');
  const legacy = entries.find(entry => entry.id === 'legacy');
  if (legacy?.token !== null && legacy?.token != null && !/^[!-~]{1,512}$/.test(legacy.token)) throw new Error('默认 Token 必须是可见 ASCII 字符，不能包含空格');
  const secrets = { inboundTokens: Object.fromEntries(entries.filter(entry => entry.id !== 'legacy' && entry.token !== null).map(entry => [entry.id, entry.token])) };
  if (legacy) {
    config.listener.authTokenName = legacy.name.trim();
    config.listener.authTokenEnabled = legacy.enabled;
    if (legacy.token !== null) secrets.inboundAuthToken = legacy.token;
  } else if (document.secretStatus?.inboundAuthToken?.configured) secrets.inboundAuthToken = '';
  config.listener.authTokens = entries.filter(entry => entry.id !== 'legacy').map(entry => ({ id: entry.id, name: entry.name.trim(), enabled: entry.enabled, token: '' }));
  return { config, secrets, opensAccess: !entries.length && inboundTokenDraft(document).length > 0 };
}
