# 单一引擎并行迁移执行记录

> 目标：在不改变旧引擎默认运行路径、schema v6、HTTP wire 或 SQLite 格式的前提下，建立
> `shared/` 单一引擎真源，使用整文件平台条件编译与边界注入，完成双端旁路验证。
>
> 边界：本文件只记录本次实现；已有 `plan.md` 和其它未提交文件不覆盖、不重写。本轮不提交、
> 不 push、不打 tag、不部署、不使用 Docker。

## 阶段

- [x] ✅ 1. 审计工作区与建立安全网
- [x] ✅ 2. 建立 `shared/` crates、feature 互斥检查和 boundary API
- [x] ✅ 3. 接入 Linux/macOS 可选 unified-engine facade，legacy 保持默认
- [x] ✅ 4. 加入契约/replay 基础测试并完成双端 feature 构建
- [x] ✅ 5. 同步最终用户开箱指导和 Help 状态引导
- [x] ✅ 6. 完成最小必要验证并记录未执行的外部验收

## 记录

实现过程中每完成一个阶段，在对应条目前增加 `✅`，并在下方记录验证命令与结果。

### 阶段 1 记录

- 保留既有 `plan.md` 修改和未跟踪的架构分析文档，未覆盖或清理。
- Linux `cargo test --workspace --locked`：349 项通过。
- macOS `cargo test --workspace --locked`：328 项通过。
- 旧引擎仍是两端默认实现；本阶段没有修改生产路由。

### 阶段 2 记录

- 新增 `shared/Cargo.toml`、`kekulv-core`、`kekulv-runtime` 和正式包名
  `kekulv-engine`；没有建立 `engine-v2`、`linux-new` 或 `macos-new`。
- `kekulv-engine` 的 `platform/mod.rs` 只按整文件选择 `linux.rs` 或
  `macos.rs`；无 feature 与同时启用两个 feature 都以同一条
  `compile_error!` 拒绝编译。
- 公共入口提供非泛型 `Engine`、`EngineServices`、`InboundRequest`、
  `PlatformBoundary` 和受限 `EngineCapabilities`。`handle_inbound_request`
  固定先做 CIDR，再做平台控制鉴权/动作鉴权，再进入入站 auth，最后才消费
  body；公共 `engine/*` 文件没有平台分支。
- `cargo test --workspace --locked --features platform-linux`：224 项测试通过；
  `--features platform-macos`：226 项测试通过；新增测试确认错误入站 auth
  在消费请求体前返回、且未注入的空 macOS control token 永不授权，保持请求体
  惰性和控制边界。

### 阶段 3 记录

- Linux/macOS `kekulv-proxy` 均新增可选 `unified-engine` feature 和整文件
  facade；默认 feature 仍为空，旧 `engine.rs` 未删除、未改为默认实现。
- facade 暴露共享 core/runtime、engine、replay 类型和 typed router；Linux 使用
  `platform-linux`，macOS 使用 `platform-macos`。两端 `cargo test --workspace
  --locked --features unified-engine` 均通过，且各自 facade 契约测试通过。
- macOS 平台文件保留 control token、`/__notify`、`/__reload`、通知去重和
  迁移通知；Linux 平台文件保留 loopback `/__status` 策略，控制写端点继续
  由 Linux facade/既有 Admin 处理。

### 阶段 4 记录

- replay/conformance 使用 `ReplayTransport`、legacy 专用 mock transport 和固定夹具，
  不访问真实 Provider；Linux/macOS facade 测试都把冻结的旧 `engine.rs` 与 shared
  `kekulv-engine` 逐请求对拍，覆盖 503 → 第二入口成功的重试顺序、稳定响应头、
  SSE 帧顺序和 RuntimeEvent 投影。
- 规范化随机 ID、请求时间、持续时长和流式时间字段后，legacy/shared 回放无未裁定差异。
  曾发现 `StreamTrace.last_chunk_at_ms` 的时钟抖动，已纳入比较投影；这不是
  放宽业务语义，帧顺序、字节数、终态和 usage 仍严格比较。
- 旧 workspace 回归：Linux 349 项、macOS 328 项通过；启用 unified-engine
  后分别为 Linux 352 项、macOS 331 项通过（新增每端 2 个 legacy/shared
  replay 对拍用例）。
- 无 feature 与双 feature 的负向编译检查均按预期失败，并包含
  `enable exactly one platform feature`。

### 阶段 5 记录

