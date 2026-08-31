# 移植规格:SumpterCore 纯逻辑(源:Swift 原文逐行阅读)

> 第三份梳理报告的替代品——子代理两度失联后由主线直接读原文产出。
> 各节内容均已落实到 Rust 实现并有测试;本文供后续校对与回溯。

## 1. AppConfig decode 特判(Swift `init(from:)` 行为 → Rust `normalized()`)

| 位置 | Swift 行为 | Rust 落点 |
|---|---|---|
| `ModelMapping.upstreamModel` | decode 时过 `ModelName.clean` | normalized() |
| `RouteTarget.model` | decode 时 clean;**非 Optional,恒编码(含空串)** | 字段 String + default;normalized() clean |
| `RouteTarget.endpointID` | init 时 trim、空归 nil | normalized() |
| `RouteTarget.effort` | 缺省 nil=跟随请求；合法值为 none/auto/minimal/low/medium/high/xhigh/max | serde 可选字段；命中规则后作为 PlannedEndpoint 覆盖 |
| `Endpoint.stickyGroup` | trim、空归 nil | normalized() |
| `Endpoint.name` / `FeatureRule.name` | 键缺失回退为各自 id | normalized()(空串也回退,视为等价修正) |
| `Endpoint.catalog` | 非 Optional,空目录 encode 时**不落盘** | Option + 空归 None |
| `RetryPolicy` | decode 即 clamp(轮数/时长 ≥0、并发 ≥1) | normalized() |
| `Endpoint.protocol` | 显式四态：`auto`（默认）/`anthropic`/`openai`/`openai-responses`；`auto` 不是出站协议 | `EndpointProtocolMode`；TargetFormat 仍是三种 `ProviderProtocol` |
| `AppConfig.featureRules` | decode 时 `BuiltInFeatureRules.normalized`:内建三条**前置**并纠 name/match,只保留用户的 enabled/target;缺失补 canonical(停用态);自定义原序在后 | `builtin_rules::normalized`(normalized() 内调用) |
| `AppConfig.endpoints` | 扁平 Provider 入口序列；磁盘无 `pools`。Swift 计算属性 `pools` / `primaryPool` 仅 UI 兼容 | 同 |
| `AppConfig.bootstrap` | 空 bootstrap（零入口、无池级模型规则）；模型必须在入口 `mappings` 中显式声明 | `AppConfig::bootstrap()` |
| retry 两个超时 | encode **显式写 null**(「不设截止」是可见选择) | serialize 不 skip |
| `ModelMapping.failoverTimeoutSeconds` | encodeIfPresent(省略) | skip_serializing_if |
| `ModelMapping.id` | 仅 UI 选中用,不落盘 | Rust 不存在 |

## 2. ModelName(模型名解析)

- `parse`:trim → 剥尾部 `\[[^\]]*\]\s*$`(**只一次**)→ trim → 最后一对括号内 trim+lowercase
  是合法 effort 才剥(none/auto/minimal/low/medium/high/xhigh/max;`ultra` 故意不在列)。
- `ModelPattern.matches`:双方 clean;空 pattern 恒 false;尾 `*` 前缀匹配;否则相等。
  构造时 trim 存储。

## 3. RequestInspector 指纹(逐字符串,不可改动)

- CC identity:`You are Claude Code, Anthropic's official CLI for Claude.`
- websearch:单条 user 消息(strictText)normalized+trim 前缀
  `Perform a web search for the query: `;system 含
  `You are an assistant for performing a web search tool use`;tools 恰 1 个
  (name==web_search、type 前缀 web_search);tool_choice 强制 {type:tool,name:web_search}。
- webfetch:tools 空、单 user、system 含 CC identity;文本前缀 `Web page content:\n---\n`,
  其后仍含 `\n---\n`;结尾二选一:
  `Provide a concise response based on the content above. Include relevant details, code examples, and documentation excerpts as needed.`
  或(含 `Provide a concise response based only on the content above. In your response:`
  且以 `- Never produce or reproduce exact song lyrics.` 结尾)。
