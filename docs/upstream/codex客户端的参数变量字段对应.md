# Codex 客户端参数、变量与事件字段对应说明

> 适用范围：本仓库 Linux 与 macOS 两套代理实现。
>
> 本文按当前源码整理，区分四类数据：**客户端原始请求参数**、**代理生成的运行事件**、**Codex 回合元数据**、**诊断抓包数据**。这四类字段不能混为一谈。
>
> 归因字段于 2026-09-07 对照本地 Codex `121f91fd5d` 的 `codex-rs/core/src/responses_metadata.rs` 复核。本文其它协议转换段落为迁移参考；当前转发契约以 `docs/architecture.md` 为准。Sumpter 的解析入口是 `crates/sumpter-core/src/events.rs`。

## 1. 先明确四类字段

| 类型 | 典型位置 | 用途 | 是否包含原始内容 |
|---|---|---|---|
| 客户端请求参数 | HTTP Header、Responses/Chat JSON body | 模型请求、工具声明、线程身份和协议参数 | 是 |
| `RuntimeEvent` | `runtime.sqlite3`、Admin API、运行监控页面 | 脱敏后的路由、状态、失败、工具和流诊断 | 否，不保存 prompt/正文 |
| `codexMetadata` | `RuntimeEvent.codexMetadata` | Codex 线程、回合、代理、工作区和工具环境摘要 | 只保存安全、有界投影 |
| Diagnostic Capture | 诊断捕获页面和捕获快照 | 临时分析完整请求链、Header、Body、Chunk | 可能包含未脱敏内容 |

因此：

- 请求体里的 `tools` 表示客户端**声明了哪些工具**。
- `RuntimeEvent.toolCalls` 表示响应流中**实际观察到调用了哪些工具**。
- `codexMetadata.toolNamespacesInfo` 表示 Codex 当时**注册了哪些工具命名空间和函数**。
- 三者含义不同，不能相互替代。

---

## 2. Codex/OpenAI 入站路径与请求用途

### 2.1 支持的主要路径

| 请求路径 | 入站协议 `sourceFormat` | 默认用途 |
|---|---|---|
| `/v1/responses`、`/responses`、`/backend-api/codex/responses` | `openai-responses` | 普通 Responses 请求，通常为 `standard` |
| `/v1/responses/compact`、`/responses/compact`、`/backend-api/codex/responses/compact` | `openai-responses` | `compact`，上下文压缩 |
| `/v1/chat/completions`、`/chat/completions` | `openai` | Chat Completions 请求 |
| `/v1/completions`、`/completions` | OpenAI Legacy Completions | `standard` |
| `/v1/images/generations`、`/backend-api/codex/images/generations` | 图片生成适配器 | `image_generation` |
| `/v1/images/edits`、`/backend-api/codex/images/edits` | 图片编辑适配器 | `image_edit` |
| `/v1/alpha/search`、`/backend-api/codex/alpha/search` | Responses 原生适配器 | `alpha_search` |
| `/v1/messages/count_tokens`、`/messages/count_tokens` | Anthropic Count Tokens | `token_count` |

`sourceFormat` 只根据入站路径确定，不根据 User-Agent 或请求体形状猜测。路径与请求体协议不匹配时，代理会拒绝请求。

### 2.2 `requestPurpose` 请求用途

`requestPurpose` 是代理生成的观测标签，只用于日志、统计和排障，不等于 Codex 的 `requestKind`，也不直接决定是否为子代理。

| 值 | 中文解释 |
|---|---|
| `standard` | 普通主对话或未命中独立用途指纹的请求 |
| `session_title` | Claude Code 内部会话标题生成 |
| `websearch` | WebSearch 搜索请求 |
| `webfetch` | WebFetch 网页抓取/总结请求 |
| `classifier` | 自动模式或安全分类器请求 |
| `compact` | Responses 上下文压缩请求 |
| `image_generation` | 图片生成请求 |
| `image_edit` | 图片编辑请求 |
| `alpha_search` | Codex Alpha Search 独立搜索 |
| `token_count` | Token 计数请求 |

注意：`standard` 更准确的含义是“普通/未命中特殊用途”，不能作为“已确认主代理”的证据。

---

## 3. Codex Responses 请求体参数

以下为代理当前能够识别、路由、透传或安全转换的主要 Responses 字段。

