# Sumpter 深度代码审查与维护总结

审查日期：2026-08-31  
审查对象：/Users/kkl/Documents/claude/sumpter  
用户指定仓库：https://github.com/domoxiaojun/sumpter  
本地源码快照：SOURCE_COMMIT = c75905b1553ea40d790e6ff1f3fc43ced07830d5

> 文档状态：本文主体记录的是架构搬迁前快照，文中的 `crates/kekulv-*`、
> `linux/crates/*`、`macos/crates/*` 路径和“双轨默认入口”结论属于历史证据。
> 2026-08-31 的架构整理已将当前 Rust 真源迁移到 `crates/sumpter-*`，平台实现迁移到
> `adapters/`，入口迁移到 `apps/`。当前结构、依赖方向和验证命令以
> [`architecture.md`](architecture.md) 为准；本文件保留作风险审查和迁移前对照，不要据此
> 判断当前活动路径。

## 重构后校准

- 根 `Cargo.toml` 是唯一 Rust workspace，包含 7 个 package：3 个共享 crate、2 个平台 adapter、2 个 daemon/sidecar app。
- Linux/macOS 默认入口都通过对应 adapter 构造共享 `sumpter-engine::Engine`；旧 legacy workspace 不在活动源码树中。
- `platforms/linux/webui/`、`platforms/linux/web/`、`platforms/macos/app/`、部署脚本和协议兼容字段按用户边界保持不变；它们不应被本轮架构迁移的结果误认为已完成品牌迁移。
- 已复核：`cargo fmt --all -- --check`、`cargo check --workspace --locked`、`cargo test --workspace --locked` 均通过。

## 0. 迁移前快照结论

以下结论针对迁移前快照；当前目录、依赖方向和默认入口以 `docs/architecture.md` 为准。这是一个功能面很丰富、已经有较完整测试和可观测性设计的 Rust 代理项目，适合作为认真维护的 beta/内部生产候选；但当时仍不应把它描述成“统一引擎已经成为双端默认生产路径”“干净安装后一键即可稳定运行”或“已完成生产部署”。最大的原因不是基础功能缺失，而是以下三件事同时存在：

1. 共享 kekulv-engine 已经可以构建和回放对拍，但 Linux/macOS 默认 daemon 仍主要使用各自的 legacy kekulv-proxy::engine 副本。修复共享代码不一定改变用户实际运行的路径。
2. SQLite 运行时已经有 worker、投影、rollup、分页和删除语义，但 history_generation、change_seq、pricing maintenance 等边界仍有高置信度的一致性风险。
3. 事件导出和诊断捕获的隐私边界写得很认真，但部分文档承诺与真实序列化内容不一致；如果用户把导出文件带离本机，泄露本地工作区路径或其它归因元数据的风险不能忽略。

总体判断：基础路由/鉴权/重试/事件管线已经达到“可继续开发”的水平；在把默认运行路径、历史快照和导出隐私契约收紧前，建议把项目定位为“有测试保护的 beta”，而不是无条件的生产级网关。

## 1. 审查范围、证据和边界

### 1.1 远程仓库边界

本次先实时查询了用户给出的 GitHub 仓库：

~~~
GET https://api.github.com/repos/domoxiaojun/sumpter
full_name: domoxiaojun/sumpter
size: 0
default_branch: main
private: false
archived: false

GET https://api.github.com/repos/domoxiaojun/sumpter/contents
HTTP 404
message: This repository is empty.
~~~

因此不能从远程仓库得到提交历史、分支、CI、Issue、发布物或线上配置。本目录也没有 .git：不能可靠执行 git log、git blame、远程差异审查或确认工作区的 Git 状态；本次没有初始化 Git。

本报告审查的是本地快照中的源码、测试、示例配置和文档，而不是对 GitHub 当前内容的审查。SOURCE_COMMIT 只说明快照来源，不等价于当前远程仓库存在该提交。

### 1.2 实际检查内容

- 根共享 workspace：crates/kekulv-core、crates/kekulv-runtime、crates/kekulv-engine。
- Linux legacy workspace：linux/crates/kekulv-proxy、linux/crates/kekulvd、WebUI、systemd/发布脚本。
- macOS legacy workspace：macos/crates/kekulv-proxy、macos/crates/kekulvd、SwiftUI App/Core、打包脚本。
- docs/、docs/upstream/、USAGE.md、macos/CONFIG.md、linux/specs/admin-api.md、示例配置和测试 fixture。
- 静态控制流、边界条件、错误/隐私语义、跨目录源码漂移，以及项目自身的 check/test/lint/脚本门禁。

