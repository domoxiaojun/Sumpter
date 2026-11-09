import test from 'node:test';
import assert from 'node:assert/strict';
import { requestPolicy, validateRequestPolicy } from '../src/utils/requestPolicy.js';
import { mergeRuntimeEvent } from '../src/utils/runtimeEvents.js';
import { failureKindLabel } from '../src/utils/helpers.js';
import { traceReasons } from '../src/utils/requestPolicy.js';

test('策略默认值启用连续无响应保护，合并局部设置并保留协议声明', () => {
  const source = { endpoints: { a: { limits: { maxConcurrent: 2 }, protocolModels: { 'openai-responses': { m: { strictTools: false } } } } } };
  const draft = requestPolicy(source);
  assert.equal(draft.streamProtection, 'complete');
  assert.equal(draft.maxTotalAttempts, 0);
  assert.equal(draft.responseWaitTimeoutSeconds, 0);
  assert.equal(draft.maxConsecutiveNoResponse, 3);
  assert.equal(draft.noResponseCooldownSeconds, 300);
  assert.equal(draft.endpoints.a.limits.maxQueued, 128);
  assert.equal(draft.endpoints.a.protocolModels['openai-responses'].m.strictTools, false);
  draft.endpoints.a.protocolModels['openai-responses'].m.strictTools = true;
  assert.equal(source.endpoints.a.protocolModels['openai-responses'].m.strictTools, false);
  validateRequestPolicy(draft);
});

test('连续无响应保护允许显式关闭，拒绝越界设置并显示保护终态', () => {
  const disabled = requestPolicy({ maxConsecutiveNoResponse: 0, noResponseCooldownSeconds: 60 });
  validateRequestPolicy(disabled);
  assert.equal(disabled.maxConsecutiveNoResponse, 0);
  for (const [key, value] of [['maxConsecutiveNoResponse', -1], ['maxConsecutiveNoResponse', 101], ['maxConsecutiveNoResponse', 1.5], ['noResponseCooldownSeconds', 0], ['noResponseCooldownSeconds', 3601]]) {
    assert.throws(() => validateRequestPolicy({ ...disabled, [key]: value }));
  }
  assert.equal(failureKindLabel('no_response_retry_exhausted'), '连续未收到响应，已停止重试');
  assert.equal(traceReasons.no_response_retry_exhausted, '连续未收到响应，已停止向该入口重试');
});

test('策略拒绝空数字、非整数、越界值和未知流模式', () => {
  for (const [key, value] of [['streamBufferMib', ''], ['maxTotalAttempts', 1.5], ['responseWaitTimeoutSeconds', 86401], ['maxBufferedMib', 0], ['streamBufferSeconds', -1], ['streamProtection', 'guess']]) {
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
