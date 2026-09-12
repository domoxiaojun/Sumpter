# macOS 更新发布

macOS App 使用 Sparkle 2.9.6。Sparkle 只负责客户端更新逻辑，更新包和 `appcast.xml` 仍需托管在用户可访问的 HTTPS 地址。

跨平台版本与发布门禁统一见 [发布指南](../../../docs/releasing.md)。当前自动发布为 arm64，Apple 签名与公证状态以实际构建结果为准。

## 首次安装与自动更新产物

`package-app.sh` 会生成：

- `dist/Sumpter.dmg`：首次安装包。
- `dist/Sumpter-<版本>-macos.zip`：Sparkle 更新包。

DMG 内还会放入 `安装 Sumpter.command` 和 `安装说明.txt`。没有 Apple Developer notarization 时，用户可运行这个安装器；它会先校验旁边 App 的 Bundle ID 和代码签名，再只移除该 App 的 quarantine，并优先安装到用户可写的 `/Applications` 或 `~/Applications`。它不会关闭全局 Gatekeeper。

Sparkle 更新包必须是完整 `.app` 的 zip，而不是只替换 `sumpterd`。App 退出时 sidecar 会随父进程结束，Sparkle 再原子替换整个 App 并重启。

## 打包配置

下面的手动打包命令在 `platforms/macos/app/` 目录执行。启用自动更新时需要同时提供 feed 与公钥：

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

当前 GitHub 发布默认使用 ad-hoc 签名，未配置 Apple 公证。若需要 Apple 的开发者信任链，另外配置 Developer ID 签名和 notarization。`CODESIGN_IDENTITY` 设置为证书名称后，脚本会使用 hardened runtime 和 timestamp 签名；省略时使用 ad-hoc 签名；这可用于当前发布方式，但不提供 Apple 开发者身份背书。notarization 仍需在 CI/发布环节完成。

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

## 本机构建与正式分发

当前源码树就是 monorepo：Rust workspace 在仓库根，macOS 输入在 `platforms/macos/`。
**本机测试包**用仓库根脚本：

```bash
# 在 monorepo 根
./scripts/build-macos-dmg.sh --clean
```

该脚本调用 `package-app.sh`，走根 `Cargo.toml` 构建 `sumpterd-macos`，默认 ad-hoc 签名，产物在
`platforms/macos/app/dist/`。自动更新分发需要下面的 Sparkle 密钥；Developer ID 和公证是另一套可选的 Apple 分发配置，当前 CI 未实现公证。

GitHub Actions 工作流已提升到仓库根 `.github/workflows/`，按当前根 workspace、
`platforms/linux/` 和 `platforms/macos/` 路径运行。推送与 Cargo workspace 版本一致的 `vX.Y.Z` tag
会触发 Linux Release、GHCR 容器和 macOS Release；macOS Release 还会更新 `macos-updates` 分支。

若自行托管 Sparkle feed，仍需要：

- `SPARKLE_PUBLIC_ED_KEY`：`generate_keys` 输出的公钥。
- `SPARKLE_PRIVATE_KEY`：Sparkle 私钥；只放本机钥匙串或 CI Secret，不提交仓库。
- `CODESIGN_IDENTITY`：可选 Developer ID 证书名称；省略时为 ad-hoc。CI 还必须实际导入相应证书和私钥。

`SPARKLE_FEED_URL` 应指向稳定地址，不跟随版本号变化。当前发布工作流写入的 feed 位于
`https://raw.githubusercontent.com/domoxiaojun/sumpter/macos-updates/macos/appcast.xml`；
发布工作流需要仓库 Actions 的 `SPARKLE_PRIVATE_KEY` 和 `SPARKLE_PUBLIC_ED_KEY`；没有这些 Secret，
macOS Release 会在构建前失败。
