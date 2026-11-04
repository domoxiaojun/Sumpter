import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, rmSync, writeFileSync, readFileSync, existsSync, realpathSync, symlinkSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { createHash } from 'node:crypto';
import { join } from 'node:path';
import { execFileSync, spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { collectHeaders, prepareLaunch, manage, sanitizeRemote, diagnoseClients } from '../clients/client-attribution.mjs';
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
    ANTHROPIC_BASE_URL: 'http://127.0.0.1:57878',
    GROK_CONFIG: JSON.stringify({ endpoints: { xai_api_base_url: 'http://127.0.0.1:57878/v1', cli_chat_proxy_base_url: 'http://127.0.0.1:57878' } }),
    SUMPTER_GEMINI_BASE_URL: 'http://127.0.0.1:57878', SUMPTER_AUTH_TOKEN: 'synthetic-token' };
  return { root, home, repo, nested, env };
}
const textHeaders = (text, sep) => Object.fromEntries(text.split(sep).filter(Boolean).map((line) => { const i=line.indexOf(':'); return [line.slice(0,i).trim(), line.slice(i+1).trim()]; }));

test('pi launcher loads the request hook and preserves arguments, environment and exit status', (t) => {
  const f = fixture(t);
  const args = ['--provider', 'sumpter', '--resume', 'session with spaces', '--no-extensions'];
  const launch = prepareLaunch('pi', args, f.env, f.nested);
  assert.equal(launch.command, 'pi');
  assert.deepEqual(launch.args.slice(2), args);
  assert.equal(launch.args[0], '-e');
  assert.equal(launch.args[1], fileURLToPath(new URL('../clients/pi-project-attribution.ts', import.meta.url)));
  assert.equal(launch.env.SUMPTER_PI_ATTRIBUTION, '1');
  assert.equal(launch.env.SUMPTER_PI_BIN, undefined);
  assert.equal(launch.env.USER, f.env.USER);
  // Use an executable shim to verify the actual child command and exit propagation.
  const shim = join(f.root, 'pi-shim');
  writeFileSync(shim, '#!/bin/sh\nprintf "%s\\n" "$@"\nexit 7\n', { mode: 0o755 });
  const child = spawnSync(process.execPath, [source, 'run', 'pi', '--', ...args], {
    env: { ...f.env, SUMPTER_PI_BIN: shim }, encoding: 'utf8',
  });
  assert.equal(child.status, 7, child.stderr);
  assert.deepEqual(child.stdout.trimEnd().split('\n'), launch.args);
});

test('pi package, config, auth and informational commands bypass attribution injection', (t) => {
  const f = fixture(t);
  for (const args of [
    ['install', 'npm:@czottmann/pi-automode'], ['remove', 'npm:@czottmann/pi-automode'],
    ['uninstall', 'npm:@czottmann/pi-automode'], ['update', '--extensions'], ['list'],
    ['config'], ['auth', 'print-api-key'], ['--help'], ['--version'],
  ]) {
    const launch = prepareLaunch('pi', args, { ...f.env, SUMPTER_PI_ATTRIBUTION: '1' }, f.nested);
    assert.equal(launch.command, 'pi');
    assert.deepEqual(launch.args, args);
    assert.equal(launch.env.SUMPTER_PI_ATTRIBUTION, undefined);
  }
});

for (const shell of ['bash', 'zsh']) {
test(`pi ${shell} wrapper forwards package commands without splitting them into prompts`, { skip: !existsSync(`/bin/${shell}`) }, (t) => {
  const f = fixture(t);
  const rc = join(f.home, `.${shell}rc`);
  manage('install', 'pi', { shell, rc }, f.env);
  const shim = join(f.root, 'pi-shim');
  writeFileSync(shim, '#!/bin/sh\nprintf "%s\\n" "$@"\n', { mode: 0o755 });
  const child = spawnSync(`/bin/${shell}`, ['-c', 'source "$1"; pi install "npm:@czottmann/pi-automode"', 'test', rc], {
    env: { ...f.env, SUMPTER_PI_BIN: shim }, encoding: 'utf8',
  });
  assert.equal(child.status, 0, child.stderr);
  assert.deepEqual(child.stdout.trimEnd().split('\n'), ['install', 'npm:@czottmann/pi-automode']);
});
}

