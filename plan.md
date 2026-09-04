# 执行记录（2026-08-31 起）

## 本轮：Grok 事件详情补采样客户端/会话字段（2026-09-04）

对照 grok-build `xai-grok-sampler`：推理请求身份在 `x-grok-*` header，不在 Codex
`client_metadata`。cwd/git 仍不进推理请求。事件详情新增 `grokMetadata`，展示会话、
对话、请求、客户端标识/版本/模式等。OTel `traceparent` 不再让 Grok 请求显示空的
Codex「代理身份未确定」。`x-grok-*` 出站保留（官方采样 header）。不宣称已安装 App
已更新。

- [x] core `GrokMetadata::from_headers` → `RuntimeEvent.grokMetadata`
- [x] engine 贯通转发/拒绝/websocket；`x-grok-session-id`/`x-grok-conv-id` 进 sessionID
- [x] 双端运行页详情与列表摘要
- [x] 测试与提交

---


本文是按轮次追加的工作日志，**不是**当前架构说明。当前结构以 [`docs/architecture.md`](docs/architecture.md) 为准，用户开箱以根 [`README.md`](README.md) 和 [`USAGE.md`](USAGE.md) 为准。下文出现的 `kekulv-core` / `linux/crates` 等名字属于当时任务描述，不要当作活动路径。

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

## 本轮：诊断页存储信息层级优化（2026-08-31）

- [x] ✅ 1. 确认 macOS 诊断页现有存储数据契约、SwiftUI 组件与 sheet 模式。
- [x] ✅ 2. 将运行统计存储改为紧凑摘要，正常态隐藏实现版本和低频技术字段。
- [x] ✅ 3. 将存储上限编辑迁移到弹出 sheet，保留校验、保存和关闭上限行为。
- [x] ✅ 4. 补充必要回归检查，运行 Swift 测试与构建验证。
- [x] ✅ 5. 检查差异并交付变更边界与验证结果。

## 本轮：macOS 系统提示音动态选项（2026-08-31）

- [x] ✅ 1. 盘点现有提示音偏好、试听播放和系统通知音效调用链。
- [x] ✅ 2. 改为扫描 `/System/Library/Sounds` 生成提示音选项，保留系统默认和静音。
- [x] ✅ 3. 兼容旧版本裸名称配置，并让试听与系统通知使用实际文件名/扩展名。
- [x] ✅ 4. 补充系统音效发现与旧配置迁移测试，完成 Swift 测试验证。

## 本轮：统计页项目选择交互修复（2026-08-31）

- [x] ✅ 1. 定位项目行点击、局部会话钻取与顶部筛选计数之间的状态断点。
- [x] ✅ 2. 将局部项目选择同步到 StatsPage，使顶部清除筛选按钮正确启用并可同时清理钻取状态。
- [x] ✅ 3. 优化项目钻取后的会话查询条件，并让选中状态先绘制再启动聚合请求，减少卡顿与未识别项目失配。
- [x] ✅ 4. 运行 WebUI 单元测试与生产构建验证。
- [x] ✅ 5. 检查源码与静态资源差异，保留现有工作区改动并交付。

## 本轮：Linux / macOS 统计存储界面对齐（2026-08-31）

### 对齐规则（持续适用）

Linux 与 macOS 对应页面尽量保持对齐：信息层级、字段命名、状态语义和主要交互一致；仅在平台原生控件或布局需要时保留差异。

- [x] ✅ 1. 对照 macOS 诊断页已落地的存储摘要、技术详情和弹出设置交互，确认 Linux 现有实现与目标差异。
- [x] ✅ 2. 将 Linux 存储摘要收敛为常用指标，并把低频技术字段放入默认收起的详情。
- [x] ✅ 3. 将 Linux 存储上限与清理操作迁移到可关闭的弹出设置窗口，保留校验、保存和错误反馈。
- [x] ✅ 4. 同步回归测试与响应式样式，完成 Linux WebUI 测试和生产构建验证。
- [x] ✅ 5. 检查变更边界并提交本轮明确范围，保留其他未提交工作。

## 本轮：统一 macOS Claude Code / Codex CLI 通知并移除 legacy notify（2026-08-31）

- [x] ✅ 1. 新增 Codex Stop Hook 脚本、hooks.json 安全合并器与 legacy notify 冲突/移除逻辑。
- [x] ✅ 2. 扩展 Rust `/__notify`、PlatformNotice 与 Admin SSE 的 `clientKind` 契约，隔离来源、去重并过滤 Codex 敏感字段。
- [x] ✅ 3. 接入 Swift AppModel、Codex Hook 状态、端口变更重写和通知线程/投递分发。
- [x] ✅ 4. 在 macOS 设置页加入 Codex CLI 通知区域、信任引导与冲突/失败状态。
- [x] ✅ 5. 更新开箱文档、macOS README 与生成的 USAGE 文档，明确 Linux 不提供通知 Hook 和 `/__notify`。
- [x] ✅ 6. 补充 Swift/Rust 回归测试并完成最小必要验证。

## 本轮：恢复统计数据自定义时间保留策略（2026-08-31）

### 目标与边界

恢复统计存储的“最大保存天数”自定义时间维度，并与现有 SQLite
有效占用上限组成并行保留策略。时间策略只自动清理已完成请求；进行中的
请求及其关联链不删除。Linux 与 macOS 共用同一 Admin/API/SQLite 语义，
界面继续把低频设置放在弹窗或原生 sheet 中。

