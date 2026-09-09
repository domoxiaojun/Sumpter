# Changelog

本文件是 Linux / macOS 共用的版本记录。版本以根 `Cargo.toml` 的 `workspace.package.version` 为准，WebUI 同步版本。历史条目保留当时的产品名与验证记录，不表示当前版本已发布。Linux 包内副本由 `scripts/maintenance/sync-project-metadata.mjs` 同步。

格式参考 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/)，版本号遵循语义化版本。

## [Unreleased]

## [0.4.2] - 2026-09-10

### 新增

- 事件列表、详情、SSE 与统计统一显示缓存命中证据、确认状态和 Token 用量；补充工具名称、Hook 类型及客户端会话/代理字段。
- Linux/macOS 事件详情按路由、用量、会话、响应、工具和高级诊断分组，HTTP 状态与最终结果独立展示。

### 修复

- macOS 运行事件恢复紧凑行高，以及详情左侧可滚动的客户端请求和多次上游尝试列表；切换隐藏上游时保留当前详情与请求链。
- 客户端归因安装器统一管理五种客户端的 bash/zsh 启动包装器；pi 同时安装配套扩展，合并显示安装状态，更新保留首次备份，还原保留用户后续修改。
- 对齐双端归因 Shell 选择与操作说明；保留恢复会话的原始参数，明确安装后从新终端启动客户端。
- 修复事件分页及横向滚动交互，更新已提交的 Linux WebUI 资源；恢复统一 CI 检查入口，并让 Linux 测试跳过未安装的 zsh。

### 变更与兼容性

- 共享 runtime 存储切换为 SeaORM 与内嵌 SQLite；统一用量统计、查询和导出，不需要外部数据库。
- Runtime SQLite schema 升级为 v5，projection 升级为 v10。旧库仍按既有策略阻断启动，不自动迁移或清空历史；从旧版本升级前应备份数据，保留历史需另行处理兼容性。配置文件仍为 schema v7。

## [0.4.1] - 2026-09-09

### 修复

- 兼容 Ubuntu Release runner 的 Clippy 版本，简化归因 facets 的条件判断并保持各维度忽略自身筛选的行为。
- Codex 归因脚本测试在没有 zsh 的 Linux 环境中自动跳过 zsh 专属 shell 测试，保留 bash 覆盖。

## [0.4.0] - 2026-09-09

### 新增

- Runtime 事件统一记录客户端变体、代理角色、代理名称及主子线程/回合关系，覆盖 Codex CLI/TUI/Desktop、Claude Code、Grok Build、Gemini CLI、pi 和 OpenAI 兼容入口。
- 项目归因优先使用完整 workspace，再使用客户端声明；同一 session 的唯一项目可前向补全，多项目和缺少证据分别标记为 `multiple_workspaces` 与 `project_context_missing`。
- Analytics、事件筛选、facets、维度、趋势、导出和 Linux/macOS 管理界面支持六个归因维度，并保留 `unknown` 的缺失字段语义。
- Linux 与 macOS 的 Codex 归因安装、状态检查、更新和还原入口统一接入现有客户端归因安装器。

### 变更

- Runtime SQLite schema 升级到 v4、projection 升级到 v9；旧版本数据库启动时结构化阻断，确认后清空并重建，不迁移或回填历史事件。
- 新增旧库/高版本库启动状态，Linux/macOS Admin、HTTP 和 WebSocket 授权及 macOS sidecar 均遵循同一阻断策略。
- 主代理、子代理、guardian、review、memory、title、automation、system 和 ambient 请求纳入统一归因统计，同时保持内部 header 在出站前剥离。
- 统一安装脚本由脚本本身获取配套资源，Linux 远程安装与 macOS 内置资源保持一致；同步更新使用说明、架构和项目结构文档。

### 修复

- 修复归因字段在 runtime 列表、SSE、统计、导出与双端 UI 之间缺失或口径不一致的问题。
- 修复旧 WebUI hash 资源残留，重建并同步 Linux 管理台静态资源。

