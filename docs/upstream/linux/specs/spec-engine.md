# Linux Rust 规格：传输层与代理引擎

> 同步蓝本：`macos/specs/spec-engine.md`；来源提交与 Linux 有意差异记录在
> `RUST_UPSTREAM.md`。
> Linux 权威差异：daemon 为 standalone；Admin 默认 `127.0.0.1:57879`（CLI/env 可覆盖）、
> 配置目录凭据文件初始化 WebUI 内置登录，API/SSE 使用会话 Cookie + CSRF；proxy 数据面只保留 loopback `/__status`，
> `/__reset-stats`、`/__reload`、`/__notify` 恒为 404；没有通知/Hook/SSE notify。

> **当前运行统计实现（runtime API v1，2026-08-22）**：运行统计唯一持久化文件是
> `<config-dir>/runtime.sqlite3`，由 `kekulv-proxy::runtime_store` 的单连接后台线程批量写入。
> `<config-dir>/stats.json` 只是只读历史归档，新版本不读取、不导入、不写入、不删除。旧
> `/__runtime`、`/__reset-stats` 和 Admin runtime/reset API 均已删除；Admin 统一使用
> `/admin/api/runtime/summary|events|analytics|reset`，SSE 运行事件名为 `runtime-change`。
> 配置只用 XDG schema v6 `config.json`；自动迁移 schema v3/v4/v5。冲突时以本段和
> `admin-api.md` 为准。

> `GET /admin/api/runtime/analytics` 支持 `range`、`clientKind`、`project`、`sessionID`
> （以及 `client_kind`、`session_id` 兼容别名）筛选。响应包含 `tokenUsage`、`projects`、
> `sessions` 和 `facets`；客户端请求按筛选条件匹配，关联 upstream 事件按 `requestID`
> 跟随。Token 原始 usage 与协议去重口径见 §5 的统计规则。

> 本文是 Rust 重写的**行为对照蓝本**,由逐行阅读 Swift 源码产出:
> `SumpterTransport/{LocalHTTPServer,TransportTypes}.swift`、`SumpterProxy/{ProxyEngine,ProxyHealth}.swift`
> 及其引用的 SumpterCore 支撑类型、`SumpterProxyTests/` 全部测试。
> 「照抄」为默认;标注【Rust 修正】处是明确决定不照抄的点。
> 正文中的 Swift 行为用于对照，不覆盖顶部 Linux 差异、§6-7 与 §9；本文也不表示本轮已经
> 在 Linux 编译或运行测试，当前验证边界以 `README.md` 为准。

## 1. 入站服务(Swift: LocalHTTPServer)

**bind/listen**
- `ListenerConfig`:默认 `host="127.0.0.1"`,`port=57878`,`allowedCIDRs=[]`,`authToken=""`。
- host trim 后:空串、`"0.0.0.0"`、`"::"` = 监听全部接口;其余绑定指定地址。SO_REUSEADDR。
- 入站无 TLS。端口须能装进 u16。
- 配置变更仅当 host/port 变化才重绑监听(不打断进行中连接)。

**HTTP/1.1 支持面(Swift 版行为)**
- 一连接一请求、无 keep-alive、响应恒 `Connection: close`;不支持入站 chunked 请求体;
  method 不校验;header key 解析后小写。
- 流式响应:HTTP chunked 帧(hex 长度 + CRLF + payload + CRLF,`0\r\n\r\n` 收尾),
  强写 `Transfer-Encoding: chunked`,强删调用者的 Content-Length。
- 客户端断开监测:handler 运行期间并行监视连接,断开即取消 handler 任务树。
  【Rust 对应】axum/hyper 天然 keep-alive——语义要保住的是:**连接/请求体 drop → 立即取消
  上游请求与重试循环**(记 499)。keep-alive 本身可以保留(是改进,不是回归)。
- 解析/handler 兜底:`400 {"error":"bad_request","message":"<err>"}`。

**入站端点清单**(path 先剥 `?query`)

| 方法 | 路径 | 鉴权 | 作用 |
|---|---|---|---|
| 任意 | `/v1/messages` | CIDR + 入站 authToken | Anthropic SourceFormat；Native Adapter 或 Translator |
| 任意 | `/v1/messages/count_tokens`、`/messages/count_tokens` | 同上 | Claude Count Tokens 独立 Native Adapter |
| 任意 | `/v1/chat/completions`、`/chat/completions` | 同上 | OpenAI Chat SourceFormat；Native Adapter 或 Translator |
| 任意 | `/v1/responses`、`/responses`、`/backend-api/codex/responses` | 同上 | OpenAI Responses SourceFormat；Native Adapter 或 Translator |
| 任意 | `/v1/responses/compact`、`/responses/compact`、`/backend-api/codex/responses/compact` | 同上 | Responses Compact 独立 Native Adapter |
| 任意 | `/v1/completions`、`/completions` | 同上 | Legacy Completions 独立 Native Adapter |
| 任意 | `/v1/images/generations`、`/images/generations`、`/backend-api/codex/images/generations` | 同上 | OpenAI / Grok 图片生成独立 Adapter |
| 任意 | `/v1/images/edits`、`/images/edits`、`/backend-api/codex/images/edits` | 同上 | JSON / multipart 图片编辑独立 Adapter |
| 任意 | `/v1/alpha/search`、`/alpha/search`、`/backend-api/codex/alpha/search` | 同上 | Codex Alpha Search 独立 Native Adapter |
| 任意 | `/__status` | CIDR + 真实 loopback peer | 脱敏 ProxyStatus JSON |
| 任意 | `/__runtime` | — | 固定 404；改走 Admin API |
| 任意 | `/__reset-stats` | — | 固定 404；改走 Admin API |
| 任意 | `/__notify` | — | Linux 固定 404；通知功能已移除 |
| 任意 | `/__reload` | — | Linux 固定 404；改走 Admin API/SIGHUP |
| — | 其他 | CIDR | `404 {"error":"not_found","path":...}` |

- 无 models / 健康端点；Responses WebSocket、Realtime / Live、Videos、Files 等不同生命周期
  协议未接入，全部 404。
- 入站 authToken 对全部业务端点生效:`x-api-key` 精确匹配,或 `Authorization: Bearer`
  (bearer 大小写不敏感);空 token = 不校验。
- CIDR 对所有路径最先执行:环回恒放行;remoteAddress 解析不出时仅 allowedCIDRs 为空才放行;
  403 `{"error":"client_forbidden"}`。

**错误 JSON 形状**:普通错误保持扁平对象；代理最终上游失败在保留旧 `error` 键的同时增加
`requestID`、`failureKind`、`failurePhase`，按场景增加数值 `timeoutMS`/
`upstreamStatusCode` 与 `upstreamRequestID`，响应头同步返回 `X-Sumpter-Request-Id`。
400 `route_planning`/`invalid_request`/`bad_request`(带 message);
401 `inbound_auth_required`、`missing_secret`(带 endpoint_id、endpoint);
403 `client_forbidden`/`loopback_only`;404 `not_found`;
500 `stream_failed`/`reload_failed`;502 `upstream_unavailable`(带 message)。真实上游可重试状态
耗尽仍保留 `upstream_retryable_status`，但最终反馈取**最后一次真实失败**，较早的上游 HTTP
502 不得覆盖随后发生的 DNS/TCP/TLS 错误。
空响应合成 Anthropic 风格 `{"type":"error","error":{"type":"api_error","message":"upstream returned empty response"}}`;
503 `no_endpoint`(带 pool)/`proxy_stopped`/`no_reload_handler`;
重试耗尽按最后可重试状态码回 `{"error":"upstream_retryable_status"}`。

