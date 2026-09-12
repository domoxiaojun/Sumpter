# 版本与发布流程

Linux 与 macOS 使用同一个 `vX.Y.Z` 版本和根 `.github/workflows/release.yml`。其他工作流只验证，不对外发布。版本源是根 Cargo workspace，WebUI 的 package.json/package-lock.json 同步版本；Release 正文来自根 `CHANGELOG.md` 的对应版本章节。

## 发布前

1. 需求、代码、文档和平台验收完成，相关 PR 合入 main；在待发布提交上核对 CI 成功。
2. 更新 workspace 与 WebUI 版本及锁文件，将 Unreleased 的变化移入 `## [X.Y.Z]`，按平台写清行为和兼容性。
3. 运行 `node scripts/maintenance/sync-project-metadata.mjs --write`，完成受影响层检查，提交最终版本。
4. 核对 GitHub Actions Secrets、产物地址、Sparkle build number 与签名方式。正式 tag 不可改写；同一候选重试前先检查已有构建/发布，只取消被替代的构建。
5. 创建并推送与 workspace 一致的 `vX.Y.Z` tag；也可手动选择一个已经存在的 tag 重跑。不要仅凭 push 成功宣告发布完成。

## 自动流程

| 阶段 | 内容 | 写权限 |
| --- | --- | --- |
| Resolve | 校验 tag 形状与 workspace 版本 | 只读 |
| Quality | 文档/元数据契约、WebUI 测试与已提交产物一致性 | 只读 |
| Linux | Rust 验证、交叉构建两架构、归档和 checksum | 只读 |
| Container validation | 使用 Linux 二进制验证 amd64/arm64 镜像 | 只读 |
| macOS | 测试 App、构建包、Sparkle appcast 与 checksum | 只读 |
| Publish | 所有构建成功后，串行发布 GHCR、Release 和更新 feed | contents/packages write |

发布不是跨服务原子事务。GHCR、GitHub Release、appcast 任一步失败都可能留下部分已发布结果；应检查现状后修复同一 tag 的失败阶段，不重新打正式 tag。现有流程允许同 tag 重试并替换 Release 附件，维护者应仅在恢复失败发布时使用。

## 产物与平台范围

- Linux：`sumpter-linux-x86_64.tar.gz`、`sumpter-linux-aarch64.tar.gz`、`SHA256SUMS`；包内二进制为 `sumpterd`，并携带独立部署输入 `compose.yaml`、`DOCKER.md`（不含源码构建 override）。
- macOS：`sumpter-macos-X.Y.Z.dmg`、`sumpter-macos-X.Y.Z.zip`、`appcast.xml`、`macOS-SHA256SUMS`。
- 容器：当前仓库对应 GHCR image 的 `linux/amd64` 与 `linux/arm64` manifest。

当前 macOS Release 默认 `ARCH=arm64`，不提供 universal 包；本机脚本可指定 x86_64，但不能据此宣称 Intel 已通过自动发布验收。macOS 最低系统版本是 14。

## 签名与更新设置

必需 Secrets 是 `SPARKLE_PRIVATE_KEY` 和 `SPARKLE_PUBLIC_ED_KEY`，缺失会阻断 macOS 构建，因此也会阻断统一发布。`CODESIGN_IDENTITY` 可选；设置名称不等于 CI 钥匙串中已存在证书与私钥。

当前工作流没有自动导入 Apple Developer ID 证书，也没有 notarization 步骤。默认使用 ad-hoc 签名；Sparkle 更新签名不代表 Apple 签名或公证。需要面向普通用户的 Apple 信任链时，应先补齐证书注入和公证流程，再标注为已签名公证分发。现有手动流程见 [macOS 更新](../platforms/macos/app/UPDATE.md)。

App 更新 feed 使用 `macos-updates` 分支。新仓库的 `github.run_number` 会重新计数；若延续已有用户更新链，设置仓库变量 `MACOS_BUILD_NUMBER_BASE` 为大于已有最大 build number 的非负整数，工作流用它加当前 run number。已有用户需要保持 Sparkle key 对和旧 feed 的可达性；私有仓库的 raw feed 也必须核对匿名客户端能否访问。

## 发布验收

- Actions 结束且所有必需 job 成功，确认 tag 指向实际待发布提交。
- Release 公开状态、两个 Linux 包、DMG、zip、appcast、两个 checksum 文件均完整且校验匹配。
- GHCR manifest 确实包含两架构，appcast 下载 URL 可达且签名与 App 公钥匹配。
- 在目标 Linux 上验证 systemd 启动、Admin 登录与实际代理请求；在 macOS 上验证安装启动、sidecar 与更新检查。
- 若本次改动涉及容器部署：先用 `./scripts/check.sh docker` 校验模板，发布后核对 Linux 包内的 `compose.yaml` / `DOCKER.md` 与仓库一致，并在真机跑一次 `docker compose up -d`、登录和停机迁移。
- 用户安装入口是 GitHub Release 与仓库 raw 脚本。若仍维护 `sf.domob.org` 静态镜像，须单独同步并核对；GitHub 发布成功不会自动更新该镜像。

记录执行过的验证与未执行的安装/流量验收。不要把历史测试数、编译退出码或上传成功当作完整交付证明。
