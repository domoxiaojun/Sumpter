import test from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
const root = new URL('../../', import.meta.url);
const text = path => readFileSync(new URL(path, root), 'utf8');
const json = path => JSON.parse(text(path));

test('workspace, WebUI and changelog share one release version', () => {
  const version = text('Cargo.toml').match(/\[workspace\.package\][\s\S]*?\nversion = "([^"]+)"/)[1];
  assert.equal(json('platforms/linux/webui/package.json').version, version);
  const lock = json('platforms/linux/webui/package-lock.json');
  assert.equal(lock.version, version);
  assert.equal(lock.packages[''].version, version);
  assert.ok(text('CHANGELOG.md').includes(`## [${version}]`));
  assert.ok(text('.github/workflows/release.yml').includes('test -f CHANGELOG.md'));
});
test('safe configuration examples stay equivalent across platforms', () => {
  const config = json('config.example.json');
  assert.equal(config.schemaVersion, 7);
  assert.equal(config.listener.authToken, '');
  assert.equal(config.listener.host, '127.0.0.1');
  assert.ok(config.endpoints.length > 0);
  for (const endpoint of config.endpoints) {
    assert.equal(endpoint.enabled, false);
    assert.ok(new URL(endpoint.baseURL).hostname.endsWith('.invalid'));
    assert.ok(!endpoint.apiKey || /example|placeholder|replace|invalid/i.test(endpoint.apiKey));
  }
  for (const platform of ['linux', 'macos']) assert.deepEqual(json(`platforms/${platform}/config.example.json`), config);
});
test('shared crates do not depend on platform crates and obsolete workflows stay removed', () => {
  for (const name of ['core', 'runtime', 'engine']) {
    assert.doesNotMatch(text(`crates/sumpter-${name}/Cargo.toml`), /sumpter-(?:linux|macos)-adapter|sumpterd-(?:linux|macos)/);
  }
  for (const name of ['ci', 'release', 'container']) {
    assert.equal(existsSync(new URL(`platforms/linux/.github/workflows/${name}.yml`, root)), false);
  }
});
test('macOS build paths pin the same stable Xcode and SDK baseline', () => {
  const selector = text('platforms/macos/app/select-xcode.sh');
  assert.match(selector, /required_version=26\.6/);
  assert.match(selector, /required_build=17F113/);
  assert.match(selector, /required_sdk=26\.5/);
  for (const workflow of ['.github/workflows/ci.yml', '.github/workflows/release.yml']) {
    assert.match(text(workflow), /runs-on: macos-26/);
    assert.match(text(workflow), /select-xcode\.sh/);
  }
});
