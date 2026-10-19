# Codex 流式链路兼容性审计

## 2026-10-19 内部请求归因修复

固定参考 Codex main `89461ea6833832b0551ce24ee32205824d5c108d`；0.163.0 的元数据定义通过 tag peeled SHA `83086188ae0614bd882818ff1aa23f3194962012` 的原始文件核对。

- 内部角色统一使用明确线程来源、canonical subagent kind 和兼容 header。Guardian、记忆和审查不再因旧的 `isSubagent` 投影落成普通子代理；不凭 `/root`、prompt 或路径推断项目或身份。
- `historyInitialization`、`analyticsEnabled` 为有界观测字段；`mcpAttribution` 只保存白名单状态、错误原因、来源数量，不保存工具/连接器来源正文。原始请求继续透明转发。
- 每条 Responses WebSocket 在 `streamTrace.websocketTrace.requests` 中记录用途计数、控制操作次数及最近 64 条 create 明细（序号、turn/response ID、generate、终态）。累计计数不受明细淘汰影响；`dropped`、`unobservedFrames`、`unassociatedEvents` 明确表示观测边界。控制帧不产生推理请求；并发或迟到事件无法唯一关联时不猜归属。连接结束后汇总，不作为实时推理进度接口。
- 连接事件的元数据使用最近 create 的请求身份，避免首条 prewarm 污染后续回合。显式 prewarm 与 Guardian 在 101 后、首业务帧前、没有上游尝试且正常 Close/EOF 时归为未使用连接取消；仍保留 EOF 异常证据。RST、异常 Close、畸形/冲突元数据、已发送 `generate:false` 的连接不放宽。
- 统计 API（Linux `/admin/api/runtime/analytics`、macOS `/admin/runtime/analytics`）的 `codexRequests` 将 `prewarm`、`turn`、`compaction`、`memory`、`unknown` 单独计数；它与现有客户端 HTTP/连接数不同，不能相加。旧 WS 缺少逐请求记录时只增加 `unobservedWebsocketConnections`，不补猜正式回合。上下文压缩和持久记忆提取/整理分别显示。
- 在既有 schema/projection 上增加内部用途计数辅助投影，随事件同事务更新和删除，聚合不扫描大 payload。首次启动对兼容库分批读取已有元数据，修正角色并建立计数，保留原始事件和其它字段；旧的不兼容 schema 继续沿用原有阻断规则。辅助初始化使用事务与完成标记，可失败重试。

本轮只做本地源码和合成上游验证；真实安装构建、线上请求归因与 CPA 五分钟长连接验收仍须在部署后确认。

归因修复验证：Rust 1.88 workspace fmt/check、963 项测试、all-target Clippy 通过；1 项既有性能夹具默认忽略，辅助投影丢失后的恢复用例另行通过。Linux/macOS WebSocket 合同 37 / 24 项包含在本次 Rust 结果中。WebUI 183 项测试及生成资源构建通过；macOS App 构建、233 项 XCTest 与 35 项 Swift Testing 通过；docs 12 项、同步及离线链接检查通过。源码和资源尚未提交、发布或安装。

## 2026-10-19 后续治理实施

本阶段沿用下节固定的 Sumpter/CPA/Codex 源码基线，保留第一轮修复；以下为后续变更，历史验证计数不代表本阶段结果。

- Retry-After 上限从 30 秒提高到 300 秒，HTTP、SSE 探针、WebSocket 握手错误共用解析；HTTP 响应头和本地错误提示同步截断。指数退避自身仍最多 30 秒。所有后续尝试和重试等待检查请求总预算，等待无法容纳时返回最后响应；不重写原生 SSE/WS 业务错误帧。
- 增加按 engine 隔离的运行时能力账本。普通目录提供 ID，受限的可选 CPA 目录补充能力；不能扩大模型集合或覆盖明确拒绝。按实际 native-first 路由、上游模型名和入口指纹取交集，未知、刷新失败或入口变化均保守关闭声明。工具搜索、网页搜索、WebSocket、服务等级独立；模板不再决定这四项路由能力。账本不持久化，重启后等待刷新。
- 跨协议对 native-only 控制字段及外部密文返回结构化 `unsupported_field`；只含控制项的输入也可先完成路由，原生透传不依赖其能否归一化为普通消息。拒绝日志保留固定字段路径和协议方向，不写正文或密文。
- Ping/Pong 保留控制帧优先级和空闲续期，排除出业务消息计数。双 adapter 合同覆盖短时间多轮 Ping/Pong 后继续收业务帧、随后无心跳触发配置截止；已有首消息等待、Close、EOF、RST 和 interrupt 后继续合同保持适用。

### CPA/Codex 真实链路验收步骤

