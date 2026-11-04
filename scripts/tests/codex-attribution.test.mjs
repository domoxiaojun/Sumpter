import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync, spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { prepareLaunch, collectHeaders, manage } from '../clients/client-attribution.mjs';

function fixture(t) {
  const home = mkdtempSync(join(tmpdir(), 'sumpter-codex-'));
  t.after(() => rmSync(home, { recursive: true, force: true }));
  const repo = join(home, '中文 project');
  const codexHome = join(home, '.codex');
  mkdirSync(repo); mkdirSync(codexHome);
  execFileSync('git', ['init', '-q', repo]);
  execFileSync('git', ['-C', repo, 'remote', 'add', 'origin', 'https://user:synthetic@example.invalid/team/project.git?token=synthetic']);
  const config = join(codexHome, 'config.toml');
  writeFileSync(config, 'model_provider="cpa"\n[model_providers.cpa]\nname="CPA"\nbase_url="http://127.0.0.1:57878/v1"\n[model_providers.work]\nbase_url="http://127.0.0.1:57878/v1"\n[model_providers.override]\nbase_url="http://127.0.0.1:57878/v1"\n[profiles.work]\nmodel_provider="work"\n');
  return { home, repo, config, env: { HOME: home, CODEX_HOME: codexHome, PATH: process.env.PATH, USER: 'tester', SHELL: '/bin/zsh' } };
}

test('Codex shares Claude project collection and honors cwd, profile and CLI selectors without changing config', (t) => {
  const f = fixture(t);
  const original = readFileSync(f.config, 'utf8');
  for (const [extra, provider] of [[[], 'cpa'], [['-p', 'work'], 'work'], [['--profile=work', '-c', 'model_provider="override"'], 'override']]) {
    const args = [...extra, '-C', f.repo, 'exec', 'hello world'];
    const launch = prepareLaunch('codex', args, { ...f.env, SUMPTER_CODEX_X_SUMPTER_SESSION_ID: 'stale' }, f.home);
    assert.deepEqual(launch.args.slice(0, args.length), args);
    for (const [name, value] of Object.entries(collectHeaders('claude', f.repo, f.env))) {
      const variable = `SUMPTER_CODEX_${name.replaceAll('-', '_').toUpperCase()}`;
      assert.equal(launch.env[variable], value);
      assert.ok(launch.args.includes(`model_providers.${provider}.env_http_headers.${name}="${variable}"`));
    }
    assert.equal(launch.env.SUMPTER_CODEX_X_SUMPTER_SESSION_ID, undefined);
    assert.equal(launch.env.OPENAI_CUSTOM_HEADERS, undefined);
    assert.ok(!launch.args.includes(`model_provider="${provider}"`) || extra.length > 0);
  }
  assert.equal(readFileSync(f.config, 'utf8'), original);
  const resumed = prepareLaunch('codex', ['resume', 'existing-session'], f.env, f.repo);
  assert.deepEqual(resumed.args.slice(0, 2), ['resume', 'existing-session']);
  const outside = prepareLaunch('codex', [], f.env, f.home);
  assert.equal(outside.env.SUMPTER_CODEX_X_SUMPTER_GIT_REMOTE, undefined);
  assert.equal(decodeURIComponent(outside.env.SUMPTER_CODEX_X_SUMPTER_WORKSPACE), f.home);
});

test('Codex install/update/restore keeps user edits and shell launch sees the actual directory', (t) => {
  const f = fixture(t);
  const shim = join(f.home, 'codex-shim');
  writeFileSync(shim, `#!${process.execPath}\nconsole.log(JSON.stringify({args:process.argv.slice(2),workspace:process.env.SUMPTER_CODEX_X_SUMPTER_WORKSPACE}));\n`, { mode: 0o700 });
  for (const shell of ['bash', 'zsh'].filter((name) => existsSync(`/bin/${name}`))) {
    const rc = join(f.home, `.${shell}rc`);
    const options = { shell, rc };
    writeFileSync(rc, '# existing\n');
    manage('enable', 'codex', options, f.env);
    manage('enable', 'codex', options, f.env);
    assert.equal(manage('status', 'codex', options, f.env)[0].status, 'installed');
    const output = execFileSync(`/bin/${shell}`, ['-c', 'source "$1"; codex exec "hello world"', 'test', rc], {
      env: { ...f.env, SUMPTER_CODEX_BIN: shim }, cwd: f.repo, encoding: 'utf8',
    });
    const result = JSON.parse(output);
    assert.equal(decodeURIComponent(result.workspace), execFileSync('git', ['-C', f.repo, 'rev-parse', '--show-toplevel'], { encoding: 'utf8' }).trim());
    assert.deepEqual(result.args.slice(0, 2), ['exec', 'hello world']);
    writeFileSync(rc, readFileSync(rc, 'utf8') + '# later\n');
    manage('restore', 'codex', options, f.env);
    assert.equal(readFileSync(rc, 'utf8'), '# existing\n# later\n');
  }
});

