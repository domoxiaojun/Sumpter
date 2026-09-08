#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';
const root = new URL('../../', import.meta.url);
const mode = process.argv[2] ?? '--check';
if (!['--check', '--write'].includes(mode) || process.argv.length > 3) {
  console.error('Usage: node scripts/maintenance/sync-project-metadata.mjs [--check|--write]');
  process.exit(2);
}
const copies = [
  ['LICENSE', 'platforms/linux/LICENSE'],
  ['CHANGELOG.md', 'platforms/linux/CHANGELOG.md'],
  ['config.example.json', 'platforms/linux/config.example.json'],
  ['config.example.json', 'platforms/macos/config.example.json'],
];
for (const [source, target] of copies) {
  const expected = readFileSync(new URL(source, root));
  if (mode === '--write') writeFileSync(new URL(target, root), expected);
  else if (!expected.equals(readFileSync(new URL(target, root)))) {
    console.error(`需要同步: ${target} (源: ${source})`);
    process.exitCode = 1;
  }
}
if (!process.exitCode) console.log(`项目元数据${mode === '--write' ? '同步' : '检查'}通过`);