| 请求字段 | 类型 | 中文解释 | 代理行为 |
|---|---|---|---|
| `model` | string | 客户端请求的模型名 | 清理合法 effort 后形成 `clientModel`；路由后形成 `effectiveModel`；最终映射为 `upstreamModel` |
| `instructions` | string | 系统级/开发者级指令 | 原生 Responses 路径保留；桥接到 Anthropic 时转成 `system` |
| `input` | string/array | 用户输入、历史消息、函数调用和函数结果 | 原生路径保留；桥接时转换成 Anthropic `messages` |
| `tools` | array | 客户端声明的工具列表 | 原生路径保留；桥接时只转换能够安全表达的工具 |
| `tool_choice` | string/object | 工具选择策略 | 原生路径保留；桥接时映射为 Anthropic `tool_choice`，无法表达则拒绝 |
| `reasoning` | object | 推理配置 | 当前桥接主要识别 `reasoning.effort`；其它无法安全表达的字段会拒绝桥接 |
| `reasoning.effort` | string | 推理强度 | 支持 `none/auto/minimal/low/medium/high/xhigh/max` 等合法档位 |
| `max_output_tokens` | number | 最大输出 Token 数 | 桥接时映射为 Anthropic `max_tokens` |
| `stream` | boolean | 是否要求流式输出 | 缺省按非流式；代理仍统一观察响应终止事件 |
| `temperature` | number | 随机性参数 | 原生路径保留；桥接时按数值透传 |
| `top_p` | number | 核采样参数 | 原生路径保留；桥接时按数值透传 |
| `include` | array | 要求响应附带额外数据 | 原生路径保留；当前 Responses→Anthropic 桥接只接受空数组 |
| `client_metadata` | object | Codex 客户端附加元数据 | 解析安全字段到 `codexMetadata`，原始请求仍正常转发 |

### 3.1 `input` 数组项目

| 项目类型/字段 | 中文解释 | 桥接处理 |
|---|---|---|
| `type=message` | 普通消息项目 | 按 `role` 和 `content` 转换 |
| `role` | `system/developer/user/assistant` | `system/developer` 合并到系统指令，其他转成对话消息 |
| `content` | 文本或内容块数组 | 当前桥接安全支持文本类内容块 |
| `type=function_call` | 模型此前发出的函数调用 | 转成 Anthropic `tool_use` |
| `name` | 工具/函数名称 | 作为工具调用名称 |
| `call_id` | 工具调用关联 ID | 对应 Anthropic `tool_use.id` |
| `arguments` | JSON 字符串或对象参数 | 转成工具输入对象 |
| `type=function_call_output` | 客户端返回的工具执行结果 | 转成 Anthropic `tool_result` |
| `output` | 工具执行结果 | 作为 `tool_result.content` |
| `type=reasoning` | 历史推理项目 | 仅当没有需要保真的 summary/加密内容时可安全忽略；否则拒绝桥接 |
| `summary` | 推理摘要 | 非空时不能无损桥接到 Anthropic |
| `encrypted_content` | 加密推理内容 | 存在时不能无损桥接 |

### 3.2 工具声明常见字段

| 字段 | 中文解释 |
|---|---|
| `type` | 工具类型，例如 `function`、`web_search` |
| `name` | 工具名称 |
| `description` | 工具用途描述 |
| `parameters` | 工具输入 JSON Schema |
| `strict` | 是否严格按 Schema 生成参数 |

原生协议会尽量保留未知字段。只有发生协议转换时，代理才执行能力检查；无法安全表达的工具、引用、reasoning 或内容块会拒绝转换，不能静默丢失。

---

## 4. OpenAI Chat Completions 对应字段

Codex 或其它 OpenAI 兼容客户端也可能通过 Chat Completions 路径进入。

| Chat 字段 | Responses/内部对应 | 中文解释 |
|---|---|---|
| `model` | `model` | 客户端模型 |
| `messages` | `input` | 对话消息列表 |
| `messages[].role` | `input[].role` | `system/developer/user/assistant/tool` |
| `messages[].content` | `input[].content` | 消息正文或内容块 |
| `messages[].tool_calls` | `function_call` | 助手发出的工具调用 |
| `tool_call_id` | `call_id` | 工具调用关联 ID |
| `tools` | `tools` | 工具声明 |
| `tool_choice` | `tool_choice` | 工具选择策略 |
| `reasoning_effort` | `reasoning.effort` | 推理强度 |
| `max_completion_tokens` / `max_tokens` | `max_output_tokens` | 输出 Token 上限 |
| `stream` | `stream` | 流式输出开关 |
| `temperature` | `temperature` | 随机性 |
| `top_p` | `top_p` | 核采样 |
| `stop` | 无完全等价 Responses 字段 | 停止序列；桥接时转为 Anthropic `stop_sequences` |

---

## 5. Codex Header 与输出字段对应

