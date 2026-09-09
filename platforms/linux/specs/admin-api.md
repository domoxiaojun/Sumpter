# Sumpter Linux Rust Admin API

本文是 Linux WebUI 与 daemon 之间的唯一管理契约。
运行时管理端点还包括：`GET/PUT /runtime/pricing`、`GET /runtime/export`、`GET /runtime/export/estimate`、`GET /runtime/request-chain`、`GET /runtime/facets`。历史诊断记录可能携带 `pinnedIP` 字段；新请求不再生成该字段。
源码树可执行文件是 `sumpterd-linux`，
发布包内二进制仍名为 `sumpterd`。配置格式为 `config.json` schema v7；自动迁移 schema v3/v4/v5，
旧 Swift `keys.json` API 不再适用。

## 当前运行统计契约（runtime API v1，2026-08-22）

运行统计唯一持久化后端是 `<config-dir>/runtime.sqlite3`。SQLite 使用 Rust
`rusqlite` bundled 构建、WAL 和 `synchronous=FULL`；请求热路径只更新内存快照并把事件放入
有界后台批次，SQLite 连接只属于专用 worker 线程。数据库故障时内存快照和 SSE 继续可用，
状态通过 `storage.state=degraded|backpressure` 暴露；pending 达到硬上限后业务请求返回
`503 runtime_storage_backpressure`，Admin 仍保持可用。

旧 `<config-dir>/stats.json` 仅作为只读历史归档：新版本不读取、不导入、不写入、不删除，
reset 也只清理 SQLite。Linux 旧 `GET /admin/api/runtime` 与
`POST /admin/api/reset-stats` 已删除并返回 404；proxy 数据面 `/__runtime` 与
`/__reset-stats` 始终返回 404。

运行统计 API 为：

- `GET /admin/api/runtime/summary`
- `GET /admin/api/runtime/events`（`beforeSeq`/`afterChangeSeq` keyset cursor，默认 10、最大 200）
- `GET /admin/api/runtime/events/:id`
- `GET /admin/api/runtime/analytics?range=today|1h|24h|7d|30d|all&clientKind=...&project=...&sessionID=...&from=...&to=...`
- `GET /admin/api/runtime/trends`、`/errors`、`/projects`、`/sessions`、`/storage`
- `GET/PUT /admin/api/runtime/retention`（`revision`、可选 `maxAgeDays` 时间上限与可选 `storageLimitBytes` SQLite 存储上限）
- `DELETE /admin/api/runtime/session?sessionID=...`（按完整会话键删除客户端事件、关联上游尝试及统计）
- `GET /admin/api/runtime/session/export?sessionID=...`（导出脱敏会话 JSON，不含正文、凭据或诊断捕获）
- `POST /admin/api/runtime/cleanup/preview`、`POST /admin/api/runtime/cleanup`（按 `olderThan` 一次性清理历史统计）
- `POST /admin/api/runtime/reset`
- `POST /admin/api/runtime/recreate`（删除并按当前 schema 重建 `runtime.sqlite3`）

`/runtime/retention` 接受 `expectedRevision`、`maxAgeDays` 与 `storageLimitBytes`；两个设置均可为 `null`，分别关闭对应条件。旧的自动清理字段不再属于协议，
带入后返回 400。`maxAgeDays` 是滚动 24 小时的整日保存窗口，达到后自动轮换最旧的已完成请求组；`storageLimitBytes` 是 SQLite 有效占用上限，达到后同样轮换。两者按 OR 关系执行，任一条件先达到即可触发，后台 worker 会在写入、启动、策略变更和低流量周期检查时补偿执行。
请求组内只要存在进行中事件，整组都会跳过，避免客户端事件与上游尝试链条残缺；无 `requestID` 的孤立事件按单行处理。未设置两个上限时统计事件不会按条数、天数或时间自动删除，只会在用户明确执行 reset、会话删除等
删除操作时清理。历史统计也可通过按时间清理接口由用户明确删除；升级时不会迁移旧
`runtime_retention` 列；若检测到旧表结构，存储面板会提示用户手动清理（只清空事件）或“重置并新建数据库”（移除旧字段），避免旧限制静默失效。

