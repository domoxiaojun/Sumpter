# 请求策略与客户端诊断

代理 HTTP 单请求体与解压输出默认均为 **256 MiB**，入站受控缓冲共享 **1024 MiB**，最多 **4** 个请求同时读体、解压与解析，最多 **2** 个同时解压。读体空闲 **30 秒**、总时限 **300 秒**。鉴权先于资源申请与读体，超大小返回 413，读体超时返回 408，资源不足返回 429 并附 Retry-After，网络读取或压缩格式错误返回 400。这些是入站保护，不限制正常上游生成或 SSE 长流时长。

`requestPolicy.ingress` 与 `requestPolicy.websocket` 的所有值都必须是有限正整数。旧 schema v7 配置省略字段时使用安全默认值，无数据库迁移。HTTP 缓冲按需申请，覆盖仍存活的原始体、解压输出与正文重写副本；Bytes 的共享引用只计一次，排队、重试和响应期间仍保留的正文不会提前归还预算。它是受控字节缓冲预算，不是整个进程 RSS 上限；解析器对象、TLS/网络栈等仍有额外开销。增长期间新旧缓冲同时计账，因此额度紧张时可能在达到单体上限前返回 429。

Admin 登录正文独立限制为 2 MiB，密码计算前还执行全局和真实 TCP 来源令牌桶，并最多同时运行两个密码 worker。429 响应携带等待秒数，登录页显示倒计时。客户端转发头不作为限速来源。

Responses 流式重试只适用于可证明可重放的请求。含 MCP、shell、computer-use 或未知服务端工具的请求，以及文件/视频等写资源请求，不会在上游已经产生输出后透明重发；客户端工具和明确只读工具仍可按原策略保护。

WebUI 和 macOS 的“Claude Code 路由”页面提供“全客户端请求策略”，此面板对所有客户端生效。策略应用于普通 HTTP/SSE 的实际上游尝试；模型目录刷新、健康探测和 WebSocket 连接不计入 HTTP 上游尝试额度；WS 使用单独的连接配额。路由授权仍由模型组及入口映射决定。无需迁移或清空运行数据库。

## 入站安全额度

| 字段（位于 requestPolicy） | 默认值 | 允许范围 |
| --- | --- | --- |
| ingress.maxBodyMib | 256 | 1–1024 MiB |
| ingress.maxDecodedBodyMib | 256 | 1–1024 MiB |
| ingress.maxBufferedMib | 1024 | 1–16384 MiB，至少等于上述两项之和 |
| ingress.maxPreprocessing | 4 | 1–64 |
| ingress.maxDecompressions | 2 | 1–64 |
| ingress.bodyIdleTimeoutSeconds | 30 | 1–3600 秒 |
| ingress.bodyReadTimeoutSeconds | 300 | 1–86400 秒，不得小于空闲时限 |
| websocket.maxFrameMib | 256 | 1–1024 MiB，不得超过消息上限 |
| websocket.maxMessageMib | 256 | 1–1024 MiB，累计所有分片 |
| websocket.maxConnections | 4 | 1–1024 |
| websocket.firstMessageTimeoutSeconds | 30 | 1–3600 秒 |

WebSocket 在连接上游前申请共享连接配额，超额返回 429 + Retry-After；两个方向和固定 IP 上游连接都使用同一份消息/帧限制。超大帧或累计分片超限发送 1009，释放中继和配额。握手后必须在期限内收到第一个客户端业务消息，Ping/Pong 和上游问候不能延长期限；之后沿用原有空闲策略。超过 64 MiB、仍在配置上限内的正常消息保持可用。

配置保存影响新请求与新连接；既有 WS 保留握手时的消息大小上限。降低共享额度不强杀现有工作；在占用回落前，新申请快速返回 429，由客户端依 Retry-After 重试。响应回放的 maxBufferedMib 和上述 ingress.maxBufferedMib 是分别记账的预算。

## 默认值和交互预设

旧配置没有 requestPolicy 时仍采用原有总尝试预算和完整缓冲模式，并启用连续无响应保护（同一入口连续 3 次，拦截 300 秒）。这是首响应前连接失败或超时的默认行为变化；显式设置 maxConsecutiveNoResponse=0 可关闭。并发和 RPM 默认不限；缓冲有共享预算，避免并发长流逐一占满单流上限。

