import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, realpathSync, rmSync, symlinkSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const script = fileURLToPath(new URL('../../platforms/linux/scripts/migrate-kekulv.sh', import.meta.url));
const sha = bytes => createHash('sha256').update(bytes).digest('hex');

// The package installer has its own transaction suite. Here its interface and
// systemd/network responses are faked, while filesystem copies/moves are real.
function fixture(t, mode = '') {
  const base = realpathSync(mkdtempSync(join(tmpdir(), 'sumpter-installer-test.')));
  t.after(() => rmSync(base, { recursive: true, force: true }));
  const root = join(base, 'rootfs');
  const file = (path, value, perms) => {
    mkdirSync(resolve(path, '..'), { recursive: true });
    writeFileSync(path, value, perms ? { mode: perms } : undefined);
  };
  const data = join(root, 'var/lib/kekulv');
  file(join(data, 'config.json'), '{"schemaVersion":6,"endpoints":[],"listener":{"authToken":"synthetic"}}\n');
  file(join(data, 'admin-password'), 'synthetic-admin\n', 0o600);
  file(join(data, 'runtime.sqlite3'), Buffer.from([0, 1, 255, 6, 4, 8]));
  file(join(data, 'runtime.sqlite3-wal'), 'synthetic-wal');
  file(join(data, 'nested/stats.json'), '{"requests":73}');
  file(join(root, 'opt/kekulv/kekulvd'), 'old-executable');
  file(join(root, 'opt/kekulv.previous/kekulvd'), 'older-executable');
  file(join(root, 'etc/systemd/system/kekulv.service'), '[Service]\nUser=kekulv\n');
  file(join(root, 'etc/systemd/system/kekulv.service.d/50-admin-listen.conf'), '[Service]\nEnvironment=KEKULV_ADMIN_HOST=0.0.0.0\nEnvironment=KEKULV_ADMIN_PORT=57879\n');
  const statePath = join(base, 'state.json');
  const initial = { oldActive: !mode.startsWith('inactive'), oldEnabled: !mode.startsWith('inactive'), newActive: false, newEnabled: false };
  file(statePath, JSON.stringify(initial));
  const shim = `#!${process.execPath}
const fs = require('node:fs'), path = require('node:path');
const base = process.env.MIGRATION_FIXTURE, root = base + '/rootfs';
const mode = process.env.MIGRATION_MODE, args = process.argv.slice(2);
const command = path.basename(process.argv[1]);
const statePath = base + '/state.json';
const s = JSON.parse(fs.readFileSync(statePath, 'utf8'));
const exists = p => fs.existsSync(p);
fs.appendFileSync(base + '/commands.log', command + ' ' + args.join(' ') + '\\n');
function end(code=0, output='') { process.stdout.write(output); process.exit(code); }
if (command === 'sleep') end();
if (command === 'mv') {
  const [src, dest] = args.filter(a => a !== '--');
  if (mode === 'retire-failure' && src.endsWith('/kekulv.service') && dest.endsWith('/unit')) end(1);
  fs.renameSync(src, exists(dest) && fs.statSync(dest).isDirectory() ? path.join(dest, path.basename(src)) : dest);
  end();
}
if (command === 'curl') {
  const url = args.find(a => /^https?:/.test(a));
  if (url.startsWith('http:')) end(0, mode === 'health-failure' ? '500' : '204');
  if (mode === 'download-failure') end(22);
  const dest = args[args.indexOf('--output') + 1];
  fs.copyFileSync(base + (url.endsWith('SHA256SUMS') ? '/SHA256SUMS' : '/release.tar.gz'), dest);
  end();
}
if (command !== 'systemctl') end(99);
const old = args.includes('kekulv.service'), unit = old ? 'kekulv' : 'sumpter';
const prefix = old ? 'old' : 'new', unitPath = root + '/etc/systemd/system/' + unit + '.service';
switch (args[0]) {
  case 'show-environment': case 'daemon-reload': end(); break;
  case 'show': {
    const property = args[args.indexOf('-p') + 1];
    const properties = {
      LoadState: exists(unitPath) ? 'loaded' : 'not-found', FragmentPath: unitPath,
      ExecStart: '{ path=/opt/kekulv/kekulvd ; argv[]=/opt/kekulv/kekulvd --systemd-scope system --config-dir /var/lib/kekulv --web-root /opt/kekulv/web ; ignore_errors=no ; }',
      User: 'kekulv', DropInPaths: exists(unitPath+'.d') ? unitPath+'.d/50-admin-listen.conf' : '',
      MainPID: s[prefix+'Active'] ? '4321' : '0',
    };
    if (!(property in properties)) end(98);
    end(0, properties[property]+'\\n'); break;
  }
  case 'is-active': end(s[prefix+'Active'] ? 0 : 3); break;
  case 'is-enabled': end(s[prefix+'Enabled'] ? 0 : 1, s[prefix+'Enabled'] ? 'enabled\\n' : 'disabled\\n'); break;
  case 'stop': if (old && mode === 'stop-failure') end(1); s[prefix+'Active'] = false; break;
  case 'start': s[prefix+'Active'] = true; break;
  case 'enable': s[prefix+'Enabled'] = true; break;
  case 'disable': s[prefix+'Enabled'] = false; break;
  default: end(97);
}
fs.writeFileSync(statePath, JSON.stringify(s));
`;
  for (const name of ['systemctl', 'curl', 'sleep']) file(join(base, 'bin', name), shim, 0o755);
  if (mode === 'retire-failure') file(join(base, 'bin', 'mv'), shim, 0o755);
  file(join(base, 'bin', 'realpath'), `#!${process.execPath}
const fs = require('node:fs');
let args = process.argv.slice(2); if (args[0] === '-e') args.shift();
process.stdout.write(fs.realpathSync(args[0]) + '\\n');
`, 0o755);
  const machine = execFileSync('uname', ['-m'], { encoding: 'utf8' }).trim();
  const arch = /arm64|aarch64/.test(machine) ? 'aarch64' : 'x86_64';
  const packageName = `sumpter-linux-${arch}`;
  const pkg = join(base, 'archive', packageName);
  file(join(pkg, 'scripts/install.sh'), `#!/usr/bin/env bash
set -euo pipefail
root=$SUMPTER_SYSTEM_TEST_ROOT
printf '%s\\n' "$*" > "$MIGRATION_FIXTURE/install-args"
mkdir -p "$root/opt/sumpter" "$root/etc/systemd/system/sumpter.service.d"
printf new > "$root/opt/sumpter/sumpterd"
printf new > "$root/etc/systemd/system/sumpter.service"
systemctl enable sumpter.service
systemctl start sumpter.service
if [[ $MIGRATION_MODE == *install-failure ]]; then exit 41; fi
`, 0o755);
  for (const path of ['sumpterd', 'config.example.json', 'sumpter-system.service', 'web/index.html']) file(join(pkg, path), 'fixture');
  if (mode === 'archive-symlink') symlinkSync('/etc/passwd', join(pkg, 'outside'));
  execFileSync('tar', ['-czf', join(base, 'release.tar.gz'), '-C', join(base, 'archive'), packageName]);
  file(join(base, 'SHA256SUMS'), `${mode === 'bad-checksum' ? '0'.repeat(64) : sha(readFileSync(join(base, 'release.tar.gz')))}  ${packageName}.tar.gz\n`);
  const env = { ...process.env, PATH: `${join(base, 'bin')}:${process.env.PATH}`, SUMPTER_MIGRATION_SELFTEST: '1', SUMPTER_INSTALLER_SELFTEST: '1', SUMPTER_SYSTEM_TEST_ROOT: root, MIGRATION_FIXTURE: base, MIGRATION_MODE: mode };
  return {
    base, root, data, file, initial,
    run: (...args) => spawnSync('bash', [script, ...args], { env, encoding: 'utf8', timeout: 20000 }),
    state: () => JSON.parse(readFileSync(statePath, 'utf8')),
    log: () => existsSync(join(base, 'commands.log')) ? readFileSync(join(base, 'commands.log'), 'utf8') : '',
  };
}

