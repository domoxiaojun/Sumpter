# Sumpter 当前架构

维护基线是「一份共享引擎，多端适配器」：Linux 和 macOS 共用同一套数据面、请求透传、模型映射、重试和运行时存储；平台差异只出现在 adapter、app 和 `platforms/`。Codex `/v1/live` 的 SDP/multipart bootstrap 是共享引擎内唯一的 quicksilver 封装特例，公开 `/v1/realtime` 仍保持原生透传。

## 结构树

```text
.
├── Cargo.toml                         # 唯一 Rust workspace
├── Cargo.lock
├── crates/                            # 跨平台真源
│   ├── sumpter-core/                  # 配置、路由、调度、模型映射和事件契约
│   ├── sumpter-runtime/               # SQLite worker、投影、rollup、查询、导出
│   └── sumpter-engine/                # HTTP 数据面、转发、重试、relay、回放
│       └── src/
│           ├── boundary.rs            # PlatformBoundary / EngineServices
│           ├── engine/                # 入站、转发、事件、捕获、生命周期
│           ├── outbound.rs
│           ├── request_build.rs
│           ├── replay.rs
│           └── health.rs
├── adapters/                          # 平台实现，不承载共享业务逻辑
│   ├── linux/sumpter-linux-adapter/
│   │   ├── src/platform.rs            # Linux 权限、状态和平台动作
│   │   ├── src/admin*.rs              # Linux Admin facade
│   │   ├── src/server.rs              # Linux HTTP 组装和优雅关停
│   │   └── src/engine.rs              # 注入 Linux boundary 的薄 wrapper
│   └── macos/sumpter-macos-adapter/
│       ├── src/platform.rs            # macOS control token、通知、reload
│       ├── src/admin.rs               # macOS Admin facade
│       ├── src/server.rs              # macOS HTTP 组装
│       └── src/engine.rs              # 注入 macOS boundary 的薄 wrapper
├── apps/                              # 可执行入口，只做 composition
│   ├── linux/sumpterd/                # sumpterd-linux
│   └── macos/sumpterd/                # sumpterd-macos
├── platforms/                         # 非 Rust 的平台产品输入
│   ├── linux/                         # WebUI、安装、systemd、Docker、发布输入
│   │   ├── webui/                     # WebUI 源码
│   │   ├── web/                       # 已构建静态资源
│   │   ├── scripts/                   # 安装 / 交叉构建 / 发布输入
│   │   ├── deploy/                    # service / 反代模板
│   │   ├── specs/                     # Linux Admin API 规格
│   │   ├── integrations/              # 外部客户端集成示例
│   │   └── .github/workflows/         # 独立 Linux 发布树 workflow 输入
│   └── macos/                         # SwiftUI 壳、脚本和打包输入
│       ├── scripts/                   # macOS 随 App 内置的客户端配置脚本
│       └── app/                       # Swift Package、测试、DMG / Sparkle
├── .github/workflows/                 # 当前 monorepo 的 CI / Release / Container workflows
├── docs/                              # 当前说明和历史对照
└── scripts/                           # 仓库级辅助脚本
```

Rust 真源只在根 `crates/`、`adapters/` 和 `apps/`。没有嵌套 Rust workspace，也没有第二份 core / runtime / engine 或 legacy proxy。独立 Linux 发布包可以在发布阶段把 `platforms/linux/` 提升为包根，这不是源码树的目录约定。

## 依赖方向

```text
apps/<platform>/sumpterd
    └── adapters/<platform>
            └── sumpter-engine
                    ├── sumpter-runtime
                    │       └── sumpter-core
                    └── sumpter-core
```

约束：

1. `sumpter-core` 不依赖网络、SQLite、操作系统或 UI。
2. `sumpter-runtime` 只依赖 core 和 bundled SQLite；不依赖 adapter 或 app。
3. `sumpter-engine` 只通过 `PlatformBoundary`、`EngineServices` 接收平台能力和上游传输；不直接引用 Linux / macOS crate。
4. adapter 可以依赖 shared crate，shared crate 不得反向依赖 adapter。
5. app 只负责参数解析、配置目录、监听启动、信号 / EOF 生命周期和平台组合，不复制请求处理逻辑。

## 平台边界