## 2. 出站客户端(Swift: PinnedHTTPSClient)

- 目标:`host = pinnedIP ?? 域名`,`port = baseURL.port ?? 443`,`serverName(SNI+证书校验)= 域名`,
  `Host 头 = 域名`(非 80/443 带 `:port`)。**TCP 连 IP、SNI/验证用域名。**
- pinned IP 的轮选不在传输层——引擎逐个给单 IP。
- TLS:仅设 SNI,其余系统默认(版本不锁、系统信任链)。**恒 TLS**,无明文出站。
- **连接不复用**:每次新建连接,`Connection: close`,结束即断。取消表现为 connectionFailed
  (非 CancellationError)——赛跑输家判定依赖此。
  【Rust 对应】reqwest 关连接池(`pool_max_idle_per_host(0)`)或每 endpoint 独立 Client;
  pinned IP 用自定义 `Resolve`;关自动解压(不启用 gzip/brotli 特性)。
- 请求序列化:强写 `Host`、`Content-Length`;缺省补 `Accept-Encoding: identity`、`Connection: close`。
- 流式读取:阶段一(responseTimeout 约束)connect→写→读响应头;之后 body 每次读单独受
  `streamIdleTimeoutSeconds` 约束。上游 chunked → 解帧后 payload 逐块回调(入站侧重新封帧);
  流结束 = 对端关闭(不看 Content-Length)。
- 非流式:单一超时包住 connect+写+读完整响应;对端提前关容忍不完整 body 再 parse。

**超时归属**(与记忆「responseTimeout 只管首字节」一致)

| 阶段 | 控制字段 |
|---|---|
| 连接建立 | 无独立超时,并入下行 |
| 首字节/响应头(流式) | `min(retry.responseTimeoutSeconds, mapping.failoverTimeoutSeconds)`,皆 nil 不限 |
| 流式块间空闲 | `retry.streamIdleTimeoutSeconds`(nil 不限) |
| 非流式整响应 | 同上 min 值(此路径是总闸) |
| 可重试故障跨轮总时长 | `retry.maxRetryDurationSeconds`(引擎层) |

**没有任何硬编码的请求超时**。引擎仅保留 pinned IP 候选的健康记录，不会掐断进行中的请求。稳定会话归属
不按时间过期；长思考(分类器、深度推理)只受
上表三项配置约束;若被 15/30 秒掐断,查的是映射级 `failoverTimeoutSeconds`,不是引擎常量。
测试:`engine.rs::no_hardcoded_request_timeout_when_config_says_none`。

## 3. 引擎请求生命周期(Swift: ProxyEngine actor)

主路径(/v1/messages 流式)逐步:
1. CIDR → 2. 入站 auth(失败 401 inbound_auth_required)。
   **请求体在这两步之前不读取**:两步只看 remote 与 header,被拒请求的正文一个字节都不进内存
   (错误 token + 大 body 不会先占满 `MAX_BODY_BYTES`)。代价是 401/403 事件的归因只含 header
   部分(`CodexMetadata::from_request(&headers, None)`),正文派生字段等 `to_bytes` 之后再补。
   测试:`engine.rs::ingress_preflight_never_polls_rejected_or_bodyless_request_bodies`。
3. 由路径确定 SourceFormat：Messages=`anthropic`、Chat=`openai`、Responses 及 Codex 别名=
   `openai-responses`。不从 UA 或 body 形状猜协议；路径与结构不匹配返回 400 invalid_request。
   再解析 body(JSON object;提取 model/system/messages/tools/raw)。
   `clientModel = clean(model)`:剥尾部 `[...]` 与**合法** `(effort)` 后缀
   (none/auto/minimal/low/medium/high/xhigh/max;未知括号保留)。
4. `requestPurpose` 指纹打标(standard/session_title/websearch/webfetch/classifier),
   严格互斥判定,**只打标不路由**。
5. 分流规则:第一条 enabled 且命中。条件 AND(requestKind 内建严格检测器/toolTypePrefix/
   systemContains/messagesContain(每条消息前 800 字符,大小写不敏感)/modelEquals,至少一个)。
   命中后:`target.endpointID` 非空 → 只留该入口且**跳过映射筛选**;该入口停用/删除 → 降级整池。
   旧配置的 `target.poolID` 在加载时统一改为 `primary`；目标入口或整池没有可用入口时报错。
   `target.model` 换生效模型；`target.protocol`（内部 `protocolOverride`）只能指定三种真实
   TargetFormat，不允许 `auto`。
6. 选池:归一化后只有统一 `primary` Provider 池；池或入口声明该模型即命中，否则
   400 noPoolForModel。
7. plannedEndpoints:enabled;入口 mappings 必须命中，所有入口均支持 `prefix-*`
   通配。池级没有模型规则；每个入口的映射都是显式承接范围。得出 upstreamModel(空=同名)、thinking(缺省
   adaptive)、context(缺省 standard = 透传)、failoverTimeoutSeconds（所有映射均生效，与全局
   首响应截止取较小值）。随后解析入口四态协议：Auto 无规则目标时取 SourceFormat，有目标时取
   规则协议；固定入口只能取自身协议，若规则目标不匹配则剔除。先选 TargetFormat==SourceFormat
   的原生候选，存在时完全丢弃桥接候选；没有原生候选时才保留安全 Translator 候选。最终为空 →
   同一入口映射同时命中时，精确模型名优先于 `prefix-*` 通配；最终为空 → 400
   `NoCompatibleProtocol` 或 noEnabledEndpoint。阶段选定后，原生失败不切入桥接阶段。
8. orderedEndpoints:先完成 `RoutePlanner::plan`，再解析会话身份并以最终
   `(source, sessionID, RoutePlan.poolID, RoutePlan.effectiveModel, RoutePlan.featureRuleID)`
   构造 `StickyKey`。规范编码见 spec-core §4，计算得到 64 位 SHA-256 `affinityID`；原始
   session ID 只在本次构造期间存在，不写日志、事件或文件。
   - 有 trim 后非空的 `X-Claude-Code-Session-Id` 时保留原值和大小写，标为稳定来源；否则沿用
     system + 首条 user 内容指纹，标为临时来源。来源标签参与摘要，二者不会碰撞。
   - 入口 >1 时重排。所有入口统一经 `scheduling_group()` 分组：显式
     `stickyGroup` 使用组名，空值使用 `endpoint.id` 自成一组；按入口配置顺序生成去重组列表，
     同组内多个入口保持配置顺序。空分组入口正常参与分流，不再固定追加到尾部。
   - Provider 候选按 Endpoint `priority` 升序，同优先级按配置顺序；同一 `stickyGroup` 内入口
     连续排列，组优先级取组内最低值。已有会话归属组优先，归属失效后按当前优先级重新
     选择；不再用 affinity 哈希计算入口起点，也不做账号健康/冷却排序。
   - 稳定会话在首次出站前写入 `session_affinity.json` v2，仅保存
     `affinityID -> {schedulingGroup, updatedAt}`；原子替换并保持 0600。v1 旧摘要全部丢弃并
     返回可写空状态，下一次稳定会话请求可覆盖为 v2；未知版本、无效 JSON 或损坏 v2 拒绝
     加载并保持本次运行不可写，绝不覆盖原文件。内容指纹归属只存内存并受临时条目回收约束。
   - 明确故障迁移并成功后才按现有 CAS 规则原子更新归属。其它会话产生的全局冷却不能跳过
     本会话的固定入口，并发旧请求不能覆盖已迁移归属；传输失败不会建立账号级冷却。
