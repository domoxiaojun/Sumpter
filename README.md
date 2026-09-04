# Sumpter

本地优先的 AI 请求代理。把 Claude Code、Codex 和其它客户端接到多个上游 Provider，按模型 mapping、优先级和粘性分组调度，失败时 failover，并把请求链记在本机 SQLite。

当前版本 **0.3.4**（schema v6）。Rust 数据面只有一份共享实现；Linux 与 macOS 通过各自 adapter 接入。

源码仓库：[domoxiaojun/sumpter](https://github.com/domoxiaojun/sumpter) · 许可证 MIT

## 它解决什么

客户端只认一个本机 Base URL。Sumpter 在本机把请求路由到你配置的入口：

- 默认数据面 `http://127.0.0.1:57878`
- 入口是扁平的 `endpoints[]`，每个入口必须用 `mappings[]` 声明承接的客户端模型
- 同一次会话尽量粘在同一 `stickyGroup`，可重试故障后再换组
- 数据面通常不重建协议：任意 HTTP 方法/路径都按原始请求透传给选定 Provider；`GET /v1/models` 按 mapping 生成本地目录，Codex Live POST bootstrap 是 quicksilver 封装特例；WebSocket Upgrade 统一双向 relay
- 密钥、Admin 密码、请求体和诊断捕获只留在本机受限文件里

OpenAI 兼容客户端的 Files/Videos 资源查询与下载、Responses WebSocket 和无 `call_id` 的
`GET /v1/realtime` 都走同一条透传链。`GET /v1/models` 按本地 mapping 生成目录，不转发到上游；Codex 带 `client_version` 时返回 `{models:[...]}`。`POST /v1/live`、`POST /v1/realtime` 和 `POST /v1/realtime/calls` 是 Codex Live 特例：Sumpter 会把
SDP/multipart 封装为 quicksilver JSON 后交给配置的 Live mapping（默认 `gpt-live-1-codex`），并把 POST `/v1/realtime` 出站改写到 `/v1/realtime/calls?intent=quicksilver&architecture=avas`。无 `call_id` 的 GET `/v1/realtime` 会去掉这些 WebRTC query。
Realtime/Live WebSocket 会先完成上游握手再向客户端返回 `101`；ephemeral client-secret
返回的 session 配置会继续用于 `session.update` 和后续 calls 请求。
除此之外只做入站鉴权、Provider 选择、已配置模型 mapping 的必要替换、failover/retry 和连接
relay；上游是否真正提供相应能力仍取决于你配置的 Provider。

## 两端产品

| | macOS | Linux |
|---|---|---|
| 形态 | 菜单栏 App + Rust sidecar | 前台 daemon + Web Admin |
| 配置 | `~/Library/Application Support/Sumpter/config.json` | 普通用户：`~/.config/sumpter/config.json`；system 安装：`/var/lib/sumpter/config.json` |
| 管理界面 | App 设置窗 | 浏览器 `http://127.0.0.1:57879/admin/` |
| 入站鉴权 | `listener.authToken`（非空才启用） | 同左 |
| 热重载 | App 里保存 / 重启 | WebUI 保存，或 `SIGHUP` |
| 平台能力 | Claude Code / Codex / Grok Build 系统通知 | systemd、静态 musl 包、Docker |

Linux 配置目录、systemd 单元、发布包二进制和协议 header 现已统一为 `sumpter` / `Sumpter`（例如 `~/.config/sumpter`、`sumpter.service`、`X-Sumpter-*`）。旧的 `kekulv` 名字不再识别，需卸载后重装。

## 快速开始

完整开箱、客户端接入和排错见 **[USAGE.md](USAGE.md)**。第一次只需：安装/启动 → 配置一个 Provider → 连接客户端 → 发一条请求。

### macOS

打开 DMG，双击「安装Sumpter.command」。细节与 Gatekeeper 处理见 [`platforms/macos/app/INSTALL.txt`](platforms/macos/app/INSTALL.txt)。

### Linux

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh https://sf.domob.org/kkl/sumpter-install.sh
bash /tmp/sumpter-install.sh
```

`sudo bash` 会装成 system 服务（daemon 仍以低权限 `sumpter` 用户运行）。Docker、systemd、反代和卸载见 [`platforms/linux/README.md`](platforms/linux/README.md)。

### 接客户端

```bash
# Claude Code
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878

# Codex / 其它 OpenAI 兼容客户端
# Codex 的 Base URL 必须带 /v1
export OPENAI_BASE_URL=http://127.0.0.1:57878/v1
```

模型名必须能被已启用入口的 `mappings.clientPattern` 接住，否则返回 400。`listener.authToken` 非空时，客户端 API key 使用同一个值。

配置模板抄 `platforms/linux/config.example.json`（入口默认关闭，域名是 `.invalid`）。字段说明见 [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md)（两端同一份 schema）。

## 仓库结构

```text
crates/                 共享 core / runtime / engine
adapters/linux/         Linux 平台边界、Admin facade、server 组装
adapters/macos/         macOS 平台边界、sidecar facade、通知
apps/linux/             sumpterd-linux 可执行入口
apps/macos/             sumpterd-macos 可执行入口
platforms/linux/        WebUI、静态资源、systemd、安装与 Linux 发布输入
platforms/macos/        SwiftUI App、客户端脚本、图标、Sparkle 与 DMG 发布输入
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
| 配置、路由、模型映射、事件契约 | `crates/sumpter-core/` |
| SQLite 写入、投影、分页、导出 | `crates/sumpter-runtime/` |
| 入站、重试、relay、回放、健康检查 | `crates/sumpter-engine/` |
| Linux 权限、Admin、systemd 语义 | `adapters/linux/`、`apps/linux/` |
| macOS 控制通道、通知、sidecar 生命周期 | `adapters/macos/`、`apps/macos/` |
| WebUI / SwiftUI / 安装与发布输入 | `platforms/linux/`、`platforms/macos/` |

`docs/upstream/` 是迁移对照，不是当前 API 契约。`config.example.json` 只放禁用的合成入口和 `.invalid` 主机。

## 开发

这是 Rust 2024 workspace，要求 Rust 1.88+。代码结构、开发约定和文档同步方式见
[`AGENTS.md`](AGENTS.md)。

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
- 代理先做 CIDR 与入站鉴权，再读请求体；错误 token 请求请使用小请求体。
- 诊断导出默认脱敏；未脱敏导出可能含本地路径和其它归因元数据。
- Linux 公网暴露 Admin 时必须自己加 HTTPS 反代；默认只绑 loopback。