### 验证

- Rust workspace、WebUI、macOS 构建与测试，以及 docs、脚本同步、Shell/Node 语法和差异检查全部通过。

## [0.3.9] - 2026-09-08

### 修复

- Linux 发布包和安装器在 UTC 下归一化解压文件 mtime，避免 UTC+ 时区将 epoch 0 解释为 Unix epoch 前时间并触发 Admin 静态文件服务异常。

## [0.3.8] - 2026-09-08

### 修复

- Linux WebUI 在局域网 HTTP 管理页新建模型组无响应：不再依赖 `crypto.randomUUID`。
- 统计页复制事件 JSON 在非 HTTPS 下无反馈；主题写入 localStorage 失败时不再整页起不来。

### 新增

- Linux system 安装可从标准 Kekulv 布局迁移：脚本单独放到旧服务器，按架构从 GitHub Release 下载并校验 SHA-256。
- 统一归因 wrapper 支持临时运行 pi（`run pi`），与扩展同目录即可。

### 文档

- 安装、归因改为 GitHub Release / 仓库 raw；归因在启动客户端的主机执行，可先临时 `run` 再 `install`。
- README 与使用说明对齐 schema v7 与当前客户端集合。

## [0.3.7] - 2026-09-08

### 修复

- macOS 打包优先使用 hdiutil 创建 HFS+ / UDZO 镜像，修复发布 runner 上 diskutil 格式不受支持导致的 DMG 生成失败。
- 包含 0.3.6 的 pi 本机安装、远程管理及 CI 修复。

### 文档

- 使用说明与 Linux 安装入口改为 GitHub Release / 仓库 raw 链接；归因脚本默认从仓库下载，旧的独立 CC/Grok 配置器说明降为兼容路径。
- README 与现行使用说明对齐 0.3.7 / schema v7：安装、客户端、默认端口、统一归因和 CI/Release 分工。

## [0.3.6] - 2026-09-08

### 修复

- macOS 的 pi 归因接入本机检查、安装与还原按钮；Linux 使用同一安装器，通过远程脚本下载资源并管理客户端主机配置。
- pi 重复安装与更新保留首次备份；还原恢复原扩展，原先未安装时移除扩展。全部客户端操作包含 pi，下载失败时不改写配置。
- 同步 Linux WebUI 发布资源，修复 Rust 1.88 Clippy 检查失败。

## [0.3.5] - 2026-09-08

### 修复

- 统一 macOS 与 Linux 的 pi 项目归因安装入口；macOS 支持本机安装，Linux 支持远程脚本安装、状态检查与还原。
- 收敛 GitHub Actions，删除重复的容器 PR 校验与独立链接 CI。


### 工程与文档

- 统一项目首页、共享配置说明、开发与贡献规则、安全报告流程及新仓库迁移说明。
- 增加统一检查入口和 macOS CI，集中版本记录，清理过时的独立 Linux 发布流程。

## [0.3.4] - 2026-09-08

### 新增

- 配置升级到 schema v7，增加模型组、入口绑定与双端模型组编辑器。
- 统一 Claude Code / Grok Build / Gemini CLI 项目归因安装器；内置 pi 项目与会话归因扩展。
- macOS 归因面板自动检查安装状态，支持安装配置、还原配置和操作后复查；Linux 新增交互式一键配置脚本，支持远程下载与状态检查。
- 支持 Gemini Developer API 原生请求与流式响应，补齐两端协议合同测试。
- 会话粘性时长可配置，统计页支持按项目清除粘性归属。
- Runtime Analytics 增加 Codex 线程功能分类与归因范围：`ambient_*`、自动化、审查、记忆整理和子代理等无项目上下文的请求单独归入后台功能；普通项目排行、facets、维度分页和项目导出不再膨胀 `unidentified_project`，总请求量仍完整保留。
- 运行事件投影升级到 v4，列表、SSE、历史回填、Analytics 和导出统一返回 `codexThreadClass`、`attributionScope`，并保持 Linux/macOS wire 一致。
- Provider 探测复用数据面的鉴权和指纹头，入口连接复用、模型目录去重、运行库原子重建及统计范围边界进一步收口。
- 新增只读的 Scriptable、`tsx` 和 Scripting 小组件/脚本示例，用于查看 Linux 进行中的客户端请求。

