import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';

test('帮助与关于页面保留双端公开项和安全边界', async () => {
  const [app, context, sidebar, help, about] = await Promise.all([
    readFile(new URL('../src/App.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/context/AppContext.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/components/Sidebar.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/pages/HelpPage.jsx', import.meta.url), 'utf8'),
    readFile(new URL('../src/pages/AboutPage.jsx', import.meta.url), 'utf8'),
  ]);

  for (const route of ['help', 'about']) {
    assert.match(app, new RegExp(`case '${route}'`));
    assert.match(context, new RegExp(`'${route}'`));
    assert.match(sidebar, new RegExp(`id: '${route}'`));
  }
  assert.match(help, /Domo Mido|USAGE\.md/);
  assert.match(about, /Domo Mido/);
  assert.match(about, /status\?\.version/);
  assert.match(help, /不要把 config\.json、API key、入站 Token/);
  assert.match(help, /github\.com\/domoxiaojun\/sumpter/);
  assert.doesNotMatch(help, /UnifiedAttributionPanel/);
  assert.match(help, /navigate\('security'\)/);
  assert.doesNotMatch(help, /setup-client-attribution\.sh install pi/);
});
