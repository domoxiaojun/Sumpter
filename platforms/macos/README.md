# Sumpter macOS

macOS 产品是 SwiftUI 菜单栏 App + Rust sidecar，配置为 schema v7，版本与仓库根 [Cargo.toml](../../Cargo.toml) 一致。App 源码在 `app/`；sidecar 由根 workspace 的 `sumpterd-macos` 提供，经 `adapters/macos` 注入平台边界，通过 `127.0.0.1` Admin API 与 App 通信。

正式包从 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest) 下载 `sumpter-macos-*.dmg`。当前自动发布为 Apple Silicon、ad-hoc 签名、无公证。用户安装见 [`app/INSTALL.txt`](app/INSTALL.txt)，开箱见 [使用指南](../../USAGE.md)。

源码与测试位置见 [项目结构](../../docs/project-structure.md)，开发环境见 [开发指南](../../docs/development.md)，共享依赖与平台边界见 [架构说明](../../docs/architecture.md)。

## 构建

工具链经 Homebrew 的 rustup 安装（keg-only）时，cargo / rustc 不在默认 PATH：

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

# 从仓库根执行
./scripts/check.sh rust
./scripts/check.sh macos
cargo build --locked -p sumpterd-macos
```

本机测试 DMG（仓库根）：

```bash
./scripts/build-macos-dmg.sh --clean
```

产物默认是 ad-hoc 签名的 `platforms/macos/app/dist/Sumpter-local.dmg`，不公证、不上传。正式分发见 [`app/UPDATE.md`](app/UPDATE.md)。首次打开被拦截见 [`app/INSTALL.txt`](app/INSTALL.txt)。

## 入站 API

sidecar 使用共享引擎处理 HTTP、流式响应和 WebSocket。完整路径、协议转换、模型目录及 Realtime/Live 特例统一见 [使用指南](../../USAGE.md#4-协议与路径)，入口协议字段见 [配置说明](../../docs/configuration.md#入站协议与入口五态)。Provider 的实际权限和媒体/Realtime 能力仍需目标上游实测。

想让「统计」页按项目区分 Claude Code / Grok / Gemini / Codex CLI/TUI / pi 请求，在 App 的**安全**页选择客户端并安装配置。打包后的统一安装器在 `Sumpter.app/Contents/Resources/`。原理见 [客户端项目归因](../../USAGE.md#8-让-claude-code--grok-build-按项目统计可选)。

## 统一通知

通知设置同时支持 Claude Code、Codex CLI 与 Grok Build，并共用一套总开关、通知类别、系统授权、声音和测试入口。各客户端也可以单独「安装配置 / 移除配置」；总开关一次装上或卸掉全部三路。Claude 的 Hook 写入 `~/.claude/settings.json`；Codex 的 Hook 写入 `CODEX_HOME/hooks.json`（未设置时 `~/.codex/hooks.json`），同一个 `hooks/sumpter-codex-notify.zsh` 脚本覆盖 `PermissionRequest`、`Stop`、`SubagentStop`、`Interrupt` 四个有用户价值的生命周期事件；Grok 写入独占的 `$GROK_HOME/hooks/sumpter-notify.json`（默认 `~/.grok/hooks/`），覆盖 `Notification`（`permission_prompt` / `idle_prompt` / `task_complete`）、`Stop`、`StopFailure`、`StopCancelled`、`SubagentStop`。不改 `~/.grok/config.toml`，也不安装 Grok 终端 OSC `[ui.notifications.hooks]`。普通工具、压缩和会话生命周期事件不默认弹系统通知，避免噪声。Codex 桌面在用户配置打开钩子即可；CLI 若从未信任过再运行 `/hooks`。Hook 已写入即显示「已配置」，收到一次真实通知后升为「已验证」。Grok 全局 hook 默认受信任，无需再跑 `/hooks`。Codex 与 Grok 通知只使用 Sumpter 生成的固定安全文案，不转发 transcript、prompt、`lastAssistantMessage`、工具参数或原始错误详情。Grok 的 `Stop` 只作观察，脚本不打印 JSON，避免挡住回合结束。

启用 Codex 通知时，App 会直接移除 `config.toml` 中已知的 `SkyComputerUseClient … turn-ended` legacy `notify`，不保留备份也不自动恢复；自定义或无法安全解析的 legacy 配置会保留并显示冲突。Claude、Codex 与 Grok 的通知线程按客户端来源隔离。若 Grok 因 `compat.claude.hooks` 扫到 Claude 的 notify 脚本，脚本会把 `GROK_*` 环境改标为 `clientKind=grok_build`，避免横幅写成 Claude。Linux daemon 不提供通知 Hook 和 `/__notify`。

## Gemini CLI

推荐在 App「安全」页用统一安装器接入 Gemini。资源内也内置 `gemini-sumpter-wrapper.mjs`：设置 `SUMPTER_GEMINI_BASE_URL` 与 `SUMPTER_AUTH_TOKEN` 后用 `node` 调用；新会话自动带稳定的 `--session-id` 和 `X-Sumpter-Session-Id`，项目名默认取 Git 根目录名（非 Git 目录取当前目录名），可通过 `SUMPTER_GEMINI_PROJECT` 覆盖。Gemini 使用 Developer API 原生路径，Vertex、OAuth、Service Account 和 Code Assist 不在范围内。
