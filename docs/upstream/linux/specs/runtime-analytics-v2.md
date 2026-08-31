# Runtime Analytics v2（Linux / macOS 同步契约）

## 目标与边界

- 普通 runtime SQLite 只保存已经脱敏、有界的事件字段；不新增保存 prompt、响应正文、Headers、
  Cookie、Authorization、API Key 或完整本机路径。
- `privacy=stored` 表示按数据库原始存储字段导出、不做第二次脱敏；它不等于完整诊断捕获。
- 实时更新继续使用现有 SSE 与 `afterChangeSeq`；稳定历史浏览使用持久化快照，两条链路互不替代。
- “全量”是当前数据库中仍保留的历史快照。界面只展示当前仍可核对的保留量和用户主动删除量；
  新版本不会再因为条数或天数自动删除事件，只有用户设置 `storageLimitBytes` 时才按容量轮换旧数据。

## 兼容策略

现有 `/runtime/summary`、游标形态 `/runtime/events`、`/runtime/analytics`、事件详情、SSE、reset、
会话删除与旧会话导出继续可用。`GET /runtime/events?view=page` 才进入 v2 分页形态；v2 参数不得与
`beforeSeq` / `afterChangeSeq` 混用。

## 稳定事件分页

`GET /runtime/events?view=page&page=1&pageSize=10&snapshotSeq=&historyGeneration=`

- `pageSize` 只接受 `10 / 25 / 50 / 100 / 200`，默认 `10`。WebUI 可记住用户选择，
  但每次超出允许集合的值都必须回到 `10`。
- 支持 `kind/outcome/clientKind/requestPurpose/requestID/endpointID/model/projectID/sessionID/`
  `failureKind/failurePhase/from/to`。
- v2 历史分页默认只统计已经持久化的 completed rows；进行中事件继续由运行页/SSE 展示，避免
  1 秒批量落盘和原地终态更新破坏精确 COUNT 与筛选快照。运行页将进行中事件作为独立的实时
  overlay 展示，并明确标注“不占用分页名额”；持久事件表的每页行数必须严格等于 `pageSize`
  （最后一页除外）。
- 首次读取在同一只读事务内取得 `snapshotSeq`、`historyGeneration`、精确 `COUNT(*)` 与当前页。
- 后续只查询 `seq <= snapshotSeq`；`historyGeneration` 变化返回 `409 runtime_snapshot_expired`。
- 返回：`apiVersion/events/page/pageSize/totalCount/totalPages/snapshotSeq/historyGeneration/`
  `resetGeneration/hasNext/hasPrevious/nextCursor/previousCursor/filters`。
- 新写入不改变历史快照；reset、会话删除等显式删除会增加 `historyGeneration`。历史上已经被
  用户重置或显式删除后，旧快照涉及已删除的区间仍可能返回 `409 runtime_snapshot_trimmed` 并给出当前 `retainedFromSeq`，
  但当前版本不会再产生新的自动淘汰区间。

`GET /runtime/request-chain?requestID=...` 返回数据库与未落盘 recent changes 合并后的 client 事件和
全部 upstream attempts，按 `seq/changeSeq` 稳定排序。

## 查询型能力

- `GET /runtime/trends?range=today|1h|24h|7d|30d|all&granularity=auto|hour|day&...filters`
  返回请求、成功/失败/取消、故障转移、Token/cache、平均 TTFB/总耗时、阈值超出数量与估算成本时序点；
  延迟只保留平均值和阈值计数。
- `GET /runtime/errors?page=1&pageSize=50&...filters`
  按 `failureKind/failurePhase/endpoint/model/upstreamStatus` 聚合，返回次数、影响请求/会话数、
  首次/最后出现、故障转移后的恢复数及少量脱敏样本 event ID。
- `GET /runtime/projects`、`GET /runtime/sessions`
  支持 `page/pageSize/search/sort/order/from/to/snapshotSeq/historyGeneration`，返回统一分页对象；
  高基数项目/会话不再随主 Analytics 一次全部返回。每个分组行返回 `clientKinds`，用于在不改变
  项目身份和来源分组的前提下展示实际发起请求的客户端。
- 原 `/runtime/analytics` 改读规范化列；详情和兼容补列期间才允许读取 `payload_json`。

### Codex 后台线程归因

事件投影额外返回 `codexThreadClass` 与 `attributionScope`。`threadSource` 的稳定分类包括
`user`、`ambient`、`system`、`title`、`automation`、`automated_review`、`guardian_review`、
`memory_consolidation`、`subagent`、`feature`、`unknown`；未知的 Codex feature 名只归入
`feature`，不把 feature 名或线程/代理 ID 当作项目。`attributionScope=project` 仅表示存在
结构化 Codex workspace 或客户端明确声明的项目；无项目上下文的后台线程为
`internal_feature`，普通未知线程为 `unknown`。

`clientRequests` 总数仍包含后台功能请求，以保持总量和趋势自洽；`projects` 与项目 facets
默认排除 `attributionScope=internal_feature`，另由 `internalFeatureRequests` 和
`internalFeatures` 展示后台功能数量与线程分类。事件详情可显示“功能线程/归因范围”，但不提供
Finder 定位按钮。该分类只基于入站结构化字段，不能从 `installationID`、`threadID`、
`agentName`、`parentThreadID` 或请求路径反推项目。

