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

test('unused Guardian sockets keep transport evidence without claiming a failed inference', () => {
  const trace = { stage: 'awaiting_first_message', closedBy: 'client', attemptCount: 0, abnormalClose: true, relayError: 'client_eof_without_close', transportErrorKind: 'eof_without_close' };
  const event = { kind: 'client', statusCode: 101, phase: 'completed', outcome: 'cancelled', message: 'websocket_unused_guardian_connection_closed', streamTrace: { websocketTrace: trace } };
  assert.equal(friendlyEventMessage(event), 'Guardian 未使用连接已关闭（预热阶段，未发送业务请求）');
  assert.match(formatWebSocketTrace(trace), /异常关闭/);
  assert.match(formatWebSocketTrace(trace), /eof_without_close/);
  assert.equal(friendlyEventMessage({ ...event, message: 'client_closed_before_first_message' }), '客户端侧 WebSocket 连接关闭（可能经过反代）');
  assert.equal(friendlyEventMessage({ ...event, outcome: 'failed' }), '首条业务消息前 WebSocket 异常断开');
  assert.equal(friendlyEventMessage({ ...event, streamTrace: { websocketTrace: { ...trace, stage: 'relay' } } }), '客户端侧 WebSocket 连接关闭（可能经过反代）');
});