所有带 `range` 的运行统计端点统一接受 `today|1h|24h|7d|30d|all`。`today` 表示自然日；调用方应
同时传入本地午夜 `from`（以及可选的 `to`）。调用方提供 `from` 时服务端以该本地边界为准，
不会再与 UTC 日界取交集；省略时才使用 UTC 日界兜底，避免把缺少边界的请求静默解释为全量。

`clientKind`、`endpointID`、`project`、`sessionID`、`model`、`requestPurpose`、`outcome`、
`failureKind`、`failurePhase` 都是可选的精确筛选项，也接受对应的
`client_kind`、`session_id` 兼容参数。多个筛选项按 AND 组合；客户端事件按这些条件匹配，
关联的上游尝试通过同一 `requestID` 纳入统计。`facets` 始终列出所选时间范围内的可用
筛选值及请求数，便于前端生成下拉选项；它不包含会话正文。

`runtime/analytics` 除请求、延迟和路由维度外，还返回：

- `tokenUsage`：原始上游字段 `inputTokens`、`outputTokens`、`cacheReadInputTokens`、
  `cacheCreationInputTokens`、`reasoningTokens`、兼容字段 `totalTokens`（输入 + 输出），
  以及协议去重字段 `uncachedInputTokens`、`processedInputTokens`、
  `processedTotalTokens`、`observedRequests`。
- `projects`、`sessions`：与其它维度行相同的请求统计及上述 Token 字段。项目名有**两个来源，
  可信度不同**：①脱敏 Codex workspace 的本地目录名（客户端结构化采集，优先），仅在本地路径缺失时
  使用 Git remote 名作为降级值；②客户端用入站 `X-Sumpter-Project` / `X-Sumpter-Workspace` /
  `X-Sumpter-Git-Remote` header **自称**的归因，只在①缺位时生效，`projectSource` 记
  `client_declared` 以区分可信度（Claude Code 不上行 workspace 结构，走这条）。两者都没有为
  `unidentified_project`，多个 workspace 为 `multiple_workspaces`。这三个 `x-sumpter-*` 是入站
  专用，出站黑名单会剥离，绝不转发给上游。会话优先使用客户端事件
  顶层 `sessionID`（Claude Code 的 `x-claude-code-session-id`，也兼容 `session_id`/
  `session-id`），其次使用 Codex metadata 的 `sessionID`、`threadID`，都缺失时为
  `unidentified_session`。本地项目行另外返回可选 `workspacePaths`，内容是已经脱敏的
  workspace 路径尾部（例如 `.../projects/demo`），仅供本机界面定位目录，不包含完整绝对路径；
  客户端声明的工作区走同一字段、同一脱敏口径。
  分页维度行另外返回 `clientKinds`，仅表示该分组内实际记录到的入站客户端，不改变项目身份或来源分组。
- `GET /runtime/dimensions?kind=model`：按模型分页返回同一组请求、Token、缓存和成本字段；调用方可
  叠加 `projectID`/`project` 或 `sessionID` 精确查看某个项目、会话内各模型的分别用量。该组合筛选
  不改变全局统计快照，适合项目/会话行的局部钻取。
- `codexThreadClass` 与 `attributionScope`：Codex 事件的线程功能分类和归因范围。`ambient_*` 等
  后台线程在没有可信 workspace/client 项目声明时记为 `internal_feature`，不会进入普通
  `projects`/项目 facets 的 `unidentified_project`；总请求数仍保留，并通过
  `internalFeatureRequests`、`internalFeatures` 单独统计。代理不会根据线程 ID、agentName 或
  路径猜项目。
- `facets`：`clientKinds`、`endpoints`、`projects`、`sessions`、`models`、
  `requestPurposes`、`failureKinds`、`failurePhases` 八组 `{value,count}`，用于生成可选筛选器。
- 总体与每个维度行同时返回 `pending`/`clientPending` 和 `successRate`/
  `clientSuccessRate`。成功率分母是已完成的成功 + 失败 + 取消；未完成或旧事件未上报结果的请求
  单独计入待定，不会把待定误算成失败。

会话删除禁止使用 `unidentified_session`，删除动作在 SQLite 事务内重算 counters、入口排行和
Token usage，并递增 `resetGeneration`。导出格式为 `sumpter-session-export-v1`，只包含脱敏
RuntimeEvent 及其 requestID 关联的 upstream 尝试；不会删除或导出独立诊断捕获。

