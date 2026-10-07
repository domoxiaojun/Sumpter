# Codex 流式链路兼容性审计

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
