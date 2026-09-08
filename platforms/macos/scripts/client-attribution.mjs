#!/usr/bin/env node
// Canonical standalone launcher/installer. Ship copies via sync-client-attribution.mjs.
import { spawn, execFileSync } from 'node:child_process';
import { basename, dirname, join, resolve } from 'node:path';
import { homedir, userInfo } from 'node:os';
import { randomUUID, createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, existsSync, renameSync, realpathSync, statSync, lstatSync, openSync, closeSync, unlinkSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

const clients = ['claude', 'grok', 'gemini'];
const ownedHeaders = new Set(['x-sumpter-client', 'x-sumpter-project', 'x-sumpter-workspace',
  'x-sumpter-git-remote', 'x-sumpter-user', 'x-sumpter-session-id', 'x-sumpter-attribution-encoding']);
const safe = (value, max = 4096) => typeof value === 'string' && value.trim()
  && !/[\u0000-\u001f\u007f]/u.test(value) && Buffer.byteLength(value, 'utf8') <= max ? value.trim() : undefined;
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const shellQuote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
const read = (path) => existsSync(path) ? readFileSync(path, 'utf8') : '';
function parseConfig(text, label) {
  try { return JSON.parse(text); }
  catch { throw new Error(`${label} 不是有效 JSON，未改写原配置`); }
}
function git(cwd, args) {
  try { return execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8', timeout: 1000, maxBuffer: 16384, stdio: ['ignore', 'pipe', 'ignore'] }).trim(); }
  catch { return ''; }
}
export function sanitizeRemote(value) {
  if (!safe(value)) return undefined;
  try {
    const url = new URL(value);
    if (!['http:', 'https:', 'ssh:', 'git:'].includes(url.protocol)) return undefined;
    url.username = ''; url.password = ''; url.search = ''; url.hash = '';
    return url.toString();
  } catch {
    const match = /^(?:[^@\s]+@)?([a-z\d.-]+):([^?#\s]+)(?:[?#].*)?$/iu.exec(value);
    return match ? `ssh://${match[1]}/${match[2]}` : undefined;
  }
}
export function collectHeaders(client, cwd, env) {
  const workspace = git(cwd, ['rev-parse', '--show-toplevel']) || resolve(cwd);
  let remote = git(workspace, ['remote', 'get-url', 'origin']);
  if (!remote) {
    const first = git(workspace, ['remote']).split('\n')[0];
    if (first) remote = git(workspace, ['remote', 'get-url', first]);
  }
  let user = env.USER || env.LOGNAME;
  if (!user) { try { user = userInfo().username; } catch { /* optional */ } }
  const values = {
    'X-Sumpter-Project': (client === 'gemini' && env.SUMPTER_GEMINI_PROJECT) || basename(workspace),
    'X-Sumpter-Workspace': workspace,
    'X-Sumpter-Git-Remote': sanitizeRemote(remote),
    'X-Sumpter-User': user,
  };
  const headers = { 'X-Sumpter-Attribution-Encoding': 'uri-v1' };
  for (const [key, value] of Object.entries(values)) {
    const cleaned = safe(value);
    if (cleaned) headers[key] = encodeURIComponent(cleaned);
  }
  // Do not invent Claude/Grok session IDs: their resume behavior belongs to the CLI.
  return headers;
}
function keepHeaders(headers) {
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !ownedHeaders.has(name.trim().toLowerCase())));
}
function mergeText(raw, delimiter, headers) {
  const lines = (raw || '').split(delimiter).filter((line) => {
    const key = line.slice(0, line.indexOf(':')).trim().toLowerCase();
    return line.trim() && !ownedHeaders.has(key);
  });
  return [...lines, ...Object.entries(headers).map(([key, value]) => `${key}: ${value}`)].join(delimiter === '\n' ? '\n' : ', ');
}
function claudeConflicts(cwd, env) {
  // Claude settings env overrides process env. Refuse silently ineffective setup.
  for (const file of [join(env.CLAUDE_CONFIG_DIR || join(env.HOME || homedir(), '.claude'), 'settings.json'),
    join(cwd, '.claude/settings.json'), join(cwd, '.claude/settings.local.json')]) {
    if (!existsSync(file)) continue;
    const settings = parseConfig(read(file), file);
    if (settings.env?.ANTHROPIC_CUSTOM_HEADERS) throw new Error(`${file} 设置了 ANTHROPIC_CUSTOM_HEADERS，会覆盖动态归因；请先移除该设置。`);
  }
}
export function prepareLaunch(client, args, env = process.env, cwd = process.cwd()) {
  if (!clients.includes(client)) throw new Error('客户端必须是 claude、grok 或 gemini');
  const headers = collectHeaders(client, cwd, env);
  const next = { ...env };
  const forwarded = [...args];
  if (client === 'claude') {
    claudeConflicts(cwd, env);
    next.ANTHROPIC_CUSTOM_HEADERS = mergeText(env.ANTHROPIC_CUSTOM_HEADERS, '\n', headers);
  } else if (client === 'grok') {
    if (env.GROK_CONFIG_PATH) throw new Error('GROK_CONFIG_PATH 会覆盖动态归因；请先解除该覆盖。');
    const config = parseConfig(env.GROK_CONFIG || '{}', 'GROK_CONFIG');
    if (!object(config) || (config.models !== undefined && !object(config.models))) throw new Error('GROK_CONFIG 必须是包含 models 对象的 JSON 配置');
    config.models ??= {};
    if (config.models.extra_headers !== undefined && !object(config.models.extra_headers)) throw new Error('GROK_CONFIG models.extra_headers 必须是对象');
    config.models.extra_headers = { ...keepHeaders(config.models.extra_headers || {}), ...headers };
    next.GROK_CONFIG = JSON.stringify(config);
  } else {
    const base = safe(env.SUMPTER_GEMINI_BASE_URL);
    const token = safe(env.SUMPTER_AUTH_TOKEN);
    if (!base || !token) throw new Error('Gemini 需要 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN');
    const url = new URL(base);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Gemini Base URL 必须是无凭据、query、fragment 的 HTTP(S) 地址');
    const sessionArg = args.some((arg) => /^--(session-id|session-file|resume|list-sessions)(=|$)/u.test(arg) || /^-r(?:$|=)/u.test(arg));
    // Explicit/resumed sessions must keep the CLI's identity. Optional override only for a fresh session.
    if (!sessionArg) {
      const id = env.SUMPTER_GEMINI_SESSION_ID || randomUUID();
      if (!safe(id, 256) || !/^[\x21-\x7e]+$/u.test(id)) throw new Error('SUMPTER_GEMINI_SESSION_ID 必须是有界 ASCII 标识');
      headers['X-Sumpter-Session-Id'] = id;
      forwarded.push('--session-id', id);
    }
    next.GOOGLE_GEMINI_BASE_URL = base;
    next.GEMINI_API_KEY = token;
    next.GEMINI_CLI_CUSTOM_HEADERS = mergeText(env.GEMINI_CLI_CUSTOM_HEADERS, ',', headers);
    next.GEMINI_API_KEY_AUTH_MECHANISM = 'x-goog-api-key';
    for (const key of ['GOOGLE_GENAI_USE_VERTEXAI', 'GOOGLE_GENAI_USE_GCA', 'GOOGLE_VERTEX_BASE_URL', 'GEMINI_CLI_USE_COMPUTE_ADC']) delete next[key];
  }
  return { command: env[`SUMPTER_${client.toUpperCase()}_BIN`] || (client === 'gemini' && env.GEMINI_CLI_BIN) || client, args: forwarded, env: next };
}
function marker(client, legacy = false) {
  const name = legacy ? `${client === 'claude' ? 'cc' : client}-project-attribution` : `client-attribution ${client}`;
  return [`# >>> sumpter ${name} >>>`, `# <<< sumpter ${name} <<<`];
}
function removeBlock(text, [begin, end]) {
  const lines = text.split(/(?<=\n)/u);
  let active = false; let captured = ''; let remaining = ''; let count = 0;
  for (const line of lines) {
    if (line.trimEnd() === begin) {
      if (active || count) throw new Error('发现重复或嵌套归因标记，未修改 rc');
      active = true; count++;
    }
    if (active) captured += line; else remaining += line;
    if (line.trimEnd() === end) {
      if (!active) throw new Error('发现不完整归因标记，未修改 rc');
      active = false;
    }
  }
  if (active) throw new Error('发现不完整归因标记，未修改 rc');
  return { remaining, captured };
}
function atomic(path, contents, mode = 0o600) {
  mkdirSync(dirname(path), { recursive: true });
  const temp = `${path}.${randomUUID()}.tmp`;
  try { writeFileSync(temp, contents, { mode, flag: 'wx' }); renameSync(temp, path); }
  finally { if (existsSync(temp)) unlinkSync(temp); }
}
function managePi(action, options, env, source) {
  if (!['install', 'status', 'uninstall', 'restore'].includes(action)) throw new Error('pi 支持 install、status、uninstall、restore；临时加载请使用 pi -e');
  const home = env.HOME || homedir();
  const target = resolve(home, '.pi/agent/extensions/pi-project-attribution.ts');
  const dir = join(env.XDG_DATA_HOME || join(home, '.local/share'), 'sumpter', 'attribution');
  const key = createHash('sha256').update(target).digest('hex');
  const statePath = join(dir, `pi-${key}.json`);
  const snapshot = () => {
    let info;
    try { info = lstatSync(target); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
    if (!info.isFile()) throw new Error('pi 扩展路径不是普通文件，请先检查该路径');
    return { bytes: readFileSync(target).toString('base64'), mode: info.mode & 0o777 };
  };
  const original = snapshot();
  const stateText = read(statePath);
  const previous = stateText ? parseConfig(stateText, 'pi 还原记录') : null;
  if (previous && (previous.version !== 1 || previous.target !== target
    || !(previous.original === null || (typeof previous.original?.bytes === 'string' && Number.isInteger(previous.original.mode))))) {
    throw new Error('pi 还原记录不完整，未修改扩展');
  }
  const resource = join(dirname(source), 'pi-project-attribution.ts');
  const expected = existsSync(resource) ? readFileSync(resource) : null;
  if (['install', 'status'].includes(action) && !expected?.length) throw new Error('缺少配套 pi-project-attribution.ts，请使用完整安装包或重新下载');
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const status = !original ? 'absent' : expected && original.bytes === expected.toString('base64') ? 'installed' : 'outdated';
  const item = { client: 'pi', status, rc: target, shell: 'extension', canRestore: Boolean(previous && !same(original, previous.original)) };
  if (action === 'status') return [item];
  if (action === 'restore' && !previous) return [{ ...item, status: 'unchanged' }];
  if (options.dryRun) return [{ ...item, status: action === 'install' ? 'installed' : action === 'restore' ? 'restored' : 'uninstalled' }];
  mkdirSync(dir, { recursive: true });
  const lock = join(dir, 'install.lock');
  const descriptor = openSync(lock, 'wx', 0o600);
  try {
    if (!same(snapshot(), original) || read(statePath) !== stateText) throw new Error('pi 配置已被其他进程修改，请重试');
    if (action === 'install') {
      // Keep the first snapshot through updates and repeated installs, including an absent original.
      if (!previous) atomic(statePath, JSON.stringify({ version: 1, target, original }));
      if (status !== 'installed') atomic(target, expected, original?.mode ?? 0o600);
    } else {
      if (original && (action === 'uninstall' || !same(original, previous.original))) {
        atomic(`${target}.sumpter-attribution-bak-${Date.now()}-${randomUUID()}`, Buffer.from(original.bytes, 'base64'), original.mode);
      }
      const restored = action === 'restore' ? previous.original : null;
      if (restored) atomic(target, Buffer.from(restored.bytes, 'base64'), restored.mode);
      else if (original) unlinkSync(target);
      if (action === 'restore') unlinkSync(statePath);
    }
  } finally { closeSync(descriptor); unlinkSync(lock); }
  return [{ ...item, status: action === 'install' ? 'installed' : action === 'restore' ? 'restored' : 'uninstalled' }];
}

export function manage(action, client, options = {}, env = process.env, source = fileURLToPath(import.meta.url)) {
  if (client === 'pi') return managePi(action, options, env, source);
  if (client !== 'all') return manageShell(action, client, options, env, source);
  // Validate both destinations and required resources before either installer writes.
  const shellPreview = manageShell(action, client, { ...options, dryRun: true }, env, source);
  const piPreview = managePi(action, { ...options, dryRun: true }, env, source);
  if (action === 'status' || options.dryRun) return [...shellPreview, ...piPreview];
  return [...manageShell(action, client, options, env, source), ...managePi(action, options, env, source)];
}

function manageShell(action, client, options, env, source) {
  if (!['install', 'status', 'uninstall', 'restore', 'snippet'].includes(action) || ![...clients, 'all'].includes(client)) throw new Error('用法：client-attribution.mjs install|status|uninstall|restore|snippet claude|grok|gemini|all [--shell bash|zsh] [--rc 文件] [--dry-run]');
  const shell = options.shell || basename(env.SHELL || '');
  if (!['bash', 'zsh'].includes(shell)) throw new Error('自动安装支持 bash/zsh；其他 shell 请使用 run 子命令');
  const home = env.HOME || homedir();
  let rc = resolve(options.rc || (shell === 'zsh' ? join(env.ZDOTDIR || home, '.zshrc')
    : join(home, process.platform === 'darwin' && existsSync(join(home, '.bash_profile')) ? '.bash_profile' : '.bashrc')));
  // Resolve existing ancestors too: macOS /var and /tmp aliases must use the same
  // restore record before and after a previously absent rc file is created.
  let ancestor = rc;
  const suffix = [];
  while (!existsSync(ancestor) && dirname(ancestor) !== ancestor) {
    suffix.unshift(basename(ancestor));
    ancestor = dirname(ancestor);
  }
  rc = join(realpathSync(ancestor), ...suffix); // Preserve rc symlinks.
  const dir = join(env.XDG_DATA_HOME || join(home, '.local/share'), 'sumpter', 'attribution');
  const installed = join(dir, 'client-attribution.mjs');
  const selected = client === 'all' ? clients : [client];
  const original = read(rc);
  let text = original;
  const changes = [];
  for (const name of selected) {
    const current = removeBlock(text, marker(name));
    const legacy = removeBlock(current.remaining, marker(name, true));
    const statePath = join(dir, `${name}-${Buffer.from(rc).toString('base64url')}.json`);
    const previous = existsSync(statePath) ? JSON.parse(read(statePath)) : null;
    const block = `${marker(name)[0]}\n${name}() { command node ${shellQuote(installed)} run ${name} -- "$@"; }\n${marker(name)[1]}\n`;
    const status = current.captured && existsSync(installed)
      ? (current.captured === block && read(installed) === read(source) ? 'installed' : 'outdated')
      : current.captured ? 'broken' : legacy.captured ? 'legacy' : 'absent';
    const canRestore = Boolean(previous && (current.captured || legacy.captured !== previous.legacy));
    if (action === 'status') { changes.push({ client: name, status, rc, shell, canRestore }); continue; }
    if (action === 'snippet') { changes.push({ client: name, snippet: `${name}() { command node ${shellQuote(source)} run ${name} -- "$@"; }` }); continue; }
    if (action === 'install') {
      if (name === 'claude') claudeConflicts(process.cwd(), env);
      if (name === 'grok' && env.GROK_CONFIG_PATH) throw new Error('请先解除 GROK_CONFIG_PATH 覆盖');
      text = legacy.remaining;
      if (text && !text.endsWith('\n')) text += '\n';
      text += block;
      // Reinstall does not overwrite the pre-migration block retained for restore.
      changes.push({ client: name, statePath, state: previous || { rc, legacy: legacy.captured }, status: 'installed' });
    } else {
      if (action === 'restore' && !previous) {
        if (client !== 'all') throw new Error(`${name} 没有统一安装器的还原记录`);
        changes.push({ client: name, status: 'unchanged', rc });
        continue;
      }
      text = current.remaining;
      if (action === 'restore') {
        text = removeBlock(text, marker(name, true)).remaining;
        if (text && !text.endsWith('\n')) text += '\n';
        text += previous.legacy;
      }
      changes.push({ client: name, status: action === 'restore' ? 'restored' : 'uninstalled', rc });
    }
  }
  if (['status', 'snippet'].includes(action) || options.dryRun) return changes.map(({ state, statePath, ...item }) => item);
  mkdirSync(dir, { recursive: true });
  const lock = join(dir, 'install.lock');
  const descriptor = openSync(lock, 'wx', 0o600);
  try {
    if (read(rc) !== original) throw new Error('rc 已被其他进程修改，请重试');
    if (action === 'install') {
      atomic(installed, readFileSync(source), 0o600);
      for (const change of changes) atomic(change.statePath, JSON.stringify(change.state));
    }
    if (text !== original) {
      const backup = `${rc}.sumpter-attribution-bak-${Date.now()}-${randomUUID()}`;
      atomic(backup, original);
      atomic(rc, text, existsSync(rc) ? statSync(rc).mode & 0o777 : 0o600);
      for (const change of changes) change.backup = backup;
    }
  } finally { closeSync(descriptor); unlinkSync(lock); }
  return changes.map(({ state, statePath, ...item }) => item);
}
async function main(argv) {
  if (['--help', '-h'].includes(argv[0]) || argv.length === 0) {
    console.log('用法：node client-attribution.mjs install|status|uninstall|restore claude|grok|gemini|pi|all [--shell bash|zsh] [--rc 文件] [--dry-run]\n临时运行：node client-attribution.mjs run claude|grok|gemini -- [原始参数]\n需要 Node.js 18+。pi 安装后执行 /reload，其他客户端新开终端；只在连接 Sumpter 的客户端上启用。');
    return;
  }
  let [action, client, ...rest] = argv;
  if (action === 'run') {
    if (rest[0] === '--') rest.shift();
    const launch = prepareLaunch(client, rest);
    const child = spawn(launch.command, launch.args, { env: launch.env, stdio: 'inherit' });
    for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) process.on(signal, () => child.kill(signal));
    child.on('error', () => { console.error(`无法启动 ${client}，请检查客户端安装与 PATH`); process.exitCode = 1; });
    child.on('exit', (code, signal) => { process.exitCode = code ?? ({ SIGINT: 130, SIGTERM: 143, SIGHUP: 129 }[signal] || 1); });
    return;
  }
  const options = {};
  while (rest.length) {
    const flag = rest.shift();
    if (flag === '--dry-run') options.dryRun = true;
    else if (['--shell', '--rc'].includes(flag) && rest.length) options[flag.slice(2)] = rest.shift();
    else throw new Error(`未知或缺值的安装参数：${flag}`);
  }
  console.log(JSON.stringify(manage(action, client, options), null, 2));
}
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const args = basename(process.argv[1]) === 'gemini-sumpter-wrapper.mjs'
    ? ['run', 'gemini', '--', ...process.argv.slice(2)] : process.argv.slice(2);
  main(args).catch((error) => { console.error(error.message); process.exitCode = 2; });
}
