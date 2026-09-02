# Sumpter 文档索引

先读根 [`README.md`](../README.md) 了解产品与仓库地图，再按用途打开下面的文件。

`upstream/` 是历史源码快照，内容保持原样，**不是**当前 API 契约或目录约定。现行 Rust 代码只有根目录一份 workspace；Linux / macOS 通过 adapter 注入平台能力。

## 先读这些

| 你想做什么 | 读哪份 |
|---|---|
| 产品定位、仓库结构、开发命令 | 根 [`README.md`](../README.md) |
| 安装、接 Claude/Codex、排错 | 根 [`USAGE.md`](../USAGE.md) |
| 配置字段 | [`../platforms/macos/CONFIG.md`](../platforms/macos/CONFIG.md)（两端同一份 schema v6） |
| 当前 Rust 分层与变更归属 | [`architecture.md`](architecture.md) |
| 仓库协作规则 | [`../AGENTS.md`](../AGENTS.md) |

## 源码布局

- `crates/sumpter-core/`：配置、路由、调度、模型映射和事件契约。
- `crates/sumpter-runtime/`：共享 SQLite worker、投影、rollup、查询和导出。
- `crates/sumpter-engine/`：共享 HTTP 数据面、转发、重试、relay、回放和生命周期。
- `adapters/linux/sumpter-linux-adapter/`：Linux 平台边界、Admin、server。
- `adapters/macos/sumpter-macos-adapter/`：macOS 平台边界、Admin、通知、sidecar server。
- `apps/linux/sumpterd/`、`apps/macos/sumpterd/`：可执行入口；只做参数、配置目录、监听和生命周期。
- `platforms/macos/scripts/`、`platforms/macos/app/`：macOS 客户端配置脚本、SwiftUI 壳、测试、Sparkle 和 DMG 打包。
- `platforms/linux/webui/`：WebUI 源码与测试；`platforms/linux/web/` 是已构建静态资源。
- `platforms/linux/scripts/`、`platforms/linux/deploy/`、`platforms/linux/.github/workflows/`：Linux 安装、systemd 与独立发布树输入；当前 monorepo 的 GitHub workflow 在根 `.github/workflows/`。
- `scripts/sync-usage-docs.py`、`docs/usage-onboarding.md`、`docs/usage-path-matrix.json`：开箱正文模板和同步检查。

根 `Cargo.toml` 是唯一 Rust workspace。共享引擎与平台的最小接口是 `crates/sumpter-engine/src/boundary.rs`。

## 跨平台 UI 对齐

Linux 与 macOS 对应页面尽量对齐：信息层级、字段命名、状态语义和主要交互一致；仅在平台原生控件或布局需要时保留差异。新增或调整页面时，先对照另一端已确认的交互，再补平台特有适配。

用户可见品牌和 Linux 运行时名字都是 **Sumpter**：配置目录 `~/.config/sumpter`、systemd 单元 `sumpter.service`、发布包二进制 `sumpterd`、header `X-Sumpter-*`、环境变量 `SUMPTER_*`。旧的 `kekulv` 名字不再识别。

## 平台文档

- Linux 安装、systemd、Docker、Admin 反代：[`../platforms/linux/README.md`](../platforms/linux/README.md)
- Linux 发布包内的使用指南：[`../platforms/linux/USAGE.md`](../platforms/linux/USAGE.md)
- Linux Admin API：[`../platforms/linux/specs/admin-api.md`](../platforms/linux/specs/admin-api.md)
- macOS sidecar 与本机构建：[`../platforms/macos/README.md`](../platforms/macos/README.md)
- macOS 首次打开被拦截：[`../platforms/macos/app/INSTALL.txt`](../platforms/macos/app/INSTALL.txt)
- macOS Sparkle 发布：[`../platforms/macos/app/UPDATE.md`](../platforms/macos/app/UPDATE.md)
- Linux WebUI 构建：[`../platforms/linux/webui/README.md`](../platforms/linux/webui/README.md)

涉及 Rust 数据面时以根 workspace、adapter 和测试为准，不要按 `platforms/linux/` 里尚未适配的发布脚本路径去猜 crate 名。

## 历史对照（不要当现行契约）

- [`upstream/README.md`](upstream/README.md)：重构前的双目录产品说明。
- [`upstream/engine-unification-plan.md`](upstream/engine-unification-plan.md)：单一引擎并行迁移记录。
- [`upstream/双端引擎架构分析.md`](upstream/双端引擎架构分析.md)、[`upstream/parity-ledger.md`](upstream/parity-ledger.md)：双端差异与对拍基线。
- [`upstream/macos/CONFIG.md`](upstream/macos/CONFIG.md)：当时的 schema v6 字段说明；现行字段以 `platforms/macos/CONFIG.md` 为准。
- [`upstream/linux/specs/`](upstream/linux/specs/)：当时的 core / engine / analytics / Admin 规格。
- [`upstream/claude-code客户端的参数变量字段对应.md`](upstream/claude-code客户端的参数变量字段对应.md)、[`upstream/codex客户端的参数变量字段对应.md`](upstream/codex客户端的参数变量字段对应.md)：客户端请求与事件字段对照；文中的 `linux/crates/sumpter-*` 路径已过期。
- [`code-review-2026-08-31.md`](code-review-2026-08-31.md)：架构搬迁前的风险审查。主体结论针对当时的双轨 legacy 入口，不要用来判断当前活动路径。
- 根 [`plan.md`](../plan.md)：按轮次追加的工作日志，不是当前架构说明。

`SOURCE_COMMIT` 只记录某次镜像所基于的源提交，不是 GitHub 默认分支的当前位置。