test('pi install can add the same dynamic wrapper pattern as the other clients', (t) => {
  const f = fixture(t);
  const rc = join(f.home, '.zshrc');
  const result = manage('install', 'pi', { shell: 'zsh', rc }, f.env);
  assert.deepEqual(result.map((item) => item.client), ['pi']);
  assert.equal(result[0].shell, 'zsh');
  assert.equal(result[0].extension, undefined);
  assert.match(readFileSync(rc, 'utf8'), /pi\(\) \{ command node .* run pi --/u);
  assert.equal(existsSync(join(f.home, '.pi')), false);
  assert.match(readFileSync(join(f.home, '.local/share/sumpter/attribution/pi-project-attribution.ts'), 'utf8'), /before_provider_headers/u);
});

test('clients share Unicode project/root/user/sanitized remote and preserve unrelated settings', (t) => {
  const f = fixture(t);
  const base = collectHeaders('claude', f.nested, f.env);
  assert.equal(decodeURIComponent(base['X-Sumpter-Workspace']), f.repo);
  assert.equal(decodeURIComponent(base['X-Sumpter-Project']), "中文项目 ' space");
  assert.equal(decodeURIComponent(base['X-Sumpter-Git-Remote']), 'https://example.invalid/team/repo.git');
  const rawEnv = { ...f.env,
    ANTHROPIC_CUSTOM_HEADERS: 'X-Test: unchanged\nx-sumpter-project: stale\nX-Sumpter-Session-Id: stale\nX-Sumpter-Request-Kind: stale',
    GROK_CONFIG: JSON.stringify({ ...JSON.parse(f.env.GROK_CONFIG), other: 42, models: { temperature: 0.2, extra_headers: { 'X-Test': 'unchanged', 'x-sumpter-project': 'stale', 'X-Sumpter-Session-Id': 'stale', 'X-Sumpter-Agent-Role': 'stale' } } }),
    GEMINI_CLI_CUSTOM_HEADERS: 'X-Test: unchanged, x-sumpter-project: stale, X-Sumpter-Client-Variant: stale',
  };
  for (const client of ['claude', 'grok', 'gemini']) {
    const args = ['--model', 'test-model', 'prompt with spaces'];
    const launch = prepareLaunch(client, args, rawEnv, f.nested);
    const headers = client === 'claude' ? textHeaders(launch.env.ANTHROPIC_CUSTOM_HEADERS, '\n')
      : client === 'grok' ? JSON.parse(launch.env.GROK_CONFIG).models.extra_headers : textHeaders(launch.env.GEMINI_CLI_CUSTOM_HEADERS, ',');
    for (const [key,value] of Object.entries(base)) assert.equal(headers[key], value);
    assert.equal(headers['X-Test'], 'unchanged');
    assert.equal(headers['x-sumpter-project'], undefined);
    assert.equal(headers['X-Sumpter-Request-Kind'], undefined);
    assert.equal(headers['X-Sumpter-Agent-Role'], undefined);
    assert.equal(headers['X-Sumpter-Client-Variant'], undefined);
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
  assert.equal(prepareLaunch('grok',[],{...f.env,GROK_CONFIG:'{synthetic-secret'},f.repo).env.GROK_CONFIG, '{synthetic-secret');
  assert.equal(prepareLaunch('grok',[],{...f.env,GROK_CONFIG_PATH:'/private/config'},f.repo).attribution.enabled, false);
  mkdirSync(join(f.home,'.claude')); writeFileSync(join(f.home,'.claude/settings.json'),JSON.stringify({env:{ANTHROPIC_CUSTOM_HEADERS:'X: synthetic'}}));
  assert.throws(() => prepareLaunch('claude',[],f.env,f.repo), /覆盖动态归因/);
});

test('Gemini explicit/resume session arguments never acquire a new wrapper identity', (t) => {
  const f=fixture(t);
  for(const args of [['--resume','last'],['--resume=last'],['-r'],['--session-file=data'],['--list-sessions']]) {
    const launch=prepareLaunch('gemini',args,{...f.env,SUMPTER_GEMINI_SESSION_ID:'stale'},f.repo);
    assert.deepEqual(launch.args,args);
    assert.equal(/X-Sumpter-Session-Id:/i.test(launch.env.GEMINI_CLI_CUSTOM_HEADERS),false);
  }
});

test('Gemini preserves explicit IDs and complete UUID resume identity without changing arguments', (t) => {
  const f = fixture(t);
  const id = '12345678-1234-4234-8234-123456789abc';
  for (const args of [['--session-id', id], [`--session-id=${id}`], ['--resume', id], [`-r=${id}`]]) {
    const launch = prepareLaunch('gemini', args, { ...f.env, SUMPTER_GEMINI_SESSION_ID: 'stale' }, f.repo);
    assert.deepEqual(launch.args, args);
    assert.ok(launch.env.GEMINI_CLI_CUSTOM_HEADERS.includes(`X-Sumpter-Session-Id: ${id}`));
    assert.ok(!launch.env.GEMINI_CLI_CUSTOM_HEADERS.includes('stale'));
  }
});

test('Gemini respects attached resume, option values and end-of-options without changing the prompt', t => {
  const f = fixture(t);
  const uuid = '12345678-1234-1234-1234-123456789abc';
  for (const args of [['-r5'], ['-rlatest'], [`-r${uuid}`], [`-r=${uuid}`]]) {
    const launch = prepareLaunch('gemini', args, f.env, f.repo);
    assert.deepEqual(launch.args, args);
    const headers = textHeaders(launch.env.GEMINI_CLI_CUSTOM_HEADERS, ',');
    assert.equal(headers['X-Sumpter-Session-Id'], args[0].includes(uuid) ? uuid : undefined);
  }
  for (const args of [['--', '--resume', 'explain this'], ['--prompt', '--resume'], ['-p', '--session-id']]) {
    const launch = prepareLaunch('gemini', args, f.env, f.repo);
    const headers = textHeaders(launch.env.GEMINI_CLI_CUSTOM_HEADERS, ',');
    const index = launch.args.indexOf('--session-id');
    // The first occurrence can be a prompt value; locate the injected ID by value.
    const idIndex = launch.args.indexOf(headers['X-Sumpter-Session-Id']);
    assert.ok(index >= 0 && idIndex > 0);
    assert.equal(launch.args[idIndex - 1], '--session-id');
    const original = [...launch.args]; original.splice(idIndex - 1, 2);
    assert.deepEqual(original, args);
    if (args[0] === '--') assert.deepEqual(launch.args.slice(2), args);
  }
  assert.throws(() => prepareLaunch('gemini', [], { ...f.env, SUMPTER_GEMINI_SESSION_ID: 'invalid:session' }, f.repo), /标识/);
});

for (const shell of ['bash', 'zsh']) {
  test(`unified ${shell} legacy uninstall and successive restore cycles preserve the current configuration`, t => {
    const f = fixture(t);
    const opts = { shell, rc: join(f.home, `.${shell}rc`) };
    const legacy = value => `# >>> sumpter cc-project-attribution >>>\nsource /synthetic/${value}.sh\n# <<< sumpter cc-project-attribution <<<\n`;
    writeFileSync(opts.rc, '# keep\n' + legacy('old'));
    manage('uninstall', 'claude', { ...opts, dryRun: true }, f.env);
    assert.equal(manage('status', 'claude', opts, f.env)[0].status, 'legacy');
    manage('uninstall', 'claude', opts, f.env);
    assert.equal(readFileSync(opts.rc, 'utf8'), '# keep\n');
    assert.equal(manage('status', 'claude', opts, f.env)[0].status, 'absent');
    for (const version of ['old', 'new']) {
      writeFileSync(opts.rc, '# keep\n' + legacy(version));
      manage('install', 'claude', opts, f.env);
      manage('install', 'claude', opts, f.env);
      writeFileSync(opts.rc, readFileSync(opts.rc, 'utf8') + '# later edit\n');
      manage('restore', 'claude', opts, f.env);
      assert.equal(readFileSync(opts.rc, 'utf8'), '# keep\n# later edit\n' + legacy(version));
      assert.equal(manage('status', 'claude', opts, f.env)[0].canRestore, false);
    }
  });
}

test('updating any client keeps an installed Pi companion paired and rejects incomplete bundles before writes', t => {
  const f = fixture(t);
  const old = join(f.root, 'old'); mkdirSync(old);
  const extension = readFileSync(new URL('../clients/pi-project-attribution.ts', import.meta.url), 'utf8');
  writeFileSync(join(old, 'client-attribution.mjs'), readFileSync(source, 'utf8').replaceAll('2.0.0', '1.9.0'));
  writeFileSync(join(old, 'pi-project-attribution.ts'), extension.replaceAll('2.0.0', '1.9.0'));
  manage('install', 'pi', {}, f.env, join(old, 'client-attribution.mjs'));
  manage('install', 'claude', {}, f.env);
  const dir = join(f.env.XDG_DATA_HOME, 'sumpter/attribution');
  const installed = join(dir, 'client-attribution.mjs');
  assert.equal(readFileSync(join(dir, 'pi-project-attribution.ts'), 'utf8'), extension);
  assert.equal(manage('status', 'pi', {}, f.env)[0].status, 'installed');
  const before = readFileSync(installed);
  const rcBefore = readFileSync(join(f.home, '.bashrc'));
  writeFileSync(join(old, 'pi-project-attribution.ts'), extension); // wrong release for old launcher
  assert.throws(() => manage('install', 'claude', {}, f.env, join(old, 'client-attribution.mjs')), /版本不匹配/);
  rmSync(join(old, 'pi-project-attribution.ts'));
  assert.throws(() => manage('install', 'claude', {}, f.env, join(old, 'client-attribution.mjs')), /缺少配套/);
  assert.deepEqual(readFileSync(installed), before);
  assert.deepEqual(readFileSync(join(f.home, '.bashrc')), rcBefore);
  writeFileSync(join(dir, 'pi-project-attribution.ts'), extension.replaceAll('2.0.0', '1.9.0'));
  const shim = join(f.root, 'pi-shim');
  writeFileSync(shim, '#!/bin/sh\necho CLIENT_STARTED\n', { mode: 0o700 });
  const result = spawnSync(process.execPath, [installed, 'run', 'pi', '--', 'hello'], { env: { ...f.env, SUMPTER_PI_BIN: shim }, encoding: 'utf8' });
  assert.equal(result.status, 2);
  assert.match(result.stderr, /版本不匹配/);
  assert.doesNotMatch(result.stdout, /CLIENT_STARTED/);
});

for (const platform of ['linux', 'macos']) {
  for (const client of ['cc', 'grok']) {
    test(`legacy ${platform}/${client} sanitizes remotes before passing headers to the client`, t => {
      const f = fixture(t);
      const installer = fileURLToPath(new URL(`../../platforms/${platform}/scripts/${client}-project-attribution.sh`, import.meta.url));
      const rc = join(f.home, '.bashrc');
      execFileSync('/bin/bash', [installer, 'install', '--shell', 'bash', '--rc', rc], { env: f.env });
      const command = client === 'cc' ? 'claude' : 'grok';
      writeFileSync(join(f.root, command), `#!/bin/sh\nprintf '%s' "$${client === 'cc' ? 'ANTHROPIC_CUSTOM_HEADERS' : 'GROK_CONFIG'}"\n`, { mode: 0o700 });
      for (const [remote, expected] of [
        ['https://user:synthetic@example.invalid/team/repo.git?token=synthetic#private', 'https://example.invalid/team/repo.git'],
        ['ssh://user:synthetic@example.invalid:2222/team/repo.git#private', 'ssh://example.invalid:2222/team/repo.git'],
        ['git@example.invalid:team/repo.git?token=synthetic', 'ssh://example.invalid/team/repo.git'],
        ['file:///private/repo', undefined],
      ]) {
        execFileSync('git', ['-C', f.repo, 'config', 'remote.origin.url', remote]);
        for (const shell of ['/bin/bash', '/bin/zsh'].filter(existsSync)) {
          const output = execFileSync(shell, ['-c', `. "$1"; ${command}`, 'remote-test', rc], { env: { ...f.env, PATH: `${f.root}:${f.env.PATH}` }, cwd: f.repo, encoding: 'utf8' });
          const headers = client === 'cc' ? textHeaders(output, '\n') : JSON.parse(output).models.extra_headers;
          assert.equal(headers['X-Sumpter-Git-Remote'], expected);
          assert.doesNotMatch(output, /synthetic|token=|private\/repo/);
        }
      }
    });
  }
}

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
  assert.deepEqual(manage('status','all',opts,f.env).map(x=>x.status),['installed','installed','installed','installed','installed']);
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
  assert.deepEqual(result.map(x => x.result), ['restored', 'unchanged', 'unchanged', 'unchanged', 'unchanged']);
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
  let extensionMismatch = false;
  const resources = {
    '/__sumpter/client-attribution.mjs': source,
    '/__sumpter/pi-project-attribution.ts': fileURLToPath(new URL('../clients/pi-project-attribution.ts', import.meta.url)),
  };
  const server = createServer((req, res) => {
    if (!resources[req.url] || req.headers.authorization !== 'Bearer synthetic-token') {
      res.writeHead(401).end(); return;
    }
    if (extensionMissing && req.url.endsWith('.ts')) { res.writeHead(404).end(); return; }
    let body = readFileSync(resources[req.url]);
    if (extensionMismatch && req.url.endsWith('.ts')) body = Buffer.from(body.toString().replace(/SUMPTER_ATTRIBUTION_BUNDLE_VERSION: \d+\.\d+\.\d+/, 'SUMPTER_ATTRIBUTION_BUNDLE_VERSION: 0.0.0'));
    res.end(body);
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
  extensionMismatch = true;
  assert.notEqual((await run('synthetic-token')).code, 0);
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
  extensionMismatch = false;
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
  const uninstalled = await run('synthetic-token', 'uninstall');
  assert.equal(uninstalled.code, 0, uninstalled.output);
  assert.match(uninstalled.output, /pi：未安装/);
});

test('remote Linux setup downloads from SUMPTER_RESOURCE_BASE without listener auth', async (t) => {
  const { createServer } = await import('node:http');
  const { spawn } = await import('node:child_process');
  const f = fixture(t);
  const script = join(f.root, 'setup-client-attribution.sh');
  writeFileSync(script, readFileSync(new URL('../../platforms/linux/scripts/setup-client-attribution.sh', import.meta.url)));
  const resources = {
    '/client-attribution.mjs': source,
    '/pi-project-attribution.ts': fileURLToPath(new URL('../clients/pi-project-attribution.ts', import.meta.url)),
  };
  const server = createServer((req, res) => {
    if (!resources[req.url]) { res.writeHead(404).end(); return; }
    res.end(readFileSync(resources[req.url]));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  t.after(() => server.close());
  const run = client => new Promise((resolve, reject) => {
    const child = spawn('/bin/bash', [script, 'install', client, '--shell', 'bash'], {
      env: { ...f.env, SUMPTER_RESOURCE_BASE: `http://127.0.0.1:${server.address().port}` },
    });
    let output = '';
    child.stdout.on('data', data => { output += data; });
    child.stderr.on('data', data => { output += data; });
    child.on('error', reject);
    child.on('close', code => resolve({ code, output }));
  });
  const result = await run('all');
  assert.equal(result.code, 0, result.output);
  assert.match(result.output, /claude：已安装/);
  assert.match(result.output, /pi：已安装/);
  const privatePi = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/pi-project-attribution.ts');
  const extension = readFileSync(resources['/pi-project-attribution.ts'], 'utf8');
  writeFileSync(privatePi, extension.replaceAll('2.0.0', '1.9.0'));
  const update = await run('claude');
  assert.equal(update.code, 0, update.output);
  assert.equal(readFileSync(privatePi, 'utf8'), extension);
});

function legacyPiFixture(t, original = null) {
  const f = fixture(t);
  const target = join(f.home, '.pi/agent/extensions/pi-project-attribution.ts');
  const dir = join(f.env.XDG_DATA_HOME, 'sumpter/attribution');
  mkdirSync(join(f.home, '.pi/agent/extensions'), { recursive: true });
  mkdirSync(dir, { recursive: true });
  const shipped = readFileSync(new URL('../clients/pi-project-attribution.ts', import.meta.url));
  writeFileSync(target, shipped);
  const key = createHash('sha256').update(target).digest('hex');
  const state = join(dir, `pi-${key}.json`);
  writeFileSync(state, JSON.stringify({ version: 1, target, original }));
  return { ...f, target, state, shipped, dir };
}

for (const original of [null, { bytes: Buffer.from([0, 255, 10, 65]).toString('base64'), mode: 0o640 }]) {
  for (const client of ['pi', 'all']) {
    test(`migrate owned global extension to private wrapper (${client}, original=${Boolean(original)})`, (t) => {
      const f = legacyPiFixture(t, original);
      const opts = { shell: 'zsh' };
      const provider = join(f.home, '.pi/agent/models.json');
      writeFileSync(provider, '{"keep":true}');
      assert.equal(manage('status', client, opts, f.env).find(x => x.client === 'pi').status, 'legacy');
      manage('install', client, { ...opts, dryRun: true }, f.env);
      assert.deepEqual(readFileSync(f.target), f.shipped);
      assert.equal(existsSync(join(f.home, '.zshrc')), false);
      manage('install', client, opts, f.env);
      if (original) {
        assert.deepEqual(readFileSync(f.target), Buffer.from(original.bytes, 'base64'));
        assert.equal(statSync(f.target).mode & 0o777, original.mode);
      } else assert.equal(existsSync(f.target), false);
      assert.equal(existsSync(f.state), false);
      assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'installed');
      manage('install', 'pi', opts, f.env);
      manage('restore', 'pi', opts, f.env);
      assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'absent');
      assert.equal(readFileSync(provider, 'utf8'), '{"keep":true}');
      if (original) assert.deepEqual(readFileSync(f.target), Buffer.from(original.bytes, 'base64'));
      else assert.equal(existsSync(f.target), false);
    });
  }
}

test('migration recognizes previous private version before replacing it', (t) => {
  const f = legacyPiFixture(t);
  writeFileSync(f.target, '// previous shipped version');
  writeFileSync(join(f.dir, 'pi-project-attribution.ts'), '// previous shipped version');
  manage('install', 'pi', {}, f.env);
  assert.equal(existsSync(f.target), false);
  assert.deepEqual(readFileSync(join(f.dir, 'pi-project-attribution.ts')), f.shipped);
});

for (const kind of ['modified', 'symlink', 'removed', 'untracked']) {
  test(`private install preserves ${kind} global extension`, (t) => {
    const f = legacyPiFixture(t, kind === 'removed' ? { bytes: Buffer.from('// original').toString('base64'), mode: 0o600 } : null);
    if (kind === 'modified') writeFileSync(f.target, '// user edit');
    if (kind === 'symlink') {
      rmSync(f.target);
      writeFileSync(join(f.root, 'user.ts'), '// user symlink');
      symlinkSync(join(f.root, 'user.ts'), f.target);
    }
    if (kind === 'removed') rmSync(f.target);
    if (kind === 'untracked') rmSync(f.state);
    const before = existsSync(f.target) ? readFileSync(f.target) : null;
    manage('install', 'pi', {}, f.env);
    const status = manage('status', 'pi', {}, f.env)[0];
    assert.equal(status.status, 'installed');
    if (kind !== 'untracked') assert.match(status.note, /保留/);
    manage('restore', 'pi', {}, f.env);
    if (before) assert.deepEqual(readFileSync(f.target), before);
    else assert.equal(existsSync(f.target), false);
    assert.equal(existsSync(f.state), kind !== 'untracked');
  });
}

test('pi first install never creates global directory and missing resource fails before shell mutation', (t) => {
  const f = fixture(t);
  manage('install', 'pi', {}, f.env);
  manage('install', 'pi', {}, f.env);
  assert.equal(existsSync(join(f.home, '.pi')), false);
  manage('restore', 'pi', {}, f.env);
  assert.equal(readFileSync(join(f.home, '.bashrc'), 'utf8'), '');
  assert.equal(manage('restore', 'pi', {}, f.env)[0].result, 'unchanged');
  const standalone = join(f.root, 'client-attribution.mjs');
  writeFileSync(standalone, readFileSync(source));
  assert.throws(() => manage('install', 'all', { shell: 'bash' }, f.env, standalone), /缺少配套/);
  assert.equal(readFileSync(join(f.home, '.bashrc'), 'utf8'), '');
  assert.equal(existsSync(join(f.home, '.pi')), false);
});

test('corrupt old backup blocks migration before shell or global writes', (t) => {
  const f = legacyPiFixture(t);
  writeFileSync(f.state, '{broken');
  assert.throws(() => manage('install', 'pi', {}, f.env), /不是有效 JSON/);
  assert.equal(existsSync(join(f.home, '.bashrc')), false);
  assert.deepEqual(readFileSync(f.target), f.shipped);
});

for (const action of ['restore', 'uninstall']) {
  test(`${action} also retires recorded global-only legacy install`, (t) => {
    const f = legacyPiFixture(t);
    manage(action, 'pi', {}, f.env);
    assert.equal(existsSync(f.target), false);
    assert.equal(existsSync(f.state), false);
  });
}

for (const platform of ['linux', 'macos']) {
  for (const client of ['cc', 'grok']) {
    test(`legacy ${platform}/${client} treats shell metacharacters in paths as data`, (t) => {
      const f = fixture(t);
      const installer = fileURLToPath(new URL(`../../platforms/${platform}/scripts/${client}-project-attribution.sh`, import.meta.url));
      const rc = join(f.home, 'new " \' $(touch rc-injected) `touch rc-backtick`', 'profile');
      const snippet = join(f.home, 'snippet " \' $(touch snippet-injected) `touch snippet-backtick`\nfile.sh');
      const env = { ...f.env, [client === 'cc' ? 'SUMPTER_CC_SNIPPET' : 'SUMPTER_GROK_SNIPPET']: snippet };
      const options = { env, cwd: f.root, encoding: 'utf8' };
      const run = (action, ...args) => execFileSync('/bin/bash', [installer, action, '--shell', 'bash', '--rc', rc, ...args], options);
      run('install', '--dry-run');
      assert.equal(existsSync(rc), false);
      assert.equal(existsSync(snippet), false);
      run('install'); // exercise mkdir with an untrusted path
      const installed = readFileSync(rc, 'utf8');
      run('install'); // exercise backup with an untrusted path
      assert.equal(readFileSync(rc, 'utf8'), installed);
      for (const shell of ['/bin/bash', '/bin/zsh'].filter(existsSync)) {
        const command = client === 'cc' ? 'claude' : 'grok';
        const result = execFileSync(shell, ['-c', `. "$1"; typeset -f ${command}`, 'source-test', rc], options);
        assert.match(result, new RegExp(`${command}\\s*\\(\\)`));
        if (client === 'grok') {
          const grok = join(f.root, 'grok');
          writeFileSync(grok, '#!/bin/sh\nprintf "%s" "$GROK_CONFIG"\n', { mode: 0o755 });
          const config = execFileSync(shell, ['-c', '. "$1"; grok', 'legacy-grok-test', rc], {
            ...options,
            env: {
              ...env,
              PATH: `${f.root}:${env.PATH}`,
              GROK_CONFIG: JSON.stringify({ models: { extra_headers: {
                'X-Test': 'keep', 'X-Sumpter-Request-Kind': 'stale',
              } } }),
            },
          });
          const headers = JSON.parse(config).models.extra_headers;
          assert.equal(headers['X-Test'], 'keep');
          assert.equal(headers['X-Sumpter-Request-Kind'], undefined);
        }
      }
      run('uninstall', '--dry-run');
      assert.equal(readFileSync(rc, 'utf8'), installed);
      assert.equal(existsSync(snippet), true);
      run('uninstall'); // exercise rm with an untrusted path
      assert.equal(existsSync(snippet), false);
      assert.equal(readFileSync(rc, 'utf8'), '');
      for (const marker of ['rc-injected', 'rc-backtick', 'snippet-injected', 'snippet-backtick']) {
        assert.equal(existsSync(join(f.root, marker)), false, `executed filename: ${marker}`);
      }
    });
  }
}

for (const shell of ['bash', 'zsh']) {
  test(`pi and all share one complete ${shell} status and preserve user edits on restore`, { skip: !existsSync(`/bin/${shell}`) }, (t) => {
    const f = fixture(t);
    const opts = { shell };
    const rc = join(f.home, shell === 'zsh' ? '.zshrc' : '.bashrc');
    const provider = join(f.home, '.pi/agent/models.json');
    mkdirSync(join(f.home, '.pi/agent'), { recursive: true });
    writeFileSync(provider, '{"keep":true}');
    writeFileSync(rc, '# original\n');
    const initial = manage('status', 'all', opts, f.env);
    assert.equal(initial.length, 5);
    assert.equal(new Set(initial.map(item => item.client)).size, 5);
    manage('install', 'all', opts, f.env);
    const status = manage('status', 'pi', opts, f.env);
    assert.equal(status.length, 1);
    assert.equal(status[0].shell, shell);
    assert.equal(status[0].rc, rc);
    assert.equal(status[0].status, 'installed');
    assert.deepEqual(manage('status', 'all', opts, f.env).filter(item => item.client === 'pi'), status);
    assert.match(readFileSync(rc, 'utf8'), /pi\(\) \{ command node .* run pi --/u);
    const shim = join(f.root, 'pi-shim');
    writeFileSync(shim, '#!/bin/sh\nprintf "%s\\n" "$SUMPTER_PI_ATTRIBUTION" "$@"\nexit 7\n', { mode: 0o755 });
    const child = spawnSync(`/bin/${shell}`, ['-c', 'source "$1"; pi --resume "session with spaces"', 'test', rc], {
      env: { ...f.env, SUMPTER_PI_BIN: shim }, encoding: 'utf8',
    });
    assert.equal(child.status, 7, child.stderr);
    assert.deepEqual(child.stdout.trimEnd().split('\n'), ['1', '-e', realpathSync(join(f.env.XDG_DATA_HOME, 'sumpter/attribution/pi-project-attribution.ts')), '--resume', 'session with spaces']);
    const resource = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/pi-project-attribution.ts');
    rmSync(resource);
    assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'broken');
    manage('install', 'pi', opts, f.env);
    writeFileSync(rc, readFileSync(rc, 'utf8') + '# later edit\n');
    manage('restore', 'all', opts, f.env);
    assert.equal(readFileSync(rc, 'utf8'), '# original\n# later edit\n');
    assert.equal(readFileSync(provider, 'utf8'), '{"keep":true}');
    assert.equal(manage('status', 'pi', opts, f.env)[0].status, 'absent');
    assert.equal(manage('status', 'pi', opts, f.env)[0].canRestore, false);
    assert.equal(manage('restore', 'pi', opts, f.env)[0].result, 'unchanged');
  });
}

test('pi rejects unsupported shells and broken markers before installing extension', (t) => {
  const f = fixture(t);
  assert.throws(() => manage('install', 'pi', { shell: 'fish' }, f.env), /bash\/zsh/);
  assert.equal(existsSync(join(f.home, '.pi')), false);
  writeFileSync(join(f.home, '.bashrc'), '# >>> sumpter client-attribution pi >>>\n');
  assert.throws(() => manage('install', 'pi', {}, f.env), /不完整/);
  assert.equal(existsSync(join(f.home, '.pi')), false);
});

test('doctor is read-only, reports explicit provider opt-in and never exposes URL credentials', t => {
  const { home, env } = fixture(t);
  mkdirSync(join(home, '.pi', 'agent'), { recursive: true });
  mkdirSync(join(home, '.codex'), { recursive: true });
  const models = JSON.stringify({ providers: { sumpter: { baseUrl: 'http://user:synthetic-secret@127.0.0.1:57878/v1?token=private', headers: { 'x-sumpter-client': 'pi', Authorization: 'secret-value' } }, another: { baseUrl: 'http://localhost:9999/v1' } } });
  writeFileSync(join(home, '.pi', 'agent', 'models.json'), models);
  writeFileSync(join(home, '.codex', 'config.toml'), 'model_provider = "custom"\n[model_providers.custom]\nbase_url = "http://127.0.0.1:57878/v1"\napi_key = "secret-value"\n');
  const results = diagnoseClients('all', {}, { ...env, ANTHROPIC_BASE_URL: 'https://user:secret@remote.invalid/private-token-path?token=secret' });
  const pi = results.find(r => r.client === 'pi');
  assert.equal(pi.sources[0].explicitOptIn, true);
  assert.equal(pi.sources[1].explicitOptIn, false);
  assert.equal(pi.sources[0].matchesProxyOrigin, true);
  assert.equal(results.find(r => r.client === 'codex').sources[0].baseURL, 'http://127.0.0.1:57878/v1');
  assert.ok(results.every(r => r.traffic === 'unknown'));
  assert.doesNotMatch(JSON.stringify(results), /synthetic-secret|secret-value|private-token-path|token=|user:/);
  assert.equal(readFileSync(join(home, '.pi', 'agent', 'models.json'), 'utf8'), models);
  assert.equal(existsSync(join(home, '.local', 'share', 'sumpter')), false);
});

test('doctor keeps unreadable or malformed configuration unknown without exposing its text', t => {
  const { home, env } = fixture(t);
  mkdirSync(join(home, '.pi', 'agent'), { recursive: true });
  writeFileSync(join(home, '.pi', 'agent', 'models.json'), '{"apiKey":"secret-value" invalid');
  const [result] = diagnoseClients('pi', {}, env);
  assert.equal(result.configuration, 'unknown');
  assert.ok(result.issues.length > 0);
  assert.doesNotMatch(JSON.stringify(result), /secret-value/);
});

test('v2 enable, disable, update and re-enable retain source identity and independent switches', t => {
  const f = fixture(t);
  const enabled = manage('enable', 'all', {}, f.env);
  const sourceId = enabled[0].sourceId;
  assert.ok(sourceId);
  assert.equal(enabled[0].availableVersion, enabled[0].installedVersion);
  assert.equal(manage('disable', 'claude', {}, f.env)[0].enabled, false);
  assert.equal(manage('status', 'gemini', {}, f.env)[0].enabled, true);
  const updated = manage('install', 'claude', {}, f.env)[0];
  assert.equal(updated.status, 'disabled');
  assert.equal(updated.sourceId, sourceId);
  const launch = prepareLaunch('claude', [], f.env, f.repo);
  assert.equal(launch.attribution.enabled, false);
  assert.equal(launch.env.ANTHROPIC_CUSTOM_HEADERS, '');
  assert.equal(manage('enable', 'claude', {}, f.env)[0].enabled, true);
  assert.equal(prepareLaunch('claude', [], f.env, f.repo).attribution.sourceId, sourceId);
});

test('already loaded modern shell wrapper obeys disable on its next invocation', t => {
  const f = fixture(t);
  const rc = join(f.home, '.bashrc');
  manage('enable', 'claude', { rc }, f.env);
  const shim = join(f.root, 'claude-shim');
  writeFileSync(shim, '#!/bin/sh\nprintf "%s\\n" "$ANTHROPIC_CUSTOM_HEADERS"\n', { mode: 0o700 });
  const result = spawnSync('/bin/bash', ['-c', 'source "$1"; command node "$2" disable claude --shell bash --rc "$1" >/dev/null; claude', 'test', rc, source], {
    env: { ...f.env, SUMPTER_CLAUDE_BIN: shim }, cwd: f.repo, encoding: 'utf8',
  });
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), '');
  const status = manage('status', 'claude', { rc }, f.env)[0];
  assert.equal(status.lastLaunch.enabled, false);
  assert.equal(status.lastLaunch.collectorVersion, status.installedVersion);
});

