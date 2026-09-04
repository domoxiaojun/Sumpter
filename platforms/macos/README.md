# Sumpter macOS

macOS 产品是 SwiftUI 菜单栏 App + Rust sidecar。App 源码在 `app/`；sidecar 由根 workspace 的 `sumpterd-macos` 提供，经 `adapters/macos` 注入平台边界，通过 `127.0.0.1` Admin API 与 App 通信。

当前架构见 [`docs/architecture.md`](../../docs/architecture.md)。重构前的双端对照在 `docs/upstream/`，不是现行契约。

## 构建

工具链经 Homebrew 的 rustup 安装（keg-only）时，cargo / rustc 不在默认 PATH：

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

cargo build --manifest-path ../../Cargo.toml -p sumpterd-macos
cargo test --manifest-path ../../Cargo.toml --workspace
cargo test --manifest-path ../../Cargo.toml -p sumpter-core
cargo clippy --manifest-path ../../Cargo.toml --workspace --all-targets -- -D warnings
```

本机测试 DMG（仓库根）：

```bash
./scripts/build-macos-dmg.sh --clean
```

产物默认是 ad-hoc 签名的 `platforms/macos/app/dist/Sumpter-local.dmg`，不公证、不上传。正式分发见 [`app/UPDATE.md`](app/UPDATE.md)。首次打开被拦截见 [`app/INSTALL.txt`](app/INSTALL.txt)。

## 入站 API

sidecar 支持 Claude `/v1/messages`，OpenAI Chat / Responses 的 Native Adapter 与安全 Translator，以及 Images Generations / Edits、Legacy Completions、Claude Count Tokens、Responses Compact、Codex Alpha Search、Responses WebSocket、Realtime/Live、Files、Videos 和 `/v1/models`。`GET /v1/models` 按本地 mapping 生成目录（Codex `client_version` 返回 `{models:[...]}`），不转发到上游。其余资源 HTTP 与 WebSocket 协议只由 sidecar 做鉴权、Provider 选择和 relay，原始 path/query、multipart、二进制响应及 WebSocket 帧交给上游；Codex Live bootstrap（`POST /v1/live`、`POST /v1/realtime`、`POST /v1/realtime/calls`）会将 SDP/multipart 封装为 quicksilver JSON（默认模型 `gpt-live-1-codex`），并把 POST `/v1/realtime` 出站改写到 `/v1/realtime/calls?intent=quicksilver&architecture=avas`；无 `call_id` 的 `GET /v1/realtime` 仍是公开 Realtime WebSocket，出站会去掉这些 WebRTC query。Realtime/Live WebSocket 会先完成上游握手再向客户端返回 `101`，ephemeral client-secret 的 session 配置会继续用于 `session.update` 和后续 calls 请求。完整路径与四态入口协议见根目录 [`USAGE.md`](../../USAGE.md#4-协议与路径)。Provider 的实际权限和媒体/Realtime 能力仍需目标上游实测。

想让「统计」页按项目区分 Claude Code 请求，在跑 CC 的机器上运行 `cc-project-attribution.sh install`。打包后的 App 里脚本在 `Sumpter.app/Contents/Resources/`，源码树则是 `platforms/macos/scripts/`。App 的**安全**页有完整引导。原理见 [`USAGE.md` §8](../../USAGE.md#8-让-claude-code-按项目统计可选)。

## 统一通知

通知设置同时支持 Claude Code、Codex CLI 与 Grok Build，并共用一套总开关、通知类别、系统授权、声音和测试入口。各客户端也可以单独「安装配置 / 移除配置」；总开关一次装上或卸掉全部三路。Claude 的 Hook 写入 `~/.claude/settings.json`；Codex 的 Hook 写入 `CODEX_HOME/hooks.json`（未设置时 `~/.codex/hooks.json`），同一个 `hooks/sumpter-codex-notify.zsh` 脚本覆盖 `PermissionRequest`、`Stop`、`SubagentStop`、`Interrupt` 四个有用户价值的生命周期事件；Grok 写入独占的 `$GROK_HOME/hooks/sumpter-notify.json`（默认 `~/.grok/hooks/`），覆盖 `Notification`（`permission_prompt` / `idle_prompt` / `task_complete`）、`Stop`、`StopFailure`、`StopCancelled`、`SubagentStop`。不改 `~/.grok/config.toml`，也不安装 Grok 终端 OSC `[ui.notifications.hooks]`。普通工具、压缩和会话生命周期事件不默认弹系统通知，避免噪声。Codex 写入后仍需在 Codex CLI 执行 `/hooks` 信任，收到真实 SSE 后才会显示「已验证」。Grok 全局 hook 默认受信任，无需再跑 `/hooks`。Codex 与 Grok 通知只使用 Sumpter 生成的固定安全文案，不转发 transcript、prompt、`lastAssistantMessage`、工具参数或原始错误详情。Grok 的 `Stop` 只作观察，脚本不打印 JSON，避免挡住回合结束。

启用 Codex 通知时，App 会直接移除 `config.toml` 中已知的 `SkyComputerUseClient … turn-ended` legacy `notify`，不保留备份也不自动恢复；自定义或无法安全解析的 legacy 配置会保留并显示冲突。Claude、Codex 与 Grok 的通知线程按客户端来源隔离。若 Grok 因 `compat.claude.hooks` 扫到 Claude 的 notify 脚本，脚本会把 `GROK_*` 环境改标为 `clientKind=grok_build`，避免横幅写成 Claude。Linux daemon 不提供通知 Hook 和 `/__notify`。

## 分层

依赖单向向下，与根 workspace 一致：

- `sumpter-core` — 配置模型（schema v6）、路由、粘性调度、访问控制、协议桥接纯函数。无网络、无平台依赖。
- `sumpter-runtime` — 共享 SQLite 事件存储与查询。
- `sumpter-engine` — 入站服务、pinned-IP 出站、failover、SSE relay、统计事件。平台能力只通过 `PlatformBoundary` 注入。
- `sumpter-macos-adapter` — control token、通知、reload、Admin facade、HTTP 组装。
- `sumpterd-macos` — 可执行入口：组装、握手 JSON、stdin EOF 随父进程退出。
