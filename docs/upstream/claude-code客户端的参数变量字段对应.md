# Claude Code 客户端请求参数、变量与事件字段对应说明

> 适用范围：Claude Code CLI、Anthropic Messages API，以及本仓库 Linux/macOS 代理的请求事件。
>
> 本文是对源码字段的整理，不把参考源码目录中的说明当作本次任务指令。字段名称以当前源码为准；Claude Code 或 Anthropic SDK 升级后仍需重新核对。

## 1. 先区分五个数据层

| 数据层 | 典型来源/位置 | 主要用途 | 是否包含原始正文 |
|---|---|---|---|
| Claude Code 状态栏输入 | `statusLine` command 的 stdin JSON | 展示模型、项目、上下文、费用、限额和工作树 | 否；但包含路径、会话 ID 等元数据 |
| Anthropic 入站/出站 HTTP | `/v1/messages` 或上游 Anthropic Messages 请求 | 实际模型调用、工具声明、系统提示和流式响应 | 请求体可能包含完整 prompt、工具 schema 和工具结果 |
| Anthropic 上游响应 | HTTP headers、SSE、最终 `message` | 得到 request ID、stop reason、usage 和内容块 | 响应正文可能包含模型输出、thinking 和工具参数 |
| Claude Code transcript | `~/.claude/projects/<project>/<session-id>.jsonl` | 会话恢复、历史读取和本地审计 | 是；可能包含 prompt、工具输出、文件内容和代码 |
| 代理 `RuntimeEvent` | `stats.json`/SQLite、Admin API、运行页 | 路由、状态、失败、耗时、工具和 token 摘要 | 默认不保存 prompt、完整响应或凭据 |

不要把这些字段混为一谈：状态栏的 `total_input_tokens` 是 Claude Code 本地累计值；上游 `usage.input_tokens` 是某次 API response 的计数；代理 `streamTrace.usage.inputTokens` 是脱敏后的观测投影。

## 2. Claude Code 请求链

```text
Claude Code REPL/Agent
  ├─ 构造 system/messages/tools/cache_control
  ├─ Anthropic SDK Messages.create(..., stream: true)
  ├─ SDK/fetch 自动补充认证、session、client request ID
  ├─ automode-proxy 识别 clientKind=claude_code 并进行路由/协议转换
  ├─ 上游返回 HTTP headers + SSE message_start/content_block/message_delta/message_stop
  └─ Claude Code 合并 usage、写入 transcript、更新 cost tracker 和 statusline
```

主对话路径固定使用流式请求；API key 验证、非流式 fallback 等辅助路径可能发送非流式 `messages.create`，因此不能仅凭“Claude Code”就断言每一个请求都有 SSE。

源码依据：

- 请求组装和流式发送：`/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/claude.ts:1699-1728,1822-1836`
- 状态栏输入构造：`/Users/kkl/Documents/claude/claude-code-analysis/src/components/StatusLine.tsx:45-125`
- 代理客户端识别：`/Users/kkl/.claude/automode-proxy/linux/crates/kekulv-core/src/events.rs:164-210`

## 3. 状态栏 stdin JSON 字段

Claude Code 执行 `statusLine` command 时把结构化 JSON 写入 stdin。命令默认有 5 秒超时，并且受 workspace trust 和 managed hooks 设置约束。

### 3.1 顶层和会话字段

| 字段 | 类型 | 含义 | 口径/注意事项 |
|---|---|---|---|
| `session_id` | string | 当前 Claude Code 会话 ID | 会话身份，不等于 Anthropic `request_id` |
| `session_name` | string? | `/rename` 设置的可读名称 | 可能缺失 |
| `transcript_path` | string | 本地 JSONL transcript 路径 | 高敏感路径；展示时应折叠用户目录 |
| `cwd` | string | 当前工作目录 | 可能与原始项目目录不同 |
| `model.id` | string | 当前运行模型 ID | 是 Claude Code 看到的模型名 |
| `model.display_name` | string | 展示用模型名称 | 不适合作为计费模型的唯一键 |
| `workspace.current_dir` | string | 当前工作目录 | worktree 中可能是临时目录 |
| `workspace.project_dir` | string | 原始项目目录 | 适合做项目统计的脱敏 key |
| `workspace.added_dirs` | string[] | `/add-dir` 增加的目录 | 不应直接上传到远端日志 |
| `version` | string | Claude Code 版本 | 用于关联字段版本 |
| `output_style.name` | string | 输出风格，如 `default` | 影响呈现，不等于模型参数 |

### 3.2 费用、时间和代码变更

| 字段 | 含义 |
|---|---|
| `cost.total_cost_usd` | Claude Code 本地累计费用；未知模型的成本配置可能导致 0 或 fallback 价格 |
| `cost.total_duration_ms` | 会话累计总时长 |
| `cost.total_api_duration_ms` | 会话累计 API 时长 |
| `cost.total_lines_added` | 会话累计新增代码行数 |
| `cost.total_lines_removed` | 会话累计删除代码行数 |