9. 路由定型即插 in-flight client 事件(statusCode 0 表示尚未收到响应头,不计数)。
10. forward 循环(轮次 round,对每入口):
    - 取消检查(客户端断开刹车)。
    - failover 归因:上一尝试入口 id 存在且 ≠ 当前(换 pinned IP 不算)。
    - openai 系入口 + 请求带 tools → 跳过该入口,记 400 事件(流式路径守卫;
      【Rust 修正】非流式路径同样加上,Swift 版不对称)。
    - secret 缺失 → **立即 401 missing_secret 终止整个请求(不 failover)**。
    - pinned 候选:pinnedIPs;exclusive=false 追加 nil(DNS 兜底)。按 IP 健康重排:
      近期成功倒序最前、未试过按全局 rotation 分散、冷却(60s)沉底。
    - 候选 >1 且 `pinnedIPConcurrency` >1 → **并发赛跑**:分批同发;第一个拿到非可重试
      响应头者独占回写;胜者完成 → 掐掉其余;**输家(connectionFailed)零记账**(无事件、
      不计失败、不打冷却);胜者回写中途失败 → 原样上抛不再 failover;全员可重试/失败 →
      聚合交外层。
    - 串行:onResponse 若状态码可重试 → 抛 retryableStatus(未写客户端字节,可安全换下家);
      否则 accepted:插 in-flight upstream 事件,并**原地回填 client in-flight 事件的入口归属**
      (endpointID/name/host/upstreamModel + pinned/bridge token;收到响应头后 statusCode 保留真实
      HTTP 状态;timestamp 保持请求开始时刻,UI 靠它算流式已持续秒数),再回写响应头(桥接入口重写
      `200 + text/event-stream + Cache-Control: no-cache`;anthropic 直连剥 hop-by-hop 与
      content-length 后原样转)。流结束:桥 flush → finish → 记成功。
    - 异常分类:accepted 后出错 → 上抛(截断流,无 failover);retryableStatus → 记入本轮可重试
      聚合;首响应前 Timeout/ConnectionFailed 同样记入;InvalidResponse 只在本轮 failover，
      不单独触发跨轮。
11. 整轮结束:本轮出现任一明确可重试 HTTP 状态或首响应前 Timeout/ConnectionFailed，且未取消、
    轮数/时长闸允许 → 等待 `max(0.5s × 1.7^(round-1), Retry-After)` 后再来一轮；两者都封顶
    30 秒。多个数字 `Retry-After` 取最大值，负数、日期格式和非法值忽略。中间错误不透传；
    闸停止后按最后一次真实失败返回（HTTP 可重试状态保留原码，传输失败返回 502）。
    `maxDeferredRounds=0 && maxRetryDurationSeconds=0` 才真正无限；sleep/request future 随客户端
    断开一并 drop，不留下后台重试。

**failover 触发面**
- 换下家:状态码 ∈ retryableStatusCodes 默认
  `[401, 402, 403, 429, 502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 529, 530]`
  (`401/402/403` 表示当前入口凭据/额度/权限不可用;`400` 为请求错误,不换入口);任何传输错误;
  非流式「200+空 body+Content-Length:0」合成 502。
  【Rust 修正】Swift 版空响应检查用大写 `Content-Length` 键而解析后是小写,生产路径失配——
  Rust 用大小写不敏感匹配。
- 直接透传:2xx/3xx;不在可重试集的错误码(如 404、500);accepted 之后的任何失败。

**RetryPolicy 语义**(配置仅前 6 个；`deferredStatusCodes` 仅作历史兼容别名，与 retryable 集相同)
- `responseTimeoutSeconds: Option<f64>` — 流式=响应头截止;非流式=整响应截止。
  **编码时显式写 null(不省略)**。与映射级 failoverTimeoutSeconds 取 min。
- `streamIdleTimeoutSeconds: Option<f64>` — 流式块间空闲,null 不限。
- `sessionStickyRetries: i64`(默认 2,下限 0)— 同一次请求中当前粘性调度组首次失败后的额外
  尝试次数；只有该组的 `1 + N` 次尝试都遇到可重试故障才访问其它调度组，其它组成功立即改绑；
  0=首次失败后立即切换。
- `maxDeferredRounds: i64`(默认 0)— 历史字段名；现控制全部可重试故障的最大轮数，0=不限轮。
- `maxRetryDurationSeconds: f64`(默认 0)— 全部可重试故障的墙钟总闸，0=不限时长。
- `pinnedIPConcurrency: i64`(默认 3,下限 1)。
- 只有上述轮数和总时长**同时为 0**才无限；退避为 0.5s、乘数 1.7、封顶 30s，数字
  `Retry-After` 与退避取较大值并同样封顶。

**pinned IP 健康与会话归属记账**
- IP(流式/赛跑路径):连不上 → 冷却 60s;连上(无论 HTTP 状态)→ 记成功清冷却。
  HTTP 4xx/5xx 不动 IP 健康。
- 入口账号不建立健康/冷却状态；200 只按现有 CAS 规则写会话归属，传输失败按当前
  priority/配置顺序继续 failover。
- 会话粘性重试只发生在同一次请求内：当前粘性组先完整尝试，随后最多再完整重试
  `sessionStickyRetries` 次；每个 pass 必须由可重试故障触发，全部耗尽后才访问其它组。
  其它组 200 且 CAS 允许替换时立即改绑；并发旧请求不能覆盖已迁移归属。

**客户端断开**:取消传播撕掉上游连接;client 事件记 **499**(`client_disconnected: ...`),
不计成功也不计失败,不写 lastError。断开特征:取消信号或错误串命中
EPIPE/32、ECONNRESET/54、ENOTCONN/57、"broken pipe"、"connection reset"、"socket is not connected"。

**出站请求构造**
- 入站 header 黑名单(不透传):host、authorization、x-api-key、content-length、content-type、
  connection、keep-alive、proxy-authenticate、proxy-authorization、te、trailer、
  transfer-encoding、upgrade、accept-encoding;其余透传(如 anthropic-version 与 user-agent)。
- 强制写入(先删同名任意大小写,严防双份——DashScope 见 `application/json,application/json`
  直接 500):`Authorization: Bearer <key>` **和** `x-api-key: <key>` 双发、
  `Accept-Encoding: identity`、`Content-Type: application/json`。
- **User-Agent 透传入站**(2026-08-18 起):claude-cli / codex 等客户端各用其真实 UA；入站
  **未带** UA 才回填 `claude-cli/2.1.220 (external, cli)`(无 UA 探针打 DashScope 会 405)。
- anthropic 协议 body:raw 原样,model 换 upstreamModel;客户端模型带合法 `(effort)` 后缀时
  优先注入(none=删 thinking+output_config.effort;auto=thinking adaptive;
  档位=adaptive+output_config.effort),否则按入口 thinking(disabled 删 /
  adaptive 写 `{"type":"adaptive"}` / passthrough 不动)。
- **出站不使用应用层代理**(`outbound.rs` 硬编码 `no_proxy()`，`config.json` 里没有代理开关):
  不读 `HTTP(S)_PROXY`/`ALL_PROXY` 环境变量,也不读系统代理设置。三条理由:①代理由自己解析
  域名,会让 pinned IP 的 `resolve()` 静默失效 —— 两个特性语义互斥;②中间插一层会引入不受控的
  HTTP 版本协商与 header 改写,破坏本节这套指纹;③环境变量隐式且随启动方式变化(systemd 与手动
  启动环境不同),会造成「同一份 config 两种行为」。
  **TUN 模式代理不受此影响也不试图绕过**:那是网络层路由劫持,应用无从感知,流量照样经过代理——
  「全局走代理」这个需求由 TUN 承担。要给单个入口指定代理属于未实现的功能,不要通过把
  `baseURL` 指向本地代理端口来变相实现(会改写 Host 与 URL 语义)。
