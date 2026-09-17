## 1. 准备好三类信息

| 信息 | 填在哪里 | 谁使用 |
| --- | --- | --- |
| 上游地址、API Key、模型名 | 「入口库」或 `endpoints` | Sumpter 连接 Provider |
| 代理地址、入站 Token | 客户端连接设置；Token 在「安全」配置 | 客户端连接 Sumpter |
| 管理用户名、密码 | Linux WebUI 登录页 | 浏览器管理 Linux 服务 |

上游 Key 和入站 Token 是两套凭据。Linux 管理密码也不能代替 API Token。macOS App 使用本机 sidecar 的内部控制凭据，无需 Linux WebUI 密码。

安装入口：{{installation_links}}。

| 项目 | 默认位置 |
| --- | --- |
| 客户端代理地址 | `http://127.0.0.1:57878` |
| Linux 管理页 | `http://127.0.0.1:57879/admin/` |
| macOS 配置 | `~/Library/Application Support/Sumpter/config.json` |
| Linux 用户安装配置 | `${XDG_CONFIG_HOME:-~/.config}/sumpter/config.json` |
| Linux system 安装配置 | `/var/lib/sumpter/config.json` |
| Compose 配置 | 部署目录的 `config/config.json`，容器内为 `/config/config.json` |

`127.0.0.1` 指当前这台机器。客户端连接远程服务器时，使用服务器的可达地址或 SSH 隧道。不要把监听用的 `0.0.0.0` 填成客户端访问地址。

## 2. 配好第一个上游

先完成一条普通文本请求，再逐项启用其它能力。

1. 打开「入口库」，新建入口并填写名称。
2. 填入服务商提供的 API 基础地址和 API Key。地址是上游 API 地址，不是服务商控制台网页。
3. 选择与上游相符的协议。`auto` 可用于支持多种 API 的服务；协议标签不会替上游实现缺失的 API。
4. 获取模型目录，添加你要使用的模型。获取目录只提供候选，**不会自动开放全部模型**。
5. 在映射中设置客户端模型名。上游名称相同时留空 `upstreamModel`；不同时填写实际名称。
6. 保存并启用入口，然后进入「模型组」，启用一个组、添加该模型、绑定这个入口。

复制完整配置示例时，示例入口、功能分流和默认模型组都处于停用状态；`.invalid` 域名只是占位符。替换真实信息并启用相应项目后才能使用。

模型必须同时满足：入口有映射、模型组开放它、入口绑定允许它。删除入口映射后，模型组里残留的名称不会继续授权该模型。

## 3. 设置访问权限

在「安全」设置非空的代理入站 Token，并将同一个值填到客户端 API Key / Token 字段。这个 Token 由你管理，不必与任何 Provider Key 相同。

`listener.allowedCIDRs` 是允许访问的 IP 网段列表，空数组表示不按网段限制。Compose 默认只将端口发布到宿主机回环地址；远程使用按部署教程设置 SSH 隧道、VPN 或 HTTPS 反向代理。

经反向代理接入时，运行事件默认显示代理地址。把实际代理的 IP 或网段填进 `listener.trustedProxyCIDRs`（安全页「可信代理 IP / CIDR」），并让代理设置 `X-Forwarded-For` 或 `X-Real-IP`，事件就会显示真实客户端 IP。它只影响事件显示，不改变入站 Token 与 CIDR 白名单的判定。

Linux 首次管理用户名为 `kkl`，密码来自部署时准备的 `admin-password`。登录后可在「安全」修改用户名和密码；文件会保存为 Argon2 哈希 JSON，初始密码随即失效。重启服务会让现有浏览器会话失效，需要重新登录。

## 4. 接入客户端

下面都以客户端和 Sumpter 在同一台机器为例。`your-enabled-model` 必须换成已经配置好的客户端模型名，Token 必须换成自己的代理入站 Token。将配置合并到已有文件，不要覆盖其它 Provider 或个人设置。

