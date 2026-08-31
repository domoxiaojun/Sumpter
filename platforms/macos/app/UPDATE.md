# macOS 更新发布

macOS App 使用 Sparkle 2.9.6。Sparkle 只负责客户端更新逻辑，更新包和 `appcast.xml` 仍需托管在用户可访问的 HTTPS 地址。

## 首次安装与自动更新产物

`package-app.sh` 会生成：

- `dist/Sumpter.dmg`：首次安装包。
- `dist/Sumpter-<版本>-macos.zip`：Sparkle 更新包。

DMG 内还会放入 `安装 Sumpter.command` 和 `安装说明.txt`。没有 Apple Developer notarization 时，用户可运行这个安装器；它会先校验旁边 App 的 Bundle ID 和代码签名，再只移除该 App 的 quarantine，并优先安装到用户可写的 `/Applications` 或 `~/Applications`。它不会关闭全局 Gatekeeper。

Sparkle 更新包必须是完整 `.app` 的 zip，而不是只替换 `sumpterd`。App 退出时 sidecar 会随父进程结束，Sparkle 再原子替换整个 App 并重启。

## 打包配置

正式包需要同时提供：

```bash
: "${VERSION:?请先设置 VERSION，例如 X.Y.Z}"
: "${BUILD_NUMBER:?请先设置单调递增的 BUILD_NUMBER}"
export SHORT_VERSION="$VERSION"
export BUILD_VERSION="$BUILD_NUMBER"
export SPARKLE_FEED_URL=https://updates.example.com/sumpter/appcast.xml
export SPARKLE_PUBLIC_ED_KEY='Sparkle generate_keys 输出的公钥'
# 正式包替换为 Developer ID Application 证书名称；省略时仅 Ad-hoc 测试签名
export CODESIGN_IDENTITY='Developer ID Application: Example (TEAMID)'
./package-app.sh
```

`SPARKLE_FEED_URL` 和 `SPARKLE_PUBLIC_ED_KEY` 未设置时，开发包仍可构建，但菜单栏中的“检查更新…”会禁用；只设置其中一个会直接拒绝打包。

正式分发还必须使用 Apple Developer ID 签名和 notarization。`CODESIGN_IDENTITY` 设置为证书名称后，脚本会使用 hardened runtime 和 timestamp 签名；省略时的 Ad-hoc 签名仅适合本机测试，不能作为面向普通用户的信任链。notarization 仍需在 CI/发布环节完成。

## 没有 Apple Developer 账号

仍可使用当前脚本的默认 Ad-hoc 签名，把 DMG/zip 和 `appcast.xml` 托管到 GitHub Releases、GitHub Pages 或自己的 HTTPS 站点。Sparkle 的 Ed25519 更新签名不收费，也不要求 Apple Developer 账号。

但首次安装的用户通常需要在 Finder 中右键 App 选择“打开”，或在“系统设置 → 隐私与安全性”里允许打开。这个模式适合自己使用或少量明确知情的用户；不能保证普通用户像 notarized App 一样双击即装，也不应伪装成 Apple 已验证的软件。

## 生成 appcast

把一个或多个版本化 zip 放进同一个 archives 目录，并将私钥通过安全环境注入：

```bash
: "${VERSION:?请先设置 VERSION，例如 X.Y.Z}"
export SPARKLE_DOWNLOAD_URL_PREFIX="https://github.com/domoxiaojun/sumpter/releases/download/v${VERSION}/"
./generate-appcast.sh /path/to/release-archives
```

本机运行时脚本会从 macOS 钥匙串读取 `ed25519` 私钥；CI 环境才设置 `SPARKLE_PRIVATE_KEY`，并通过 stdin 传给 Sparkle 工具。脚本会生成或更新 `/path/to/release-archives/appcast.xml`。将 `appcast.xml` 放到稳定地址（例如 GitHub Pages），将 zip 上传到 GitHub Release 或对象存储；`SPARKLE_DOWNLOAD_URL_PREFIX` 必须与实际资产地址匹配。

建议每个版本同时发布 DMG、Sparkle zip、appcast.xml 和人工核对用的 `macOS-SHA256SUMS`。Sparkle 自身的 Ed25519 签名不能被普通 checksum 取代。

## GitHub 在线发布流程