test('real Codex sends project headers to a loopback Responses endpoint', { skip: !process.env.CODEX_ATTRIBUTION_TEST_BIN, timeout: 45000 }, async (t) => {
  const f = fixture(t);
  let captured;
  const server = createServer((req, res) => {
    req.resume();
    req.on('end', () => {
      if (!req.url.endsWith('/responses')) { res.writeHead(404).end(); return; }
      captured = req.headers;
      res.writeHead(200, { 'Content-Type': 'text/event-stream' });
      res.end('event: response.completed\ndata: {"type":"response.completed","response":{"id":"resp_test","status":"completed","output":[],"usage":{"input_tokens":1,"output_tokens":0,"total_tokens":1}}}\n\n');
    });
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(() => { server.closeAllConnections(); server.close(); });
  writeFileSync(f.config, `model_provider="cpa"\n[model_providers.cpa]\nname="CPA"\nbase_url="http://127.0.0.1:${server.address().port}/v1"\nwire_api="responses"\nrequires_openai_auth=false\nhttp_headers={"X-Test"="keep", "Authorization"="Bearer synthetic-auth", "X-SuMpTeR-Agent-Name"="stale-name", "X-Sumpter-Git-Remote"="https://stale.invalid/old.git", "X-Sumpter-Request-Kind"="memory"}\nenv_http_headers={"x-Sumpter-Agent-Role"="SYNTHETIC_ROLE", "X-From-Env"="SYNTHETIC_KEEP"}\n`);
  const original = readFileSync(f.config, 'utf8');
  const args = ['exec', '--ephemeral', '--skip-git-repo-check', '--model', 'gpt-5.4', '--sandbox', 'read-only', '-c', 'model_providers.cpa.http_headers={"X-CLI-Keep"="cli-value","X-Sumpter-Session-Id"="stale-session"}', '--', 'Say OK'];
  const launch = prepareLaunch('codex', args, { ...f.env, SUMPTER_BASE_URL: `http://127.0.0.1:${server.address().port}`, SYNTHETIC_ROLE: 'subagent', SYNTHETIC_KEEP: 'env-value' }, f.home);
  assert.ok(!launch.args.some(arg => arg.includes('synthetic-auth')), 'never copy config credentials into argv');
  const child = spawn(process.env.CODEX_ATTRIBUTION_TEST_BIN, launch.args, { env: launch.env, cwd: f.home, stdio: ['ignore', 'pipe', 'pipe'] });
  t.after(() => child.kill());
  child.stdout.resume(); child.stderr.resume();
  const timer = setTimeout(() => child.kill(), 35000);
  t.after(() => clearTimeout(timer));
  await once(child, 'exit');
  assert.ok(captured, 'real Codex must reach the local HTTP endpoint');
  const expected = collectHeaders('claude', f.home, f.env);
  for (const [key, value] of Object.entries(expected)) assert.equal(captured[key.toLowerCase()], value);
  assert.equal(captured['x-test'], 'keep');
  assert.equal(captured['x-sumpter-client'], 'codex');
  assert.equal(captured.authorization, 'Bearer synthetic-auth');
  assert.equal(captured['x-cli-keep'], 'cli-value');
  assert.equal(captured['x-from-env'], 'env-value');
  for (const key of ['session-id', 'agent-name', 'request-kind', 'git-remote', 'agent-role']) {
    // Codex merges header tables and has no key deletion override. Empty values
    // are intentionally inert to Sumpter's bounded metadata parser.
    assert.ok(!captured[`x-sumpter-${key}`], `stale ${key} must be neutralized`);
  }
  assert.equal(readFileSync(f.config, 'utf8'), original);
});

test('Codex management bypasses attribution without misreading prompt or option values', t => {
  const f = fixture(t);
  rmSync(f.config);
  for (const args of [['login', 'status'], ['logout'], ['-C', f.repo, 'mcp', 'list'], ['--config', 'model_provider="openai"', 'features', 'list'], ['completion', 'bash'], ['--help']]) {
    const launch = prepareLaunch('codex', args, f.env, f.repo);
    assert.deepEqual(launch.args, args);
    assert.deepEqual(launch.env, f.env);
  }
  for (const args of [['exec', 'login'], ['--model', 'login', 'exec', 'hi'], ['--', 'login'], ['exec', '--', '--help']]) {
    assert.deepEqual(prepareLaunch('codex', args, f.env, f.repo).args, args);
  }
});

test('Codex cleanup recognizes TOML keys, CLI overrides and project headers without copying their values', t => {
  const f = fixture(t);
  writeFileSync(f.config, `model_provider="cpa"\n[model_providers.cpa.http_headers]\n"X-\\u0053umpter-Agent-Name"="synthetic-private"\n'x-Sumpter-Request-Kind'='memory'\nX-Sumpter-Client-Variant='desktop'\nAuthorization='Bearer synthetic-auth'\nX-Label='X-Sumpter-Custom.Field'\n`);
  mkdirSync(join(f.repo, '.codex'));
  writeFileSync(join(f.repo, '.codex/config.toml'), '[model_providers.cpa.env_http_headers]\n"X-Sumpter-Future"="SYNTHETIC_FUTURE"\n');
  const args = ['exec', '-c', 'model_providers.cpa.http_headers={"X-Sumpter-Session-Id"="old", "X-Keep"="keep"}', '--', 'prompt'];
  const launch = prepareLaunch('codex', args, f.env, f.repo);
  assert.deepEqual(launch.args.slice(-2), ['--', 'prompt']);
  for (const name of ['X-Sumpter-Agent-Name', 'x-Sumpter-Request-Kind', 'X-Sumpter-Client-Variant', 'X-Sumpter-Future', 'X-Sumpter-Session-Id']) {
    assert.ok(launch.args.includes(`model_providers.cpa.http_headers.${name}=""`));
    assert.ok(launch.args.includes(`model_providers.cpa.env_http_headers.${name}=""`));
  }
  assert.ok(!launch.args.some(arg => /synthetic-private|synthetic-auth/u.test(arg)));
  assert.ok(!launch.args.some(arg => arg.includes('http_headers.Authorization=')));
  writeFileSync(join(f.repo, '.codex/config.toml'), '[model_providers.cpa.http_headers]\n"X-Sumpter-Custom.Field"="old"\n');
  assert.throws(() => prepareLaunch('codex', args, f.env, f.repo), /含点号/);
});
