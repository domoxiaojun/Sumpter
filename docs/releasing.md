# 发布指南

发布由根目录 `.github/workflows/release.yml` 负责。版本来源是根 `Cargo.toml` 的 workspace 版本；Linux、WebUI、macOS 和配置示例应保持一致。

## 发布前

1. 更新 `Cargo.toml`、`CHANGELOG.md` 和必要的 WebUI 版本。
2. 运行 `node scripts/maintenance/sync-project-metadata.mjs --write` 与 `uv run scripts/maintenance/sync-usage-docs.py --write`。
3. 运行 `./scripts/check.sh docs`、`rust`、`web`；macOS 发布在 macOS runner 上验证。
4. 检查 `git diff --check`，确认没有配置、数据库、密钥、构建缓存或诊断捕获。
5. 创建并推送与 workspace 版本完全一致的 `vX.Y.Z` tag。

## CI 产物

Release 先构建并验证，再由唯一 publish job 上传：

- Linux：`sumpter-linux-x86_64.tar.gz`、`sumpter-linux-aarch64.tar.gz`、`SHA256SUMS`。
- 容器：GHCR `ghcr.io/domoxiaojun/sumpter`，amd64 / arm64 manifest。
- macOS：DMG、Sparkle zip、`appcast.xml`、`macOS-SHA256SUMS`。

Linux 包把 `platforms/linux/` 作为包根，包含 `sumpterd`、WebUI、Compose 模板、systemd 和安装脚本。发布成功不表示任何机器已经安装或部署。

## 发布验收

以 GitHub Actions job 成功、GitHub Release 非草稿、资产名称和 SHA256 可下载为准。容器还要确认 manifest 的两种架构和 GHCR 标签；macOS 还要确认 zip 是完整 `.app`、Sparkle 签名和 feed 地址。Apple Developer 签名、公证和生产反代属于独立部署层，不由源码构建结果证明。

## 发布前进与重试

publish job 跨版本串行执行，并在写入 GHCR、Release 附件和 appcast 前重新读取 GitHub Release 列表及 `macos-updates` 当前 feed。串行只避免同时写入，不保证版本顺序，因此另有以下前进检查：

- 发现更高的规范 `vX.Y.Z` Release（包括 draft）时，拒绝旧 tag 的整个发布。当前工作流将版本附件、浮动镜像标签与 Sparkle feed 作为一次统一发布，不提供旧 tag 的 versioned-only 回补模式，避免部分输出已变更后再判断升级顺序。
- 已正式发布的同版本不得重新上传或覆盖资产，也不得重写浮动镜像标签。需要修正正式版本时发布新的版本号。
- 同版本仍为 draft、且不存在更新的发布边界时，可以重试并补齐候选资产。draft 在推送镜像前创建，因此一次失败的镜像发布也会为后续旧 tag 保留前进边界。
- 候选 appcast 的产品版本必须对应当前 tag，且产品版本与 `sparkle:version` 都必须高于当前 feed。若 CI 重置导致 build number 未前进，先调整 `MACOS_BUILD_NUMBER_BASE` 再重试。
- 远端状态读取失败或 appcast 版本字段缺失时直接停止，不把失败视为“首次发布”。

本地可用 `node --test scripts/tests/release-publication.test.mjs` 检查这些规则；该测试只读取合成状态，不连接发布服务。

不要重写已存在的正式 tag。替换同版本候选前先取消过时的 queued/in-progress workflow，再触发新的候选构建；已经进入 publish 的运行先单独检查，避免两个运行同时上传资产。
