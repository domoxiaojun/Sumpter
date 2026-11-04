#!/usr/bin/env node
// SUMPTER_ATTRIBUTION_BUNDLE_VERSION: 2.0.0
// Canonical standalone launcher/installer. Ship copies via sync-client-attribution.mjs.
import { spawn, execFileSync } from 'node:child_process';
import { basename, dirname, join, resolve } from 'node:path';
import { homedir, hostname, userInfo } from 'node:os';
import { randomUUID, createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, existsSync, renameSync, realpathSync, statSync, lstatSync, unlinkSync, symlinkSync, readdirSync, rmdirSync, rmSync } from 'node:fs';
import { fileURLToPath, pathToFileURL } from 'node:url';

const clients = ['claude', 'grok', 'gemini', 'codex', 'pi'];
// Keep the launcher and its companion Pi extension from being mixed across releases.
export const ATTRIBUTION_BUNDLE_VERSION = '2.0.0';
const DEFAULT_PROXY = 'http://127.0.0.1:57878';
const revision = (text, companion = '') => `${bundleVersion(text) || 'unknown'}-${createHash('sha256').update(text).update('\0').update(companion).digest('hex').slice(0, 12)}`;
const companionURL = new URL('./pi-project-attribution.ts', import.meta.url);
export const COLLECTOR_REVISION = revision(readFileSync(fileURLToPath(import.meta.url), 'utf8'), existsSync(companionURL) ? readFileSync(companionURL, 'utf8') : '');
const PI_PASSTHROUGH_COMMANDS = new Set(['install', 'remove', 'uninstall', 'update', 'list', 'config', 'auth']);
const PI_PASSTHROUGH_FLAGS = new Set(['--help', '-h', '--version', '-v']);
const CODEX_PASSTHROUGH_COMMANDS = new Set(['login', 'logout', 'mcp', 'plugin', 'completion', 'doctor', 'features', 'apply', 'execpolicy', 'archive', 'unarchive', 'delete', 'migrate-rollouts', 'migrate-nfs-db']);
const CODEX_VALUE_FLAGS = new Set(['-c', '--config', '-C', '--cd', '-p', '--profile', '-m', '--model', '-i', '--image', '-s', '--sandbox', '-a', '--ask-for-approval', '--enable', '--disable', '--local-provider', '--add-dir']);
const isOwnedHeader = (name) => name.trim().toLowerCase().startsWith('x-sumpter-');
const safe = (value, max = 4096) => typeof value === 'string' && value.trim()
  && !/[\u0000-\u001f\u007f]/u.test(value) && Buffer.byteLength(value, 'utf8') <= max ? value.trim() : undefined;
