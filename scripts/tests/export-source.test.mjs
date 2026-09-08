import test from 'node:test';
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readlinkSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { exportSource } from '../maintenance/export-source.mjs';

function fixture(t) {
  const parent = mkdtempSync(join(tmpdir(), 'sumpter-export-test-'));
  t.after(() => rmSync(parent, { recursive: true, force: true }));
  const source = join(parent, 'source');
  mkdirSync(source);
  execFileSync('git', ['init', '-q'], { cwd: source });
  return { parent, source, target: join(parent, 'export') };
}
test('exports current bytes, new files and executable mode without history or ignored data', t => {
  const { parent, source, target } = fixture(t);
  writeFileSync(join(source, '.gitignore'), 'target/\nconfig.json\n');
  writeFileSync(join(source, 'app.sh'), '#!/bin/sh\nexit 0\n');
  chmodSync(join(source, 'app.sh'), 0o755);
  writeFileSync(join(source, 'deleted.txt'), 'gone');
  writeFileSync(join(source, 'plan.md'), 'local');
  mkdirSync(join(source, '.claude'));
  writeFileSync(join(source, '.claude', 'settings.local.json'), '{}');
  execFileSync('git', ['add', '.'], { cwd: source });
  rmSync(join(source, 'deleted.txt'));
  writeFileSync(join(source, 'new.md'), 'new content');
  writeFileSync(join(source, 'config.json'), 'private');
  writeFileSync(join(source, 'app.sh'), '#!/bin/sh\nexit 1\n');
  const result = exportSource(source, target);
  assert.equal(result.count, 3);
  assert.equal(readFileSync(join(target, 'app.sh'), 'utf8'), '#!/bin/sh\nexit 1\n');
  assert.ok(statSync(join(target, 'app.sh')).mode & 0o111);
  for (const name of ['.git', 'config.json', 'plan.md', 'deleted.txt']) assert.equal(existsSync(join(target, name)), false);
  const manifest = JSON.parse(readFileSync(result.manifestPath));
  for (const file of manifest.files) assert.equal(file.sha256, createHash('sha256').update(readFileSync(join(target, file.path))).digest('hex'));
  assert.equal(realpathSync(join(parent, 'export.manifest.json')), result.manifestPath);
  assert.throws(() => exportSource(source, target), /已存在/);
});
test('rejects tracked runtime data before creating output', t => {
  const { source, target } = fixture(t);
  writeFileSync(join(source, 'config.json'), 'private');
  execFileSync('git', ['add', '.'], { cwd: source });
  assert.throws(() => exportSource(source, target), /拒绝运行数据/);
  assert.equal(existsSync(target), false);
});
test('rejects symlink source files and destination aliases inside repository', t => {
  const { parent, source, target } = fixture(t);
  writeFileSync(join(parent, 'outside'), 'private');
  symlinkSync(join(parent, 'outside'), join(source, 'link'));
  assert.throws(() => exportSource(source, target), /仓库外的链接/);
  symlinkSync(source, join(parent, 'alias'));
  assert.throws(() => exportSource(source, join(parent, 'alias', 'nested')), /仓库外/);
});

test('preserves internal links and rejects targets omitted from the snapshot', t => {
  const { source, target } = fixture(t);
  writeFileSync(join(source, 'resource.sh'), 'resource');
  symlinkSync('resource.sh', join(source, 'alias.sh'));
  exportSource(source, target);
  assert.equal(readlinkSync(join(target, 'alias.sh')), 'resource.sh');
  assert.equal(readFileSync(join(target, 'alias.sh'), 'utf8'), 'resource');
  writeFileSync(join(source, 'plan.md'), 'local only');
  symlinkSync('plan.md', join(source, 'plan-alias'));
  assert.throws(() => exportSource(source, `${target}-second`), /不在导出清单/);
});
