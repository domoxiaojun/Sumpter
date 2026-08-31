# Sumpter

本地优先的多协议 AI 代理。把 Claude Code、Codex 和其它 OpenAI 兼容客户端接到多个上游 Provider，按模型 mapping、优先级和粘性分组调度，失败时 failover，并把请求链记在本机 SQLite。

当前版本 **0.3.4**（schema v6）。Rust 数据面只有一份共享实现；Linux 与 macOS 通过各自 adapter 接入。

源码仓库：[domoxiaojun/sumpter](https://github.com/domoxiaojun/sumpter) · 许可证 MIT

## 它解决什么

客户端只认一个本机 Base URL。Sumpter 在本机把请求路由到你配置的入口：

- 默认数据面 `http://127.0.0.1:57878`
- 入口是扁平的 `endpoints[]`，每个入口必须用 `mappings[]` 声明承接的客户端模型
- 同一次会话尽量粘在同一 `stickyGroup`，可重试故障后再换组
- 协议由路径决定，不靠 User-Agent 猜：Anthropic Messages、OpenAI Chat、OpenAI Responses，以及 Count Tokens、Images、Compact、Legacy Completions、Codex Alpha Search
- 密钥、Admin 密码、请求体和诊断捕获只留在本机受限文件里

不支持 Responses WebSocket、Realtime / Live、Videos、Files，以及 `/v1/models`。

## 两端产品

| | macOS | Linux |
|---|---|---|
| 形态 | 菜单栏 App + Rust sidecar | 前台 daemon + Web Admin |
| 配置 | `~/Library/Application Support/Sumpter/config.json` | 普通用户：`~/.config/kekulv/config.json`；system 安装：`/var/lib/kekulv/config.json` |
| 管理界面 | App 设置窗 | 浏览器 `http://127.0.0.1:57879/admin/` |
| 入站鉴权 | `listener.authToken`（非空才启用） | 同左 |
| 热重载 | App 里保存 / 重启 | WebUI 保存，或 `SIGHUP` |
| 平台能力 | Claude Code / Codex 系统通知 | systemd、静态 musl 包、Docker |

Linux 运行时目录、服务单元和部分协议 header 仍使用历史名 `kekulv`（例如 `~/.config/kekulv`、`kekulv.service`、`X-Kekulv-*`）。这是兼容契约，不是文档笔误。

## 快速开始

完整开箱、客户端接入和排错见 **[USAGE.md](USAGE.md)**。顺序固定为：

安装/启动 → 找到配置 → 启用入口与 mapping → 配置 Claude/Codex → 首个成功请求 → 查看请求链

### macOS

打开 DMG，双击「安装Sumpter.command」。细节与 Gatekeeper 处理见 [`platforms/macos/app/INSTALL.txt`](platforms/macos/app/INSTALL.txt)。

### Linux

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/kekulv-install.sh https://sf.domob.org/kkl/kekulv-install.sh
bash /tmp/kekulv-install.sh
```

`sudo bash` 会装成 system 服务（daemon 仍以低权限 `kekulv` 用户运行）。Docker、systemd、反代和卸载见 [`platforms/linux/README.md`](platforms/linux/README.md)。

### 接客户端

```bash
# Claude Code
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878

# Codex / 其它 OpenAI 兼容客户端
export OPENAI_BASE_URL=http://127.0.0.1:57878/v1
```

模型名必须能被某个已启用入口的 `mappings.clientPattern` 接住，否则返回 400。`listener.authToken` 非空时，Claude 用 `ANTHROPIC_AUTH_TOKEN`，其它客户端把它当成 API key。

本机自检：

```bash
curl --noproxy '*' http://127.0.0.1:57878/__status
```

配置模板抄 `platforms/linux/config.example.json`（入口默认关闭，域名是 `.invalid`）。字段说明见 [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md)（两端同一份 schema）。

## 仓库结构

```text
crates/                 共享 core / runtime / engine
adapters/linux/         Linux 平台边界、Admin facade、server 组装
adapters/macos/         macOS 平台边界、sidecar facade、通知
apps/linux/             sumpterd-linux 可执行入口
apps/macos/             sumpterd-macos 可执行入口
platforms/linux/        WebUI、静态资源、systemd、安装与 Linux 发布输入
platforms/macos/        SwiftUI App、图标、Sparkle 与 DMG 发布输入
docs/                   当前架构、开箱模板与历史上游对照
scripts/                仓库级维护脚本
```

依赖方向固定为：

```text
apps/<platform> → adapters/<platform> → sumpter-engine → sumpter-runtime → sumpter-core
```

共享 crate 不得反向依赖 adapter、操作系统或 UI。平台差异通过 `crates/sumpter-engine/src/boundary.rs` 注入。可执行入口只做参数、配置目录、监听和生命周期，不复制请求处理逻辑。

| 需求 | 改这里 |
|---|---|
| 配置、路由、协议桥接、事件契约 | `crates/sumpter-core/` |
| SQLite 写入、投影、分页、导出 | `crates/sumpter-runtime/` |
| 入站、重试、relay、回放、健康检查 | `crates/sumpter-engine/` |
| Linux 权限、Admin、systemd 语义 | `adapters/linux/`、`apps/linux/` |
| macOS 控制通道、通知、sidecar 生命周期 | `adapters/macos/`、`apps/macos/` |
| WebUI / SwiftUI / 安装与发布输入 | `platforms/linux/`、`platforms/macos/` |

`docs/upstream/` 是迁移对照，不是当前 API 契约。`config.example.json` 只放禁用的合成入口和 `.invalid` 主机。

## 开发与验证

Rust 2024 workspace，要求 Rust 1.88+。Homebrew macOS 上 cargo 常不在默认 PATH：

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
uv run scripts/sync-usage-docs.py --check
```

可执行文件是 `sumpterd-linux` 和 `sumpterd-macos`。没有 `--all-features` 平台矩阵；平台行为由 adapter 注入。

Linux WebUI：

```bash
cd platforms/linux/webui
npm ci
npm test
npm run build    # 产物写到 platforms/linux/web/
```

macOS App 是 `platforms/macos/app/` 下的 Swift Package。本机测试 DMG：

```bash
./scripts/build-macos-dmg.sh --clean
```

默认跑 Rust/Swift 测试，生成 `platforms/macos/app/dist/Sumpter-local.dmg`（ad-hoc 签名，不公证、不上传）。`--skip-tests` 可用于后续增量打包。

贡献约定见 [`AGENTS.md`](AGENTS.md)。开箱正文改 `docs/usage-onboarding.md` 和 `docs/usage-path-matrix.json`，再用 `scripts/sync-usage-docs.py` 同步根 `USAGE.md` 与 `platforms/linux/USAGE.md`。

独立 Linux 发布包会在发布阶段把 `platforms/linux/` 提升为包根；这不改变源码树目录规范。现有交叉构建、release preflight 和部分 macOS 打包脚本仍可能带独立发布树或旧 sidecar 命名假设，不能只凭根 workspace 测试通过就宣称 DMG / Linux 发布链已恢复。

## 文档

| 你想做什么 | 读哪份 |
|---|---|
| 安装、接客户端、排错 | [USAGE.md](USAGE.md) |
| 每个配置字段 | [platforms/macos/CONFIG.md](platforms/macos/CONFIG.md) |
| Linux 安装、systemd、Docker、Admin 反代 | [platforms/linux/README.md](platforms/linux/README.md) |
| macOS 首次打开被拦截 | [platforms/macos/app/INSTALL.txt](platforms/macos/app/INSTALL.txt) |
| 当前架构与变更归属 | [docs/architecture.md](docs/architecture.md) |
| 文档总索引 | [docs/README.md](docs/README.md) |
| Linux Admin API | [platforms/linux/specs/admin-api.md](platforms/linux/specs/admin-api.md) |
| 仓库协作规则 | [AGENTS.md](AGENTS.md) |

## 安全

- 真实 `apiKey`、`authToken`、Admin 密码、Cookie、请求体和 raw 捕获不要进 git、Issue 或聊天记录。
- 配置文件权限应为 `0600`。不要用仓库里的 example 当生产文件原地填 key。
- 代理先做 CIDR 与入站鉴权，再读请求体；不要用大 body 测试错误 token。
- 诊断导出默认脱敏；未脱敏导出可能含本地路径和其它归因元数据。
- Linux 公网暴露 Admin 时必须自己加 HTTPS 反代；默认只绑 loopback。