| 入站 Header | 输出到 `codexMetadata` | 中文解释 |
|---|---|---|
| `x-codex-turn-metadata` | 多个 canonical 字段 | JSON 格式的 Codex 回合元数据，优先级较高 |
| `x-codex-installation-id` | `installationID` | 安装实例标识；事件里只保存短 SHA-256 指纹 |
| `x-codex-window-id` | `windowID` | Codex 窗口标识 |
| `x-codex-parent-thread-id` | `parentThreadID` | 父线程 ID |
| `x-openai-subagent` | `subagentHeader` | 子代理兼容 Header 证据，例如 `collab_spawn` |
| `originator` | `originator` | 请求来源组件 |
| `x-codex-beta-features` | `betaFeatures` | Codex Beta 功能标识 |
| `x-openai-memgen-request` | `memgenRequest` | Memory Generation 请求标识 |
| `x-openai-internal-codex-responses-lite` | `responsesLite` | Responses Lite 内部标识 |
| `x-codex-ws-stream-request-start-ms` | `wsStreamRequestStartMS` | WebSocket 请求开始时间，Unix 毫秒 |
| `session-id` / `session_id` | `sessionID` | 会话身份兜底字段 |
| `thread-id` / `thread_id` | `threadID` | 线程身份兜底字段 |
| `x-client-request-id` | `threadID` 低优先级兜底 | 缺少 thread ID 时的兼容身份 |
| `turn-id` / `turn_id` | `turnID` | 回合身份兜底字段 |
| `User-Agent` | 外层 `clientKind` | 用于识别客户端类型，不进入 `codexMetadata` |

### 5.1 客户端识别 `clientKind`

| 判定条件 | `clientKind` | 中文显示 |
|---|---|---|
| UA 以 `claude-cli/` 开头 | `claude_code` | Claude Code |
| UA 大小写不敏感包含 `codex` | `codex` | Codex |
| UA 包含 `grok-shell` | `grok_build` | Grok Build |
| UA 未识别，但走 OpenAI 兼容路径 | `openai_compat` | OpenAI 兼容客户端 |
| UA 未识别，且走 Anthropic 路径 | `unknown` | 未知客户端 |

当前没有独立的 `chatgpt` 枚举。因此 ChatGPT 发来的请求如果没有明确 Codex UA，只能可靠标记为 `openai_compat`，不能凭路径或模型名猜成 ChatGPT。

---

## 6. `client_metadata` 与 `codexMetadata` 来源优先级

解析优先级从高到低为：

```text
bodyCanonical
  > bodyFlat
  > headerCanonical
  > headers
  > identityFallback
```

| 来源名 | 实际位置 | 说明 |
|---|---|---|
| `bodyCanonical` | `body.client_metadata["x-codex-turn-metadata"]` | canonical JSON 字符串，最高优先级 |
| `bodyFlat` | `body.client_metadata` 的扁平兼容字段 | 兼容中间件或旧客户端投影 |
| `headerCanonical` | Header `x-codex-turn-metadata` | canonical Header 版本 |
| `headers` | `x-codex-*`、`x-openai-*` 等直接 Header | 直接兼容字段 |
| `identityFallback` | `session-id`、`thread-id`、`turn-id` 等 | 最低优先级身份兜底 |

高优先级字段已经有值时，低优先级不同值不会覆盖它，只会记录：

- `hasConflicts=true`
- `conflicts=["字段名:被忽略来源"]`

冲突记录不保存被忽略的原始值。

---

## 7. `codexMetadata` 完整字段字典

### 7.1 身份、线程与回合

| 输出字段 | canonical 输入字段 | 中文解释 | 能否单独判断子代理 |
|---|---|---|---|
| `installationID` | `installation_id` | 安装实例短 SHA-256 指纹，不是原始 ID | 否 |
| `sessionID` | `session_id` | Codex 会话 ID | 否 |
| `threadID` | `thread_id` | 当前线程 ID | 否 |
| `agentName` | `agent_name` | 完整 AgentPath，例如 `/root/worker`；不是文件系统路径 | 否 |
| `turnID` | `turn_id` | 当前回合 ID | 否 |
| `windowID` | `window_id` | Codex 窗口标识 | 否 |
| `windowNumber` | `window_number` | 上下文窗口序号，非负整数，0 是有效值 | 否 |
| `contextWindowID` | `context_window_id` | 上下文窗口 ID，不作为会话或项目 ID | 否 |
| `requestKind` | `request_kind` | Codex 请求类型，例如 `turn`、`memory` | 否 |
| `forkedFromThreadID` | `forked_from_thread_id` | 当前线程从哪个线程 fork | 否 |
| `forkedFromOrdinalExclusive` | `forked_from_ordinal_exclusive` | fork 历史排他边界序号，非负整数 | 否 |
| `parentThreadID` | `parent_thread_id` | 父线程 ID | 否 |
| `parentTurnID` | `parent_turn_id` | 父回合 ID | 否 |
| `rootTurnID` | `root_turn_id` | 根回合 ID | 否 |
| `turnTrigger` | `turn_trigger` | 回合触发来源，只作有界标签记录 | 否 |

### 7.2 子代理与线程来源

| 输出字段 | 输入来源 | 中文解释 | 子代理证据 |
|---|---|---|---|
| `subagentHeader` | `x-openai-subagent` | 兼容 Header 值，例如 `collab_spawn` | 是，只要非空 |
| `subagentKind` | canonical `subagent_kind` | canonical 子代理类型，例如 `thread_spawn` | 是，只要非空 |
| `threadSource` | canonical `thread_source` | 线程来源 | 仅 `subagent` 或 `memory_consolidation` 是明确证据 |
| `isSubagent` | 代理计算字段 | 当前是否检测到明确子代理证据 | 是，最终汇总结果 |
| `parentThreadIDInferred` | 代理计算字段 | 父线程是否由 fork 关系推断，而非 authoritative 字段 | 否，只说明父关系来源 |