- classifier:system 含 `You are a security monitor for autonomous AI coding agents.`;
  最后一条消息是 user 且 strictText;trim 后前缀 `<transcript>\n` 且含 `\n</transcript>`;
  tools 空时:无 stop_sequences = 命中(thinking 阶段);stop_sequences 为数组且含
  `</block>` 或 `</severity>` = 命中(fast 阶段);其他形状不命中。
  tools 非空:恰 1 个 name==classify_result 且强制 tool_choice(旧单阶段兼容)。
- session_title:tools 空、单 user、system(normalized)同时含 `Write the title in ` 与
  `Keep technical terms and code identifiers in their original form.`;文本 `<session>…</session>`。
- `detectedRequestKind`:三检测器**互斥**,恰一命中才返回;requestPurpose:
  isSessionTitle 且无 featureKind → sessionTitle;两者同时像 → standard(不猜)。
- strictText:纯字符串,或非空数组且每块都是 `{type:"text"(缺省 text), text:String}`;
  其他(tool_result、嵌套)→ nil,**不参与内建识别**(只供自定义 messagesContain)。
- 自定义规则条件全 AND 且至少一条:requestKind / toolTypePrefix(有目标前缀工具且
  **无 type 为空的客户端工具**)/ systemContains(大小写不敏感)/
  messagesContain(每条消息渲染文本前 800 字符,嵌套 content 递归、块间空格连接)/
  modelEquals(双方 clean)。

## 4. StickyKey / AffinityHasher

会话粘性不是单一会话摘要，而是按路由结果隔离的复合命名空间：

```text
(session identity source, session_id, pool_id, effective_model, feature_rule_id)
    -> affinity_id -> schedulingGroup
```

- `session identity source` 有两个值：`stable` 或 `content`。有
  `X-Claude-Code-Session-Id` 时取 trim 后的原值并保留大小写；缺少或 trim 后为空时，
  取现有的内容指纹（`system` 前 4000 字符 + `|` + 首条 user 文本前 2000 字符，
  非纯文本按 `pythonStyleJSONString` 渲染，见 §6），并标记为临时 `content` 来源。
- `pool_id`、`effective_model`、`feature_rule_id` 必须来自最终 `RoutePlan`；普通路由的
  `feature_rule_id` 为 `None`。客户端原始模型、上游别名和计划前的池不能代替这些字段。
- `content` 来源的 `session_id` payload 沿用上述内容拼接后计算的 32 位小写 MD5；
  MD5 只用于生成临时内容指纹，不再作为最终归属键。
- `affinity_id` 使用以下规范二进制编码后计算 SHA-256，输出 64 位小写十六进制：原样域前缀
  `kekulv-sticky-v2`（无 NUL、无长度）→ 单字节来源标签（`stable=0x01`、`content=0x02`）→
  依次写入 `session_id`、`pool_id`、`effective_model` 的 `u64` 大端 UTF-8 字节长度及 UTF-8
  字节 → `feature_rule_id` 可选标签（`None=0x00`；`Some=0x01` 后再写 `u64` 大端长度和
  UTF-8 字节）。不得把原始 session ID 写入日志、事件或文件。
- 稳定来源的 `affinity_id` 才能进入持久化归属表；内容指纹只保存在进程内，并受现有
  临时条目回收规则约束。

`session_affinity.json` 的唯一可写格式为：

```json
{
  "version": 2,
  "sessions": {
    "<64位小写SHA-256>": {
      "schedulingGroup": "account-a",
      "updatedAt": 1234567890
    }
  }
}
```

