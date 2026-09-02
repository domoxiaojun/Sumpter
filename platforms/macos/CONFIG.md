# config.json 说明（schema v6）

两端共用同一份磁盘格式。macOS 读 `~/Library/Application Support/Sumpter/config.json`，Linux 读
`$XDG_CONFIG_HOME/sumpter/config.json`（否则 `~/.config/sumpter`；system 安装为 `/var/lib/sumpter`）。
权限必须是 0600，文件含明文 key。

当前格式是 schema v6；启动时自动迁移 schema v3 / v4 / v5。旧的 `keys.json`（Python 兼容 wire）
**不再读取，也不会被改写**，可以留在原地当 key 的抄写来源。

`platforms/macos/config.example.json` 与 `platforms/linux/config.example.json` 是可直接改的模板。开箱步骤见根目录
[`USAGE.md`](../../USAGE.md)。

## 顶层

| 字段 | 说明 |
|---|---|
| `schemaVersion` | 固定 6，保存时由 App / WebUI / daemon 写入 |
| `listener` | `host` / `port` / `allowedCIDRs` / `authToken`（空 = 不校验入站） |
| `retry` | 转发与重试参数，全局一份 |
| `endpoints` | 扁平 Provider 入口列表；调度按 `priority` 再按数组顺序 |
| `featureRules` | 分流规则；内建规则的 `name` / `match` 由程序归一化，改坏了会被纠回来 |

磁盘上**没有** `pools`、`globalModels` 或 `featureRules[].target.poolID`。这些只在迁移旧文件时读取。

## retry

- `responseTimeoutSeconds`：首响应总截止秒数，`null` = 由客户端决定
- `streamIdleTimeoutSeconds`：流式两次吐字之间的最长间隔，`null` = 允许无限空闲
- `max500Retries`：当前入口收到 HTTP 500 后的额外重试次数；`0` = 不额外重试
- `failoverOn500`：HTTP 500 重试耗尽后是否切换入口；默认 `true`，关闭后在当前入口直接返回 500
- `retryDelaySeconds`：为最终失败响应准备的 `retry_delay` 秒数；是否返回由 `passThroughRetryDelay` 控制
- `passThroughRetryDelay`：是否将 `retry_delay` 与 `Retry-After` 透传给客户端，默认 `true`
- `maxDeferredRounds`：所有可重试故障的最大轮数（历史字段名保留兼容），`0` = 不限轮数
- `maxRetryDurationSeconds`：所有可重试故障的跨轮总上限秒数，`0` = 不限总时长
- `sessionStickyRetries`：同一次请求中，当前粘性调度组遇到非 500 可重试故障后的额外重试次数；全部遇到可重试故障后才访问其它调度组，其它组成功后立即改绑会话。`0` = 首次失败后立即切换；默认 `2`
- `pinnedIPConcurrency`：pinned IP 并发竞速数

可跨轮的 HTTP 状态为 `401/402/403/429/502/503/504/520-527/529/530`；HTTP 500 仅按
`max500Retries` 在当前入口内重试，是否切换入口由 `failoverOn500` 控制，不进入跨轮无限重试。另含首响应前的
超时和连接失败。只有 `maxDeferredRounds` 与 `maxRetryDurationSeconds` **同时为 0** 才是
真正无限重试。轮间按 `0.5s × 1.7` 指数退避，最多 30 秒；数字 `Retry-After` 与退避取较大值，
同样封顶 30 秒。客户端断开会立即取消等待和上游请求。

## 入站协议与入口四态

SourceFormat 只由路径决定：`/v1/messages` 是 `anthropic`，`/v1/chat/completions` 是
`openai`，`/v1/responses` 及 Codex Responses 别名是 `openai-responses`。路径和请求结构
不匹配时返回 `invalid_request`；User-Agent 只用于识别、统计和 Provider Header，不参与路由。

每个 `endpoints[].protocol` 必须显式保存一个值：

- `auto`：自动（三协议），新入口默认；普通请求解析成当前 SourceFormat。
- `anthropic`：固定 Anthropic Messages。
- `openai`：固定 OpenAI Chat Completions。
- `openai-responses`：固定 OpenAI Responses。

`auto` 只代表入口能力模式，实际 TargetFormat 永远是后三种之一。SourceFormat 与 TargetFormat
相同时 Native Adapter 自动保留原始请求/响应字段；不同才进入已注册 Translator。只要有原生候选，
桥接候选就不会混入同一次重试，原生阶段失败也不再切换到桥接阶段。

## endpoints — 入口