### 修复与性能

- 修复“今天”和相对时间范围的边界合并、项目筛选/分页/导出的一致性，以及存储卡有效占用与文件大小展示混淆。
- 收紧 Web Admin 运行页、统计页、Provider 表格和窄窗口布局；进行中请求保持低开销、局部化视觉反馈。
- 补齐跨端 runtime 诊断字段、文档、兼容回填和契约测试；不从线程 ID、安装 ID、代理路径或 `parentThreadID` 猜测项目。

### 验证

- 双端源码、Rust/WebUI/Swift 测试与差异检查已通过；workspace 与 WebUI 版本统一为 `0.3.4`。

## [0.3.3] - 2026-08-27

### 新增

- macOS 与 Linux 的 Provider 入口交互继续对齐：入口支持整行拖拽排序，目标行按上半部/下半部决定前后插入，并保留键盘/按钮排序备用操作。
- Provider 入口表补齐固定列宽、局部横向滚动、入口专属成本价格入口和模型目录操作；Linux 明确提示任意列均可拖动。
- 运行页与统计页继续使用共享主题表面、响应式分页和进行中请求的低开销多色流动背景；macOS 进行中请求组改用纯 SwiftUI 动效层。

### 修复与性能

- 修复 macOS Provider 原生 `SwiftUI.Table` 拖拽与范围多选竞争导致拖动时连续多行被选中的问题；改为可控行列表，普通单击、Command/Shift 多选与拖拽排序互不干扰。
- 修复 Provider 拖拽期间数据源同步重入导致的卡死风险；排序保存延后并合并快速连续操作，控件点击不再被拖拽劫持。
- 收口双端统计/运行页表格样式、窄窗口布局、滚动边界、分页跳转、缓存命中率与入口维度展示，避免横向内容挤压和无效省略。

### 验证与发布

- macOS XCTest 131 项、Swift Testing 28 项、Linux WebUI 测试 95 项全部通过；发布前执行双端版本、格式、差异和构建门禁。

## [0.3.2] - 2026-08-25

### 新增

- Analytics v3 统计工作区按“概览、趋势、错误、维度、Token/缓存、成本、存储、导出”分面，趋势一次只展示一个选定指标，并提供读屏数据表。
- Provider 配置和路由界面改用扁平 Provider 候选序列；旧池配置只在服务端迁移阶段读取，新配置与新 wire 不再输出池字段。
- 完整诊断导出增加当前、选中和全量范围向导，默认脱敏，未脱敏导出需要明确风险确认；最近事件默认每页 10 条。

### 修复与性能

- 修复筛选切换后 facet 选项消失、旧响应覆盖新结果、无分母比例误显示为 0% 等问题。
- 统计查询继续只读取规范化投影与 rollup，不按事件上限解析 `payload_json`；宽表、局部滚动、移动端抽屉和浅/深色层级统一优化。
- 修复诊断捕获 1024 MB 输入在开始/停止或刷新后回退 512 MB，以及 Linux 缓存命中率缺失的问题。

### 验证与发布边界

- 本地双端测试、WebUI 构建和 Release build 随本次变更复核；本版本仅准备源码和本地验证，不提交、不推送、不打 tag、不触发 GitHub Actions。

## [0.3.1] - 2026-08-24

### 新增

