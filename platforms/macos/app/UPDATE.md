# macOS 更新与分发

Sumpter macOS App 使用 Sparkle 2.9.6。DMG 用于首次安装，版本化 zip 和 `appcast.xml` 用于应用内更新。更新替换完整 `.app`，不会只替换 sidecar 二进制。

## 本机构建

从仓库根执行：

```bash
./scripts/build-macos-dmg.sh --clean
```

产物位于 `platforms/macos/app/dist/`：`Sumpter.dmg` 和 `Sumpter-<version>-macos.zip`。默认使用 ad-hoc 签名；本机构建通过不表示已签名、公证或安装成功。

## Sparkle 配置

正式更新需要稳定 HTTPS feed、Sparkle Ed25519 公钥和私钥。私钥只放钥匙串或 CI Secret：

```bash
export SHORT_VERSION=0.4.18
export BUILD_VERSION=1
export SPARKLE_FEED_URL='https://updates.example.com/sumpter/appcast.xml'
export SPARKLE_PUBLIC_ED_KEY='仅填公钥'
export CODESIGN_IDENTITY='Developer ID Application: Example (TEAMID)' # 可选
./platforms/macos/app/package-app.sh
```

只设置 feed 或公钥会拒绝打包；两者都不设置时可生成无自动更新的开发包。Developer ID、hardened runtime、公证和 Sparkle 签名是独立配置，必须以实际产物检查为准。

## 生成 appcast

将完整 App zip 放进归档目录，注入私钥并生成：

```bash
export SPARKLE_DOWNLOAD_URL_PREFIX='https://github.com/domoxiaojun/sumpter/releases/download/v0.4.18/'
./platforms/macos/app/generate-appcast.sh /path/to/archives
```

`appcast.xml` 的下载 URL 必须与实际托管资产一致。发布时同时提供 DMG、Sparkle zip、appcast 和 `macOS-SHA256SUMS`。普通 checksum 不能替代 Sparkle Ed25519 签名。

## 发布检查

确认 workspace 版本、tag、zip 内完整 `.app`、Bundle ID、签名、feed、更新签名、Release 资产和 checksum 一致。没有 Apple Developer 账号时可以发布 ad-hoc 包，但用户可能需要右键“打开”，不能声称已通过 Apple 公证。