| 字段 | 默认值 | 含义 |
| --- | --- | --- |
| maxTotalAttempts | 0 | 沿用原 retry 预算；设为正整数后限制一个 HTTP 请求的总发送次数，包含粘性、换入口、500、SSE 和跨轮重试 |
| responseWaitTimeoutSeconds | 0 | 从转发开始到客户端响应头提交的总等待上限，包含排队、重试、退避和缓冲；0 不限，已开始交付的长流不截断 |
| maxConsecutiveNoResponse | 3 | 同一会话、同一请求在同一入口连续未收到响应头的失败上限；含连接错误与单次响应头超时，跨 CLI 重发累计；0 关闭，最大 100 |
| noResponseCooldownSeconds | 300 | 达到连续无响应上限后，相同请求在该入口的拦截秒数；1–3600，拦截期间重复请求不延长截止 |
| global.maxConcurrent | 0 | 全局同时处理的上游请求；0 不限，名额保留至上游流结束或取消 |
| global.maxQueued | 128 | 最多等待请求数；0 立即返回本地 429 |
| global.queueTimeoutSeconds | 30 | 最长排队等待秒数 |
| global.requestsPerMinute | 0 | 滚动 60 秒内实际上游尝试数，重试也计数；0 不限 |
| streamProtection | complete | 完整缓冲；prelude 是可选前奏保护 |
| streamBufferMib | 16 | 单流回放正文额度 |
| streamBufferChunks | 8192 | 单流上游响应块数；不等同 SSE 事件数 |
| streamBufferSeconds | 0 | 缓冲时限；0 仅受其它额度和流空闲超时约束 |
| maxBufferedMib | 128 | 所有回放流共享的正文及块元数据额度 |

“填入交互预设”将总尝试上限设为 6、全局并发设为 8、排队数设为 16、排队时限设为 15 秒。保存前可以调整；预设不会自动启用流错误重试或改写其它 retry 字段。

仓库的合成 config.example.json 使用这一交互预设；既有配置缺省值仍按上表处理，不会自动套用。

入口限制在 endpoints 的精确入口 ID 下设置 limits，字段与 global 相同。全局与入口约束同时生效；队列和时限采用两者较严格的约束。可以在同一调度组内优先使用空闲候选；本地拥塞不自动跨组启用后备。

排队已满或超时返回 HTTP 429、scope=local、error=queue_full/queue_timeout 和 Retry-After: 1。事件 failureKind=local_admission_rejected，不降低供应商健康状态。客户端取消会移除等待项，不发送上游请求。

## 等待截止与连接中断

responseWaitTimeoutSeconds 可设为 0–86400 秒，默认 0 禁用。显式开启后，从路由完成开始转发计时，排队、上游连接与响应头等待、错误正文探测、退避和 Responses 缓冲共用截止；到时取消未完成工作，返回本地 HTTP 504，failureKind=response_wait_timeout、timeoutMS=配置值，不冒充上游 504 或客户端取消。提交客户端响应后解除这个截止，正常长回答不截断；WebSocket 不受影响。

retry.responseTimeoutSeconds 则是每次上游请求的响应头超时，超时后仍按重试策略继续；retry.maxRetryDurationSeconds 只在重试边界检查，不强制截断正在等待的尝试。显式选择无限循环时，将 maxDeferredRounds、maxRetryDurationSeconds、maxTotalAttempts、responseWaitTimeoutSeconds 和 maxConsecutiveNoResponse 均设为 0。显式正值限制仍会结束重试；客户端关闭连接也会终止当前工作。

HTTP 请求处理或响应体提前释放记为 client_disconnected，保留 499 和 cancelled 计数兼容性，两端显示“连接中断（原因未确定）”。它不能证明用户主动取消，也可能来自客户端超时/自动重试、连接丢失或服务取消；旧 client_cancelled 记录不迁移、不删除。

运行页选中请求链后持续更新进行中请求；终态停止，暂停刷新或离开页面停止读取。快照按 changeSeq 合并，不覆盖较新的事件；最多显示 512 条，超出时显示“至少”及截断提示。客户端计时不是模型已经开始输出的证据。

