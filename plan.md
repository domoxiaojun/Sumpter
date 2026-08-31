# 当前执行计划

- [x] ✅ 1. 盘点仓库结构、构建约束、依赖与当前工作区状态
- [x] ✅ 2. 深读 `kekulv-core`：配置、路由、调度、协议桥接与错误/事件契约
- [x] ✅ 3. 深读 `kekulv-runtime`：SQLite 存储、查询、并发与生命周期语义
- [x] ✅ 4. 深读 `kekulv-engine`：请求生命周期、转发、重试、回放、健康检查与平台边界
- [x] ✅ 5. 对照测试、fixture、示例配置和文档，核对契约覆盖与漂移
- [x] ✅ 6. 运行最小必要验证，整理风险分级、维护地图和后续建议
- [x] ✅ 7. 输出审查总结并收尾

## 本轮：平台源码镜像（2026-08-31）

- [x] ✅ 确认本目录就是共享根，保留既有 `crates/`、锁文件和 `target/`，不覆盖已有文件。
- [x] ✅ 复制 macOS SwiftUI `Package.swift`、Sources、Tests、Rust workspace、图标和打包脚本。
- [x] ✅ 复制 Linux Rust workspace、WebUI 源码/测试、已构建静态 WebUI、构建/安装脚本、部署单元和集成脚本。
- [x] ✅ 排除 `target/`、Swift `.build/`、DMG/ZIP、`dist/`、`release/`、`node_modules/` 和真实运行数据。
- [x] ✅ 将镜像内双端 `kekulv-engine` 路径指向共享根 `crates/kekulv-engine`，并让 Linux 组装脚本兼容本目录拓扑。
- [x] ✅ 更新镜像文档索引和本地链接，说明根 `crates/`、双端源码与未复制的生成物边界。
- [x] ✅ 补齐开箱文档模板、路径矩阵与 `scripts/sync-usage-docs.py`，并通过 `uv run ... --check`。
- [x] ✅ 本地验证通过：双端 legacy/unified Cargo check、共享引擎双平台测试、SwiftUI 构建/139 项测试、WebUI 113 项测试与 clean `npm ci` 构建、Linux cross-build check/release-preflight。
- [ ] 用户在本机运行 macOS SwiftUI/DMG 或 Linux feature 构建进行最终验收。

## 本轮：单一引擎、多端适配架构整理与品牌工程标识切换（2026-08-31）

- [x] ✅ 1. 建立迁移基线：清点重复 crate、workspace、默认入口和旧品牌工程标识；锁定“不改前端/业务行为”的边界。
- [x] ✅ 2. 合并 Rust workspace：以根 `crates/` 为唯一共享 core/runtime/engine 来源，移除 Linux/macOS 的重复共享 crate 和 legacy proxy 入口。
- [x] ✅ 3. 建立平台适配边界：Linux/macOS 只保留 daemon/sidecar 组装、系统生命周期、路径、权限与平台控制实现；共享引擎不再依赖具体平台实现。
- [x] ✅ 4. 切换默认入口：Linux daemon 与 macOS sidecar 均直接使用共享 engine；保留前端目录、静态资源和 SwiftUI 功能不变。
- [x] ✅ 5. 机械切换工程品牌：Rust 包、二进制、workspace 路径和架构文档统一使用 `sumpter/Sumpter`；协议字段、部署脚本、前端和 SwiftUI 内容按边界保留。
- [x] ✅ 6. 执行最小必要验证：Cargo check/test、格式检查、workspace/路径引用检查；兼容协议字段中的旧品牌残留已单独记录。
- [x] ✅ 7. 更新架构文档与计划，交付结构树、变更边界和未覆盖的前端/协议品牌残留清单。

### 本轮明确未覆盖

- `platforms/linux/scripts/assemble-shared-tree.sh`、`cross-build.sh`、`release-preflight.sh` 仍按独立 Linux 发布树查找 `crates`/`kekulvd`；本轮不改部署、安装、发布和前端代码，需另开“发布链适配”任务。
- `X-Kekulv-*`、`KEKULV_*`、`kekulv.service`、`~/.config/kekulv`、导出格式标识和 sticky domain 属于兼容协议/数据字段，未在架构搬迁中机械替换。

## 本轮：平台目录规范化（2026-08-31）

- [x] ✅ 1. 盘点 Linux/macOS 平台输入，区分平台源码/资源与可重建缓存，锁定不改 WebUI/SwiftUI 逻辑的边界。
- [x] ✅ 2. 将平台产品输入整体迁移到 `platforms/linux/`、`platforms/macos/`，不复制或覆盖缓存和已有文件。
- [x] ✅ 3. 仅更新必要的 Rust、打包/发布入口和文档路径引用，保持运行时协议与业务行为不变。
- [x] ✅ 4. 验证根 workspace、平台静态资源路径、Swift Package 入口和目录结构，记录仍需单独适配的发布链问题。
- [x] ✅ 5. 更新架构文档、计划和维护入口，交付最终结构树与变更边界。

### 本轮边界与遗留

- 平台输入整体保留，包含可重建缓存；本轮没有删除 `target/`、`.build/`、`node_modules/` 或发布产物。
- WebUI/SwiftUI 业务逻辑、协议字段和用户可见产品命名未改；SwiftUI 中仅更新了源码路径字符串以匹配新拓扑。
- `platforms/linux/scripts/cross-build.sh`、`release-preflight.sh` 与 macOS 发布脚本仍有独立发布树、旧 profile 或 sidecar 命名假设；DMG/Linux 发布链需另开适配任务并单独验收。
- 验证记录：根 workspace `cargo fmt/check/test`、`uv ... --check`、TOML/Shell/本地链接检查通过；Swift Package 独立 scratch 构建与 139 项测试通过。现有迁移前 `.build` 直接复用会因旧绝对路径失败，未删除该缓存。

## 本轮：本地 macOS DMG 构建入口（2026-08-31）

- [x] ✅ 1. 新增仓库根目录本地 DMG 构建脚本，复用现有 macOS 打包链并提供测试/清理/架构选项。
- [x] ✅ 2. 运行脚本完成 Sumpter 本地 Release DMG、App 与 ZIP 构建。
- [x] ✅ 3. 验证 DMG、App、代码签名、压缩包和 SHA-256，交付本机测试产物。

## 本轮：Sumpter 品牌与仓库地址全量切换（2026-08-31）

- [x] ✅ 1. 盘点对外品牌、macOS App/Swift 模块、Linux WebUI、文档与仓库链接，区分可重命名标识和必须保留的兼容协议字段。
- [x] ✅ 2. 将 macOS App 的展示名、SwiftPM 产品/模块、安装入口和打包产物统一为 `Sumpter`，保持业务逻辑不变。
- [x] ✅ 3. 将 Linux WebUI 与 macOS UI 中的品牌文案和 GitHub 仓库链接统一为 `Sumpter` / `domoxiaojun/sumpter`，同步构建静态 Web 资源。
- [x] ✅ 4. 更新公开文档、示例、发布脚本中的仓库地址和品牌名称；协议 header、环境变量、数据格式、配置目录等兼容契约不做破坏性迁移。
- [x] ✅ 5. 执行 Swift、WebUI、Cargo 与打包产物验证，复核旧品牌残留只存在于明确的兼容字段或历史上游文档。