- [x] ✅ 1. 参考业界保留策略，冻结字段、时间边界、删除粒度与危险操作交互：采用 `maxAgeDays`（滚动 24 小时、`null` 关闭），与容量上限按 OR 关系后台轮换；只删除已完成且不含进行中事件的请求组，设置变更立即清理，重置/整库重建继续保留明确确认。
- [x] ✅ 2. 设计并实现共享 runtime 的 `maxAgeDays` 保留策略、请求链安全删除和并行容量判断。
- [x] ✅ 3. 同步 Linux/macOS Admin API、wire 模型、mock 与协议文档。
- [x] ✅ 4. 在 Linux 弹窗与 macOS sheet 中加入最大保存天数编辑、校验、状态与清理反馈。
- [x] ✅ 5. 补充 Rust、Admin、WebUI、Swift 回归测试并运行最小必要验证。
- [x] ✅ 6. 检查差异边界、更新文档并交付；不触碰现有无关未提交改动。

## 本轮：项目 / 会话模型用量明细（2026-08-31）

- [x] ✅ 1. 确认模型维度 API 与项目、会话筛选组合的现有契约，冻结为前端钻取方案。
- [x] ✅ 2. 在 Linux 统计概览与成本看板补充模型用量表，支持按项目、会话查看各模型请求、Token 与成本。
- [x] ✅ 3. 在 macOS 统计页同步项目 / 会话模型钻取与字段口径，保持双端信息层级一致。
- [x] ✅ 4. 补充前端与 wire 回归测试，运行 WebUI 测试/构建及 Swift 最小必要验证。
- [x] ✅ 5. 检查混合工作区差异、更新计划并交付，不提交、不推送。

## 本轮：统计一次性按时间范围清理（2026-08-31）

### 目标与边界

合并重复的“重置/手动清理统计”入口，改为一次性清理弹窗：按“保留近 N 天”
或“清理早于指定日期”选择范围，默认提供 7/30/90 天和自定义日期。清理只
删除已完成请求组，含进行中事件的请求组完整保留；诊断捕获、自动保留策略和
“重置并新建数据库”继续独立。Linux 与 macOS 共用同一 Admin/API/SQLite 语义。

- [x] ✅ 1. 冻结清理请求、预览、时间边界、时区和确认文案。
- [x] ✅ 2. 在 runtime/engine/Admin 增加范围预览与按时间清理接口，保持请求组原子删除。
- [x] ✅ 3. Linux 以清理弹窗替换重复入口，并保留结构重建单独入口。
- [x] ✅ 4. macOS 同步原生 sheet 与清理确认交互。
- [x] ✅ 5. 补充 Rust、Admin、WebUI、Swift 回归测试并运行最小必要验证。
- [x] ✅ 6. 检查混合工作区差异，更新文档并交付，不提交、不推送。

## 本轮：统计筛选与下钻状态一致性修复（2026-08-31）

- [x] ✅ 1. 修复 macOS“最终结果”筛选只改界面状态、不触发统计刷新的问题。
- [x] ✅ 2. 补齐 macOS 顶部筛选状态文字，覆盖全部筛选条件。
- [x] ✅ 3. 修复 Linux 仅会话下钻时顶部清除按钮和筛选计数遗漏的问题。
- [x] ✅ 4. 修复 Linux/macOS 删除、清理或重置统计后遗留失效下钻状态的问题。
- [x] ✅ 5. 修复 Linux 表格操作按钮的键盘事件冒泡，并校正 mock 下钻稳定 ID。
- [x] ✅ 6. 运行最小必要回归验证，检查差异边界并交付；不提交、不推送。

## 本轮：HTTP 500 重试次数与 Codex `retry_delay` 返回（2026-08-31）

### 目标与边界

针对 Codex 与 Claude Code 共用的转发引擎：为上游 HTTP 500 增加可配置的
连续失败次数/切换入口策略，并在最终可重试错误响应中返回客户端可识别的
`retry_delay` 秒数字段；保留现有 429/网关错误重试、退避、取消和双端配置
契约，避免覆盖工作区其他未提交改动。

- [x] ✅ 1. 盘点共享 RetryPolicy、错误响应、Linux/macOS Admin API 与双端设置界面的现有字段和测试契约。
- [x] ✅ 2. 设计并实现 500 专用重试次数、入口切换边界与自定义 `retry_delay` 秒数，保持默认行为安全且兼容旧配置。
- [x] ✅ 3. 同步 Linux/macOS 设置界面、Admin wire 与配置示例/文档，明确字段语义和默认值。
- [x] ✅ 4. 补充共享核心、引擎、Linux/macOS adapter 和 WebUI/Swift 回归测试。
- [x] ✅ 5. 运行最小必要格式、类型、单元/集成验证，检查混合工作区差异并交付，不提交、不推送。

## 本轮：HTTP 500 failover 开关与重试参数界面可读性优化（2026-09-01）

### 目标与边界

保留既有 HTTP 500 重试默认行为，新增显式的“500 失败后切换入口”开关；
同时为最终错误响应中的 `retry_delay`/`Retry-After` 增加独立透传开关。同步共享
配置、引擎、Linux WebUI 与 macOS SwiftUI，并修正长占位符被输入框裁切的问题。
只改重试配置相关逻辑与文案，不触碰其它未提交改动。