- v2 加载时严格验证摘要、非空 `schedulingGroup` 和有限 `updatedAt`。
- v1 识别为旧命名空间，丢弃全部归属并返回可写空状态；下一次稳定会话写盘时原子替换为 v2。
- 未知版本、无效 JSON 或损坏 v2 返回错误；调用方必须将本次运行标记为不可写，保留原文件。
- 写盘继续原子替换，并在 Unix 上保持 0600。

## 5. 调度器(AccountScheduler / PinnedIPScheduler)

- Provider 候选先按模型映射/启用状态过滤，再按 Endpoint `priority` 升序；同优先级保持
  配置顺序。显式 `stickyGroup` 使用组名，空值使用自身 `endpoint.id` 作为独立组；同组
  入口保持配置顺序，组内优先级取最低值。已有会话归属组优先，归属失效后按上述顺序
  重新选择；不再使用 affinity 哈希旋转，也没有账号级健康/冷却排序。
- 所有 Provider 入口执行上述粘性重排；来自旧多池配置的入口迁移后与其它入口使用同一优先级、粘性和 failover 语义。
- pinned IP:近期成功(按成功时间倒序,不参与 rotation)→ untried(rotation 滚动)
  → cooling(rotation 滚动);`None`(DNS 兜底)恒 untried。
- 仅保留 pinned IP 健康记录与内存兼容粘性表上限 2000；稳定会话归属持久化且不超时。

## 6. JSONValue.pythonStyleJSONString(临时内容指纹兜底渲染)

- string → JSON 引号(不转义斜杠);number → 整值无小数点否则 Double 插值;
  bool → true/false;null → null;array → `[a, b]`(`, ` 分隔);
- object → 键序 = 偏好键(type,text,content,role,id,name,input,tool_use_id 中存在者,按此固定序)
  + 其余字母序;`{k: v, …}` 形式。确定性输出(可移植)。

## 7. OpenAI 桥接(chat/completions)

**请求**(`makeRequest`):system 文本非空 → 首条 system 消息;user/assistant 消息
flattenText(text 块直取、tool_result 递归、非空 `\n` 连接);`stream: true`、
`stream_options`(Swift 漏 CodingKeys 写成 `includeUsage`,【Rust 修正】用规范
`include_usage`);max_tokens/temperature/top_p 从 raw 透传;
reasoning_effort:入参(入口注入)优先,否则模型名后缀。

**响应状态机**(`OpenAIStreamEventBridge`,增量):
- buffer 累积(\r\n→\n),按 `\n\n` 切事件;`data:` 行提取(多行 \n join);
  空/`[DONE]`/decode 失败跳过。
- 首个 chunk:model = chunk.model ?? "unknown",发 message_start
  (id/type=message/role=assistant/model/content:[]/stop_reason:null/usage 0/0)
  + content_block_start(index 0, text block 空)。
- `choices[0].delta.content` 非空 → content_block_delta(text_delta),outputTokens += 1。
- `choices[0].finish_reason` → stopReason 映射:stop→end_turn、length→max_tokens、
  tool_calls→tool_use、content_filter→end_turn、默认 end_turn。
- `usage.completion_tokens` → 覆盖 outputTokens。
- finish():残余 buffer 再解一次;未 started 补 start;
  收尾三连 content_block_stop + message_delta{stop_reason, stop_sequence:null,
  usage{output_tokens}} + message_stop。
- SSE 序列化:`event: {name}\ndata: {canonicalJSON(sortedKeys,不转义斜杠)}\n\n`;
  messageID 形状 `msg_oai_<32hex>`。

## 8. Responses 桥(/v1/responses)

**请求**:system → `instructions`;user → input_text、assistant → output_text
(空文本消息跳过);max_tokens → `max_output_tokens`;effort → `reasoning.effort`;
无 messages 键;`stream: true`。