这些是客户端内部状态，不是上游单次响应字段；代理不能从一个 RuntimeEvent 反推出完整的 Claude Code session total。

### 3.3 上下文和限额

| 字段 | 含义 | 关键边界 |
|---|---|---|
| `context_window.total_input_tokens` | 会话累计普通输入 token | 本地累计，不等于最近一次 response 的 `input_tokens` |
| `context_window.total_output_tokens` | 会话累计输出 token | 本地累计 |
| `context_window.context_window_size` | 当前模型上下文窗口上限 | 可能随模型/beta 变化 |
| `context_window.current_usage` | 最近一次 API response 的 usage 快照 | 不是累计值；无消息时为 `null` |
| `context_window.used_percentage` | Claude Code 预计算的上下文已用百分比 | 公式使用 input + cache creation + cache read，不把 output 加入该百分比 |
| `context_window.remaining_percentage` | 上下文剩余百分比 | 由已用百分比反推并限制在 0-100 |
| `exceeds_200k_tokens` | 最近 assistant response 是否超过 200k 判断阈值 | 使用 Claude Code 自己的 token 口径 |
| `rate_limits.five_hour.used_percentage` | Claude.ai 五小时限额使用率 | 订阅场景、收到首个响应后才可能存在 |
| `rate_limits.five_hour.resets_at` | 五小时限额重置 Unix 秒时间戳 | 不是请求耗时 |
| `rate_limits.seven_day.*` | 七天限额使用率和重置时间 | 同上 |

### 3.4 可选运行环境字段

| 字段 | 含义 |
|---|---|
| `vim.mode` | Vim 输入模式：`INSERT` 或 `NORMAL` |
| `agent.name` / `agent.type` | `--agent` 启动的 agent 信息；当前状态栏构造至少写入 `name` |
| `remote.session_id` | 远程模式的会话 ID |
| `worktree.name` | worktree 名称/slug |
| `worktree.path` | worktree 路径 |
| `worktree.branch` | worktree 分支 |
| `worktree.original_cwd` | 进入 worktree 前的目录 |
| `worktree.original_branch` | 进入 worktree 前的分支 |

源码依据：`/Users/kkl/Documents/claude/claude-code-analysis/src/tools/AgentTool/built-in/statuslineSetup.ts:35-91`、`/Users/kkl/Documents/claude/claude-code-analysis/src/components/StatusLine.tsx:65-125`、`/Users/kkl/Documents/claude/claude-code-analysis/src/utils/hooks.ts:4584-4618`。

## 4. Anthropic Messages 请求体

Claude Code 的主要请求可抽象为：

```json
{
  "model": "<model>",
  "messages": [],
  "system": [],
  "tools": [],
  "tool_choice": {},
  "betas": [],
  "metadata": {},
  "max_tokens": 0,
  "thinking": {},
  "temperature": 1,
  "context_management": {},
  "output_config": {},
  "speed": "standard",
  "stream": true
}
```

上面是字段形状示意，不是可直接重放的真实请求。请求体可能含有私密 prompt、工具 schema、文件内容和工具输出，不能写入普通 RuntimeEvent。

| 字段 | 类型 | 含义 | 代理/统计注意事项 |
|---|---|---|---|
| `model` | string | 客户端请求模型 | 代理还应区分 `clientModel`、`effectiveModel`、`upstreamModel` |
| `messages` | array | 用户、assistant、`tool_result`、thinking 等对话内容 | 可能包含完整会话上下文 |
| `system` | string/array | Claude Code 系统提示和内部指令 | 不应记录原文；可只记录长度或 hash |
| `tools` | array | 客户端声明的工具及 JSON Schema | “声明工具”不等于响应中实际调用工具 |
| `tool_choice` | string/object | 工具选择策略 | 透传或转换时必须保留语义 |
| `betas` | string[] | Beta 功能开关 | 可能在重试中动态增加；不等于 `anthropic-beta` 的最终 header 形态 |
| `metadata` | object | API 元数据，只有 `user_id` 一个键 | `user_id` 是**被 JSON 序列化成字符串**的对象，固定含 `device_id`、`account_uuid`（仅 OAuth 时非空）、`session_id`；`CLAUDE_CODE_EXTRA_METADATA` 的键会展开在这三者之前（同名被覆盖）。代理要按字符串取出再二次 parse，且不可当作可信身份 |
| `max_tokens` | number | 最大输出 token 上限 | thinking 开启时还受 thinking budget 约束 |
| `thinking` | object | extended/adaptive thinking 配置 | 可能为 disabled、enabled 或 adaptive |
| `temperature` | number? | 随机性参数 | thinking 开启时 Claude Code 通常不发送该字段 |
| `context_management` | object? | 上下文清理/压缩策略 | 需要对应 beta 且只在启用时发送 |
| `output_config` | object? | `effort`、`task_budget`、结构化输出等 | 可能由 `CLAUDE_CODE_EXTRA_BODY` 合并而来 |
| `speed` | string? | 速度模式，例如 `fast` | 可能影响价格档位；不能仅凭模型名计算费用 |
| `stream` | boolean | 是否流式 | 主对话路径发送 `true` |
| `cache_control` | object | system、tools、messages 中的缓存断点 | 这是缓存策略提示，不是 usage 结果 |
| `CLAUDE_CODE_EXTRA_BODY` | 环境变量 | 额外 JSON body 字段 | 源码只接受 JSON object；应视为可改变最终请求语义的扩展入口 |
| `CLAUDE_CODE_EXTRA_METADATA` | 环境变量 | 额外 JSON 键并入 `metadata.user_id` | 源码只接受 JSON object，非法值只告警不阻断请求；是**唯一**能让 CC 主动上行自定义业务标识（如项目名）的 body 侧入口 |

