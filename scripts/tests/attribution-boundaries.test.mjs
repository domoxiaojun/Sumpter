import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, readdirSync, existsSync, utimesSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { connectionBinding, prepareLaunch, manage } from '../clients/client-attribution.mjs';

const launcherURL = new URL('../clients/client-attribution.mjs', import.meta.url).href;

function fixture(t) {
  const home = mkdtempSync(join(tmpdir(), 'sumpter-attribution-boundaries-'));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const env = { HOME: home, XDG_DATA_HOME: join(home, '.local/share'), USER: 'tester', PATH: process.env.PATH,
    CODEX_HOME: join(home, '.codex'), SUMPTER_AUTH_TOKEN: 'synthetic-token' };
  mkdirSync(env.CODEX_HOME);
  return { home, env };
}

function clientEnvironment(f, actual, expected) {
  writeFileSync(join(f.env.CODEX_HOME, 'config.toml'), `model_provider="test"\n[model_providers.test]\nbase_url=${JSON.stringify(actual)}\n`);
  return { ...f.env, SUMPTER_BASE_URL: expected, ANTHROPIC_BASE_URL: actual,
    SUMPTER_GEMINI_BASE_URL: actual,
    GROK_CONFIG: JSON.stringify({ endpoints: { xai_api_base_url: actual, cli_chat_proxy_base_url: actual } }) };
}

for (const client of ['claude', 'grok', 'gemini', 'codex']) {
  test(`${client} binding keeps mount prefixes while accepting its API version suffix`, t => {
    const f = fixture(t);
    const base = 'https://gateway.example.invalid';
    const suffix = client === 'gemini' ? '/v1beta' : '/v1';
    for (const [expected, actual] of [
      [base, base + suffix], [base + suffix, base],
      [base + '/sumpter', base + '/sumpter/'],
      [base + '/sumpter', base + '/sumpter' + suffix],
      [base + '/sumpter' + suffix, base + '/sumpter'],
    ]) {
      const env = clientEnvironment(f, actual, expected);
      assert.equal(connectionBinding(client, [], env, f.home, expected), 'matched', `${expected} -> ${actual}`);
      assert.equal(prepareLaunch(client, [], env, f.home).attribution.enabled, true);
    }
    for (const actual of [base, base + '/other-service', base + '/sumpter-other',
      base + '/sumpter/other', base + '/Sumpter', base + '/other-service' + suffix]) {
      const expected = base + '/sumpter';
      const env = clientEnvironment(f, actual, expected);
      assert.equal(connectionBinding(client, [], env, f.home, expected), 'mismatch', actual);
      const launch = prepareLaunch(client, [], env, f.home);
      assert.equal(launch.attribution.enabled, false);
      assert.ok(!JSON.stringify(launch).includes('X-Sumpter-Workspace'), actual);
    }
  });
}

test('binding rejects ambiguous or credential-bearing URLs before collecting workspace headers', t => {
  const f = fixture(t);
  for (const actual of ['https://user:secret@gateway.invalid/sumpter',
    'https://gateway.invalid/sumpter?key=secret', 'https://gateway.invalid/sumpter#fragment',
    'https://gateway.invalid/sumpter\n', 'file:///sumpter']) {
    const expected = 'https://gateway.invalid/sumpter';
    const env = clientEnvironment(f, actual, expected);
    assert.equal(connectionBinding('claude', [], env, f.home, expected), 'unknown');
    assert.equal(prepareLaunch('claude', [], env, f.home).attribution.enabled, false);
  }
});

async function lockProcess(t, dir, code) {
  const child = spawn(process.execPath, ['--input-type=module', '-e', `
    import { acquireInstallLock } from ${JSON.stringify(launcherURL)};
    const dir = process.env.SUMPTER_TEST_LOCK_DIR;
    ${code}
  `], { env: { ...process.env, SUMPTER_TEST_LOCK_DIR: dir }, stdio: ['ignore', 'ignore', 'pipe', 'ipc'] });
  let errors = '';
  child.stderr.on('data', chunk => { errors += chunk; });
  t.after(() => { if (child.exitCode == null && child.signalCode == null) child.kill('SIGKILL'); });
  await new Promise((resolve, reject) => {
    child.once('message', resolve);
    child.once('error', reject);
    child.once('exit', () => reject(new Error(errors || 'lock child exited before ready')));
  });
  return child;
}

