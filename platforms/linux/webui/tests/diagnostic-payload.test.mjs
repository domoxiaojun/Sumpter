import assert from 'node:assert/strict';
import test from 'node:test';
import { diagnosticPayloadText, diagnosticChunksText, hasCompressedCapture } from '../src/utils/diagnosticPayload.js';
import { failureKindLabel } from '../src/utils/helpers.js';

test('legacy text and reversible binary payloads have distinct presentations', () => {
  assert.equal(diagnosticPayloadText('中文'), '中文');
  assert.match(diagnosticPayloadText('H4sI', 'base64'), /Base64 · 二进制原始字节/);
  assert.match(diagnosticPayloadText('H4sI', 'base64'), /H4sI$/);
  assert.equal(diagnosticPayloadText('must not display', 'base64', true), '(已脱敏：原始数据已移除)');
  assert.equal(failureKindLabel('response_observation_failed'), '无法确认响应完成');
});

test('chunk metadata and legacy compression warnings remain explicit', () => {
  const headers = [{ name: 'Content-Encoding', value: 'identity, gzip' }];
  assert.equal(hasCompressedCapture(headers), true);
  assert.equal(hasCompressedCapture([{ name: 'content-encoding', value: 'IDENTITY' }]), false);
  const chunk = { atMS: 10, bytes: 3, data: 'AP8=', dataEncoding: 'base64', truncated: true };
  assert.match(diagnosticChunksText([chunk], true), /10ms · 3 bytes · 已截断/);
  assert.doesNotMatch(diagnosticChunksText([chunk], true), /旧格式/);
  assert.match(diagnosticChunksText([{ data: 'legacy' }], true), /无法保证还原/);
  assert.doesNotMatch(diagnosticChunksText([{ ...chunk, redacted: true }]), /AP8=/);
});