- Analytics v3 收口事件、项目、会话和错误视图：服务端分页返回精确总数，支持筛选、排序、每页
  `10/25/50/100/200` 条与稳定快照；趋势、请求链、Failover、Token、缓存和延迟统计沿用同一筛选口径。
- 保留策略增加事件数、保留天数和实时字节上限的组合配置，并提供容量、索引、投影与聚合健康状态；
  自动淘汰按完整请求链执行，不清理进行中的请求。
- 完整诊断捕获改为轻量索引优先、详情按需加载，并提供受保护的全量快照流式下载；导出前明确提示
  捕获可能含明文，避免把所有 JSON 一次性塞进浏览器内存。
- 新增 `scripts/cc-project-attribution.sh`：Claude Code 项目统计配置器，随部署包分发。安装后提供按
  当前目录生成 `X-Kekulv-Project` / `-Workspace` / `-Git-Remote` 的 `claude` wrapper，让 Web Admin
  的「项目 Token 排行」可以区分 CC 请求；此前 CC 不上行工作目录，所有请求都会堆在「未识别项目」，
  会话维度不受影响且一直是零配置的。zsh 与 bash 通用，macOS 与 Linux 通用，支持 `install` /
  `uninstall` / `restore` / `status` / `snippet` / `fish-snippet` 子命令和 `--dry-run`。
- 配置器改动最小且可逆：只向 rc 文件加入带标记的 `source` 块并写入独立 snippet，安装前自动生成
  时间戳备份，`restore` 覆盖前再保存一份 `.kekulv-prerestore-*`。如果检测到
  `~/.claude/settings.json` 的 `env` 已写死 `ANTHROPIC_CUSTOM_HEADERS`，会拒绝安装，避免环境变量
  被覆盖后静默失效。

### 文档

- `USAGE.md` §5.5 重写为分平台配置指南：明确归因 header 必须在**运行 Claude Code 的机器**上配置，
  一键三步说明 macOS/Linux 差异，手工配置降为备选。
- 修正三处会导致配置失败或误判的说明：值必须纯 ASCII（CC 遇到非 ASCII 会直接退出，中文目录名会
  让整个会话起不来）；不能写进 `settings.json` 的 `env`（会覆盖进程变量且不做插值）；以及项目名
  和本机路径并非完全不会外泄——三个 header 虽会在出站前剥离，但同一请求 body 本来就可能包含
  工作目录绝对路径、`CLAUDE.md` 全文和 `git status` 摘要。

### 性能与界面

- 热查询使用规范化投影列、组合索引和小时聚合，旧数据分批回填，避免 Analytics 逐行解析大量
  `payload_json`；分页和趋势查询不再受 100,000 条 JSON 解析上限拖慢。
- Token 数字统一使用千位分隔符，缓存命中率按协议归一化的 Token 分母计算；宽表使用固定列宽、
  数字右对齐、局部横向滚动和独立分页栏，页面纵向滚动不再被表格容器截断。

### 修复与兼容

- 修复捕获设置输入 `1024 MB` 在开始后被异步刷新回 `512 MB` 的问题，并保留编辑中的草稿。
- 最近事件分页默认改为 10 条，仍可选择 25/50/100/200 条；旧事件中的 `poolID` 只读兼容，新的事件、
  列表和详情 JSON 不再输出恒为 `primary` 的字段。
- 修复 Linux 维度缓存率查询：SQL 拼接缺少空格和新增列后的读取索引错位会让缓存分子/分母为空或查询
  失败；现在正确返回 Token/请求两种命中率及有效、未知分母，并兼容旧版统计响应。
- 普通 runtime SQLite 仍不保存 prompt、响应正文、Headers 或凭据；脱敏/原始导出边界和完整诊断捕获
  的独立存储约束保持不变。

### 验证

本次改动不触及 Rust 引擎与 daemon 二进制，仅涉及交付脚本、文档和 macOS App UI。配置器自测 59 条
（macOS 本地 bash 3.2 与 zsh 双跑；CI 的 Linux runner 显式只跑 bash 分支）；macOS 侧 129 条 XCTest
加 7 条 swift-testing 全绿。fish 的等价配置未经实测，输出中已明确标注。

