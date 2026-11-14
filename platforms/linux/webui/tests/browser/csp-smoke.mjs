#!/usr/bin/env node
// 浏览器冒烟：用演示数据构建 WebUI，按 daemon 的 Admin CSP 提供静态文件，再用本机 Chrome 逐页检查。
// 不引入浏览器测试依赖，直接走 DevTools 协议。需要 Chrome/Chromium（可用 CHROME_BIN 指定）和 Node 22+。
import { spawn, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { runInteractionChecks } from './interaction-checks.mjs';

const webui = fileURLToPath(new URL('../..', import.meta.url));
const repo = path.resolve(webui, '../../..');
const routes = ['run', 'providers', 'model-groups', 'routing', 'security', 'statistics', 'diagnostics', 'help', 'about'];
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const failures = [];
const check = (ok, message) => { if (!ok) failures.push(message); console.log(`${ok ? '  ok  ' : '  FAIL'} ${message}`); };

function findChrome() {
  const candidates = [
    process.env.CHROME_BIN,
    '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome',
    '/Applications/Chromium.app/Contents/MacOS/Chromium',
    '/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge',
    ...['google-chrome', 'google-chrome-stable', 'chromium', 'chromium-browser']
      .map((name) => spawnSync('which', [name], { encoding: 'utf8' }).stdout.trim()),
  ];
  return candidates.find((candidate) => candidate && fs.existsSync(candidate));
}

function adminCsp() {
  const source = fs.readFileSync(path.join(repo, 'adapters/linux/sumpter-linux-adapter/src/admin_auth_routes.rs'), 'utf8');
  const policy = source.match(/"(default-src [^"]+)"/)?.[1];
  if (!policy) throw new Error('未能从 admin_auth_routes.rs 读到 Admin CSP');
  return policy;
}

const chromePath = findChrome();
if (!chromePath) {
  console.error('未找到 Chrome/Chromium；请设置 CHROME_BIN 后重试。');
  process.exit(2);
}

const workDir = fs.mkdtempSync(path.join(os.tmpdir(), 'sumpter-webui-smoke-'));
const outDir = path.join(workDir, 'web');
const build = spawnSync(process.execPath, [path.join(webui, 'node_modules/vite/bin/vite.js'), 'build', '--outDir', outDir, '--emptyOutDir', '--logLevel', 'error'], {
  cwd: webui, env: { ...process.env, VITE_SUMPTER_MOCK: '1' }, encoding: 'utf8',
});
if (build.status !== 0) {
  console.error(build.stdout, build.stderr);
  process.exit(1);
}

const csp = adminCsp();
const contentTypes = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.css': 'text/css', '.png': 'image/png', '.woff2': 'font/woff2' };
let missingAsset = null; // 模拟升级后被替换掉的分包
const server = http.createServer((request, response) => {
  const relative = decodeURIComponent(new URL(request.url, 'http://localhost').pathname);
  const file = path.join(outDir, relative === '/' ? 'index.html' : relative);
  const exists = file.startsWith(outDir) && fs.existsSync(file) && fs.statSync(file).isFile();
  if (!exists || (missingAsset && missingAsset.test(relative))) { response.writeHead(404); response.end(); return; }
  response.writeHead(200, {
    'Content-Type': contentTypes[path.extname(file)] || 'application/octet-stream',
    'Content-Security-Policy': csp,
    'X-Content-Type-Options': 'nosniff',
  });
  fs.createReadStream(file).pipe(response);
});
await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

const profile = path.join(workDir, 'profile');
const chrome = spawn(chromePath, [
  '--headless=new', '--no-first-run', '--no-default-browser-check', '--enable-unsafe-swiftshader',
  `--user-data-dir=${profile}`, '--remote-debugging-port=0', '--window-size=1440,1000', 'about:blank',
  ...(process.env.CI ? ['--no-sandbox'] : []),
], { stdio: 'ignore' });

