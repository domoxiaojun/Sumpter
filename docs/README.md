# Sumpter 文档索引

这里的 `upstream/` 是从历史源码快照复制的参考文档，文件内容保持原样，不是当前 API 契约。
当前 Rust 代码使用根目录唯一 workspace；Linux/macOS 通过 adapter 注入平台能力。

## 当前镜像布局

- `crates/sumpter-core/`：纯逻辑层，配置、路由、调度、协议转换和事件契约。
- `crates/sumpter-runtime/`：共享 SQLite worker、projection、rollup、查询和导出。
- `crates/sumpter-engine/`：共享 HTTP 数据面、转发、重试、relay、回放和生命周期。
- `adapters/linux/sumpter-linux-adapter/`：Linux 平台边界、Admin、server 和组合 wrapper。
- `adapters/macos/sumpter-macos-adapter/`：macOS 平台边界、Admin、server 和组合 wrapper。
- `apps/linux/sumpterd/`、`apps/macos/sumpterd/`：只负责启动参数、配置目录、监听和退出生命周期。
- `platforms/macos/app/`：SwiftUI 壳、测试、Sparkle 清单和 DMG 打包脚本；业务逻辑未修改。
- `platforms/linux/webui/`：WebUI 源码、测试、`package-lock.json`；`platforms/linux/web/` 是已构建静态资源。
- `platforms/linux/scripts/`、`platforms/linux/deploy/`、`platforms/linux/.github/workflows/`：交叉构建、安装、服务和发布输入。
- `scripts/sync-usage-docs.py`、`docs/usage-onboarding.md`、`docs/usage-path-matrix.json`：开箱文档的模板、路径矩阵和同步检查。
- `SOURCE_COMMIT`：记录本次镜像所基于的源提交；源工作区的未提交改动也按当时文件内容复制。

没有复制 `target/`、Swift `.build/`、`node_modules/` 或 DMG/ZIP 等生成物；依赖可按各目录的
lockfile 重新生成。

## 当前架构优先阅读

- `architecture.md`：当前单一共享引擎、多端 adapter 的结构树、依赖方向和变更边界。
- 根 `Cargo.toml`：唯一 Rust workspace 成员和共享依赖版本。
- `crates/sumpter-engine/src/boundary.rs`：共享引擎与平台能力的最小接口。
- `adapters/linux/sumpter-linux-adapter/src/platform.rs`、`adapters/macos/sumpter-macos-adapter/src/platform.rs`：平台策略实现。

## 历史/对照文档

- `upstream/engine-unification-plan.md`：重构前的单一引擎并行迁移记录。
- `upstream/双端引擎架构分析.md`：重构前的双端差异与边界注入分析。
- `upstream/macos/CONFIG.md`：schema v6 配置字段和 mapping 语义。
- `upstream/macos/specs/spec-core.md`、`upstream/linux/specs/spec-core.md`：核心行为对照。
- `upstream/linux/specs/runtime-analytics-v2.md`：SQLite 统计、分页、筛选和导出契约。
- `upstream/linux/specs/seams.md`：core/runtime/engine 的组件边界。
- `upstream/claude-code客户端的参数变量字段对应.md`、
  `upstream/codex客户端的参数变量字段对应.md`：客户端请求与事件字段。

## 平台集成参考

`platforms/linux/README.md`、`platforms/linux/USAGE.md`、`platforms/linux/specs/admin-api.md`、
`platforms/macos/README.md`、`platforms/macos/CONFIG.md` 和根 `USAGE.md` 是现有开箱/构建入口；涉及 Rust 数据面时以根 workspace、
adapter 和测试为准。`upstream/` 下的同名文件仅用于迁移对照。

## 迁移背景

`upstream/parity-ledger.md` 是历史对拍基线，`upstream/双端引擎架构分析.md` 也包含旧裁定过程。
它们可用于理解迁移背景，但不应替代当前源码和测试。

## 本次未复制

已排除平台缓存和生成物（`target/`、`.build/`、`dist*/`、`release/`、`node_modules/`、DMG/ZIP）、
真实运行数据和密钥，以及 agent 工作规则和历史计划文档。Linux 的 Dockerfile/Compose 与发布
workflow 作为构建输入保留，但本机验证不调用 Docker。
