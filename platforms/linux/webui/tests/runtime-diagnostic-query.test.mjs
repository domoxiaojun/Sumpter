import assert from 'node:assert/strict';
import test from 'node:test';

globalThis.window = { location: { search: '' } };
const { api } = await import('../src/services/api.js');

test('diagnostic view is explicit and preserves request cancellation', async () => {
  const requests = [];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options) => {
    requests.push({ url: new URL(url, 'http://test.invalid'), options });
    return new Response('{}', { status: 200 });
  };
  try {
    const controller = new AbortController();
    await api.getRuntimeAnalytics('all');
    await api.getRuntimeAnalytics('all', { projectID: 'test-project' }, {
      diagnosticsOnly: true, signal: controller.signal,
    });
    assert.equal(requests[0].url.searchParams.get('view'), null);
    assert.equal(requests[1].url.searchParams.get('view'), 'diagnostics');
    assert.equal(requests[1].url.searchParams.get('projectID'), 'test-project');
    assert.equal(requests[1].options.signal, controller.signal);
    assert.equal('diagnosticsOnly' in requests[1].options, false);
    assert.equal(api.mockApiPromise, undefined);
  } finally {
    globalThis.fetch = originalFetch;
  }
});