- 路径 = baseURL.path + 入站路径拼接去重斜杠。
- `anthropic-beta` = **客户端原值(全部透传、按逗号去重)** ∪ `claude-code-20250219,
  interleaved-thinking-2025-05-14,redact-thinking-2026-02-12`
  (+`context-1m-2025-08-07` 若 context=oneMillion —— **强制补;standard 既不补也不剥**,
  客户端自己带 1M 时照样发出;+`effort-2025-11-24` 若带 effort 后缀)。

## 4. Native Adapter 与 Translator 接线点

- `EndpointProtocolMode` 是四态入口配置：`auto`、`anthropic`、`openai`、
  `openai-responses`。`auto` 先解析成真实 TargetFormat，永远不发送给上游；规则目标协议只能是
  三种真实 `ProviderProtocol`。
- `SourceFormat == TargetFormat` 时自动使用 Native Adapter：保留原始请求体和未知字段，只做模型
  改写、effort/thinking/context beta、鉴权、安全 Header 与 Provider 兼容处理；响应正文不改写，
  只清理 Header 并观察协议终止事件。
- `SourceFormat != TargetFormat` 时才按 `(SourceFormat, TargetFormat)` 选择 Translator。Chat 上游
  路径拼 `/v1/chat/completions`，Responses 上游路径拼 `/v1/responses`，Anthropic 拼
  `/v1/messages`。非 200 不翻译，原样 relay 或走可重试。
- Translator 必须先运行请求能力检查器；无法安全表达工具、reasoning、引用或未知内容块时剔除
  候选，禁止静默丢失。响应转换只对已支持的流式/非流式组合生效。

### 4.1 WebSearch 交上游搜索(2026-08-02,Rust 版特性)

原则:**桥只翻译,不代答**——搜索由上游基础设施执行；代理只在请求通过严格的
`RequestPurpose::WebSearch` 指纹后，依据 RoutePlanner 已确定的最终 TargetFormat 选择搜索表达，
绝不让模型闭卷编"搜索结果"。Provider 不再声明独立的 WebSearch 能力字段。

- anthropic：保留原生 `web_search` 工具和相关内容块，不注入 OpenAI 搜索参数。
- openai chat：注入 `web_search_options: {}`；只对严格 WebSearch 用途放宽对应的 tools 守卫，
  其余带 tools 请求的保护面不变。
- openai responses：注入 `tools: [{"type": "web_search"}]` 内建工具；不注入
  `tool_choice`，避免假定不同上游都接受同一种强制语法。
- 用途判定、能力检查与请求构建使用同一个 `RequestPurpose` 和最终 TargetFormat，避免出现
  "候选已放行但没有注入搜索"的中间态。
- 回程映射:
  - responses:`response.output_item.done`(web_search_call)→ 合成
    `server_tool_use`(input 带 query)+ `web_search_tool_result`(content 空)
    两个完成块,文本块索引顺延——形状对齐 ccc 实测(结果以 text+引用为主,
    tool_result content 允许空,CC 生产接受);
  - chat:搜索服务端隐式发生,无独立结果事件;`url_citation` 注解去重收集,
    流尾以「引用:」清单追加进文本(模型正文通常已带链接,注解为补充保障)。
- ccc 实测依据(`macos/todos-bridge-perf-search.md` §0):三协议服务端搜索全通。

### 4.2 Grok 联网能力适配(2026-08-13)

- 判定依据是规则最终选择的逻辑模型名(`RoutePlan.effectiveModel`)经 clean 后以
  `grok-` 开头；不从 ccc 等入口改写后的不透明 upstreamModel 反推。
- 仅作用于严格指纹的 WebSearch/WebFetch，普通主对话、标题和分类器不因 Grok 前缀自动联网。
- WebSearch 按最终 TargetFormat 选择检索能力：
  - anthropic：保留 `web_search` 工具，并把 Claude Code 的命名强制选择
    `{type:"tool",name:"web_search"}` 改为 ccc/Grok 兼容的 `{type:"any"}`；
  - openai chat：普通严格 WebSearch 请求使用 `web_search_options:{}`；需要 Responses 的
    Grok 检索不在固定 Chat 入口上隐式升级；
  - openai responses：注入 `tools:[{"type":"web_search"}]`。
- Grok 检索需要 Responses 时，Auto 入口解析为 `openai-responses`，固定 Responses 入口可参与，
  固定 OpenAI Chat 入口被拒绝；没有兼容入口时返回 `NoCompatibleProtocol`。
- WebFetch 只有正文含 `http://`/`https://` URL 时启用同一三协议适配，用于 X/受限页面补抓；
  无 URL 时仍只总结客户端已抓取正文，避免重复联网改变语义。
- `thread_fetch`、`browse_page`、`web_search`、`x_keyword_search` 等由 ccc/Grok 的服务端
  检索编排内部选择；代理不把它们伪装成客户端工具，否则 Claude Code 会等待本地 tool_result。
- OpenAI Chat/Responses 回程沿用 §4.1 的引用和 `web_search_call` 桥接；Anthropic 原样透传
  上游产生的 `server_tool_use`、`web_search_tool_result` 和带引用文本。

### 4.3 入站 OpenAI 兼容层(2026-08-18;Codex 等 OpenAI 系客户端)

客户端说哪种方言由路径决定，上游格式由 RoutePlanner 解析；Native Adapter 自动启用，不再有
全局开关。

- 入站路径:`/v1/chat/completions`、`/chat/completions` → Chat；`/v1/responses`、
  `/responses`、`/backend-api/codex/responses` → Responses。Chat 路径收到 Responses 结构，或
  Responses 路径收到 Chat 结构，均返回 `invalid_request`。CIDR/入站 auth 与 `/v1/messages` 同规。
- `/v1/responses/compact`、`/responses/compact`、`/backend-api/codex/responses/compact` 是
  Responses unary JSON 操作；只选择 `auto` 或 `openai-responses` 入口。代理从 `model` 构造
  最小路由视图，复用粘性、retry/failover 和事件管线，出站固定
  `/v1/responses/compact`，响应原样回写。
- Translator 请求侧可复用 `kekulv_core::bridge_in` 转为 Anthropic Messages body 后进
  `handle_planned`；Native Adapter 则保留原始 body，不经过 Anthropic 中间格式。两者都共用
  路由、粘性、failover、pinned IP 赛跑、取消和 §3 的可重试跨轮状态机。
- 工具调用完整转换:chat `assistant.tool_calls`→`tool_use`、`role:tool`→`tool_result`；Responses
  `function_call`/`function_call_output` 同理。工具声明、tool_choice、system/developer/instructions、
  reasoning effort 和 token 上限同步换成 Anthropic 形状。转换失败返回 400，并记录
  `inbound_convert_failed: <原因>`。
- 响应侧:统一 Anthropic SSE 尾挂客户端方言桥(仅 200；错误体原样返回)。支持文本、工具参数
  增量、Responses reasoning、stop reason 和 usage；`stream:false` 聚合为单个 JSON。
- 粘性:`session_id` 头作为 `x-claude-code-session-id` 的兜底稳定身份来源。
- UA:按 §3 透传，Codex 客户端出站仍是 Codex UA。