### Claude Code

在启动 Claude Code 的终端设置：

```bash
export ANTHROPIC_BASE_URL='http://127.0.0.1:57878'
export ANTHROPIC_AUTH_TOKEN='你的代理入站 Token'
claude --model your-enabled-model
```

确认客户端没有仍然生效的另一套连接设置。需要长期使用时，将连接配置保存到客户端支持的配置位置。

### Codex 与 OpenAI 兼容客户端

Codex 自定义 Provider 使用 Responses API，Base URL 带 `/v1`。将下面配置合并到 `~/.codex/config.toml`：

```toml
model_provider = "sumpter"
model = "your-enabled-model"

[model_providers.sumpter]
name = "Sumpter"
base_url = "http://127.0.0.1:57878/v1"
wire_api = "responses"
env_key = "SUMPTER_API_KEY"
```

在启动客户端的环境中设置：

```bash
export SUMPTER_API_KEY='你的代理入站 Token'
codex
```

其它 OpenAI 兼容客户端也通常使用 `http://127.0.0.1:57878/v1`，按客户端实际发送的 Chat Completions 或 Responses API 配置上游。图形客户端需在其自身连接设置中填写地址和凭据，不能假定它继承终端环境。

### Grok Build

在 Grok Build 的模型连接设置中，将 API 地址改为 Sumpter 的 `/v1` 地址，API Key 改为代理入站 Token，模型改为已开放的客户端模型名。保留 Grok 原有的其它配置。项目归因使用下一节的统一安装器。

### Gemini CLI

先安装 Gemini CLI 和下一节的归因包装器，再设置并启动：

```bash
export SUMPTER_GEMINI_BASE_URL='http://127.0.0.1:57878'
export SUMPTER_AUTH_TOKEN='你的代理入站 Token'
gemini --model your-enabled-model
```

包装器使用 Gemini Developer API 原生路径并注入连接配置。需要单独运行时，也可调用随包提供的 `gemini-sumpter-wrapper.mjs`。Vertex、Code Assist、OAuth 和 Service Account 不是这条接入方式。`SUMPTER_GEMINI_PROJECT` 可覆盖默认项目名。

### pi

在 `~/.pi/agent/models.json` 的 `providers` 中添加：

```json
{
  "providers": {
    "sumpter": {
      "baseUrl": "http://127.0.0.1:57878/v1",
      "api": "openai-responses",
      "apiKey": "$SUMPTER_API_KEY",
      "headers": { "X-Sumpter-Client": "pi" },
      "models": [{ "id": "your-enabled-model" }]
    }
  }
}
```

```bash
export SUMPTER_API_KEY='你的代理入站 Token'
pi --provider sumpter --model your-enabled-model
```

| pi 的 API 类型 | Base URL |
| --- | --- |
| `openai-responses` / `openai-completions` | `http://127.0.0.1:57878/v1` |
| `anthropic-messages` | `http://127.0.0.1:57878` |
| `google-generative-ai` | `http://127.0.0.1:57878/v1beta` |

按实际模型补充上下文、输出上限和 reasoning 能力。若本机明确关闭入站认证，要求非空 Key 的客户端可以填非空占位值 `sumpter`。

## 5. 让统计识别项目和会话

归因脚本收集客户端明确提供的项目、用户和会话信息，让统计能够按项目分组。**在运行客户端的机器安装**；仅运行代理的服务器不需要安装它。包装器面向已经连接 Sumpter 的客户端。

macOS 本机用户在 App「安全」页选择客户端并安装配置。其它情况可在客户端终端执行，无需 `sudo`：

```bash
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh
bash setup-client-attribution.sh status all
bash setup-client-attribution.sh install all
```

也可以将 `all` 换成 `claude`、`grok`、`gemini`、`codex` 或 `pi`，用 `--shell bash` 或 `--shell zsh` 选择终端。安装后新开终端，并重新启动客户端。

