import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, existsSync, realpathSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { collectHeaders, prepareLaunch, manage, sanitizeRemote } from '../clients/client-attribution.mjs';
const source = fileURLToPath(new URL('../clients/client-attribution.mjs', import.meta.url));
function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'sumpter-unified-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const home = join(root, 'home'); mkdirSync(home);
  const repo = join(root, "中文项目 ' space"); mkdirSync(repo);
  execFileSync('git', ['init', '-q', repo]);
  execFileSync('git', ['-C', repo, 'config', 'remote.origin.url', 'https://user:synthetic@example.invalid/team/repo.git?token=synthetic#secret']);
  const nested = join(repo, 'nested'); mkdirSync(nested);
  const env = { HOME: home, XDG_DATA_HOME: join(home, '.local/share'), SHELL: '/bin/bash', USER: 'tester', PATH: process.env.PATH,
    SUMPTER_GEMINI_BASE_URL: 'http://127.0.0.1:57878', SUMPTER_AUTH_TOKEN: 'synthetic-token' };
  return { root, home, repo, nested, env };
}
const textHeaders = (text, sep) => Object.fromEntries(text.split(sep).filter(Boolean).map((line) => { const i=line.indexOf(':'); return [line.slice(0,i).trim(), line.slice(i+1).trim()]; }));

test('three clients share Unicode project/root/user/sanitized remote and preserve unrelated settings', (t) => {
  const f = fixture(t);
  const base = collectHeaders('claude', f.nested, f.env);
  assert.equal(decodeURIComponent(base['X-Sumpter-Workspace']), f.repo);
  assert.equal(decodeURIComponent(base['X-Sumpter-Project']), "中文项目 ' space");
  assert.equal(decodeURIComponent(base['X-Sumpter-Git-Remote']), 'https://example.invalid/team/repo.git');
  const rawEnv = { ...f.env,
    ANTHROPIC_CUSTOM_HEADERS: 'X-Test: unchanged\nx-sumpter-project: stale\nX-Sumpter-Session-Id: stale',
    GROK_CONFIG: JSON.stringify({ other: 42, models: { temperature: 0.2, extra_headers: { 'X-Test': 'unchanged', 'x-sumpter-project': 'stale', 'X-Sumpter-Session-Id': 'stale' } } }),
    GEMINI_CLI_CUSTOM_HEADERS: 'X-Test: unchanged, x-sumpter-project: stale',
  };
  for (const client of ['claude', 'grok', 'gemini']) {
    const args = ['--model', 'test-model', 'prompt with spaces'];
    const launch = prepareLaunch(client, args, rawEnv, f.nested);
    const headers = client === 'claude' ? textHeaders(launch.env.ANTHROPIC_CUSTOM_HEADERS, '\n')
      : client === 'grok' ? JSON.parse(launch.env.GROK_CONFIG).models.extra_headers : textHeaders(launch.env.GEMINI_CLI_CUSTOM_HEADERS, ',');
    for (const [key,value] of Object.entries(base)) assert.equal(headers[key], value);
    assert.equal(headers['X-Test'], 'unchanged');
    assert.equal(headers['x-sumpter-project'], undefined);
    for (const [key,value] of Object.entries(headers)) assert.doesNotThrow(() => new Headers({ [key]: value }));
    assert.deepEqual(launch.args.slice(0,3), args);
    if (client !== 'gemini') assert.equal(headers['X-Sumpter-Session-Id'], undefined);
    if (client === 'grok') { const cfg=JSON.parse(launch.env.GROK_CONFIG); assert.equal(cfg.other,42); assert.equal(cfg.models.temperature,0.2); }
    if (client === 'gemini') assert.equal(headers['X-Sumpter-Session-Id'],launch.args.at(-1));
  }
  assert.equal(rawEnv.ANTHROPIC_CUSTOM_HEADERS.includes('stale'), true, 'no parent environment mutation');
});

test('new directory clears stale metadata, fallbacks and validation do not expose secrets', (t) => {
  const f=fixture(t); const headers=collectHeaders('grok',f.home,f.env);
  assert.equal(decodeURIComponent(headers['X-Sumpter-Workspace']), f.home);
  assert.equal(headers['X-Sumpter-Git-Remote'],undefined);
  assert.equal(sanitizeRemote('git@example.invalid:repo.git?secret=1'), 'ssh://example.invalid/repo.git');
  for(const remote of ['file:///local/secret','https://host/\nsecret','relative/path']) assert.equal(sanitizeRemote(remote),undefined);
  assert.throws(() => prepareLaunch('grok',[],{...f.env,GROK_CONFIG:'{synthetic-secret'},f.repo), /GROK_CONFIG 不是有效 JSON/);
  assert.throws(() => prepareLaunch('grok',[],{...f.env,GROK_CONFIG_PATH:'/private/config'},f.repo), /覆盖/);
  mkdirSync(join(f.home,'.claude')); writeFileSync(join(f.home,'.claude/settings.json'),JSON.stringify({env:{ANTHROPIC_CUSTOM_HEADERS:'X: synthetic'}}));
  assert.throws(() => prepareLaunch('claude',[],f.env,f.repo), /覆盖动态归因/);
});