没有做的事情：真实 Provider 流量、压力/长连接测试、真实用户数据迁移、安装后的 SwiftUI/DMG 运行验收、Linux 线上部署、生产网络和外部服务可用性验证。

## 2. 项目体量与维护地图

以下是快照的近似体量（按源码文件统计，不含生成物）：

| 区域 | 规模 | 维护定位 |
|---|---:|---|
| crates/ 共享 Rust | 36 个 .rs，约 34,730 行 | 目标中的跨平台真源；目前 unified engine 旁路使用 |
| linux/crates/ Rust | 30 个 .rs | Linux legacy proxy/daemon，另含共享代码副本 |
| macos/crates/ Rust | 30 个 .rs | macOS legacy proxy/sidecar，另含共享代码副本 |
| macos/app/Sources/ Swift | 59 个 .swift，约 28,989 行 | 菜单栏 App、Admin client、UI、sidecar 生命周期 |
| linux/webui/src/ | 约 41 个前端源文件 | React/Vite Admin UI；linux/web/ 是构建产物 |

### 2.1 目录职责

| 路径 | 责任 | 主要注意事项 |
|---|---|---|
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/config.rs | schema v6 配置模型、归一化、协议/重试字段 | 磁盘格式是契约；不要直接把 docs/upstream 当当前 API |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/config_store.rs | 配置目录、迁移、原子保存、旧格式拒绝 | 迁移先备份再替换；平台路径由外层决定 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/routing.rs | feature rule、mapping、协议候选、优先级/粘性组 | endpoint 的显式 mappings 是当前运行时路由来源 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/bridge.rs | Anthropic → OpenAI Chat/Responses SSE 与 body bridge | 增量字节解码是本次发现的重点风险 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/bridge_in.rs | OpenAI Chat/Responses → Anthropic bridge | 工具、reasoning、未知块不能静默丢弃 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/stream_terminal.rs | SSE/JSON 终态追踪 | 终态决定成功/失败/不完整的记账 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/events.rs | RuntimeEvent、Codex 元数据、诊断结构、展示分类 | payload_json 与 projection 的隐私分层要保持一致 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs | SQLite worker、事件 upsert、投影、rollup、删除/重建 | 单线程 worker 是并发和维护的核心边界 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_query.rs | 只读查询、分页、analytics、export、pricing | snapshot 与 export 必须使用同一套水位语义 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs | 共享入站、转发、重试、relay、事件和捕获 | 目前是 shared staging，不是默认双端生产入口 |
| /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/platform/ | Linux/macOS 边界注入 | 控制 token、通知、/__status 权限在这里分流 |
| /Users/kkl/Documents/claude/sumpter/linux/crates/kekulv-proxy/src/admin.rs | Linux Admin API、会话、CSRF、配置/运行时操作 | 当前 Linux Admin 不是 shared engine 的一部分 |
| /Users/kkl/Documents/claude/sumpter/linux/crates/kekulvd/src/main.rs | Linux daemon、双 listener、信号和 systemd 适配 | 默认实例仍构造 legacy Engine |
| /Users/kkl/Documents/claude/sumpter/macos/crates/kekulvd/src/main.rs | macOS legacy sidecar | unified-engine 仅 feature 开启时进入另一路入口 |
| /Users/kkl/Documents/claude/sumpter/macos/app/Sources/SumpterApp/SidecarController.swift | Swift spawn、握手、停止、孤儿 PID 回收 | PID 复用风险见风险表 |
| /Users/kkl/Documents/claude/sumpter/linux/webui/src/ | Admin UI | 改 UI 源码后重新 build；不要手工编辑 linux/web/ |

## 3. 端到端架构和请求链

### 3.1 数据面请求链

