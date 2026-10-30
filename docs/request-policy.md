# 请求策略与客户端诊断

WebUI 和 macOS 的“Claude Code 路由”页面提供“全客户端请求策略”，此面板对所有客户端生效。策略应用于普通 HTTP/SSE 的实际上游尝试；模型目录刷新、健康探测和 WebSocket 连接不计入此限额。路由授权仍由模型组及入口映射决定。无需迁移或清空运行数据库。

## 默认值和交互预设

旧配置没有 requestPolicy 时仍采用原有重试次数和完整缓冲模式。并发和 RPM 默认不限；缓冲新增共享预算，避免并发长流逐一占满单流上限。

| 字段 | 默认值 | 含义 |
| --- | --- | --- |
| maxTotalAttempts | 0 | 沿用原 retry 预算；设为正整数后限制一个 HTTP 请求的总发送次数，包含粘性、换入口、500、SSE 和跨轮重试 |
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

## 重试和流式恢复

仅识别可确认的结构化错误：insufficient_quota、billing_hard_limit_reached、Google 日额度等不会在同一请求内反复尝试同一入口；已知 invalid_api_key / authentication_error / invalid_grant 也停止同入口重试。无法确认的错误保持原状态码策略。Codex Live 的既有鉴权隔离和有界 bootstrap 行为保留。

Retry-After、retry-after-ms 和 Google RetryInfo 统一取有效等待提示，并加正向抖动。等待仍受既有 300 秒提示上限和请求重试预算约束；超大提示不会导致等待数小时。总尝试预算只统计 Sumpter 的请求，不能限制客户端或 CPA 内部再次重试。

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

## 只读客户端诊断

在运行客户端的机器、对应终端环境执行：

```bash
node client-attribution.mjs doctor all --proxy-url http://127.0.0.1:57878
```

发行包脚本位于 scripts 目录；统一安装器的私有副本位于 ~/.local/share/sumpter/attribution/client-attribution.mjs（可由 XDG_DATA_HOME 改变）。可将 all 改为 claude、codex、gemini、pi、grok。

doctor 只显示可见配置中的脱敏地址、安装状态、实际已安装包装器版本和 pi 显式标记；不访问鉴权存储、不执行客户端或模型请求、不写文件。项目配置、启动参数和其它进程环境可能覆盖这些来源，因此不能把“未观察到”当成“绕过代理”。地址同源只说明主机和端口匹配，不证明实际流量或协议可用。

macOS 诊断页可执行本机配置检查，但 Finder 启动的 App 无法读取另一个终端当前的环境；WebUI 显示近期已观察到的客户端流量并提供命令，浏览器不读取远程客户端文件。

pi 项目归因必须显式配置 X-Sumpter-Client: pi。Chat Completions SDK 的额外会话亲和头通过 model.compat.sendSessionAffinityHeaders=true 启用；localhost/127.0.0.1 不再自动代表 Sumpter。项目根目录和 Git remote 缓存最多 64 个工作目录、TTL 30 秒，合并在途读取；CWD 换键即重新读取，失败不缓存，会话与请求来源仍逐次取值。

## 客户端来源与验证矩阵

版本指本次读取的本地源码快照，夹具是合成协议数据，不包含真实用户内容。

| 参考项目 | 固定提交 | 本次验证边界 |
| --- | --- | --- |
| Codex CLI | 5e769a1cda | 原生 Responses 工具/turn-state 形态、预热与 append/steer/interrupt 观察；grammar 的有损桥接拒绝 |
| pi | 1f78cea7a + 本地定制 | 原生工具 ID/结果；归因钩子、短期缓存与只读诊断；本地 SDK 显式亲和配置及 Responses cacheRetention=none |
| Gemini CLI | b633c0703 | 原生多轮 functionCall/thoughtSignature；既有双 adapter 签名回放和协议转换合同继续运行 |
| CLIProxyAPI | 8289ec4c + 本地定制 | 仅作为设计参照；其 fidelity hook 不能假定所有 CPA 安装都有 |
| magpie | 244d80e23 | 仅作为并发、队列与诊断设计参照 |

pi 的 ctx.streamSimple/requestSource 是此工作区的本地扩展；Sumpter 归因仍兼容未提供该来源字段的 before_provider_headers 钩子。独立调用 SDK 的后台扩展不保证经过会话钩子。上级 pi 源码修改需在该仓库单独构建/发布后才影响其安装版本，Sumpter 发布不会分发 pi SDK 补丁。

夹具位于 [client_contracts_v1.json](../tests/fixtures/client_contracts_v1.json)，双 adapter 入口为 [request_policy.rs](../tests/contracts/request_policy.rs)。正常 CI 离线执行；真实供应商、已安装客户端和设备验收需单独进行。