test('Gemini explicit/resume session arguments never acquire a new wrapper identity', (t) => {
  const f=fixture(t);
  for(const args of [['--resume','last'],['--resume=last'],['-r'],['--session-id','existing'],['--session-file=data'],['--list-sessions']]) {
    const launch=prepareLaunch('gemini',args,{...f.env,SUMPTER_GEMINI_SESSION_ID:'stale'},f.repo);
    assert.deepEqual(launch.args,args);
    assert.equal(/X-Sumpter-Session-Id:/i.test(launch.env.GEMINI_CLI_CUSTOM_HEADERS),false);
  }
});

test('bash/zsh install is idempotent, migrates legacy blocks and restores locally without deleting later edits', (t) => {
  const f=fixture(t);
  for(const shell of ['bash','zsh'].filter((name) => existsSync(`/bin/${name}`))) {
    const rc=join(f.home,`.${shell}rc`);
    const old='# >>> sumpter cc-project-attribution >>>\nsource /synthetic/old.sh\n# <<< sumpter cc-project-attribution <<<\n';
    writeFileSync(rc, '# user rc\n'+old);
    const opts={shell,rc};
    assert.equal(manage('status','claude',opts,f.env)[0].status,'legacy');
    manage('install','all',{...opts,dryRun:true},f.env);
    assert.equal(existsSync(join(f.env.XDG_DATA_HOME,'sumpter/attribution')),false);
    const first=manage('install','all',opts,f.env);
    assert.equal(readFileSync(first[0].backup,'utf8'),'# user rc\n'+old);
    const installed=readFileSync(rc,'utf8');
    assert.equal(installed.includes('synthetic/old'),false);
    assert.equal(installed.includes('synthetic-token'),false);
    manage('install','all',opts,f.env); assert.equal(readFileSync(rc,'utf8'),installed);
    assert.deepEqual(manage('status','all',opts,f.env).map(x=>x.status),['installed','installed','installed','installed']);
    execFileSync(`/bin/${shell}`,['-n',rc]);
    writeFileSync(rc,installed+'# later user change\n');
    manage('restore','claude',opts,f.env);
    const restored=readFileSync(rc,'utf8');assert.ok(restored.includes(old));assert.ok(restored.includes('# later user change'));assert.ok(restored.includes('client-attribution grok'));
    manage('uninstall','grok',opts,f.env);assert.equal(manage('status','grok',opts,f.env)[0].status,'absent');
    manage('uninstall','grok',opts,f.env);
    // Use a separate install state for the next shell's dry run.
    rmSync(f.env.XDG_DATA_HOME,{recursive:true,force:true});
  }
});

test('actual shell functions forward argv and child-scoped headers; standalone Gemini alias uses same code', (t) => {
  const f=fixture(t);
  const bin=join(f.root,'bin');mkdirSync(bin);
  const fake=join(bin,'fake');
  writeFileSync(fake,`#!${process.execPath}\nconsole.log(JSON.stringify({args:process.argv.slice(2),headers:process.env.ANTHROPIC_CUSTOM_HEADERS,gemini:process.env.GEMINI_CLI_CUSTOM_HEADERS}));\n`,{mode:0o700});
  for(const shell of ['bash','zsh'].filter((name) => existsSync(`/bin/${name}`))) {
    const rc=join(f.home,`.${shell}rc`);const env={...f.env,SUMPTER_CLAUDE_BIN:fake,GEMINI_CLI_BIN:fake};
    manage('install','claude',{shell,rc},env);
    const out=execFileSync(`/bin/${shell}`,['-c','source "$1"; claude "a b" "$(printf x)"','test',rc],{env,cwd:f.repo,encoding:'utf8'});
    const result=JSON.parse(out);assert.deepEqual(result.args,['a b','x']);assert.ok(result.headers.includes('uri-v1'));
  }
  const out=execFileSync(process.execPath,[fileURLToPath(new URL('../clients/gemini-sumpter-wrapper.mjs',import.meta.url)),'--resume=last'],{env:{...f.env,GEMINI_CLI_BIN:fake},cwd:f.repo,encoding:'utf8'});
  assert.deepEqual(JSON.parse(out).args,['--resume=last']);
  const linked=join(f.root,'gemini-sumpter-wrapper.mjs');
  symlinkSync(fileURLToPath(new URL('../clients/gemini-sumpter-wrapper.mjs',import.meta.url)),linked);
  const linkedOut=execFileSync(process.execPath,[linked,'--resume=last'],{env:{...f.env,GEMINI_CLI_BIN:fake},cwd:f.repo,encoding:'utf8'});
  assert.deepEqual(JSON.parse(linkedOut).args,['--resume=last']);
});