test('successful migration keeps source bytes, carries data and retires old units only after health checks', t => {
  const f = fixture(t);
  const before = readFileSync(join(f.data, 'config.json'));
  const result = f.run('--version', 'v0.3.7');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  for (const name of ['config.json', 'admin-password', 'runtime.sqlite3', 'runtime.sqlite3-wal', 'nested/stats.json']) {
    assert.deepEqual(readFileSync(join(f.root, 'var/lib/sumpter', name)), readFileSync(join(f.data, name)));
    assert.equal(statSync(join(f.root, 'var/lib/sumpter', name)).mode & 0o777, 0o600);
  }
  assert.deepEqual(readFileSync(join(f.data, 'config.json')), before);
  assert.deepEqual(f.state(), { oldActive: false, oldEnabled: false, newActive: true, newEnabled: true });
  assert.equal(existsSync(join(f.root, 'opt/kekulv')), false);
  assert.equal(existsSync(join(f.root, 'etc/systemd/system/kekulv.service')), false);
  assert.match(readFileSync(join(f.base, 'install-args'), 'utf8'), /--admin-host 0.0.0.0 --admin-port 57879/);
  const log = f.log();
  assert.ok(log.indexOf('/SHA256SUMS') < log.indexOf('systemctl stop kekulv.service'));
  assert.equal((log.match(/\/healthz/g) || []).length, 5);
  const repeat = f.run();
  assert.notEqual(repeat.status, 0, 'repeated migration must refuse to replace the new installation');
  assert.deepEqual(f.state(), { oldActive: false, oldEnabled: false, newActive: true, newEnabled: true });
});

