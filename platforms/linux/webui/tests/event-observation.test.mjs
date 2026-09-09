import assert from 'node:assert/strict';
import test from 'node:test';
import { eventCacheLabel, eventCacheTokenRatio, eventHttpTone, eventAgentLabel, eventDetailGroups } from '../src/utils/eventPresentation.js';
import { mergeRuntimeEvent } from '../src/utils/runtimeEvents.js';

test('HTTP, final outcome and upstream cache evidence stay independent', () => {
  const event = { kind: 'client', statusCode: 200, outcome: 'failed', cacheRead: { state: 'hit', readTokens: 1280, finality: 'confirmed' } };
  assert.match(eventCacheLabel(event), /已命中/);
  assert.equal(eventHttpTone(event), 'var(--status-good)');
  assert.equal(eventCacheLabel({ statusCode: 200 }), '缓存未知');
  assert.equal(eventCacheLabel({ kind: 'notify' }), '缓存不适用');
});

test('token ratio requires known counts and protocol-specific denominator', () => {
  const event = { targetFormat: 'anthropic', cacheRead: { state: 'hit', readTokens: 20, finality: 'confirmed' }, usageSummary: { inputTokens: 80 } };
  assert.equal(eventCacheTokenRatio(event), null);
  assert.equal(eventCacheTokenRatio({ ...event, usageSummary: { inputTokens: 80, cacheCreationInputTokens: 0 } }), 0.2);
  assert.equal(eventCacheTokenRatio({ ...event, targetFormat: 'gemini', usageSummary: { inputTokens: 100 } }), 0.2);
  assert.equal(eventCacheTokenRatio({ ...event, targetFormat: 'openai', usageSummary: { inputTokens: 0 } }), null);
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
