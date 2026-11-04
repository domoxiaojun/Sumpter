import test from 'node:test';
import assert from 'node:assert/strict';
import { attributionCommand, defaultAttributionProxyURL, latestCollectorEvidence } from '../src/utils/clientAttribution.js';

test('direct Admin pages use the configured proxy listener, including custom ports and IPv6', () => {
  assert.equal(defaultAttributionProxyURL('http://127.0.0.1:57879/#security',
    { host: '127.0.0.1', port: 57878 }, { host: '127.0.0.1', port: 57879 }), 'http://127.0.0.1:57878');
  assert.equal(defaultAttributionProxyURL('http://localhost:9001/admin/',
    { host: '127.0.0.1', port: 9000 }, { host: 'localhost', port: 9001 }), 'http://localhost:9000');
  assert.equal(defaultAttributionProxyURL('http://192.0.2.10:9001/',
    { host: '0.0.0.0', port: 9000 }, { host: '0.0.0.0', port: 9001 }), 'http://192.0.2.10:9000');
  assert.equal(defaultAttributionProxyURL('http://[::1]:57879/',
    { host: '::1', port: 57878 }, { host: '::', port: 57879 }), 'http://[::1]:57878');
});

test('public reverse-proxy addresses and missing listener metadata require an explicit URL', () => {
  const proxy = { host: '127.0.0.1', port: 57878 };
  const admin = { host: '127.0.0.1', port: 57879 };
  for (const url of ['https://gateway.invalid/', 'http://gateway.invalid/',
    'http://127.0.0.1:57879/sumpter/admin/', 'http://192.0.2.10:57879/']) {
    assert.equal(defaultAttributionProxyURL(url, proxy, admin), '');
  }
  assert.equal(defaultAttributionProxyURL('http://127.0.0.1:57879/', proxy), '');
  assert.equal(defaultAttributionProxyURL('http://127.0.0.1:57879/', { ...proxy, port: 0 }, admin), '');
  const custom = 'https://gateway.invalid/sumpter';
  assert.ok(attributionCommand('enable', 'claude', 'bash', custom).endsWith(`--proxy-url '${custom}'`));
});

test('client install commands validate endpoints and quote shell metacharacters', () => {
  assert.equal(attributionCommand('enable', 'codex', 'zsh', 'http://localhost:57878/v1'), "bash setup-client-attribution.sh enable codex --shell zsh --proxy-url 'http://localhost:57878/v1'");
  assert.equal(attributionCommand('disable', 'codex', 'zsh', ''), 'bash setup-client-attribution.sh disable codex --shell zsh');
  for (const url of ['https://user:secret@host', 'ftp://host', 'https://host?token=secret', 'https://host/#x', 'https://host/\nfoo']) {
    assert.throws(() => attributionCommand('enable', 'pi', 'bash', url));
  }
  assert.match(attributionCommand('enable', 'claude', 'bash', "http://localhost/a'$(id)"), /a'\\''\$\(id\)'$/u);
  assert.throws(() => attributionCommand('enable', 'codex;touch', 'bash', 'http://localhost'));
});

test('traffic evidence requires actual client event with collector and source, and can scope the computer', () => {
  const event = { kind: 'client', clientKind: 'codex', timestamp: 20, clientDeclared: { sourceId: 'host-a', collectorVersion: '2.0.0-test' } };
  assert.equal(latestCollectorEvidence([{ ...event, kind: 'upstream' }, { ...event, clientDeclared: {} }], 'codex'), null);
  assert.equal(latestCollectorEvidence([event], 'claude'), null);
  assert.equal(latestCollectorEvidence([event], 'codex', 'host-b'), null);
  assert.equal(latestCollectorEvidence([event, { ...event, timestamp: 10 }], 'all', 'host-a'), event);
});
