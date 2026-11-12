export function hasCompressedCapture(headers = []) {
  return headers.some(({ name, value }) => String(name).toLowerCase() === 'content-encoding'
    && String(value).split(',').some((encoding) => encoding.trim().toLowerCase() !== 'identity'));
}

export function diagnosticPayloadText(data, encoding, redacted = false) {
  if (redacted) return '(已脱敏：原始数据已移除)';
  if (!encoding || encoding === 'utf8') return data ?? '';
  const label = encoding === 'base64' ? 'Base64 · 二进制原始字节' : `未知编码：${encoding}`;
  return `[${label}]\n${data ?? ''}`;
}

export function diagnosticChunksText(chunks = [], compressed = false) {
  const warning = compressed && chunks.some((chunk) => !chunk.dataEncoding && !chunk.redacted)
    ? '旧格式文本捕获：压缩字节可能已丢失，无法保证还原。\n\n' : '';
  return warning + chunks.map((chunk) => `[+${chunk.atMS}ms · ${chunk.bytes} bytes${chunk.truncated ? ' · 已截断' : ''}]\n${diagnosticPayloadText(chunk.data, chunk.dataEncoding, chunk.redacted)}`).join('\n\n');
}