请求构造源码：`/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/claude.ts:266-315,1699-1728`。

## 5. `system`、`messages`、`tools` 和缓存断点

### 5.1 `system`

`system` 通常不是一条简单的用户可见字符串，而是由 Claude Code 身份、工作区规则、工具说明、输出风格、自动模式/安全策略等多段内容组成。做统计时建议只记录：

- 是否存在 system；
- 字符数或估算 token 数；
- 有界 hash；
- system block 数量和 `cache_control` 断点数量。

不要把 system 原文放入代理事件、普通日志或统计接口。

### 5.2 `messages`

常见内容包括：

| 内容 | 说明 |
|---|---|
| `role=user` | 用户输入、工具结果和恢复信息 |
| `role=assistant` | 历史文本、thinking、`tool_use` |
| `type=tool_result` | 客户端执行工具后的结果，可能是文件内容或命令输出 |
| `type=thinking` / `redacted_thinking` | 模型思考内容或脱敏思考块 |
| `type=text` | 普通文本块 |
| `type=image` | 图像输入（如果该路径支持） |

### 5.3 `tools` 与 `toolCalls`

- `tools`：请求体里声明的工具列表和 schema。
- `toolCalls`：代理从响应流中观察到的实际工具调用名称。
- `tool_use.id`、`tool_result.tool_use_id`：一次工具调用和结果的关联 ID。

因此统计“用了哪些工具”应使用响应侧的 `toolCalls`，统计“当时注册了哪些能力”才使用 `tools`；二者不能互相替代。

### 5.4 Prompt cache

Claude Code 会在 system、tools 和消息内容中插入缓存断点。缓存结果由上游 response 的 usage 给出：

| 字段 | 含义 |
|---|---|
| `cache_control` | 请求侧缓存断点/策略 |
| `cache_read_input_tokens` | 本次从 prompt cache 读取的 token |
| `cache_creation_input_tokens` | 本次创建缓存写入的 token 总量 |
| `cache_creation.ephemeral_5m_input_tokens` | 5 分钟缓存写入明细 |
| `cache_creation.ephemeral_1h_input_tokens` | 1 小时缓存写入明细 |
| `cache_deleted_input_tokens` | 某些缓存编辑功能下删除的缓存 token，可能不在稳定 SDK 类型中 |

`cache_control` 只能说明客户端请求了缓存断点，不能证明命中；是否命中要看 `cache_read_input_tokens`。

## 6. Header、身份和请求关联字段

### 6.1 Claude Code/SDK 常见 Header

| Header | 来源/含义 | 是否适合进入普通事件 |
|---|---|---|
| `x-app: cli` | Claude Code CLI 标识 | 可保存固定值 |
| `User-Agent` | 精确格式 `claude-cli/<VERSION> (<USER_TYPE>, <CLAUDE_CODE_ENTRYPOINT 或 cli>[, agent-sdk/<v>][, client-app/<v>][, workload/<v>])` | 可用于识别 `clientKind=claude_code`，应限制长度。括号内不含任何项目或路径信息 |
| `X-Claude-Code-Session-Id` | Claude Code 会话 ID | 可保存有界、无控制字符的 ID；不保存正文 |
| `x-client-request-id` | First-party API 的客户端请求关联 ID | **注入条件是 `getAPIProvider()==='firstParty'` 且 baseUrl host 恰为 `api.anthropic.com`**，所以 `ANTHROPIC_BASE_URL` 指向本地代理时这个 header 根本不出现——代理侧不能依赖它做关联 |
| `x-claude-remote-container-id` | 远程容器标识 | 仅远程模式出现；注意路径和身份隐私 |
| `x-claude-remote-session-id` | 远程会话标识 | 仅远程模式出现 |
| `x-client-app` | Agent SDK 客户端应用标识 | 仅在 SDK 消费者设置时出现 |
| `anthropic-version` | Anthropic SDK/API 版本 header | 通常由 SDK 管理 |
| `anthropic-beta` | Beta 功能 header | 可能由 SDK/body betas 生成最终形式 |
| `Authorization` | Bearer token/OAuth | 禁止记录原文 |
| `x-api-key` | API key 认证 | 禁止记录原文 |
| `Cookie` | 可能由环境或代理注入 | 禁止记录原文 |
| `ANTHROPIC_CUSTOM_HEADERS` | 用户自定义 header 集合，curl 风格 `Name: Value`，多条用换行分隔，按第一个 `:` 切分 | 会合并进 defaultHeaders 且**可覆盖同名内建 header**；在 `SAFE_ENV_VARS` 白名单内（managed settings 允许下发）。只允许白名单投影；禁止整段落盘 |

