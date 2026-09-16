import { readFileSync, appendFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

const stableTag = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const numericVersion = /^\d+(?:\.\d+)*$/;

export function compareVersions(left, right) {
  if (!numericVersion.test(left) || !numericVersion.test(right)) {
    throw new Error('版本必须由数字和点组成');
  }
  const a = left.split('.').map(BigInt);
  const b = right.split('.').map(BigInt);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const difference = (a[index] ?? 0n) - (b[index] ?? 0n);
    if (difference !== 0n) return difference > 0n ? 1 : -1;
  }
  return 0;
}

// 只读取本仓库 generate_appcast 输出的数字版本字段；不解析外部实体或执行 XML。
export function appcastVersions(xml) {
  if (!xml.trim()) return [];
  if (/<!DOCTYPE|<!ENTITY/i.test(xml) || !/<rss\b/.test(xml) || !/<\/rss>/.test(xml)) {
    throw new Error('appcast XML 格式不受支持');
  }
  const items = [...xml.matchAll(/<item\b[^>]*>([\s\S]*?)<\/item>/g)];
  if (items.length === 0) throw new Error('appcast 缺少版本条目');
  return items.map(([, item]) => {
    const field = name => {
      const element = item.match(new RegExp(`<sparkle:${name}\\b[^>]*>\\s*([^<]+?)\\s*</sparkle:${name}>`));
      const enclosure = item.match(/<enclosure\b[^>]*>/)?.[0] ?? '';
      const attribute = enclosure.match(new RegExp(`\\bsparkle:${name}=["']([^"']+)["']`));
      const value = element?.[1] ?? attribute?.[1];
      if (!value || !numericVersion.test(value)) throw new Error(`appcast 缺少有效的 ${name}`);
      if (element && attribute && element[1] !== attribute[1]) throw new Error(`appcast ${name} 字段冲突`);
      return value;
    };
    return { build: field('version'), version: field('shortVersionString') };
  });
}

export function verifyPublication({ tag, releases, candidateAppcast, currentAppcast = '' }) {
  const parsed = tag.match(stableTag);
  if (!parsed) throw new Error('发布 tag 必须是规范的 vX.Y.Z');
  const version = tag.slice(1);
  if (!Array.isArray(releases)) throw new Error('Release 列表必须是数组');
  const rows = releases.flat();
  let resumeDraft = false;
  for (const release of rows) {
    if (!release || typeof release.tag_name !== 'string' || typeof release.draft !== 'boolean') {
      throw new Error('Release 状态字段不完整，拒绝发布');
    }
    if (release.tag_name === tag) {
      if (!release.draft) throw new Error(`${tag} 已正式发布，不允许覆盖附件或浮动标签`);
      resumeDraft = true;
    }
    // draft 是浮动输出的预留边界：上一轮可能已推镜像、尚未来得及公开 Release。
    if (stableTag.test(release.tag_name) && compareVersions(release.tag_name.slice(1), version) > 0) {
      throw new Error(`已有更高版本 ${release.tag_name}，拒绝 ${tag} 回退发布通道`);
    }
  }
  const candidate = appcastVersions(candidateAppcast);
  if (candidate.length !== 1 || compareVersions(candidate[0].version, version) !== 0) {
    throw new Error('候选 appcast 必须恰好包含当前 tag 的一个版本');
  }
  for (const current of appcastVersions(currentAppcast)) {
    if (compareVersions(candidate[0].version, current.version) <= 0) {
      throw new Error('候选 appcast 产品版本没有前进');
    }
    if (compareVersions(candidate[0].build, current.build) <= 0) {
      throw new Error('候选 Sparkle build 版本没有前进，请提高 MACOS_BUILD_NUMBER_BASE');
    }
  }
  return { resumeDraft, version, build: candidate[0].build };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const [tag, releasesPath, candidatePath, currentPath] = process.argv.slice(2);
    if (!tag || !releasesPath || !candidatePath || !currentPath) {
      throw new Error('用法: node check-release-publication.mjs TAG RELEASES_JSON CANDIDATE_XML CURRENT_XML');
    }
    const result = verifyPublication({
      tag,
      releases: JSON.parse(readFileSync(releasesPath, 'utf8')),
      candidateAppcast: readFileSync(candidatePath, 'utf8'),
      currentAppcast: readFileSync(currentPath, 'utf8'),
    });
    if (process.env.GITHUB_OUTPUT) {
      appendFileSync(process.env.GITHUB_OUTPUT, `resume_draft=${result.resumeDraft}\n`);
    }
    console.log(`发布前进检查通过：${tag}，Sparkle ${result.build}，${result.resumeDraft ? '恢复 draft' : '新发布'}`);
  } catch (error) {
    console.error(`发布被拒绝：${error.message}`);
    process.exitCode = 1;
  }
}
