# 架构说明

Sumpter 的核心边界是“客户端请求进入一个地址，代理按配置选择上游并记录结果”。Linux 与 macOS 共享数据面，平台 adapter 只负责监听、Admin、生命周期和原生能力。

## 请求路径

```text
客户端
  → listener 访问检查与请求识别
  → 模型 / 能力 / featureRule 路由规划
  → 会话粘性、优先级、随机或轮询调度
  → Provider 上游认证与 raw relay
  → 响应 / 流 / WebSocket 回传
  → RuntimeEvent 与 usage 写入 SQLite
```

会话请求按协议择路。入口协议与客户端一致时保留客户端方法、路径、查询、请求体和响应流，代理只做鉴权、映射、路由、重试与上游凭据注入；不一致时在该模型的映射范围内选择转换候选，把请求转成目标协议后发出，响应再转回客户端方言。Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 与 Gemini `generateContent`/`streamGenerateContent` 四个会话协议之间两两可转；Compact、countTokens、embedContent、图片生成与 Realtime 等没有会话语义的接口不参与转换，仍按原生路径转发。

转换面按「能力检查 + 构造」同源实现。目标协议表达不了的项（服务端工具、provider 文件引用、Responses 没有等价参数的 `stop_sequences` 等）显式拒绝并回报具体字段，不静默降级。Chat/Responses 的 JSON Schema 约束及工具结果中的图片只转换到能保留其语义的目标；Responses 的 `previous_response_id` 等服务端上下文引用不能跨协议还原时拒绝转换，原生请求不受此限制。Gemini SSE 文本按增量拼接，工具调用的流式与非流式结束原因保持一致。

客户端工具以本轮 `tools` 声明为准：即使带有 `defer_loading:true` 且历史发现结果已被压缩，真实工具仍加入目标协议的工具定义；仅剔除带该标记的 `DeferredToolPlaceholder` 占位符。`tool_reference` 保留名称与原位置，未知引用或强制调用未声明工具仍明确拒绝，不从摘要文字推断或重建工具。Chat/Responses 保留显式 `strict`，Anthropic 未设置时输出 `strict:false`，避免 Responses 默认严格化改变可选参数；并发限制映射到 `parallel_tool_calls`。Gemini 无法表达的 `strict:true` 和禁用并发调用明确拒绝；非直接 `allowed_callers` 约束也不跨协议降级。原生 Anthropic 转发保留工具定义和引用块。

Responses 参数按 item ID / output index 关联，接收增量及 `function_call_arguments.done`、`output_item.done`、`response.completed.output` 的完整快照；稳定前缀实时发送，使 JSON 闭合的分片暂缓至工具项或响应结束；期间快照可将 `{}` 或较短对象补齐为更多字段。已有字段值和稳定前缀不可改写，工具项结束后已提交的参数不可变更；冲突、歧义和畸形 JSON 明确失败。反向桥共用 Anthropic 工具块校验，保留起始 `input` 和无参数调用。各桥接方向在调用完成前验证参数为完整 JSON 对象，失败后不再接受后续成功终态，不把截断参数替换成 `{}`。

HTTP/SSE 按请求及业务终态结算；Realtime/Live WebSocket 保留连接与关闭诊断，握手成功不等于模型生成成功。一个客户端请求可包含多次上游尝试，RuntimeEvent 用同一 request ID 关联它们，界面分别展示客户端最终结果和上游尝试链。

HTTP 响应观察支持 identity、gzip、br、zlib 格式的 deflate 与 zstd，最多四层组合编码按逆序增量解码。原生转发及原始捕获保留线上字节和 Content-Encoding；协议观察及转换读取解码视图。压缩响应必须转发尾部并在 EOF 校验后结算，Gemini 终态后的用量帧继续计入。观察块最多 64 KiB，每次响应累计解码观察上限 64 MiB；未知编码、非法压缩、压缩截断或超限以 response_observation_failed 表示“无法确认响应完成”，保留 HTTP 状态和明确原因，不伪装成网络中断或成功。原生响应继续转发，已开始的响应不重放；协议转换遇到解码失败时停止产生转换正文，不合成完成或错误帧。

Responses 仅支持 HTTP/SSE；通过鉴权的 WS 升级探测在共享入站层返回 426，不建立上游连接、不计推理事件。两个 adapter 只为 Realtime/Live 路径进行 WS 升级。模型目录始终输出 `prefer_websockets=false`，与上游模板或能力缓存无关；不提供重新开启 Responses WS 的配置开关。

历史 `websocketTrace.requests` 结构和 `runtime_codex_requests` 计数投影继续用于旧记录的读取、查询与导出；新 HTTP/SSE 请求沿用逐请求投影。移除入口不迁移或清空数据库，不回填历史缺失字段。运行汇总 API v1 移除可选的 `responsesWebSocketConnections`，两端界面不再用连接合成请求行。

## 配置与调度