**独立 Native Adapter**：Legacy Completions 只选择 `auto`/`openai`，Claude Count Tokens 只选择
`auto`/`anthropic`，Responses Compact 与 Alpha Search 只选择 `auto`/`openai-responses`。
Images Generations / Edits 维持独立图片 Adapter 和现有模型路由，不把图片能力等同于三种对话
协议。它们继续复用模型池、粘性、鉴权、首响应前 failover、统计和诊断。

- JSON 体仅把顶层 `model` 改成路由后的逻辑模型，其它已知或未知字段保留；Images 未填模型时
  默认 `gpt-image-2`。因此 OpenAI 的 `partial_images` / `output_format` 与 Grok 的
  `aspect_ratio` / `resolution` 等扩展参数无需代理枚举。
- multipart Images Edits 只读取 `model` / `stream` 用于路由；原始 body 与 `Content-Type`
  字节级转发，避免重建 boundary 或二进制内容。
- Alpha Search 与 CPA 一致，出站设置 `Originator: codex_cli_rs`，并移除提交层字段
  `prompt_cache_key`、`prompt_cache_retention`；其它字段保留。
- Images 流使用独立终止方言：`image_generation.completed` / `image_edit.completed` 成功，
  对应 `*.failed` / `error` 失败。Legacy Completions 使用 Chat 的 `[DONE]`。

**Native Adapter** 自动处理三种同协议请求，不由配置开关控制：

- 请求体保留原始字段，只执行路由模型名与必要兼容改写；
- 两侧不挂 Translator，上游响应正文原样回写；
- 路由、粘性、failover、跨轮重试、取消和统计仍走同一管线；
- relay 对**客户端实际可见字节**运行有界、跨 chunk 的 SSE frame 观察器，不改写内容：
  Responses `response.completed`、Chat `[DONE]`、Anthropic `message_stop` 立即按成功完成并结束
  body；`response.incomplete` / `response.failed` / 协议 `error` 保留 HTTP 200，但按结构化失败
  记账。终止事件前客户端 drop 仍为 499；Native 流 EOF 前没有终止事件则为
  `stream_interrupted`，不能误算成功。

## 5. 统计与事件

**RuntimeSnapshot**(内存实时窗口，不是 Admin 持久化 wire):clientRequests / clientSuccesses / clientFailures /
upstreamAttempts / upstreamSuccesses / upstreamFailures / failovers / recentEvents[]。

**Token Usage analytics**：只从 `kind=client` 的完成事件读取 `streamTrace.usage`，同一
请求的 upstream retry 不重复累计；`requestPurpose=token_count` 不算模型用量。原始字段
`inputTokens`、`outputTokens`、`cacheReadInputTokens`、`cacheCreationInputTokens`、
`reasoningTokens` 保留上游语义，并兼容 `totalTokens=input+output`。统计字段为
`uncachedInputTokens`、`processedInputTokens`、`processedTotalTokens`、
`observedRequests`，同时返回 `tokenAccountingSemantics`（`unknown|subset|independent|mixed`）
和 `tokenAccountingQuality`（`unknown|partial|complete|mixed`）。Anthropic 的缓存读写
独立于输入，OpenAI Chat/Responses 的缓存通常是输入子集；未知协议不做猜测。Anthropic
`cache_creation.ephemeral_5m_input_tokens` 与 `ephemeral_1h_input_tokens` 汇入缓存写入。

**Token 过滤维度**：`clientKind` 使用事件已识别的客户端键，`project` 使用脱敏 workspace
项目键，`sessionID` 优先使用客户端事件顶层会话头（Claude Code 的
`x-claude-code-session-id`，兼容 `session_id`/`session-id`），其次使用 Codex metadata
的 `sessionID`、`threadID`；缺失值分别归入
`unrecorded_client`、`unidentified_project`、`unidentified_session`。项目和会话排行只
展示标识与聚合计数，不保存 prompt、响应正文或凭据。

**RuntimeEvent 字段**:id(UUID 串,upsert 键)、timestamp(**Swift JSONEncoder 默认 = Apple
reference date(2001-01-01)秒数 Double!兼容旧 stats.json 必须按此纪元转换**)、
kind("client"|"upstream")、endpointID?、endpointName?、upstreamHost?、
clientModel?、upstreamModel?、statusCode(进行中且尚未收到响应头时为 0;收到响应头后保留真实 HTTP 状态;499=客户端取消)、durationMS、
failover: bool、message?、outcome?(succeeded/failed/cancelled)、
sourceFormat?(入站路径确定的真实协议)、targetFormat?(本次实际出站真实协议)、
routeMode?(native|translated)、
failureKind?(response_timeout/connection_failed/invalid_response/upstream_http_status/
stream_idle_timeout/stream_interrupted/upstream_response_incomplete/upstream_response_failed/
endpoints_exhausted/client_cancelled/client_request_rejected)、
failurePhase?(before_response/response_headers/response_stream)、failureDetail?、
phase?("inFlight"/"completed"；新事件必须显式写入，缺省仅兼容旧数据并视为完成)、featureRuleID?、requestID?(一次 client 请求及其
全部 upstream 尝试共享)、upstreamStatusCode?(实际收到的上游状态)、upstreamRequestID?、
timeoutMS?(实际生效的首响应/流空闲阈值)、
requestPurpose?(standard/session_title/websearch/webfetch/classifier/compact/image_generation/
image_edit/alpha_search/token_count,缺省=旧数据)、
clientKind?(claude_code/codex/grok_build/openai_compat/unknown；缺省=旧数据或无客户端事件，
显式 unknown 才表示无法识别的 Anthropic 客户端)、
ttfbMS?(首字节毫秒数,缺省=从未 accepted 或旧数据;口径见 §5.2)。旧事件里的 `poolID`
仍可读取，但只作为内部兼容值，新事件、详情和列表 wire 均不再输出。
`streamTrace`?(响应流/JSON 的脱敏摘要:chunkCount、bytesReceived、maxChunkGapMS、lastChunkAtMS、terminalEvent、
stopReason 以及上游公开的 usage token 计数;不保存 prompt、正文、请求头或价格)。

`client_request_rejected` 仅用于代理在任何真实上游尝试之前拒绝的 client 事件，
包括入站鉴权、请求体解析/转换、路由规划或协议兼容性失败；这类事件固定
`failurePhase=before_response`，并继续用原 `message`/`failureDetail` 保存具体原因，
不得把它误记为连接失败或入口耗尽。

**clientKind 判定**(与 requestPurpose 正交:前者是「谁在发」,后者是「发的什么」):
- 只看入站 `User-Agent` 与入站方言,**不猜请求体内容**。
- UA 以 `claude-cli/` 开头 → `claude_code`;UA 含 `codex`(大小写不敏感)→ `codex`;
  UA 含 `grok-shell` → `grok_build`(包括 `grok-pager/... grok-shell/...` 形态)。
  UA 优先于方言,已识别客户端即使经兼容层入站也保留具体类型。
- UA 认不出时按入站方言兜底:OpenAI 兼容层记 `openai_compat`,Anthropic 入站记
  `unknown`。client 与 upstream 事件都带；规划/鉴权阶段就被拒的请求也带。

**计数口径**(todos「统计 bug 修复」批次固化的规格,必须继承):
- 一次客户端请求恒一条 client 事件;每次上游尝试各一条 upstream 事件。
- in-flight 插入不计数;完成时按同 id 原地更新(位置不动)计数一次。
- 新事件按 `outcome` 计数；旧事件缺字段时才回退到 HTTP 状态。succeeded 清空
  `lastError`(自愈)，failed 计失败，cancelled 不计成败。
- HTTP 状态与最终结果分离：已发出 200 后断流仍保留 `statusCode=200`，但
  `outcome=failed`；client/upstream 都计失败。客户端取消的 upstream 尝试保留真实状态，
  `outcome=cancelled`，不得误计上游失败。