test('corrupt restore records and policy fail closed but cannot block disable or uninstall', t => {
  const f = fixture(t);
  const rc = join(f.home, '.bashrc');
  manage('enable', 'claude', { rc }, f.env);
  const dir = join(f.env.XDG_DATA_HOME, 'sumpter/attribution');
  writeFileSync(join(dir, `claude-${Buffer.from(rc).toString('base64url')}.json`), '{bad');
  writeFileSync(join(dir, 'collection.json'), '{bad');
  const status = manage('status', 'claude', { rc }, f.env)[0];
  assert.equal(status.backupState, 'invalid');
  assert.equal(status.policyState, 'invalid');
  assert.equal(prepareLaunch('claude', [], f.env, f.repo).attribution.enabled, false);
  assert.equal(manage('disable', 'claude', { rc }, f.env)[0].enabled, false);
  assert.equal(manage('uninstall', 'claude', { rc }, f.env)[0].status, 'absent');
});

test('unbound and mismatched connections skip metadata without changing client arguments', t => {
  const f = fixture(t);
  for (const env of [{ ...f.env, ANTHROPIC_BASE_URL: 'https://external.invalid' }, { ...f.env, ANTHROPIC_BASE_URL: '' }]) {
    const launch = prepareLaunch('claude', ['hello'], env, f.repo);
    assert.equal(launch.attribution.enabled, false);
    assert.equal(launch.env.ANTHROPIC_CUSTOM_HEADERS, '');
    assert.deepEqual(launch.args, ['hello']);
  }
  mkdirSync(join(f.repo, '.claude'));
  writeFileSync(join(f.repo, '.claude/settings.local.json'), JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'https://external.invalid' } }));
  assert.equal(prepareLaunch('claude', [], f.env, f.repo).attribution.binding, 'mismatch');
  assert.equal(prepareLaunch('grok', [], { ...f.env, GROK_CONFIG: JSON.stringify({ endpoints: { xai_api_base_url: 'http://127.0.0.1:57878', cli_chat_proxy_base_url: 'https://external.invalid' } }) }, f.repo).attribution.binding, 'mismatch');
});

