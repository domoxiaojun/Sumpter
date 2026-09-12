export const USER_AGENT_PROTOCOLS = Object.freeze([
  { key: 'anthropic', label: 'Anthropic' },
  { key: 'openai', label: 'OpenAI（Chat / Responses）' },
  { key: 'gemini', label: 'Gemini' },
]);

export function normalizeUserAgentSettings(value) {
  if (value === undefined) return {};
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new TypeError('User-Agent 配置必须是对象');
  }
  const result = {};
  for (const { key, label } of USER_AGENT_PROTOCOLS) {
    const rule = value[key];
    if (rule === undefined) continue;
    if (!rule || typeof rule !== 'object' || Array.isArray(rule)) {
      throw new TypeError(`${label} UA 配置必须是对象`);
    }
    const mode = rule.mode === undefined ? 'auto' : rule.mode;
    const raw = rule.value === undefined ? '' : rule.value;
    if (!['auto', 'override'].includes(mode)) throw new TypeError(`${label} UA 模式无效`);
    if (typeof raw !== 'string') throw new TypeError(`${label} UA 必须是字符串`);
    if (new TextEncoder().encode(raw).length > 512) throw new TypeError(`${label} UA 不能超过 512 字节`);
    if (/[\x00-\x1f\x7f]/.test(raw)) throw new TypeError(`${label} UA 不能包含换行或控制字符`);
    const text = raw.trim();
    if (mode === 'override' || text) result[key] = { mode, ...(text ? { value: text } : {}) };
  }
  return result;
}

export function userAgentSummary(settings = {}) {
  return USER_AGENT_PROTOCOLS.map(({ key }) => {
    const rule = settings[key];
    const name = { anthropic: 'Anthropic', openai: 'OpenAI', gemini: 'Gemini' }[key];
    const status = rule?.mode === 'override' ? (rule.value ? '强覆盖' : '强覆盖（默认）') : (rule?.value ? '自动' : '默认');
    return `${name} ${status}`;
  }).join(' · ');
}