1. 记录实际运行的 Sumpter 版本/提交、CPA 版本/提交及启动构建、Codex CLI 或 Desktop 的实际版本；不能用参考 checkout HEAD 代替运行版本。统一 UTC 时间，保留测试窗口和关联请求/会话 ID，不记录凭据或正文。
2. 在获准使用的真实环境中，分别测试首业务帧前等待、连续超过五分钟的 Ping/Pong 活动后再收业务帧、按显式空闲配置停止心跳，以及同连接 interrupt 后继续。对照 Close 正常关闭与 EOF/RST；不重放已发送的业务请求。
3. 把 Sumpter 的 `attemptCount`、`upstreamHost`、`closedBy`、`relayError`、`transportErrorKind`、`lastEventType` 与同窗口 CPA 会话日志、Codex 客户端终态放到同一时间线上。`closedBy=upstream` 本身不足以判定 CPA 根因；需要 CPA 读超时及最后活动时间共同证明。
4. 若 Ping/Pong 正常而 CPA 仍在原五分钟读截止退出，单列外部阻塞任务“CPA Codex WebSocket 会话在成功处理 Ping/Pong 后续期读截止”，包含 raw/persistent 路径、无心跳反例和部署后复验。Sumpter 不注入伪造心跳、不放宽本地超时来掩盖、不重放业务请求。

本阶段只运行本地合成上游合同，不调用真实模型，不修改 CPA/Codex。真实运行版本、CPA 同窗口日志和客户端终态尚未收集；五分钟真实链路验收未执行，外部风险仍开放。本地合同通过不能写为线上心跳问题已修复。

本阶段验证：Rust 1.88 的 `./scripts/check.sh rust` 全通过（workspace fmt/check、956 项测试、all-target Clippy；1 项既有性能夹具按默认忽略）。其中双 adapter 的 engine 合同为 Linux 163 项、macOS 169 项，WebSocket 合同为 36 / 23 项，目录合同各 4 项。`./scripts/check.sh docs` 的同步检查和 12 项文档合同、lychee 离线链接检查通过。未打包、签名、安装、发布或部署；无公开配置或数据库 schema 变更。

## 2026-10-19 兼容性跟进

本轮 Sumpter 基线为 `5490419`（0.4.39）；只读参考 CPA `2422c6b0f5fe16495320fa29456493874c1977a0`（v8.0.33）、Codex main `89461ea6833832b0551ce24ee32205824d5c108d`。Codex 正式版 0.163.0 与 main、实际安装构建分别看待。以下结论更新本页历史审计中的外部风险判断，不将历史测试结果作为本轮验证。

- 模板更新不再要求远端包含 `gpt-5.5`；存在时仍优先作为默认模板，否则按 payload 顺序选第一个合法模板。重复 slug、无效窗口和推理默认值仍拒绝更新，保留上一份有效模板。内置模板同步自 models 仓库 `c999ce04e22624772ff538c9d2c861789b9dd357`，其中 `gpt-6.1-sol.context_window` 为 500000；路由聚合仍取保守交集，不等于实际上游授权或容量。
- 未知模型的通用回退及非 Codex 路由清理 Responses Lite、实验工具、动态推理更新和专属加速能力，使用直接工具模式；不会因为回退基础来自支持新功能的模型，就宣称其它模型同样支持。
- 首帧后的本地 WebSocket 错误保留 `type: error`、`error.type/message`，增加有效的 HTTP `status` 和稳定 `error.code`；没有上游 HTTP 响应时客户端状态为 502。有效的退避提示通过 `error.headers.retry-after` 交付，向上取整并沿用现有 30 秒上限；升级前后都遵守 `passThroughRetryDelay`。不会转发其它上游错误头。
- HTTP 200 Responses 流的有限前奏探针增加 `slow_down`、`server_is_overloaded`，读取结构化错误中的 Retry-After。事件与 HTTP 头中的合法提示取最大值，复用现有数字/HTTP-date 校验和时间上限；原有预算、开关和 background 限制保持适用。正文、工具输出或未知事件开始后不重放请求；余额、权限、组织/项目限额错误不增加自动重试。
- Anthropic `max_tokens` 转 Responses 时，流式发送 `response.incomplete`，非流式返回 `status: incomplete`，二者都有 `incomplete_details.reason: max_output_tokens` 和原始 usage。内部桥报告 Incomplete，运行事件按上游未完整响应记账；普通完成仍发送 `response.completed`。
- 工具搜索的 `supports_search_tool` 与网页搜索能力独立解析和聚合；网页搜索声明不能开启延迟工具发现。本轮不扩展目录发现为 CPA 完整路由能力查询，也不自动开启 Claude/Gemini 的高级工具转换。
- 共享原生 WebSocket 合同覆盖 interrupt 后同连接继续，以及加密输出回放、client_metadata、内容元数据、工具增量和 compaction_trigger 的逐帧保真。CPA 已新增 interrupt 并修复迟到中断；这项外部风险不再按旧版本结论处理。

本轮验证：Rust 1.88 workspace fmt/check、938 项测试、all-target Clippy 通过；1 项既有性能夹具默认忽略。随后补充的升级前退避开关合同在 Linux/macOS 各 1 项通过；文档 12 项合同和同步检查、离线链接及差异检查通过。覆盖新错误码及事件退避、额度反例、流式/非流式截断与 usage、模型回退及两端 WebSocket 生命周期。