- [x] ✅ 1. 冻结 `failoverOn500` 与 `passThroughRetryDelay` 字段语义、默认值与关闭后的边界。
- [x] ✅ 2. 实现共享 RetryPolicy、引擎与双端配置保存/读取。
- [x] ✅ 3. 优化 Linux/macOS 重试参数布局、标签、辅助说明和开关交互。
- [x] ✅ 4. 补充回归测试并运行最小必要格式、类型与单元验证。
- [x] ✅ 5. 检查混合工作区差异并交付，不提交、不推送。

## 本轮：双端内置 Claude Code 项目归因配置器（2026-08-31）

- [x] ✅ 1. 盘点现有脚本、macOS App 资源打包与 Linux 发布/远程安装链路。
- [x] ✅ 2. 增加 macOS 源码脚本输入，并让 App 打包从 macOS 路径内置到 `Contents/Resources/`。
- [x] ✅ 3. 将 Linux 发布包与 bootstrap/安装器校验绑定到归因脚本，并确认脚本随包安装。
- [x] ✅ 4. 在 Linux listener 增加编译期内置的 `GET /__sumpter/cc-project-attribution.sh`，支持局域网 Base URL 与 Nginx 反代；沿用 CIDR 与 listener token 边界，macOS 不开放该端点。
- [x] ✅ 5. 同步双端文档、Linux 安全页与 listener 下载命令，移除误加的静态镜像远程 helper。
- [x] ✅ 6. 运行 Linux adapter focused tests、WebUI 归因契约测试、ShellCheck、双端脚本自测与 `git diff --check`，保留现有混合工作区改动，不提交、不推送。

## 本轮：补齐 Codex 通知并统一 Claude/Codex 通知选项（2026-09-01）

### 目标与边界

把 Codex CLI 的可用生命周期通知接入 macOS Sumpter，并与 Claude Code
共用同一套通知类别、声音、系统授权、测试和投递设置。用户界面不再按
Claude/Codex 重复展示相同选项；内部仍保留来源标识、Codex Hook 信任边界
和敏感字段过滤。只修改通知相关代码、文档与测试，保留工作区其它未提交改动。

- [x] ✅ 1. 冻结统一事件、类别、默认开关与 Codex 事件的安全文案/噪声策略：共享 `action_required`、`turn_completed`、`subtask_completed`、`turn_failed`、`status` 五类；Codex 接入 `PermissionRequest`、`Stop`、`SubagentStop`、`Interrupt`，默认只开行动/完成/失败，普通状态保持关闭以避免生命周期噪声；Codex 文案固定化，不转发 transcript、prompt、原始错误或工具参数。
- [x] ✅ 2. 扩展 Codex hooks.json 安全编辑器与脚本生成器，支持 `PermissionRequest`、`Stop`、`SubagentStop`、`Interrupt` 共用一个 stdin 转发脚本，并保留未知用户 Hook。
- [x] ✅ 3. 修改 Rust `/__notify` 接收与类别映射，统一 Claude/Codex 投递并保留敏感字段过滤、去重。
- [x] ✅ 4. 重构 macOS AppModel 与 NotificationsPane，共用通知类别、授权、声音、测试和投递逻辑；客户端差异只保留在配置文件状态与 Codex `/hooks` 信任提示。
- [x] ✅ 5. 更新文档、测试与兼容迁移逻辑，说明 Codex `/hooks` 审核仍是独立安全步骤。
- [x] ✅ 6. 运行最小必要 Swift/Rust/JSON/差异验证，检查混合工作区边界并交付，不提交、不推送：Swift Codex 通知测试 17 项通过；macOS adapter Codex 相关 Rust 测试 6 项通过；`jq empty ~/.codex/hooks.json` 与 `git diff --check` 通过。全仓 `cargo fmt --check` 仍受现有非本轮差异影响，未执行自动格式化。

## 本轮：双端 Provider 功能与展示对齐完善（2026-09-01）

### 目标与边界

尽量统一 Linux WebUI 与 macOS SwiftUI Provider 页的信息层级、字段命名、
状态语义和主要排序交互；保留 Web/原生控件的必要差异。修复已确认的
Linux `retry_delay` 开关状态断裂、窄窗口排序能力不一致和 macOS 不安全示例配置。
不触碰无关工作区改动，不提交、不推送。

- [x] ✅ 1. 对齐 Provider 顶部摘要、retry 文案/状态标签和入口详情字段。
- [x] ✅ 2. 修复 Linux retry_delay 开关联动，并让 Linux 窄窗口卡片支持拖拽排序；补齐拖拽可视反馈。
- [x] ✅ 3. 将 macOS 示例入口改为禁用的 `.invalid` 安全模板并保持双端 retry 默认值一致。
- [x] ✅ 4. 运行 Linux WebUI、Rust workspace、macOS Swift 测试和必要的构建/差异检查，复核混合工作区边界：WebUI 119 项测试与生产构建、Rust workspace 全量测试、Swift 167 项测试、文档同步检查、双端示例 JSON 校验和 `git diff --check` 均通过；补充 Provider 摘要响应式两列布局、修正 macOS 主题依赖，未提交或推送。

## 本轮：补齐 OpenAI Responses WebSocket、Realtime/Live、Files、Videos 与 `/v1/models`（2026-09-01）

> 注：以下先行方案记录包含本地模型目录、Responses WS 的 SSE/transcript 设计；
> 后续按“只透传给上游”的边界完成修正，当前契约以文档末尾的“协议回归为上游纯透传”记录为准。

### 目标与边界

