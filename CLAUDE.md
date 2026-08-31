# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Sumpter 是本地优先的多协议 AI 代理：把 Claude Code / Codex / OpenAI 兼容客户端接到多个上游 Provider，按 mapping、优先级和会话粘性调度，失败时 failover，请求链写入本机 SQLite。**一份共享 Rust 引擎 + 两个平台 adapter**；macOS 是「菜单栏 App + Rust sidecar」，Linux 是「前台 daemon + Web Admin」。

协作规则、提交规范（Conventional Commits + crate scope）见 [`AGENTS.md`](AGENTS.md)；当前分层与变更归属见 [`docs/architecture.md`](docs/architecture.md)。

## 常用命令

### Rust workspace（仓库根，唯一 Cargo workspace）

Homebrew macOS 上 cargo 常不在默认 PATH：`export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`。

```bash
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
uv run scripts/sync-usage-docs.py --check      # 文档同步门禁
```

单测：

```bash
cargo test -p sumpter-core --test routing            # 单个集成测试文件
cargo test -p sumpter-core --test routing sticky      # 再按名字过滤
cargo test -p sumpter-engine --test replay_conformance
cargo test -p sumpter-linux-adapter --test engine     # 平台 boundary 注入后的行为
cargo test -p sumpter-macos-adapter --test engine
cargo test -p sumpter-runtime --lib retention        # crate 内单元测试按名字过滤
```

本地跑守护进程（不要指向真实配置目录）：

```bash
cargo run -p sumpterd-linux -- --config-dir /tmp/sumpter-dev --no-web
cargo run -p sumpterd-macos -- --config-dir /tmp/sumpter-dev --foreground
```

`--foreground` 只对 macOS sidecar 有意义（关闭 stdin EOF 监视）。可执行文件名是 `sumpterd-linux` / `sumpterd-macos`。

### Linux WebUI（`platforms/linux/webui/`）

```bash
npm ci
npm test                                  # node --test tests/*.test.mjs
node --test tests/runtime-pagination.test.mjs   # 单个契约测试
npm run build                             # 产物写到 ../web/
```

### macOS App（`platforms/macos/app/`，Swift Package）

```bash
swift build --package-path platforms/macos/app --product SumpterApp
swift test --package-path platforms/macos/app
swift test --package-path platforms/macos/app --filter CodexHookEditingTests
./scripts/build-macos-dmg.sh --clean      # 本机 ad-hoc DMG（默认先跑 Rust + Swift 测试）
```

`--skip-tests` 用于增量重打包。正式分发走 `platforms/macos/app/package-app.sh`（Developer ID + Sparkle + 公证）。

## 架构

依赖方向是单向的，不允许反向：

```text
apps/<platform>/sumpterd → adapters/<platform> → sumpter-engine → sumpter-runtime → sumpter-core
```

- `crates/sumpter-core/`：纯逻辑。配置 schema、路由规划、粘性调度、访问控制（CIDR）、协议桥接、事件契约。**不依赖网络、SQLite、OS、UI。**
- `crates/sumpter-runtime/`：SQLite worker、投影、rollup、查询、导出。只依赖 core + bundled SQLite。
- `crates/sumpter-engine/`：平台中立数据面。入站分发、鉴权、重试/failover、流式 relay、诊断捕获、健康评估、回放。
- `adapters/{linux,macos}/`：平台边界实现 + Admin facade + HTTP 组装。
- `apps/{linux,macos}/sumpterd/`：只做参数解析、配置目录、监听、信号/EOF 生命周期，**不复制请求处理逻辑**。
- `platforms/`：非 Rust 的平台产品输入（WebUI 源码、已构建静态资源、SwiftUI、安装与发布脚本）。

### 唯一的平台缝：`crates/sumpter-engine/src/boundary.rs`

`PlatformBoundary` 提供 `authorize_status`、`platform_action`、`handle_platform_action`、`validate_opened_capture`；`EngineServices` 注入上游 transport + boundary。引擎默认用 `NoopPlatform`，所以共享 crate 能在没有 OS 控制面时独立测试。**新增平台能力时扩这个 trait，不要在共享 crate 里 `#[cfg(target_os)]`。** workspace 没有平台 feature 矩阵，`--all-features` 不能替代两个 adapter 的 `--test engine`。

### 请求生命周期

`adapters/*/src/server.rs` → `Engine::handle_inbound_request`（`crates/sumpter-engine/src/engine/mod.rs`，5000+ 行，是数据面主体）：

