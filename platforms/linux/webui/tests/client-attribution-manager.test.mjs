import test from 'node:test';
import assert from 'node:assert/strict';
import { attributionCommand, latestCollectorEvidence } from '../src/utils/clientAttribution.js';

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