## 连续无响应与重复请求保护

对 Messages、Chat Completions、Completions、Responses（含 compact）和 Gemini 会话生成的 POST 请求，代理按入口累计首响应前连接失败与响应头超时。默认同一入口连续失败 3 次即停止在本次请求中向它重发，继续检查未耗尽的后备入口；没有可继续尝试的候选时，结束客户端请求。收到任何实际 HTTP 响应即清零该入口的首响应前失败计数，包括 429/503；这些 HTTP 错误继续使用原重试策略。已提交响应的长流、文件/视频/Realtime 等资源操作以及 WebSocket 不使用这项保护。

达到保护阈值后，返回本地 HTTP 422，error.code 和 failureKind 为 no_response_retry_exhausted，failurePhase=before_response、scope=local、retryable=false，附 x-should-retry: false 和 cache-control: no-store。cooldownSeconds 表示最早可重新尝试的受保护入口剩余秒数；响应不附自动重试等待头。原有单次上游尝试仍记为连接失败/响应超时和本地 502，不伪造上游 HTTP 状态或上下文超限。两端请求详情与时间线显示“连续未收到响应，已停止重试”。客户端应结束自动重发，检查网关或调整上下文后重新发起请求；自行忽略终止提示的客户端仍可能继续显示忙，但命中保护的请求不会发送到该入口。

保护按配置快照、入站 Token 身份、客户端来源、会话、请求路径、协议相关头、入口和实际出站正文摘要隔离。客户端更换 request ID 不会清零，修改正文、切换会话或更新凭据/路由配置可重新尝试。没有明确会话标识时使用既有会话指纹与请求来源。正文不会为恢复重试而自动压缩或删减；工具调用和签名保留原有转发规则。内存中只保留摘要与计数，最多 2048 项，未达到阈值的跨请求失败记录空闲 30 分钟后过期；保护重启后清空，容量淘汰也可能提前解除跨请求保护，不写运行数据库。单次请求内已经耗尽的入口仍保持耗尽。

外部监控应分开记录重试活动和有效进展：失败事件、退避结束、连接建立及生命周期帧不能重置“无有效输出”计时。有效进展需来自实际输出或工具执行/结果；HTTP 200 与日志数量增长不足以证明进展。原故障的“约 60 秒 TLS EOF”仍需要目标实例的上游/网关日志确认原因，不能仅据旧请求较长就认定超出模型上下文上限。

## 重试和流式恢复

仅识别可确认的结构化错误用于诊断：insufficient_quota、billing_hard_limit_reached、Google 日额度等仍按 429/503 的首响应前重试策略留在代理内部，不向客户端透传中间错误；已知 invalid_api_key / authentication_error / invalid_grant 与模型不支持仍停止同入口重试。无法确认的错误保持原状态码策略。Codex Live 的既有鉴权隔离和有界 bootstrap 行为保留。

Retry-After、retry-after-ms 和 Google RetryInfo 统一取有效等待提示，并加正向抖动。等待仍受既有 300 秒提示上限和请求重试预算约束；超大提示不会导致等待数小时。总尝试预算只统计单个入站请求内 Sumpter 的尝试；连续无响应保护可跨客户端重发，但都不能控制 CPA 在一次上游连接内部的重试。

Responses SSE 仅在 retry.maxStreamErrorRetries > 0 或 retry.failoverOnStreamError=true 时启用保护。background 请求和压缩流不进入这条回放缓冲路径。

- complete：默认保留到终态，允许在尚未提交客户端之前恢复中途断流；客户端看到输出较晚。
- prelude：首个非生命周期帧触发提交，随后直接转发；为保守起见，未知或畸形事件也触发提交。提交后的失败由客户端处理，不透明重放。
- 达到字节、块数、时间或总额度时，保留已经读取的字节并恢复透传，详情记录具体原因。额度按读取边界检查，可能暂存当前刚读入的一个块；不承诺进程 RSS 严格等于配置值。网络栈、解析器和普通透传缓冲也不属于该预算。

## 显式兼容声明

支持 strictTools、grammarTools、developerRole、structuredOutput 四个三态字段：true、false、null（未知）。未知保留原生请求；false 只在实际用到该特性时跳过入口；全部候选均不支持则返回 400，不静默删除工具、改写角色或猜测能力。

