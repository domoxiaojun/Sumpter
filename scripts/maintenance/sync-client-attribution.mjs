#!/usr/bin/env node
import { readFileSync, writeFileSync } from 'node:fs';
const source = readFileSync(new URL('../clients/client-attribution.mjs', import.meta.url));
const paths = [
  'platforms/linux/scripts/client-attribution.mjs',
  'platforms/macos/scripts/client-attribution.mjs',
  'platforms/macos/app/Sources/SumpterApp/Resources/client-attribution.mjs',
  // Legacy Gemini command remains a standalone download, generated from the same source.
  'scripts/clients/gemini-sumpter-wrapper.mjs',
  'platforms/linux/scripts/gemini-sumpter-wrapper.mjs',
  'platforms/macos/scripts/gemini-sumpter-wrapper.mjs',
  'platforms/macos/app/Sources/SumpterApp/Resources/gemini-sumpter-wrapper.mjs',
];
const check = process.argv.includes('--check');
const groups = [
  { source, paths },
  {
    source: readFileSync(new URL('../clients/pi-project-attribution.ts', import.meta.url)),
    paths: [
      'platforms/linux/scripts/pi-project-attribution.ts',
      'platforms/macos/scripts/pi-project-attribution.ts',
      'platforms/macos/app/Sources/SumpterApp/Resources/pi-project-attribution.ts',
    ],
  },
];
for (const { source, paths } of groups) {
for (const path of paths) {
  const url = new URL(`../../${path}`, import.meta.url);
  if (check) {
    try { if (!source.equals(readFileSync(url))) throw new Error(); }
    catch { console.error(`归因脚本副本需要同步：${path}`); process.exitCode = 1; }
  } else writeFileSync(url, source);
}
}
