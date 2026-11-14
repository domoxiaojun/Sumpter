// 懒加载分包失败的浏览器文案各不相同；升级 daemon 后旧标签页请求已被替换的 hash 文件时出现。
const CHUNK_LOAD_PATTERNS = [
  /dynamically imported module/i, // Chrome / Firefox
  /Importing a module script failed/i, // Safari
  /Unable to preload CSS/i, // Vite 预加载样式失败
];

export function isChunkLoadError(error) {
  const message = String(error?.message || error || '');
  return error?.name === 'ChunkLoadError' || CHUNK_LOAD_PATTERNS.some((pattern) => pattern.test(message));
}

export function errorSummary(error) {
  const message = String(error?.message || error || '').trim();
  return message.length > 300 ? `${message.slice(0, 300)}…` : message;
}