## [0.3.0] - 2026-08-24

### 新增

- Runtime Analytics v2 提供稳定快照分页、精确筛选总数和完整请求链；项目、会话、错误分组都改为
  服务端搜索、排序与分页，不再要求浏览器一次加载当前保留期内的全部事件。
- 新增请求量、成功/失败/取消、Failover、Token、缓存、延迟分位数与估算成本趋势；错误聚合可按
  失败类型、阶段、入口、模型和上游状态下钻到少量脱敏样本。
- 新增运行库容量与健康探针，以及按事件数、保留天数、磁盘软上限组合执行的保留策略。淘汰以完整
  请求链为单位，进行中请求不会被清理，并分别记录累计观察、保留、自动淘汰和用户删除数量。
- 新增用户维护的精确模型价格表，使用修订号做乐观并发控制；成本结果同时报告已定价、未定价和
  Token 记账口径未知的覆盖情况，不按模型名猜测价格。
- 新增 CSV / JSONL 流式导出及导出前估算，支持事件、项目和会话范围。`stored` 仅表示导出普通
  runtime SQLite 已保存的字段，必须再次确认；它不是包含 Headers、Body 的完整诊断捕获。

### 性能

- runtime SQLite 增加可增量升级的规范化投影列、组合索引与小时聚合。新事件直接写入投影，旧事件
  由后台低优先级分批补列；热查询不再逐行解析完整 `payload_json`，启动也不会同步扫描全部历史。
- 无筛选趋势在聚合状态完整时读取小时聚合；筛选、快照边缘或价格覆盖需要精确计算时回退到规范化
  明细列。Web Admin 保留 SSE 实时更新，同时将历史浏览与长表格限制在各自的分页和滚动边界内。

### 修复与兼容

- 缓存命中率同时报告 Token 口径与请求口径的合格分母和未知覆盖率；缺字段、未知协议语义与明确
  返回 `0` 继续严格区分，避免把不可计算的记录当成 0% 或混入分母。
- reset、显式删除和自动保留分别维护快照代数与保留边界；新事件不会改变正在浏览的快照，访问已
  淘汰区间时返回明确的快照失效原因。现有 summary、事件详情、SSE、reset 和会话操作接口继续兼容。
- 普通 runtime SQLite 的隐私边界不变：不新增保存 prompt、响应正文、Headers、凭据或完整本机路径；
  完整诊断捕获仍是默认关闭、独立存储且包含敏感明文的手动功能。

## [0.2.4] - 2026-08-24

### 修复

- 修正缓存命中率分母。OpenAI/兼容协议的缓存读取是 `inputTokens` 的子集；Anthropic 的
  普通输入、缓存读取与缓存写入相互独立。界面现在统一使用服务端按协议归一化的
  `cacheReadInputTokens ÷ processedInputTokens`，未知口径或必需字段缺失时显示 `—`；此前截图中
  因重复累加得到的 `47.1%` 会正确显示为 `89.0%`。
- 诊断捕获容量输入保留用户正在编辑的草稿，异步索引刷新不会再把 `1024 MB` 改回服务端旧值
  `512 MB`。
- 汇总行遇到未知 Token 记账口径时保持 `unknown`，不再降成看似可计算的 `mixed`。

### 性能

- 诊断页先读取最多 200 条的轻量索引，只有明确选中请求后才加载明文详情；Headers、Body 与
  Chunks 默认折叠并按需生成，完整单条 JSON 只在复制或下载时编码。
- 大详情的克隆与 JSON 编码移到 blocking 线程，避免占用异步 Admin worker；单条记录本身仍可能
  接近配置的捕获容量，选择超大记录时仍需等待网络传输与浏览器解析。

### 新增

