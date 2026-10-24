export function formatOutputTokensPerSecond(value) {
  const number = Number(value);
  if (value == null || value === '' || !Number.isFinite(number)) return '—';
  return `${number.toFixed(1)} tok/s`;
}

export function outputSpeedDetail(requests) {
  if (requests == null || !Number.isFinite(Number(requests))) return '首字之后的生成速度';
  return `基于 ${Number(requests).toLocaleString('en-US')} 个可计速请求`;
}