Token 只从客户端完成事件的上游公开 usage 累计，`token_count` 计数查询不计入汇总，避免
failover 上游尝试重复计数。缓存字段始终保留原始值；`processed*` 按实际出站协议避免
重复计算：Anthropic 的缓存读写与 `inputTokens` 独立，
`processedInputTokens=input+cacheReadInputTokens+cacheCreationInputTokens`；OpenAI Chat/
Responses 及兼容网关的缓存通常是输入子集，`processedInputTokens=inputTokens`。
协议无法确认时不猜测，`tokenAccountingSemantics=unknown`；同一聚合混合口径时为
`mixed`。`tokenAccountingQuality` 表示 usage 是完整、部分缺失还是混合。

缓存读写只在上游明确返回时计入：Anthropic 的 `cache_read_input_tokens`、
`cache_creation_input_tokens` 及 `cache_creation.ephemeral_5m_input_tokens`/
`ephemeral_1h_input_tokens` 会归一到统一字段；缺失与显式 `0` 保持可区分。

SSE `/admin/api/events` 的运行事件统一为 `runtime-change`，SSE `id` 是单调
`changeSeq`；in-flight 完成更新沿用原 `seq`。reset 在 SQLite 事务提交后广播 `stats-reset`，
并携带 `resetGeneration`。以下旧版 runtime 段落仅保留字段语义和历史兼容背景，不能作为当前
存储或端点契约。

## 1. 地址与安全边界

- Admin listener **默认**绑定 `127.0.0.1:57879`。
- 可通过 daemon 启动参数覆盖（**不**进入 `config.json`，SIGHUP 不改 Admin 绑定）：
  - `--admin-host <ip>` / `--admin-port <port>`
  - 环境变量 `SUMPTER_ADMIN_HOST` / `SUMPTER_ADMIN_PORT`（适合 systemd `Environment=` 或 drop-in）
  - 优先级：CLI > 环境变量 > 默认
- Admin 凭据默认读取 `<config-dir>/admin-password`；`--admin-password-file <path>` 或
  `SUMPTER_ADMIN_PASSWORD_FILE=<path>` 可覆盖，优先级为 CLI > 环境变量 > 默认路径。文件缺失时
  daemon 拒绝启动，文件上限为 16 KiB。旧格式仍接受非空 UTF-8 单行密码（可有一个末尾
  LF/CRLF），初始用户名为 `kkl`；在安全页修改凭据后，文件会原子迁移为 v1 JSON，密码使用
  Argon2 哈希保存。SIGHUP 只重载 `config.json`，不重读 Admin 凭据文件。
- WebUI 位于 `/admin/`，API 前缀为 `/admin/api/`。
- `GET /healthz` 是唯一不经过 Admin 鉴权的 listener 存活探针，成功只返回 204 空响应，
  不暴露配置、版本或运行状态。
- `/admin/` HTML、模块、字体和图标公开加载，以便呈现内置登录页；这些静态响应使用
  `Cache-Control: no-store`，避免升级后浏览器混用新旧 ES 模块。除 `GET /admin/api/auth/session` 与
  `POST /admin/api/auth/login` 外，Admin API 与 SSE 都要求有效的 HttpOnly 会话 Cookie；
  未登录或会话过期返回 401 `admin_auth_required`，同时清理过期 Cookie。
- Admin 只接受内置登录签发的会话 Cookie，不提供浏览器原生 HTTP 鉴权 challenge。会话 Cookie 使用
  `Path=/admin`、`HttpOnly`、`SameSite=Strict`、24 小时有效期；HTTPS 反代通过
  `X-Forwarded-Proto: https` 或 `Forwarded: proto=https` 告知 daemon 后，Cookie 同时带
  `Secure`。服务端最多保留 64 个内存会话，daemon 重启会要求重新登录。
- Admin 不再校验 socket peer、`Host` 或 `Origin`，以支持 loopback 上游后的公网 HTTPS
  反代域名。daemon 端口仍应只绑定 loopback；反代必须保留浏览器 Cookie，并正确设置
  `X-Forwarded-Proto`。
- 非 loopback 管理入口必须由外层提供 HTTPS；登录请求会传输用户名和密码，不得直接暴露在
  明文公网 HTTP。通常应保持 daemon loopback，只开放反代的 443。
- 不发送 CORS 许可头。WebUI 不把管理密码或会话令牌保存到 localStorage；会话令牌只存在于
  浏览器管理的 HttpOnly Cookie 中。
