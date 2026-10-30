import test from 'node:test';
import assert from 'node:assert/strict';
import { requestPolicy, validateRequestPolicy } from '../src/utils/requestPolicy.js';
import { mergeRuntimeEvent } from '../src/utils/runtimeEvents.js';

test('策略默认值保留旧重试语义，合并局部设置并保留协议声明', () => {
  const source = { endpoints: { a: { limits: { maxConcurrent: 2 }, protocolModels: { 'openai-responses': { m: { strictTools: false } } } } } };
  const draft = requestPolicy(source);
  assert.equal(draft.streamProtection, 'complete');
  assert.equal(draft.maxTotalAttempts, 0);
  assert.equal(draft.endpoints.a.limits.maxQueued, 128);
  assert.equal(draft.endpoints.a.protocolModels['openai-responses'].m.strictTools, false);
  draft.endpoints.a.protocolModels['openai-responses'].m.strictTools = true;
  assert.equal(source.endpoints.a.protocolModels['openai-responses'].m.strictTools, false);
  validateRequestPolicy(draft);
});

test('策略拒绝空数字、非整数、越界值和未知流模式', () => {
  for (const [key, value] of [['streamBufferMib', ''], ['maxTotalAttempts', 1.5], ['maxBufferedMib', 0], ['streamBufferSeconds', -1], ['streamProtection', 'guess']]) {
    assert.throws(() => validateRequestPolicy({ ...requestPolicy(), [key]: value }));
  }
  const draft = requestPolicy();
  draft.global.queueTimeoutSeconds = 301;
  assert.throws(() => validateRequestPolicy(draft));
});

test('请求列表投影不会覆盖详情中的路由时间线，完整事件仍可清除', () => {
  const previous = { id: 'r', changeSeq: 2, requestTrace: { steps: [{ action: 'queue', durationMs: 20 }], omitted: 0 } };
  assert.deepEqual(mergeRuntimeEvent(previous, { id: 'r', changeSeq: 3, detailsOmitted: true }).requestTrace, previous.requestTrace);
  assert.equal(mergeRuntimeEvent(previous, { id: 'r', changeSeq: 3, requestTrace: null }).requestTrace, null);
});