| 字段 | 说明 |
|---|---|
| `id` | 唯一标识；**用量页按它聚合统计**，改 id 会让历史断成两截 |
| `baseURL` / `apiKey` | 上游根地址与凭据 |
| `protocol` | 四态：`auto`（默认）/ `anthropic` / `openai` / `openai-responses` |
| `enabled` | `false` 不参与调度 |
| `priority` | 非负整数，数值越小越优先；缺省 / `0` 不落盘，同级保持配置数组顺序 |
| `pinnedIPs` | 填了就优先按这些 IP 直连（TLS SNI 仍是域名） |
| `pinnedIPExclusive` | 只走这些 IP、不回落 DNS；关闭 = IP 与 DNS 一起竞速 |
| `stickyGroup` | 留空时使用入口 ID 作为独立粘性组；填写相同组名的入口共享会话归属，组内线路按配置顺序尝试 |
| `keepAlive` | 入口级出站连接复用；省略或 `false` 不落盘（关闭）。界面新建入口默认打开 |
| `catalog` | 「获取模型」拉回的目录与状态，纯展示，不参与路由；空目录不落盘 |
| `mappings` | 本入口显式承接的客户端模型。空数组 = 不承接任何模型 |

`mappings[]` 每条：`clientPattern` / `upstreamModel`（留空 = 同名）/ `thinking` / `context` /
`failoverTimeoutSeconds`（可选；映射级首响应截止，与全局 `responseTimeoutSeconds` 取较小值）。
同一入口同时命中精确模型名和 `prefix-*` 通配时，精确映射优先；同级规则按配置顺序。

**当前配置承接哪些模型** = 各入口 `mappings.clientPattern` 的并集，不存在单独的池级模型列表。
某个模型只会发给声明了它的入口，不会拉别的入口去当垫背。

## featureRules — 分流规则

`endpointID`（可选）钉住某一个入口：钉了就只走它、并绕过映射筛选（显式点名即强制）；入口被
停用或删除时自动退回入口序列 failover。没有 `poolID`。
`protocol`（可选）是目标协议，只能写 `anthropic` / `openai` / `openai-responses`，不能写
`auto`。Auto 入口可解析为目标协议；固定入口只有与目标匹配时才参与。
`effort`（可选）在规则命中后覆盖客户端的推理等级；省略表示跟随原请求。可选值为
`none` / `auto` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`。
该覆盖只影响实际出站请求，不参与模型映射或会话粘性键。

三条内建规则的 `match.requestKind` 固定为 `websearch` / `webfetch` / `classifier`。
程序会按 Claude Code 独立子请求的完整形状严格识别，不会扫描主会话的全部历史消息。
旧 v3 配置里的 `toolTypePrefix` / `messagesContain` / `systemContains` 内建匹配会在加载时
自动规范化为 `requestKind`；启用状态和 `target` 保持不变。自定义规则仍可使用
`toolTypePrefix` / `systemContains` / `messagesContain` / `modelEquals`，多个条件是 AND 关系。

## 旧 schema v3 / v4 / v5 迁移

迁移前先创建 `config.before-schema-v6-时间.json` 的 0600 原始备份，随后原子写入并复读验证。
失败时恢复旧字节；已经是 v6 的文件不重复迁移或生成备份。

- v3：`listener.inboundDialectPassthrough=true` 会把当时的入口改为 `auto`；为 false 或缺失时
  保留三种固定协议，入口缺少 `protocol` 时按旧默认迁移为 `anthropic`。旧全局字段会先转为入口
  显式映射后删除，旧入口的 `searchDialect` 也会删除。
- v5：仍可能带 `pools` / `globalModels`。加载时把池按原顺序展平为顶层 `endpoints`（原主池在前，
  迁入入口分配更高的连续 `priority`）；入口 ID、地址、Key、映射和顺序保持不变。空 `mappings`
  的入口会先得到一份当时的 `globalModels` 拷贝，然后删除 `pools[].globalModels`。
  `featureRules[].target.poolID` 删除。
- schema v6 若仍携带 `pools` 等已删除字段会被明确拒绝，必须先完成迁移。

Swift 壳为了旧 UI 代码仍可能有计算属性 `pools` / `primaryPool`，它们只是把全部 `endpoints`
看成同一序列，**不写进 config.json**。

## 已经消失的东西

`main_accounts` / `main_domains` / `main_providers` / `backup_accounts` / `cooldown` /
`feature_routes` / `secretRef` / `pythonKind` / `1m` / `m1` / `ip_timeout` / `backup_id` /
`has_tool_type` / `searchDialect` / `pools` / `globalModels` / `target.poolID` /
`listener.inboundDialectPassthrough` —— 这些旧字段连同对应兼容分支一起删除了。