- 所有 POST/PUT/PATCH/DELETE 必须使用 `Content-Type: application/json`；无参数的启停、重载、
  reset、logout 也发送 `{}`，否则返回 415 `json_required`。登录后的写请求还必须发送当前
  session 响应中的 `X-Sumpter-CSRF`，缺失或错误返回 403 `csrf_required`。
- 推荐远程拓扑是 `浏览器 --HTTPS--> Nginx/OpenResty --HTTP loopback--> sumpterd`，鉴权只由
  daemon 做一层；不在反代重复配置登录挑战或其它登录页。
- 错误统一为 JSON：`{"error":"code","message":"说明"}`。

Admin 与 proxy 是两个 listener：proxy 地址来自 `config.listener`，停止或重绑 proxy 不应
中断 Admin/WebUI。

### 1.0.1 Linux proxy 内置归因脚本

Linux proxy listener 固定提供 `GET /__sumpter/setup-client-attribution.sh`、
`GET /__sumpter/client-attribution.mjs`、`GET /__sumpter/pi-project-attribution.ts`、
以及兼容路径 `GET /__sumpter/cc-project-attribution.sh` 与
`GET /__sumpter/grok-project-attribution.sh`。响应为编译期内置脚本，并带
`Content-Type: text/x-shellscript` 或对应类型与 `Cache-Control: no-store`。
用户文档的默认安装入口是 GitHub 仓库 raw；本路径供无法访问 GitHub 且代理已运行时使用。
调用方应下载后在实际运行客户端的主机执行。该路径遵守 `listener.allowedCIDRs`；当
`listener.authToken` 非空时，必须带相同的 `Authorization: Bearer <token>` 或 `x-api-key`。
除 `GET` 外返回 405。macOS sidecar 不提供该路径。

例如：

```bash
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh
bash setup-client-attribution.sh install all
```

通过 Nginx 对外提供时建议（跨机器时应）设置非空 `listener.authToken`；不要把无认证的 proxy
listener 直接暴露到公网。

### 1.1 登录、退出与修改凭据

#### `GET /admin/api/auth/session`

未登录也返回 200：`{"authenticated":false}`。已登录返回：

```json
{
  "authenticated": true,
  "username": "kkl",
  "csrfToken": "opaque-csrf-token",
  "expiresAt": 1787270400
}
```

`expiresAt` 是 Unix 秒。前端只把 `csrfToken` 保存在当前页面内存，用于写请求；刷新页面后重新
调用 session 端点。会话 Cookie 本身不可被 JavaScript 读取。

#### `POST /admin/api/auth/login`

请求 `{"username":"kkl","password":"..."}`。成功返回与 session 相同的已登录形状，并设置
会话 Cookie；失败返回 401 `invalid_credentials`。用户名或密码只用于本次校验，不写入日志或
响应。旧单行凭据首次登录仍使用用户名 `kkl`。

#### `POST /admin/api/auth/logout`

需要当前会话、JSON `{}` 与 `X-Sumpter-CSRF`。成功撤销当前服务端会话、清理 Cookie，并返回
`{"authenticated":false}`。

#### `PUT /admin/api/auth/credentials`

需要当前会话与 CSRF。请求：

```json
{
  "currentPassword": "current-password",
  "username": "new-admin",
  "newPassword": "new-password-at-least-12-chars"
}
```

用户名必须为 1-64 个非控制字符且首尾无空格；新密码必须为 12-512 个字符且不能含控制字符。
成功后使用同目录 0600 临时文件、fsync 和原子 rename 写入 v1 JSON/Argon2，撤销全部旧会话并
签发一个新会话。若 rename 已成功但父目录 fsync 失败，响应仍含新会话并附 `warning`；其它写入
失败返回 500 `credentials_write_failed`，原凭据继续有效。

忘记凭据时，在可信终端把凭据文件覆盖为新的非空单行密码并重启 daemon；这会恢复初始用户名
`kkl`。不要把密码内容输出到日志、工单或聊天。

## 2. 状态、启停与运行事件

### `GET /admin/api/status`

返回 daemon 与 proxy 当前状态：