test('malformed markers, rc symlinks and per-client uninstall preserve user state', (t) => {
  const f=fixture(t);const target=join(f.home,'actual'); const rc=join(f.home,'rc');writeFileSync(target,'# rc\n');symlinkSync(target,rc);
  manage('install','claude',{shell:'bash',rc},f.env);assert.equal(realpathSync(rc),target);
  manage('uninstall','claude',{shell:'bash',rc},f.env);assert.equal(readFileSync(target,'utf8'),'# rc\n');
  writeFileSync(target,'# >>> sumpter client-attribution claude >>>\nbroken');
  assert.throws(()=>manage('install','claude',{shell:'bash',rc},f.env),/不完整/);
  assert.equal(readFileSync(target,'utf8'),'# >>> sumpter client-attribution claude >>>\nbroken');
});

test('distributed scripts are generated from one canonical source', () => {
  execFileSync(process.execPath,[fileURLToPath(new URL('../maintenance/sync-client-attribution.mjs',import.meta.url)),'--check']);
});

test('status distinguishes outdated and missing scripts; restore all skips clients without records', (t) => {
  const f = fixture(t);
  const opts = { shell: 'bash', rc: join(f.home, '.bashrc') };
  assert.equal(manage('status', 'claude', opts, f.env)[0].canRestore, false);
  manage('install', 'claude', opts, f.env);
  assert.equal(manage('status', 'claude', opts, f.env)[0].canRestore, true);
  const installed = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/client-attribution.mjs');
  writeFileSync(installed, '// old version');
  assert.equal(manage('status', 'claude', opts, f.env)[0].status, 'outdated');
  rmSync(installed);
  assert.equal(manage('status', 'claude', opts, f.env)[0].status, 'broken');
  const result = manage('restore', 'all', opts, f.env);
  assert.deepEqual(result.map(x => x.status), ['restored', 'unchanged', 'unchanged', 'unchanged']);
  assert.equal(manage('status', 'claude', opts, f.env)[0].canRestore, false);
});