项目上下文的来源边界：`cwd`/`runtimeWorkspaceRoots` 只有在 Codex 端被写入结构化
workspace metadata 时才可用于归因；`parentThreadID` 仅用于请求链和父子关系，不是项目字段；
`projectId` 只有作为客户端显式声明（或未来协议明确的项目字段）才可信。当前 Codex Desktop
的后台线程创建逻辑可能不携带这些字段，代理无法从安装、窗口、线程或代理路径恢复项目；需要
Codex 源端在创建/继承线程时显式传递 workspace/project context 才能归入项目。

## 流式导出

- `GET /runtime/export/estimate?scope=events|projects|sessions&format=csv|jsonl&privacy=redacted|stored`
  返回 `rowCount/estimatedBytes/snapshotSeq/historyGeneration/privacyScope`。
- `GET /runtime/export` 使用相同筛选与快照，以有界 channel 流式输出，不在 Rust 或前端拼完整文件。
- `privacy=stored` 必须带 `confirmStored=true`；响应使用 attachment、`no-store`、`nosniff`，并带快照、
  行数与隐私模式响应头。

## 存储、手动清理与价格

- `GET /runtime/storage`：精确 retained/completed/inFlight、最早/最新时间、payload/live/allocated/WAL/
  pending bytes、schema/backfill/index 状态与用户删除计数。普通 summary 只读缓存。
- `GET/PUT /runtime/retention`：只包含 `revision` 与可选 `storageLimitBytes`。旧的自动清理字段不再属于
  协议，传入后返回 400；未设置上限时统计记录只在用户明确执行 reset、会话删除等删除动作时清理。
  `storageLimitBytes` 是 SQLite 有效占用上限，达到后自动轮换最旧的已完成请求整组，新的请求继续写入，进行中的请求不会删除；传 `null` 关闭上限。
- `GET/PUT /runtime/pricing`：全量替换、`expectedRevision` 乐观并发；价格按 exact effective model 与
  生效时间匹配，使用整数 `perMillionMicros`，绝不根据模型名或别名猜价。成本响应必须给出
  `pricedRequests/unpricedRequests/unknownAccountingRequests/complete/currency/priceVersion`。
- Token/cache 继续保留 `NULL=上游未提供`、`0=明确返回 0`。用户可见的四项核心字段固定为
  `inputTokens`（读取）、`outputTokens`（写入）、`cacheReadInputTokens`（缓存读取）和
  `cacheCreationInputTokens`（缓存写入）。缓存读取的 Token 命中率只使用已知协议语义的
  `SUM(cacheRead) / SUM(processedInput)`，同时返回 eligible/unknown coverage；无合格分母时为
  `null`。缓存写入只显示写入 Token 数，不展示“写入占比/写入率”，也不能把 cache creation
  称为命中率或从混合语义、未知分母推导百分比。
- 成本只计算 completed client 请求并排除 token-count 请求和 upstream attempt；缺字段、未知协议或
  缺价格均计入未覆盖，不得补成 0 元。

## Schema 与性能

- schema v2 用快速 `ALTER TABLE ADD COLUMN` 增加 session/project/model/endpoint/protocol/failure/
  latency/Token/cache/usage-quality 等规范列，新写直接填充。
- 旧行由同一 SQLite worker 低优先级、幂等分批补列；普通写入优先，启动不得同步解析 100,000 行。
- 补列完成后后台建立经过 `EXPLAIN QUERY PLAN` 验证的组合索引并执行 `PRAGMA optimize`。
- 所有可选筛选动态生成实际 `WHERE`，禁止 `(? IS NULL OR column = ?)`。
- 无筛选趋势优先读小时聚合；带高维筛选时只扫描规范化列，不解析整条 `payload_json`。
- 写入批次与后台 projection maintenance 不执行 retention prune；保留事件计数随写入和显式删除
  事务更新。历史自动清理字段可能仍留在旧 SQLite 表中，但当前版本不读取、不显示、不执行迁移；
 旧库用户可按界面提示手动清理事件；若要移除旧版自动清理字段，必须执行“重置并新建数据库”。

基线（2026-08-24，本机只读活库约 11.7k 行）：当前可空 OR 筛选计划为全表扫描；改为动态 WHERE
后使用 `kind_seq`，20 次 warmup 后基准约从 15.9ms 降到 11.8ms。现有 `load_snapshot` 窗口查询约
41.9ms，按 kind 分别走 limit 查询约 6.2ms；最终实现需要在 100k 合成库复测且不得回退。

## 表格、列宽与滚动

- 事件、错误、项目、会话与价格表按字段类型定义稳定宽度：状态/数值/时间/操作列不被压缩，模型、
  项目与错误摘要弹性伸缩，长 ID/URL/路径支持省略并可查看完整值。
- 宽表只在自身容器产生横向滚动，不允许制造页面级横向溢出；分页区与主要操作不放进横向滚动区。
- 页面使用一个主要纵向滚动容器；表格可固定表头，但避免表格内部再出现竞争性的纵向滚动条。
- Linux 支持触控板、Shift+滚轮、键盘与可见 focus；macOS 使用原生 Table/ScrollView 行为并适配最小窗口。
- 390 / 1024 / 1440 px Web 宽度及 macOS 最小窗口都必须验证无裁切、无双滚动条和无被遮挡操作。