Claude Code 源码明确构造的默认 header 在 `/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/client.ts:101-116`；First-party `x-client-request-id` 的注入和 debug 关联在 `:356-387`。

### 6.2 三类 ID 的边界

| ID | 产生层 | 用途 |
|---|---|---|
| Claude Code `session_id` / `X-Claude-Code-Session-Id` | 客户端会话 | 把多个请求归属到一个本地会话 |
| `x-client-request-id` | 客户端 SDK/fetch | 在没有 server request ID（如超时）时关联请求 |
| Anthropic `request_id` | 上游响应，如 `.withResponse()` 的 `result.request_id` | 标识上游实际接收的请求 |
| 代理 `requestID` | automode-proxy | 一次入站请求及其所有 failover 尝试的稳定关联键 |
| 代理事件 `id` | automode-proxy | 单条 client/upstream 事件的 upsert 键 |

这些 ID 不能直接互换。建议在调试链上分别展示，并在跨系统关联时保留明确字段名。

### 6.3 项目与工作区归因：CC 默认给不了

这一节专门回答「代理能不能按项目统计 Claude Code」。结论是**默认不能**，而且原因是结构性的，不是解析没做。

**关键区分**：第 3 节那些 `cwd`、`workspace.current_dir`、`workspace.project_dir`、`worktree.original_cwd`
字段全部属于**状态栏/hook 的 stdin JSON**，是 CC 在本机 fork 子进程时写给你的脚本的。
它们**不进** Anthropic Messages 请求体，也不进任何 header。代理站在 HTTP 层，看不到这些值。

CC 上行请求里与身份有关的、代理**确实**能看到的只有这些：

| 通道 | 字段 | 是否含项目信息 |
|---|---|---|
| header | `X-Claude-Code-Session-Id` | 无。是进程内会话 UUID，`/clear` 等操作会 `regenerateSessionId()` 换新值 |
| header | `User-Agent`、`x-app: cli` | 无。只能判定 `clientKind=claude_code` |
| body | `metadata.user_id`（JSON 字符串） | 无。只有 `device_id` / `account_uuid` / `session_id` |

所以 CC 的所有请求在项目维度上是同质的，代理只能把它们归入「未识别项目」。

**会话维度是例外，它已经通了**：`X-Claude-Code-Session-Id` 与 `metadata.user_id.session_id`
都由 CC 无条件发送（同一个 `getSessionId()`）。代理侧 `observed_session_id()` 就是专门取这个
header 的（也兼容 `session_id`/`session-id`，有界且拒控制字符），值进 `sessions` 统计维度，
还兼作粘性调度键。所以 CC 的会话归因**零配置就有**，不需要设任何环境变量。

**要归因，只有让 CC 主动带上。** 源码里有两个官方入口，都不需要改 CC、不需要包 wrapper：

| 方式 | 配置 | 落点 | 取舍 |
|---|---|---|---|
| 自定义 header（**本仓库已实现**） | `ANTHROPIC_CUSTOM_HEADERS` 里给 `X-Kekulv-Project` / `X-Kekulv-Workspace` / `X-Kekulv-Git-Remote` | HTTP header | 代理零成本读取，读完即从出站剥离，上游中转站看不到；配置见 `USAGE.md` |
| body metadata | `CLAUDE_CODE_EXTRA_METADATA='{"project":"myproj"}'` | `metadata.user_id` 内的键 | 会被一起发给上游（中转站也看得到）；代理需二次 parse 字符串。本仓库**未**采用 |

两者都是**进程级环境变量**，所以得在启动 CC 的地方按目录设置（`direnv`、shell wrapper、per-project `.envrc` 之类），
CC 自己不会随 `cd` 更新。这也意味着归因值是**用户自称的**，不像 Codex 的 workspace metadata 那样由客户端结构化生成——
代理侧应当把它标成独立的 `project_source`（例如 `client_declared`），不要和 Codex 的 `workspace_local` 混为一类可信度。

**两个实测约束（2026-08-24 用假上游逐条验证，配置时踩得到）：**

1. **值必须纯 ASCII。** 含非 ASCII 时 CC 直接报
   `Invalid value for distinct header N of M parsed from ANTHROPIC_CUSTOM_HEADERS: it contains a
   non-ASCII character` 并**退出**，请求根本不发出。所以中文目录名会让 `claude` 在该项目完全不可用，
   而不是仅归因缺失。代理侧反倒能处理 UTF-8（`clean_header` 只拒控制字符），瓶颈纯在客户端校验。