~~~
客户端 HTTP
  └─> listener 绑定
      └─> loopback/CIDR 判断
          └─> 平台控制动作鉴权（macOS 的 /__notify、/__reload）
              └─> /__status 权限判断
                  └─> 入站 auth（x-api-key 或 Bearer）
                      └─> 通过后才消费 body（上限 64 MiB）
                          └─> JSON/multipart 校验
                              └─> SourceFormat（由路径决定）
                                  └─> RoutingRequest
                                      └─> feature rule / model mapping
                                          └─> RoutePlanner
                                              └─> native 优先或 translated candidate
                                                  └─> session sticky / pinned IP 排序
                                                      └─> serial 或并发 race
                                                          └─> response header 前 retry/failover
                                                              └─> native relay 或 SSE/JSON bridge
                                                                  └─> terminal tracker
                                                                      └─> CompletionGuard
                                                                          └─> RuntimeEvent
                                                                              └─> SQLite worker/projection/rollup
                                                                                  └─> Admin 查询、SSE、导出
~~~

共享入口在 /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs:2044-2245。它保留 HTTP method/URI/header 的语义，先做访问边界，再按路径分派 /v1/messages、OpenAI Chat、Responses、Compact、Completions、Images、Alpha Search 和 /__status。

### 3.2 协议面

- Anthropic Messages：/v1/messages。
- OpenAI Chat：/v1/chat/completions 及无 /v1 别名。
- OpenAI Responses：/v1/responses、/responses、Codex backend 别名。
- 独立 adapter：count tokens、responses compact、legacy completions、images、alpha search。
- 明确不当作普通 body 透传：Responses WebSocket、Realtime/Live、Files、Videos 查询下载、/v1/models。

SourceFormat 只由路径确定，避免依据 User-Agent 或 body 形状猜协议。EndpointProtocolMode::Auto 只是入口能力模式；运行时会解析成真实的 ProviderProtocol，不会作为出站协议写入事件。

### 3.3 路由和 failover

RoutePlanner（/Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/routing.rs:825-1041）的关键语义：

- 普通请求只能命中已启用 endpoint 的显式 mappings；空 mapping 不承接模型。
- 精确 model mapping 优先于前缀通配；同级保持配置顺序。
- feature rule 先匹配；固定 endpoint 被删除/停用时可退回候选序列。
- native candidate 存在时，不把 translated candidate 混入同一轮候选。
- 入口按调度组最低 priority 排序，同组保持配置顺序。

引擎在 /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs:2973-3290 执行：先复用 session sticky 组，再按 sessionStickyRetries 重试当前组；组内使用 pinned IP 健康排序，候选多且并发度大于 1 时进行 race；响应头前的网络错误和指定 HTTP 状态才进入 retry/failover。客户端断开会通过取消传播停止后续重试。

### 3.4 响应终态与记账

CompletionGuard 在 /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs:4438-4820 负责把一次客户端请求和每次 upstream attempt 关联起来：

- 收到响应头先写 in-flight 事件，完成时用相同 event ID 原地 upsert。
- SSE/JSON 终态明确为 completed、failed 或 incomplete；不能只因 transport EOF 就算成功。
- 响应体被 drop 且没有显式完成时，补写 499/cancelled，并阻止后续重试。
- client 事件的 duration/TTFB 是请求视角；upstream 事件的 duration/TTFB 是单次尝试视角，两者不能混加。

## 4. 配置、路由和迁移契约

### 4.1 schema v6

/Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/config.rs:61-177 定义顶层 schemaVersion、listener、retry、endpoints、featureRules。加载时 from_json 保真，进入引擎前用 AppConfig::normalized() 做：

- retry 非负/并发下限归一；
- endpoint/rule 名称回退到 ID；
- sticky group、target endpoint trim；
- model 名清洗；
- 内建 websearch/webfetch/classifier 规则 canonical 化；
- 空 catalog 不落盘。

旧 schema v3/v4/v5 会在配置层迁移并生成时间戳备份；schema v6 运行时不再读取 pools、globalModels 或 target.poolID。这一点是维护时最容易被旧文档误导的地方：docs/upstream/ 是迁移参考，不是当前 wire contract。

### 4.2 listener 和安全边界

- 默认数据面是 127.0.0.1:57878；Admin 是另一个 listener（Linux 默认 127.0.0.1:57879）。
- allowedCIDRs 先于 body 读取；loopback 单独放行。
- authToken 为空才是不鉴权；非空接受精确 x-api-key 或 Authorization: Bearer。
- body 上限为 64 MiB，且错误 token 不会先缓冲大 body。
- Linux Admin 使用 HttpOnly session cookie + CSRF；/healthz 只返回 204，不泄露状态。
- /__status 脱敏返回 authToken，macOS 还支持控制 token 查询；Linux 只允许 loopback。

