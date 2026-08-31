// 写请求 body 契约:Linux admin 的写端点一律用 `Json<Value>` 提取器解析 body
// (crates/kekulv-proxy/src/admin.rs 的 require_json),空 body 会被判非法 JSON 返回 400。
// 前端曾对无参写请求只声明 Content-Type 而不发 body,导致「重置统计」等按钮必然 400。
// 这里用非 mock 模式(不带 ?mock=1)拦下 fetch,断言真实请求形状。
import assert from 'node:assert/strict';
import test from 'node:test';

globalThis.window = { location: { search: '' } };

const calls = [];
globalThis.fetch = async (url, init) => {
  calls.push({ url, init });
  return {
    ok: true,
    status: 200,
    headers: { get: () => null },
    text: async () => JSON.stringify({ reset: true }),
  };
};

const { api } = await import('../src/services/api.js');

function lastCall() {
  assert.ok(calls.length > 0, '应当发出过请求');
  return calls[calls.length - 1];
}

test('无参写请求必须带非空 JSON body', async () => {
  for (const [name, invoke] of [
    ['resetRuntime', () => api.resetRuntime()],
    ['recreateRuntime', () => api.recreateRuntime()],
    ['toggleProxy(true)', () => api.toggleProxy(true)],
    ['toggleProxy(false)', () => api.toggleProxy(false)],
    ['logout', () => api.logout()],
  ]) {
    calls.length = 0;
    await invoke();
    const { init } = lastCall();
    assert.equal(init.method, 'POST', `${name} 应当是 POST`);
    assert.equal(
      init.headers['Content-Type'],
      'application/json',
      `${name} 应当声明 JSON`,
    );
    assert.ok(init.body != null, `${name} 必须带 body,否则 admin 侧 EOF 400`);
    assert.doesNotThrow(
      () => JSON.parse(init.body),
      `${name} 的 body 必须是合法 JSON`,
    );
  }
});

test('调用方自带 body 时不被覆盖', async () => {
  calls.length = 0;
  await api.request('/reload', { method: 'POST', body: JSON.stringify({ a: 1 }) });
  assert.deepEqual(JSON.parse(lastCall().init.body), { a: 1 });
});

test('读请求不加 body、不声明 JSON', async () => {
  calls.length = 0;
  await api.request('/status');
  const { init } = lastCall();
  assert.equal(init.body, undefined, 'GET 不应带 body');
  assert.equal(init.headers['Content-Type'], undefined, 'GET 不应声明 JSON');
});
