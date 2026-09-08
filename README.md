# Sumpter

本机 AI 请求代理。客户端只连一个地址，由 Sumpter 做入口选择、模型映射、会话粘性、重试和运行记录。

Linux 与 macOS 共用一份 Rust 引擎和 **schema v7** `config.json`。当前版本以根 [Cargo.toml](Cargo.toml) 为准（现为 **0.3.7**），许可证 [MIT](LICENSE)。源码仓库：[domoxiaojun/sumpter](https://github.com/domoxiaojun/sumpter)。

| | 默认 |
| --- | --- |
| 代理 | `http://127.0.0.1:57878`（Codex / OpenAI 兼容客户端使用 `.../v1`） |
| Linux 管理页 | `http://127.0.0.1:57879/admin/` |
| 配置 | schema v7；旧 v3–v6 启动时备份后迁移 |

## 安装

产物在 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest)。

**macOS 14+（当前自动发布 Apple Silicon）**

1. 下载 `sumpter-macos-*.dmg`。
2. 打开后双击「安装 Sumpter.command」，或把 App 拖到「应用程序」后右键打开。
3. 首次打开被拦截时见 [INSTALL.txt](platforms/macos/app/INSTALL.txt)。当前包是 ad-hoc 签名，没有 Apple 公证。

**Linux（x86_64 / aarch64 静态 musl）**

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/install.sh
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

`--repo` 只选下载来源。可同时加 `--admin-host`、`--admin-port`、`--admin-password-file`、`--version vX.Y.Z`。`sudo` 安装为 system 服务，daemon 仍以低权限 `sumpter` 用户运行。已有 `config.json` 与 `admin-password` 不会被覆盖。

容器镜像为 `ghcr.io/domoxiaojun/sumpter`，Compose 默认拉 `:latest`。完整 systemd / Docker / 反代见 [Linux 指南](platforms/linux/README.md)。

## 接入客户端

先在入口库添加上游并绑定模型组，再让**跑客户端的机器**指向代理。本机 daemon 用 `127.0.0.1`；Linux 服务常被远程调用，此时改成该主机可达的 `host:port`，并设置入站 `authToken`。

| 客户端 | 怎么接 |
| --- | --- |
| Claude Code | `ANTHROPIC_BASE_URL=http://<代理>:57878` |
| Codex | Base URL 必须带 `/v1` |
| Grok Build / Gemini CLI / OpenAI 兼容 | 按协议选根地址或 `/v1` |
| pi | `~/.pi/agent/models.json` 增加 provider，并设 `X-Sumpter-Client: pi` |

项目统计的归因装在**启动 Claude / Grok / Gemini / pi 的那台电脑**，不要装到只跑 daemon 的 Linux 上。客户端就在这台 Mac、且用本机 App 时，打开「设置 → 安全」安装。其它机器（包括连远程 Linux 代理的笔记本）在客户端主机执行：

```bash
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh
bash setup-client-attribution.sh install all
```

开箱、排错和协议范围见 [使用指南](USAGE.md)。字段说明见 [配置说明](docs/configuration.md) 与 [config.example.json](config.example.json)。真实密钥放在仓库外。

## 开发

需要 Rust 1.88+、Node.js 22、uv；macOS App 另需完整 Xcode 与 Swift 6+。命令从仓库根执行：

```bash
npm ci --prefix platforms/linux/webui
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
# macOS 主机另执行
./scripts/check.sh macos
```

结构见 [项目结构](docs/project-structure.md)，命令见 [开发指南](docs/development.md)，PR 见 [贡献指南](CONTRIBUTING.md)。

## 仓库地图

| 目录 | 责任 |
| --- | --- |
| `crates/` | 共享配置、路由、运行存储和代理引擎 |
| `adapters/`、`apps/` | 平台边界与可执行入口 |
| `platforms/linux/` | WebUI、systemd、安装与打包 |
| `platforms/macos/` | SwiftUI、通知、DMG / Sparkle |
| `scripts/`、`.github/` | 检查、资源同步、CI 与 Release |
| `docs/` | 现行指南；`templates/` 生成输入；`archive/` 与 `upstream/` 只作历史 |

CI（`ci.yml`）只验证。推送 `vX.Y.Z` tag 才会跑 Release：Linux 包、GHCR、macOS 包与 GitHub Release。构建通过不等于已发布或已安装。
