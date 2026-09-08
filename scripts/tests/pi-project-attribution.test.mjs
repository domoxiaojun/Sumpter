import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, rm, readFile, realpath } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import extension, { sanitizeRemote } from '../clients/pi-project-attribution.ts';

test('pi attribution follows current workspace/session, opts in per provider, and clears stale headers', async () => {
  const root = await realpath(await mkdtemp(join(tmpdir(), 'sumpter-pi-')));
  try {
    const repo = join(root, '中文 project');
    const nested = join(repo, 'nested');
    const plain = join(root, 'plain');
    await mkdir(nested, { recursive: true });
    await mkdir(plain);
    execFileSync('git', ['init', '-q', repo]);
    execFileSync('git', ['-C', repo, 'config', 'remote.origin.url', 'https://user:secret@example.invalid/team/repo.git?token=secret#private']);
    let handler;
    extension({ on: (name, callback) => { assert.equal(name, 'before_provider_headers'); handler = callback; } });
    let session = 'session-a';
    const ctx = { cwd: nested, sessionManager: { getSessionId: () => session } };
    const untouched = { Authorization: 'Bearer synthetic' };
    await handler({ headers: untouched }, ctx);
    assert.deepEqual(untouched, { Authorization: 'Bearer synthetic' });
    const headers = { 'X-Sumpter-Client': 'pi', Authorization: 'Bearer synthetic' };
    await handler({ headers }, ctx);
    assert.equal(headers['x-sumpter-session-id'], 'session-a');
    assert.equal(decodeURIComponent(headers['x-sumpter-workspace']), repo);
    assert.equal(decodeURIComponent(headers['x-sumpter-project']), '中文 project');
    assert.equal(decodeURIComponent(headers['x-sumpter-git-remote']), 'https://example.invalid/team/repo.git');
    assert.equal(headers.Authorization, 'Bearer synthetic');
    for (const value of Object.values(headers)) if (value !== null) assert.doesNotThrow(() => new Headers({ test: value }));
    session = 'fork-b'; ctx.cwd = plain;
    await handler({ headers }, ctx);
    assert.equal(headers['x-sumpter-session-id'], 'fork-b');
    assert.equal(decodeURIComponent(headers['x-sumpter-workspace']), plain);
    assert.equal(headers['x-sumpter-git-remote'], null);
    ctx.cwd = join(root, 'missing'); session = 'resume-c';
    await handler({ headers }, ctx);
    assert.equal(headers['x-sumpter-session-id'], 'resume-c');
    assert.equal(headers['x-sumpter-git-remote'], null);
    const conflict = { 'x-sumpter-client': 'pi', 'X-Sumpter-Client': 'other' };
    await handler({ headers: conflict }, ctx);
    assert.equal(conflict['x-sumpter-workspace'], undefined);
  } finally { await rm(root, { recursive: true, force: true }); }
});

test('remote sanitizer rejects unsupported paths and removes credentials', () => {
  assert.equal(sanitizeRemote('git@example.invalid:team/repo.git?secret=x'), 'ssh://example.invalid/team/repo.git');
  assert.equal(sanitizeRemote('ssh://user:password@example.invalid/team/repo.git#secret'), 'ssh://example.invalid/team/repo.git');
  for (const value of [undefined, '/local/path', 'file:///secret', 'https://example.invalid/\nsecret']) assert.equal(sanitizeRemote(value), undefined);
});

test('Linux and macOS package resources match the canonical pi extension', async () => {
  const source = await readFile(new URL('../clients/pi-project-attribution.ts', import.meta.url), 'utf8');
  for (const path of ['platforms/linux/scripts/pi-project-attribution.ts', 'platforms/macos/scripts/pi-project-attribution.ts', 'platforms/macos/app/Sources/SumpterApp/Resources/pi-project-attribution.ts']) {
    assert.equal(await readFile(new URL(`../../${path}`, import.meta.url), 'utf8'), source);
  }
});
