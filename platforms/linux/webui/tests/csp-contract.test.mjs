import assert from 'node:assert/strict';
import { readdir, readFile } from 'node:fs/promises';
import test from 'node:test';

import { chartColorVariables } from '../src/utils/chartColors.js';
import { errorSummary, isChunkLoadError } from '../src/utils/errorBoundary.js';

const read = (path) => readFile(new URL(path, import.meta.url), 'utf8');

async function adminCspDirectives() {
  const source = await read('../../../../adapters/linux/sumpter-linux-adapter/src/admin_auth_routes.rs');
  const policy = source.match(/"(default-src [^"]+)"/)?.[1];
  assert.ok(policy, '未能从 admin_auth_routes.rs 读到 Admin CSP');
  return new Map(policy.split(';').map((part) => {
    const [name, ...values] = part.trim().split(/\s+/);
    return [name, values];
  }));
}

async function sourceFiles(dir = '../src/') {
  const files = [];
  for (const entry of await readdir(new URL(dir, import.meta.url), { withFileTypes: true })) {
    if (entry.isDirectory()) files.push(...await sourceFiles(`${dir}${entry.name}/`));
    else if (/\.(jsx|js)$/.test(entry.name)) files.push(`${dir}${entry.name}`);
  }
  return files;
}

test('WebUI 不依赖 Admin CSP 禁止的内联样式', async () => {
  const csp = await adminCspDirectives();
  const styleSources = csp.get('style-src') ?? csp.get('default-src');
  if (styleSources.includes("'unsafe-inline'")) return;
  for (const file of await sourceFiles()) {
    // 只检查代码：说明性注释里可以提到 <style>。
    const source = (await read(file)).replace(/\/\*[\s\S]*?\*\//g, '').replace(/^\s*\/\/.*$/gm, '');
    assert.doesNotMatch(source, /<style[\s>]/, `${file} 渲染了会被 CSP 拦截的 <style>`);
    assert.doesNotMatch(source, /dangerouslySetInnerHTML/, `${file} 使用了 dangerouslySetInnerHTML`);
    assert.doesNotMatch(source, /createElement\(\s*['"]style['"]/, `${file} 在运行时创建 <style>`);
  }
});

test('构建产物不含 Admin CSP 禁止的 data: 资源，也不含演示数据', async () => {
  const csp = await adminCspDirectives();
  const fontSources = csp.get('font-src') ?? csp.get('default-src');
  const assets = await readdir(new URL('../../web/assets/', import.meta.url));
  if (!fontSources.includes('data:')) {
    assert.match(await read('../vite.config.js'), /assetsInlineLimit:\s*0/);
    for (const name of assets.filter((asset) => asset.endsWith('.css'))) {
      assert.doesNotMatch(await read(`../../web/assets/${name}`), /url\(\s*["']?data:font/, `${name} 内联了字体`);
    }
  }
  assert.deepEqual(assets.filter((asset) => /^mockApi/.test(asset)), [], '提交的 web/ 不应包含演示数据分包');
});

test('图表系列颜色以容器 CSS 变量给出', () => {
  assert.deepEqual(chartColorVariables({ value: { label: '请求', color: 'var(--chart-1)' } }), {
    '--color-value': 'var(--chart-1)',
  });
  assert.deepEqual(chartColorVariables({ a: { theme: { light: '#111', dark: '#eee' } }, b: { label: '无颜色' } }), {
    '--color-a': 'light-dark(#111, #eee)',
  });
  assert.deepEqual(chartColorVariables({ a: { theme: { dark: '#eee' } } }), { '--color-a': '#eee' });
  assert.deepEqual(chartColorVariables(undefined), {});
});

test('分包加载失败与普通渲染异常分开识别', () => {
  for (const message of [
    'Failed to fetch dynamically imported module: http://127.0.0.1:57879/admin/assets/StatsPage-abc.js',
    'error loading dynamically imported module: http://host/admin/assets/RunPage-abc.js',
    'Importing a module script failed.',
    'Unable to preload CSS for /admin/assets/index-abc.css',
  ]) {
    assert.equal(isChunkLoadError(new TypeError(message)), true, message);
  }
  assert.equal(isChunkLoadError(new TypeError("Cannot read properties of undefined (reading 'map')")), false);
  assert.equal(isChunkLoadError(null), false);
  assert.equal(errorSummary(new Error('x'.repeat(400))).length, 301);
  assert.equal(errorSummary(undefined), '');
});