安装器管理终端启动包装器。Claude 和 Grok 的配置环境覆盖可能与其冲突，按安装器提示处理；不要在不清楚作用时删除已有配置。

pi 包装器会自动加载私有 `pi-project-attribution.ts`，保存在 `${XDG_DATA_HOME:-~/.local/share}/sumpter/attribution/`。Provider 须带 `X-Sumpter-Client: pi`，pi 版本须支持 `before_provider_headers`。包管理命令仍直接交给 pi。`/reload` 不能加载新的 shell 配置；后台插件直接调用模型 SDK 的请求也不一定经过 pi 的会话钩子。

Codex 包装器用于 CLI/TUI，自定义 Provider 的连接设置保持原有内容；它不会注入已运行的会话或 Codex Desktop 图形进程。客户端自带的结构化 workspace 信息仍优先使用。

要撤销某个客户端的包装器：

```bash
bash setup-client-attribution.sh restore pi
```

发送请求后，在「运行」检查客户端、项目和会话，再在「统计」按这些维度筛选。没有请求时，“暂无可判定数据”并不表示安装失败；没有可靠项目证据时保留“未识别”。

`X-Sumpter-Workspace` 等 `X-Sumpter-*` 归因头会在发往上游前剥离。常规统计使用经过清洗的投影；诊断捕获可能包含原始正文、路径和凭据，不应直接分享。

## 6. 配置多个入口和模型组

入口库负责“连接谁”，模型组负责“哪些模型可以通过哪些入口”。一个代理地址可以服务多个模型组，客户端继续使用模型名，无需选择组专属地址。

| 设置 | 作用 |
| --- | --- |
| 组优先级 | 数值小的组先进入候选序列 |
| 绑定优先级 | 同一组内，数值小的入口优先 |
| `priority` | 按优先级和配置顺序选择 |
| `randomSticky` | 新会话从最低优先级的候选调度组中随机选择，后续保持会话归属 |
| `roundRobinSticky` | 新会话依次分配到同级调度组，后续保持会话归属 |
| `stickyGroup` | 多个入口使用同一个值时，共享调度归属；组内仍按配置顺序尝试 |
| 绑定 `overrides` | 仅为这个绑定中的模型覆盖上游名称或入口优先级 |

例如，主用入口优先级为 `0`，备用入口为 `10`，主用出现可切换故障时才访问备用。如果希望新会话分散到多个入口，给它们相同优先级和不同调度组，再选择随机或轮询粘性。

会话粘性会优先沿用已有归属，因此修改优先级后，旧会话不一定立即换入口。可在项目或会话操作中清除粘性，再发新请求。轮询游标随进程重启或配置重载重新开始，已有有效归属仍会保留。

## 7. 设置故障切换与超时

通常先保留默认值，确认上游可用后再按实际需求调整。

- `responseTimeoutSeconds` 限制等待首响应的时间；映射级 `failoverTimeoutSeconds` 可进一步缩短某模型的等待时间，两者取较小值。
- `streamIdleTimeoutSeconds` 限制流式响应连续无数据的时间。首响应和流式空闲是不同阶段。
- `max500Retries` 控制当前入口收到 500 后的额外重试；`failoverOn500` 决定耗尽后是否切换入口。
- `sessionStickyRetries` 控制当前粘性调度组遇到非 500 可重试故障后的额外尝试。
- `maxDeferredRounds` 与 `maxRetryDurationSeconds` 都为 `0` 时，可重试故障没有轮数和总时长上限；客户端断开会取消等待。希望请求尽快结束时，应设置有限上限。
- `passThroughRetryDelay` 控制失败响应中的 `Retry-After` / `retry_delay` 是否继续传给客户端；它不是入口冷却时间开关。

收到上游错误后，代理按重试策略处理并尽量保留最终上游状态和错误内容。尚未发往上游的认证、路由、存储或连接问题仍可能产生本地错误。

## 8. 理解协议和高级能力