2. **`settings.json` 的 `env` 优先级高于进程环境变量，且不做插值。** 写在 `env` 里的
   `ANTHROPIC_CUSTOM_HEADERS` 会覆盖 shell 设的值（同理会覆盖 `ANTHROPIC_BASE_URL` 等），
   而 `$PWD` / `${CLAUDE_PROJECT_DIR}` / `$HOME` 五种写法实测全部**字面**传出。两条合起来
   意味着：一旦把该键写进全局 `settings.json`，动态 wrapper 就永久失效，且只能是一个固定项目名。

**归因链路已完整实现**（不需要额外改代理）：`ClientDeclaredMetadata::from_headers`
（`kekulv-core/src/events.rs`，header 名大小写不敏感）→ `event_project_projection` /
`project_identity`（`kekulv-proxy/src/runtime_store.rs`，Codex 结构化 workspace 优先，缺位才用
declared，内部优先级 `project` → `workspace` → `git_remote`）→ SQLite 投影列 `project_id` /
`project_name` / `project_source` → 前端「客户端声明」标签。出站剥离由 `HEADER_BLOCKLIST` 保证，
Linux 侧有测试断言上游收不到任何 `x-kekulv-` 前缀。

**但「不外泄」只对 header 成立。** CC 每条请求的 body 里本来就带工作目录绝对路径、`CLAUDE.md`
全文与 `git status` 摘要，而代理对 `system` / `messages` 一字不改（出站 body 只改
`prompt_cache_key` / `prompt_cache_retention` / `thinking` / `output_config` / `model`）。
所以配这三个 header **不增加**任何外泄面，也不减少——它只决定代理能不能按项目统计。

源码：`ANTHROPIC_CUSTOM_HEADERS` 解析在
`/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/client.ts:330-354`（并在 `:105-116` 合并）；
`CLAUDE_CODE_EXTRA_METADATA` 与 `metadata.user_id` 组装在
`/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/claude.ts:503-527`；
`SAFE_ENV_VARS` 白名单在 `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/managedEnvConstants.ts:108-129`。

## 7. thinking、effort、speed 和上下文管理

| 概念 | 请求字段 | 说明 |
|---|---|---|
| thinking 开关 | `thinking.type` | disabled/enabled/adaptive 等；会影响可用的 temperature 语义 |
| thinking budget | `thinking.budget_tokens` | 非 adaptive 模式的思考预算，必须小于 `max_tokens` |
| effort | `output_config.effort` | Claude Code 可把路由/配置的 effort 写入 output config |
| task budget | `output_config.task_budget` | API 侧任务预算，不等于本地计费 token |
| structured output | `output_config.format` | 结构化输出格式；需要相应 beta |
| fast mode | `speed: "fast"` | 可能改变模型价格档；由 response usage 的 `speed` 确认实际生效模式 |
| context management | `context_management` | 上下文清理、thinking 清除或压缩相关策略 |

代理统计应同时保留请求中的逻辑配置和响应中的实际 `speed`；不能只根据 `model` 或客户端 UI 猜费用。

## 8. 上游响应和 SSE 生命周期

### 8.1 常见响应字段

| 字段 | 含义 |
|---|---|
| HTTP status | 响应头状态；200 不代表流一定完整结束 |
| `request-id`/SDK `request_id` | Anthropic 上游请求 ID |
| `message_start` | 初始 message 元数据，通常带 input/cache usage |
| `content_block_start` | text、thinking、tool_use、server_tool_use 等块开始 |
| `content_block_delta` | 文本、thinking、工具 JSON 增量或签名增量 |
| `content_block_stop` | 内容块结束 |
| `message_delta` | stop reason 和最终/更新后的 output usage |
| `message_stop` | 消息流终止 |
| `stop_reason` | `end_turn`、`tool_use`、`max_tokens` 等有限值 |
| `usage` | token/cache/server-tool 计数 |

Claude Code 在 `message_start` 和 `message_delta` 中调用 `updateUsage`；源码明确把 streaming usage 当作**累计值**，并避免用 0 覆盖已收到的 input/cache 字段。因此：

```text
正确：保存每个 response 的最终 usage，或按 message/response ID 去重后统计
错误：把每个 SSE event 的 input_tokens/cache_read_input_tokens 直接相加
```

源码依据：`/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/claude.ts:1980-2215,2914-3037`。

### 8.2 HTTP 200 与最终结果

响应头已经返回 200 后，仍可能出现 stream idle timeout、客户端断开、上游断流或协议不完整。代理应以 RuntimeEvent 的 `outcome`、`phase` 和 `failureKind` 判断最终结果，不能只看 status code。

## 9. Anthropic usage/token 字段

### 9.1 单次 response usage

```json
{
  "input_tokens": 0,
  "output_tokens": 0,
  "cache_read_input_tokens": 0,
  "cache_creation_input_tokens": 0,
  "cache_creation": {
    "ephemeral_5m_input_tokens": 0,
    "ephemeral_1h_input_tokens": 0
  },
  "server_tool_use": {
    "web_search_requests": 0,
    "web_fetch_requests": 0
  },
  "service_tier": "standard",
  "inference_geo": "...",
  "iterations": [],
  "speed": "standard"
}
```