for (const mode of ['download-failure', 'bad-checksum', 'archive-symlink']) {
  test(`${mode} leaves the running old service and data untouched`, t => {
    const f = fixture(t, mode);
    const result = f.run();
    assert.notEqual(result.status, 0);
    assert.deepEqual(f.state(), f.initial);
    assert.doesNotMatch(f.log(), /systemctl (stop|disable|start)/);
    assert.equal(existsSync(join(f.root, 'var/lib/sumpter')), false);
  });
}

for (const mode of ['install-failure', 'health-failure', 'stop-failure', 'inactive-install-failure', 'retire-failure']) {
  test(`${mode} restores the old service without removing its data or leaving Sumpter active`, t => {
    const f = fixture(t, mode);
    const before = readFileSync(join(f.data, 'runtime.sqlite3-wal'));
    const result = f.run();
    assert.notEqual(result.status, 0);
    assert.match(result.stdout, /已恢复旧服务状态/);
    assert.deepEqual(f.state(), f.initial);
    assert.deepEqual(readFileSync(join(f.data, 'runtime.sqlite3-wal')), before);
    assert.equal(existsSync(join(f.root, 'var/lib/sumpter')), false);
    assert.equal(existsSync(join(f.root, 'opt/kekulv/kekulvd')), true);
  });
}

test('check mode and conflicting targets never stop the old service', t => {
  const f = fixture(t);
  const result = f.run('--check');
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(f.log(), /curl|systemctl (stop|disable|start)/);
  f.file(join(f.root, 'var/lib/sumpter/config.json'), 'existing-new-config');
  assert.notEqual(f.run().status, 0);
  assert.deepEqual(f.state(), f.initial);
  assert.equal(readFileSync(join(f.root, 'var/lib/sumpter/config.json'), 'utf8'), 'existing-new-config');
});

test('symlinked old data and external password overrides are rejected before any stop', t => {
  const f = fixture(t);
  const outside = join(f.base, 'outside');
  f.file(outside, 'unrelated');
  symlinkSync(outside, join(f.data, 'unexpected-link'));
  assert.notEqual(f.run().status, 0);
  rmSync(join(f.data, 'unexpected-link'));
  f.file(join(f.root, 'etc/systemd/system/kekulv.service.d/50-admin-listen.conf'), '[Service]\nEnvironment=KEKULV_ADMIN_PASSWORD_FILE=/custom/password\n');
  assert.notEqual(f.run().status, 0);
  assert.deepEqual(f.state(), f.initial);
  assert.equal(readFileSync(outside, 'utf8'), 'unrelated');
});

test('invalid arguments are rejected before a download and explicit host/port overrides are forwarded', t => {
  const f = fixture(t);
  for (const args of [['--version', '../../main'], ['--admin-port', '99999'], ['--admin-host', 'x\nY'], ['--admin-host', '999.1.1.1'], ['--purge']]) {
    assert.notEqual(f.run(...args).status, 0);
  }
  const result = f.run('--admin-host', '127.0.0.1', '--admin-port', '57880');
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.match(readFileSync(join(f.base, 'install-args'), 'utf8'), /--admin-host 127.0.0.1 --admin-port 57880/);
});
