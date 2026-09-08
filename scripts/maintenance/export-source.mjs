#!/usr/bin/env node
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, lstatSync, mkdirSync, readFileSync, readlinkSync, realpathSync, symlinkSync, renameSync, rmSync, writeFileSync, copyFileSync, chmodSync } from 'node:fs';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

export function exportSource(source, destination) {
  const root = realpathSync(source);
  const requested = resolve(destination);
  // Resolve the existing parent so symlink aliases cannot put output inside source.
  const target = resolve(realpathSync(dirname(requested)), requested.split(sep).at(-1));
  const inside = relative(root, target);
  if (!inside || (!inside.startsWith(`..${sep}`) && inside !== '..' && !isAbsolute(inside))) {
    throw new Error('目标必须在源仓库外');
  }
  const manifestPath = `${target}.manifest.json`;
  if (existsSync(target) || existsSync(manifestPath)) throw new Error('目标或旁置清单已存在，拒绝覆盖');
  const listed = execFileSync('git', ['ls-files', '-z', '--cached', '--others', '--exclude-standard'], { cwd: root }).toString().split('\0').filter(Boolean);
  const skipped = new Set(['plan.md', 'todos.md', 'SOURCE_COMMIT']);
  const files = [];
  for (const name of [...new Set(listed)].sort()) {
    if (skipped.has(name) || name.startsWith('.claude/')) continue;
    if (isAbsolute(name) || name.split('/').some(p => p === '..')) throw new Error('非法源码路径');
    const path = resolve(root, name);
    let stat;
    try { stat = lstatSync(path); } catch (error) {
      if (error.code === 'ENOENT') continue;
      throw error;
    }
    // Check every ancestor too: an untracked child under a symlink can escape root.
    for (let ancestor = dirname(path); ancestor !== root; ancestor = dirname(ancestor)) {
      if (lstatSync(ancestor).isSymbolicLink()) throw new Error(`拒绝符号链接目录: ${name}`);
    }
    if (!stat.isFile() && !stat.isSymbolicLink()) throw new Error(`只允许普通文件或内部链接: ${name}`);
    if (/(^|\/)(\.git|\.env(?:\..*)?|\.venv|\.audit|node_modules|target|dist|\.build|__pycache__|config\.json|keys\.json|admin-password|\.control_token)(\/|$)/i.test(name)
      || /\.(db|sqlite|sqlite3)(?:-(?:wal|shm))?$|\.(dmg|zip|log|pem|p12|pfx|key)$|\.bak(?:-|$)/i.test(name)) {
      throw new Error(`拒绝运行数据、凭据或缓存: ${name}`);
    }
    let link;
    if (stat.isSymbolicLink()) {
      link = readlinkSync(path);
      const resolved = realpathSync(path);
      const local = relative(root, resolved);
      if (isAbsolute(link) || local.startsWith(`..${sep}`) || isAbsolute(local)) {
        throw new Error(`拒绝指向仓库外的链接: ${name}`);
      }
    }
    files.push({ name, path, mode: stat.mode & 0o777, link });
  }
  if (!files.length) throw new Error('没有可导出的源码');
  const included = new Set(files.map(file => file.path));
  for (const file of files.filter(file => file.link !== undefined)) {
    // Both the immediate target and resolved target must be part of the snapshot.
    if (!included.has(resolve(dirname(file.path), file.link)) || !included.has(realpathSync(file.path))) {
      throw new Error(`链接目标不在导出清单内: ${file.name}`);
    }
  }
  const stage = `${target}.stage-${process.pid}`;
  mkdirSync(stage); // Fail if a previous stage exists; never reuse it.
  let published = false;
  try {
    const manifest = [];
    for (const file of files) {
      const output = resolve(stage, file.name);
      mkdirSync(dirname(output), { recursive: true });
      if (file.link !== undefined) symlinkSync(file.link, output);
      else {
        copyFileSync(file.path, output);
        chmodSync(output, file.mode);
      }
      const bytes = file.link !== undefined ? Buffer.from(file.link) : readFileSync(output);
      manifest.push({ path: file.name, type: file.link !== undefined ? 'symlink' : 'file', sha256: createHash('sha256').update(bytes).digest('hex') });
    }
    writeFileSync(manifestPath, `${JSON.stringify({ files: manifest }, null, 2)}\n`, { flag: 'wx' });
    renameSync(stage, target);
    published = true;
    return { target, manifestPath, count: manifest.length };
  } finally {
    if (!published) {
      rmSync(stage, { recursive: true, force: true });
      // A failed exclusive manifest write must never remove somebody else's file.
    }
  }
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 3) {
    console.error('Usage: node scripts/maintenance/export-source.mjs /absolute/path/to/new-source');
    process.exit(2);
  }
  try {
    console.log(JSON.stringify(exportSource(fileURLToPath(new URL('../../', import.meta.url)), process.argv[2]), null, 2));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