**状态机**(`ResponsesEventMachine`):事件识别以 data JSON 的 `type` 为准:
- `response.created` / `response.in_progress`:取 response.model,确保 start。
- `response.output_text.delta`:delta 非空 → text_delta,outputTokens += 1。
- `response.completed` / `response.incomplete` / `response.failed`:
  model/usage.output_tokens 覆盖;stopReason:status=="incomplete" 且
  incomplete_details.reason=="max_output_tokens" → max_tokens,否则 end_turn;
  发收尾(finished 置位,不重复)。
- finish():残余解一次、未 start 补齐、**未 finished 兜底补收尾**
  (上游断连时客户端不至于挂流)。

**路径**:chat → base 以 `/v1` 结尾则 `{base}/chat/completions` 否则 `{base}/v1/chat/completions`;
responses 同规则拼 `/responses`。均加 `Accept: text/event-stream`。
SourceFormat 只由入站路径决定，不读取 UA 或 body 形状；路径与结构不匹配返回 `invalid_request`。
入口四态先解析为真实 TargetFormat；仅 SourceFormat 与 TargetFormat 不同时进入 Translator，
非 200 原样 relay/走可重试。
回客户端响应头恒改写 `200 + text/event-stream; charset=utf-8 + Cache-Control: no-cache`。

### 8.1 入站 OpenAI 桥(`bridge_in`)

与上述“Anthropic 请求 → OpenAI 上游”方向相反，`bridge_in` 将 Codex/OpenAI 客户端的 Chat
或 Responses 请求转换成统一 Anthropic Messages 形状，并将 Anthropic SSE 转回原客户端方言。
转换覆盖文本、system/developer、工具声明与选择、tool call/result、reasoning effort、usage、
流式和非流式聚合；`session_id` 粘性及重试不在桥内实现，而由 Engine 统一处理。协议选择权归
RoutePlanner；原生候选存在时不混入桥接候选，无法安全表达的工具、reasoning、引用或未知内容块
必须拒绝，禁止静默丢失。

## 9. ConfigStore / ControlTokenStore

- 路径:`~/Library/Application Support/kekulv/` 下 config.json / runtime.sqlite3 / 旧 stats.json 归档 / proxy.log /
  `.control_token` / `.autostart`;Rust 版新增 `kekulvd.pid`。
- save(Swift 侧保留):pretty+sortedKeys+不转义斜杠;写 `.tmp` + replaceItem;0600;
  保存时强写 schemaVersion=6；v3/v4/v5→v6 结构性迁移前备份
  `config.before-schema-v6-<时间戳>.json`，迁移失败恢复原始字节。
- ControlToken:已存在且非空(trim)复用;否则 16 随机字节 → 32 位小写 hex;0600。
  Rust 版归 kekulvd 生成,Swift 只读。

## 10. ConfigWarnings 与 InputValidation

- ConfigWarnings 全部规则已逐条移植(warnings.rs,文案逐字一致);顺序说明:分组类
  提示按首次出现序(Swift 字典无序),测试断言「包含」。
- InputValidation(URL/入口 ID 字符集/超时文本框/唯一 ID 生成)是 **UI 表单预检,留 Swift 不移植**。
# Codex 回合元数据观测契约（2026-08-20）

macOS sidecar 与 Linux 使用同一 `RuntimeEvent.codexMetadata` camelCase wire 形状。字段白名单包含 thread/turn/parent/root/fork/window、完整 AgentPath `agentName`（如 `/root/worker`）、subagent header/kind、sandbox/review 状态、workspace/tool namespace/compaction 摘要及状态标记；installation ID 只保存短 SHA-256 指纹。

canonical body > flat body > canonical header > direct header > identity fallback；`client_metadata.agent_name` 仅作 flat 兼容投影，canonical 值优先。`agentName` 只表示完整代理路径，不能单独用于判断主/子代理；`collab_spawn` 是 header 证据，不得仅凭 prompt、模型或工具调用猜测子代理。所有值有界；畸形/超限只影响观测，不阻断转发。敏感 header、正文、凭据、routing/turn state/tracing 原值不得保存。旧 stats 缺字段可读写兼容。