### 4.3 配置维护规则

1. 变更配置字段时，同时改 config.rs、golden fixture、Linux/macOS 文档和 WebUI contract test。
2. 任何新模型都必须在 endpoint 的 mappings[] 声明，不要重新引入池级全局模型表。
3. 变更协议 bridge 时，分别覆盖 native、single bridge、双向 bridge、stream=false、工具和未知块。
4. 迁移代码必须保留原文件备份，并测试损坏文件不会被 bootstrap 覆盖。

## 5. Runtime SQLite 设计

### 5.1 写入路径

/Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs:2555-2839 创建专用 runtime-sqlite worker：

- 主线程通过有界 channel 发送 Write/Flush/Reset/Recreate/DeleteSession/SetRetention/ReplacePricing。
- 相同 event ID 的 in-flight → completed 使用同一 seq，每次状态变化增加 change_seq。
- 有 pending event/byte 上限，超限进入 backpressure；数据面会返回 503，避免无限吃内存。
- 批量事务写入后标记 projection/rollup maintenance；投影列用于查询，payload_json 保留详情源。

这是一个合理的“内存快速路径 + SQLite 最终一致”设计，但调用方必须区分：内存 summary 可以比 SQLite 新，lastCommitAt/pending/rollup 状态才表示持久化进度。

### 5.2 查询和分页

runtime_query.rs 提供 events page、request chain、analytics、facets、trends、errors、dimensions、projects、sessions、storage、pricing 和流式 export。分页在同一只读事务内取得 snapshot、count 和 page，要求 projection ready，并以 historyGeneration 检测显式 reset/delete。

当前实现的隐含假设是 seq 足以代表稳定历史；但 in-flight 原地更新只改变 change_seq，这正是风险表中的 snapshot 问题来源。后续建议把 snapshot token 明确设计为 (historyGeneration, seq, changeSeq) 或数据库不可变版本，而不是只传一个 snapshotSeq。

### 5.3 rollup、pricing、retention

- 小时 rollup 只在 projection 完成、generation/seq 对齐且 pricing revision 对齐时读取。
- 价格以 exact model/effective time 匹配，未知价格和未知 token accounting 不补成 0。
- retention 当前是显式容量上限；reset/session delete 是用户触发的删除，不应被误写成后台无限清理。
- recreate 是 schema cutover，会 drop/recreate runtime-owned tables；诊断捕获在设计上不属于该操作范围。

## 6. Admin、UI 和生命周期

### 6.1 Linux

/Users/kkl/Documents/claude/sumpter/linux/crates/kekulv-proxy/src/admin.rs:556-639 注册配置、代理启停、runtime v2 查询、pricing、retention、导出、诊断捕获、凭据和 autostart 路由。admin_guard 在 :641-681 执行 session、JSON Content-Type 和 CSRF 检查。

/Users/kkl/Documents/claude/sumpter/linux/crates/kekulvd/src/main.rs:266-363 的默认启动顺序是：加载/归一化配置 → 构造 legacy Engine → 安装信号 → 启动 stats flusher → 启动 Admin → 启动 Proxy → 写 PID。关停时先停止 Admin 接入，再停止 Proxy、flush stats/capture/session affinity、删除 PID。

### 6.2 macOS

legacy sidecar 通过 stdin EOF、SIGTERM/SIGINT 和握手 JSON 与 SwiftUI 壳协作。Swift 侧 UI/状态/通知/诊断是独立实现；共享引擎的 macOS platform boundary 只在 unified-engine profile 中注入控制 token、/__notify 和 /__reload。

### 6.3 双轨是当前最大的结构性维护税

- Linux 默认 main.rs 直接 use kekulv_proxy::engine::Engine；shared engine 只在 linux/crates/kekulv-proxy/src/unified_engine.rs 通过 feature 暴露。
- macOS 默认 main.rs 在未启用 unified-engine 时导入 legacy 模块；实验入口在 macos/crates/kekulvd/src/unified.rs。
- shared/runtime 与两端副本已经有 replay/conformance 测试，但这只能证明当前快照对拍，不会自动让生产 daemon 使用 shared 代码。

