# Sumpter macOS Rust sidecar

Sumpter 的 Rust 后端 sidecar:本地 Anthropic 兼容代理引擎,由 SwiftUI 菜单栏壳
(`app/`)spawn 并通过 127.0.0.1 admin API 控制。共享引擎迁移记录见
`../../docs/upstream/engine-unification-plan.md`，当前镜像计划见 `../../plan.md`。

```bash
cargo build --manifest-path ../../Cargo.toml -p sumpterd-macos  # debug 构建
cargo test --manifest-path ../../Cargo.toml --workspace          # 全部测试
cargo test --manifest-path ../../Cargo.toml -p sumpter-core      # 单个 crate
cargo clippy --manifest-path ../../Cargo.toml --workspace --all-targets # lint
```

工具链经 Homebrew 的 rustup 安装(keg-only),cargo/rustc 不在默认 PATH:

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"   # 或写进 ~/.zshrc
```

## 入站 API

sidecar 支持 Claude `/v1/messages`，OpenAI Chat / Responses 的 Native Adapter 与安全
Translator，以及 Images Generations / Edits、Legacy Completions、Claude Count Tokens、
Responses Compact 与 Codex Alpha Search 独立 Adapter。Images 同时覆盖 OpenAI GPT Image 与
Grok Image 参数，JSON 未知字段会保留，multipart 编辑不会重建上传体。Responses WebSocket、
Realtime / Live、Videos、Files 仍不支持；完整路径、四态入口协议与迁移说明见根目录
[`USAGE.md`](../../USAGE.md#4-协议与路径)。

想让「统计」页按项目区分 Claude Code 请求，在跑 CC 的机器上运行
`cc-project-attribution.sh install`（配置器跨平台通用；打包后的 App 里在
`Sumpter.app/Contents/Resources/`，源码构建则是仓库的 `platforms/linux/scripts/`）。
App 的**安全**页有完整引导（当前状态、三步命令、平台差异、三个陷阱、回退），
也能直接在 Finder 里定位脚本。原理见
[`USAGE.md` §8](../../USAGE.md#8-让-claude-code-按项目统计可选)。

## crate 分层(依赖单向向下)

- `sumpter-core` — 纯逻辑:配置模型(schema v6,与 `platforms/linux/` 同源同语义)、
  路由、粘性调度、访问控制、协议桥接纯函数。无网络无平台依赖。
- `sumpter-engine` — tokio/axum/reqwest:入站服务、pinned-IP 出站、failover、
  SSE relay、统计事件、admin API。
- `sumpterd-macos` — 可执行入口:组装、进程生命周期(stdin EOF 随父进程退出)。
