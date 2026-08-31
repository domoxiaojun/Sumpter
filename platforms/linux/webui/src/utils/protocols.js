export const SOURCE_FORMATS = Object.freeze([
  Object.freeze({ value: 'anthropic', label: 'Anthropic Messages' }),
  Object.freeze({ value: 'openai', label: 'OpenAI Chat Completions' }),
  Object.freeze({ value: 'openai-responses', label: 'OpenAI Responses' }),
]);

export const ENDPOINT_PROTOCOL_MODES = Object.freeze([
  Object.freeze({ value: 'auto', label: '自动（三协议）' }),
  ...SOURCE_FORMATS,
]);

const SOURCE_FORMAT_VALUES = new Set(SOURCE_FORMATS.map((item) => item.value));
const ENDPOINT_PROTOCOL_VALUES = new Set(ENDPOINT_PROTOCOL_MODES.map((item) => item.value));

export function isSourceFormat(value) {
  return SOURCE_FORMAT_VALUES.has(String(value || ''));
}

export function isEndpointProtocol(value) {
  return ENDPOINT_PROTOCOL_VALUES.has(String(value || ''));
}

export function normalizeEndpointProtocol(value, fallback = 'auto') {
  const protocol = String(value || '').trim();
  return isEndpointProtocol(protocol) ? protocol : fallback;
}

export function endpointProtocolLabel(value) {
  const protocol = normalizeEndpointProtocol(value);
  return ENDPOINT_PROTOCOL_MODES.find((item) => item.value === protocol)?.label || protocol;
}

export function sourceFormatLabel(value) {
  const format = String(value || '');
  return SOURCE_FORMATS.find((item) => item.value === format)?.label || format || '-';
}

export function routeModeLabel(value) {
  if (value === 'native') return '原生';
  if (value === 'translated') return '桥接';
  return value || '-';
}