| 字段 | 含义 | 统计口径 |
|---|---|---|
| `input_tokens` | 未从 prompt cache 读取的普通输入 token | 输入计费的一部分 |
| `output_tokens` | 模型生成 token | 输出计费 |
| `cache_read_input_tokens` | 缓存命中读取 token | 通常有独立低价 |
| `cache_creation_input_tokens` | 缓存创建写入总量 | 通常有独立写入价 |
| `cache_creation.ephemeral_5m_input_tokens` | 5 分钟缓存写入明细 | 适合细分缓存写入成本 |
| `cache_creation.ephemeral_1h_input_tokens` | 1 小时缓存写入明细 | 适合细分缓存写入成本 |
| `server_tool_use.web_search_requests` | Anthropic 服务端 web search 次数 | 可能按请求次数计费 |
| `server_tool_use.web_fetch_requests` | Anthropic 服务端 web fetch 次数 | 记录次数；具体价格按上游规则 |
| `service_tier` | 服务层级 | 不等于模型名 |
| `inference_geo` | 推理区域 | 可能缺失或受服务方控制 |
| `iterations` | 服务端工具循环/迭代 usage | 不能当作普通 SSE event 数 |
| `speed` | 实际速度模式 | Opus fast 等价格判断需要使用它 |

### 9.2 总量和上下文的不同公式

Claude Code 内部至少有三种不同口径：

1. **完整单次 response token**：`input + cache_creation + cache_read + output`。
2. **上下文百分比**：`input + cache_creation + cache_read`，源码没有把 output 加入 statusline 百分比。
3. **服务端最终上下文窗口**：有 `iterations` 时使用最后一次 iteration 的 `input + output`；没有 iteration 时使用顶层 `input + output`，该路径按源码注释排除 cache。

所以“总 token”“本次上下文占用”“计费 token”必须分别命名，不能用一个 `total_tokens` 字段覆盖所有含义。

### 9.3 费用计算

Claude Code 的成本计算形式为：

```text
cost = input_tokens / 1M × input_price
      + output_tokens / 1M × output_price
      + cache_read_input_tokens / 1M × cache_read_price
      + cache_creation_input_tokens / 1M × cache_write_price
      + web_search_requests × web_search_request_price
```

当前源码示例价格档：

| 价格档 | 输入 / 输出 | 缓存写入 / 读取 |
|---|---:|---:|
| Sonnet | $3 / $15 每百万 token | $3.75 / $0.30 |
| Opus 4/4.1 | $15 / $75 每百万 token | $18.75 / $1.50 |
| Opus 4.5/4.6 标准 | $5 / $25 每百万 token | $6.25 / $0.50 |
| Opus 4.6 fast | $30 / $150 每百万 token | $37.50 / $3.00 |
| Haiku 3.5 | $0.80 / $4 每百万 token | $1 / $0.08 |
| Haiku 4.5 | $1 / $5 每百万 token | $1.25 / $0.10 |

价格会随上游更新；未知模型可能使用默认价格并被标记为 unknown model cost。源码依据：`/Users/kkl/Documents/claude/claude-code-analysis/src/utils/modelCost.ts:22-88,128-141`、`/Users/kkl/Documents/claude/claude-code-analysis/src/cost-tracker.ts:255-301`。

## 10. Transcript JSONL 字段

典型 assistant 记录形状：

```json
{
  "type": "assistant",
  "message": {
    "id": "msg_...",
    "model": "claude-...",
    "role": "assistant",
    "content": [],
    "stop_reason": "tool_use",
    "usage": {
      "input_tokens": 0,
      "output_tokens": 0,
      "cache_read_input_tokens": 0,
      "cache_creation_input_tokens": 0
    }
  },
  "session_id": "...",
  "timestamp": "..."
}
```

统计 transcript 时必须注意：

- 同一个 API response 可能因多个 content block 产生多行 assistant 记录；
- 这些记录可能共享同一个 `message.id`；
- usage 常是累计/完整值，不能按每一行简单累加；
- 应按 `message.id` 或请求关联 ID 去重，并选择最终记录；
- transcript 可能包含完整工具输出、文件内容和代码，不能直接上传到普通 RuntimeEvent。

Claude Code 自身也通过 assistant message ID 识别并处理并行工具调用产生的拆分记录。相关实现见 `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/tokens.ts:22-37` 及 `/Users/kkl/Documents/claude/claude-code-analysis/src/cli/print.ts` 的 transcript 处理代码。

## 11. 代理 RuntimeEvent 字段

当前代理事件的稳定字段位于 Linux/macOS `kekulv-core/src/events.rs`。事件通常分为 `kind=client`、`kind=upstream` 和 `kind=notify`。

### 11.1 客户端、模型和协议