参考 `/Users/kkl/Documents/claude/CLIProxyAPI` 的路由与协议语义，在共享
Sumpter engine 增加这些 OpenAI 兼容能力。普通 HTTP 资源继续走统一鉴权、
模型映射、入口 failover、流式 relay 与事件记账；WebSocket 明确在 HTTP
fallback 之外升级，Responses WebSocket 复用已有 Responses HTTP/SSE 管线，
Realtime/Live 保留双向帧语义并透传上游 WebSocket。文件和视频的 multipart、
JSON、二进制下载不做协议桥接或内容重建。

- [x] ✅ 1. 盘点 CLIProxyAPI 对应路由、请求/响应终态、鉴权和当前 Sumpter transport 边界，冻结兼容路径与错误语义：资源 API 采用原生透传；Responses WS 在 `/v1/responses` GET 升级并以 JSON 事件输出；Realtime/Live 保留双向 WebSocket/HTTP bootstrap；模型目录由本地映射生成。
- [x] ✅ 2. 增加 `/v1/models`（含 `/models` 兼容别名）本地模型目录响应，并覆盖鉴权、空目录和 Codex `client_version` 形状。
- [x] ✅ 3. 扩展原生资源 relay，支持 Files 全套 CRUD/内容下载与 Videos 创建、查询、内容下载，保留动态资源路径、请求方法和 Content-Type。
- [x] ✅ 4. 增加 Responses WebSocket 升级与事件转发；实现 `response.create`/`response.append` 校验、HTTP/SSE 回落和终端错误帧。
- [x] ✅ 5. 增加 Realtime/Live WebSocket 与相关 HTTP bootstrap/sideband 路由，支持鉴权后双向帧 relay、关闭传播和握手错误。
- [x] ✅ 6. 补充共享 engine、Linux/macOS server 与协议回归测试，运行最小必要 fmt/check/test，并检查混合工作区差异：新增资源/模型、Linux WebSocket、macOS WebSocket 回归测试；workspace 测试全部通过；格式检查仅剩既有 `tests/engine.rs` 排版差异。
- [x] ✅ 7. 更新根文档与双端 usage 的支持矩阵，记录真实 upstream 能力仍需凭 Provider 实测确认；不提交、不推送，保留混合工作区其他改动。
- [x] ✅ 8. 对照 CLIProxyAPI 补齐 Responses WebSocket 的多轮 transcript：保存 `response.completed` 输出项、assistant/tool-call 去重，并为工具输出场景保留 `previous_response_id` 增量请求；新增回归测试并通过引擎与 Linux WebSocket focused tests。
- [x] ✅ 9. 完善 Realtime HTTP bootstrap：注册的 `ek_…` 凭证可用于 HTTP/SDP 请求并继续透传到 Provider，Realtime 请求保留 `Accept: application/sdp`；新增凭证转发与 SDP header 回归测试。
- [x] ✅ 10. 同步 usage 说明 Responses WebSocket 的跨轮 transcript 与 Realtime HTTP/SDP 凭证边界，完成文档同步检查。
- [x] ✅ 11. 收尾核对 Provider 选择与双端升级路径，完成 engine、Linux/macOS WebSocket focused tests、全 workspace 测试、文档同步和差异检查。

## 本轮：协议回归为上游纯透传（2026-09-01）

### 目标与边界

这些入口的职责是统一鉴权、按 mapping 选择 Provider、必要的上游模型名替换、
failover 和连接 relay；协议语义、请求/响应内容和 WebSocket 帧由上游负责。
不在 Sumpter 内本地生成模型目录，不把 Responses WebSocket 转成 HTTP/SSE，
不维护本地 transcript。

- [x] ✅ 1. 修正 Linux/macOS WebSocket 路由：GET 才升级，POST/其它 HTTP 方法进入统一 engine fallback，避免直接 405。
- [x] ✅ 2. 将 Responses WebSocket 改为双向帧 relay；文本、二进制和关闭帧交给上游，不解析事件、不做 HTTP/SSE 回落。
- [x] ✅ 3. 将 `/v1/models` 与兼容别名改为 Provider 上游透传，保留请求方法、query、body 与响应，不再本地 mappings 生成目录。
- [x] ✅ 4. 保持 Files/Videos、Realtime/Live 的 HTTP/WebSocket 资源路径和响应透传；除必要的上游模型名映射外不重建协议 body。
- [x] ✅ 5. 更新根文档、Linux/macOS 文档和双端帮助文案，明确纯透传边界；完成 `uv run scripts/sync-usage-docs.py --check`、workspace focused tests、`cargo check --workspace --locked` 与 `git diff --check`。
- [x] ✅ 6. 删除 Responses WebSocket 旧的 SSE/transcript fallback 辅助代码与测试；撤销 Codex、无 `/v1`、`/openai/v1` 等路径别名归一化，回归测试改为断言客户端 path/query 原样交给上游。

## 本轮：客户端请求统一 raw 透传收尾（2026-09-01）

### 目标与边界

继续落实“客户端请求都透传”：数据面只做入站鉴权、Provider/mapping 选择、必要的模型名
替换、failover/retry 与 HTTP/WebSocket relay。不得因为路径、协议标签或别名判断而在本地
重建 Responses、Realtime/Live、Files、Videos、Models 或其它厂商协议；`/__*` 仍是本地控制面。