当前计算规则：

```text
isSubagent =
  subagentHeader 非空
  或 subagentKind 非空
  或 threadSource == "subagent"
  或 threadSource == "memory_consolidation"
```

只有已有子代理证据、缺少 authoritative `parentThreadID`，并且存在 `forkedFromThreadID` 时，代理才会用 fork 来源补出父线程，同时设置 `parentThreadIDInferred=true`。

### 7.3 沙箱、审查与时间

| 输出字段 | canonical 输入字段 | 中文解释 |
|---|---|---|
| `sandbox` | `sandbox` | 沙箱策略或沙箱类型 |
| `sandboxMode` | `sandbox_mode` | 沙箱运行模式 |
| `autoReviewEnabled` | `auto_review_enabled` | 是否启用自动审查 |
| `nodeReplAutoReviewRequired` | `node_repl_auto_review_required` | Node REPL 是否必须经过自动审查 |
| `nodeReplDisabled` | `node_repl_disabled` | 是否禁用 Node REPL |
| `historyIngestRequested` | `history_ingest_requested` | 是否请求历史导入；false 与未记录分开保存 |
| `turnStartedAtUnixMS` | `turn_started_at_unix_ms` | 回合开始时间，Unix 毫秒 |

### 7.4 工作区 `workspaces`

`workspaces` 是以脱敏后的工作区路径为 key 的 Map。完整用户目录不会原样保存。

| 嵌套字段 | 输入字段 | 中文解释 |
|---|---|---|
| `associatedRemoteURLs` | `associated_remote_urls` | 远程仓库地址 Map；去除凭据、query 和 fragment |
| `latestGitCommitHash` | `latest_git_commit_hash` | 最近 Git 提交哈希摘要 |
| `hasChanges` | `has_changes` | 工作区是否存在未提交改动 |

### 7.5 工具环境 `toolNamespacesInfo`

`toolNamespacesInfo` 表示客户端注册的工具能力，不等于本次响应实际调用的工具。

| 层级 | 字段 | 中文解释 |
|---|---|---|
| namespace | `name` | 命名空间原始名称 |
| namespace | `functions` | 该命名空间下的函数 Map |
| function | `name` | 函数原始名称 |
| function | `direct` | 是否直接调用 |
| function | `codeModeName` | Code Mode 中使用的名称；输入为 `code_mode_name` |
| function | `deferred` | 是否为延迟加载/延迟解析函数 |
| function | `source` | 工具来源摘要 |
| source | `kind` | 工具来源类型 |
| source | `serverName` | MCP/工具服务器名称；输入为 `server_name` |

### 7.6 上下文压缩 `compaction`

| 字段 | 中文解释 |
|---|---|
| `trigger` | 触发压缩的条件或来源 |
| `reason` | 执行压缩的原因 |
| `implementation` | 压缩实现方式 |
| `phase` | 压缩发生阶段 |
| `strategy` | 压缩策略 |

### 7.7 传输附加信息与解析状态

| 输出字段 | 中文解释 |
|---|---|
| `extras` | 不属于保留字段、且通过 key/value 约束的字符串扩展字段 |
| `originator` | 请求来源组件 |
| `betaFeatures` | Codex Beta 功能标识 |
| `memgenRequest` | Memory Generation 请求标识 |
| `responsesLite` | Responses Lite 标识 |
| `wsStreamRequestStartMS` | WebSocket 流请求开始时间 |
| `sources` | 实际观察到的解析来源列表 |
| `redactedFields` | 被主动脱敏或忽略的字段名，不含原值 |
| `malformed` | JSON、字段类型或结构异常 |
| `truncated` | 数据超过长度/数量限制而被截断 |
| `hasConflicts` | 多个来源存在不一致字段 |
| `conflicts` | `字段名:被忽略来源` 列表，不保存冲突原值 |

---

## 8. Codex 元数据安全限制

### 8.1 大小和数量限制

| 项目 | 当前限制 |
|---|---:|
| canonical Codex metadata JSON | 64 KiB |
| 普通 ID/标签 | 通常 128 字节 |
| `agentName` | 256 字节 |
| 工作区路径、远程 URL | 256 字节 |
| 工作区数量 | 32 |
| 每工作区远程 URL 数量 | 32 |
| 工具命名空间数量 | 64 |
| 每命名空间函数数量 | 128 |
| 函数总数 | 512 |
| `extras` 数量 | 16 |

这处 64 KiB 限制只针对 `x-codex-turn-metadata` 的安全解析，不是诊断抓包容量限制，也不会阻断原请求转发。超限只会设置 `truncated=true`。

### 8.2 不进入普通事件的敏感字段

以下原值不得进入 `RuntimeEvent` 或 `codexMetadata`：