test('live installation owners remain protected and a killed owner no longer blocks disable', { timeout: 10_000 }, async t => {
  const f = fixture(t);
  const options = { shell: 'bash', rc: join(f.home, '.bashrc') };
  manage('enable', 'claude', options, f.env);
  const dir = join(f.env.XDG_DATA_HOME, 'sumpter/attribution');
  const lock = join(dir, 'install.lock');
  const child = await lockProcess(t, dir, `
    acquireInstallLock(dir);
    process.send('locked');
    setInterval(() => {}, 60_000);
  `);
  const owner = JSON.parse(readFileSync(join(lock, readdirSync(lock)[0]), 'utf8'));
  assert.equal(owner.pid, child.pid);
  const old = new Date(Date.now() - 24 * 3600_000);
  utimesSync(lock, old, old);
  assert.throws(() => manage('disable', 'claude', options, f.env), /另一个归因管理进程/);
  assert.equal(manage('status', 'claude', options, f.env)[0].enabled, true);
  const exited = once(child, 'exit');
  child.kill('SIGKILL');
  await exited;
  assert.equal(manage('disable', 'claude', options, f.env)[0].enabled, false);
  assert.equal(existsSync(lock), false);
  const policy = JSON.parse(readFileSync(join(dir, 'collection.json'), 'utf8'));
  assert.equal(policy.clients.claude.enabled, false);
});

test('legacy empty locks have a grace period and stale ones no longer block uninstall', t => {
  const f = fixture(t);
  const options = { shell: 'bash', rc: join(f.home, '.bashrc') };
  manage('enable', 'claude', options, f.env);
  const lock = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/install.lock');
  writeFileSync(lock, '', { mode: 0o600 });
  assert.throws(() => manage('uninstall', 'claude', options, f.env), /等待 5 分钟/);
  assert.equal(manage('status', 'claude', options, f.env)[0].enabled, true);
  const stale = new Date(Date.now() - 6 * 60_000);
  utimesSync(lock, stale, stale);
  const result = manage('uninstall', 'claude', options, f.env)[0];
  assert.equal(result.enabled, false);
  assert.equal(result.registration, 'absent');
  assert.equal(existsSync(lock), false);
});

test('concurrent stale-lock recoverers cannot steal a replacement owner', { timeout: 10_000 }, async t => {
  const f = fixture(t);
  const dir = join(f.home, 'locks');
  const holder = await lockProcess(t, dir, `
    acquireInstallLock(dir); process.send('locked'); setInterval(() => {}, 60_000);
  `);
  const exited = once(holder, 'exit');
  holder.kill('SIGKILL');
  await exited;
  const workers = await Promise.all(Array.from({ length: 8 }, () => lockProcess(t, dir, `
    import { writeFileSync, unlinkSync } from 'node:fs';
    import { join } from 'node:path';
    process.send('ready');
    process.once('message', () => {
      let release;
      try {
        release = acquireInstallLock(dir);
        writeFileSync(join(dir, 'critical'), String(process.pid), { flag: 'wx' });
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 100);
        unlinkSync(join(dir, 'critical'));
        process.send('entered');
      } catch (error) {
        if (!/归因管理进程|安装锁正在被其他进程/u.test(error.message)) {
          console.error(error.message); process.exitCode = 1;
        }
      } finally { release?.(); process.disconnect(); }
    });
  `)));
  let entered = 0;
  const results = workers.map(child => {
    child.on('message', value => { if (value === 'entered') entered++; });
    return once(child, 'exit');
  });
  for (const child of workers) child.send('go');
  for (const [code, signal] of await Promise.all(results)) assert.deepEqual([code, signal], [0, null]);
  assert.ok(entered > 0);
  assert.equal(existsSync(join(dir, 'install.lock')), false);
  assert.equal(existsSync(join(dir, 'critical')), false);
  assert.deepEqual(readdirSync(dir), []);
});