- 建立 canonical `docs/usage-onboarding.md` 与
  `docs/usage-path-matrix.json`，由 `scripts/sync-usage-docs.py` 生成根
  `USAGE.md` 和 Linux 发布根 `USAGE.md`；路径矩阵明确配置、Admin、鉴权、
  mapping、归因脚本所在机器和脱敏边界。
- Linux WebUI Help 与 macOS Help 使用统一六状态状态机：未启动、未配置、无
  mapping、客户端未接入、首次失败、首次成功；每个状态只给一个主要下一步。
- `uv run --no-project scripts/sync-usage-docs.py --check` 通过；WebUI
  `npm test` 113 项通过，`npm run build` 通过；macOS
  `swift test --filter SumpterCoreTests` 139 项通过。构建产生的新 hashed
  WebUI 资产已保留在发布树。

### 阶段 6 记录

- 格式/脚本/发布树验证已完成：三套 workspace `cargo fmt --all -- --check`、
  `taplo check`、`bash -n`、`shellcheck`、`git diff --check` 均通过。
- `bash linux/scripts/assemble-shared-tree.sh --check` 通过；带 `--verify`
  的全新 staging tree 成功完成 Linux unified-engine `cargo check`、共享文件
  SHA-256 校验和 staging 内 `release-preflight.sh`。
- 根 Linux `release-preflight.sh` 通过；共享 SHA 清单路径已固定为相对
  `shared/` 目录，避免发布根从不同工作目录校验失败。
- 本轮没有执行：真实双端干净环境的 Claude Code/Codex 首个请求、Linux 独立
  仓库的正式发布/安装/部署、线上观察，以及任何提交、push、tag 或 schema/
  SQLite/配置迁移。默认生产路径仍为旧引擎，失败时可关闭 `unified-engine`。

### 阶段 7：macOS unified-engine 实验包

- [x] ✅ 为 `macos/crates/kekulvd` 增加仅构建时启用的 `unified-engine` feature；
  未启用时仍编译 legacy 入口。
- [x] ✅ 打包脚本增加 `ENGINE_PROFILE=legacy|unified`，统一引擎默认输出到带
  `unified-engine-test` 标记的独立 DMG/ZIP 名称，不覆盖 `macos/app/dist/`。
- [x] ✅ 生成 arm64 测试包：`macos/app/dist-unified-engine-test/`；App 元数据
  写入 `SumpterEngineProfile=unified`，sidecar `--version` 明确标记
  `(unified-engine)`。
- [x] ✅ 验证 release 编译、共享 macOS workspace 全套平台/回放测试、macOS
  facade replay 测试（3 项）、Swift 测试、DMG/ZIP 结构与 ad-hoc 签名，以及隔离配置
  下的 Admin、`/__status`、`/__notify`、`/__reload`、错误 token 和 pid 清理。
- [x] ✅ 本阶段不安装、不启动 Swift App、不公证、不发布、不提交、不 push、不打
  tag；统一 Admin 仍是用于旁路验证的紧凑兼容层，不宣称完整 UI parity。

### 阶段 8：Admin 契约修复与重打测试包

- [x] ✅ 修复 macOS unified sidecar 的统计 Admin 契约：补齐 v3 事件分页
  (`apiVersion`、`totalCount`、snapshot/history 字段)、storage、facets、trends、
  errors、dimensions、projects、sessions、pricing、export estimate，以及 Swift
  当前使用的 request-chain、session/export、diagnostic-capture 路由；legacy 默认
  引擎和旧 `engine.rs` 保持不变。
- [x] ✅ 用 release sidecar + 隔离临时配置对 Admin 路由逐项探测：summary、events、
  analytics、storage、facets、trends、errors、dimensions、projects、sessions、
  pricing、export estimate 全部 HTTP 200；事件分页字段和 storage 字段断言通过，
  未带控制 token 的请求保持 HTTP 403。
- [x] ✅ Swift 测试 139 项 XCTest + 29 项 Swift Testing 全部通过；arm64 App、
  arm64 unified sidecar、ad-hoc 签名、DMG 挂载内容和 ZIP 完整性均验证通过。
  新测试包位于 `macos/app/dist-unified-engine-fixed/`，未覆盖正式 `dist/` 包。
- [x] ✅ 本阶段仍不安装、不启动 Swift App、不公证、不发布、不提交、不 push、不打
  tag；仅提供本地测试包，真实 Provider/线上副作用未执行。