- `Authorization`
- `x-api-key` / `api_key`
- Cookie / Set-Cookie
- `x-oai-attestation`
- `x-codex-routing-hint`
- `x-codex-turn-state`
- `traceparent` / `tracestate`
- prompt、instructions、input、output、body 正文

普通事件最多在 `redactedFields` 中记录“存在并被脱敏的字段名”。

---

## 9. `RuntimeEvent` 完整字段字典

### 9.1 事件身份与生命周期

| 字段 | 中文解释 | 缺失/特殊值含义 |
|---|---|---|
| `id` | 单条事件唯一 UUID，也是事件原地更新键 | 必填 |
| `timestamp` | 事件时间，使用 Apple reference date 秒数 | 不是 Unix 秒 |
| `kind` | `client`、`upstream` 或 `notify` | `client` 是请求整体；`upstream` 是单次尝试 |
| `requestID` | 同一次客户端请求与全部上游尝试的关联 ID | 缺失通常是旧事件 |
| `phase` | `inFlight` 或 `completed` | 缺失通常是旧事件 |
| `outcome` | `succeeded`、`failed`、`cancelled` | 进行中、通知或旧事件可能缺失 |
| `statusCode` | 客户端实际可见 HTTP 状态 | `0`=未收到响应头；`499`=客户端断开/取消 |
| `durationMS` | 请求或尝试总耗时 | 进行中时通常持续更新 |
| `ttfbMS` | 首字节/响应头延迟 | 未收到响应头或旧事件时为空 |

`client` 事件的 `ttfbMS` 包含 failover 和跨轮重试的总等待；`upstream` 事件只表示该次入口尝试的响应头延迟。

### 9.2 客户端、用途和模型链

| 字段 | 中文解释 | 证据边界 |
|---|---|---|
| `clientKind` | 入站客户端类型 | 只按 UA 和入站方言识别，不猜请求体 |
| `requestPurpose` | 代理识别的请求用途 | 不等于 `requestKind`，不判断子代理 |
| `clientModel` | 客户端请求的模型 | 请求体 `model` 清理后的值 |
| `effectiveModel` | 路由定型后的逻辑模型 | 可能由特征规则 `target.model` 改写 |
| `upstreamModel` | 实际发送给上游的模型名 | 可能是入口私有别名 |

模型链应按以下顺序理解：

```text
clientModel → effectiveModel → upstreamModel
客户端要求    路由逻辑模型      真正出站模型
```

### 9.3 路由和入口

| 字段 | 中文解释 |
|---|---|
| `poolID` | 路由池 ID，当前统一 Provider 通常为 `primary` |
| `featureRuleID` | 命中的特征分流规则 ID |
| `endpointID` | 入口稳定 ID |
| `endpointName` | 入口显示名称 |
| `upstreamHost` | 实际上游 Host |
| `sourceFormat` | 入站路径确定的真实协议 |
| `targetFormat` | 本次实际选择的出站协议 |
| `routeMode` | `native` 原生适配或 `translated` 协议转换 |
| `failover` | 是否从一个入口切换到另一个入口 |

协议常见值：

- `anthropic`：Anthropic Messages
- `openai`：OpenAI Chat Completions
- `openai-responses`：OpenAI Responses

### 9.4 HTTP、失败和追踪

| 字段 | 中文解释 | 重要说明 |
|---|---|---|
| `upstreamStatusCode` | 实际收到的上游 HTTP 状态 | 连接失败/响应头前超时时为空 |
| `upstreamRequestID` | 上游返回的安全白名单追踪 ID | 用于关联上游日志 |
| `failureKind` | 结构化失败分类 | 比自由文本和 HTTP 状态更权威 |
| `failurePhase` | 失败发生阶段 | 用于区分响应头前失败和 200 后断流 |
| `failureDetail` | 有界、去 URL/凭据后的技术详情 | 供详情页排障，不作为分类依据 |
| `message` | 引擎机器可读 token 或通知文本 | 请求事件由 UI 翻译；通知事件可直接是人类文本 |
| `timeoutMS` | 实际生效的超时阈值 | 可能是首响应或流空闲阈值 |

### 9.5 `failureKind` 值

| 值 | 中文解释 |
|---|---|
| `response_timeout` | 首响应超时，尚未及时收到上游响应头 |
| `connection_failed` | DNS/TCP/TLS/连接建立或发送失败 |
| `invalid_response` | 上游响应无法解析 |
| `upstream_http_status` | 上游返回错误 HTTP 状态 |
| `stream_idle_timeout` | 已收到响应头，但响应流长时间没有新数据 |
| `stream_interrupted` | 已收到响应头，但流传输异常中断 |
| `upstream_response_incomplete` | 上游以明确 incomplete 协议事件结束 |
| `upstream_response_failed` | 上游以明确 failed 协议事件结束 |
| `endpoints_exhausted` | 所有可用入口均已尝试失败或被跳过 |
| `client_cancelled` | 下游连接断开或请求被取消 |
| `client_request_rejected` | 代理在真实上游尝试前拒绝客户端请求 |