```json
{
  "running": true,
  "daemonRunning": true,
  "runtimeApiVersion": 1,
  "state": "running",
  "generation": "config-hash",
  "version": "0.2.0",
  "uptimeSeconds": 120,
  "listener": {
    "host": "127.0.0.1",
    "port": 57878,
    "allowedCIDRs": [],
    "hasAuthToken": false
  },
  "providers": 2,
  "endpoints": 2,
  "counters": {
    "clientRequests": 0,
    "clientSuccesses": 0,
    "clientFailures": 0,
    "upstreamAttempts": 0,
    "failovers": 0
  },
  "health": {
    "state": "idle",
    "sampleCount": 0
  },
  "lastError": null
}
```

`running` 表示 proxy listener 是否运行；Admin daemon 本身能返回此响应时即仍存活。
`state` 当前为 `running|stopped`；启动或重绑失败通过对应错误响应和 `lastError` 表达。

### `POST /admin/api/proxy/start`

启动已停止的 proxy listener，使用当前已加载的配置。幂等；已运行时仍返回 200 和
`{"running":true,"proxy":{"running":true,"state":"running","host":"127.0.0.1","port":57878}}`。

### `POST /admin/api/proxy/stop`

停止 proxy listener，但保留 daemon、Admin/WebUI 与已落盘统计。幂等；响应形状同 start，
其中 `running`/`proxy.running` 为 false。

### 运行事件字段语义（适用于 v1 payload）

`RuntimeEvent` 使用 `id` 原地更新 in-flight 行；`timestamp` 是 2001-01-01 UTC 起的秒数，
`durationMS`/`ttfbMS` 是毫秒，`phase` 为 `inFlight|completed`。生产者显式写入阶段与最终结果；
字段缺失表示未记录，不从 HTTP 状态推断完成、成功、失败或取消。

`statusCode=200` 表示客户端链路记录了 HTTP 成功状态；`upstreamStatusCode` 是直接上游返回的
HTTP 状态。中转服务即使返回 200，仍可能在响应协议中报告内部调用失败，此时 `outcome=failed`
与 HTTP 200 同时保留。断流只说明响应传输/完成失败，不能据此断言中转内部模型调用失败。
未被上游明确报告的内部状态保持未知。统计与排行按 `outcome` 判定最终结果。

完整事件通过详情接口和 `runtime-change.event` 提供，分页使用轻量投影。普通事件不包含
请求正文、Authorization、Cookie 或 API Key。

### `GET /admin/api/events`

响应 `Content-Type: text/event-stream`。事件类型：

- `runtime-change`：data 为 `{seq,changeSeq,event}`，`id` 等于 `changeSeq`。
- `config-reloaded`：`{"generation":"..."}`。
- `stats-reset`：`{}`。
- `proxy-state`：含 `running`、`host` 与 `port`。

服务端只发送上述连字符规范名；客户端可为旧实现兼容下划线别名。流应定时发
`: keep-alive` 注释；客户端断开必须取消订阅。

### `POST /admin/api/runtime/reset`

在一个 SQLite 事务中删除全部 `runtime_events`、清零绝对计数并递增
`resetGeneration`；序列号不回绕，提交成功后广播 `stats-reset`。旧 `stats.json` 保持原样，
该接口保留为兼容的全量清空操作；按时间清理请使用下方的 cleanup 接口。

### `POST /admin/api/runtime/cleanup/preview` 与 `POST /admin/api/runtime/cleanup`

请求体为 `{ "olderThan": <Apple reference-date seconds> }`，删除时间严格早于截止点的
已完成事件。带 `requestID` 的事件按请求组处理：只有请求组内最新事件也早于截止点且不含
进行中事件时才会整组删除；含进行中事件或较新事件的请求组完整保留。无 `requestID` 的
孤立事件按单行处理。`preview` 只返回 `deletableEvents`、`deletableRequests` 和
`remainingEvents`；实际 cleanup 另返回 `historyGeneration`。诊断捕获、自动保留策略和
数据库结构不受影响，操作成功会广播 `stats-reset` 以使客户端刷新快照。

### `POST /admin/api/runtime/recreate`

显式删除当前 `runtime.sqlite3` 中的运行统计表，并按当前版本重新初始化数据库。
该操作会移除旧版 `runtime_retention` 自动清理字段；诊断捕获、旧 `stats.json` 归档和配置不受影响。
这是不可撤销的破坏性操作，前端必须在确认对话框中明确告知用户。普通 `reset` 只清空事件，
不会改变已有表结构，因此旧库提示只有在 recreate 后消失。

