import test from 'node:test';
import assert from 'node:assert/strict';
import { createRequestChainLoader, mergeRequestChain, chainInFlight } from '../src/utils/requestChain.js';

const row = (id, seq, phase = 'inFlight') => ({ id, seq, changeSeq: seq, requestID: 'r', kind: id === 'r' ? 'client' : 'upstream', phase });
const chain = (events, truncated = false) => ({ requestID: 'r', events, truncated });
const flush = async () => { for (let i = 0; i < 6; i++) await Promise.resolve(); };
function clock() {
  let now = 0, id = 0;
  const tasks = new Map();
  return {
    now: () => now,
    schedule: (fn, delay) => { tasks.set(++id, { fn, at: now + delay }); return id; },
    unschedule: key => tasks.delete(key),
    async tick(ms) {
      now += ms;
      for (const [key, task] of [...tasks]) if (task.at <= now) { tasks.delete(key); task.fn(); }
      await flush();
    },
  };
}

test('request chain merges new attempts, rejects older snapshots and bounds retained events', () => {
  const terminal = { ...row('r', 1, 'completed'), changeSeq: 10, statusCode: 499 };
  const merged = mergeRequestChain(chain([terminal]), [row('r', 1), row('a', 2)]);
  assert.equal(merged.events[0].statusCode, 499);
  assert.equal(merged.events.length, 2);
  assert.equal(chainInFlight(merged), false);
  assert.equal(mergeRequestChain(merged, [{ ...row('other', 3), requestID: 'elsewhere' }]).events.length, 2);
  const bounded = mergeRequestChain(chain([row('r', 1)]), Array.from({ length: 600 }, (_, i) => row(`a${i}`, i + 2)));
  assert.equal(bounded.events.length, 512);
  assert.equal(bounded.truncated, true);
});

test('slow chain reads never overlap or get cancelled by repeated updates; terminal stops polling', async () => {
  const time = clock(), values = [];
  let resolve, calls = 0, signal;
  const loader = createRequestChainLoader({ ...time, enabled: () => true, onValue: v => values.push(v),
    read: s => { calls++; signal = s; return new Promise(r => { resolve = r; }); } });
  loader.refresh(true); await time.tick(0);
  for (let i = 0; i < 20; i++) loader.refresh();
  await time.tick(5000);
  assert.equal(calls, 1); assert.equal(signal.aborted, false);
  resolve(chain([row('r', 1)])); await flush();
  await time.tick(1000); assert.equal(calls, 2);
  resolve(chain([row('r', 2, 'completed')])); await flush();
  await time.tick(5000); assert.equal(calls, 2); assert.equal(values.length, 2);
  loader.dispose();
});

test('pause hides in-flight refresh results, resume catches up, selection disposal ignores late results', async () => {
  const time = clock(), values = [];
  let resolve, calls = 0, enabled = true, signal;
  const loader = createRequestChainLoader({ ...time, enabled: () => enabled, onValue: v => values.push(v),
    read: s => { calls++; signal = s; return new Promise(r => { resolve = r; }); } });
  loader.refresh(true); await time.tick(0);
  resolve(chain([row('r', 1)])); await flush(); await time.tick(1000);
  enabled = false; loader.synchronize();
  resolve(chain([row('r', 2)])); await flush();
  await time.tick(5000); assert.equal(calls, 2); assert.equal(values.length, 1);
  enabled = true; loader.synchronize(); await time.tick(0); assert.equal(calls, 3);
  loader.dispose(); assert.equal(signal.aborted, true);
  resolve(chain([row('r', 3, 'completed')])); await flush();
  assert.equal(values.length, 1);
});

test('temporary first read failure recovers while an unsupported endpoint stops polling', async () => {
  for (const status of [503, 404]) {
    const time = clock(); let calls = 0;
    const loader = createRequestChainLoader({ ...time, enabled: () => true, onValue: () => {},
      read: async () => { calls++; if (calls === 1) throw Object.assign(new Error('test'), { status }); return chain([row('r', 1, 'completed')]); } });
    loader.refresh(true); await time.tick(0); await time.tick(1000); await time.tick(1000);
    assert.equal(calls, status === 503 ? 2 : 1);
    loader.dispose();
  }
});
