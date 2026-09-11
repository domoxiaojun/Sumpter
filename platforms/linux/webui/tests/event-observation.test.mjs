import assert from 'node:assert/strict';
import test from 'node:test';
import { eventCacheLabel, eventCacheTokenRatio, eventCacheHitRateLabel, eventHttpTone, eventAgentLabel, eventDetailGroups, eventUsageLabel, eventHasObservedUsage } from '../src/utils/eventPresentation.js';
import { mergeRuntimeEvent } from '../src/utils/runtimeEvents.js';
import { eventLogicalModel, friendlyEventMessage } from '../src/utils/helpers.js';

test('event list shows the routed logical model independently of client and upstream models', () => {
  const event = { clientModel: 'claude-opus-5', effectiveModel: 'gpt-5.6-terra', upstreamModel: 'provider-alias' };
  assert.equal(eventLogicalModel(event), 'gpt-5.6-terra');
  assert.equal(eventLogicalModel({ ...event, effectiveModel: null }), '—');
  assert.equal(eventLogicalModel({ effective_model: ' gpt-5.6-terra ' }), 'gpt-5.6-terra');
});

test('HTTP, final outcome and upstream cache evidence stay independent', () => {
  const event = { kind: 'client', statusCode: 200, outcome: 'failed', cacheRead: { state: 'hit', readTokens: 1280, finality: 'confirmed' } };
  assert.equal(eventCacheLabel(event), '缓存读取 1,280');
  assert.equal(eventHttpTone(event), 'var(--status-good)');
  assert.equal(eventCacheLabel({ statusCode: 200 }), '缓存读取 —');
  assert.equal(eventCacheLabel({ kind: 'notify' }), '缓存读取 不适用');
});

test('usage distinguishes pending, missing, zero and partial reports', () => {
  const pending = { phase: 'inFlight', statusCode: 0, usageSummary: {} };
  assert.equal(eventHasObservedUsage(pending), false);
  assert.equal(eventUsageLabel(pending), '等待用量');
  assert.equal(eventUsageLabel({ phase: 'completed' }), '未报告用量');
  assert.equal(eventHasObservedUsage({ usageSummary: { inputTokens: null, outputTokens: -1 } }), false);

  const zero = { phase: 'inFlight', usageSummary: { inputTokens: 0 }, cacheRead: { state: 'miss', readTokens: 0, finality: 'confirmed' } };
  assert.equal(eventHasObservedUsage(zero), true);
  assert.equal(eventUsageLabel(zero), '输入 0 · 输出 —');
  assert.equal(eventCacheLabel(zero), '缓存读取 0');

  const partial = { phase: 'inFlight', streamTrace: { usage: { inputTokens: 33, outputTokens: 3 } }, cacheRead: { state: 'hit', readTokens: 222950, finality: 'provisional' } };
  assert.equal(eventHasObservedUsage(partial), true);
  assert.equal(eventUsageLabel(partial), '输入 33 · 输出 3');
  assert.equal(eventCacheLabel(partial), '缓存读取 222,950');
  assert.equal(partial.cacheRead.finality, 'provisional');
});

test('in-flight status distinguishes waiting for headers from receiving and streaming', () => {
  const pending = { phase: 'inFlight', statusCode: 0 };
  assert.equal(friendlyEventMessage(pending), '等待响应');
  assert.equal(friendlyEventMessage({ ...pending, statusCode: 200 }), '接收响应中');
  const streaming = { ...pending, statusCode: 200, streamTrace: { chunkCount: 4 } };
  assert.equal(friendlyEventMessage(streaming), '流式输出中');
  assert.equal(friendlyEventMessage({ ...streaming, statusCode: 0, upstreamStatusCode: 200 }), '等待响应');
});

test('token ratio requires known counts and protocol-specific denominator', () => {
  const event = { targetFormat: 'anthropic', cacheRead: { state: 'hit', readTokens: 20, finality: 'confirmed' }, usageSummary: { inputTokens: 80 } };
  assert.equal(eventCacheTokenRatio(event), null);
  assert.equal(eventCacheTokenRatio({ ...event, usageSummary: { inputTokens: 80, cacheCreationInputTokens: 0 } }), 0.2);
  assert.equal(eventCacheTokenRatio({ ...event, targetFormat: 'gemini', usageSummary: { inputTokens: 100 } }), 0.2);
  assert.equal(eventCacheTokenRatio({ ...event, targetFormat: 'openai', usageSummary: { inputTokens: 0 } }), null);
});