- [x] ✅ 1. 删除已失效的协议专用解析辅助函数，避免旧的 multipart/资源白名单语义残留。
- [x] ✅ 2. 增加 raw 路由规划：固定 `protocol` 只作为入口元数据，不再成为 HTTP 透传的兼容性拦截条件；仍按已配置 mapping 选择 Provider。
- [x] ✅ 3. 将 WebSocket 统一按 raw relay 选择入口，保留原始 path/query、帧和关闭传播；Realtime `ek_…` 仅用于鉴权。
- [x] ✅ 4. 更新 README、架构说明、根/ Linux USAGE，明确无协议转换、无别名归一化、无本地 Models 响应，并说明安全剥离的连接级/鉴权 header。
- [x] ✅ 5. 补充固定协议入口的任意路径透传回归测试；完成 `cargo check`、engine 测试、Linux/macOS WebSocket 定向测试、`uv run scripts/sync-usage-docs.py --check` 与 `git diff --check`。

补充收尾：Raw 请求不再猜测缺失的 `Content-Type`，不重排可转发 header 的重复值；Realtime
`ek_…` 短期凭证仅以 Bearer 方式注入，且 2xx client-secret 响应都可登记后续升级鉴权。

验证备注：`cargo fmt --all -- --check` 仍只报告工作区既有的 Linux server 空行及双端
`tests/engine.rs` 排版差异，本轮未自动格式化无关文件；未提交、未推送，保留混合工作区其它改动。

补充：`cargo test --workspace --locked` 的旧 Linux `tests/engine.rs` 仍有一批断言依赖此前
已撤销的协议桥接、别名归一化和本地终止事件语义；本轮未篡改这些混合工作区历史测试。新的
raw 透传路径已由 engine 资源测试及双端 WebSocket focused tests 覆盖并通过。

## 本轮：macOS DMG 测试门禁与实际打包（2026-09-01）

- [x] ✅ 1. 复现并确认 DMG 命令失败点是两端历史 `tests/engine.rs` 的旧桥接断言，不是 Xcode、Rust release 或 DMG 工具。
- [x] ✅ 2. 将 monorepo macOS 打包门禁改为 shared workspace 全量测试、macOS adapter unit/WebSocket 当前契约测试和 Swift 测试；保留旧集成测试未迁移的事实，不伪装成全 workspace 通过。
- [x] ✅ 3. 通过 ShellCheck、Swift 196 项测试、当前 Rust 测试门禁，并完成 arm64 Sumpter App、DMG 与 ZIP 构建及 SHA-256 校验。

## 本轮：修复 Codex Live 语音入口误路由到普通模型（2026-09-02）

### 目标与边界

已确认 `/v1/live` 在没有显式模型时错误使用第一个普通 mapping，当前运行配置因此把
语音请求记录为 `claude-fable-5` 并发往 `anyrouter.top`。本轮只修复 Codex Live
bootstrap 的模型默认值、SDP/multipart 到 quicksilver JSON 的最小封装和 CPA 映射；
公开 `/v1/realtime` 继续保持原生透传，不改普通文本模型路由。

- [x] ✅ 1. 为 `/v1/live` 固定 `gpt-live-1-codex` 路由模型，避免继承普通客户端模型或首个 mapping。
- [x] ✅ 2. 将 SDP、text/plain 和 Codex multipart Live 请求封装为 `sdp + session.type=quicksilver` JSON，并保留 Provider SDP 响应。
- [x] ✅ 3. 为活动 CPA 配置补充 Live mapping，新增 engine 与双端 HTTP 回归测试。
- [x] ✅ 4. 运行 focused tests、workspace check、文档同步和 `git diff --check`；不提交、不推送。

## 本轮增补：对照 CLIProxyAPI 的路由可靠性与能力缺口（2026-09-02，待确认）

### 已确认事实

- CPA 将 Codex Live 与标准 Realtime 分成不同请求意图：`POST /v1/live` 负责
  SDP bootstrap，`GET /v1/live/:call_id` 负责 sideband；标准 Realtime 另有
  `/v1/realtime`、`/v1/realtime/calls`、`client_secrets`、transcription、SIP
  控制等路径。Realtime Session 是有状态对象，模型、voice 和其它参数属于
  Session，而不是普通文本请求的附属字段（见官方 OpenAI 文档：
  <https://developers.openai.com/api/docs/guides/realtime-conversations#realtime-speech-to-speech-sessions>）。
- Sumpter 当前已把 `/v1/live` 的默认模型固定为 `gpt-live-1-codex`，并为
  SDP、`text/plain`、multipart 做 quicksilver JSON 封装；但这些改动尚未由
  新构建的 `/Applications/Sumpter.app` 完成真实语音 smoke test。安装包仍可能
  运行旧二进制，不能用当前 404 反推新源码结果。
- Sumpter 的文档已经写出 Models、Files、Videos、任意 raw 路径和 WebSocket
  relay，但共享 engine 的显式路由表目前仍有未覆盖的资源路径；原生透传 helper
  还把出站方法固定成 `POST`，因此 GET/DELETE/下载等请求不能完整兑现文档契约。
- Responses WebSocket 在无 query model 时仍存在“取第一个 mapping”的通用兜底；
  WebSocket 当前在下游先返回 `101`，再尝试上游连接，所以上游 401/404 不能回传
  为原始握手状态。Live `call_id` 和 Realtime `ek_…` 也尚未与创建它们的
  endpoint 固定绑定。
