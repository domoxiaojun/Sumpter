import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { appcastVersions, compareVersions, verifyPublication } from '../maintenance/check-release-publication.mjs';

const feed = (version, build) => `<?xml version="1.0"?><rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item><sparkle:version>${build}</sparkle:version><sparkle:shortVersionString>${version}</sparkle:shortVersionString><enclosure url="https://example.invalid/app.zip" /></item></channel></rss>`;
const input = overrides => ({ tag: 'v0.4.16', releases: [], candidateAppcast: feed('0.4.16', '120'), currentAppcast: feed('0.4.15', '119'), ...overrides });

test('数字版本比较不会按字符串排序或丢失大整数精度', () => {
  assert.equal(compareVersions('0.4.10', '0.4.9'), 1);
  assert.equal(compareVersions('9007199254740993', '9007199254740992'), 1);
  assert.equal(compareVersions('120', '120.0'), 0);
});

test('新版本发布通过，未完成 draft 可重试', () => {
  assert.equal(verifyPublication(input()).resumeDraft, false);
  assert.equal(verifyPublication(input({ releases: [[{ tag_name: 'v0.4.16', draft: true }]] })).resumeDraft, true);
});

test('已正式发布的相同 tag 不得重新覆盖附件或浮动镜像', () => {
  assert.throws(() => verifyPublication(input({ releases: [{ tag_name: 'v0.4.16', draft: false }] })), /已正式发布/);
});

test('串行排队并非版本顺序：旧 tag 在新版本或新 draft 后都必须拒绝', () => {
  for (const draft of [false, true]) {
    assert.throws(() => verifyPublication(input({ releases: [[{ tag_name: 'v0.5.0', draft }]] })), /更高版本/);
  }
});

test('appcast 的产品版本和 Sparkle build 均必须前进', () => {
  for (const build of ['119', '118']) {
    assert.throws(() => verifyPublication(input({ candidateAppcast: feed('0.4.16', build) })), /Sparkle build/);
  }
  for (const version of ['0.4.16', '0.5.0']) {
    assert.throws(() => verifyPublication(input({ currentAppcast: feed(version, '118') })), /产品版本/);
  }
  assert.throws(() => verifyPublication(input({ candidateAppcast: feed('0.4.17', '120') })), /当前 tag/);
});

test('支持旧式 enclosure 版本属性，拒绝缺字段或非数字 feed', () => {
  assert.deepEqual(appcastVersions('<rss><channel><item><enclosure sparkle:version="123" sparkle:shortVersionString="0.4.16" /></item></channel></rss>'), [{ build: '123', version: '0.4.16' }]);
  assert.throws(() => appcastVersions('<rss><channel><item /></channel></rss>'), /条目/);
  assert.throws(() => appcastVersions(feed('0.4.16', 'invalid')), /version/);
  assert.throws(() => appcastVersions('<!DOCTYPE rss><rss></rss>'), /格式/);
  assert.equal(verifyPublication(input({ currentAppcast: '' })).version, '0.4.16');
});

test('发布检查位于串行 publish 内且先于所有浮动输出', () => {
  const workflow = readFileSync(new URL('../../.github/workflows/release.yml', import.meta.url), 'utf8');
  const publish = workflow.slice(workflow.indexOf('  publish:'));
  const guard = publish.indexOf('check-release-publication.mjs');
  assert.ok(guard > publish.indexOf('group: release-publish'));
  for (const output of ['Build and publish multi-architecture image', 'gh release upload', 'Publish stable appcast branch']) {
    assert.ok(guard < publish.indexOf(output), output);
  }
  assert.ok(publish.indexOf('gh release create') < publish.indexOf('Build and publish multi-architecture image'));
  assert.match(publish, /--json isDraft/);
  assert.match(publish, /gh api --paginate --slurp/);
});
