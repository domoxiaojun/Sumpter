import test from 'node:test';
import assert from 'node:assert/strict';

globalThis.window = { location: { search: '' } };
const { api } = await import('../src/services/api.js');

test('脱敏导出传递有限解析额度，raw 保持原参数且下载不聚合正文', () => {
  const anchors = [];
  globalThis.document = {
    createElement() { const anchor = { click() {}, remove() {} }; anchors.push(anchor); return anchor; },
    body: { appendChild() {} },
  };
  try {
    api.downloadDiagnosticCapture({ privacy: 'redacted', requestID: 'sample' });
    api.downloadDiagnosticCapture({ privacy: 'redacted', maxParseMib: 256 });
    api.downloadDiagnosticCapture({ privacy: 'raw', confirmRaw: true });
    assert.equal(new URL(anchors[0].href, 'https://sample.invalid').searchParams.get('maxParseMib'), '64');
    assert.equal(new URL(anchors[1].href, 'https://sample.invalid').searchParams.get('maxParseMib'), '256');
    assert.equal(new URL(anchors[2].href, 'https://sample.invalid').searchParams.get('maxParseMib'), null);
    for (const maxParseMib of [0, 257, Infinity, 1.5]) assert.throws(() => api.downloadDiagnosticCapture({ privacy: 'redacted', maxParseMib }));
  } finally { delete globalThis.document; }
});

test('登录限速提示保留 Retry-After，供登录页显示等待秒数', async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async () => new Response(JSON.stringify({ error: 'login_rate_limited', message: '请等待后重试' }), { status: 429, headers: { 'Retry-After': '6' } });
  try {
    await assert.rejects(api.request('/auth/login', { method: 'POST', body: '{}' }), error => error.status === 429 && error.retryAfterSeconds === 6);
  } finally { globalThis.fetch = original; }
});