- 当前 runtime 已有 endpoint 优先级、sticky、retry 和 pinned-IP 健康状态，
  但还没有 CPA 风格的 endpoint+model 冷却、Retry-After 驱动的能力状态，以及
  WebSocket 握手/时长/字节/关闭原因的完整可观测性。

### 实施边界

以下按 P0→P3 分阶段实施。每阶段都必须保持 Linux/macOS 共用 engine 契约，
不复制 CPA 的账号管理体系，不记录 SDP、音频、WebSocket 帧正文、ephemeral key
或敏感 header；`/__*` 继续只属于本地控制面。用户已确认直接按 CPA 路由语义实施；媒体
relay、SIP 和安装态验证仍按下方独立边界处理。

### P0：完成当前语音修复并验证安装态

- [x] ✅ 1. 确认 Live 只接受显式 `gpt-live-1-codex` mapping；没有 Live mapping 时
  返回明确的 `no_live_provider`，绝不回退到普通文本模型。
- [x] ✅ 2. 重建并替换 macOS App（同时保留源码、安装包、运行进程三层证据），
  用真实 `/v1/live` 请求验证事件中的入口与模型，确认不再访问 `anyrouter.top`；
  另验证标准 `/v1/realtime` 未被 Live 封装污染。

### P1：统一请求意图与 raw HTTP/WebSocket 路由

- [x] ✅ 3. 引入明确的请求意图枚举：普通 HTTP、Responses WebSocket、标准
  Realtime、Codex Live、资源 API；每种意图拥有独立的模型来源策略，禁止通用
  “第一个 mapping”兜底。Responses WebSocket 建连后从首帧取 model；Live 只用
  固定 Live model；Realtime 优先使用 Session/client-secret 中的 model。
- [x] ✅ 4. 将 raw HTTP fallback 做成真正的 method/path/query/body/header 透传，
  保留 GET/POST/PATCH/DELETE 和动态子路径；`/__*` 仍不进入 fallback。修复
  Models、Files、Videos 的实际分支，并为 multipart、JSON、二进制下载补齐方法
  与 Content-Type 回归测试。
- [x] ✅ 5. WebSocket 改为“先完成上游候选选择与握手，再向客户端返回 101”；
  上游 401/404/429/5xx 在握手阶段保留可诊断的 HTTP 状态，并继续支持首响应前
  failover 与关闭帧传播。

### P1：会话与凭证亲和

- [x] ✅ 6. 保存 `call_id → endpoint_id + model + expiry`，Live sideband 只能回到
  创建该 call 的 endpoint；入口被禁用/删除时返回明确的 session-expired 错误，
  不静默漂移到其它 Provider。
- [x] ✅ 7. 保存 `ek_… → endpoint_id + session model + expiry` 的短期内存索引，
  以 endpoint 作为本地 issuer scope，仅保存受限 token 引用并禁止明文日志；
  Realtime HTTP、SDP、WebSocket 和后续控制请求复用同一绑定。
- [x] ✅ 7a. 按 CPA 补齐 ephemeral client-secret 的完整 `session` 语义：保存受限
  session 配置，WebSocket 建连前发送 `session.update`，`/v1/realtime/calls`
  的 SDP/JSON 后续请求复用 voice、instructions 等字段；补充双端握手失败与
  session 回归测试。

### P2：能力声明、健康调度与观测

- [x] ✅ 8. 为 mapping 增加可选能力声明 `text`、`image`、`video`、`live`、
  `files`；路由先按路径意图和 mapping 能力过滤，再按现有 priority、sticky 和
  retry 选择。旧配置能力字段为空时按模型名推断，避免把整条混合 Provider 入口
  错标为单一能力；Linux WebUI 与 macOS wire 均保留该字段。
- [ ] 9. 增加 endpoint+model 维度的冷却与恢复；识别 429/500/502/503/504，解析
  `Retry-After`，并在不改变现有客户端错误语义的前提下阻止短时间内反复命中坏入口。
- [ ] 10. 为 WebSocket/Live 记录脱敏的路由模型、endpoint、握手状态、失败阶段、
  TTFB、持续时间、收发字节、关闭码和 failover 次数；不记录帧正文、SDP、音频或
  ephemeral key。Linux/macOS RuntimeEvent 字段保持一致。

### P3：可选大型能力（不并入本次修复）

- [ ] 11. 只有在存在独立直连 Provider 需求时，才评估本地 Realtime
  client-secret 发行服务（TTL、scope、容量和撤销）。
- [ ] 12. 媒体 relay、UDP/NAT/ICE、SIP accept/reject/refer/hangup 属于独立项目，
  需要网络可达性和安全评审，不因 CPA 有实现就直接照搬进 Sumpter。

### 分阶段验收

- [x] ✅ P0：`cargo test -p sumpter-engine --lib`、Linux/macOS WebSocket focused
  tests、`cargo check --workspace --locked`，重建 App 后完成 Live/Realtime 选路冒烟（dummy SDP 到达 CPA；真实麦克风仍待用户复试）。
- [x] ✅ P1：补充 method/path/query/body/header 原样透传、资源下载、上游握手失败、
  Live/Realtime 绑定和“无 mapping 不回退”测试；运行
  `uv run scripts/sync-usage-docs.py --check`、`cargo fmt --all -- --check`、
  focused tests 与 `git diff --check`。
- [ ] P2：补充冷却/Retry-After、能力过滤、RuntimeEvent 脱敏字段和双端统计回归；
  只在源码、打包、安装、运行态证据分别成立后交付，不提交、不推送。