## 3. 配置读写与重载

### `GET /admin/api/config`

```json
{
  "generation": "config-hash",
  "config": {
    "schemaVersion": 7,
    "listener": {},
    "retry": {},
    "endpoints": [],
    "modelGroups": [],
    "featureRules": []
  },
  "secretStatus": {
    "inboundAuthToken": {"configured": true, "last4": "1234"},
    "endpoints": {
      "endpoint-id": {
        "apiKey": {"configured": true, "last4": "abcd"}
      }
    }
  }
}
```

- `config` 使用 schema v7 原生 camelCase 形状。每个 endpoint 必须显式写单值 `protocol`：
  `auto`、`anthropic`、`openai` 或 `openai-responses`；遗留 `protocols` 数组和
  `listener.inboundDialectPassthrough` 均拒绝。
- `auto` 只表示入口能力模式，实际 TargetFormat 由 RoutePlanner 解析为三种真实协议；
  `featureRules[].target.protocol` 仍只能是 `anthropic`、`openai` 或 `openai-responses`。
- 响应中的 `listener.authToken` 与每个 `endpoint.apiKey` 必须为空或省略，绝不返回明文。
- `secretStatus` 只表达是否已配置及可选尾四位。
- `modelGroups` 为可选数组；省略兼容旧入口映射，空数组关闭自动模型路由，显式 `null` 拒绝。组内保存 `id/name/enabled/priority/models/bindings`；绑定引用 `endpointID`，可设置 `enabled/priority/models/overrides`。绑定 `models` 省略或为 `null` 表示全部组内模型，空数组表示不承接；覆盖仅允许组内已选精确模型。组 ID 唯一、入口引用存在、优先级非负、模型范围有效，否则拒绝写入。
- `retry.max500Retries` 控制单个入口 HTTP 500 后的额外重试次数，0 表示不额外重试；
  `retry.failoverOn500` 控制 500 重试耗尽后是否切换入口，默认 `true`，关闭时直接返回当前入口的 500；
  `retry.retryDelaySeconds` 为可选正数；`retry.passThroughRetryDelay` 默认 `true`，控制最终可重试失败响应
  是否返回顶层 `retry_delay` 数字字段并附带取整向上的 `Retry-After`。
- provider apiKey 的明文只经 `GET /admin/api/endpoint-secret` 单条按需读取（见 §4）；
  配置视图这条脱敏红线不因此放宽，`listener.authToken` 与 admin 密码没有任何读取端点。

### `PUT /admin/api/config`

请求体严格为：

```json
{
  "expectedGeneration": "上次 GET 获得的 config-hash",
  "config": {},
  "secretUpdates": {
    "inboundAuthToken": "",
    "endpoints": {
      "endpoint-id": {"apiKey": "new-secret"}
    }
  }
}
```

- `expectedGeneration` 必填；与服务端当前 generation 不同返回 409 `generation_conflict`，不得覆盖。
- `config.schemaVersion` 必须等于 7。校验失败返回 400 `invalid_config`，原文件和运行态保持不变。
- `secretUpdates` 中字段缺失 = 保留；空字符串 = 明确清除；非空字符串 = 设置。
- 不能在 `config` 的脱敏 secret 字段里提交 `***` 等占位串。
- 成功写入必须使用同目录临时文件、0600 权限和原子 rename。
- 成功响应返回与 GET config 相同的脱敏快照，并附加 `warnings` 与最新 `proxy` 状态。
- host/port 变化时 daemon 负责重绑 proxy；Admin listener 始终保持可用。绑定失败返回
  500 `listener_rebind_failed`，不得把半套配置发布到运行态；磁盘回滚结果写入错误消息。

服务端至少校验：listener host（IP、`localhost` 或空值/全接口）/port/CIDR、endpoint ID 唯一、URL scheme/host、
Base URL 不得包含 userinfo、query 或 fragment、入口 `protocol` 必须是五态之一（含 `gemini`）、模型映射、feature target、
可选超时和 `retryDelaySeconds` 为正数、重试轮数/次数/时长非负；遗留的 Provider WebSearch 能力字段必须拒绝，
WebSearch 表达由严格 RequestPurpose 与最终 TargetFormat 自动选择。