test('Gemini disable preserves connection and authentication but leaves session handling native', t => {
  const f = fixture(t);
  manage('enable', 'gemini', {}, f.env);
  const enabled = prepareLaunch('gemini', ['hello'], f.env, f.repo);
  manage('disable', 'gemini', {}, f.env);
  const disabled = prepareLaunch('gemini', ['hello'], f.env, f.repo);
  for (const key of ['GOOGLE_GEMINI_BASE_URL', 'GOOGLE_API_KEY', 'GEMINI_API_KEY']) assert.equal(disabled.env[key], enabled.env[key]);
  assert.deepEqual(disabled.args, ['hello']);
  assert.equal(disabled.env.GEMINI_CLI_CUSTOM_HEADERS, '');
  assert.equal(prepareLaunch('gemini', [], { ...f.env, SUMPTER_AUTH_TOKEN: '' }, f.repo).attribution.enabled, false);
});

test('restore preview explicitly identifies legacy target while disable retains the backup', t => {
  const f = fixture(t);
  const rc = join(f.home, '.bashrc');
  const legacy = '# >>> sumpter cc-project-attribution >>>\nclaude() { echo legacy; }\n# <<< sumpter cc-project-attribution <<<\n';
  writeFileSync(rc, legacy);
  manage('enable', 'claude', { rc }, f.env);
  const status = manage('disable', 'claude', { rc }, f.env)[0];
  assert.equal(status.restoreTarget, 'legacy');
  assert.equal(status.canRestore, true);
  assert.equal(manage('restore', 'claude', { rc }, f.env)[0].registration, 'legacy');
  assert.equal(readFileSync(rc, 'utf8'), legacy);
});