let exitCode = 1;
try {
  let debugPort = 0;
  for (let attempt = 0; attempt < 100 && !debugPort; attempt += 1) {
    await sleep(100);
    try { debugPort = Number(fs.readFileSync(path.join(profile, 'DevToolsActivePort'), 'utf8').split('\n')[0]); } catch { /* Chrome 尚未就绪 */ }
  }
  if (!debugPort) throw new Error('Chrome 未开放调试端口');
  const targets = await (await fetch(`http://127.0.0.1:${debugPort}/json`)).json();
  const socket = new WebSocket(targets.find((target) => target.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve); socket.addEventListener('error', reject); });

  let sequence = 0;
  const pending = new Map();
  let exceptions = [];
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data);
    if (message.id && pending.has(message.id)) { pending.get(message.id)(message.result ?? message.error); pending.delete(message.id); }
    if (message.method === 'Runtime.exceptionThrown') {
      const details = message.params.exceptionDetails;
      exceptions.push(String(details.exception?.description || details.text).split('\n')[0]);
    }
  });
  const send = (method, params = {}) => new Promise((resolve) => {
    sequence += 1;
    pending.set(sequence, resolve);
    socket.send(JSON.stringify({ id: sequence, method, params }));
  });
  const evaluate = async (expression) => (await send('Runtime.evaluate', { expression, returnByValue: true })).result?.value;
  const waitFor = async (expression, timeoutMs = 15_000) => {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (await evaluate(expression)) return true;
      await sleep(150);
    }
    return false;
  };

  await send('Page.enable');
  await send('Runtime.enable');
  await send('Page.addScriptToEvaluateOnNewDocument', {
    source: `window.__cspViolations = [];
      document.addEventListener('securitypolicyviolation', (event) => window.__cspViolations.push(event.effectiveDirective + ' ' + String(event.blockedURI).slice(0, 24)));`,
  });
  const open = async (route) => {
    exceptions = [];
    await send('Page.navigate', { url: 'about:blank' });
    await send('Page.navigate', { url: `${origin}/?mock=1#${route}` });
    return waitFor(`!!document.querySelector('main h1')`);
  };
  // Paper Shaders 会向 <head> 注入一个带 data-paper-shader 的 <style>，CSP 拦截它是预期行为（等价规则在 shadcn.css）。
  // 除它之外不允许出现任何运行时 <style> 或其他违规。
  const pageReport = () => evaluate(`(() => ({
    violations: window.__cspViolations,
    paperStyles: document.querySelectorAll('style[data-paper-shader]').length,
    otherStyles: document.querySelectorAll('style:not([data-paper-shader])').length,
    boundary: !!document.querySelector('[data-slot="error-boundary"]'),
  }))()`);

  console.log(`Chrome: ${chromePath}\nCSP: ${csp}\n`);
  for (const route of routes) {
    const rendered = await open(route);
    await sleep(600);
    const report = await pageReport();
    const unexpected = report.violations.filter((violation) => !(violation.startsWith('style-src-elem') && report.paperStyles > 0));
    const allowed = report.violations.length - unexpected.length;
    const errors = exceptions.filter((message) => !/WebGL is not supported/.test(message));
    check(rendered && !report.boundary, `#${route} 渲染完成`);
    check(unexpected.length === 0 && allowed <= report.paperStyles && report.otherStyles === 0,
      `#${route} 无 CSP 违规${unexpected.length ? `：${[...new Set(unexpected)].join(', ')}` : ''}${report.otherStyles ? `（存在 ${report.otherStyles} 个运行时 <style>）` : ''}`);
    check(errors.length === 0, `#${route} 无未捕获异常${errors.length ? `：${errors[0]}` : ''}`);

    if (route === 'statistics') {
      const chart = await evaluate(`(() => {
        const container = document.querySelector('[data-chart]');
        const curve = container?.querySelector('.recharts-area-curve, .recharts-line-curve');
        const sortButtons = [...document.querySelectorAll('.data-table-sort-button')].map((button) => button.getBoundingClientRect().height);
        return {
          variable: container ? getComputedStyle(container).getPropertyValue('--color-value').trim() : '',
          stroke: curve ? getComputedStyle(curve).stroke : '',
          sortButtons: sortButtons.length,
          minSortHeight: sortButtons.length ? Math.min(...sortButtons) : 0,
        };
      })()`);
      check(Boolean(chart.variable) && Boolean(chart.stroke) && chart.stroke !== 'none', `趋势图曲线有颜色（--color-value=${chart.variable || '未设置'}，stroke=${chart.stroke || '无曲线'}）`);
      check(chart.sortButtons > 0 && chart.minSortHeight >= 24, `排序按钮点击高度 ≥ 24px（${chart.sortButtons} 个，最小 ${chart.minSortHeight}px）`);
    }
  }

  console.log('\n弹窗、导航和命令面板交互回归');
  await runInteractionChecks({
    open,
    evaluate,
    waitFor,
    check,
    click: (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`),
    focus: (selector) => evaluate(`document.querySelector(${JSON.stringify(selector)}).focus()`),
    fill: (selector, value) => evaluate(`(() => {
      const input = document.querySelector(${JSON.stringify(selector)});
      Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(input, ${JSON.stringify(value)});
      input.dispatchEvent(new Event('input', { bubbles: true }));
    })()`),
    press: async (key, options = {}) => {
      const virtualKey = { ArrowDown: 40, ArrowUp: 38, Escape: 27, End: 35, PageDown: 34, k: 75 }[key];
      await send('Input.dispatchKeyEvent', { type: 'keyDown', key, code: key === 'k' ? 'KeyK' : key, windowsVirtualKeyCode: virtualKey, ...options });
      await send('Input.dispatchKeyEvent', { type: 'keyUp', key, code: key === 'k' ? 'KeyK' : key, windowsVirtualKeyCode: virtualKey, ...options });
    },
    setViewport: (width, height) => send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false }),
  });

  console.log('\n分包丢失（模拟 daemon 升级后仍开着的旧标签页）');
  await open('about');
  missingAsset = /\/assets\/StatsPage-/;
  await evaluate(`location.hash = '#statistics'`);
  const fallbackShown = await waitFor(`!!document.querySelector('[data-slot="error-boundary"][data-error-kind="chunk"]')`, 8_000);
  const shell = await evaluate(`({ nav: !!document.querySelector('[data-slot="sidebar"], nav'), reload: [...document.querySelectorAll('[data-slot="error-boundary"] button')].some((button) => button.textContent.includes('刷新页面')) })`);
  check(fallbackShown && shell.reload, '页面区域显示“需要刷新”提示');
  check(Boolean(shell.nav), '侧栏与顶栏仍然可用');
  await evaluate(`location.hash = '#about'`);
  check(await waitFor(`!!document.querySelector('main h1') && !document.querySelector('[data-slot="error-boundary"]')`, 8_000), '切到其他页面后恢复正常');
  missingAsset = null;

  socket.close();
  exitCode = failures.length ? 1 : 0;
  console.log(failures.length ? `\n${failures.length} 项失败` : '\n全部通过');
} catch (error) {
  console.error(error);
} finally {
  chrome.kill();
  server.close();
  await sleep(300);
  fs.rmSync(workDir, { recursive: true, force: true, maxRetries: 5, retryDelay: 200 });
}
process.exit(exitCode);