发布仓库（`domoxiaojun/sumpter`）里 `.github/workflows/macos-release.yml` 与 Linux 的
`release.yml` 监听同一个 `v*` tag，所以一个 tag 同时出两个平台的产物。工作流构建前会校验
源码 monorepo 使用根 `Cargo.toml`；独立发布树则使用 `macos/Cargo.toml`。无论拓扑如何，
macOS 版本、共享 workspace 版本与 tag 三者必须一致。

先在仓库的 Settings → Secrets and variables → Actions 中配置：

- `SPARKLE_PUBLIC_ED_KEY`：`generate_keys` 输出的公钥。
- `SPARKLE_PRIVATE_KEY`：Sparkle 私钥；只放 GitHub Actions Secret，不提交仓库。
- `CODESIGN_IDENTITY`：可选。没有 Apple Developer 账号时留空，工作流使用 Ad-hoc 签名。

### 推送方式（不要直接推 monorepo）

上游 monorepo 里平台输入位于 `platforms/linux/` 与 `platforms/macos/`，而发布仓库的**根就是 Linux 版**、
macOS 挂在 `macos/` 子目录。**不要把整个 monorepo 直接推到发布仓库**，必须用合成提交把平台 tree 拼成发布仓库的形状：

```bash
cd <monorepo 根>
set -euo pipefail
VERSION="${VERSION:?请先设置 VERSION，例如 X.Y.Z}"
TAG="v${VERSION}"
git fetch --tags origin

if git show-ref --verify --quiet "refs/tags/${TAG}" \
  || git ls-remote --exit-code --tags origin "refs/tags/${TAG}" >/dev/null 2>&1; then
  echo "tag 已存在：${TAG}" >&2
  exit 1
fi

FINAL_ROOT=$(git rev-parse HEAD)
LINUX_TREE=$(git rev-parse "${FINAL_ROOT}:platforms/linux")
MACOS_TREE=$(git rev-parse "${FINAL_ROOT}:platforms/macos")
PARENT=$(git rev-parse origin/main)

RELEASE_TMP_DIR=$(mktemp -d)
RELEASE_INDEX="${RELEASE_TMP_DIR}/index"
trap 'rm -f "$RELEASE_INDEX" "$RELEASE_INDEX.lock"; rmdir "$RELEASE_TMP_DIR" 2>/dev/null || true' EXIT
GIT_INDEX_FILE="$RELEASE_INDEX" git read-tree "$LINUX_TREE"
GIT_INDEX_FILE="$RELEASE_INDEX" git read-tree --prefix=macos/ "$MACOS_TREE"
RELEASE_TREE=$(GIT_INDEX_FILE="$RELEASE_INDEX" git write-tree)

CANDIDATE=$(git commit-tree "$RELEASE_TREE" -p "$PARENT" -m "release: ${TAG}")
git merge-base --is-ancestor "$PARENT" "$CANDIDATE"
test "$(git rev-parse "${CANDIDATE}^{tree}")" = "$RELEASE_TREE"
test "$(git rev-parse "${CANDIDATE}:macos")" = "$MACOS_TREE"

git tag -a "$TAG" "$CANDIDATE" -m "release: ${TAG}"
test "$(git cat-file -t "$TAG")" = tag
git push --atomic origin \
  "$CANDIDATE:refs/heads/main" \
  "refs/tags/${TAG}"

# CI 不监听 main/tag push，需要发布后显式触发。
gh workflow run ci.yml --repo domoxiaojun/sumpter --ref main
```

版本 tag 会自动触发 Linux Release、Container 和 macOS Release；`ci.yml` 只监听 pull request 与
手动触发，单独推 `main` 不会触发这四条工作流。以上 atomic push 保证远端分支与 annotated tag
要么一起推进、要么都不推进。

macOS 工作流会构建 DMG、Sparkle zip、签名后的 `appcast.xml` 和 `macOS-SHA256SUMS`，创建或更新
GitHub Release，并将稳定 feed 发布到 `macos-updates` 分支：

```text
https://raw.githubusercontent.com/domoxiaojun/sumpter/macos-updates/macos/appcast.xml
```

因此 App 内的 `SPARKLE_FEED_URL` 不跟随版本变化。工作流需要仓库 `contents: write` 权限；首次运行会自动创建 `macos-updates` 分支。