`crates/sumpter-engine/src/boundary.rs` 是唯一边界入口：

- `authorize_status`：控制面 `/__status` 的平台访问策略；
- `platform_action`：声明平台专属动作（macOS 通知 / reload，Linux 保持 404）；
- `handle_platform_action`：执行通知、reload 等副作用；
- `validate_opened_capture`：由平台提供文件身份校验；
- `EngineServices`：注入上游传输和 `PlatformBoundary`。

共享引擎默认使用 `NoopPlatform`，因此 core / runtime / engine 可以在没有操作系统控制面时独立测试。实际二进制由对应 adapter 注入具体 `Platform`。

对应页面优先保持同一信息层级、字段命名、状态语义和主要交互；只有原生控件、窗口形态或平台生命周期确有差异时才保留平台化表现。运行统计存储统一使用 `maxAgeDays` 与 `storageLimitBytes` 的 OR 轮换语义，进行中的请求组整体保护；低频技术字段进入详情，策略编辑使用 Linux 弹窗 / macOS sheet。

## 变更归属

| 需求 | 修改位置 | 不应修改 |
| --- | --- | --- |
| 配置、路由、模型映射、事件契约 | `crates/sumpter-core/` | adapter / app 内复制实现 |
| SQLite 写入、投影、分页、导出 | `crates/sumpter-runtime/` | Linux / macOS 各维护一份 runtime |
| 入站、重试、relay、回放、健康检查 | `crates/sumpter-engine/` | 平台目录中的 proxy engine |
| Linux 权限、Admin、systemd、listener 内置脚本 | `adapters/linux/`、`apps/linux/` | shared engine 引入 Linux 依赖 |
| macOS control token、通知、sidecar 生命周期 | `adapters/macos/`、`apps/macos/` | shared engine 引入 Swift / macOS 依赖 |
| WebUI / SwiftUI / 安装和发布输入 | `platforms/linux/`、`platforms/macos/` | 把 UI 逻辑搬进 shared crate |

HTTP header、环境变量、服务单元名、导出格式标识和 sticky domain 属于兼容协议或运行时数据字段。改工程包名或目录时不要机械替换这些字段。

## 构建与验证

在仓库根目录：

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"

cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
uv run scripts/sync-usage-docs.py --check
```

workspace 有 7 个 Rust package：3 个共享库、2 个平台 adapter、2 个 daemon / sidecar。Linux WebUI、SwiftUI、安装脚本和发布包不在这组 Cargo 测试里，改动后走各自的前端 / 打包验证。

本机测试 DMG：`./scripts/build-macos-dmg.sh`。该脚本走 monorepo 根 workspace，产物是 ad-hoc 签名的 `Sumpter-local.dmg`。

发布输入分为当前 monorepo 根 workflow 与独立 Linux 发布树兼容脚本：

- `platforms/linux/scripts/cross-build.sh` 已识别根 workspace，可从仓库根构建 `sumpterd-linux` 并生成双架构包；`assemble-shared-tree.sh` 与 `release-preflight.sh` 仍主要服务独立 Linux 发布树。
- 当前 monorepo 的 GitHub workflow 在根 `.github/workflows/`；`platforms/linux/.github/workflows/` 仅保留独立 Linux 发布树输入，GitHub 不会发现该嵌套目录。
- `platforms/macos/app/package-app.sh` 已识别 monorepo 根 workspace 并构建 `sumpterd-macos`，但仍保留独立发布树分支和 `ENGINE_PROFILE` 兼容选项。

## 品牌与运行时名字

工程包、macOS App、Linux 配置目录、systemd 单元、发布包二进制、环境变量、入站 header 和导出格式统一为 `sumpter` / `Sumpter`：

- 配置：`~/.config/sumpter`、`/var/lib/sumpter`、`/opt/sumpter`
- 二进制与单元：`sumpterd`、`sumpter.service`
- 环境变量 `SUMPTER_*` / `SUMPTERD_*`
- 入站 header `X-Sumpter-*`
- 导出格式 `sumpter-session-export-v1`
- 粘性域 `sumpter-sticky-v3`

旧的 `kekulv` 路径、单元、header 和环境变量不再识别。已有安装需要卸载后重装，不要做原地双读迁移。