| 字段 | 含义 |
|---|---|
| `clientKind` | `claude_code`、`codex`、`grok_build`、`openai_compat`、`unknown` |
| `clientModel` | 入站请求体中的模型 |
| `effectiveModel` | 路由定型后的逻辑模型 |
| `upstreamModel` | 实际发给上游的模型名/别名 |
| `sourceFormat` | 入站真实协议，例如 Anthropic/OpenAI/OpenAI Responses |
| `targetFormat` | 出站真实协议 |
| `routeMode` | native 或 bridge 等路由方式 |
| `requestPurpose` | 请求用途标签，不是上游字段，也不参与路由决策 |
| `endpointID` / `endpointName` | 命中的上游入口 |
| `poolID` | 上游 provider pool |
| `featureRuleID` | 命中的功能/分流规则 |
| `failover` | 是否发生入口切换 |

`clientKind=claude_code` 的识别依据是 UA 以 `claude-cli/` 开头；不能仅凭模型名或请求体猜测客户端。

### 11.2 请求身份、状态和耗时

| 字段 | 含义 |
|---|---|
| `id` | 单条事件的 UUID/upsert key |
| `requestID` | 一次客户端请求及其全部上游尝试共享的请求 ID |
| `sessionID` | 客户端提供的稳定会话 ID，例如 Claude Code session header |
| `upstreamRequestID` | 上游返回的安全 request ID |
| `phase` | `inFlight` 或 `completed` |
| `outcome` | `succeeded`、`failed`、`cancelled` |
| `statusCode` | 客户端侧 HTTP 状态；499 表示客户端取消 |
| `upstreamStatusCode` | 实际收到的上游 HTTP 状态 |
| `ttfbMS` | 到收到上游响应头的时间 |
| `durationMS` | 请求/尝试总时长；client 与 upstream 口径不同 |
| `timeoutMS` | 生效的 response/stream idle 超时阈值 |
| `failureKind` | 结构化失败分类 |
| `failurePhase` | `before_response`、`response_headers`、`response_stream` |
| `failureDetail` | 有界、脱敏的技术详情 |

HTTP 200 只代表响应头已成功，不代表流完整成功；终态应以 `outcome` 和 `failureKind` 为准。

### 11.3 工具、流和 usage

| 字段 | 含义 |
|---|---|
| `toolCalls` | 该响应流实际观察到的工具调用名称 |
| `streamTrace.chunkCount` | 收到的流块数量 |
| `streamTrace.bytesReceived` | 收到的字节数 |
| `streamTrace.maxChunkGapMS` | 最大块间间隔 |
| `streamTrace.lastChunkAtMS` | 最后一个块时间 |
| `streamTrace.terminalEvent` | 观察到的终止事件 |
| `streamTrace.stopReason` | Anthropic `stop_reason` 或 OpenAI `finish_reason` |
| `streamTrace.usage.inputTokens` | 脱敏投影的 input token |
| `streamTrace.usage.outputTokens` | 脱敏投影的 output token |
| `streamTrace.usage.cacheReadInputTokens` | 脱敏投影的缓存读取 token |
| `streamTrace.usage.cacheCreationInputTokens` | 脱敏投影的缓存创建 token |

当前 RuntimeEvent wire 结构使用 `streamTrace.usage`，不是把完整上游 usage 原样放进事件，也不是默认保存完整响应正文。源码依据：`/Users/kkl/.claude/automode-proxy/linux/crates/kekulv-core/src/events.rs:1500-1535,1655-1757`。

### 11.4 `requestPurpose` 当前值

| 值 | 含义 |
|---|---|
| `standard` | 普通主对话或未命中特殊指纹的请求 |
| `session_title` | 会话标题生成 |
| `websearch` | WebSearch 辅助请求 |
| `webfetch` | WebFetch 辅助请求 |
| `classifier` | 分类器请求 |
| `compact` | 上下文压缩请求 |
| `image_generation` / `image_edit` | 图片请求 |
| `alpha_search` | Alpha Search 请求 |
| `token_count` | token 计数请求 |

`standard` 只表示“普通/未命中特殊用途”，不能单独证明是主代理，也不能单独证明不是子代理。

## 12. 推荐的统计展示口径

如果要在客户端或代理 Admin 中做统计，建议至少拆成以下维度：

| 统计项 | 推荐字段/计算 | 不能替代的字段 |
|---|---|---|
| 请求次数 | 唯一 `requestID` 数量 | 事件行数；一次请求可能有 client + 多个 upstream |
| 总普通输入 | 去重后的 `input_tokens` 求和 | `context_window.total_input_tokens` |
| 总输出 | 去重后的 `output_tokens` 求和 | 最后一条 SSE 的 output 增量 |
| 缓存命中 | `cache_read_input_tokens` 求和 | `cache_control` 断点数量 |
| 缓存写入 | `cache_creation_input_tokens` 求和，并细分 5m/1h | cache creation 是否一定产生命中 |
| 缓存命中率 | `cache_read / (input + cache_read)`，明确分母 | 直接用上下文百分比 |
| 当天统计 | 按事件 timestamp 转换到用户时区 | Unix/Apple epoch 混用 |
| 项目统计（本机 statusLine 场景） | 脱敏后的 `workspace.project_dir` | 直接保存完整本机路径 |
| 项目统计（代理场景） | 只能用客户端显式声明的 header/metadata 键（见 6.3）；缺失时归入「未识别项目」 | `project_dir`／`cwd`——它们不在上行请求里，代理拿不到；也不要拿 prompt 里出现的路径反推 |
| 模型统计 | `effectiveModel`、`upstreamModel` 分列 | 只按 `clientModel` 计费 |
| 费用 | 使用 response usage + 实际 `speed` + 当前价格表 | 从模型名猜费用 |