### 9.6 `failurePhase` 值

| 值 | 中文解释 |
|---|---|
| `before_response` | 收到上游响应头之前 |
| `response_headers` | 接收或处理响应头阶段 |
| `response_stream` | 已进入响应流传输阶段 |

HTTP `200` 不等于最终成功。若响应头已经返回 200，随后发生断流或收到 `response.failed`，事件会记录：

```text
statusCode = 200
outcome = failed
failurePhase = response_stream
failureKind = stream_interrupted / upstream_response_failed / ...
```

这是正确状态，不应仅根据 HTTP 200 改成成功。

### 9.7 工具与流诊断

| 字段 | 中文解释 |
|---|---|
| `toolCalls` | 响应流中实际观察到的工具调用名称；不是请求声明的 `tools` |
| `streamTrace` | 脱敏的流计时和字节摘要，不含正文/Header |
| `streamTrace.chunkCount` | 收到的 Chunk 数量 |
| `streamTrace.bytesReceived` | 累计收到字节数 |
| `streamTrace.maxChunkGapMS` | 相邻 Chunk 最大间隔 |
| `streamTrace.lastChunkAtMS` | 最后一个 Chunk 相对请求开始的时间 |
| `streamTrace.terminalEvent` | 最后观察到的协议终止事件 |

缺失含义必须区分：

- 进行中且无 `toolCalls`：尚未观察到工具调用。
- 已完成且 `toolCalls=[]`：未观察到工具调用。
- 旧事件没有 `toolCalls` 键：旧事件未记录，不能断言没有工具调用。
- 通知事件：工具和流诊断不适用。

---

## 10. `message` 机器 token 对应中文

| token/前缀 | 中文解释 |
|---|---|
| `pinned <ip>` | 本次尝试使用 IP 直连 |
| `passthrough <协议>` | 使用对应协议原样透传 |
| `bridge <协议>` | 使用对应协议桥接转换 |
| `deferred_rounds <n>` | 可重试故障共执行了 n 轮上游尝试 |
| `unmatched_no_tools` | 无工具、单轮请求，但用途指纹未识别 |
| `upstream_retryable_status` | 上游持续返回可重试错误，最终耗尽 |
| `all endpoints failed` | 所有入口不可用、被停用或被守卫跳过 |
| `inbound_auth_required` | 入站认证失败 |
| `openai_tools_unsupported` | 当前 OpenAI 协议入口不支持该工具请求 |
| `body is not JSON` | 请求体不是合法 JSON |
| `body is not an object` | 请求体不是 JSON object |
| `anthropic request shape invalid` | Anthropic 请求结构不满足最低要求 |
| `inbound_convert_failed: <原因>` | OpenAI/Responses 入站请求无法安全转换 |
| `stream interrupted: <原因>` | 响应头已经发出，随后流中断 |
| `client_disconnected...` | 客户端连接断开或取消 |

`message` 只作为补充信息。新事件的最终成败优先看 `outcome`、`failureKind` 和 `failurePhase`。

---

## 11. Diagnostic Capture 抓包字段

诊断捕获不是普通 RuntimeEvent。它可能保存原始 Header、Body 和响应 Chunk，必须短时间、非敏感流量使用，导出前必须人工检查。

### 11.1 捕获快照 `DiagnosticCaptureSnapshot`

| 字段 | 中文解释 |
|---|---|
| `enabled` | 是否正在捕获 |
| `startedAt` | 捕获开始时间 |
| `maxBytes` | 捕获容量上限 |
| `capturedBytes` | 已捕获字节数 |
| `limitReached` | 是否达到容量上限并自动停止 |
| `stopReason` | 停止原因，例如手动停止 |
| `records` | 捕获的请求记录列表 |

索引接口为避免大快照刷新卡顿最多返回最近 200 条记录；`recordCount` 表示保留记录总数，
`indexTruncated=true` 表示 `records` 已截断。按 `requestID` 的详情接口仍可读取任意保留记录。

当前核心默认值为 512 MB；UI 可在开始捕获前传入 `maxBytes`。这与 Codex metadata 的 64 KiB 解析上限完全不同。

### 11.2 单次请求 `DiagnosticRequestCapture`

| 字段 | 中文解释 |
|---|---|
| `requestID` | 请求链关联 ID |
| `timestamp` | 请求开始时间 |
| `method` | HTTP 方法 |
| `path` | 入站路径 |
| `inboundHeaders` | 原始入站 Header 列表 |
| `inboundBody` | 原始入站请求体 |
| `inboundBodyBytes` | 原始入站 Body 字节数 |
| `inboundBodyTruncated` | 入站 Body 是否在捕获中被截断 |
| `clientKind` | 入站客户端类型 |
| `requestPurpose` | 请求用途 |
| `clientModel` | 客户端模型 |
| `effectiveModel` | 路由逻辑模型 |
| `poolID` | 路由池 |
| `featureRuleID` | 命中规则 |
| `sourceFormat` | 入站协议 |
| `targetFormat` | 出站协议 |
| `routeMode` | 原生或协议转换 |
| `attempts` | 全部上游尝试 |
| `clientChunks` | 实际写给客户端的 Chunk |
| `completedAtMS` | 完成时间/相对时间 |
| `statusCode` | 最终客户端状态 |
| `outcome` | 最终结果 |
| `failureKind` | 失败分类 |
| `failureDetail` | 技术错误详情 |
| `truncated` | 整条捕获是否有内容被截断 |

