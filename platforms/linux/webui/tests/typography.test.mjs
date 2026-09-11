import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';

// 字体曾经只在 CSS 里声明却从未加载,Linux 上直接回退到 DejaVu;
// 这里锁住"自托管 + 回退链"两件事,防止再次漂回系统字体。
test('webui self-hosts Inter, Noto Sans SC and JetBrains Mono variable fonts', async () => {
  const main = await readFile(new URL('../src/main.jsx', import.meta.url), 'utf8');
  for (const pkg of ['@fontsource-variable/inter', '@fontsource-variable/noto-sans-sc', '@fontsource-variable/jetbrains-mono']) {
    assert.match(main, new RegExp(`import '${pkg}/index\\.css';`), `${pkg} 必须在 main.jsx 里引入`);
  }
  const pkgJson = JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8'));
  for (const pkg of ['@fontsource-variable/inter', '@fontsource-variable/noto-sans-sc', '@fontsource-variable/jetbrains-mono']) {
    assert.ok(pkgJson.dependencies[pkg], `${pkg} 必须是 dependencies`);
  }
});

test('font stacks lead with the bundled variable families and end with a CJK fallback', async () => {
  const variables = await readFile(new URL('../src/styles/variables.css', import.meta.url), 'utf8');
  assert.match(variables, /--font-sans: 'Inter Variable',[^;]*var\(--font-cjk\);/);
  assert.match(variables, /--font-mono: 'JetBrains Mono Variable',[^;]*var\(--font-cjk\);/);
  assert.match(variables, /--font-cjk: 'Noto Sans SC Variable',[^;]*sans-serif;/);
});

test('no stylesheet references a font family that is not bundled or a system fallback', async () => {
  const files = ['base.css', 'components.css', 'workspace.css', 'animations.css'];
  for (const file of files) {
    const css = await readFile(new URL(`../src/styles/${file}`, import.meta.url), 'utf8');
    assert.doesNotMatch(css, /Plus Jakarta Sans|'Inter'|"Inter"|'JetBrains Mono'(?! Variable)/, `${file} 引用了未打包的字体`);
  }
});