以下片段放在 requestPolicy.endpoints 中；入口 ID 和上游模型 ID 必须与实际配置对应：

```json
{
  "example-main": {
    "limits": { "maxConcurrent": 4 },
    "models": { "*": { "strictTools": null } },
    "protocolModels": {
      "openai-responses": {
        "synthetic-model": {
          "strictTools": true,
          "grammarTools": false,
          "developerRole": true,
          "structuredOutput": null
        }
      }
    }
  }
}
```

协议键为 anthropic、openai、openai-responses、gemini。查找次序为协议内精确模型、协议内 *、通用精确模型、通用 *；匹配的是整个档案，未填字段保持未知，不从较宽档案继承。严格协议桥仍可拒绝目标协议无法表达的内容。

普通模型目录的 sumpter_compatibility 是 **Responses 路由候选的保守交集**：有明确 false 时为 false，全部明确支持才为 true，否则为 null。这是用户配置声明，不是实时供应商认证，不用于声明其它入站协议的能力。既有能力来源和冲突信息继续保留。

模型目录对重叠刷新合并探测；后续刷新仍向上游验证。上游提供 ETag 时使用 If-None-Match/304，按入口指纹（包括凭据、固定解析 IP）、URL 和完整请求头隔离。缓存保留一小时，最多 32 项/16 MiB；失败沿用既有旧值与时间标记。它不缓存模型推理响应。

## 路由解释和阶段耗时

两端请求详情展示 requestTrace，最多 64 步，额外步骤用 omitted 计数；分页省略详情时保留已加载的时间线。trace 随事件原始 payload 持久化，旧事件没有时间线则不显示。

- 规划、候选与跳过：展示授权候选、原生/转换、显式能力拒绝和同组容量选择。
- 排队：获得发送名额之前的等待。
- 响应头：一次发送到收到上游 HTTP 头的耗时，包含连接和上游准备；不会伪造独立 DNS/TLS 时间。
- 缓冲：保护阶段等到终态、首输出或预算回退的时间。
- 重试：计划等待与实际等待分别记录；取消的等待没有“等待结束”。
- 首个响应块、完成：对应代理转发阶段；首块可以是生命周期帧，不能当作首个业务 Token。

客户端 TTFB 包含本地等待、重试和缓冲；最终上游 TTFB 与输出速度仍使用既有独立字段。trace 不增加提示词、工具参数或凭据；正文仍仅通过原有按需诊断捕获。

## 客户端识别诊断

在安全页检查客户端身份绑定，并在运行页核对最近请求的归因依据。客户端接入诊断以实际请求记录为依据；没有观察到请求时保持未知，代理地址配置成功并不证明实际流量或协议可用。

客户端识别直接读取请求的原生标记。缺少产品标记的客户端使用独立入站 Token 绑定；缺少项目或会话证据时保持未知。脚本退役与迁移说明见 [客户端归因](client-attribution.md)。

## 客户端来源与验证矩阵

版本指本次读取的本地源码快照，夹具是合成协议数据，不包含真实用户内容。

| 参考项目 | 固定提交 | 本次验证边界 |
| --- | --- | --- |
| Codex CLI | 5e769a1cda | 原生 Responses 工具/turn-state 形态、预热与 append/steer/interrupt 观察；grammar 的有损桥接拒绝 |
| pi | 1f78cea7a + 本地定制 | 原生工具 ID/结果；本地 SDK 显式亲和配置及 Responses cacheRetention=none |
| Gemini CLI | b633c0703 | 原生多轮 functionCall/thoughtSignature；既有双 adapter 签名回放和协议转换合同继续运行 |
| CLIProxyAPI | 8289ec4c + 本地定制 | 仅作为设计参照；其 fidelity hook 不能假定所有 CPA 安装都有 |
| magpie | 244d80e23 | 仅作为并发、队列与诊断设计参照 |

夹具位于 [client_contracts_v1.json](../tests/fixtures/client_contracts_v1.json)，双 adapter 入口为 [request_policy.rs](../tests/contracts/request_policy.rs)。正常 CI 离线执行；真实供应商、已安装客户端和设备验收需单独进行。