### 11.3 单次上游尝试 `DiagnosticAttemptCapture`

| 字段 | 中文解释 |
|---|---|
| `id` | 尝试记录 ID |
| `endpointID` | 入口 ID |
| `endpointName` | 入口名称 |
| `protocol` | 尝试使用的协议字符串 |
| `sourceFormat` | 该请求的入站协议 |
| `targetFormat` | 该次尝试的实际出站协议 |
| `routeMode` | 原生或协议转换 |
| `pinnedIP` | 该次尝试使用的固定 IP |
| `startedAtMS` | 尝试开始时间 |
| `outboundMethod` | 出站 HTTP 方法 |
| `outboundURL` | 完整出站 URL |
| `outboundHeaders` | 出站 Header |
| `outboundBody` | 出站请求体 |
| `outboundBodyBytes` | 出站 Body 字节数 |
| `outboundBodyTruncated` | 出站 Body 是否被截断 |
| `responseStatus` | 上游响应状态 |
| `responseHeaders` | 上游响应 Header |
| `upstreamChunks` | 上游响应 Chunk |
| `error` | 传输或捕获错误 |
| `completedAtMS` | 尝试完成时间 |

Chunk 结构：

| 字段 | 中文解释 |
|---|---|
| `atMS` | Chunk 到达的相对时间 |
| `bytes` | Chunk 字节数 |
| `data` | 捕获的 Chunk 内容 |
| `truncated` | 该 Chunk 内容是否截断 |

Header 结构仅包含：

- `name`：Header 名称
- `value`：Header 值

---

## 12. 子代理判断边界

### 12.0 Guardian、WebSocket 与项目归因

- `x-openai-subagent=guardian` 是 Codex 内部 Guardian 安全审查身份。界面显示「Guardian 安全审查」，保留原始 Header 与 `isSubagent` 兼容字段。
- `codexThreadClass` 优先使用 canonical `thread_source`；没有它时，仅用已知的 `subagent_kind` / `x-openai-subagent` 值回退。例如 `guardian` → `guardian_review`，`memory_consolidation` → `memory_consolidation`。不伪造 `threadSource`。
- 有结构化工作区或明确项目声明时仍归属该项目；只有内部功能身份、没有项目证据时归为 `internal_feature`。不能从 session/thread ID 或模型名称猜项目。
- WebSocket 首帧的顶层与 `response` 嵌套元数据都会合并。Body 投影优先于握手兼容 Header，保留工作区、父子关系、工具信息及新增字段；冲突只记字段名与来源。这里只观察首帧，不宣称完成了长连接内逐回合统计。
- SQLite 投影版本 8 会重算仍保有 payload 的旧事件分类；此前未保存的 Body 字段无法凭空补回。
- `body is not JSON` 表示代理入口解析失败，并不表示 Guardian 审查否决。此时仅能保留 Header 身份。JSON 推理入口现支持 zstd/gzip 请求解压：先认证，限制解压后大小，移除失效的 Content-Encoding/Content-Length，再解析归因字段。资源与 multipart 请求不进入此解压分支。未知编码、损坏压缩流和超限分别返回明确错误。该兼容处理不能证明截图实例的根因；用户已选择跳过远端核验，仍缺该实例的 Content-Type、Content-Encoding、请求体字节数等证据。

新增的五个 canonical 字段支持详情、JSON 导出和持久化；不将上下文窗口或 fork 序号新增为项目统计分组，也不改写原始请求。`turnTrigger`、`contextWindowID` 曾可能进入字符串 extras，新版本将其提升为有类型的字段；三个整数/布尔字段此前会被忽略。

### 12.1 可以确认子代理的证据

- `subagentHeader` 非空，即观察到 `x-openai-subagent`。
- canonical `subagentKind` 非空。
- `threadSource=subagent`。
- `threadSource=memory_consolidation`。
- 最终汇总字段 `isSubagent=true`。

### 12.2 不能单独判断子代理的字段

- `agentName=/root`、`/root/worker` 或其它 AgentPath。
- `requestKind=memory`、`turn` 或其它请求类型。
- `threadID`、`parentThreadID`、`forkedFromThreadID`、`rootTurnID`。
- `clientKind=codex`。
- Responses/Chat 请求路径。
- 模型名、reasoning effort。
- Prompt、instructions、input 内容。
- 请求声明的工具或实际工具调用名称。
- 耗时、TTFB、Chunk 数和终止事件。
- Failover、入口名称和协议转换。