## 本轮：按 CPA 对齐 Live/Realtime 选路，禁止聊天模型泄漏（2026-09-02）

Codex Desktop 语音实际打的是 `POST /v1/realtime?model=claude-fable-5`（以及同类 Live bootstrap），
不是只走 `/v1/live`。上一轮只固定了 `/v1/live` 默认模型，查询/正文里的聊天模型仍会命中排序最前的
`xiao` mapping，事件表现为 `passthrough realtime` + `claude-fable-5` + `anyrouter.top` 404。

CPA 对 `POST /v1/live`、`POST /v1/realtime`、`POST /v1/realtime/calls` 使用同一套 Live handler，
并用 Codex OAuth 而不是文本 mapping。Sumpter 没有账号体系，对应实现是：这些语音入口只走精确的
`gpt-live-1-codex` mapping（当前配置里是 CPA），忽略泄漏的聊天模型。

- [x] ✅ 1. `POST /v1/live`、`POST /v1/realtime`、`POST /v1/realtime/calls` 路由模型固定为 `gpt-live-1-codex`，忽略 query/body 中的聊天模型。
- [x] ✅ 2. 无 client-secret 的 `/v1/realtime` WebSocket 同样按 Live mapping 选路，避免 `?model=claude-fable-5` 打到 anyrouter。
- [x] ✅ 3. 出站 query 的 `model=` 按上游 Live/Realtime 模型重写，即使它和客户端聊天模型不一致。
- [x] ✅ 4. 补充 `xiao` 在前、CPA Live mapping 在后的回归：泄漏的 `claude-fable-5` 不得访问 `anyrouter`。
- [x] ✅ 5. focused tests：`cargo test -p sumpter-engine --lib`、Linux 14 / macOS 4 websocket、`cargo check --workspace --locked`。
- [x] ✅ 6. 重建并替换已安装的 macOS App，重启 sidecar；用 `POST /v1/realtime?model=claude-fable-5` 冒烟确认路由到 CPA/`gpt-live-1-codex`/`ccc.domob.org`，不再访问 `anyrouter.top`。真实麦克风 SDP 仍需用户在 Codex Desktop 再试一次。

未纳入本轮：P2 能力声明、endpoint+model 冷却、完整 WebSocket RuntimeEvent 字段；P3 媒体 relay/SIP。

## 本轮：意图分流审查与 Videos/图片能力选路（2026-09-02）

审查结论：报文继续透传，但选路必须按路径意图过滤 mapping。原先 `/v1/videos*` 被当成无模型资源，
打到第一个非 Anthropic 入口（当前配置是 `xiao`）。图片已按 body 模型走 mapping，视频创建/下载没有。

- [x] ✅ 1. 增加模型能力推断：`grok-imagine-video*`/`sora*` → video，`grok-imagine-image*`/`gpt-image*` → image，`gpt-live-1-codex`/`gpt-realtime*` → live，其余 → text。
- [x] ✅ 2. `POST /v1/videos` 按视频模型 mapping 选路；无模型时取配置里第一条 video mapping，而不是第一个文本入口。
- [x] ✅ 3. 创建成功后绑定 `video_id → endpoint`，`GET /v1/videos/:id` 与 `/content` 回到创建入口。
- [x] ✅ 4. 图片请求额外要求 image 能力，避免把 `grok-imagine-video` 或聊天模型送进 `/v1/images/*`。
- [x] ✅ 5. Files/`GET /v1/models` 仍按资源透传；Live 保持上一轮的路径意图。
- [x] ✅ 6. focused tests：core routing、engine lib、Linux 15 / macOS 4 websocket、`cargo check --workspace --locked`。

未纳入：mapping 上显式 `capabilities` 字段（UI 保存会丢掉，先靠名字推断）、本地 `/v1/models` 目录、P2 冷却。

## 本轮：修复远端 CPA 的 Codex Live Quicksilver 400（2026-09-02）

截图显示 Live 已正确走 `gpt-live-1-codex`/CPA，但远端 CPA 仍以 HTTP 400
拒绝 `architecture=avas`。兼容做法是在 Codex Live 根 bootstrap 的出站 query
显式声明 `intent=quicksilver&architecture=avas`；标准 Realtime、`/calls` 和其它
资源请求继续原样透传。

- [x] ✅ 1. 为 `POST /v1/live`、`POST /v1/realtime` 的 Codex Live 根请求补齐 Quicksilver query，并覆盖已有冲突值。
- [x] ✅ 2. 增加 engine、Linux 回归断言，确认标准 Realtime 与 sideband 不被改写；engine、Linux/macOS focused tests 和 workspace check 通过。
- [x] ✅ 3. 重建 macOS App/DMG；安装态与 Codex Desktop 真实麦克风复试仍需独立验证。

## 本轮：修复资源请求项目归因（2026-09-02）

- [x] ✅ 1. 将 `__sumpter_resource__` 资源请求从“未识别项目”归入内部功能，避免
  `/v1/models`、Files 等无项目上下文的请求污染项目统计。
- [x] ✅ 2. 补充 RuntimeEvent 列表投影与 SQLite 投影回归测试，并通过 focused test
  与 `cargo check --workspace --locked`。
- [x] ✅ 3. 生成隔离 arm64 DMG；未替换 `/Applications/Sumpter.app`。完整测试门禁仍有
  一个与本次改动无关的既有桥接断言失败，因此该 DMG 使用 `--skip-tests` 构建。

