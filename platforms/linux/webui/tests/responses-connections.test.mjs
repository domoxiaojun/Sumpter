import assert from 'node:assert/strict';
import test from 'node:test';
import { runtimeSummaryRevision } from '../src/utils/runtimeSync.js';
import {
  initialConnectionsObservation, receiveConnectionsSummary, failConnectionsSummary,
  visibleConnectionsObservation, connectionsPresentation,
} from '../src/utils/responsesConnections.js';

const zero = { total: 0, awaitingFirstMessage: 0, guardianAwaitingFirstMessage: 0, connectingUpstream: 0, relaying: 0, oldestFirstMessageWaitMS: null };
const receive = (value) => receiveConnectionsSummary({ responsesWebSocketConnections: value });

test('distinguishes an old daemon, initial load, read failure, and a measured zero', () => {
  assert.match(connectionsPresentation(initialConnectionsObservation).message, /正在读取/);
  assert.equal(connectionsPresentation(receiveConnectionsSummary({ apiVersion: 1 })).message, '当前版本未提供');
  const measured = connectionsPresentation(receive(zero));
  assert.equal(measured.message, null);
  assert.deepEqual(measured.rows.map(([, value]) => value), ['0', '0', '0', '0', '0', '—']);
  assert.equal(connectionsPresentation(failConnectionsSummary(initialConnectionsObservation, 'offline')).message, '连接数据暂不可用');
});

test('failed reads and a paused display retain the last measurement, then resume and recover', () => {
  const before = receive({ ...zero, total: 8, awaitingFirstMessage: 8, guardianAwaitingFirstMessage: 8, oldestFirstMessageWaitMS: 1200 });
  const failed = failConnectionsSummary(before, 'offline');
  assert.equal(failed.value, before.value);
  assert.equal(connectionsPresentation(failed).error, '更新失败，保留上次数据');
  const newer = receive({ ...before.value, oldestFirstMessageWaitMS: 2500 });
  assert.equal(visibleConnectionsObservation(before, newer, false), before);
  assert.equal(visibleConnectionsObservation(before, newer, true), newer);
  assert.notEqual(connectionsPresentation(before).rows.at(-1)[1], connectionsPresentation(newer).rows.at(-1)[1]);
  assert.equal(connectionsPresentation(receive(zero)).error, null);
});

test('live connection changes update the observation without invalidating historical analytics', () => {
  const first = { apiVersion: 1, counters: { clientRequests: 3 }, responsesWebSocketConnections: { ...zero, total: 1, awaitingFirstMessage: 1, oldestFirstMessageWaitMS: 500 } };
  const later = { ...first, responsesWebSocketConnections: { ...first.responsesWebSocketConnections, oldestFirstMessageWaitMS: 1500 } };
  assert.equal(runtimeSummaryRevision(first), runtimeSummaryRevision(later));
  assert.notDeepEqual(receiveConnectionsSummary(first), receiveConnectionsSummary(later));
  assert.notEqual(runtimeSummaryRevision(first), runtimeSummaryRevision({ ...later, counters: { clientRequests: 4 } }));
});