所以界面应区分：

| 维度 | 示例 | 含义 |
|---|---|---|
| 客户端 | `Codex` | 谁发送了请求 |
| 请求用途 | `standard`、`compact`、`memory` | 请求用来做什么；其中 `memory` 属于 `requestKind` |
| 代理身份 | 子代理/未发现子代理证据 | 是否有明确子代理元数据 |

当元数据存在但没有子代理证据时，推荐显示：

```text
Codex · 未发现子代理证据
```

不建议直接显示“已确认主代理”。“没有发现证据”和“已经确认不是子代理”不是同一件事。

---

## 13. 缺失字段、未知值和旧事件的统一解释

| 展示状态 | 正确含义 |
|---|---|
| `unknown` / `clientKind=unknown` | 当前解析器明确运行过，但没有识别出 Anthropic 客户端 |
| `openai_compat` | 明确走 OpenAI 兼容层，但 UA 不是已知产品 |
| 字段不存在 | 旧事件、路由前拒绝、尚未进入对应阶段或该事件不适用 |
| `notify` 事件缺少请求字段 | 不适用，不是未知，也不是旧事件推断 |
| `statusCode=0` | 尚未收到响应头 |
| `HTTP 200 + phase=inFlight` | 已收到成功响应头，但最终协议结果尚未确定 |
| `HTTP 200 + outcome=failed` | 响应头成功，但流或协议最终失败 |
| 缺少 `toolCalls` 且无新生命周期字段 | 旧事件未记录，不能断言没有工具调用 |
| 缺少 `codexMetadata` | 旧事件、非 Codex 请求或客户端没有发送可解析元数据 |

---

## 14. UI 推荐主次层级

### 主信息区

- 最终结果 `outcome`
- HTTP 状态 `statusCode`
- 生命周期 `phase`
- 入站客户端 `clientKind`
- 请求用途 `requestPurpose`
- 模型链 `clientModel → effectiveModel → upstreamModel`
- 入口名称 `endpointName`
- Failover
- TTFB 和总耗时

### 二级路由区

- `poolID`
- `featureRuleID`
- `endpointID`
- `upstreamHost`
- `sourceFormat → targetFormat · routeMode`
- `upstreamStatusCode`

### 失败、工具与流诊断区

- `failureKind / failurePhase`
- `failureDetail`
- `toolCalls`
- `streamTrace`
- `timeoutMS`
- `message`
- `requestID / upstreamRequestID / event id`

### Codex 上下文区

- “子代理”或“未发现子代理证据”
- `requestKind`
- `threadID / parentThreadID / turnID / rootTurnID`
- `agentName`
- `workspaces`
- `toolNamespacesInfo`
- `compaction`
- `sources / redactedFields / malformed / truncated / conflicts`

---

## 15. 当前实现已知注意点

1. `requestPurpose=standard` 当前部分界面会翻译成“主请求”或“主对话”，它只表示普通用途，不是主代理证据。
2. 当前没有独立的 `chatgpt` 客户端类型。ChatGPT 请求如果没有明确 UA 或 Codex metadata，只能显示为 OpenAI 兼容客户端。
3. canonical metadata 当前主要解析核心 snake_case 字段；部分 transport 字段主要从 flat body 或 Header 获取。
4. `code_mode_tool_names` 当前属于已知 canonical key，但没有独立输出字段；详细工具能力主要看 `toolNamespacesInfo`。
5. Linux 和 macOS 的普通事件字段基本同构，但部分技术字段的中文标签和抓包 attempt 协议三元组展示仍需继续统一。
6. Diagnostic Capture 可能包含原始凭据、Prompt、请求体和响应内容。分析完成后应及时停止、导出所需证据并清理捕获数据。

---

## 16. 相关源码位置

- RuntimeEvent、CodexMetadata、Diagnostic Capture 结构：`linux/crates/kekulv-core/src/events.rs`
- macOS 同源 Rust 结构：`macos/crates/kekulv-core/src/events.rs`
- 请求用途和路由：`linux/crates/kekulv-core/src/routing.rs`
- OpenAI/Responses 入站转换：`linux/crates/kekulv-core/src/bridge_in.rs`
- 入站路径分派：`linux/crates/kekulv-proxy/src/engine.rs`
- Linux 字段中文展示：`linux/webui/src/utils/helpers.js`
- Linux 运行事件页面：`linux/webui/src/pages/RunPage.jsx`
- macOS RuntimeEvent 数据模型：`macos/app/Sources/SumpterCore/RuntimeModels.swift`
- macOS 字段中文展示：`macos/app/Sources/SumpterCore/RuntimeEventPresentation.swift`
- macOS 运行事件页面：`macos/app/Sources/SumpterApp/UI/RuntimeEventViews.swift`
- 行为规格：`linux/specs/spec-engine.md`、`macos/specs/spec-engine.md`