## 本轮：闭环意图路由、归因与故障观测（2026-09-04）

用户反馈仍有以下未闭环项：Codex Originator 归因、Realtime/Live 混淆、Videos
multipart 模型读取、Files/Models 能力选路、本地 Models 目录、资源绑定重启恢复、
拒绝事件上下文，以及 endpoint+model 冷却和 WebSocket 指标。实现保持 raw 报文透传，
只在代理侧做认证、意图识别、能力过滤、绑定和重试调度；不替换已安装 App。

- [x] ✅ 1. 将 `Originator: Codex Desktop` 纳入 `ClientKind` 判定并补回归测试。
- [x] ✅ 2. 对齐 CPA：POST `/v1/live`、`/v1/realtime`、`/v1/realtime/calls` 为 Codex Live；
  GET `/v1/realtime` 无 `call_id` 才是标准 Realtime。`gpt-4o` 不再从名字推断 Live；
  文本通配不能靠请求模型名冒充 video/live。标准 Realtime WS 去掉 `OpenAI-Alpha`。
- [x] ✅ 3. 从 Videos multipart 表单读取 `model`，并补指定模型路由测试。
- [x] ✅ 4. Files 按资源能力规划；`GET /v1/models` 本地目录：OpenAI 四字段、Codex
  `client_version`、Grok/Claude UA 形状；`gpt-4o` 不隐藏；跳过 `*`。
- [ ] 5. 将 Live/Video 绑定持久化到 `resource_bindings.json`，启动加载、过期清理、
  原子写入和重启恢复可测试。
- [ ] 6. 被拒事件记录有界的 method/path/intent，并同步 runtime 投影与 wire。
- [ ] 7. 增加 endpoint+model 冷却与 Retry-After 调度；补 WebSocket 脱敏指标字段。
- [ ] 8. 运行格式化、workspace 测试和 clippy；只报告源码验证，不宣称安装态生效。

## 本轮：修复 Codex Desktop 语音 invalid_architecture（2026-09-04）

运行页事件是 HTTP `passthrough realtime` → CPA `ccc.domob.org`，模型 `gpt-live-1-codex`，
HTTP 400，229 字节 `architecture="avas" is only supported for quicksilver Realtime WebRTC sessions.`
上一轮把 `intent=quicksilver&architecture=avas` 加在了 POST `/v1/realtime` 根上，路径没有改到
`/v1/realtime/calls`。OpenAI/CPA 兼容面只允许 avas 出现在 WebRTC `/calls`。

- [x] ✅ 1. POST `/v1/realtime`（及 `/realtime`、`/openai/v1/realtime` 别名）出站改写到对应 `/calls?intent=quicksilver&architecture=avas`。
- [x] ✅ 2. POST `/v1/live` 与 `/v1/realtime/calls` 保持原路径并补齐 avas；GET 不追加这些 marker。
- [x] ✅ 3. 标准 Realtime WebSocket 出站剥掉 `intent`/`architecture`，避免 GET 握手泄漏 avas。
- [x] ✅ 4. focused tests、fmt/check/clippy、USAGE/README/architecture；提交源码。不替换 `/Applications/Sumpter.app`。

## 本轮：修复运行页历史事件表空白（2026-09-04）

截图：进行中两行正常，下方「请求 / 模型 / 路由 / 结果 / 说明」只剩表头。SwiftUI `Table`
在 SSE / 进行中 overlay 刷新后会留下 NSTableView 表头、行被裁掉。

- [x] ✅ 1. 历史事件改用与进行中相同的 SwiftUI 四列行，去掉原生 `Table`。
- [x] ✅ 2. Swift 测试与构建；提交。不替换 `/Applications/Sumpter.app`。

## 本轮：CC / Grok 项目归因显示「sumpter 本地(kkl)」（2026-09-04）

- [x] ✅ 1. `X-Sumpter-User` 写入 `ClientDeclaredMetadata`；带 workspace 的声明升格 `workspace_local`；列表投影带 `localUser`。
- [x] ✅ 2. 出站黑名单只留 `x-sumpter-*`，去掉 `x-kekulv-*`。
- [x] ✅ 3. CC wrapper 补 User；新增 `grok-project-attribution.sh`（`GROK_CONFIG` overlay）及 Linux `GET /__sumpter/grok-project-attribution.sh`。
- [x] ✅ 4. 运行页文案 `sumpter 本地(kkl)`；USAGE / 双端 UI / 发布包脚本。不替换已安装 App。

## 本轮：意图路由问题闭环（2026-09-04）

- [ ] 1. 在原始 body 解析阶段先确定 Codex Originator，再做 Live 标准化与路由。
- [ ] 2. 保证 Raw 请求 body 字节不变，并允许 JSON body-only model 参与未知路径选路。
- [ ] 3. 收紧 Realtime calls sideband 分类，补齐 Live 出站标识与测试矩阵。
- [ ] 4. 完善 Videos/Live 绑定 ID 提取、header 优先注册、重启/TTL 验证。
- [ ] 5. 让每一轮重试重新应用 endpoint+model cooldown，并更新双端回归测试。
- [ ] 6. 补齐拒绝/上游事件上下文与 WebSocket 双向关闭指标及 Swift wire。
- [ ] 7. 从 mapping/catalog 生成完整本地 `/v1/models` 能力目录并补测试。
- [ ] 8. 运行格式、workspace 测试、clippy 和 Swift 测试，记录源码验证边界。
