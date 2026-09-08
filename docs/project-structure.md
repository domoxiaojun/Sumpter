# 项目结构

Sumpter 在一个仓库内维护共享 Rust 代理、Linux 服务与 macOS App。根 [Cargo.toml](../Cargo.toml) 管理 7 个 Rust package；WebUI 和 Swift App 分别使用自己的 npm 与 Swift Package 清单。

本文负责目录、源码与测试的定位。依赖约束和请求处理见 [架构说明](architecture.md)，环境、命令和资源同步见 [开发指南](development.md)。

## 目录地图

```text
.
├── Cargo.toml / Cargo.lock          # 唯一 Rust workspace 与依赖锁
├── config.example.json             # 两端共用的安全配置模板
├── crates/                         # 共享 Rust 实现
│   ├── sumpter-core/               # 配置、路由、调度、协议与事件模型
│   ├── sumpter-runtime/            # SQLite 存储、查询、聚合与导出
│   └── sumpter-engine/             # HTTP / WebSocket 管线、重试与 relay
├── adapters/                       # 平台能力、Admin API 与 HTTP 服务组装
│   ├── linux/sumpter-linux-adapter/
│   └── macos/sumpter-macos-adapter/
├── apps/                           # Rust 可执行入口
│   ├── linux/sumpterd/              # sumpterd-linux
│   └── macos/sumpterd/              # sumpterd-macos
├── platforms/                      # UI、平台资源、安装和打包输入
│   ├── linux/
│   │   ├── webui/                  # React / Vite 源码与前端测试
│   │   ├── web/                    # 受版本控制的前端构建产物
│   │   ├── scripts/                # 安装、运维、交叉构建与客户端资源
│   │   ├── deploy/                 # systemd 与反向代理模板
│   │   ├── specs/                  # Linux Admin API 文档
│   │   └── integrations/           # Scriptable / TSX 集成示例
│   └── macos/
│       ├── app/                    # Swift Package、测试、App 打包与更新
│       │   ├── Sources/SumpterApp/  # SwiftUI、AppModel、AdminClient 与资源
│       │   ├── Sources/SumpterCore/ # Swift 配置模型、界面逻辑与客户端配置辅助
│       │   └── Tests/              # Swift 测试
│       └── scripts/                # macOS 客户端脚本与同步副本
├── tests/contracts/                # 两个 adapter 复用的行为测试
├── scripts/
│   ├── check.sh                    # 统一检查入口
│   ├── build-macos-dmg.sh          # 本机 DMG 构建入口
│   ├── clients/                    # 共享客户端归因源码与生成的兼容入口
│   ├── maintenance/                # 文档、资源同步与源码导出
│   └── tests/                      # 仓库工具和客户端脚本测试
├── .github/                        # CI、发布工作流、Issue / PR 模板
└── docs/
    ├── README.md                   # 文档索引
    ├── *.md                        # 结构、架构、配置、开发与发布说明
    └── templates/                  # 文档生成输入
```

根目录保留 README、USAGE、CHANGELOG、LICENSE、贡献/安全政策和工具规则，便于仓库与分发工具直接发现。任务草稿（`todos.md` / `plan.md`）已忽略，不入库。

## 按需求找源码

| 需要修改的内容 | 入口 |
| --- | --- |
| 配置字段、迁移、模型组与路由 | [core/src](../crates/sumpter-core/src/) 的 `config.rs`、`config_store.rs`、`model_groups.rs`、`routing.rs` |
| 共享事件字段与调度策略 | [core/src](../crates/sumpter-core/src/) 的 `events.rs`、`scheduler.rs` |
| SQLite 写入、保留策略与统计查询 | [runtime/src](../crates/sumpter-runtime/src/) 的 `runtime_store/`、`runtime_query/` |
| 入站协议、上游转发、重试与流式响应 | [engine/src/engine](../crates/sumpter-engine/src/engine/)；传输接口与请求构造在其上层 `outbound.rs`、`request_build.rs` |
| Linux Admin、登录、系统服务与 HTTP 组装 | [Linux adapter](../adapters/linux/sumpter-linux-adapter/src/)；命令行启动参数在 [Linux app](../apps/linux/sumpterd/src/main.rs) |
| macOS Admin、control token、通知与 sidecar 组装 | [macOS adapter](../adapters/macos/sumpter-macos-adapter/src/) 与 [macOS app 入口](../apps/macos/sumpterd/src/main.rs) |
| Web 页面、组件、API 请求与样式 | [webui/src](../platforms/linux/webui/src/) 的 `pages/`、`components/`、`services/`、`styles/` |
| SwiftUI 页面、App 状态、sidecar 管理与客户端配置 | [SumpterApp](../platforms/macos/app/Sources/SumpterApp/) 的 `UI/`、`AppModel+*.swift`、`SidecarController.swift`；Swift 模型及辅助逻辑在 [SumpterCore](../platforms/macos/app/Sources/SumpterCore/) |
| 共享客户端归因与副本同步 | [scripts/clients](../scripts/clients/) 与 [脚本目录说明](../scripts/README.md) |
| 安装、打包、版本发布 | 平台目录的 `scripts/`、macOS `app/package-app.sh` 和根 [.github/workflows](../.github/workflows/)；流程见 [发布指南](releasing.md) |