1. `platform_action` 先截平台专属动作（macOS 通知/reload；Linux 返回 404），再处理共享的 `/__status`。
2. **协议由入站路径决定，不猜 User-Agent**：`/v1/messages`、`/v1/chat/completions`、`/v1/responses`（含 `/backend-api/codex/responses`）、`count_tokens`、compact、images、legacy completions。路径表在 `mod.rs` 的入站 dispatch `match` 里（搜 `"/v1/messages" =>`）。
3. CIDR + `listener.authToken` 鉴权先于读 body（`MAX_BODY_BYTES` = 64 MiB），拒绝的请求不会被迫分配 payload。
4. `RoutePlanner`（`core/routing.rs`）按 `mappings.clientPattern` + `featureRules` 选入口，产出 `PlannedEndpoint`（含 `source_format` / `protocol` / `RouteMode::Native|Translated`）。匹配不到任何启用入口 → 400。
5. 同一会话尽量粘在同一 `stickyGroup`；组内先按 `sessionStickyRetries` 重试，再换组，换组成功即改绑（CAS 规则在 `core/scheduler.rs`）。
6. 出站走 `outbound.rs`：pinned IP + SNI 分离（TCP 连 IP，SNI/证书校验/Host 用域名）、恒 TLS、不复用连接、http1 only、不跟随重定向、不走系统代理。
7. 需要转协议时走 `core/bridge.rs`（出站 Anthropic→OpenAI/Responses）与 `core/bridge_in.rs`（入站 OpenAI/Responses→Anthropic Messages）。
8. 事件记账 → runtime SQLite。客户端断连 → 响应体流 drop → 上游请求与重试循环立刻撕停，`CompletionGuard` 补记 499（不计成功也不计失败）。

### 不能随手改的契约

- **`config.json` schema v6**：顶层是扁平 `endpoints[]`，每个入口必须有 `mappings[]`；没有 `pools`。serde 字段名是 camelCase 契约；部分 `Option` 字段序列化成显式 `null`（对齐 Swift），不是可省略键。启动/热重载时 v3/v4/v5 自动备份后原子迁移（`core/config_store.rs`），`normalized()` 还会把旧池级模型规则改写成入口显式映射。含密钥的写入一律临时文件 + 0600 + 原子 rename。
- **`core/events.rs` 磁盘形状**：`timestamp` 是 Apple reference date（2001-01-01Z）秒数浮点；`None` 省略键；`id` 大写 UUID；键字母序。改结构体字段顺序或 serde 属性会破坏已落盘数据与两端 UI。
- **重试面**：`RetryPolicy::RETRYABLE_STATUS_CODES` 是常量、不可配置（401/402/403/429/5xx 网关族）。400 原样返回，不换入口。
- **runtime 热路径**：代理只更新内存快照并推入有界队列；SQLite 连接归 worker 线程，Admin 读用短连接，不碰代理状态锁。
- **Admin 面**：Linux 是 `/admin/api/*`（HttpOnly 会话 Cookie + CSRF + `/admin/api/events` SSE），凭据在 `<config-dir>/admin-password`，改密后变 Argon2 哈希 JSON；macOS 是握手返回的动态端口，鉴权是 loopback + `X-Control-Token`。
- **macOS sidecar 生命周期**：stdout 第一行是握手 JSON（`adminPort`/`proxyPort`/`pid`/`generation`），日志走 stderr；**stdin EOF = 父进程退出信号**。
- **Swift `SumpterCore` 是 App 侧的并行实现**（`Routing.swift`、`OpenAIBridge.swift`、`RuntimeModels.swift` 等）。改共享行为或 wire 字段时两边都要动，并跑 `RuntimeV2WireContractTests` 一类的契约测试。

## 约定与陷阱

- **`USAGE.md` 与 `platforms/linux/USAGE.md` 是生成物**：改 `docs/usage-onboarding.md` + `docs/usage-path-matrix.json`，再 `uv run scripts/sync-usage-docs.py --write`。
- **`platforms/linux/web/` 是构建产物**：改 UI 要动 `platforms/linux/webui/` 再 `npm run build`。
- **两端 UI 对齐**：同名页面的信息层级、字段命名、状态语义保持一致；只在原生控件/布局需要时留差异。改统计或运行页时先对照另一端。
- **根 workspace 通过 ≠ 发布链通过**：`platforms/linux/scripts/{assemble-shared-tree,cross-build,release-preflight}.sh` 和 `platforms/linux/.github/workflows/` 仍带独立发布树假设（发布时把 `platforms/linux/` 提升为包根）；根目录没有 `.github/`，这些 workflow 不会自动跑。
- **历史材料不是现行契约**：`docs/upstream/`（含旧 `kekulv-*` 路径）、`docs/code-review-2026-08-31.md`、`platforms/linux/CHANGELOG.md`。`plan.md` 和 `todos.md` 是按轮次追加的工作日志。
- 品牌名从历史 `kekulv` 硬切到 `sumpter` 的工作正在进行（见 `todos.md`）：配置目录、systemd 单元、环境变量 `SUMPTER_*`、header `X-Sumpter-*`、导出格式标识都以 `sumpter` 为目标。不要重新引入 `kekulv`，也不要改 `docs/upstream/` 和 CHANGELOG 里的历史条目。
- 真实 `apiKey`、`authToken`、Admin 密码、Cookie、请求体和 raw 捕获不进 git / Issue / 聊天记录。`config.example.json` 只放 `enabled: false` 的合成入口和 `.invalid` 主机。不要在仓库里的 example 上原地填 key。
- 用 bundled SQLite，不要为本地开发引入外部数据库。
