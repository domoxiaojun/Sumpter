#!/usr/bin/env node
// SUMPTER_ATTRIBUTION_BUNDLE_VERSION: 0.4.57
// Canonical standalone launcher/installer. Ship copies via sync-client-attribution.mjs.
import { spawn, execFileSync } from 'node:child_process';
import { basename, dirname, join, resolve } from 'node:path';
import { homedir, userInfo } from 'node:os';
import { randomUUID, createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, existsSync, renameSync, realpathSync, statSync, lstatSync, openSync, closeSync, unlinkSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

const clients = ['claude', 'grok', 'gemini', 'codex', 'pi'];
// Keep the launcher and its companion Pi extension from being mixed across releases.
export const ATTRIBUTION_BUNDLE_VERSION = '0.4.57';
const PI_PASSTHROUGH_COMMANDS = new Set(['install', 'remove', 'uninstall', 'update', 'list', 'config', 'auth']);
const PI_PASSTHROUGH_FLAGS = new Set(['--help', '-h', '--version', '-v']);
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
// Read only the selectors used to attach headers; Codex still loads the full
// configuration and owns provider/auth/model resolution. Never print config text.
function codexLaunchContext(args, env, cwd) {
  const selectors = new Map();
  let section = '';
  const text = read(join(env.CODEX_HOME || join(env.HOME || homedir(), '.codex'), 'config.toml'));
  for (const line of text.split('\n')) {
    if (/^\s*\[/u.test(line)) {
      section = /^\s*\[profiles\.(?:([\w-]+)|"([\w-]+)"|'([\w-]+)')\]\s*(?:#.*)?$/u.exec(line)?.slice(1).find(Boolean) ?? '#other';
      continue;
    }
    const field = /^\s*(model_provider|profile)\s*=\s*(.*)$/u.exec(line);
    if (field && section !== '#other') {
      const value = /^(?:"([\w-]+)"|'([\w-]+)')\s*(?:#.*)?$/u.exec(field[2])?.slice(1).find(Boolean);
      if (!value) throw new Error('Codex 归因暂不支持该 profile/model_provider 写法；未修改配置。');
      selectors.set(`${section}:${field[1]}`, value);
    }
  }
  let profile = selectors.get(':profile');
  let providerOverride;
  let directory = cwd;
  for (let i = 0; i < args.length && args[i] !== '--'; i++) {
    const arg = args[i];
    if (['-C', '--cd'].includes(arg)) directory = resolve(cwd, args[++i] || '.');
    else if (arg.startsWith('--cd=')) directory = resolve(cwd, arg.slice(5));
    else if (arg.startsWith('-C') && arg.length > 2) directory = resolve(cwd, arg.slice(2));
    else if (['-p', '--profile'].includes(arg)) profile = args[++i];
    else if (arg.startsWith('--profile=')) profile = arg.slice(10);
    else if (arg.startsWith('-p') && arg.length > 2) profile = arg.slice(2);
    else {
      const raw = ['-c', '--config'].includes(arg) ? args[++i]
        : arg.startsWith('--config=') ? arg.slice(9)
          : arg.startsWith('-c') && arg.length > 2 ? arg.slice(2) : '';
      const pair = /^(model_provider|profile)\s*=\s*(?:"([\w-]+)"|'([\w-]+)'|([\w-]+))\s*$/u.exec(raw || '');
      if (pair?.[1] === 'model_provider') providerOverride = pair.slice(2).find(Boolean);
      if (pair?.[1] === 'profile') profile = pair.slice(2).find(Boolean);
    }
  }
  const provider = providerOverride || (profile && selectors.get(`${profile}:model_provider`)) || selectors.get(':model_provider');
  if (!provider || ['openai', 'ollama', 'lmstudio', 'amazon-bedrock', 'amazon-bedrock-runtime'].includes(provider)) {
    throw new Error('Codex 归因需要已有的 Sumpter 自定义连接配置；内置连接不支持此 header 注入方式。');
  }
  return { provider, directory };
}
/** Inspect visible configuration only. Never read auth stores, invoke a client, or write files. */
export function diagnoseClients(client = 'all', options = {}, env = process.env) {
  if (![...clients, 'all'].includes(client)) throw new Error('未知客户端');
  const home = env.HOME || homedir();
  let installed = [];
  try { installed = manage('status', client, options, env); } catch { /* Unsupported shell remains unknown. */ }
  let wrapperVersion = 'unknown';
  try {
    const path = join(env.XDG_DATA_HOME || join(home, '.local/share'), 'sumpter', 'attribution', 'client-attribution.mjs');
    if (existsSync(path) && statSync(path).size <= 256 * 1024) {
      wrapperVersion = /SUMPTER_ATTRIBUTION_BUNDLE_VERSION:\s*([0-9]+\.[0-9]+\.[0-9]+)/.exec(readFileSync(path, 'utf8'))?.[1] || 'unknown';
    }
  } catch { /* Missing or unreadable installed wrapper remains unknown. */ }
  const expected = new URL(options.proxyURL || 'http://127.0.0.1:57878').origin;
  const selected = client === 'all' ? clients : [client];
  return selected.map(name => {
    const sources = [], issues = [];
    const add = (source, value, marker = null) => {
      if (typeof value !== 'string' || !value.trim()) return;
      try {
        const url = new URL(value);
        if (!['http:', 'https:'].includes(url.protocol)) throw new Error('scheme');
        const path = /^\/(?:v1|v1beta|api|openai|backend-api|codex|responses)(?:\/(?:v1|v1beta|api|openai|backend-api|codex|responses))*\/?$/.test(url.pathname) ? url.pathname : url.pathname === '/' ? '/' : '/[custom-path]';
        sources.push({ source, baseURL: url.origin + path, matchesProxyOrigin: url.origin === expected, explicitOptIn: marker });
      } catch { issues.push(`${source} 的地址无法识别`); }
    };
    const json = path => {
      try {
        if (!existsSync(path)) return {};
        if (statSync(path).size > 256 * 1024) { issues.push('配置超过诊断读取上限'); return {}; }
        return JSON.parse(readFileSync(path, 'utf8'));
      } catch { issues.push('配置无法读取或不是有效 JSON'); return {}; }
    };
    if (name === 'claude') {
      add('进程环境 ANTHROPIC_BASE_URL', env.ANTHROPIC_BASE_URL);
      add('用户 settings.json env', json(join(env.CLAUDE_CONFIG_DIR || join(home, '.claude'), 'settings.json'))?.env?.ANTHROPIC_BASE_URL);
    } else if (name === 'gemini') {
      add('进程环境 GOOGLE_GEMINI_BASE_URL', env.GOOGLE_GEMINI_BASE_URL);
      add('进程环境 GOOGLE_VERTEX_BASE_URL', env.GOOGLE_VERTEX_BASE_URL);
      issues.push('实际地址还取决于所选认证方式；此处不读取凭据或执行登录');
    } else if (name === 'pi') {
      const config = json(join(env.PI_CODING_AGENT_DIR || join(home, '.pi', 'agent'), 'models.json'));
      for (const [provider, value] of Object.entries(config.providers || {}).slice(0, 128)) {
        const marker = Object.entries(value?.headers || {}).some(([key, value]) => key.toLowerCase() === 'x-sumpter-client' && value === 'pi');
        add(`models.json provider:${provider}`, value?.baseUrl, marker);
      }
      issues.push('只有显式 x-sumpter-client: pi 的 provider 启用项目归因；localhost 本身不是标记');
    } else if (name === 'codex') {
      const path = join(env.CODEX_HOME || join(home, '.codex'), 'config.toml');
      try {
        if (existsSync(path) && statSync(path).size <= 256 * 1024) {
          let section = '', provider = 'openai'; const urls = new Map();
          const literal = value => { try { return value.startsWith('"') ? JSON.parse(value) : value.slice(1, -1); } catch { return undefined; } };
          for (const line of readFileSync(path, 'utf8').split('\n')) {
            const table = line.match(/^\s*\[([^\]]+)\]\s*(?:#.*)?$/);
            if (table) { section = table[1]; continue; }
            const field = line.match(/^\s*(model_provider|base_url)\s*=\s*("(?:[^"\\]|\\.)*"|'[^']*')\s*(?:#.*)?$/);
            if (!field) continue;
            if (!section && field[1] === 'model_provider') provider = literal(field[2]);
            if (section.startsWith('model_providers.') && field[1] === 'base_url') urls.set(section.slice(16).replace(/^['"]|['"]$/g, ''), literal(field[2]));
          }
          add(`用户 config.toml provider:${provider}`, urls.get(provider));
        }
      } catch { issues.push('用户 config.toml 无法读取'); }
      add('进程环境 OPENAI_BASE_URL（是否生效由 provider 决定）', env.OPENAI_BASE_URL);
    } else {
      add('进程环境 GROK_BASE_URL', env.GROK_BASE_URL);
      add('进程环境 XAI_BASE_URL', env.XAI_BASE_URL);
      issues.push('Grok 的其它配置来源未推断，需结合客户端当前设置核对');
    }
    return { client: name, status: installed.find(row => row.client === name)?.status || 'unknown',
      wrapperVersion, diagnosisVersion: ATTRIBUTION_BUNDLE_VERSION, configuration: sources.length ? 'observed' : 'unknown', sources, issues,
      traffic: 'unknown', note: '只展示可见配置来源；项目配置、CLI 参数和运行进程环境可能覆盖它们。未观察到流量不代表绕过代理。' };
  });
}

export function prepareLaunch(client, args, env = process.env, cwd = process.cwd()) {
  if (client === 'pi') {
    // Pi package/config/auth commands must remain at argv[0]. Loading an extension
    // before them makes Pi treat the command words as initial prompt messages.
    const passthrough = PI_PASSTHROUGH_COMMANDS.has(args[0]) || PI_PASSTHROUGH_FLAGS.has(args[0]);
    if (passthrough) {
      const next = { ...env };
      delete next.SUMPTER_PI_ATTRIBUTION;
      return { command: env.SUMPTER_PI_BIN || 'pi', args: [...args], env: next };
    }
    const extension = fileURLToPath(new URL('./pi-project-attribution.ts', import.meta.url));
    if (!existsSync(extension)) throw new Error('缺少配套 pi-project-attribution.ts，请使用完整安装包');
    // Use Pi's request hook so resumed/forked sessions and workspace changes
    // are observed at dispatch time rather than frozen in launcher environment.
    return {
      command: env.SUMPTER_PI_BIN || 'pi',
      args: ['-e', extension, ...args],
      env: { ...env, SUMPTER_PI_ATTRIBUTION: '1' },
    };
  }
  if (!clients.includes(client)) throw new Error('客户端必须是 claude、grok、gemini 或 codex');
  // Informational invocations should work even before the client is connected.
  if (client === 'codex' && args.some((arg) => ['--version', '-V', '--help', '-h'].includes(arg))) {
    return { command: env.SUMPTER_CODEX_BIN || 'codex', args: [...args], env: { ...env } };
  }
  const codex = client === 'codex' ? codexLaunchContext(args, env, cwd) : undefined;
  const headers = collectHeaders(client, codex?.directory || cwd, env);
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
  } else if (client === 'codex') {
    // Same local collection as Claude; only the client's header transport differs.
    // -c splits keys on dots literally: quoting a header name would send no header.
    headers['X-Sumpter-Client'] = 'codex';
    for (const key of Object.keys(next)) if (key.startsWith('SUMPTER_CODEX_X_SUMPTER_')) delete next[key];
    for (const [name, value] of Object.entries(headers)) {
      const variable = `SUMPTER_CODEX_${name.replaceAll(/[^A-Za-z0-9]/gu, '_').toUpperCase()}`;
      next[variable] = value;
      forwarded.unshift(`model_providers.${codex.provider}.env_http_headers.${name}=${JSON.stringify(variable)}`);
      forwarded.unshift('-c');
    }
  } else {
    const base = safe(env.SUMPTER_GEMINI_BASE_URL);
    const token = safe(env.SUMPTER_AUTH_TOKEN);
    if (!base || !token) throw new Error('Gemini 需要 SUMPTER_GEMINI_BASE_URL 和 SUMPTER_AUTH_TOKEN');
    const url = new URL(base);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Gemini Base URL 必须是无凭据、query、fragment 的 HTTP(S) 地址');
    const sessionArg = args.some((arg) => /^--(session-id|session-file|resume|list-sessions)(=|$)/u.test(arg) || /^-r(?:$|=)/u.test(arg));
    const optionValue = (name) => {
      const inline = args.find((arg) => arg.startsWith(`${name}=`));
      if (inline) return inline.slice(name.length + 1);
      const index = args.indexOf(name);
      return index >= 0 && args[index + 1] && !args[index + 1].startsWith('-') ? args[index + 1] : undefined;
    };
    const explicit = optionValue('--session-id');
    const resume = optionValue('--resume') ?? optionValue('-r');
    // Only a complete UUID identifies a resumed session; ordinals/latest/prefixes
    // are selectors resolved by Gemini and cannot become the analytics identity.
    const resolvedResume = typeof resume === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(resume) ? resume : undefined;
    const provided = explicit ?? resolvedResume;
    if (provided && safe(provided, 256) && /^[\x21-\x7e]+$/u.test(provided)) {
      headers['X-Sumpter-Session-Id'] = provided;
    }
    // Fresh sessions receive the same ID in the native CLI and the header.
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
// Only retire a global extension when the old installer recorded ownership and
// its bytes still match a shipped copy. Never overwrite an untracked/user edit.
function legacyPiMigration(env, source) {
  const home = env.HOME || homedir();
  const target = resolve(home, '.pi/agent/extensions/pi-project-attribution.ts');
  const dir = join(env.XDG_DATA_HOME || join(home, '.local/share'), 'sumpter', 'attribution');
  const key = createHash('sha256').update(target).digest('hex');
  const statePath = join(dir, `pi-${key}.json`);
  const stateText = read(statePath);
  if (!stateText) return { pending: false };
  const previous = parseConfig(stateText, 'pi 还原记录');
  if (previous.version !== 1 || previous.target !== target
    || !(previous.original === null || (typeof previous.original?.bytes === 'string'
      && Number.isInteger(previous.original.mode)))) {
    throw new Error('pi 还原记录不完整，未修改扩展');
  }
  const snapshot = () => {
    let info;
    try { info = lstatSync(target); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
    if (!info.isFile()) return { nonRegular: true };
    return { bytes: readFileSync(target).toString('base64'), mode: info.mode & 0o777 };
  };
  const current = snapshot();
  const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
  const known = [join(dirname(source), 'pi-project-attribution.ts'), join(dir, 'pi-project-attribution.ts')]
    .filter(existsSync).map(path => readFileSync(path).toString('base64'));
  const alreadyRestored = same(current, previous.original);
  if (!alreadyRestored && (current === null || current.nonRegular || !known.includes(current.bytes))) {
    return { pending: false, note: '旧版全局扩展已被修改或移除，已保留现状及原始备份；私有扩展由 wrapper 自动加载。' };
  }
  return {
    pending: true,
    commit() {
      if (read(statePath) !== stateText || !same(snapshot(), current)) throw new Error('pi 配置已被其他进程修改，请重试');
      if (!alreadyRestored) {
        atomic(join(dir, `pi-global-backup-${Date.now()}-${randomUUID()}.bin`), Buffer.from(current.bytes, 'base64'), current.mode);
        if (previous.original) atomic(target, Buffer.from(previous.original.bytes, 'base64'), previous.original.mode);
        else unlinkSync(target);
      }
      unlinkSync(statePath);
    },
  };
}

export function manage(action, client, options = {}, env = process.env, source = fileURLToPath(import.meta.url)) {
  if (!['pi', 'all'].includes(client) || action === 'snippet') return manageShell(action, client, options, env, source);
  const shells = manageShell(action, client, { ...options, dryRun: true }, env, source);
  // Capture old private resource bytes before install updates them.
  const migration = legacyPiMigration(env, source);
  const results = action === 'status' || options.dryRun ? shells
    : manageShell(action, client, options, env, source, migration.commit);
  return results.map(item => {
    if (item.client !== 'pi') return item;
    return {
      ...item,
      ...(action === 'status' && migration.pending ? { status: 'legacy', canRestore: true } : {}),
      ...(migration.note ? { note: migration.note } : {}),
    };
  });
}

function manageShell(action, client, options, env, source, migrateLegacy) {
  if (!['install', 'status', 'uninstall', 'restore', 'snippet'].includes(action) || ![...clients, 'all'].includes(client)) throw new Error('用法：client-attribution.mjs install|status|uninstall|restore|snippet claude|grok|gemini|codex|pi|all [--shell bash|zsh] [--rc 文件] [--dry-run]');
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
  const piExtensionSource = join(dirname(source), 'pi-project-attribution.ts');
  const piExtensionInstalled = join(dir, 'pi-project-attribution.ts');
  const selected = client === 'all' ? clients : [client];
  if (action === 'install' && selected.includes('pi') && !read(piExtensionSource)) {
    throw new Error('缺少配套 pi-project-attribution.ts，请使用完整安装包或重新下载');
  }
  const original = read(rc);
  let text = original;
  const changes = [];
  for (const name of selected) {
    const current = removeBlock(text, marker(name));
    const legacy = removeBlock(current.remaining, marker(name, true));
    const statePath = join(dir, `${name}-${Buffer.from(rc).toString('base64url')}.json`);
    const previous = existsSync(statePath) ? JSON.parse(read(statePath)) : null;
    const block = `${marker(name)[0]}\n${name}() { command node ${shellQuote(installed)} run ${name} -- "$@"; }\n${marker(name)[1]}\n`;
    let status = current.captured && existsSync(installed)
      ? (current.captured === block && read(installed) === read(source) ? 'installed' : 'outdated')
      : current.captured ? 'broken' : legacy.captured ? 'legacy' : 'absent';
    if (name === 'pi' && status === 'installed') {
      if (!existsSync(piExtensionInstalled)) status = 'broken';
      else if (read(piExtensionInstalled) !== read(piExtensionSource)) status = 'outdated';
    }
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
      changes.push({ client: name, statePath, state: previous || { rc, legacy: legacy.captured }, status: 'installed', shell, rc });
    } else {
      if (action === 'restore' && (!previous || (name === 'pi' && !canRestore))) {
        if (client !== 'all' && client !== 'pi') throw new Error(`${name} 没有统一安装器的还原记录`);
        changes.push({ client: name, status: 'unchanged', rc, shell });
        continue;
      }
      text = current.remaining;
      if (action === 'restore') {
        text = removeBlock(text, marker(name, true)).remaining;
        if (text && !text.endsWith('\n')) text += '\n';
        text += previous.legacy;
      }
      changes.push({ client: name, status: action === 'restore' ? 'restored' : 'uninstalled', rc, shell });
    }
  }
  if (['status', 'snippet'].includes(action) || options.dryRun) return changes.map(({ state, statePath, ...item }) => item);
  mkdirSync(dir, { recursive: true });
  const lock = join(dir, 'install.lock');
  const descriptor = openSync(lock, 'wx', 0o600);
  try {
    if (read(rc) !== original) throw new Error('rc 已被其他进程修改，请重试');
    migrateLegacy?.();
    if (action === 'install') {
      atomic(installed, readFileSync(source), 0o600);
      if (selected.includes('pi') && existsSync(piExtensionSource)) {
        atomic(piExtensionInstalled, readFileSync(piExtensionSource), 0o600);
      }
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
    console.log('用法：node client-attribution.mjs install|status|uninstall|restore claude|grok|gemini|codex|pi|all [--shell bash|zsh] [--rc 文件] [--dry-run]\n只读诊断：node client-attribution.mjs doctor claude|grok|gemini|codex|pi|all [--proxy-url 实际代理地址] [--shell bash|zsh] [--rc 文件]\n临时运行：node client-attribution.mjs run claude|grok|gemini|codex|pi -- [原始参数]\n需要 Node.js 18+。所有客户端使用 bash/zsh 启动包装器；安装或还原后新开终端并重新启动客户端，pi 的 /reload 不会加载 shell 配置；只在连接 Sumpter 的客户端上启用。');
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
    else if (flag === '--proxy-url' && rest.length) options.proxyURL = rest.shift();
    else if (['--shell', '--rc'].includes(flag) && rest.length) options[flag.slice(2)] = rest.shift();
    else throw new Error(`未知或缺值的安装参数：${flag}`);
  }
  console.log(JSON.stringify(action === 'doctor' ? diagnoseClients(client, options) : manage(action, client, options), null, 2));
}
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const args = basename(process.argv[1]) === 'gemini-sumpter-wrapper.mjs'
    ? ['run', 'gemini', '--', ...process.argv.slice(2)] : process.argv.slice(2);
  main(args).catch((error) => { console.error(error.message); process.exitCode = 2; });
}