因此任何后续 PR 都必须在标题或说明里写清楚“改的是 shared、Linux legacy、macOS legacy、unified profile 还是多个路径”。否则很容易出现测试全绿、用户行为未改变的假完成。

## 7. 风险分级与修复建议

严重度说明：P1 = 应在扩大默认生产范围前处理；P2 = 应进入近期迭代并补回归测试；P3 = 文档/维护质量问题，可排入常规修复。置信度是静态证据对问题成立程度的判断，不是线上发生概率。

| 级别 | 证据（绝对路径:行号） | 影响 | 建议 | 置信度 |
|---|---|---|---|---|
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/bridge.rs:597-610 | SseBlockBuffer::push 对每个网络 chunk 单独 String::from_utf8_lossy。中文/emoji 的多字节序列若跨 chunk，被截断处会产生 �，桥接正文和诊断数据被永久破坏。 | 改为字节级缓冲或增量 UTF-8 decoder；增加“一个中文/emoji 分在两个 chunk”以及 CRLF/多行 data 测试。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_query.rs:4471-4478,4550-4579；/Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/events.rs:418-486 | privacy=stored 的 privacy_scope 声称无 full paths，但 stored JSONL/CSV 直接读取 payload_json；RuntimeEvent/Codex 元数据可能含 sourceWorkspacePaths、sourceWorkspace 等本地归因字段。 | 二选一：把 stored 明确重命名为“完整本地存储备份”并在 UI/文档强警告；或生成真正脱敏的 stored projection，并测试导出中绝不出现路径、正文、header、credential。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs:4781-4898；/Users/kkl/Documents/claude/sumpter/linux/specs/admin-api.md:78-80 | kekulv-session-export-v1 直接把 change.event 序列化到 events，而文档称它是“脱敏 RuntimeEvent”。会话导出可能携带不应离开本机的归因字段。 | 明确区分 session-backup 与 session-redacted-export；若保留脱敏名称，先按字段白名单构造导出事件，不要直接序列化 RuntimeEvent。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs:2616-2665,2816-2830,4488-4583 | pricing 替换会清除 rollup、插入 dirty bucket，但 Command::ReplacePricing 成功后没有把 worker 的 projection_maintenance 置为 true，也没有强制刷新 cached storage。若 worker 先前已空闲，可能在没有新命令时不主动重建 rollup，UI 长时间看到不完整/旧状态。 | 成功分支设置 maintenance 标志并触发 idle maintenance；刷新缓存；增加“替换价格后无新写入仍最终 rollup complete”的测试。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs:3023-3044,4285-4300,4622-4653 | recreate_database 已递增 DB history_generation 并通过 refresh_cached_storage 写回内存；公共 RuntimeStore::recreate 随后又 state.history_generation += 1，可能造成内存 summary 比 DB 多 1。Admin 返回的 generation 与下一次查询可能不一致。 | 只使用底层刷新后的 generation；为 DB meta、store summary、Admin response 加一致性回归测试。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_query.rs:1759-1807,1826-1834；/Users/kkl/Documents/claude/sumpter/crates/kekulv-runtime/src/runtime_store.rs:2893-2907,1953-1965 | 历史 snapshot 只固定 completed 行的 MAX(seq)；in-flight 完成时保留 seq、更新 change_seq。当旧 snapshot 上界已包含该 in-flight 行时，后续翻页/导出可能看到后来才完成的状态，破坏“历史不变”直觉。 | 使用 (seq, change_seq) 双水位、不可变 snapshot revision，或在 snapshot 建立时固定事件版本；补“snapshot 建立后 in-flight 完成”的分页/导出测试。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/macos/app/Sources/SumpterApp/SidecarController.swift:159-171 | 启动前只读取 PID、kill(pid,0) 成功就 SIGTERM。sidecar 崩溃后 PID 被系统复用时，可能终止无关进程。 | PID 文件保存启动 nonce、启动时间和可执行路径；回收前校验 /proc（或 macOS 等价进程信息）和 nonce，不能仅凭 PID 发信号。 | 高 |
| P1/P2 | /Users/kkl/Documents/claude/sumpter/linux/crates/kekulv-proxy/src/lib.rs:13-20；/Users/kkl/Documents/claude/sumpter/linux/crates/kekulvd/src/main.rs:266-270；/Users/kkl/Documents/claude/sumpter/macos/crates/kekulvd/src/main.rs:106-140 | shared unified engine 是旁路 feature，默认 daemon 仍走 legacy 副本。共享修复可能不影响真实用户；双端副本也会继续漂移。 | 明确 cutover 计划：短期至少把 legacy/shared replay、源码同步和两端 PR CI 设为门禁；中期让默认入口使用 shared；所有 PR 标注生效 profile。 | 高 |
| P2/P3 | /Users/kkl/Documents/claude/sumpter/linux/crates/kekulv-proxy/src/admin.rs:650-667；/Users/kkl/Documents/claude/sumpter/linux/specs/admin-api.md:129-131 | middleware 明确允许无 body 的 reset/recreate，但文档称所有写请求必须 Content-Type: application/json。客户端按文档发送/不发送 {} 会得到不同结果，形成契约漂移。 | 在文档中明确 bodyless reset/recreate exception，并为带/不带 body、带/不带 Content-Type 各加契约测试。 | 高 |
| P2 | /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs:3601-3604,3717-3737,3746-3752,3781-3794 | 翻译/终态分支多处只把 HTTP 200 视为可桥接成功；若 Provider 合法返回其它 2xx，可能绕过 bridge 或被当作非预期响应。若产品只支持 200，则代码/文档未明确这一限制。 | 确认 Provider contract；若支持所有成功响应，统一使用 status.is_success() 并补 201/204 测试；若只支持 200，在 API 文档写出限制并记录失败语义。 | 中（条件性） |
| P3 | /Users/kkl/Documents/claude/sumpter/linux/crates/kekulvd/src/main.rs:8,240 | 当前 schema 已是 v6，注释和 bootstrap 日志仍写 schema v5，容易让维护者误以为 daemon 还在使用旧格式。 | 改为 schema v6，并在日志中区分“bootstrap 创建”和“v3/v4/v5 迁移”。 | 高 |
| P1（运维安全） | /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/engine/mod.rs:73-165,1085-1315；/Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/events.rs:1887-1960 | 诊断捕获是显式 opt-in，但可保存入站/出站 URL、headers、body、响应 chunks，默认上限可达 512 MiB。即使文件权限 0600、索引脱敏，也可能含 API key、Cookie、prompt 和个人数据。 | 在 UI、文档和导出响应中突出“敏感取证模式”；启动/启用时再次确认；默认更小上限；导出前显示敏感字段警告；继续保持 raw 详情只在用户明确选择后返回。 | 高 |

