import assert from 'node:assert/strict';
import test from 'node:test';

const {
  copyTextToClipboard,
  copyWithToast,
  isRemotePlainHTTP,
} = await import('../src/utils/clipboard.js');

function fallbackDocument({ succeeds = true } = {}) {
  const textarea = {
    style: {},
    setAttribute() {},
    focus() {},
    select() {},
    setSelectionRange() {},
    remove() { this.removed = true; },
  };
  return {
    textarea,
    body: { appendChild(node) { this.appended = node; } },
    createElement: () => textarea,
    execCommand(command) { return command === 'copy' && succeeds; },
  };
}

test('Clipboard API resolves before success is returned', async () => {
  let resolveWrite;
  const writePromise = new Promise((resolve) => { resolveWrite = resolve; });
  let completed = false;
  const task = copyTextToClipboard('event json', {
    navigatorObject: { clipboard: { writeText: () => writePromise } },
    locationObject: { protocol: 'https:', hostname: 'example.test' },
  }).then((value) => { completed = true; return value; });
  await Promise.resolve();
  assert.equal(completed, false);
  resolveWrite();
  assert.deepEqual(await task, { method: 'clipboard' });
});

test('remote plain HTTP falls back to textarea copy when Clipboard API is missing', async () => {
  const documentObject = fallbackDocument();
  assert.equal(isRemotePlainHTTP({ protocol: 'http:', hostname: '10.0.0.8' }), true);
  assert.deepEqual(await copyTextToClipboard('request id', {
    navigatorObject: {},
    documentObject,
    locationObject: { protocol: 'http:', hostname: '10.0.0.8' },
  }), { method: 'execCommand' });
  assert.equal(documentObject.textarea.removed, true);
});

test('remote plain HTTP fallback runs after Clipboard API rejects', async () => {
  const documentObject = fallbackDocument();
  assert.deepEqual(await copyTextToClipboard('metadata', {
    navigatorObject: { clipboard: { writeText: async () => { throw new Error('denied'); } } },
    documentObject,
    locationObject: { protocol: 'http:', hostname: 'admin.internal' },
  }), { method: 'execCommand' });
});

test('failed fallback reports HTTPS or permission guidance and never success', async () => {
  const toasts = [];
  const result = await copyWithToast('json', '事件 JSON', (...args) => toasts.push(args), {
    navigatorObject: {},
    documentObject: fallbackDocument({ succeeds: false }),
    locationObject: { protocol: 'http:', hostname: 'admin.internal' },
  });
  assert.equal(result, null);
  assert.deepEqual(toasts, [['复制失败，请使用 HTTPS 或检查剪贴板权限', 'error']]);
});

test('Clipboard rejection on HTTPS reports a permission failure', async () => {
  await assert.rejects(
    copyTextToClipboard('json', {
      navigatorObject: { clipboard: { writeText: async () => { throw new Error('denied'); } } },
      locationObject: { protocol: 'https:', hostname: 'example.test' },
    }),
    /检查浏览器剪贴板权限/,
  );
});
