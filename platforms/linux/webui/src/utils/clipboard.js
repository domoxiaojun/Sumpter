export class ClipboardCopyError extends Error {
  constructor(message, cause) {
    super(message, cause ? { cause } : undefined);
    this.name = 'ClipboardCopyError';
  }
}

function isLocalHostname(hostname) {
  const value = String(hostname || '').toLowerCase();
  return value === 'localhost'
    || value === '127.0.0.1'
    || value === '::1'
    || value === '[::1]';
}

export function isRemotePlainHTTP(locationObject = globalThis.location) {
  return locationObject?.protocol === 'http:' && !isLocalHostname(locationObject?.hostname);
}

function legacyCopy(text, documentObject) {
  if (!documentObject?.body || typeof documentObject.createElement !== 'function'
    || typeof documentObject.execCommand !== 'function') return false;

  const textarea = documentObject.createElement('textarea');
  textarea.value = text;
  textarea.setAttribute('readonly', '');
  textarea.setAttribute('aria-hidden', 'true');
  Object.assign(textarea.style, {
    position: 'fixed',
    inset: '0 auto auto -9999px',
    opacity: '0',
    pointerEvents: 'none',
  });
  documentObject.body.appendChild(textarea);
  try {
    textarea.focus();
    textarea.select();
    textarea.setSelectionRange?.(0, text.length);
    return documentObject.execCommand('copy') === true;
  } catch {
    return false;
  } finally {
    textarea.remove();
  }
}

/**
 * Copy text without reporting success before the browser confirms it.
 * The deprecated execCommand path is intentionally limited to remote plain
 * HTTP pages, where the modern Clipboard API is normally unavailable.
 */
export async function copyTextToClipboard(text, options = {}) {
  const value = String(text ?? '');
  if (!value) throw new ClipboardCopyError('没有可复制的内容');

  const navigatorObject = options.navigatorObject ?? globalThis.navigator;
  const documentObject = options.documentObject ?? globalThis.document;
  const locationObject = options.locationObject ?? globalThis.location;
  const writeText = navigatorObject?.clipboard?.writeText;
  let clipboardError = null;

  if (typeof writeText === 'function') {
    try {
      await writeText.call(navigatorObject.clipboard, value);
      return { method: 'clipboard' };
    } catch (error) {
      clipboardError = error;
    }
  }

  if (isRemotePlainHTTP(locationObject) && legacyCopy(value, documentObject)) {
    return { method: 'execCommand' };
  }

  if (isRemotePlainHTTP(locationObject)) {
    throw new ClipboardCopyError('复制失败，请使用 HTTPS 或检查剪贴板权限', clipboardError);
  }
  if (typeof writeText !== 'function') {
    throw new ClipboardCopyError('复制失败，当前浏览器不支持剪贴板 API，请检查权限或使用 HTTPS');
  }
  throw new ClipboardCopyError('复制失败，请检查浏览器剪贴板权限', clipboardError);
}

export async function copyWithToast(text, label, addToast, options = {}) {
  try {
    const result = await copyTextToClipboard(text, options);
    addToast?.(`${label || '内容'}已复制到剪贴板`, 'success');
    return result;
  } catch (error) {
    addToast?.(error?.message || '复制失败，请检查浏览器剪贴板权限', 'error');
    return null;
  }
}