Swift 的 `SumpterCore` 是 App 使用的独立 Swift 模块。代理的共享 Rust 实现位于 `crates/`。配置或事件字段跨端变化时，还要检查 Swift 模型、WebUI 数据转换和两个 adapter 的接口契约。

## 测试归属

| 测试范围 | 位置与执行关系 |
| --- | --- |
| 共享模块单元测试 | Rust 源码中的 `#[cfg(test)]`；engine/runtime 拆分后的测试也放在对应模块内 |
| 配置、路由与回放集成测试 | [core/tests](../crates/sumpter-core/tests/)、[engine/tests](../crates/sumpter-engine/tests/) |
| 平台 HTTP / WebSocket 行为 | [Linux adapter/tests](../adapters/linux/sumpter-linux-adapter/tests/) 与 [macOS adapter/tests](../adapters/macos/sumpter-macos-adapter/tests/) |
| 跨平台公共行为 | [tests/contracts](../tests/contracts/) 通过 `#[path]` 引入两个 adapter 的测试目标，不是独立 Cargo package |
| WebUI / Swift App | [webui/tests](../platforms/linux/webui/tests/) 与 [App Tests](../platforms/macos/app/Tests/) |
| 仓库、同步与客户端工具 | [scripts/tests](../scripts/tests/)；Linux 安装和归因自测保留在 [平台 scripts](../platforms/linux/scripts/) |

验证命令统一维护在 [开发指南](development.md#按变更选择验证)，按受影响模块选择；Rust workspace 测试不包含 npm、Swift 或安装包验收。

## 维护源、生成副本与缓存

- `platforms/linux/webui/` 是 WebUI 维护源，`platforms/linux/web/` 是必须随源码提交的生成资源。
- 根 `LICENSE`、`CHANGELOG.md` 和 `config.example.json` 维护公共内容；平台内相应文件由元数据工具同步。
- `docs/templates/` 生成根和 Linux 包内 USAGE 的标记块；标记块以外的内容在各自文档维护。
- `scripts/clients/` 中的共享归因程序与 pi 扩展会同步到 Linux、macOS 和 Swift 资源目录；Gemini 兼容入口也由工具生成。平台安装器和 Shell 脚本按 [脚本目录说明](../scripts/README.md) 维护。
- `target/`、`node_modules/`、Swift `.build/` 和各平台 `dist/` 是本机依赖、缓存或打包产物，不进入源码版本管理。真实配置、日志和运行数据库也不放入源码。

具体的源文件、目标文件与同步命令只维护在 [生成副本说明](development.md#单一维护源与生成副本)。

## 源码目录与安装包

开发从仓库根执行 Cargo 和统一检查命令。`platforms/linux/` 提供打包输入，构建脚本挑选二进制、静态资源、脚本和文档组成 Linux 发布包；包内 `scripts/`、`web/` 相对包根，二进制名是 `sumpterd`。

macOS 由 `platforms/macos/app/` 的 Swift Package 和根 workspace 的 Rust sidecar 组成 App，客户端资源进入 `Sumpter.app/Contents/Resources/`。目录整理时保留安装路径、listener 下载路径与包内文件名；调整维护源位置时一并核对同步工具、打包脚本和测试引用。