const object = (value) => value !== null && typeof value === 'object' && !Array.isArray(value);
const shellQuote = (value) => "'" + value.replaceAll("'", "'\\''") + "'";
const read = (path) => existsSync(path) ? readFileSync(path, 'utf8') : '';
function bundleVersion(text) { return /SUMPTER_ATTRIBUTION_BUNDLE_VERSION:\s*([0-9]+\.[0-9]+\.[0-9]+)/u.exec(text)?.[1]; }
function validatePiBundle(launcher, extension) {
  const version = bundleVersion(launcher);
  if (!version || bundleVersion(extension) !== version) throw new Error('Pi 归因资源版本不匹配，请重新安装同一版本的完整归因资源。');
}
function insertOptions(args, options) {
  const end = args.indexOf('--');
  args.splice(end < 0 ? args.length : end, 0, ...options);
}
function codexPassthrough(args) {
  let command;
  for (let i = 0; i < args.length && args[i] !== '--'; i++) {
    const arg = args[i];
    if (['--version', '-V', '--help', '-h'].includes(arg)) return true;
    if (CODEX_VALUE_FLAGS.has(arg)) { i++; continue; }
    if (!arg.startsWith('-') && command === undefined) command = arg;
  }
  return CODEX_PASSTHROUGH_COMMANDS.has(command);
}
function geminiSessionOptions(args) {
  const valueFlags = new Set(['-m', '--model', '-p', '--prompt', '-i', '--prompt-interactive', '-e', '--extensions', '-o', '--output-format', '--proxy', '--sandbox-image', '--include-directories', '--allowed-tools', '--allowed-mcp-server-names', '--approval-mode', '--worktree', '-w']);
  let selected = false;
  const explicit = [], resumed = [];
  for (let i = 0; i < args.length && args[i] !== '--'; i++) {
    const arg = args[i];
    const session = /^(--session-id|--session-file|--resume|--list-sessions)(?:=(.*))?$/u.exec(arg);
    if (session || arg.startsWith('-r')) {
      selected = true;
      const key = session?.[1] || '--resume';
      if (key === '--list-sessions') continue;
      const attached = session ? session[2] : arg.length > 2 ? arg.slice(2).replace(/^=/u, '') : undefined;
      const value = attached ?? (args[i + 1] && !args[i + 1].startsWith('-') ? args[++i] : undefined);
      if (key === '--session-id') explicit.push(value?.trim());
      if (key === '--resume') resumed.push(value?.trim());
    } else if (valueFlags.has(arg)) i++;
  }
  return { selected, explicit: explicit.length === 1 ? explicit[0] : undefined,
    resume: resumed.length === 1 ? resumed[0] : undefined };
}
function parseConfig(text, label) {
  try { return JSON.parse(text); }
  catch { throw new Error(`${label} 不是有效 JSON，未改写原配置`); }
}
function attributionDir(env) {
  return join(env.XDG_DATA_HOME || join(env.HOME || homedir(), '.local/share'), 'sumpter', 'attribution');
}
function proxyBaseURL(value, client) {
  try {
    if (!safe(value) || /[\u0000-\u0020\u007f]/u.test(value)) return undefined;
    const url = new URL(value);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) return undefined;
    let path = url.pathname.replace(/\/+$/u, '');
    // Clients may include their API version suffix in a base URL. Strip only
    // that final segment, never a deployment's custom mount prefix.
    const suffix = client === 'gemini' ? /\/(?:v1|v1beta)$/u : /\/v1$/u;
    if (client) path = path.replace(suffix, '');
    return url.origin + path;
  } catch { return undefined; }
}
/** Independent from restore records. A corrupt policy always stops collection. */
export function readAttributionPolicy(env = process.env) {
  const path = join(attributionDir(env), 'collection.json');
  if (!existsSync(path)) return { exists: false, valid: true, clients: {} };
  try {
    if (statSync(path).size > 65536) throw new Error('size');
    const value = JSON.parse(read(path));
    if (value.version !== 2 || !/^[a-zA-Z0-9_-]{1,128}$/u.test(value.sourceId || '') || !object(value.clients)) throw new Error('schema');
    for (const [name, entry] of Object.entries(value.clients)) {
      if (!clients.includes(name) || !object(entry) || typeof entry.enabled !== 'boolean' || !proxyBaseURL(entry.proxyURL)) throw new Error('entry');
    }
    return { ...value, exists: true, valid: true };
  } catch { return { exists: true, valid: false, clients: {} }; }
}
export function collectionState(client, env = process.env) {
  const policy = readAttributionPolicy(env);
  const entry = policy.clients[client];
  return { enabled: policy.valid && (policy.exists ? entry?.enabled === true : true),
    proxyURL: entry?.proxyURL || env.SUMPTER_BASE_URL || DEFAULT_PROXY,
    sourceId: policy.sourceId, valid: policy.valid };
}
function readObject(path) {
  if (!existsSync(path)) return {};
  if (statSync(path).size > 1024 * 1024) throw new Error('配置超过读取上限');
  const value = JSON.parse(read(path));
  if (!object(value)) throw new Error('配置不是对象');
  return value;
}
// Deliberately narrow: unidentified or overriding client configuration pauses
// collection instead of changing credentials, selecting a provider, or guessing.
export function connectionBinding(client, args, env, cwd, expected) {
  const target = proxyBaseURL(expected, client);
  if (!target) return 'unknown';
  try {
    let candidates = [];
    if (client === 'pi') return 'provider_opt_in';
    if (client === 'claude') {
      const user = readObject(join(env.CLAUDE_CONFIG_DIR || join(env.HOME || homedir(), '.claude'), 'settings.json'));
      // Do not treat a possibly untrusted project setting as the only proof of
      // connection. All visible alternatives must agree with a user/env target.
      candidates = [env.ANTHROPIC_BASE_URL, user.env?.ANTHROPIC_BASE_URL].filter(value => value !== undefined && value !== '');
      if (!candidates.length) return 'unknown';
      const environments = [env, user.env || {}];
      const managed = process.platform === 'darwin' ? '/Library/Application Support/ClaudeCode/managed-settings.json' : '/etc/claude-code/managed-settings.json';
      if (existsSync(managed)) return 'unknown';
      const dirs = [];
      for (let dir = resolve(cwd); ; dir = dirname(dir)) {
        dirs.unshift(dir); if (dirname(dir) === dir) break;
      }
      for (const dir of dirs) for (const name of ['settings.json', 'settings.local.json']) {
        const settingsEnv = readObject(join(dir, '.claude', name)).env || {};
        environments.push(settingsEnv);
        if (settingsEnv.ANTHROPIC_BASE_URL !== undefined) candidates.push(settingsEnv.ANTHROPIC_BASE_URL);
      }
      if (environments.some(value => Object.entries(value).some(([key, value]) =>
        (key.startsWith('CLAUDE_CODE_USE_') || key === 'CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST' || key === 'CLAUDE_CODE_REMOTE')
        && value && value !== '0' && value !== 'false'))) return 'unknown';
      if (args.some(arg => arg === '--settings' || arg.startsWith('--settings=') || arg.startsWith('--settings-sources'))) return 'unknown';
    } else if (client === 'gemini') candidates = [env.SUMPTER_GEMINI_BASE_URL];
    else if (client === 'grok') {
      if (env.GROK_CONFIG_PATH) return 'unknown';
      const config = JSON.parse(env.GROK_CONFIG || '{}');
      // Grok's shared extra_headers cover both generation transports.
      candidates = [config.endpoints?.xai_api_base_url, config.endpoints?.cli_chat_proxy_base_url];
      for (const provider of Object.values(config.model_providers || {})) candidates.push(provider?.base_url);
    } else if (client === 'codex') {
      const context = codexLaunchContext(args, env, cwd);
      let section = '', base;
      const config = read(join(env.CODEX_HOME || join(env.HOME || homedir(), '.codex'), 'config.toml'));
      for (const line of config.split('\n')) {
        const table = /^\s*\[([^\]]+)\]/u.exec(line);
        if (table) { section = table[1].replaceAll(/["']/gu, ''); continue; }
        const field = /^\s*base_url\s*=\s*("(?:[^"\\]|\\.)*"|'[^']*')\s*(?:#.*)?$/u.exec(line);
        if (section === `model_providers.${context.provider}` && field) base = field[1].startsWith('"') ? JSON.parse(field[1]) : field[1].slice(1, -1);
      }
      // Let Codex own project-layer and complex CLI configuration. Unsupported
      // overrides are safe passthroughs, never a reason to break the client.
      for (let dir = resolve(context.directory); ; dir = dirname(dir)) {
        const project = join(dir, '.codex/config.toml');
        const userConfig = join(env.CODEX_HOME || join(env.HOME || homedir(), '.codex'), 'config.toml');
        if (existsSync(project) && (!existsSync(userConfig) || realpathSync(project) !== realpathSync(userConfig))
          && /model_provider|model_providers/u.test(read(project))) return 'unknown';
        if (dirname(dir) === dir) break;
      }
      const end = args.indexOf('--');
      for (const arg of args.slice(0, end < 0 ? args.length : end)) {
        const key = arg.replace(/^(?:--config=|-c)/u, '').split('=')[0].trim();
        if ((key.startsWith('model_providers') && !/^model_providers\.[\w-]+\.(?:env_http_headers|http_headers)(?:\.|$)/u.test(key))
          || (key.startsWith('profiles.') && key.includes('model_provider'))) return 'unknown';
      }
      candidates = [base];
    }
    if (!candidates.length || candidates.some(value => !proxyBaseURL(value, client))) return 'unknown';
    return candidates.every(value => proxyBaseURL(value, client) === target) ? 'matched' : 'mismatch';
  } catch { return 'unknown'; }
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
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !isOwnedHeader(name)));
}
function mergeText(raw, delimiter, headers) {
  const lines = (raw || '').split(delimiter).filter((line) => {
    const key = line.slice(0, line.indexOf(':')).trim().toLowerCase();
    return line.trim() && !isOwnedHeader(key);
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
// Read names only, not header values or TOML semantics. Recognize quoted/escaped
// keys and bare keys while skipping comments and multiline string contents.
// Header tables merge in Codex: empty static values neutralize stale metadata;
// empty env bindings disable inherited values without clearing unrelated headers.
function codexOwnedHeaderNames(text) {
  const names = new Set();
  const tokens = /#[^\n]*|"""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\]|\\[\s\S])*"|'[^']*'|[A-Za-z0-9_-]+/gu;
  for (const match of text.matchAll(tokens)) {
    const token = match[0];
    let name = token;
    if (token.startsWith('#') || token.startsWith('"""') || token.startsWith("'''")) continue;
    if (!/^\s*=/u.test(text.slice(match.index + token.length))) continue;
    if (token.startsWith('"')) {
      try { name = JSON.parse(token.replace(/\\U([0-9a-fA-F]{8})/gu, (_, hex) => JSON.stringify(String.fromCodePoint(parseInt(hex, 16))).slice(1, -1))); }
      catch { continue; }
    } else if (token.startsWith("'")) name = token.slice(1, -1);
    if (isOwnedHeader(name) && /^[!#$%&'*+.^_`|~A-Za-z0-9-]+$/u.test(name)) names.add(name);
  }
  return names;
}
function codexHeaderReset(args, env, cwd) {
  const names = new Set();
  const files = new Set([join(env.CODEX_HOME || join(env.HOME || homedir(), '.codex'), 'config.toml')]);
  // Project config may contribute headers even when the selected provider came
  // from the user config. Only discover private names; Codex retains precedence.
  for (let dir = resolve(cwd); ; dir = dirname(dir)) {
    files.add(join(dir, '.codex', 'config.toml'));
    if (dirname(dir) === dir) break;
  }
  for (const file of files) {
    if (!existsSync(file)) continue;
    if (statSync(file).size > 1024 * 1024) throw new Error('Codex 配置超过归因读取上限，未修改配置。');
    for (const name of codexOwnedHeaderNames(read(file))) names.add(name);
  }
  for (let i = 0; i < args.length && args[i] !== '--'; i++) {
    const arg = args[i];
    const raw = ['-c', '--config'].includes(arg) ? args[++i]
      : arg.startsWith('--config=') ? arg.slice(9)
        : arg.startsWith('-c') && arg.length > 2 ? arg.slice(2) : '';
    for (const name of codexOwnedHeaderNames(raw || '')) names.add(name);
    if (CODEX_VALUE_FLAGS.has(arg) && !['-c', '--config'].includes(arg)) i++;
  }
  return names;
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
    const state = installed.find(row => row.client === name);
    const policy = collectionState(name, env);
    return { ...state, client: name, status: state?.status || 'unknown',
      binding: connectionBinding(name, [], env, home, state?.proxyURL || policy.proxyURL),
      wrapperVersion, diagnosisVersion: ATTRIBUTION_BUNDLE_VERSION, configuration: sources.length ? 'observed' : 'unknown', sources, issues,
      traffic: 'unknown', note: '只展示可见配置来源；项目配置、CLI 参数和运行进程环境可能覆盖它们。未观察到流量不代表绕过代理。' };
  });
}

export function prepareLaunch(client, args, env = process.env, cwd = process.cwd()) {
  const policy = collectionState(client, env);
  const binding = connectionBinding(client, args, env, cwd, policy.proxyURL);
  const enabled = policy.enabled && ['matched', 'provider_opt_in'].includes(binding);
  const attribution = { enabled, binding, sourceId: policy.sourceId, collectorVersion: COLLECTOR_REVISION };
  if (client === 'pi') {
    // Pi package/config/auth commands must remain at argv[0]. Loading an extension
    // before them makes Pi treat the command words as initial prompt messages.
    const passthrough = PI_PASSTHROUGH_COMMANDS.has(args[0]) || PI_PASSTHROUGH_FLAGS.has(args[0]);
    if (passthrough || !enabled) {
      const next = { ...env };
      delete next.SUMPTER_PI_ATTRIBUTION;
      return { command: env.SUMPTER_PI_BIN || 'pi', args: [...args], env: next, attribution };
    }
    const extension = fileURLToPath(new URL('./pi-project-attribution.ts', import.meta.url));
    if (!existsSync(extension)) throw new Error('缺少配套 pi-project-attribution.ts，请使用完整安装包');
    validatePiBundle(`SUMPTER_ATTRIBUTION_BUNDLE_VERSION: ${ATTRIBUTION_BUNDLE_VERSION}`, read(extension));
    // Use Pi's request hook so resumed/forked sessions and workspace changes
    // are observed at dispatch time rather than frozen in launcher environment.
    return {
      command: env.SUMPTER_PI_BIN || 'pi',
      args: ['-e', extension, ...args],
      env: { ...env, SUMPTER_PI_ATTRIBUTION: '1' },
      attribution,
    };
  }
  if (!clients.includes(client)) throw new Error('客户端必须是 claude、grok、gemini 或 codex');
  // Informational invocations should work even before the client is connected.
  if (client === 'codex' && codexPassthrough(args)) {
    return { command: env.SUMPTER_CODEX_BIN || 'codex', args: [...args], env: { ...env } };
  }
  let codex;
  if (client === 'codex') {
    try { codex = codexLaunchContext(args, env, cwd); }
    catch { return { command: env.SUMPTER_CODEX_BIN || 'codex', args: [...args], env: { ...env }, attribution }; }
  }
  const headers = enabled ? collectHeaders(client, codex?.directory || cwd, env) : {};
  if (enabled) {
    headers['X-Sumpter-Client'] = ({ claude: 'claude_code', grok: 'grok_build', gemini: 'gemini_cli', codex: 'codex' })[client];
    headers['X-Sumpter-Collector-Version'] = COLLECTOR_REVISION;
    if (policy.sourceId) headers['X-Sumpter-Source-Id'] = policy.sourceId;
  }
  const next = { ...env };
  const forwarded = [...args];
  if (client === 'claude') {
    if (enabled) claudeConflicts(cwd, env);
    next.ANTHROPIC_CUSTOM_HEADERS = mergeText(env.ANTHROPIC_CUSTOM_HEADERS, '\n', headers);
  } else if (client === 'grok') {
    if (!enabled && (!env.GROK_CONFIG || env.GROK_CONFIG_PATH)) return { command: env.SUMPTER_GROK_BIN || 'grok', args: forwarded, env: next, attribution };
    let config;
    try { config = parseConfig(env.GROK_CONFIG || '{}', 'GROK_CONFIG'); }
    catch (error) {
      if (!enabled) return { command: env.SUMPTER_GROK_BIN || 'grok', args: forwarded, env: next, attribution };
      throw error;
    }
    if (!object(config) || (config.models !== undefined && !object(config.models))) throw new Error('GROK_CONFIG 必须是包含 models 对象的 JSON 配置');
    config.models ??= {};
    if (config.models.extra_headers !== undefined && !object(config.models.extra_headers)) throw new Error('GROK_CONFIG models.extra_headers 必须是对象');
    config.models.extra_headers = { ...keepHeaders(config.models.extra_headers || {}), ...headers };
    next.GROK_CONFIG = JSON.stringify(config);
  } else if (client === 'codex') {
    // Same local collection as Claude; only the client's header transport differs.
    // -c splits keys on dots literally: quoting a header name would send no header.
    for (const key of Object.keys(next)) if (key.startsWith('SUMPTER_CODEX_X_SUMPTER_')) delete next[key];
    const owned = codexHeaderReset(args, env, codex.directory);
    const overrides = [];
    for (const name of owned) {
      // Codex splits override paths literally, including dots in quoted keys.
      // Do not replace an entire table: that loses unrelated CLI header overrides.
      if (name.includes('.')) throw new Error('Codex 归因无法安全覆盖含点号的私有 Header，请先移除这类旧声明。');
      overrides.push('-c', `model_providers.${codex.provider}.http_headers.${name}=""`,
        '-c', `model_providers.${codex.provider}.env_http_headers.${name}=""`);
    }
    for (const [name, value] of Object.entries(headers)) {
      const variable = `SUMPTER_CODEX_${name.replaceAll(/[^A-Za-z0-9]/gu, '_').toUpperCase()}`;
      next[variable] = value;
      overrides.push('-c', `model_providers.${codex.provider}.env_http_headers.${name}=${JSON.stringify(variable)}`);
    }
    // Last override wins, including when the user supplied a header table via -c.
    insertOptions(forwarded, overrides);
  } else {
    const base = safe(env.SUMPTER_GEMINI_BASE_URL);
    const token = safe(env.SUMPTER_AUTH_TOKEN);
    if (!base || !token) {
      next.GEMINI_CLI_CUSTOM_HEADERS = mergeText(env.GEMINI_CLI_CUSTOM_HEADERS, ',', {});
      return { command: env.SUMPTER_GEMINI_BIN || env.GEMINI_CLI_BIN || 'gemini', args: forwarded, env: next, attribution: { ...attribution, enabled: false, binding: 'unknown' } };
    }
    const url = new URL(base);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password || url.search || url.hash) throw new Error('Gemini Base URL 必须是无凭据、query、fragment 的 HTTP(S) 地址');
    const { selected: sessionArg, explicit, resume } = geminiSessionOptions(args);
    // Only a complete UUID identifies a resumed session; ordinals/latest/prefixes
    // are selectors resolved by Gemini and cannot become the analytics identity.
    const resolvedResume = typeof resume === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/iu.test(resume) ? resume : undefined;
    const provided = explicit ?? resolvedResume;
    if (enabled && provided && safe(provided, 256) && /^[A-Za-z0-9_-]+$/u.test(provided)) {
      headers['X-Sumpter-Session-Id'] = provided;
    }
    // Fresh sessions receive the same ID in the native CLI and the header.
    if (enabled && !sessionArg) {
      const id = env.SUMPTER_GEMINI_SESSION_ID || randomUUID();
      if (!safe(id, 256) || !/^[A-Za-z0-9_-]+$/u.test(id)) throw new Error('SUMPTER_GEMINI_SESSION_ID 必须是有界字母、数字、下划线或连字符标识');
      headers['X-Sumpter-Session-Id'] = id;
      insertOptions(forwarded, ['--session-id', id]);
    }
    next.GOOGLE_GEMINI_BASE_URL = base;
    next.GEMINI_API_KEY = token;
    next.GEMINI_CLI_CUSTOM_HEADERS = mergeText(env.GEMINI_CLI_CUSTOM_HEADERS, ',', headers);
    next.GEMINI_API_KEY_AUTH_MECHANISM = 'x-goog-api-key';
    for (const key of ['GOOGLE_GENAI_USE_VERTEXAI', 'GOOGLE_GENAI_USE_GCA', 'GOOGLE_VERTEX_BASE_URL', 'GEMINI_CLI_USE_COMPUTE_ADC']) delete next[key];
  }
  return { command: env[`SUMPTER_${client.toUpperCase()}_BIN`] || (client === 'gemini' && env.GEMINI_CLI_BIN) || client, args: forwarded, env: next, attribution };
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

function processStart(pid) {
  try {
    return execFileSync('ps', ['-p', String(pid), '-o', 'lstart='], {
      encoding: 'utf8', env: { ...process.env, LC_ALL: 'C' }, timeout: 1000,
      maxBuffer: 256, stdio: ['ignore', 'pipe', 'ignore'],
    }).trim() || undefined;
  } catch { return undefined; }
}
function lockOwnerAlive(owner) {
  if (owner.host !== hostname()) return true; // Cannot probe another host's PID.
  try { process.kill(owner.pid, 0); }
  catch (error) { return error.code !== 'ESRCH'; }
  const started = owner.started && processStart(owner.pid);
  return !started || started === owner.started; // Detect a reused PID when ps is available.
}
function releaseInstallLock(lock, ownerFile) {
  // A unique child name protects a replacement owner even if two recoverers
  // observed the same dead process. Never recursively remove the shared lock.
  try { unlinkSync(join(lock, ownerFile)); }
  catch (error) { if (!['ENOENT', 'ENOTDIR'].includes(error.code)) throw error; }
  try { rmdirSync(lock); }
  catch (error) { if (!['ENOENT', 'ENOTDIR', 'ENOTEMPTY', 'EEXIST'].includes(error.code)) throw error; }
}
export function acquireInstallLock(dir) {
  mkdirSync(dir, { recursive: true });
  const lock = join(dir, 'install.lock');
  const token = randomUUID();
  const ownerFile = `owner-${token}.json`;
  const pending = join(dir, `.install-lock-${token}`);
  mkdirSync(pending, { mode: 0o700 });
  try {
    writeFileSync(join(pending, ownerFile), JSON.stringify({ version: 1, pid: process.pid,
      host: hostname(), started: processStart(process.pid), token }), { mode: 0o600 });
    for (let attempt = 0; attempt < 8; attempt++) {
      try {
        // A published lock is always nonempty. POSIX rename cannot replace
        // another nonempty directory, so owner metadata is visible atomically.
        renameSync(pending, lock);
        let released = false;
        return () => { if (!released) { releaseInstallLock(lock, ownerFile); released = true; } };
      } catch (error) {
        if (!['EEXIST', 'ENOTEMPTY', 'ENOTDIR'].includes(error.code)) throw error;
      }
      try {
        const info = lstatSync(lock);
        if (info.isFile() && info.size === 0) {
          // The previous installer wrote an empty file. Give an active old
          // installer a grace period; new locks are recovered by owner, not age.
          if (Date.now() - info.mtimeMs < 5 * 60_000) throw new Error('旧版归因安装锁尚未过期，请等待 5 分钟后重试。');
          const current = lstatSync(lock);
          if (current.isFile() && current.ino === info.ino && current.dev === info.dev
            && current.mtimeMs === info.mtimeMs && current.size === 0) {
            try { unlinkSync(lock); }
            catch (error) { if (!['ENOENT', 'EISDIR', 'EPERM'].includes(error.code)) throw error; }
          }
          continue;
        }
        if (!info.isDirectory()) throw new Error('归因安装锁格式异常，未修改配置。');
        const names = readdirSync(lock);
        if (names.length === 0) continue; // Another owner is finishing cleanup.
        if (names.length !== 1 || !/^owner-[a-f0-9-]{36}\.json$/u.test(names[0])) throw new Error('归因安装锁持有者信息异常，未修改配置。');
        const path = join(lock, names[0]);
        const ownerInfo = lstatSync(path);
        if (!ownerInfo.isFile() || ownerInfo.size > 4096) throw new Error('归因安装锁持有者信息异常，未修改配置。');
        const owner = JSON.parse(readFileSync(path, 'utf8'));
        if (owner.version !== 1 || !Number.isSafeInteger(owner.pid) || owner.pid < 1
          || typeof owner.host !== 'string' || names[0] !== `owner-${owner.token}.json`) throw new Error('归因安装锁持有者信息异常，未修改配置。');
        if (lockOwnerAlive(owner)) throw new Error('另一个归因管理进程正在运行，请稍后重试。');
        releaseInstallLock(lock, names[0]);
      } catch (error) {
        if (!['ENOENT', 'ENOTDIR'].includes(error.code)) throw error;
      }
    }
    throw new Error('归因安装锁正在被其他进程更新，请稍后重试。');
  } finally { rmSync(pending, { recursive: true, force: true }); }
}
function atomicLink(path, target) {
  const temp = `${path}.${randomUUID()}.tmp`;
  try { symlinkSync(target, temp); renameSync(temp, path); }
  finally { if (existsSync(temp)) unlinkSync(temp); }
}
function installBundle(dir, source) {
  const launcher = read(source);
  const companion = read(join(dirname(source), 'pi-project-attribution.ts'));
  if (!companion) throw new Error('缺少配套 pi-project-attribution.ts，请使用完整归因资源。');
  validatePiBundle(launcher, companion);
  const digest = createHash('sha256').update(launcher).update('\0').update(companion).digest('hex');
  const bundle = join(dir, 'bundles', digest);
  mkdirSync(bundle, { recursive: true, mode: 0o700 });
  atomic(join(bundle, 'client-attribution.mjs'), launcher);
  atomic(join(bundle, 'pi-project-attribution.ts'), companion);
  atomic(join(bundle, 'manifest.json'), JSON.stringify({ version: bundleVersion(launcher), revision: revision(launcher, companion), digest }));
  // Both entry points resolve through one pointer; a running process keeps its
  // immutable module directory, including its matching Pi companion.
  atomicLink(join(dir, 'current'), `bundles/${digest}`);
  atomicLink(join(dir, 'client-attribution.mjs'), 'current/client-attribution.mjs');
  atomicLink(join(dir, 'pi-project-attribution.ts'), 'current/pi-project-attribution.ts');
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
  if (!['pi', 'all'].includes(client) || ['snippet', 'disable'].includes(action)) return manageShell(action, client, options, env, source);
  const shells = manageShell(action, client, { ...options, dryRun: true }, env, source);
  // Capture old private resource bytes before install updates them.
  let migration;
  try { migration = legacyPiMigration(env, source); }
  catch (error) {
    if (!['status', 'uninstall'].includes(action)) throw error;
    migration = { pending: false, note: '旧版 Pi 备份记录损坏，已保留；当前归因配置仍可关闭或移除。' };
  }
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
  if (!['enable', 'disable', 'install', 'status', 'uninstall', 'restore', 'snippet'].includes(action) || ![...clients, 'all'].includes(client)) throw new Error('用法：client-attribution.mjs enable|disable|install|status|uninstall|restore|snippet claude|grok|gemini|codex|pi|all [--proxy-url 地址] [--shell bash|zsh] [--rc 文件] [--dry-run]');
  const installing = action === 'install' || action === 'enable';
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
  // All clients share this launcher. Updating any one must keep an existing Pi
  // companion in sync, including Pi installed for a different shell/rc file.
  if (installing && !read(piExtensionSource)) {
    throw new Error('缺少配套 pi-project-attribution.ts，请使用完整安装包或重新下载');
  }
  if (installing) validatePiBundle(read(source), read(piExtensionSource));
  const policy = readAttributionPolicy(env);
  const policyPath = join(dir, 'collection.json');
  const policyBefore = read(policyPath);
  const policyNext = { version: 2, sourceId: policy.sourceId || randomUUID(), clients: { ...policy.clients } };
  const target = options.proxyURL || env.SUMPTER_BASE_URL;
  if (installing && target && !proxyBaseURL(target)) throw new Error('Sumpter 地址必须是无凭据、query、fragment 的 HTTP(S) 地址。');
  const original = read(rc);
  let text = original;
  const changes = [];
  for (const name of selected) {
    const current = removeBlock(text, marker(name));
    const legacy = removeBlock(current.remaining, marker(name, true));
    const statePath = join(dir, `${name}-${Buffer.from(rc).toString('base64url')}.json`);
    let previous = null, backupError = false;
    const previousText = read(statePath);
    try {
      previous = previousText ? JSON.parse(previousText) : null;
      if (previous && (previous.rc !== rc || typeof previous.legacy !== 'string')) throw new Error('schema');
    } catch { previous = null; backupError = true; }
    const block = `${marker(name)[0]}\n${name}() { command node ${shellQuote(installed)} run ${name} -- "$@"; }\n${marker(name)[1]}\n`;
    let status = current.captured && existsSync(installed)
      ? (current.captured === block && read(installed) === read(source) ? 'installed' : 'outdated')
      : current.captured ? 'broken' : legacy.captured ? 'legacy' : 'absent';
    if (status === 'installed') {
      if (!existsSync(piExtensionInstalled)) status = 'broken';
      else if (read(piExtensionInstalled) !== read(piExtensionSource)) status = 'outdated';
    }
    const registration = status;
    const entry = policy.clients[name];
    const enabled = policy.valid && (policy.exists ? entry?.enabled === true : Boolean(current.captured || legacy.captured));
    if (!enabled && current.captured && ['installed', 'outdated'].includes(status)) status = 'disabled';
    const canRestore = Boolean(previous && (current.captured || legacy.captured !== previous.legacy));
    if (action === 'status') {
      let lastLaunch;
      try {
        const receipt = readObject(join(dir, `launch-${name}.json`));
        if (policy.sourceId && receipt.sourceId === policy.sourceId && typeof receipt.enabled === 'boolean'
          && ['matched', 'mismatch', 'unknown', 'provider_opt_in'].includes(receipt.binding)
          && /^[a-zA-Z0-9._-]{1,64}$/u.test(receipt.collectorVersion || '')
          && typeof receipt.startedAt === 'string' && receipt.startedAt.length <= 32 && Number.isFinite(Date.parse(receipt.startedAt))) {
          const { sourceId, enabled, binding, collectorVersion, startedAt } = receipt;
          lastLaunch = { sourceId, enabled, binding, collectorVersion, startedAt };
        }
      } catch { /* optional receipt must not block configuration management */ }
      const installedVersion = existsSync(installed) ? revision(read(installed), read(piExtensionInstalled)) : null;
      changes.push({ client: name, status, registration, rc, shell, canRestore, enabled,
        canDisable: Boolean(current.captured || legacy.captured || entry?.enabled || !policy.valid),
        backupState: backupError ? 'invalid' : previous ? 'available' : 'none',
        restoreTarget: previous?.legacy ? 'legacy' : 'absent',
        policyState: !policy.valid ? 'invalid' : enabled ? 'enabled' : 'disabled',
        proxyURL: entry?.proxyURL || target || DEFAULT_PROXY,
        sourceId: policy.sourceId || null, installedVersion, availableVersion: revision(read(source), read(piExtensionSource)),
        lastLaunch: lastLaunch || null,
        note: backupError ? '备份记录损坏；仍可关闭或移除归因，损坏记录会另存保留。'
          : !policy.valid ? '归因开关文件损坏，已停止补充采集；可关闭后重新启用。' : undefined });
      continue;
    }
    if (action === 'snippet') { changes.push({ client: name, snippet: `${name}() { command node ${shellQuote(source)} run ${name} -- "$@"; }` }); continue; }
    if (installing) {
      if (name === 'claude') claudeConflicts(process.cwd(), env);
      if (name === 'grok' && env.GROK_CONFIG_PATH) throw new Error('请先解除 GROK_CONFIG_PATH 覆盖');
      text = legacy.remaining;
      if (text && !text.endsWith('\n')) text += '\n';
      text += block;
      // Reinstall does not overwrite the pre-migration block retained for restore.
      changes.push({ client: name, statePath, invalidBackup: backupError ? previousText : null,
        state: current.captured && previous ? previous : { rc, legacy: legacy.captured }, status: 'installed', shell, rc });
    } else {
      if (action === 'restore' && backupError) throw new Error(`${name} 的备份记录损坏，不能恢复；可使用关闭或移除归因。`);
      if (action === 'restore' && (!previous || (name === 'pi' && !canRestore))) {
        if (client !== 'all' && client !== 'pi') throw new Error(`${name} 没有统一安装器的还原记录`);
        changes.push({ client: name, status: 'unchanged', rc, shell });
        continue;
      }
      // A current wrapper stays registered while disabled so Gemini connection
      // setup remains intact. Legacy snippets cannot honor the new switch.
      text = action === 'disable' ? removeBlock(text, marker(name, true)).remaining : legacy.remaining;
      if (action === 'restore') {
        text = removeBlock(text, marker(name, true)).remaining;
        if (text && !text.endsWith('\n')) text += '\n';
        text += previous.legacy;
      }
      changes.push({ client: name, statePath, invalidBackup: backupError ? previousText : null,
        status: action === 'restore' ? 'restored' : action === 'disable' ? 'disabled' : 'uninstalled', rc, shell });
    }
    policyNext.clients[name] = { enabled: installing ? action === 'enable' || (entry?.enabled ?? true) : false,
      proxyURL: (installing && target) || entry?.proxyURL || DEFAULT_PROXY, updatedAt: new Date().toISOString() };
  }
  if (['status', 'snippet'].includes(action) || options.dryRun) return changes.map(({ state, statePath, invalidBackup, ...item }) => item);
  const releaseLock = acquireInstallLock(dir);
  try {
    if (read(rc) !== original || read(policyPath) !== policyBefore) throw new Error('归因配置已被其他进程修改，请重试');
    if (!policy.valid) atomic(`${policyPath}.invalid-${Date.now()}`, policyBefore);
    for (const change of changes) if (change.invalidBackup) {
      atomic(`${change.statePath}.invalid-${Date.now()}`, change.invalidBackup);
      unlinkSync(change.statePath);
    }
    migrateLegacy?.();
    if (installing) {
      installBundle(dir, source);
      for (const change of changes) atomic(change.statePath, JSON.stringify(change.state));
    }
    atomic(policyPath, JSON.stringify(policyNext));
    if (text !== original) {
      const backup = `${rc}.sumpter-attribution-bak-${Date.now()}-${randomUUID()}`;
      atomic(backup, original);
      atomic(rc, text, existsSync(rc) ? statSync(rc).mode & 0o777 : 0o600);
      for (const change of changes) change.backup = backup;
    }
    // Complete this restoration cycle only after rc was saved successfully.
    if (action === 'restore') {
      for (const change of changes) if (change.statePath && existsSync(change.statePath)) unlinkSync(change.statePath);
    }
  } finally { releaseLock(); }
  const finalStatuses = manageShell('status', client, options, env, source);
  return finalStatuses.map(item => ({ ...item, action, result: changes.find(change => change.client === item.client)?.status,
    backup: changes.find(change => change.client === item.client)?.backup }));
}
async function main(argv) {
  if (['--help', '-h'].includes(argv[0]) || argv.length === 0) {
    console.log('用法：node client-attribution.mjs enable|disable|install|status|uninstall|restore claude|grok|gemini|codex|pi|all [--proxy-url 地址] [--shell bash|zsh] [--rc 文件] [--dry-run]\n启用：enable；关闭补充采集：disable；更新且保留开关：install；恢复安装前配置：restore；移除终端注册：uninstall。\n只读诊断：node client-attribution.mjs doctor all [--proxy-url 实际代理地址] [--shell bash|zsh]\n临时运行：node client-attribution.mjs run claude|grok|gemini|codex|pi -- [原始参数]\n需要 Node.js 18+。首次启用后新开终端并重启客户端；关闭后请重启已运行的 Claude/Grok/Gemini/Codex，Pi 新版钩子每次请求检查开关。恢复备份可能恢复旧版归因。');
    return;
  }
  let [action, client, ...rest] = argv;
  if (action === 'run') {
    if (rest[0] === '--') rest.shift();
    const launch = prepareLaunch(client, rest);
    if (launch.attribution && readAttributionPolicy(process.env).exists) {
      try { atomic(join(attributionDir(process.env), `launch-${client}.json`), JSON.stringify({ ...launch.attribution, startedAt: new Date().toISOString() })); }
      catch { /* Observational receipt must never prevent the client from starting. */ }
    }
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