### 7.1 风险之间的依赖关系

~~~
双轨默认路径
   ├─> 修复 shared 不一定修复 legacy
   └─> runtime/bridge 版本容易出现行为不一致

in-flight seq + 单水位 snapshot
   └─> 分页、session export、analytics 可能看到不同历史切面

stored/session export 直接序列化
   └─> 任何新加入 RuntimeEvent 的归因字段都可能扩大导出泄露面

pricing replacement 未唤醒 maintenance
   └─> rollup 状态长期 incomplete，进一步影响 trends/cost UI
~~~

## 8. 已验证的优点

以下不是“看起来不错”，而是本地源码和测试能直接支持的结论：

1. /Users/kkl/Documents/claude/sumpter/crates/kekulv-engine/src/lib.rs 在编译期拒绝未启用或同时启用两个平台 feature，避免把 Linux/macOS 策略混进同一构建。
2. 入站请求先做 CIDR、平台动作鉴权和 token，再在 engine/mod.rs:2090-2302 消费 body；64 MiB body limit 同时用于 router 和实际读取。
3. CompletionGuard::Drop 能将未完成响应记为 499/cancelled，并且 relay 的取消会让后续 retry 停止。
4. RoutePlanner 以 endpoint 显式 mappings 为运行时来源，旧 globalModels 只在迁移阶段处理；native candidate 优先于 translated candidate。
5. pinned IP 有健康排序、冷却、轮换、并发 race；session sticky 有 TTL、容量上限和 CAS 风格替换规则。
6. RuntimeStore 使用专用 SQLite worker、有界 channel、批量事务和同 ID in-flight upsert；projection/rollup 与查询字段分层，避免每次 analytics 都反序列化完整 payload。
7. RuntimeEvent 的 client/upstream、in-flight/completed、outcome/failurePhase 和 token accounting 字段有明确注释，帮助维护者避免把不同口径相加。
8. Linux Admin 的 session + CSRF、静态资源 no-store、/healthz 最小暴露、配置脱敏和 endpoint secret 按需读取，体现了较好的控制面分层。
9. macOS sidecar 有 stdin EOF、握手、SIGTERM/SIGKILL 和诊断快照原子写入；诊断导出还检查普通文件和打开前后的 inode/device，降低路径替换风险。
10. shared/legacy 有 replay 对拍和固定 fixture；这为后续 cutover 提供了安全网，虽然目前还不能替代默认路径切换。