- 新增受 Admin 会话保护的 `GET /admin/api/diagnostic-capture/export`：从固定配置目录流式下载最近
  一次原子落盘的完整 `diagnostic_capture.json`。这是一个包含全部 `records[]` 的快照文件，不受
  页面索引 200 条限制，也不经过前端 `fetch`/`Blob` 聚合；导出内容未脱敏，下载前会明确确认。
- Token 统计卡片、排行、事件 trace 统一使用 ASCII 千位逗号，普通请求计数保持原展示语义。

## [0.2.3] - 2026-08-23

### 修复

#### 诊断捕获详情的键名一直是错的

`GET /admin/api/diagnostic-capture/{id}` 直接序列化 `DiagnosticRequestCapture`，于是 serde 的
`rename_all = "camelCase"` 把 `request_id` 发成了 `requestId`；而索引记录是手写 `json!`，一直
是 `requestID`。两边口径分叉的后果：

- 「下载 JSON」的文件名变成 `kekulv-diagnostic-undefined-….json`
- 「上游尝试」块里的入口 ID 与出站 URL 读不出来（前端读 `endpointID` / `outboundURL`）
- macOS App 侧那几个字段是非可选的，整条详情 `keyNotFound` 解码失败，详情面板直接打不开

`requestID` / `featureRuleID` / `endpointID` / `outboundURL` / `pinnedIP` 改为字段级显式
`rename`，并各带一个小写 `alias` —— 旧 `diagnostic_capture.json` 写的是小写形状，读不回来会让
daemon 本次运行拒写原文件。mock 一直按大写口径写，所以契约测试从未抓到这个分叉，现在补了
正反两向断言。

#### 事件列表投影漏掉客户端声明的项目归因

`GET /admin/api/runtime/events` 的紧凑投影带了 `codexMetadata` 却没有 `clientDeclared`。
Web Admin 靠单事件详情兜底所以最终能显示，但列表本身与分页数据缺字段（macOS App 以列表为
主加载路径，那边受影响更重）。投影补上该字段并加 wire 测试钉死。

### 新增

- 诊断捕获详情新增「项目」一行：读捕获记录里客户端 `X-Kekulv-*` 声明的归因，不用再去
  `inboundHeaders` 原文里翻。没有声明时明说「未声明」，并指向 Codex 工作区所在的入站 Body
  `client_metadata` —— 不把 Codex 请求笼统写成「未识别项目」。
- 捕获记录新增 `clientDeclared` 字段（已脱敏、有界，计入捕获字节预算）。

### 变更

- **运行刷新与统计刷新彻底分开。** 两个间隔各自存 localStorage（`kekulv-refresh-interval-run`
  / `-statistics`），默认运行页 5 秒、统计页 15 秒，Topbar 按当前页显示「运行刷新」或
  「统计刷新」。可选档位改为 1/2/5/10/15/30 秒。SSE 正常连接时最近事件仍即时更新，完整
  analytics/summary 按统计间隔拉取。
- **Token 展示统一为四项**：读取、写入、缓存读取、缓存命中率。命中率口径固定为
  `缓存读取 ÷（读取 + 缓存读取）`，`inputTokens` 或 `cacheReadInputTokens` 缺字段时显示
  `—` 而不是把缺失当 0 算出一个假的 0%。排行表与总量卡片同步，移除处理总量、处理输入、
  推理 Token、缓存写入等冗余口径（wire 与诊断仍保留原始字段供审计）。
- 排行排序基准从 `processedTotalTokens` 改为 `inputTokens`。
- 事件里的「主代理（未发现子代理证据）」改为「主代理」——只展示有明确证据的代理关系。
- **诊断捕获记录摘掉 `poolID`。** 池概念现在只剩配置 schema、路由内部（`RoutePlan`、粘性键）
  与事件 wire；捕获里那个恒为 `primary` 的字段是展示残留，不参与任何逻辑。索引与详情都不再
  出现它，旧捕获文件里残留的 `poolID` 会被当未知字段忽略。