- `failovers` 按「本请求是否换过入口」**最多 +1**(不按尝试次数累加)。
- 赛跑输家零痕迹。

**缓冲**:按 kind 各留最近 200 条(防 upstream 挤光 client);in-flight 优先保留但单独封顶 200。
新事件在前。启动只从 SQLite 加载计数和每类最近 200 条事件；`statusCode=0` 且
`phase=inFlight` 的崩溃残留删除，其余残留按失败/取消归一并产生新的 `changeSeq`。旧
`stats.json` 不参与启动恢复，也不作为失败回退数据源。

**runtime.sqlite3**：WAL、`synchronous=FULL`、`busy_timeout=5000`、2 MiB page cache；
1 秒/64 条/256 KiB 批量提交，pending buffer 上限 4 MiB。数据库故障时内存和 SSE 继续工作，
超过硬上限返回 `runtime_storage_backpressure`，避免无限内存增长或静默丢失。统计事件不会按
条数、天数自动淘汰；设置 `storageLimitBytes` 后会按 SQLite 有效占用自动轮换最旧的已完成请求整组。
只有用户明确执行 reset、会话删除等操作，或容量上限触发轮换时才会删除统计记录；历史统计由用户手动清理后按当前契约重新开始。
`storageLimitBytes` 是 SQLite 有效占用上限，达到后自动轮换最旧的已完成请求整组，不阻断写入；进行中的请求不会删除，传 `null` 关闭上限。

**健康判定**:最近 20 条已完成 client 事件,剔除 cancelled;按 outcome 判定成功(旧事件才
回退状态码);无样本=idle;0 成功=down;
成功率 ≥0.8=healthy;否则 degraded;未运行=stopped。

**失败反馈语义**:
- `response_timeout`:响应头前达到实际阈值；`timeoutMS` 必须写入。配置 200 秒时若事件确为
  此类，阈值显示 200 秒；若更早出现 `connection_failed`，说明 TCP/TLS/对端提前断开，
  200 秒截止尚未触发。
- `upstream_http_status` 只有实际收到上游响应头时成立，并带 `upstreamStatusCode`；本地
  连接失败虽然为兼容仍可向客户端返回 502，但 `upstreamStatusCode=nil`，不得描述成
  “上游返回 502”。
- `stream_idle_timeout`/`stream_interrupted` 发生在响应头后；线上 HTTP 状态已经不可改，
  事件用 `response_stream + outcome=failed` 表达最终失败。
- `upstream_response_incomplete` / `upstream_response_failed` 表示 SSE 已正常传输到明确的协议
  终止事件，但上游声明结果未完整或失败；保留 `statusCode=200`，不得误记成客户端 499。
- 出站 reqwest 错误保留有界 source chain，并先移除 URL，避免 query 中临时签名写入统计。

**Linux 通知边界**：不安装 Claude Hook，不接受通知事件，不产生 `kind=="notify"` 的新事件，
Admin SSE 也没有 notify 类型；proxy `/__notify` 对任何方法、query 和 body 均返回 404。

### 5.1 事件消息词表(Rust 版契约,2026-08-02 起)

`RuntimeEvent.message` 只放**机器可读 token**(或「token: 原因」/「token <参数>」形状),
多 token 用 `"; "` 连接(如 `pinned 1.2.3.4; bridge openai`);中文翻译全部在 UI 层
（WebUI 展示层负责中文翻译）。Linux 不产生 notify 事件。

| token 形状 | 发出点 | 场景 |
|---|---|---|
| `timeout` | TransportError Display | 响应头截止超时 |
| `connection failed: <原因>` | TransportError Display | 连接建立/传输失败 |
| `invalid response: <原因>` | TransportError Display | 响应形状非法 |
| `stream interrupted: <原因>` | CompletionGuard | 已回写头后的断流/吐字超时(client+upstream 事件同带) |
| `client_disconnected*` | CompletionGuard Drop | 客户端取消,与 499 成对 |
| `upstream_retryable_status` | forward 收尾 | 全轮耗尽的最终失败(client 事件) |
| `all endpoints failed` | forward 收尾 | 整轮无可重试也无传输错误(如全部入口被守卫跳过,client 事件) |
| `pinned <ip>` | note_attempt / relay / guard | 该次尝试走 IP 直连(成功、可重试、传输失败都带) |
| `bridge <openai\|openai-responses>` | relay / guard | 协议桥接生效(accepted 2xx) |
| `route_mode native|translated` | relay / guard | 记录解析后的 SourceFormat、TargetFormat 与原生/桥接模式 |
| `deferred_rounds <n>` | guard | 历史 token；任一可重试故障跨轮后记录总轮数 n(>1，仅 client 事件) |
| `unmatched_no_tools` | guard | 形似 CC 内部辅助请求却未命中任何指纹(仅 client 事件),见 §5.2 |
| `inbound_auth_required` | handle_messages | 入站鉴权失败(401 client 事件) |
| `body is not JSON` / `body is not an object` | handle_messages | 请求体解析失败(400 client 事件) |
| `anthropic request shape invalid` | handle_messages | `/v1/messages` 缺少非空 `model` 或数组 `messages`，按路径拒绝且不出站 |
| `inbound_convert_failed: <原因>` | handle_openai_inbound | 入站 OpenAI 兼容请求无法转换(400 client 事件),见 §4.3 |
| planner Display 原串(`no pool accepts model <m>` / `pool not found: <id>` / `no enabled endpoint in pool <id>` / `feature rule not found: <id>`) | handle_messages | 路由规划失败(400 client 事件) |
| `openai_tools_unsupported` | forward | openai 系入口 + tools 被守卫跳过(400 upstream 事件) |

**同步义务**:词表变更必须同时改 ①本表;②`kekulv_core::events::message_tokens` 常量;
③引擎测试 `MESSAGE_TOKEN_PREFIXES` 清单;④Swift `RuntimeEventPresentation` 映射与其测试清单。
两边清单测试互钉,漂移即红。

历史兼容:Swift 时代 stats.json 里的旧串(`connectionFailed`、`invalidResponse`、
`retryableStatus(NNN)`、POSIX 断开标记、桥接跳过的硬编码中文)不再产出,但 UI 保留只读映射。

### 5.2 首字节耗时 `ttfbMS` 与用途失配提示(2026-08-03 起)

**`ttfbMS`(事件可选字段)**:到上游响应头 accepted 为止的毫秒数。加它的理由是
`durationMS` 对流式请求没有诊断力——它记的是「吐完最后一个字」,所以「上游卡住 80s」
和「正常长输出 80s」在事件里完全同形。

- **口径按事件类型不同**(看错会把「换过一次入口」误读成「这个入口很慢」):
  - client 事件 = 客户端视角的总等待,含 failover、退避与跨轮重跑的全部时间;
  - upstream 事件 = 该次尝试自身的响应头延迟。
- 非可重试响应的埋点在 `relay()`——进到 relay 即代表响应头已到。可重试 HTTP
  状态在 `note_attempt()` 收到响应头时直接记录。client 侧由
  `CompletionGuard::record_streaming_started` 钉住,之后不再变化。
- **None 的含义**:从未收到响应头(规划/鉴权失败、全轮耗尽、响应头前失败的尝试)
  或事件来自升级前的 stats.json。缺字段时序列化省略该键。
- UI:耗时列显示「首字节 → 总时长」,无 `ttfbMS` 时回退到只显示总时长;
  超过 15s(`RuntimeEventPresentation.slowTTFBThresholdMS`,经验阈值)染橙。
  **纯展示,不参与任何超时或失败判定**——超时归属仍由不变量 #1 管。