test('companion-only updates change the pair revision and keep active bundles available', t => {
  const f = fixture(t);
  const before = manage('enable', 'claude', {}, f.env)[0];
  const installed = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/client-attribution.mjs');
  const oldPath = realpathSync(installed);
  const updated = join(f.root, 'updated'); mkdirSync(updated);
  writeFileSync(join(updated, 'client-attribution.mjs'), readFileSync(source));
  writeFileSync(join(updated, 'pi-project-attribution.ts'), readFileSync(new URL('../clients/pi-project-attribution.ts', import.meta.url), 'utf8') + '\n// synthetic update\n');
  const after = manage('install', 'claude', {}, f.env, join(updated, 'client-attribution.mjs'))[0];
  assert.notEqual(before.installedVersion, after.installedVersion);
  assert.notEqual(realpathSync(installed), oldPath);
  assert.ok(existsSync(oldPath));
  assert.equal(before.sourceId, after.sourceId);
});

test('Claude skips alternate providers and project-only or disagreeing connection evidence', t => {
  const f = fixture(t);
  for (const key of ['CLAUDE_CODE_USE_BEDROCK', 'CLAUDE_CODE_USE_VERTEX', 'CLAUDE_CODE_USE_FOUNDRY', 'CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST', 'CLAUDE_CODE_REMOTE']) {
    assert.equal(prepareLaunch('claude', [], { ...f.env, [key]: '1' }, f.repo).attribution.enabled, false);
  }
  mkdirSync(join(f.repo, '.claude'));
  writeFileSync(join(f.repo, '.claude/settings.json'), JSON.stringify({ env: { ANTHROPIC_BASE_URL: 'http://127.0.0.1:57878' } }));
  assert.equal(prepareLaunch('claude', [], { ...f.env, ANTHROPIC_BASE_URL: '' }, f.repo).attribution.binding, 'unknown');
  assert.equal(prepareLaunch('claude', [], { ...f.env, ANTHROPIC_BASE_URL: 'https://external.invalid' }, f.repo).attribution.binding, 'mismatch');
});

test('missing and malformed launch receipts remain unknown without blocking status', t => {
  const f = fixture(t);
  assert.equal(manage('status', 'claude', {}, f.env)[0].lastLaunch, null);
  const status = manage('enable', 'claude', {}, f.env)[0];
  const path = join(f.env.XDG_DATA_HOME, 'sumpter/attribution/launch-claude.json');
  writeFileSync(path, JSON.stringify({ sourceId: status.sourceId, enabled: true }));
  assert.equal(manage('status', 'claude', {}, f.env)[0].lastLaunch, null);
});