- 版本号与 macOS 版统一为 `0.2.3`（发布 tag 也统一为 `v0.2.3`）。

## [0.2.2] - 2026-08-23

### 新增

#### Claude Code 项目归因

Codex 会自己上行结构化 workspace 元数据，所以统计能按项目分组；Claude Code 不会——它的
`cwd` / `workspace.project_dir` 只给本机 statusLine 和 hook 用，不进发给代理的请求，
代理在 HTTP 层看不到，于是所有 CC 请求都堆在「未识别项目」。

现在支持客户端用三个入站 header 主动声明，CC 侧通过官方 `ANTHROPIC_CUSTOM_HEADERS` 注入即可，
不用改 CC、不用包 wrapper：

- `X-Kekulv-Project` — 项目名
- `X-Kekulv-Workspace` — 工作区路径
- `X-Kekulv-Git-Remote` — Git remote

行为边界：

- 工作区只保留**脱敏后的尾两段**（`.../claude/automode-proxy`），完整绝对路径不落盘；
  Git remote 去掉凭据与 query/fragment。空值、纯空白、含控制字符的值一律丢弃。
- 三个 header 是**入站专用**，出站黑名单会剥离，绝不转发给上游中转站。
- 归因优先级是 **Codex 结构化 workspace 优先、客户端声明兜底**。声明值不复用
  `codexMetadata`，而是记为独立来源 `projectSource: "client_declared"`（界面显示「客户端声明」），
  以区分「客户端自称」与「客户端结构化采集」的可信度。
- 会话维度**无需配置**：`X-Claude-Code-Session-Id` 本来就一直在采集，也兼作粘性调度键。

配置方法见 `USAGE.md` 5.5 节。注意它是进程级环境变量，CC 启动时读一次，`cd` 到别的目录不会
自动更新，需要配 direnv 或包一层启动脚本。

#### 复制 JSON 的降级方案

- 抽出共享剪贴板工具，`navigator.clipboard` 可用时正常复制。
- 明文 HTTP 远程访问（Admin 的常见部署方式）下 Clipboard API 不可用，增加
  textarea + `execCommand('copy')` 降级。
- 处理权限拒绝与 Promise reject；**只有真正复制成功才提示成功**，失败给出 HTTPS 或权限的明确指引。
- 普通事件 JSON、Codex metadata、完整诊断捕获继续各自的脱敏边界。

### 变更

#### 移除路由预览

删除 preview-only 的 `GET /admin/api/route-preview` 及其全部配套：相关 UI、mock 数据、
wire 类型、测试、文档，以及 core 里只服务于它的 `RoutePlanner::plan_forced_feature`
系列 helper。

真实运行时路径全部保留：`RoutePlanner`、路由配置、候选匹配、路由排序、协议转换、事件诊断。

#### 统计与筛选

- 修复项目、会话、客户端三个筛选条件之间的联动：facet 不再受自身激活的筛选影响，
  选择会话 A 后同项目下的会话 B 仍然可选。
- 当前已选值即使匹配结果为 0，也不再从筛选列表中消失。
- 排行的 Token 列精简为**缓存读**与**缓存写**两列，移除输入、输出、处理输入、处理总量、
  总 Token、推理 Token、Usage 请求数等展示列。
- 所有计数与 Token 数值取消千位分隔符（`1000` 而不是 `1,000`），保持可复制、可搜索。
- 项目来源不再显示 workspace 文案，仍保留脱敏路径与来源信息。

#### 事件状态说明

列表说明改为按事件生命周期字段判定，不再用 `passthrough responses`、`bridge` 这类路由技术
标记覆盖请求结果：

| 条件 | 显示 |
|---|---|
| `phase=inFlight` | 流式输出中 |
| `outcome=succeeded` 且有 `streamTrace` | 流式输出完成 |
| `outcome=succeeded` 且无 `streamTrace` | 请求成功 |
| `outcome=failed` | 结构化失败原因 |
| `outcome=cancelled` | 取消或客户端断开 |

