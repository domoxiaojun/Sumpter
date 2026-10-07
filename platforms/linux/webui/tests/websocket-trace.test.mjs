import assert from 'node:assert/strict';
import test from 'node:test';
import { formatWebSocketTrace, friendlyEventMessage, eventOutcomeLabel, failureKindLabel } from '../src/utils/helpers.js';

test('WebSocket trace accepts old records and presents optional transport evidence', () => {
  const old = { stage: 'relay', handshakeStatus: 101 };
  assert.equal(formatWebSocketTrace(old), 'WebSocket 阶段: 消息转发 · 上游握手: HTTP 101');
  const summary = formatWebSocketTrace({ ...old, transportErrorKind: 'connection_reset', lastEventType: 'response.completed', idleTimeoutMS: 200 });
  assert.match(summary, /传输错误类型: connection_reset/);
  assert.match(summary, /最近上游事件: response.completed/);
  assert.match(summary, /空闲截止: 200ms/);
});

test('connection outcomes do not imply every response failed or deliberate user cancellation', () => {
  const event = { kind: 'client', statusCode: 101, phase: 'completed', streamTrace: { websocketTrace: { stage: 'relay', closedBy: 'upstream' } } };
  assert.equal(friendlyEventMessage({ ...event, outcome: 'succeeded' }), 'WebSocket 连接正常结束');
  assert.equal(friendlyEventMessage({ ...event, outcome: 'failed' }), 'WebSocket 连接异常结束（不代表每次响应均失败）');
  assert.equal(friendlyEventMessage({ ...event, outcome: 'failed', failureKind: 'stream_idle_timeout' }), 'WebSocket 双向帧空闲超时');
  assert.equal(friendlyEventMessage({ ...event, outcome: 'cancelled', streamTrace: { websocketTrace: { stage: 'relay', closedBy: 'client' } } }), '客户端侧 WebSocket 连接关闭（可能经过反代）');
  assert.equal(friendlyEventMessage({ outcome: 'cancelled', statusCode: 200 }), '客户端断开或取消请求');
  assert.doesNotMatch(eventOutcomeLabel({ outcome: 'cancelled' }), /主动/);
  assert.doesNotMatch(failureKindLabel('client_cancelled'), /主动/);
});