### `POST /admin/api/reload`

从磁盘重新读取 `config.json`，验证后替换 Engine 配置，必要时重绑 proxy。成功响应：

```json
{
  "generation": "new-hash",
  "warnings": [],
  "proxy": {"running": true, "state": "running", "host": "127.0.0.1", "port": 57878}
}
```

SIGHUP 必须复用同一条 reload/rebind 路径。失败保持原运行配置并记录 diagnostics lastError。

## 4. Provider、路由与诊断

### `POST /admin/api/provider-models`

请求 `{"endpointID":"id"}`。服务端按当前配置查找入口；有 secret 时先按数据面同时发送
`Authorization: Bearer` 与 `x-api-key`，失败后再按单头兼容组合依次探测，
没有配置 secret 的入口只尝试一次不带鉴权头的目录请求，兼容本地/内网无鉴权上游；
请求和响应均不得回显 secret。

```json
{
  "endpointID": "id",
  "models": ["model-a", "model-b"],
  "source": "https://main.example.invalid/v1/models",
  "updatedAt": "1786233600"
}
```

`source` 是本次成功命中的模型目录 URL；`updatedAt` 当前为 Unix 秒数字符串。入口不存在返回 404；上游失败返回
502 `models_fetch_failed`。

探测的出站行为面与 `outbound.rs` 的转发路径一致：锁 HTTP/1.1、不跟随重定向、**不走代理**、
按入口 Base URL 连接，不复用连接池。目录候选会复用
`baseURL` 的自定义路径并避免重复 `/v1`；响应体限制为 2 MiB、最多保留 5000 个模型。
否则设了代理的机器上探测与真实转发会走两条不同链路，「能取到模型」不代表「转发能通」。
因此当 `HTTP_PROXY`/`HTTPS_PROXY`/`ALL_PROXY` 存在时，`models_fetch_failed`
的错误摘要会追加一句说明：探测按转发口径直连，此处失败不代表上游不可用。该说明只点出被
检测到的变量名，不回显它的值。

### `GET /admin/api/endpoint-secret?endpointID=<id>`

编辑已有入口时按需回读该入口的 provider apiKey 明文，Web 编辑框据此预填，用户可显示、
修改或清空。

```json
{
  "endpointID": "id",
  "apiKey": "sk-example-plaintext",
  "configured": true
}
```

- 与 `GET /admin/api/config` 分工明确：配置视图始终脱敏，这里是**单条、显式指定 id、
  按需**的读取，复用同一套会话 Cookie 门禁。
- 响应必须带 `Cache-Control: no-store`。
- 只开放 provider apiKey；`listener.authToken` 与 admin 密码仍然没有任何读取端点。
- 缺少或空 `endpointID` 返回 400 `missing_endpoint_id`；入口不存在返回 404
  `endpoint_not_found`；未鉴权返回 401。
- 入口没配 Key 时返回 `apiKey: ""` + `configured: false`（即无鉴权转发），不是 404。

### `GET /admin/api/diagnostics`

返回：`version`、`uptimeSeconds`、`configPath`、`webRoot`、`generation`、
`adminListener`、`proxyListener`、`proxyRunning`、`warnings:[{level,message}]`、
`lastError`、`statsWritable`、`systemdScope`（`user|system`）与对应 scope 的
`journalctlCommand`。不得返回 secret 或完整请求正文。

### 诊断捕获：索引与明文详情分离

完整诊断捕获默认关闭，内容不脱敏，累计容量默认 `512 * 1024 * 1024` 字节，写入
`<config-dir>/diagnostic_capture.json`（0600、原子替换）。手动停止、daemon 重启都不清空记录；
重启只恢复记录并保持停止，`DELETE` 才会清空。

- `GET /admin/api/diagnostic-capture`：只返回状态和 `records[]` 轻量索引，不包含 Body、Headers 或 Chunk。
- `GET /admin/api/diagnostic-capture/{requestId}`：按请求 ID返回单条未脱敏明文详情。
- `GET /admin/api/diagnostic-capture/export`：受同一 Admin 会话鉴权保护，流式下载最近一次已原子落盘的
  完整快照（一个 JSON 文件，包含全部 `records[]`，不受索引最多 200 条的限制）。响应使用
  `Content-Disposition: attachment`、`Cache-Control: no-store` 和 `X-Content-Type-Options: nosniff`；服务端从固定
  `<config-dir>/diagnostic_capture.json` 读取，不接受路径参数，也不会把快照经 WebUI `fetch`/`Blob` 聚合到前端内存。
  后台落盘尚未完成或快照暂时不可读时返回 503 `capture_export_unavailable`，导出的快照可能比内存索引稍旧。