## 9. 验证记录

### 9.1 通过的检查

在本地快照上已执行并通过（退出码 0）：

~~~
cargo fmt --all -- --check
cargo check --workspace --features kekulv-engine/platform-linux --locked
cargo test --workspace --features kekulv-engine/platform-linux --locked
cargo test --workspace --features kekulv-engine/platform-macos --locked

cd linux && cargo check --workspace --locked
cd linux && cargo test --workspace --locked
cd macos && cargo check --workspace --locked
cd macos && cargo test --workspace --locked

cd linux && cargo test --workspace --features unified-engine --locked
cd macos && cargo test --workspace --features unified-engine --locked

uv run scripts/sync-usage-docs.py --check
actionlint linux/.github/workflows/*.yml
shellcheck linux/scripts/*.sh
shellcheck macos/app/package-app.sh
taplo check Cargo.toml crates/*/Cargo.toml linux/Cargo.toml linux/crates/*/Cargo.toml macos/Cargo.toml macos/crates/*/Cargo.toml
~~~

统一引擎 feature 测试记录：

- Linux：core 90、bridge_in 14、golden 8、routing 35、proxy 95、engine integration 102、unified_engine 3、kekulvd 4。
- macOS：core 88、bridge_in 14、golden 8、routing 35、proxy 76、engine integration 106、unified_engine 3。

此外，镜像整理阶段记录的 SwiftUI SumpterCoreTests 139 项、WebUI 113 项测试、clean npm ci build、静态资源和发布脚本检查均通过；这些是本地快照验证记录，不代表 GitHub CI 或用户机器验收已经发生。

### 9.2 未通过/需要固定工具链复核

~~~
cargo clippy --workspace --all-targets \
  --features kekulv-engine/platform-linux -- -D warnings
~~~

在本机 rustc 1.97.1 / cargo 1.97.1 下失败，主要是 collapsible_if、needless_borrows_for_generic_args、question_mark、needless_borrow，位置包括：

- /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/config_store.rs:697,1385
- /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/events.rs:1631
- /Users/kkl/Documents/claude/sumpter/crates/kekulv-core/src/stream_terminal.rs:101,407-410,553,569

这不是功能测试失败，但当前不能宣称 clippy 门禁绿。还没有在 CI 固定的 Rust 1.88.0 上复核这些 lint 是否同样触发；应把工具链版本和 lint 结果加入发布前证据。

### 9.3 Git 与部署边界

git status --short --untracked-files=all 在当前目录返回“not a git repository”。本次没有初始化 Git、没有修改业务源码、没有 commit、没有 push、没有 tag、没有部署，也没有使用 Docker 或安装数据库。

## 10. 后续开发优先级

### P0：先建立不会误导维护者的安全网

1. **决定默认引擎策略。** 短期至少在 Linux/macOS 的 PR CI 中同时跑 legacy 与 unified；长期让 shared engine 成为默认数据面，或明确维护两套实现的边界和同步机制。
2. **收紧导出隐私契约。** 先决定 stored/session export 是“完整本地备份”还是“脱敏导出”，再统一代码、Admin 文档、WebUI 文案和字段白名单。
3. **修复 runtime generation/snapshot/pricing 三个一致性问题。** 这三项直接影响分页、趋势、成本和 UI 对用户的可信度。

### P1：协议和生命周期可靠性

1. 增量 UTF-8 decoder 与跨 chunk 回归测试。
2. macOS PID 身份校验和 stale pid 测试。
3. 为 2xx bridge 行为建立明确 Provider contract。
4. 增加中途断开、上游半帧、损坏 JSON、idle timeout、并发 pinned race 的长时测试。

### P2：可维护性和运维体验

