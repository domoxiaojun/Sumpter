# 开发指南

所有命令从仓库根执行。Rust workspace 使用 Rust 1.88+；WebUI 使用 Node.js 22 和 npm；文档脚本使用 uv；Docker 部署契约测试使用 Go。SQLite 已随 Rust 运行时提供，不安装外部数据库。

## 环境和最小检查

```bash
npm ci --prefix platforms/linux/webui
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
# macOS 主机
./scripts/check.sh macos
# 需要 Go；不启动 Docker
./scripts/check.sh docker
```

| 改动 | 检查 |
| --- | --- |
| Markdown、模板、版本同步 | `./scripts/check.sh docs`；有链接时运行 lychee |
| core / runtime / engine / adapter | `./scripts/check.sh rust` |
| WebUI | `./scripts/check.sh web`，必要时做浏览器验收 |
| SwiftUI / macOS wire | `./scripts/check.sh macos` |
| Shell / TOML / Actions | shellcheck / taplo / actionlint 与定向自测 |
| Compose、Dockerfile、忽略文件 | `./scripts/check.sh docker`；真实镜像在 Linux CI 验证 |

Rust 检查包含 fmt、check、test、clippy，并使用 `--locked`。`web` 会重建并检查 `platforms/linux/web/` 是否与源码一致；需要审阅并提交生成产物。不要用全量自动格式化掩盖无关差异。

## 运行开发实例

准备一个仓库外的专用目录，避免碰到已安装服务：

```bash
install -d -m 700 /tmp/sumpter-dev
install -m 600 config.example.json /tmp/sumpter-dev/config.json
(umask 077; set -C; openssl rand -hex 32 > /tmp/sumpter-dev/admin-password)
cargo run --locked -p sumpterd-linux -- --config-dir /tmp/sumpter-dev --no-web
```

编辑 `config.json` 时至少把 listener 改为真实测试地址，入口和模型组示例默认停用。macOS App 通过 SwiftPM 或 App 运行时会管理自己的 sidecar；不要让开发实例和已安装实例共用端口或数据库。

## 维护源与同步

| 维护源 | 命令 | 生成目标 |
| --- | --- | --- |
| `docs/templates/usage-onboarding.md` + path matrix | `uv run scripts/maintenance/sync-usage-docs.py --write` | 根和 Linux `USAGE.md` 标记块 |
| 根配置、许可证、更新记录 | `node scripts/maintenance/sync-project-metadata.mjs --write` | Linux / macOS 配置副本和包元数据 |
| `scripts/clients/` | `node scripts/maintenance/sync-client-attribution.mjs` | Linux 脚本、macOS Resources、Gemini 资源 |
| WebUI 源码 | `npm run build --prefix platforms/linux/webui` | `platforms/linux/web/` |

检查模式不会写文件。修改共享行为时同时核对两个 adapter、两个客户端手册和配置示例；修改部署模板时同时更新 Compose 教程与数据目录说明。

## 提交改动

从最新 `main` 创建短分支，保持一次提交一个可说明的行为变化。只暂存本次文件，保留混合工作区中的其它改动。PR 说明触发条件、用户可见结果、影响平台、实际验证和未覆盖层次；不提交真实配置、数据库、缓存、诊断捕获或自动工具署名。