**`unmatched_no_tools`**:`matches_session_title` 等指纹钉死在 CC 2.1.x 的请求形状,
CC 一升级改文案,内部请求就会**静默退化**成「普通请求」,没有任何信号。该 token 是唯一
的可见提示。判据(`inspector::is_unmatched_no_tools`)逐条排除已知的正常形状:

- 无 tools + 仅单条 user 消息(CC 主对话恒定带 tools 且多轮);
- system 非空(排除裸 curl / 简易客户端);
- system 不含 CC 身份标识(主对话的 system 必含,内部辅助请求各有专用 system);
- 用途判为 `standard`(已识别的用途在「用途」列已经标好,不重复提示)。

剩下的就是「带专用 system 的单轮无工具请求,却谁也没匹配上」。误报会变成常驻噪音,
所以判据面在 `crates/kekulv-core/tests/routing.rs` 里逐条钉死。

## 6. Linux 本地控制机制

配置目录遵循 XDG：`$XDG_CONFIG_HOME/kekulv`，否则 `~/.config/kekulv`。

- `config.json`：显式 schema v6、0600、原子写；自动迁移 schema v3/v4/v5；仅有旧 `keys.json`
  时拒启且不迁移。
- 旧 `stats.json`：仅作为只读历史归档；新版本不解析、不导入、不覆盖、不删除。
- `diagnostic_capture.json`：仅在用户手动开启完整捕获后保存未脱敏 Headers、Body 和 Chunk，
  使用 0600 原子写；默认容量 512 MiB。daemon 启动恢复记录但保持停止，文件损坏时本次运行
  拒绝覆盖。Admin 列表/刷新只返回轻量索引，正文只通过 request ID 详情接口按需读取。
- proxy 控制写操作只存在于独立 Admin API；数据面 `/__reset-stats`、
  `/__reload`、`/__notify` 全部 404。
- Admin 默认 `127.0.0.1:57879`；host/port 与高级密码路径仅 CLI/env 可覆盖，不进入 config.json。
  默认读取配置目录 `admin-password`，静态登录壳公开，API/SSE 使用会话 Cookie + CSRF 并支持
  HTTPS 反代；Admin 登录由 daemon 内置 WebUI 会话处理。
- user scope 的自启动只允许通过 Admin 操作固定 systemd user unit `kekulv.service`；system
  scope 只读显示固定 system unit 状态，必须由管理员执行 `sudo systemctl enable|disable`，daemon
  不得因 WebUI 而以 root 运行。

## 7. Linux Admin API

唯一契约见 `specs/admin-api.md`：双 listener、内置登录会话、generation-safe
完整 v3 配置 PUT、secret 独立更新、proxy 启停、SSE、诊断和固定 systemd user/system unit。
旧 Swift/KeysConfig 的细粒度 CRUD wire 不再适用。

## 8. 行为测试清单(Rust 侧逐条重建,名称可 snake_case 化)

SumpterProxyTests 核心清单(规格级别,重建时逐条对照):
1. status 端点回引擎状态(running/port/pools)
2. 真实监听下 HTTP 打通 status
3. 空 apiKey 入口按 Linux Rust 语义无鉴权转发，且不泄漏入站鉴权头
4. CIDR 外 403 / CIDR 内无 token 401 / Bearer 正确 200
5. 明文配置 authToken 生效(x-api-key 匹配)
6. 首入口 503 → 次入口 200;attempts=2、failovers=1;事件 failover 标记正确
7. 200+空 body+CL:0 → 记 502 上游失败
8. websearch 规则命中回填 featureRuleID/poolID/实际入口
9. 主对话夹带 webfetch 文案不误触发;真 webfetch/classifier 各自命中
10. 普通请求 featureRuleID=nil、purpose=standard
11. 标题生成指纹记 session_title 不走分流;(high) 后缀剥离
12. openai 入口桥接 chat/completions + 模型改写 + SSE 桥回
13. openai-responses 入口桥接(instructions/input,无 messages)
14. adaptive+1M 映射 → body 注入 thinking、beta 含 context-1m
15. 出站 Content-Type 恰一份;Authorization 与 x-api-key 双发;anthropic-version 透传
16. 入站 UA 原样透传；缺 UA 时才回填 CC 指纹 UA
17. proxy `/__reload` 恒 404；Admin reload 经会话 Cookie + CSRF + JSON，成功返回 generation/warnings/proxy
18. proxy `/__notify` 对任意输入恒 404，SSE 不出现 notify
19. 统计落盘后新引擎重载;缺 requestPurpose 旧事件正常往返
20. 流式 relay:chunked 头、SSE 直通、0\r\n\r\n 收尾;事件 upsert 各一条
21. 响应头一到即见 in-flight 事件(不计数),完成原地更新计数一次;
    accepted 时 client in-flight 事件回填入口归属(timestamp/计数不变)
22. 加载丢弃 status 0 in-flight、其余归一为完成
23. OpenAI SSE 分片增量桥接
24. 头未回写前 503 可换入口,客户端只见 200
25. Admin 静态登录壳公开；API/SSE 会话 401、写请求 CSRF、登录/退出/改凭据与旧会话撤销；数据面无管理写旁路
26. pinned 独占不落 DNS,耗尽 503 透传
27. 503 跨轮重试到 200
28. 不限轮 + 0.02s 总闸快速刹停
29. 全局与映射超时 min 四组合(45/15→15、45/nil→45、nil/15→15、nil/nil→nil)
30. 统一 Provider 按 priority 与配置序稳定排列，不做哈希旋转
31. 流式同样跨轮重试,503 不透传
32. 取消立即停重试,client 记 499
33. 3 IP 赛跑挑中唯一活 IP
34. 全死 IP 快速失败后跨入口 failover
35. 账号粘性:首次遍历成功后第二次直接粘住;稳定会话重启后不漂移
36. 旧多池 ID 归一为同一 pool 命名空间；effectiveModel/featureRuleID 仍各自保存归属
37. 空 stickyGroup 以 endpoint.id 自成组并参与 Provider 分流;显式组与空组混合时同样可成为首选
38. 事件按 kind 各留 200,client 不被挤,in-flight 豁免
39. 展示排序:kind 过滤+时间倒序+同戳稳定
40. 赛跑输家不计失败不留事件不打冷却
41. 赛跑成功也记账号粘性;同一显式组的多线路保持组内配置顺序
42. 双入口全 429 跨轮:attempts>2 但 failovers=1
43. in-flight 也按 perKind 封顶且与完成事件配额独立
44. 全 retryable 状态、401+连接失败混合轮和 Codex Responses 混合轮均可恢复
45. 0/0 无限模式有退避；task abort 与真实 TCP 客户端断开均记 499 且停止下一轮
46. 指数退避序列、多个数字 Retry-After 取最大值、非法值忽略和 30 秒封顶
47. 三种协议的 Native Adapter 与安全 Translator、流式/非流式、工具调用和 session_id 粘性

ProxyHealthEvaluator(8 条):stopped/idle/healthy(90%)/degraded(50%)/down/
499 排除口径(全取消=idle、取消不拉成功率)/窗口只看最近 N 条/lastSuccess 可指窗口外。

Live 冒烟(对应 Swift LiveClassifierQwenSmokeTests):环境变量 `KEKULV_LIVE_SMOKE=1` 才跑;
读真实用户 config;发完整 CC 分类器指纹请求(billing-header block、transcript、stop_sequences);
断言出站带 CC UA、无双 MIME。测试基建对标:Fake 客户端(响应队列+流分片+超时快照)、
按 host 定结局、长流门控、空闲端口分配。

