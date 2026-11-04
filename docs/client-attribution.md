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

## 包装器与安装维护

统一包装器和旧版 Claude/Grok Shell 入口均在发送前去除 Git remote 的用户信息、query 与 fragment。包装器不修改客户端持久配置。Codex 的配置表采用合并语义，没有删除继承 Header 键的 CLI 操作，因此包装器将已有私有声明值置空、禁用对应环境变量绑定，再填写当前项目字段；空值不会成为服务端归因证据。用户的认证头和其他自定义 Header 保留。现有 Codex CLI 不能安全定位含点号的私有 Header 键，遇到这类配置会明确报错，需先移除该旧声明。

Codex 登录、登出、MCP 管理等非推理命令直接透传。Gemini 兼容 `-r5` 等紧邻短参数，注入的会话参数放在 `--` 之前，不改动其后的提示词。恢复选择器仍由客户端解析，未取得完整会话标识时保持未知。

### 新版首次安装与开关

采集器 bundle 独立版本为 2.0.0，与服务端身份识别规则版本 1 分开。其他电脑未安装过归因脚本时，只需在运行客户端的那台电脑安装一次；不需要迁移旧数据库或先安装旧版。

使用同一发行包中的 setup、launcher 和 Pi 扩展，在客户端主机以普通用户运行：

```sh
bash setup-client-attribution.sh enable claude --shell zsh --proxy-url http://127.0.0.1:57878
bash setup-client-attribution.sh status claude --shell zsh
bash setup-client-attribution.sh disable claude --shell zsh
```

将客户端名替换为 grok、gemini、codex、pi 或 all。远程客户端必须填写它能访问的 Sumpter 地址。首次启用后新开终端并重启客户端；macOS App 只管理本机，WebUI 提供在客户端电脑执行的命令。

| 操作 | 具体效果 |
| --- | --- |
| enable | 安装/更新配套资源并启用所选客户端，可指定绑定地址 |
| disable | 关闭补充采集，保留现代包装器和备份；清理无法读取新开关的旧 Shell 块 |
| install | 更新资源；首次安装默认开启，已存在的关闭状态保持关闭 |
| status | 只读检查配置、开关、配套版本、恢复目标和最近启动记录 |
| restore | 恢复安装前保存的归因块，可能恢复仍会采集的旧版；绝不当作关闭按钮 |
| uninstall | 关闭新版补充采集并移除终端注册，备份和私有资源保留 |

开关保存在 XDG_DATA_HOME/sumpter/attribution/collection.json，默认位于 ~/.local/share。这里仅保存匿名来源 ID、开关、绑定地址与更新时间，不保存密钥。关闭对这个用户的同一数据目录生效，跨 bash/zsh 共用。开关文件损坏时停止采集；损坏的备份记录不阻止 status、disable、uninstall，写操作将损坏内容另存保留。恢复损坏备份仍会报错。

安装操作使用带持有者信息的 install.lock 目录互斥。进程异常退出后，下次写操作可恢复本机已退出进程留下的锁；存活进程的锁不会因时间过长被抢占。旧版无持有者的空锁文件保留 5 分钟保护期，过期后自动恢复；status 不修改锁或配置。

已加载的新包装函数每次启动都重新检查开关。运行中的 Claude/Grok/Gemini/Codex 已经取得进程环境，关闭后需重启；Pi 新版钩子每个请求读取开关。已经加载的旧 Shell 函数或旧版客户端进程无法由新管理器撤回环境，需新开终端并重启。关闭不删除历史，也不屏蔽客户端原生身份和工作区信息。

Gemini 的连接准备与归因开关分开：disable 继续使用 SUMPTER_GEMINI_BASE_URL、SUMPTER_AUTH_TOKEN 准备原有 URL/认证，但不追加项目头或 session-id 参数。uninstall 会移除包装函数，因此不再负责 Gemini 的连接准备。

### 连接绑定与已知范围

