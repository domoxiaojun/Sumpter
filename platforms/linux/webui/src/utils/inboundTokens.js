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
    rows.set('legacy', { ...(rows.get('legacy') || { value: 'legacy', count: 0 }), label: config?.listener?.authTokenName || '默认 Token' });
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