统计实现必须：

1. 以 `requestID`/上游 request ID/message ID 去重；
2. 只在完成事件或确认的最终 usage 上累计；
3. 把 retry/failover 的多次 upstream 尝试和用户请求分开统计；
4. 对旧事件缺失字段使用“未知”，不要补写猜测值；
5. 在 UI 上分别展示“普通输入、输出、缓存读、缓存写、总费用”，不要只显示一个模糊的总 token。

## 13. 最小脱敏事件样例

```json
{
  "id": "EVENT-UUID",
  "kind": "client",
  "clientKind": "claude_code",
  "clientModel": "claude-opus-4-6",
  "effectiveModel": "claude-opus-4-6",
  "upstreamModel": "provider-alias",
  "sourceFormat": "anthropic",
  "targetFormat": "anthropic",
  "routeMode": "native",
  "requestPurpose": "standard",
  "requestID": "REQUEST-UUID",
  "sessionID": "SESSION-UUID",
  "phase": "completed",
  "outcome": "succeeded",
  "statusCode": 200,
  "upstreamStatusCode": 200,
  "ttfbMS": 820,
  "durationMS": 4200,
  "toolCalls": ["Read", "Edit"],
  "streamTrace": {
    "chunkCount": 184,
    "bytesReceived": 91234,
    "terminalEvent": "message_stop",
    "stopReason": "tool_use",
    "usage": {
      "inputTokens": 12000,
      "outputTokens": 1800,
      "cacheReadInputTokens": 42000,
      "cacheCreationInputTokens": 3000
    }
  }
}
```

该样例刻意不包含 Authorization、API key、Cookie、完整 URL、prompt、system、tools schema、工具参数和响应正文。

## 14. 安全和证据边界

- RuntimeEvent 默认只保存脱敏、有限长度的元数据和 usage 摘要。
- Diagnostic Capture 是单独的临时调试能力；它可能保存完整 body/chunk，不能与普通事件混用。
- `Authorization`、`x-api-key`、Cookie、OAuth token、AWS credentials 和自定义敏感 header 不得写入文档、日志或导出文件。
- 本地 transcript 是完整会话数据，读取前要确认用途，上传前必须脱敏、截断并取得明确授权。
- “上游返回 usage”只代表该 response 暴露了 token/cache 计数，不代表代理能够看到 Anthropic 账户总账单、所有组织级限额或完整服务端内部成本。
- 诊断时应同时记录 request ID、session ID、上游 request ID 和时间，不要只凭模型名或 HTTP 200 推断原因。

## 15. 源码索引

| 主题 | 源码 |
|---|---|
| 状态栏 JSON schema | `/Users/kkl/Documents/claude/claude-code-analysis/src/tools/AgentTool/built-in/statuslineSetup.ts` |
| 状态栏实际值 | `/Users/kkl/Documents/claude/claude-code-analysis/src/components/StatusLine.tsx` |
| 状态栏命令执行/超时 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/hooks.ts` |
| Anthropic client/header | `/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/client.ts` |
| Messages 请求、SSE、usage 合并 | `/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/claude.ts` |
| token 口径和 transcript 去重辅助 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/tokens.ts` |
| 上下文百分比 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/context.ts` |
| 模型价格 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/modelCost.ts` |
| 本地费用/分模型累计 | `/Users/kkl/Documents/claude/claude-code-analysis/src/cost-tracker.ts` |
| 代理 RuntimeEvent/ResponseUsage | `/Users/kkl/.claude/automode-proxy/linux/crates/kekulv-core/src/events.rs`、`macos/crates/kekulv-core/src/events.rs` |
| 自定义 header 解析 / metadata 组装 | `/Users/kkl/Documents/claude/claude-code-analysis/src/services/api/client.ts:330-354`、`src/services/api/claude.ts:503-527` |
| baseUrl first-party 判定（决定 `x-client-request-id` 是否注入） | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/model/providers.ts:25-38` |
| UA 组装 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/http.ts:18-35` |
| managed settings 环境变量白名单 | `/Users/kkl/Documents/claude/claude-code-analysis/src/utils/managedEnvConstants.ts:108-129` |
| 会话 ID 生成/重生成 | `/Users/kkl/Documents/claude/claude-code-analysis/src/bootstrap/state.ts:431-443` |
| Codex 对照文档 | `/Users/kkl/.claude/automode-proxy/codex客户端的参数变量字段对应.md` |

