# 客户端身份与请求归因

归因版本 1 在新入站请求生成观测结果。客户端、入口形态、代理角色和请求类型分别记录，Linux 与 macOS 使用同一份服务端结果。规则不参与鉴权、模型选择、检索工具改写、重试、计费或请求计数。

历史记录不重算。`requestKind` / `clientAttribution` 缺失表示“未记录”；新请求的 `unknown` 表示已执行识别但证据不足。客户端名称已识别不代表其内部任务或入口形态也已识别。

## 字段与声明

| 字段 | 用途 |
| --- | --- |
| `clientKind` | 客户端产品，保留现有稳定 wire 值 |
| `clientVariant` | CLI、TUI、桌面、IDE、网页、SDK、服务端等明确入口 |
| `agentRole` / `agentName` | 代理角色与有界名称，不从分叉或 ID 差异推断子代理 |
| `requestKind` | 独立观测类型，不替代出站处理使用的 `requestPurpose` |
| `clientAttribution` | `version`、`evidence`、`conflicts`、`issues`；只记录规则、来源和枚举值，不新增保存提示词正文 |

可通过 `X-Sumpter-Client` 声明下表的稳定客户端值，`X-Sumpter-Client-Variant` 声明入口，`X-Sumpter-Agent-Role` 声明角色，`X-Sumpter-Request-Kind` 声明当前请求类型。声明头名不区分大小写，枚举值归一化；畸形、控制字符、超长及未知枚举值忽略，不阻断转发。所有 `X-Sumpter-*` 都在发往上游前剥离。

请求类型值：`turn`、`prewarm`、`compaction`、`session_title`、`memory`、`review`、`websearch`、`webfetch`、`classifier`、`token_count`、`image_generation`、`image_edit`、`alpha_search`、`realtime`、`unknown`。

身份声明优先于原生产品标记，再由产品 UA 补充。类型优先采用专用接口和原生任务标记，再采用声明，最后采用严格指纹。同级不同值保留冲突并返回未知；较低优先级的矛盾仍记录在依据中。Codex canonical 元数据继续使用自身的 body/header 来源优先级，并报告原有冲突和截断状态。

统计接口的 `requestKind` 查询参数可用于请求分页、分组、facets 和导出；`unrecorded` 选择未记录的新字段，`unknown` 选择新规则未识别的请求。`requestKinds` 返回对应分面与汇总，分组参数支持 `requestKind` / `request_kind`。

## 客户端能力矩阵

下列 UA 是支持的合成兼容样例，不保证产品每个版本、供应商连接或入口都发送该标记。通用 SDK / 浏览器 UA 不推测客户端；所有 21 种客户端均支持显式声明。没有明确任务信号时请求类型和角色为未知。

| 客户端值 | 产品 UA 样例或原生身份 | 自动入口与请求细分 |
| --- | --- | --- |
| `codex` | `codex_cli_rs/…`、`Codex Desktop/…`、已知 Originator | CLI/TUI/桌面/VS Code 等由明确入口确定；canonical request_kind、thread_source、子代理标记；受限压缩指纹 |
| `claude_code` | `claude-cli/… (external, cli)` | UA entrypoint 区分 CLI、VS Code、SDK、GitHub Action；严格标题、搜索、抓取、分类、压缩指纹 |
| `gemini_cli` | `GeminiCLI/…`、`CloudCodeVSCode/… proxy_client=geminicli` | CLI / VS Code；固定压缩系统指令 |
| `grok_build` | `grok-shell/…`、`grok-pager/…`、`grok-desktop/…`、对应产品头 | CLI / 桌面；压缩阈值和 agent ID 不是当前压缩或子代理证据 |
| `pi` | `pi/…`、`pi (…)`、`pi-coding-agent`、显式 pi Originator | 入口未知；钩子逐请求角色声明、固定压缩系统指令 |
| `cursor` | `Cursor/…`、`cursor-agent/…`、`Cursor-CLI/…` | 后两种为 CLI；其余入口及内部请求未知 |
| `cline` | `Cline/…` | 入口与内部请求依赖明确声明 |
| `roo_code` | `roo-code/…`、`roo_cline/…`、`RooCode/…` | 同上 |
| `continue` | `Continue/…` | 同上 |
| `aider` | `aider/…` | CLI；内部请求依赖明确声明 |
| `open_code` | `OpenCode/…` | 多入口，入口与内部请求依赖明确声明 |
| `goose` | `goose/…` | 同上 |
| `open_web_ui` | `open-webui/…`、`openwebui/…` | 网页；内部请求依赖明确声明 |
| `windsurf` | `Windsurf/…`、`codeium/…` | 入口与内部请求依赖明确声明 |
| `kilo_code` | `kilo-code/…`、`kilocode/…` | 同上 |
| `lite_llm` | `LiteLLM/…` | 服务端；代表观测到的发送方，不能反推网关背后的客户端 |
| `ollama` | `Ollama/…` | 入口与内部请求依赖明确声明 |
| `lm_studio` | `LM-Studio/…`、`LMStudio/…` | 同上 |
| `chatbox` | `Chatbox/…` | 同上 |
| `deepseek_harness` | `deepseek-harness/…` | 入口未知；`x-deepseek-harness-compact: 1` 为压缩，v1 `dsh_session_log.session.origin=subagent` 为子代理 |
| `workbuddy` | `WorkBuddy/…`、`CLI/… WorkBuddy AI/…` | 入口与内部请求依赖明确声明 |

## 指纹来源与限制

- 21 种身份兼容样例及来源版本保存在 [版本化夹具](../tests/fixtures/client_attribution_v1.json)，核心规则位于 [共享观测模块](../crates/sumpter-core/src/client_attribution.rs)。旧 UA 样例沿用既有回归契约，不以宽泛产品子串匹配未知程序。
- Claude Code 辅助请求复用既有严格 inspector 的纯判断，并增加客户端、当前单条请求与指令位置约束。入口参考 [Claude Code Hub 0c508ea](https://github.com/ding113/claude-code-hub/blob/0c508eaac7e1f3c3717eb6a965b466e719f92b1e/src/app/v1/_lib/proxy/client-detector.ts) 的 entrypoint 对照，不采用其访问限制策略。
- pi 压缩系统指令核对本地源码 `1f78cea7a` 的 `packages/coding-agent/src/core/compaction/utils.ts`；Gemini 核对 `b633c0703` 的 `packages/core/src/prompts/snippets.ts`。只读取固定系统指令和直接文本开头，不复制或扫描待压缩的完整历史。
- DeepSeek 核对 [官方 wire 扩展](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/deepseek-llm-api-wire-extensions.md)，文档 blob 为 `e9a270d98e457fdd3aefc4224cc36372c099ab56`。只检查对应版本的会话头，不分析 `events` 推测当前用途。标题与自动审查没有专用用途头时保持未知。
- 固定指纹只按产品、位置和请求结构组合匹配。历史消息、工具结果、引用内容、模型名和 token 数量不能单独决定用途。普通生成接口也不证明请求来自主代理。
- 包装器只声明产品身份。pi 的可选 `requestSource.requestKind` 若由扩展提供则逐请求上报，缺失时保持未知；不假设现有 pi SDK 一定提供该字段。不会给整个启动进程设置固定用途或角色。

仅完成源码和合成合同验证时，不能据此声称所有真实客户端、供应商连接、安装包或生产流量均已验收。