test('Linux automation performs install, status and restore with isolated HOME', (t) => {
  const f = fixture(t);
  const script = fileURLToPath(new URL('../../platforms/linux/scripts/setup-client-attribution.sh', import.meta.url));
  const run = (action) => execFileSync('/bin/bash', [script, action, 'all', '--shell', 'bash'], { env: f.env, encoding: 'utf8' });
  assert.match(run('status'), /claude：未安装/);
  assert.match(run('install'), /claude：已安装/);
  assert.match(run('status'), /grok：已安装/);
  assert.match(run('status'), /pi：已安装/);
  const rc = join(f.home, '.bashrc');
  writeFileSync(rc, readFileSync(rc, 'utf8') + '# later edit\n');
  assert.match(run('restore'), /gemini：未安装/);
  assert.equal(existsSync(join(f.home, '.pi/agent/extensions/pi-project-attribution.ts')), false);
  assert.match(readFileSync(rc, 'utf8'), /# later edit/);
});

test('new rc under a symlinked parent retains the same restore record after creation', (t) => {
  const f = fixture(t);
  const alias = join(f.root, 'home-alias');
  symlinkSync(f.home, alias);
  const env = { ...f.env, HOME: alias };
  const opts = { shell: 'zsh' };
  manage('install', 'claude', opts, env);
  assert.equal(manage('status', 'claude', opts, env)[0].canRestore, true);
  manage('restore', 'all', opts, env);
  assert.equal(readFileSync(join(f.home, '.zshrc'), 'utf8'), '');
});

test('remote Linux setup downloads authenticated installer and preserves failure before writes', async (t) => {
  const { createServer } = await import('node:http');
  const { spawn } = await import('node:child_process');
  const f = fixture(t);
  const script = join(f.root, 'setup-client-attribution.sh');
  writeFileSync(script, readFileSync(new URL('../../platforms/linux/scripts/setup-client-attribution.sh', import.meta.url)));
  let extensionMissing = false;
  const resources = {
    '/__sumpter/client-attribution.mjs': source,
    '/__sumpter/pi-project-attribution.ts': fileURLToPath(new URL('../clients/pi-project-attribution.ts', import.meta.url)),
  };
  const server = createServer((req, res) => {
    if (!resources[req.url] || req.headers.authorization !== 'Bearer synthetic-token') {
      res.writeHead(401).end(); return;
    }
    if (extensionMissing && req.url.endsWith('.ts')) { res.writeHead(404).end(); return; }
    res.end(readFileSync(resources[req.url]));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => server.close());
  const run = (token, action = 'install') => new Promise((resolve, reject) => {
    const child = spawn('/bin/bash', [script, action, 'all', '--shell', 'bash'], {
      env: { ...f.env, SUMPTER_BASE_URL: `http://127.0.0.1:${server.address().port}`, SUMPTER_AUTH_TOKEN: token },
    });
    let output = '';
    child.stdout.on('data', data => { output += data; });
    child.stderr.on('data', data => { output += data; });
    child.on('error', reject);
    child.on('close', code => resolve({ code, output }));
  });
  assert.notEqual((await run('incorrect')).code, 0);
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
  extensionMissing = true;
  assert.notEqual((await run('synthetic-token')).code, 0);
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
  assert.equal(existsSync(join(f.home, '.pi')), false);
  extensionMissing = false;
  const result = await run('synthetic-token');
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /claude：已安装/);
  assert.match(result.output, /pi：已安装/);
  const restored = await run('synthetic-token', 'restore');
  assert.equal(restored.code, 0, restored.output);
  assert.match(restored.output, /pi：未安装/);
  assert.equal(existsSync(join(f.home, '.pi/agent/extensions/pi-project-attribution.ts')), false);
});

test('pi install/update preserves first backup and restores exact bytes without touching provider or shell', (t) => {
  const f = fixture(t);
  const target = join(f.home, '.pi/agent/extensions/pi-project-attribution.ts');
  mkdirSync(join(f.home, '.pi/agent/extensions'), { recursive: true });
  const original = Buffer.from([0, 255, 10, 65]);
  writeFileSync(target, original);
  const provider = join(f.home, '.pi/agent/models.json');
  writeFileSync(provider, '{"synthetic":"unchanged"}');
  const opts = { shell: 'fish' }; // pi never reads or writes shell rc.
  assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'outdated');
  manage('install', 'pi', { ...opts, dryRun: true }, f.env);
  assert.deepEqual(readFileSync(target), original);
  assert.equal(existsSync(f.env.XDG_DATA_HOME), false);
  manage('install', 'pi', opts, f.env);
  assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'installed');
  writeFileSync(target, '// later edit');
  manage('install', 'pi', opts, f.env);
  manage('install', 'pi', opts, f.env);
  assert.equal(manage('status', 'pi', opts, f.env)[0].canRestore, true);
  manage('restore', 'pi', opts, f.env);
  assert.deepEqual(readFileSync(target), original);
  assert.equal(manage('status', 'pi', opts, f.env)[0].canRestore, false);
  assert.equal(readFileSync(provider, 'utf8'), '{"synthetic":"unchanged"}');
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
});

test('pi first install restores absence; missing resource fails all before shell mutation', (t) => {
  const f = fixture(t);
  const target = join(f.home, '.pi/agent/extensions/pi-project-attribution.ts');
  manage('install', 'pi', {}, f.env);
  manage('install', 'pi', {}, f.env);
  manage('restore', 'pi', {}, f.env);
  assert.equal(existsSync(target), false);
  assert.equal(manage('restore', 'pi', {}, f.env)[0].status, 'unchanged');
  const standalone = join(f.root, 'client-attribution.mjs');
  writeFileSync(standalone, readFileSync(source));
  assert.throws(() => manage('install', 'all', { shell: 'bash' }, f.env, standalone), /缺少配套/);
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
  assert.equal(existsSync(target), false);
});

test('pi refuses symlink destinations and corrupt backup records before replacing data', (t) => {
  const f = fixture(t);
  const target = join(f.home, '.pi/agent/extensions/pi-project-attribution.ts');
  mkdirSync(join(f.home, '.pi/agent/extensions'), { recursive: true });
  const original = join(f.root, 'user-extension.ts');
  writeFileSync(original, '// unchanged');
  symlinkSync(original, target);
  assert.throws(() => manage('install', 'pi', {}, f.env), /不是普通文件/);
  assert.equal(readFileSync(original, 'utf8'), '// unchanged');
  rmSync(target);
  manage('install', 'pi', {}, f.env);
  const key = createHash('sha256').update(target).digest('hex');
  writeFileSync(join(f.env.XDG_DATA_HOME, 'sumpter/attribution', `pi-${key}.json`), '{broken');
  const installed = readFileSync(target);
  assert.throws(() => manage('restore', 'pi', {}, f.env), /不是有效 JSON/);
  assert.deepEqual(readFileSync(target), installed);
});