本轮边界：只修改 Sumpter，不修改 CPA/Codex、生产配置或数据库，不调用真实模型。CPA 当前源码仍有五分钟读取截止且 Ping/Pong 不续期的模式，本轮没有进行五分钟链路实测，不能称为已解决。源码合同通过也不能等同打包、安装或线上采用。

审计窗口：2026-09-07 至 2026-10-07（Asia/Singapore）。参考项目只读；没有调用真实模型、修改线上配置或部署。

## 基线与事件判断

Sumpter 基线为 `19c7644`（v0.4.35）；本地 CPA 为 `a2976eb8a`（v8.0.16），Codex 为 `a9abdeaff1`。审计时远端 main 分别为 CPA `57bde35179`、Codex `e95abcdf49`，通过远端 SHA 和 compare 接口只读核对增量，没有修改参考 checkout。仓库版本与用户实际运行的 CPA/Codex Desktop 构建未作等同。

用户提供的第一条事件处于 `relay`，两侧 101、1 次上游尝试、124 条上游消息：已进入真实上游转发。777121 ms 包含 335040 ms 首消息等待，不能全算上游推理时间。原诊断只有 `upstream_receive_failed`，不能继续区分 EOF、RST、TLS 或协议错误。

第二条事件为 HTTP 200 后客户端侧取消，8 块响应但没有已观察到的终态。不能仅凭此断言 Sumpter 主动断流，也不能据此证明 CPA 成功完成。两条事件的传输、回合不同，不构成同一故障的完整因果链。

## 近月更新对照

| 项目 / 提交 | 行为变化 | 对 Sumpter 的影响 |
| --- | --- | --- |
| CPA `b5ba02c2e`（09-11） | 大负载写入时避免 Pong 饥饿 | 对照排查本项目读写耦合；新增慢读端本地合同 |
| CPA `f702bc1ac`（09-13） | Responses Lite 原生保真 | 同协议请求应原生转发；不复制 CPA 的 provider 翻译层 |
| CPA `42c9680ee` / `7b6fafce1`（09-20/21） | duplex、steering、连接内 prewarm | 保持同一 socket、多条业务帧和未知字段；连接关闭不能代表每轮请求结果 |
| CPA `9e71c20d0` / `d31b61cb8`（10-01/05） | 首负载前断开、session activation 错误传播 | 属于 CPA executor 生命周期，不能直接移植为 Sumpter 重试或重放 |
| CPA `c997cbb58` / `0ea4e1dcb`（10-04/05） | flush 错误可见；Chat finish_reason 后干净 EOF 可完成转换 | 检查实际传输和终态，不把 HTTP 200 或 EOF 单独当成功；Sumpter 的 Chat 转换仍要求 `[DONE]`，本次不扩大修改 |
| Codex `d838c2346d` / `73178e7ca6`（09-24 / 10-06） | idle prewarm 与 Guardian 连接池预热 | 首消息前长时间空闲可以合法，不能新加默认首帧截止 |
| Codex `12de0e395d`（09-26） | 使用 `response.interrupt` 保持续接 | Sumpter 透明转发；参考 CPA duplex 入口未接受该类型，属于外部兼容风险 |
| Codex `ed0cc1a4ab` / `6326163b9a` / `c9253c4977`（09-30 / 10-03/05） | 路由字段提前、增量工具、指令放入 input | 对原生路径不应重排或删字段；跨协议翻译不能假定已支持 Responses Lite |
| Codex `6ba4bf9e64` / `f6cf05af1d`（09-30 / 10-06） | SSE/WS 错误 Retry-After | 真实上游业务错误帧原样保留；Sumpter 本地产生的 WS 错误形状另有适配空间 |

额外远端增量中，CPA 主要更新认证并发、Gemini/Claude 翻译和 usage；Codex 的 `ddabe594e6` 是 exec-server relay，不是 Responses 推理链路。不能根据提交标题中的 WebSocket/relay 就把不同传输的修复直接套用。

## 本次修复与验证边界

- 共享 relay 解耦读写；业务队列容量为 1，控制队列容量为 8。控制 flush/Ping/Pong 优先处理，Close 按业务顺序发送；不保留无限业务 backlog。缓冲耗尽时仍施加背压，不承诺在无限慢读或单帧写入阻塞时无条件实时心跳。
- 初始业务帧也走队列；接收上游错误时先交付已读取的消息，避免最后一帧被 EOF 抢先丢弃。不会重放业务请求或自动切换已建立的连接。
- 空闲截止只复用现有显式配置；加入可选错误类型、最近上游事件、触发截止字段。正常 101 连接结束显式记成功，客户端侧 Close 记取消，异常保留失败。
- 同一组合同通过 Linux/macOS adapter 验证；新旧事件解码、脱敏与两端摘要一起验证。

这些改动解决本地可验证的 relay 和记账问题，不足以把用户两条历史事件的根因确认为 Sumpter、CPA、Codex、Cloudflare 或 Nginx。后续线上验证需要对应运行构建、请求时间和关联日志；参考 CPA 的 interrupt、Sumpter 本地 WS 错误结构及跨协议 Responses Lite 差异保留为独立后续项。