- `PUT /admin/api/diagnostic-capture`：请求 `{"enabled":true|false,"maxBytes":number?}`；只返回
  与 GET 相同的轻量索引，不复制或回传完整捕获快照。
- `DELETE /admin/api/diagnostic-capture`：返回 `{"cleared":true}`。

索引与详情共用一套大写 ID 键名（`requestID` / `featureRuleID`，尝试里是
`endpointID` / `outboundURL`）：索引是手写 JSON，详情是直接序列化
`DiagnosticRequestCapture`，所以详情侧靠字段级 `rename` 保持一致——落到 serde 的 camelCase
默认规则上会变成 `requestId`/`endpointId`，WebUI 与 macOS 侧都读不出来。仅接受当前大写键，
不再维护旧捕获的大小写别名。详情另有 `clientDeclared`（客户端 `X-Sumpter-*` 声明的项目归因，
可选）；Codex 的结构化 workspace 不复制进捕获，仍只在入站 Body 的 `client_metadata` 里。

事件与诊断模型已删除 `poolID`；诊断尝试同时删除新请求始终为空的 `pinnedIP`。
WebSocket 摘要只保留 `clientCloseCode` / `upstreamCloseCode` 和结束方，不再提供旧汇总
`closeCode`。上述删除不触碰已有数据库或捕获文件。

索引记录包含 `requestID`、时间、请求方法/路径、客户端/模型/入口摘要、协议路径、状态/结果、
截断标记、尝试次数和客户端 Chunk 数。为避免历史抓包过多时刷新卡顿，`records` 最多返回最近
200 条；`recordCount` 是保留记录总数，`indexTruncated=true` 表示列表被截断。详情才包含入站/出站 Headers、Body、上游响应、原始
Chunk、错误和完整 JSON。页面刷新不得自动请求详情；客户端明确选择请求后再下钻，详情请求有
超时和取消保护。实现上索引元数据与完整明文记录分开缓存/加锁；后台持久化完整捕获时不会让
轻量索引请求等待正文复制。

### `GET /admin/api/autostart`

返回固定 `sumpter.service` 的 systemd 状态：

```json
{
  "available": true,
  "enabled": false,
  "unit": "sumpter.service",
  "scope": "user",
  "controllable": true,
  "reason": null
}
```

`scope=user` 时查询和控制 `systemctl --user`；`scope=system` 时查询 system manager，
`controllable=false`，`reason` 提示使用 sudo。system service 的 daemon 以低权限 `sumpter`
用户运行，不能因为 WebUI 而获得 systemd 管理权限。

### `PUT /admin/api/autostart`

请求 `{"enabled":true|false}`。实现只允许操作固定 `sumpter.service`，不得接受任意 unit、
命令或路径。user scope 的 systemd 环境不可用或命令失败时返回 503
`systemd_unavailable|systemd_failed`；随后可用 GET 重新读取状态。system scope 一律返回 403
`systemd_system_root_required`，提示运行 `sudo systemctl enable|disable sumpter.service`；不得把
daemon 改为 root 来启用该接口。

## 5. 明确移除的旧接口

- 不支持 `keys.json` 读写或自动迁移。配置目录仅有 `keys.json` 时 daemon 必须非零退出，
  原文件保持不变，并提示先人工转换为 v3 `config.json`。
- 不存在通知页、通知 Hook 或 Admin notify 事件。
- proxy listener 上的 `/__notify` 恒返回 404。
- 旧 Swift `/admin/api/main/*`、`/pools/*`、`/providers/*` 等细粒度 CRUD 不再是契约；
  WebUI 在内存中编辑完整 v3 草稿，再通过 generation-safe PUT 提交。
# Codex 元数据展示

`/admin/stats` 与运行事件中的 `codexMetadata` 可安全展示上述有界投影。详情页可以复制完整 JSON，但不得复制入站正文、Authorization、Cookie、API key 或未经脱敏的路径/URL；不存在该字段的旧事件显示为空并保持兼容。