## 9. 与 Swift 老版的行为差异(Rust 版有意变更)

1. **明文 HTTP 上游**:按 `baseURL` 的 scheme 决定 —— `http://` 走明文(默认端口 80),
   `https://` 走 TLS(默认 443)。Swift 老版恒 TLS(http 也被按 TLS 连)。
   用途:Ollama / vLLM / LM Studio 等本地服务、内网无 TLS 中转。
   pinned IP 对明文同样生效(只覆盖 DNS,不影响 scheme)。
2. **空 apiKey = 无鉴权上游**:照常转发,**不发** `Authorization` 与 `x-api-key`
   (入站的同名头已被黑名单剥除,不会泄漏客户端 key)。Swift 老版是立即
   `401 missing_secret` 终止整个请求。ConfigWarnings 仍提示
   「未配置 API Key(将以无鉴权方式转发)」,因为多数情况仍是漏填。
3. 空响应检查 header 大小写失配 → 修正为大小写不敏感。
4. 非流式路径缺 openai+tools 守卫 → 统一补上。
5. `/__status` 不再回显 `listener.authToken` 明文(改回 `***`)。
6. chat 桥的 `stream_options` 键修正为规范的 `include_usage`(Swift 版漏 CodingKeys)。
7. **事件消息词表化**(§5.1,2026-08-02):message 全部收敛为机器可读 token,超出 Swift 行为面的部分:
   - 规划/鉴权/解析失败**记 client 事件**(Swift 记;Rust 首版漏,此为回归修复);
   - client 成功事件带 `pinned`/`bridge`/`deferred_rounds` 信息 token(Swift 成功侧 message 恒空);
   - 传输失败消息带 `pinned <ip>` 前缀(Swift 只记错误串,丢失哪个 IP 失败);
   - 流中断的 upstream 事件消息与 client 同带 `stream interrupted: ` 前缀(Swift upstream 侧是裸错误串);
   - openai+tools 跳过消息由硬编码中文改为 `openai_tools_unsupported` token。
8. **chat 桥透传 `stop_sequences`**(2026-08-02):Anthropic `stop_sequences` → OpenAI `stop`
   (剔空串、限前 4 条);声明过 stop 的请求上游 `finish_reason=stop` 回映射
   `stop_reason: "stop_sequence"`(chat API 不区分自然结束与 stop 命中,按分类器
   stage-1 设计意图取 stop 命中;命中哪条不可知,`stop_sequence` 字段恒 null)。
   Swift 版桥不透传 stop(分类器 stage-1 过桥生成到 max_tokens 才停)。
   **Responses API 无 stop 参数**,该协议记为限制。
9. **跨轮重试统一**(2026-08-18):旧 `only_deferred` 会让同一轮混入 401、连接失败或超时时
   提前结束，0/0 又会无退避 busy loop。现全部 retryable HTTP 状态与首响应前
   Timeout/ConnectionFailed 共用可取消跨轮状态机；指数退避和数字 Retry-After 均封顶 30s。
   `maxDeferredRounds`、`deferred_rounds` 名称仅为磁盘/事件兼容。测试覆盖纯 502、混合 HTTP+
   网络故障、Codex Responses、无限模式节流与真实 TCP 断开。
10. 桥接 `message_start.model`:构造时以上游模型名作种子(上游流带 model 仍覆盖);
   Responses 流早期事件常不带 model,Swift/Rust 老实现恒显示 "unknown"。
11. 【实验,2026-08-02】入口级 `keepAlive: true` 开启出站连接复用(小池 2 条 +
    90s 空闲回收),省去每请求 TCP+TLS 握手(远程中转实测 ~100ms,分类器高频路径
    受益最大);新入口编辑器默认开启，显式 `false` 仍维持每请求新建连接的手写栈
    行为面，字段不落盘，老配置/老壳零感知。真实 CC 客户端本身复用连接，开池更像
    CC。按入口逐个验证中转兼容后可关闭；UI 表单支持手动切换。

## 10. 移植警示汇总

1. keep-alive 差异:hyper 多路复用下「客户端断开→取消上游」语义必须重新验证(请求体/连接 drop)。
2. runtime.sqlite3 的 timestamp 使用 Apple 纪元(2001-01-01)秒数；旧 stats.json 仅由兼容模型
   在离线/只读场景解码，不参与运行统计启动或写入。
3. 跨轮重试必须有可取消指数退避；0/0 才无限，客户端断开必须同时刹住 sleep 与上游请求。
4. 出站 scheme 决定明文 HTTP 或 TLS；HTTPS 的 SNI=域名。默认不复用连接，双发鉴权头，
   强制 CC UA + identity 编码；reqwest 关压缩、默认关连接池，pinned IP 使用自定义 Resolve。
5. 两处 Swift 不对称(空响应检查大小写失配、非流式缺 tools 守卫)——Rust 统一修正。
6. retryableStatusCodes 含 401/402/403,400 原样返回；Linux 空 apiKey 按无鉴权上游转发，不产生
   Swift 老版的 `missing_secret` 立即失败。
7. 稳定会话按 pool/effectiveModel/featureRuleID 隔离并以 SHA-256 affinity v2 持久化；
   内容指纹仅内存。空 stickyGroup 以 endpoint.id 自成组并参与 Provider 分流；仅 pinned IP 健康
   仍为内存态，重启清零。priority/CAS 语义固定，入口不再有 60s 健康冷却。
# Codex 回合元数据观测契约（2026-08-20）

`RuntimeEvent.codexMetadata` 是可选、安全有界的观测投影，不改变转发请求。解析来源按优先级为 canonical `client_metadata["x-codex-turn-metadata"]`、flat `client_metadata`、canonical header、直接 headers、identity fallback；高优先级字段覆盖低优先级字段，冲突只记录字段名和被忽略来源。

允许展示的字段包括 installation/session/thread/turn/window 标识、来自 `AgentPath` 的有界 `agentName`、request kind、fork/parent/root 关系、`x-openai-subagent` 与 `subagentKind`、thread source、sandbox、review/Node REPL 状态、turn 时间、workspace 摘要、tool namespace/function 摘要、compaction、受约束 extras、来源/冲突/截断状态。`agentName` 按 256 字节标签保存，不当作文件系统路径，也不单独作为主/子代理判据；安装标识只保存短 SHA-256 指纹；工作区路径与 remote URL 会折叠和去凭据。

`collab_spawn` 仅作为 `x-openai-subagent` 证据；`isSubagent` 只能由该 header、canonical subagent kind/thread source 判定，不能由 prompt、模型、耗时或工具名推断。parent thread 优先使用 authoritative `x-codex-parent-thread-id`，仅在已有子代理证据且存在 fork 标识时标记 inferred。

JSON、字符串、数组、workspace、namespace、function 均有数量/长度上限；畸形或超限 metadata 设置状态并继续原样转发。Authorization、Cookie、API key、正文、routing hint、turn state 和 tracing 原值不得进入事件。旧 stats 缺少该字段时必须正常解码，重新编码不得凭空增加 `codexMetadata`。

后台线程归因：事件投影同时写入 `codexThreadClass` 与 `attributionScope`。`ambient_*`、`system`、
`title`、`automation`、`guardian_review`、`memory_consolidation`、`subagent` 等无可信
workspace/client 项目声明的线程归入 `internal_feature`；普通项目维度排除这些事件，但顶层
总数保留并单独显示后台功能统计。线程名、installationID、threadID、agentName、parentThreadID
均不是项目证据，不能用来猜项目。
