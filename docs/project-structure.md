# 项目结构

Sumpter 是一个 Rust workspace，Linux、macOS 和 WebUI 共享协议与配置约定。

```text
crates/
  sumpter-core/       配置、模型名、路由规划、协议纯函数
  sumpter-runtime/    SeaORM + bundled SQLite 运行时存储与查询
  sumpter-engine/     HTTP/WebSocket 转发、重试、会话、事件与回放
adapters/
  linux/              Linux Admin、systemd 与平台服务组合
  macos/              macOS sidecar 与平台服务组合
apps/                 sumpterd-linux / sumpterd-macos 可执行入口
platforms/linux/      WebUI 源码、生成静态文件、安装、systemd、Compose
platforms/macos/      SwiftUI App、DMG、Sparkle、安装资源
scripts/              检查、同步、归因、构建辅助
 docs/                现行指南和共享模板
```

## 按需求找代码

| 需求 | 先看 |
| --- | --- |
| 配置字段、迁移、模型映射 | `crates/sumpter-core/src/config.rs`、`config_store.rs` |
| 路由、优先级、粘性、重试 | `crates/sumpter-core/src/routing.rs`、`crates/sumpter-engine/src/engine/` |
| 请求 raw 透传与协议路径 | `crates/sumpter-engine/src/engine/protocol.rs`、`request_build.rs` |
| SQLite 事件和统计 | `crates/sumpter-runtime/src/` |
| Linux Admin API | `adapters/linux/sumpter-linux-adapter/src/admin*.rs` |
| macOS sidecar / UI | `platforms/macos/app/Sources/` |
| Linux WebUI | `platforms/linux/webui/src/`；生成文件为 `platforms/linux/web/` |
| 安装、升级、systemd | `platforms/linux/scripts/`、`platforms/linux/deploy/` |
| Compose 镜像和模板 | `platforms/linux/compose.yaml`、`Dockerfile*` |
| 客户端项目归因 | `scripts/clients/`，再同步到平台资源 |

`tests/contracts/` 通过 `#[path]` 被两个 adapter 测试目标复用，不是独立 Cargo package。`platforms/linux/webui/` 是 WebUI 维护源，`platforms/linux/web/` 是需提交的构建产物；不要手工改生成文件。

## 依赖方向

应用入口依赖平台 adapter；adapter 依赖 shared engine；engine 依赖 runtime 和 core。shared crate 不依赖 Linux、macOS、WebUI 或系统服务。平台差异通过 `PlatformBoundary`、`EngineServices` 注入。

## 文件边界

真实配置、密码、数据库、日志、构建目录、诊断捕获和发布产物不入 Git。`docs/templates/usage-onboarding.md` 是两份使用手册的共享维护源；根 `config.example.json` 是两端配置模板源；运行 `./scripts/check.sh docs` 检查同步。

发布包把 `platforms/linux/` 提升为包根，因此包内路径和源码树路径不同；发布包二进制名为 `sumpterd`，源码构建目标名为 `sumpterd-linux`。源码、构建、安装、发布和生产部署是不同层次，验证其中一层不能替代另一层。