`AppConfig` 的 v7 JSON 是跨平台合同。入口保存地址、Key、协议标签和 mapping；模型组保存开放模型、绑定和调度策略；featureRules 负责独立子请求。配置保存使用 generation 检查，避免两个 Admin 页面互相覆盖。运行期 Admin 保存、磁盘重载与模型目录写入共用 daemon 配置事务；临时文件使用独立随机名称。macOS App 在 sidecar 运行时经 loopback/control-token 保护的 `PUT /admin/config` 提交 `expectedGeneration` 与 `config`，收到 generation/warnings 回执后按原流程处理监听变更；sidecar 停止时才直接离线保存。

会话粘性优先于新会话调度。`priority` 按数字和数组顺序排序；`randomSticky` 和 `roundRobinSticky` 只决定新会话首选，故障仍遵循重试与后备链。资源绑定文件保存 Live / Video 等后续请求的入口归属。

## 运行时存储

`sumpter-runtime` 使用 bundled SQLite、WAL 和有界后台写入。内存快照让请求热路径不等待每次数据库写入；Admin 通过 runtime 层异步查询 API 等待结果，最多 4 个专用查询工作器执行同步 SQLite 操作，不占用代理 Tokio 工作线程；客户端取消会停止查询/导出游标，已接纳的写操作完成提交与内存对账后才释放名额。存储退化和 backpressure 通过 Admin 状态暴露。运行统计、会话删除、清理、重置和重建是不同操作，文档不得混用。

统计只从客户端完成事件的上游 usage 聚合，pending 单独计数；缓存 Token 按协议口径保留原始值。存储计数按事件状态变更维护，并叠加有界待提交增量；保留策略删去的计数不会被旧 Engine 累计快照写回。诊断捕获独立于统计，默认关闭，可能包含未脱敏正文和凭据。SQLite 写入背压是统计降级状态，不会关闭代理数据面；事件以零游标通知客户端重新同步，Admin 仍展示 backpressure 和最后错误。

诊断 Body/Chunk 保留字符串字段，并使用可选的 inboundBodyEncoding、outboundBodyEncoding、dataEncoding 区分 utf8 与 base64；缺省是旧文本格式。二进制、压缩数据及跨块不完整的 UTF-8 使用 Base64 无损保存，容量计入编码后的长度，截断只保存可逆的原始字节前缀。索引不携带正文，详情与大块展示按需读取。原始导出保留编码；脱敏副本移除编码／压缩载荷并增加 redacted 或相应 BodyRedacted 标记，不修改源记录。旧文本捕获已经损失的二进制无法恢复，旧事件不重算；配置和数据库 schema 不变。

分页快照同时使用 `snapshotSeq`、`snapshotChangeSeq` 与 `historyGeneration`。事件入队 `seq` 不变，首次完成水位固定快照的已完成集合；后续补充 metadata 不改变该水位。缺少完成水位的旧 token 明确失效，由客户端重取快照，不静默产生重复行或漏行。数据库以补列和回填方式兼容现有事件，不重建历史。

请求事件的 `sourceIP` 由共享 core 的 `access::resolve_client_ip` 在请求进入时解析一次，HTTP 与 WebSocket 入口共用：按 `X-Real-IP`（单个纯 IP）、`X-Forwarded-For`、入站 TCP 对端的顺序取第一个合法值，支持 IPv4 / IPv6，不要求对端命中 `listener.trustedProxyCIDRs`；该列表只决定 `X-Forwarded-For` 链的取法（为空取最左侧，非空从右向左跳过所列代理）。非法或超限头跳到下一来源。不解析 `Forwarded` 与 PROXY protocol。转发头由请求方声明，解析结果只用于事件：入站鉴权、`allowedCIDRs` 与 `/__status` 继续判断 TCP 对端，上游转发头不改写。该字段随客户端请求、上游尝试、拒绝、取消和完成事件保存，两端事件列表与详情显示；旧事件或缺少网络上下文时为空。它复用事件 JSON 持久化，无需升级数据库 schema，也不回填历史记录。

## 平台边界

Linux adapter 组合 Admin API、WebUI 静态服务、systemd 控制和 Linux 生命周期。macOS adapter 组合 sidecar、原生菜单栏 / 通知和 App 控制。两端页面保持字段、状态语义与主要交互一致，窗口、sheet、通知和系统服务可按平台适配。

Admin 监听与 proxy listener 分离。`/healthz` 只证明 Admin 存活；管理 API 由 HttpOnly Cookie 和 CSRF 保护。代理入站 Token、Admin 密码、上游 API Key 和 macOS control token 互不替代。

## 安全原则

默认监听 loopback；远程 Admin 通过 HTTPS 反代、VPN 或 SSH 隧道访问。`X-Sumpter-*` 归因头仅用于入站统计，转发上游前剥离。源码、示例、日志和测试不得包含真实凭据。