- Claude：要求环境或用户设置提供连接依据，并且可见的用户/项目 ANTHROPIC_BASE_URL 候选均匹配。仅项目配置不足以证明它已获信任并生效；额外 settings CLI 入口、可见托管文件、云端/宿主管理或其他 provider 开关均跳过补充。参照 [官方设置优先级](https://code.claude.com/docs/en/settings) 和 [环境变量说明](https://code.claude.com/docs/en/env-vars)，这里采取保守检查，不模拟完整运行时配置解析。
- Codex：支持用户 config.toml 中选中的自定义 provider，以及 -p / --profile、-C / --cd 和简单 model_provider 选择。项目层 provider 或复杂 CLI provider 改写无法确认时跳过补充；不会替用户更换 provider。桌面 App 不会加载终端包装器。
- Grok：GROK_CONFIG 的 endpoints.xai_api_base_url 与 endpoints.cli_chat_proxy_base_url 都须指向绑定地址及路径，可见的 model_providers 地址也要匹配。GROK_CONFIG_PATH 及其他未解析来源保持未知。
- Gemini：核对包装器实际使用的 SUMPTER_GEMINI_BASE_URL。缺少地址/密钥时直接交由客户端处理，不报告已准备采集。
- Pi：逐请求要求 provider 的 x-sumpter-client: pi 显式标记，不根据 localhost 或模型名猜测。绑定地址字段不替代 provider 标记；该标记表示用户明确选择的归因目的地。

绑定比较协议、主机、端口和自定义路径前缀，忽略末尾斜杠以及客户端的单个 API 版本后缀（/v1，Gemini 还支持 /v1beta）。例如 /sumpter 与 /sumpter/v1 可匹配，/sumpter 与 /other-service 不匹配。它是补充采集的保守条件，不是鉴权或全功能客户端配置解析器。WebUI 仅在直连 HTTP 管理页且监听地址吻合时按实际代理端口预填；HTTPS 或路径反向代理需手动填写客户端使用的公开地址，已输入的值不会被后续加载覆盖。

无法确认不表示客户端绕过 Sumpter。除 Pi 钩子外，检查发生在启动时；运行中更换连接、/cd 或远程托管策略变化无法由启动包装器重新验证，需关闭补充并重启客户端。

### 配套更新与生效证据

任何客户端安装都写入 launcher + Pi 扩展 + manifest 完整资源对，再通过原子的 current 符号链接切换。运行中的模块继续引用旧的配套目录；旧目录保留。版本摘要同时覆盖两份源文件，因此只改 Pi 扩展也能区分版本。缺少或版本标记不匹配的配套文件会在修改终端注册前失败。

状态分成三类证据：

1. 配置：registration、enabled、proxyURL、installedVersion、availableVersion、backupState、restoreTarget。旧版恢复目标明确显示 legacy。
2. 启动：lastLaunch 中记录准备时的 enabled、binding、collectorVersion、sourceId、startedAt。不记录密钥、路径或提示词；这不是成功启动或已发请求的证明。
3. 请求：服务端 clientDeclared.sourceId 和 collectorVersion 必须匹配，才表明这个来源的该脚本版本曾被观察到。macOS 从已加载请求匹配；WebUI 显示服务端收到的来源，需要与客户端状态核对。已关闭后仍可能展示之前的请求；缺少已加载记录保持未知。

直接运行仓库 launcher 的 run 子命令可临时使用；若没有 collection.json，仍须满足上述连接绑定。常规使用推荐 enable，以生成稳定来源 ID。资源更新不等于更新已安装的 Sumpter App；只有重建并安装新 App 后，GUI 才使用新的内嵌管理器。仓库未发布的改动也不会自动出现在 GitHub 下载地址。

## 项目身份与历史兼容

新入站请求在路径脱敏前生成 workspaceIdentity：version=2、SHA-256 key、有界显示 name、scoped。规范化完整绝对路径及可用的匿名来源作用域参与散列，多个工作区先排序、去重。显示名最多 128 字节，只用于展示；完整路径不作为列表名称。

例如 /srv/customer-a/apps/api 与 /srv/customer-b/apps/api 均显示 api，但身份不同，不再因为脱敏尾部同为 .../apps/api 而合并。相同路径和来源稳定复用身份；不同来源的同一路径保持分开。来源 ID 缺失时只能按完整路径区分，不能保证不同电脑的同一路径可区分。来源 ID、散列都不构成身份认证或路径保密保证。

Codex 原生工作区优先于 wrapper 声明；原生没有 collector 来源时可用已有安装指纹作为作用域。截断的原生元数据不生成新身份，沿用既有兼容投影。当前是本地工作区分组，不自动把同一 Git remote 的多个副本合并为逻辑仓库。

新字段只在入站时生成。读取旧 JSON、重开 SQLite、筛选和导出不回填新身份，不改历史 payload；因此升级前后的记录可能分属旧、新两个项目组。历史迁移不在本次范围内。所有 X-Sumpter-*（包含 Source-Id、Collector-Version）仍在出站前剥离。