test('inline cache hit rate uses token share, preserving zero and unknown evidence', () => {
  const event = { targetFormat: 'openai-responses', cacheRead: { state: 'hit', readTokens: 203776, finality: 'confirmed' }, usageSummary: { inputTokens: 204082 } };
  assert.equal(eventCacheHitRateLabel(event), '命中率 99.9%');
  assert.equal(eventCacheHitRateLabel({ ...event, cacheRead: { state: 'miss', readTokens: 0, finality: 'confirmed' } }), '命中率 0%');
  assert.equal(eventCacheHitRateLabel({ ...event, usageSummary: { inputTokens: 0 } }), '命中率 —');
  assert.equal(eventCacheHitRateLabel({ ...event, cacheRead: { ...event.cacheRead, finality: 'provisional' } }), '命中率 —');
  assert.equal(eventCacheHitRateLabel({}), '命中率 —');
  const anthropic = { targetFormat: 'anthropic', cacheRead: { state: 'hit', readTokens: 20, finality: 'confirmed' }, usageSummary: { inputTokens: 60, cacheCreationInputTokens: 20 } };
  assert.equal(eventCacheHitRateLabel(anthropic), '命中率 20%');
  assert.equal(eventCacheHitRateLabel({ ...anthropic, usageSummary: { inputTokens: 60 } }), '命中率 —');
});

test('older pages cannot roll back live state; explicitly omitted details survive projection merges', () => {
  const previous = { id: 'r', changeSeq: 4, cacheRead: { state: 'hit' }, streamTrace: { usage: { inputTokens: 10 } }, agentName: '/root/review' };
  assert.equal(mergeRuntimeEvent(previous, { id: 'r', changeSeq: 3, cacheRead: { state: 'pending' } }), previous);
  const merged = mergeRuntimeEvent(previous, { id: 'r', changeSeq: 5, detailsOmitted: true, streamTrace: null, cacheRead: { state: 'miss' }, usageSummary: { inputTokens: 20 } });
  assert.equal(merged.cacheRead.state, 'miss');
  assert.deepEqual(merged.streamTrace, previous.streamTrace);
  const full = mergeRuntimeEvent(merged, { id: 'r', changeSeq: 6, detailsOmitted: false, streamTrace: null, outcome: null });
  assert.equal(full.streamTrace, null);
  assert.equal(full.usageSummary, null);
});

test('unknown role is not presented as root, and each detail purpose is separately expandable', () => {
  assert.equal(eventAgentLabel({}), '');
  assert.equal(eventAgentLabel({ agentRole: 'unknown' }), '');
  assert.equal(eventAgentLabel({ agentName: '/root/review', agentRole: 'subagent' }), '/root/review · 子代理');
  assert.deepEqual(eventDetailGroups.map(([id]) => id), ['routing', 'usage', 'identity', 'response', 'tools', 'advanced']);
});


test('horizontal controls reflect actual overflow and valid scroll directions', async () => {
  const { tableScrollMetrics, tableScrollHint } = await import('../src/utils/tableScroll.js');
  assert.equal(tableScrollHint(tableScrollMetrics({ clientWidth: 900, scrollWidth: 900 })), '');
  assert.equal(tableScrollHint(tableScrollMetrics({ clientWidth: 600, scrollWidth: 1000, scrollLeft: 0 })), '向右滚动查看更多列');
  assert.equal(tableScrollHint(tableScrollMetrics({ clientWidth: 600, scrollWidth: 1000, scrollLeft: 100 })), '可左右滚动查看更多列');
  assert.equal(tableScrollHint(tableScrollMetrics({ clientWidth: 600, scrollWidth: 1000, scrollLeft: 800 })), '向左滚动查看更多列');
  assert.equal(tableScrollMetrics({ clientWidth: 0, scrollWidth: 1000 }).max, 0);
});
