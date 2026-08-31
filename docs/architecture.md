# Sumpter 当前架构

本文是重构后的维护基线。目标是“一份共享引擎，多端适配器”，让 Linux 和 macOS
共享同一套数据面、协议桥接、重试和运行时存储实现；平台差异只在 adapter 和 app
层出现。

## 结构树

```text
.
├── Cargo.toml                         # 唯一 Rust workspace
├── Cargo.lock
├── crates/                            # 跨平台真源
│   ├── sumpter-core/                  # 配置、路由、调度、协议和事件契约
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
│   │   ├── src/server.rs               # Linux HTTP 组装和优雅关停
│   │   └── src/engine.rs               # 注入 Linux boundary 的薄 wrapper
│   └── macos/sumpter-macos-adapter/
│       ├── src/platform.rs            # macOS control token、通知、reload
│       ├── src/admin.rs                # macOS Admin facade
│       ├── src/server.rs               # macOS HTTP 组装
│       └── src/engine.rs               # 注入 macOS boundary 的薄 wrapper
├── apps/                              # 可执行入口，只做 composition
│   ├── linux/sumpterd/                # Linux daemon
│   └── macos/sumpterd/                # macOS sidecar
├── platforms/                         # 非 Rust 的平台产品输入
│   ├── linux/                         # Linux UI、发布、安装、systemd 输入
│   │   ├── webui/                     # WebUI 源码（本轮未修改）
│   │   ├── web/                       # 已构建静态资源（本轮未修改）
│   │   ├── scripts/                   # 构建/安装脚本（本轮仅修正路径）
│   │   ├── deploy/                    # service / 反代模板（本轮未修改）
│   │   ├── specs/                     # Linux Admin/API 规格
│   │   ├── integrations/              # 外部客户端集成示例
│   │   └── .github/workflows/         # Linux 独立发布 workflow 输入
│   └── macos/                         # SwiftUI 壳和打包输入（业务逻辑未修改）
│       └── app/                       # Swift Package、测试、DMG/Sparkle 入口
├── docs/                              # 当前说明和历史对照资料
└── scripts/                           # 仓库级辅助脚本
```

平台产品输入已集中到 `platforms/linux/` 与 `platforms/macos/`，不再把平台目录伪装成
根 workspace。Rust 真源只保留根 `crates/`、`adapters/` 和 `apps/`；不再存在嵌套 Rust
workspace，也不再维护两份 core/runtime/engine 或 legacy proxy 实现。独立 Linux 发布包
仍可在发布阶段把 `platforms/linux/` 的内容提升为发布根，但这不是源码树的目录约定。

## 依赖方向

```text
apps/sumpterd
    └── adapters/<platform>
            └── sumpter-engine
                    ├── sumpter-runtime
                    │       └── sumpter-core
                    └── sumpter-core
```

约束如下：

1. `sumpter-core` 不依赖网络、SQLite、操作系统或 UI。
2. `sumpter-runtime` 只依赖 core 和 bundled SQLite；不依赖 adapter 或 app。
3. `sumpter-engine` 只通过 `PlatformBoundary`、`EngineServices` 接收平台能力和上游传输；不直接引用 Linux/macOS crate。
4. adapter 可以依赖 shared crate，但 shared crate 不得反向依赖 adapter。
5. app 只负责参数解析、配置目录、监听启动、信号/EOF 生命周期和平台组合，不复制请求处理逻辑。

## 平台边界

`crates/sumpter-engine/src/boundary.rs` 是唯一边界入口：

- `authorize_status`：控制面 `/__status` 的平台访问策略；
- `platform_action`：声明平台专属动作（macOS 通知/reload，Linux 保持 404）；
- `handle_platform_action`：执行通知、reload 等副作用；
- `validate_opened_capture`：由平台提供文件身份校验；
- `EngineServices`：注入上游传输和 `PlatformBoundary`。

共享引擎默认使用 `NoopPlatform`，因此 core/runtime/engine 可以在没有操作系统控制面的
情况下独立测试。实际二进制分别由 Linux/macOS adapter 注入具体 `Platform`。

## 变更归属规则

| 需求 | 修改位置 | 不应修改 |
| --- | --- | --- |
| 配置、路由、协议桥接、事件契约 | `crates/sumpter-core/` | adapter/app 内复制实现 |
| SQLite 写入、投影、分页、导出 | `crates/sumpter-runtime/` | Linux/macOS 各维护一份 runtime |
| 入站、重试、relay、回放、健康检查 | `crates/sumpter-engine/` | 平台目录中的 proxy engine |
| Linux 权限、Admin、systemd 语义 | `adapters/linux/`、`apps/linux/` | shared engine 引入 Linux 依赖 |
| macOS control token、通知、sidecar 生命周期 | `adapters/macos/`、`apps/macos/` | shared engine 引入 Swift/macOS 依赖 |
| WebUI/SwiftUI/安装和发布 | `platforms/linux/`、`platforms/macos/` | 本轮不改 UI/业务逻辑；仅修正迁移造成的路径引用 |

已有 HTTP header、环境变量、服务单元名、导出格式标识和 sticky domain 属于兼容协议或
运行时数据字段。本轮只改工程包名、目录、workspace 和默认入口，不机械改动这些字段，
避免在“架构整理”中意外制造协议迁移。

## 构建与验证

在仓库根目录运行：

```bash
export PATH="/Users/kkl/.rustup/toolchains/stable-aarch64-apple-darwin/bin:$PATH"
cargo fmt --all -- --check
cargo check --workspace --locked
cargo test --workspace --locked
```

workspace 当前包含 7 个 Rust package：3 个 shared library、2 个 platform adapter、2 个
daemon/sidecar app。Linux WebUI、SwiftUI、安装脚本和发布包不属于这组 Cargo 测试；它们的
源代码和静态资源在本轮保持不变，后续如需修改应走各自的前端/打包验证流程。

注意：`platforms/linux/scripts/assemble-shared-tree.sh`、`cross-build.sh`、
`release-preflight.sh` 仍服务于独立 Linux 发布树，并按发布包根目录查找 `crates/`、
`kekulvd` 等输入；它们不是当前根 workspace 的通过证据。macOS `package-app.sh` 同样
仍有旧的发布 profile/sidecar 命名假设。后续应单独完成“发布链适配”任务，再恢复交叉构建、
DMG 和 release preflight 门禁。

## 品牌与兼容性说明

工程级 Rust package、目录和二进制入口统一为 `sumpter`/`Sumpter`。前端、SwiftUI、部署
脚本以及协议兼容字段仍保留原有内容，原因是本轮边界明确为架构整理而非 UI、业务或协议
迁移。若后续需要彻底替换用户可见品牌或协议字段，应单独建立迁移任务，提供兼容读取、
升级说明和回滚策略，不应与 workspace 搬迁混在同一个变更中。