1. 统一 schema v6 文案，修正 bodyless Admin 文档。
2. 把 engine/mod.rs 按 inbound/forward/relay/events/capture/runtime API 拆成物理模块；当前单文件方法很多，审查成本高。
3. 为 runtime migration、100k 事件分页、rollup 重建和磁盘接近上限增加可重复 benchmark/故障注入。
4. 让 SOURCE_COMMIT、实际 Git SHA、构建 profile 和 release manifest 在发布中可机器验证。

### P3：后续增强

- 统一两端共享 Admin schema 的生成/契约测试；
- 将敏感诊断字段做更细粒度的按字段开关和自动过期；
- 为新维护者补充一份“请求从路径到 RuntimeEvent 的最小示例”；
- 把关键不变量（exact mapping、native 优先、snapshot generation、privacy allow-list）写成 property/regression tests。

## 11. 新人维护 SOP

### 11.1 开始前

1. 先读本文件、/Users/kkl/Documents/claude/sumpter/AGENTS.md、/Users/kkl/Documents/claude/sumpter/docs/README.md，再用 rg/fd 缩小范围。
2. 先确认改动会进入哪条运行路径：Linux legacy、macOS legacy、shared unified，还是 Swift/WebUI。
3. 不要把 docs/upstream/ 当当前实现；以当前源码、fixture、Admin 测试和 schema v6 文档为准。

### 11.2 按变更类型工作

| 变更 | 必须同步的证据 |
|---|---|
| 配置字段/迁移 | config.rs、config_store.rs、golden、两端 config 文档、损坏/旧版本迁移测试 |
| 路由/model/protocol | routing.rs、bridge/terminal、fixture、legacy/shared replay、两平台测试 |
| retry/pinned/sticky | scheduler 纯函数测试、engine integration、取消和 failover 事件断言 |
| Runtime schema/query | worker 写入、projection/backfill、snapshot、rollup、export、reset/recreate 一致性测试 |
| Admin API | middleware 鉴权/CSRF/body contract、Linux 与 macOS facade 路由、WebUI contract test |
| SwiftUI/sidecar | Swift XCTest/Swift Testing、Rust sidecar check、握手/EOF/PID/打包结构检查 |
| WebUI | 只改 linux/webui/，运行 npm test/build，再更新 linux/web/ 构建产物 |

### 11.3 最小验证顺序

~~~
cargo fmt --all -- --check
cargo check --workspace --features kekulv-engine/platform-linux --locked
cargo test --workspace --features kekulv-engine/platform-linux --locked
cargo test --workspace --features kekulv-engine/platform-macos --locked
cd linux && cargo test --workspace --locked
cd macos && cargo test --workspace --locked
cd linux && cargo test --workspace --features unified-engine --locked
cd macos && cargo test --workspace --features unified-engine --locked
~~~

涉及文档、TOML、Shell、Actions 时，再分别运行 uv run ... --check、taplo、shellcheck、actionlint。不要使用 --all-features，因为 engine 要求恰好一个平台 feature；本机没有 Docker，也不要为本地验证安装数据库或 Docker。

### 11.4 发布和问题报告

问题报告只放脱敏的 request ID、时间、错误阶段、HTTP 状态和 profile；不要粘贴 API key、Cookie、prompt、完整路径或 raw diagnostic。发布说明必须分别写：

- 实际构建的 branch/ref/SHA 和 engine profile；
- 测试命令及结果；
- 是否验证了安装包结构、真实 Provider、干净环境和部署；
- schema/SQLite/配置是否有迁移或不可逆变化。

“构建通过”“tag 已推送”“CI 已排队”都不等于用户已安装、服务已启动或线上流量已成功。

## 12. 本轮交付边界

- 已新增本报告：/Users/kkl/Documents/claude/sumpter/docs/code-review-2026-08-31.md。
- 只把审查计划 1–7 项标记为完成；保留用户本机运行 macOS SwiftUI/DMG 或 Linux feature 构建的最终验收项。
- 未修改业务代码，因此本报告中的风险是给后续开发排期和回归测试使用的审查结论，不是已经实施的修复。
- GitHub domoxiaojun/sumpter 当前为空，本次依据本地源码快照；远程仓库恢复内容后，应重新做一次提交/CI/发布差异审查。
- 本地 check/test 不能替代真实 Provider 流量、压力测试、安装运行、跨架构打包、证书/网络环境和生产部署验证。