协议路径、路由模式与原始引擎消息仍保留在详情的诊断信息里。

#### 时间显示

- 统一显式 24 小时制，格式为 `yyyy-MM-dd HH:mm:ss` 或 `HH:mm:ss`，不再受系统 12/24 小时
  偏好影响，使用用户当前本地时区。
- 兼容 Apple reference seconds、Unix seconds、Unix milliseconds 三种时间戳，且**只转换一次**，
  修复单位识别错误导致的时间偏移与重复换算。

#### 响应式布局

- 重新审查主要表格列宽：文本、模型、项目等列弹性伸缩；数值、状态、时间、操作列使用稳定宽度。
- 长 ID、URL、路径与错误信息支持截断或换行。
- 宽表只在自身容器内横向滚动，修复窄窗口下的页面级横向溢出。
- 已在 390px / 1024px / 1440px 下做视觉验证。

### 修复

- 修复统计排行点击「请求详情」无响应：兼容 `eventIDs` / `eventIds` / `event_ids` 三种事件 ID
  字段写法，补充 mock 数据的事件 ID，并为详情加上加载状态与明确的失败信息。
- 修复详情表格第一列被压缩成逐字竖排、操作列按钮被挤压裁切的问题。
- 修复 SSE 连接正常时周期性事件同步被跳过的问题：改为基于 `afterChangeSeq` 的游标同步，
  游标为 0 不再被当作「无事可对账」（它也可能意味着新 daemon、旧事件页或刚 reset），
  支持断线重连与游标失效后自动恢复。
- 自动刷新时保留当前选中事件、滚动位置与「有新事件」提示。
- 进行中的事件耗时改为本地每秒更新，已完成事件继续用服务端最终耗时。

### 验证

Rust 全量 0 failed（`kekulv-core` 88 / `kekulv-proxy` lib 66 / engine 100 / `kekulvd` 4），
WebUI 68 项，`shellcheck` 干净，三个 installer 自测 PASS，`release-preflight PASS (0.2.2)`。

真实 daemon 端到端验证：起 `kekulvd` + 假上游打一次带三个 header 的请求，确认上游收到的 header
里没有任何 `x-kekulv-*`、没有本机路径、没有 Git 凭据；事件落库的 `workspace` 是脱敏尾段；
Admin `runtime/analytics` 返回 `projectSource: "client_declared"` 与脱敏 `workspacePaths`。

musl 二进制、systemd 与真实代理流量不在本机验证，以 CI 结果为准。

## [0.2.1] - 2026-08-23

### 变更

- 入口显式模型映射取代池级支持模型：删除 schema 的 `globalModels`，承接关系只由各入口的显式
  `mappings` 决定。旧配置由 `migrate_global_models_value` 折叠进入口映射（留 0600 备份与迁移
  提示），`schemaVersion` 保持 5，因此老用户的承接模型不会静默消失。

## [0.2.0] - 2026-08-23

首个带完整 Web Admin 的 Linux 发布版本。此前版本（0.1.x）的变化未整理进本文件，
可查 `git log` 与 `PLAN.md`。

[0.3.1]: https://github.com/domoxiaojun/sumpter/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/domoxiaojun/sumpter/compare/v0.2.4...v0.3.0
[0.2.4]: https://github.com/domoxiaojun/sumpter/releases/tag/v0.2.4
[0.2.3]: https://github.com/domoxiaojun/sumpter/releases/tag/v0.2.3
[0.2.2]: https://github.com/domoxiaojun/sumpter/releases/tag/v0.2.2
[0.2.1]: https://github.com/domoxiaojun/sumpter/releases/tag/v0.2.1
[0.2.0]: https://github.com/domoxiaojun/sumpter/releases/tag/v0.2.0