会话请求按协议择路:入口协议与客户端一致时保留客户端实际请求路径、查询和其余负载,按入口基础路径规则组合最终 URL;不一致时在该模型的映射范围内把请求转成入口协议,响应再转回客户端协议。Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 与 Gemini `generateContent`/`streamGenerateContent` 之间两两可转;Compact、countTokens、embedContent、图片生成与 Realtime 等没有会话语义的接口不参与转换。

转换不是全功能兼容:历史推理内容不会回放,服务端工具、provider 文件引用以及目标协议没有等价参数的项会被明确拒绝并说明字段,不会静默丢弃。

| 请求 | 使用要求 |
| --- | --- |
| Messages / Chat Completions / Responses | 上游支持客户端实际发送的 API |
| Gemini | 上游支持 Developer API 原生模型路径 |
| 图片、文件、视频 | 上游实现对应资源接口；异步资源后续请求依赖原入口绑定 |
| WebSocket | 上游完成真实握手后才建立客户端连接 |
| Codex Live | 入口有 `gpt-live-1-codex` 精确映射及相应能力 |
| 标准 Realtime | 配置实际语音模型的精确映射，不能用普通文本模型通配代替 |

没有可用 Live 入口时会出现 `no_live_provider`。能发送普通文本请求不代表上游支持语音、视频或所有资源接口；更改协议标签也不会补齐这些能力。

「Claude Code 路由」中的 WebSearch、WebFetch 和安全分类器分流识别独立子请求，可为其选择目标模型和入口。第一次配置可先全部关闭。字段细节见 {{configuration_link}}。

## 9. 日常查看与维护

| 页面 | 用法 |
| --- | --- |
| 运行 | 查看代理状态、进行中请求和完成记录，打开详情检查上游尝试链 |
| 入口库 | 管理上游连接、获取模型、编辑映射 |
| 模型组 | 设置开放模型、入口绑定和调度策略 |
| Claude Code 路由 | 设置特定子请求的分流目标 |
| 安全 | 设置访问控制、客户端归因；Linux 另有管理凭据 |
| 统计 | 按时间、客户端、模型、入口、项目和会话查看请求与用量 |
| 诊断 | 查看服务问题、存储状态，按需开启和导出诊断捕获 |

一条客户端请求可能包含多次上游尝试。排障时查看最终结果，不要仅凭某次尝试的状态码判断整条请求失败。首字节耗时、总耗时和进行中状态也分别表示不同阶段。

Token 统计来自上游 usage；未上报与明确的零值不同。缓存读写口径随协议不同，成本是基于配置价格的估算，不等于服务商账单。

统计保存在配置目录的 `runtime.sqlite3`，无需安装数据库。保留策略的时间和容量上限任一达到便轮换最旧的已完成请求组，进行中的整组受保护；两项都不设置时不会自动删除历史。删除会话、清空统计和重建数据库有不同范围，执行前阅读界面提示并保留需要的数据。

手工编辑配置后需重载；修改 Linux Admin 监听或凭据文件后需重启进程。备份时停止服务并复制整个配置目录，包含存在的 SQLite WAL 和资源绑定。详细步骤见对应安装教程。

## 10. 第一条请求没有成功

按以下顺序检查：

1. 客户端访问的是代理端口 `57878`，浏览器访问的是管理端口 `57879`。
2. 客户端和服务器是否在同一台机器；远程部署的端口发布和隧道是否正确。
3. 入站 Token 是否一致，Provider API Key 是否仍有效。
4. 入口、模型组、绑定是否启用，模型是否在三者允许范围内。
5. 运行详情中的实际出站地址、模型、上游错误和最终结果是否符合预期。
6. 只有特定会话出错时，用新会话复测；跨上游的加密推理状态未必兼容。

进一步排查见 {{troubleshooting_link}}。报告问题时附平台、版本、安装方式、复现步骤和脱敏错误，不附真实配置或完整诊断捕获。
