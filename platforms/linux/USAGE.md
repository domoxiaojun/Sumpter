# Sumpter使用指南

给第一次安装并接入 Claude Code 或 Codex 的用户。

**范围**：安装入口、`config.json`、接 Claude Code / Codex、常见错误。  
**不包含**：改源码、编译、发版、Git 远程。


<!-- BEGIN SUMPTER_CANONICAL_ONBOARDING -->

## 开箱路径（固定顺序）

第一次使用只做 3 件事：找到配置 → 填一个上游服务（Provider）→ 启动 Sumpter 并接入客户端。
备用入口、重试和功能规则等高级配置，等基本使用后再设置。

### 1. 找到配置文件

| 平台 | 配置文件 |
|---|---|
| macOS App | `~/Library/Application Support/Sumpter/config.json`（仅在 macOS 客户端使用） |
| Linux daemon | 普通用户：`$XDG_CONFIG_HOME/sumpter/config.json`，否则 `~/.config/sumpter/config.json`；system 安装：`/var/lib/sumpter/config.json` |

优先复制对应平台的 `config.example.json`，不要从零创建 JSON。`listener` 默认保持
`127.0.0.1:57878`，先保存配置，再启动 Sumpter。

### 2. 只填一个上游服务入口

在 `endpoints[]`（上游服务列表）中先只启用一个入口，填 `baseURL`（上游服务地址）、`apiKey`、
`enabled: true` 和 `mappings`。例如：

```json
{ "clientPattern": "gpt-5.4", "upstreamModel": "gpt-5.4" }
```

`clientPattern` 必须和客户端实际使用的模型名一致；不确定时先用精确名称。

### 3. 接入客户端

Claude Code：

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878
```

Codex：**Base URL 必须带 `/v1`**。

```toml
model = "gpt-5.4"                 # 改成你 mapping 里的模型名
model_provider = "sumpter"

[model_providers.sumpter]
name = "Sumpter"
base_url = "http://127.0.0.1:57878/v1"
wire_api = "responses"
experimental_bearer_token = "填 listener.authToken（未启用鉴权时删除此行）"
```

如果 `listener.authToken` 不为空，把同一个值填入 `experimental_bearer_token`；只在本机使用时可以删除这一行。

启动 Sumpter 后，在客户端发一条请求即可开始使用。

跨机器使用时，把 `listener.host` 改为内网可达地址并设置非空 `authToken`，不要把端口裸露到公网。

不要把 `config.json`、API key、Token、Cookie 或 raw 诊断内容提交到 Git、Issue 或聊天记录。

<!-- 由 scripts/sync-usage-docs.py 生成；请修改 docs/usage-onboarding.md 与 docs/usage-path-matrix.json 后同步。 -->

<!-- END SUMPTER_CANONICAL_ONBOARDING -->

两端当前使用 **schema v6** 的 `config.json`。启动或热重载时自动迁移旧的 schema v3 / v4 / v5；更早的
格式和旧 `keys.json` 不会被读取，可留作人工转换的草稿。迁移会先创建带时间戳的
`config.before-schema-v6-*.json` 原始备份，成功后才原子替换配置。

本文是 Linux 发布包的使用指南。源码 monorepo 中它位于 `platforms/linux/USAGE.md`；发布阶段会把
`platforms/linux/` 提升为包根，届时本文与同目录 `README.md` 位于发布包根目录。开箱模板的真源是
仓库根 `docs/usage-onboarding.md`，由 `scripts/sync-usage-docs.py` 同步到仓库根 `USAGE.md` 和本文件。

## 文档怎么读

| 你想做什么 | 读哪份 |
|---|---|
| 开箱、接客户端、排错 | 本文 |
| 每个配置字段的含义 | 本文第 5 节；macOS 字段完整说明仅在源码树的 `platforms/macos/CONFIG.md` |
| Linux 安装、systemd、Docker、Admin 反代 | [`README.md`](README.md) |
| macOS 首次打开被拦截 | 该说明不随 Linux 发布包提供；请在源码树查看 `platforms/macos/app/INSTALL.txt` |

Agent：只改**本机配置目录**里的 `config.json`，不要把填好 key 的文件提交进 git，不要读取后写进聊天记录。

---

## 1. 先搞清你在用哪一端

| | macOS App | Linux daemon |
|---|---|---|
| 配置文件 | `~/Library/Application Support/Sumpter/config.json` | 普通用户：`$XDG_CONFIG_HOME/sumpter/config.json`，否则 `~/.config/sumpter/config.json`。system 安装：`/var/lib/sumpter/config.json` |
| 权限 | `0600` | `0600` |
| 代理端口 | `listener.port`，默认 `57878` | 同左 |
| 管理界面 | 菜单栏 App 设置窗 | 浏览器 `http://127.0.0.1:57879/admin/` |
| 改配置后 | App 里保存 / 重启 | WebUI 保存，或 `SIGHUP` 热重载 |

模板：

- **推荐抄** `config.example.json`（源码 monorepo 对应 `platforms/linux/config.example.json`）：域名是 `.invalid`，入口全部 `enabled: false`，secret 是合成的 `sk-test-…`。
- `platforms/macos/config.example.json` 与 Linux 使用同一份安全模板：入口全部禁用、地址使用 `.invalid`，可直接作为结构参考；启用前务必替换为你的地址和 key。

Linux 首次 Admin 用户名是 `kkl`，密码在同目录 `admin-password`（不要在终端 `cat` 出去）。改密后该文件会变成 Argon2 哈希 JSON，不能再回读明文。

---

## 2. 安装

### 2.1 macOS

打开 DMG，双击「安装Sumpter.command」，按提示确认。安装器只处理旁边的 `Sumpter.app`，只去掉这个 App 的 quarantine 标记，不会关闭全局 Gatekeeper。

不想用安装器时，把 App 拖到「应用程序」，再在 Finder 里右键 → 打开。不要用 `spctl --master-disable`，也不要对整个磁盘执行 `xattr -dr`。macOS 安装器的完整说明在源码树 `platforms/macos/app/INSTALL.txt`；Linux 发布包不包含该安装器。

### 2.2 Linux

静态镜像一键安装（按架构自动取包）：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh https://sf.domob.org/kkl/sumpter-install.sh
bash /tmp/sumpter-install.sh
```

`sudo bash /tmp/sumpter-install.sh` 会装成 system 服务（daemon 仍以低权限 `sumpter` 用户运行）。已有 `config.json` 与 `admin-password` 不会被覆盖。

Docker、systemd、Admin HTTPS 反代、卸载见 [`README.md`](README.md)。

---

## 3. 最小开箱

目标：Claude Code / Codex 的请求由本机代理按入口映射、优先级和粘性分组调度到多个上游。

### 3.1 准备一份配置

1. 把上一节推荐的模板复制到配置路径。
2. `chmod 600` 该文件。
3. 至少改一个 `endpoints[]` 的 `baseURL`、`apiKey`、`enabled: true`，并在 `mappings` 里写下客户端实际会发的模型名。
4. `schemaVersion` 保持 `6`；新入口的 `protocol` 默认使用 `auto`。
5. 启动 App 或 Linux 服务。

没有 `config.json`、只有旧 `keys.json` 时，进程会拒启，且不会改你的旧文件。

**模型必须写在入口的 `mappings` 里。** 某个模型只会发给声明了它的入口；空 `mappings` 的入口不承接任何模型，代理也不会拿未声明的原名去碰上游。旧 schema v5 的池级 `globalModels` 只在迁移时复制到当时还没有显式映射的入口，现行配置里已经没有这个字段。

### 3.2 接 Claude Code

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878
# 若 listener.authToken 非空，必须完全一致：
export ANTHROPIC_AUTH_TOKEN='填 config 里的 authToken'
```

模型名必须能被某个已启用入口的 `mappings.clientPattern` 接住（精确名或 `prefix-*` 通配）。没有任何入口声明该模型时返回 **400**。

### 3.3 接 Codex / 其它 OpenAI 客户端

把客户端的 API Base 指到 Sumpter 本机地址。**Codex 必须使用带 `/v1` 的 Base URL**，默认填写
`http://127.0.0.1:57878/v1`；如果你改过 `listener.host` 或 `listener.port`，只替换前面的 host/port，
仍然保留最后的 `/v1`。不要填写上游 Provider 的 `baseURL`。

`~/.codex/config.toml` 可以写成：

```toml
model = "gpt-5.4"                 # 必须能被某个入口 mappings 接住
model_provider = "sumpter"

[model_providers.sumpter]
name = "Sumpter"
base_url = "http://127.0.0.1:57878/v1"
wire_api = "responses"
experimental_bearer_token = "填 listener.authToken"
```

其它 OpenAI SDK：

```bash
export OPENAI_BASE_URL=http://127.0.0.1:57878/v1
```

若 `listener.authToken` 非空，把它填入 `experimental_bearer_token`；其它 OpenAI SDK 再按各自的 API key 配置。

`endpoints[].protocol` 是入口的能力/统计元数据，保留四个配置值：

| `protocol` | 含义 |
|---|---|
| `auto` | 默认值；不限制原始 HTTP 请求的路径或 body |
| `anthropic` | 入口标签为 Anthropic |
| `openai` | 入口标签为 OpenAI Chat |
| `openai-responses` | 入口标签为 OpenAI Responses |

这些值不会作为出站协议发送给上游，也不会让 Sumpter 对 HTTP 请求做协议转换或拒绝。

---

## 4. 协议与路径

数据面通常不识别、重建或转换协议，也不维护路径别名白名单。除 `/__*` 本地控制接口外，任意
HTTP 方法和任意路径都会进入同一条转发链：入站鉴权 → Provider 选择 → failover/retry →
上游 relay。客户端的原始 path/query、可转发请求头、请求体，以及上游返回的状态、响应头和
响应体都交给上游；仅移除 Host、连接级 hop-by-hop/传输 framing 头和入站鉴权头，再注入
Provider 鉴权。
Codex Live/Realtime 是选路例外，不是报文例外：`POST /v1/live`、`POST /v1/realtime` 和
`POST /v1/realtime/calls` 会忽略当前文本会话泄漏的模型名，按 Live 意图选择
`gpt-live-1-codex` 的精确 mapping；原始 path/query、SDP 或 multipart body 和媒体类型仍原样
交给 Provider。Sumpter 不把 `/v1/realtime` 改成 `/v1/realtime/calls`，也不二次封装
Quicksilver JSON；CPA 一类 Provider 自己负责对应协议。无 `call_id` 的 `GET /v1/realtime`
仍归类为公开 Realtime WebSocket，带 call id 的后续请求则钉回创建会话的入口。
另一个例外是 `GET /v1/models`（以及 `/models`、`/openai/v1/models` 和带模型 id 的子路径）：
按当前启用入口的 `mappings` 生成本地目录，不转发到上游。普通 OpenAI 客户端拿到
`{object:"list",data:[...]}`；Codex Desktop/CLI 带 `client_version` 时拿到 `{models:[...]}`。
这样 Codex 的目录探测不会打到排序最前的任意 OpenAI 入口。
Live/Realtime 只接受可用 Provider 上的精确 mapping，不使用 `*` 通配或 Anthropic 文本入口；
缺少精确 Live mapping 时返回 `no_live_provider`，避免语音请求误发到普通模型。

Provider 选择仍以已配置的 `mappings` 为准。JSON 只用于读取路由所需的模型元数据：请求已有
顶层 `model` 或 `session.model` 且对应 mapping 配置了不同的 `upstreamModel` 时，才替换这一
个模型值；不会主动新增 `model`，不会删除或重排其它字段。无论路径是否带 `/v1`、`/openai/v1`
或其它前缀，原始路径都会按客户端写法交给 Provider，不做别名归一化。

因此以下请求都只是普通透传示例，不代表 Sumpter 内置了对应协议实现（Codex `/v1/live` 的
bootstrap 封装除外）：

- `POST /v1/responses`、`GET /v1/responses` WebSocket
- `GET/POST /v1/realtime` 及其任意子路径
- `GET/POST/DELETE /v1/files`、`/v1/videos` 及资源查询或内容下载
- 未知的厂商路径、二进制请求和非 JSON 请求

WebSocket 只按 Upgrade 请求建立统一的双向 relay；文本、二进制、ping/pong 和关闭帧不解析、不
改写。对 `/v1/realtime`、`/v1/live` 及其 sideband，Sumpter 会先完成候选 Provider 的上游
握手，再向客户端返回 `101`；上游握手的 `401/404/429/5xx` 会保留为下游 HTTP 状态，不会先
返回一个误导性的 `101`。Realtime 的短期 `ek_…` 凭证仍可用于后续 HTTP/SDP/WebSocket 请求；
其返回的 `session` 配置会在 WebSocket 建连后以 `session.update` 发送给上游，HTTP calls 也会
复用 voice、instructions 等字段。这是鉴权/会话绑定能力，不是本地协议实现。上游具体模型、
权限和媒体/Realtime 能力由 Provider 决定，需用目标 Provider 实测确认。

---

## 5. 配置文件怎么填

顶层只有：`schemaVersion`、`listener`、`retry`、`endpoints`、`featureRules`。  
磁盘上**没有** `pools`、`globalModels` 或 `target.poolID`。旧文件会在迁移时展平。

### 5.1 listener — 谁可以连进来

```json
"listener": {
  "host": "127.0.0.1",
  "port": 57878,
  "allowedCIDRs": [],
  "authToken": ""
}
```

| 字段 | 怎么用 |
|---|---|
| `host` | 本机只用 `127.0.0.1`。要给局域网用再改，并配合 CIDR / token |
| `authToken` | 空 = 不校验。非空则客户端必须带相同值 |
| `allowedCIDRs` | 空数组通常即可。环回始终放行 |

Linux 的 Web Admin 地址 **不是** 这个字段，默认永远是 `127.0.0.1:57879`。

### 5.2 retry — 先用默认即可

开箱不必改。默认接近「可重试错误就继续试」：

- `maxDeferredRounds` / `maxRetryDurationSeconds` 为 `0` = 不限轮、不限总时长
- `sessionStickyRetries` 默认 `2`：当前线路组遇到非 500 可重试故障后再整组多试 2 次，仍不行才换组
- `max500Retries` 默认 `0`：当前入口收到 HTTP 500 后的额外重试次数；`0` 表示不额外重试
- `failoverOn500` 默认 `true`：500 重试耗尽后切换下一个入口；设为 `false` 则在当前入口直接返回 500。500 不进入跨轮无限重试
- `retryDelaySeconds` 可选：为最终失败响应准备的 `retry_delay` 秒数
- `passThroughRetryDelay` 默认 `true`：是否将 `retry_delay` 与 `Retry-After` 透传给客户端；关闭后即使填写秒数也不返回
- 两个超时为 `null` = 跟客户端，代理自己不加硬截止

只有两个上限**同时为 0** 才是真无限重试。客户端一断开，代理会立刻停。

### 5.3 endpoints — Provider 入口

所有入口都按 `priority` 从小到大调度，同级保持数组顺序；同一 `stickyGroup` 内的线路连续尝试。已有稳定会话优先复用原分组，发生可重试故障后再 failover 到其它分组。

入口必填：

| 字段 | 注意 |
|---|---|
| `id` | 唯一。改 id 会让用量统计断成两截 |
| `baseURL` | 上游根，不要抄错路径 |
| `apiKey` | 明文，仅本机文件 |
| `protocol` | 必填四态：`auto`（默认）/ `anthropic` / `openai` / `openai-responses` |
| `enabled` | `false` 的入口不参与 |
| `priority` | 非负整数，越小越优先；同级按数组顺序；`0` 可不写 |
| `mappings` | **必须**声明本入口承接的客户端模型；空数组 = 不接任何模型 |
| `stickyGroup` | 空 = 用 id 当独立组；同名组共享会话 |

可选：`pinnedIPs` / `pinnedIPExclusive`、`keepAlive`（省略 = 关闭；界面里新建入口默认打开）、`catalog`（「获取模型」缓存，纯展示，不参与路由）。

### 5.4 mappings[]

- `clientPattern`：客户端发来的模型名（可通配）
- `upstreamModel`：空 = 同名转给上游
- `thinking` / `context`：保留在配置中的兼容字段；raw 透传不会据此改写客户端 body 或 header
- `failoverTimeoutSeconds`：可选的映射级首响应截止；与全局 `responseTimeoutSeconds` 同时配置时取较小值，到期且尚未收到响应时尝试下一个入口

同一入口同时命中精确模型名和 `prefix-*` 通配时，精确映射优先；同级按配置顺序。

### 5.5 featureRules — 分流（可先全关）

模板里三条内建规则默认 `enabled: false`：

- `websearch` / `webfetch` / `classifier`：只识别 Claude Code 那种独立子请求的完整形状，不会扫主会话全文。

启用规则后只影响模型/Provider 的选择；Sumpter 不会把请求改造成另一种协议，也不会注入、删除
或重排 WebSearch、Grok、工具等字段。具体能力由上游 Provider 自己处理。

要启用：设 `enabled: true`，`target.model` 必须能被某个入口的映射承接。`endpointID` 可选，钉住后只走该入口；入口失效时退回整列候选。`target.effort`、`target.protocol` 仅作为兼容配置/统计元数据，不触发数据面协议转换。

开箱可以全部保持关闭，先保证主对话能通。`featureRules` 也可以写成 `[]`（进程会补三条内建规则，默认停用）。

---

## 6. 一份可抄的骨架

把 URL、key、模型名换成你的。不要用示例里的 `.invalid` 域名去打真实流量。

```json
{
  "schemaVersion": 6,
  "listener": {
    "host": "127.0.0.1",
    "port": 57878,
    "allowedCIDRs": [],
    "authToken": ""
  },
  "retry": {
    "max500Retries": 0,
    "responseTimeoutSeconds": null,
    "retryDelaySeconds": null,
    "passThroughRetryDelay": true,
    "streamIdleTimeoutSeconds": null,
    "sessionStickyRetries": 2,
    "maxDeferredRounds": 0,
    "maxRetryDurationSeconds": 0,
    "pinnedIPConcurrency": 3
  },
  "endpoints": [
    {
      "id": "main-1",
      "name": "主入口",
      "baseURL": "https://YOUR-MAIN-HOST",
      "protocol": "auto",
      "enabled": true,
      "apiKey": "sk-YOUR-MAIN-KEY",
      "priority": 0,
      "pinnedIPs": [],
      "pinnedIPExclusive": false,
      "stickyGroup": "main-1",
      "mappings": [
        {
          "clientPattern": "claude-opus-*",
          "upstreamModel": "",
          "thinking": "adaptive",
          "context": "oneMillion"
        }
      ]
    },
    {
      "id": "cheap-1",
      "name": "Haiku 入口",
      "baseURL": "https://YOUR-SECOND-HOST",
      "protocol": "auto",
      "enabled": true,
      "apiKey": "sk-YOUR-SECOND-KEY",
      "priority": 10,
      "pinnedIPs": [],
      "pinnedIPExclusive": false,
      "mappings": [
        {
          "clientPattern": "claude-haiku-4-5-20251001",
          "upstreamModel": "claude-haiku-4-5-20251001",
          "thinking": "disabled",
          "context": "standard"
        }
      ]
    }
  ],
  "featureRules": []
}
```

更完整的 Linux 模板见 `config.example.json`；macOS 模板和字段逐项说明只在源码树 `platforms/macos/config.example.json` 与 `platforms/macos/CONFIG.md` 提供。

### 旧配置怎么迁

- schema v3 / v4 / v5 会在启动时迁到 v6，备份名为 `config.before-schema-v6-*.json`。
- 旧 `pools` 会按原顺序展平为顶层 `endpoints`；非主池入口会拿到更高的连续 `priority`。
- 旧池级 `globalModels` 只复制给当时 `mappings` 为空的入口，然后删除。
- 旧 `featureRules[].target.poolID` 删除；规则目标改走入口序列 / 可选 `endpointID`。
- 旧 `listener.inboundDialectPassthrough=true` 会把当时的入口改成 `protocol: "auto"`；为 `false` 或缺失时保留三种固定协议；缺少 `protocol` 的旧入口按历史默认迁为 `anthropic`。
- 迁移失败不会覆盖旧文件。已经是 v6 的配置不会再迁一遍。

---

## 7. 改完怎么生效

| 平台 | 做法 |
|---|---|
| macOS App | 设置里保存；需要时点重启。监听地址/端口变了会重绑 |
| Linux WebUI | 登录后改并保存 |
| Linux 手改文件 | 对进程 `SIGHUP`，或 `systemctl --user reload sumpter` / `sudo systemctl reload sumpter` |

---

## 8. 让 Claude Code / Grok Build 按项目统计（可选）

统计页有「项目 Token 排行」。Codex 会自己上行工作区信息，天生分好项目；**Claude Code 不会**
——它的 `cwd` / `project_dir` 只给本机 statusLine 和 hook 用，不进发给代理的请求，所以默认
所有 CC 请求都堆在「未识别项目」里。会话维度不受影响：CC 无条件发 `X-Claude-Code-Session-Id`，
**会话统计零配置就有**，这里配的只是项目维度。

要分项目，让客户端按启动目录带上 `X-Sumpter-Project` / `X-Sumpter-Workspace` /
`X-Sumpter-Git-Remote` / `X-Sumpter-User`（代理读完即从出站剥离）。有工作区路径时来源是
**本地项目**，带用户名时运行页显示例如 `sumpter 本地(kkl)`。

> **在哪台机器配？** 在**跑 Claude Code 的那台机器**上，不是跑 daemon 的那台。daemon 常在远程
> 或容器里（比如你连的是 `192.168.0.8`），但归因 header 是 CC 进程的环境变量，只能在 CC 本地设。
> 每台跑 CC 的机器各配一次。

### 一键配置

配置器 `cc-project-attribution.sh` 随发布包分发（Linux 解包后在 `/opt/sumpter/scripts/`；macOS
打包进 App 内的 `Sumpter.app/Contents/Resources/`；源码 monorepo 中 macOS 脚本位于
`platforms/macos/scripts/`、Linux 脚本位于 `platforms/linux/scripts/`，发布包内为 `scripts/`
下）。支持 zsh 与 bash，两个平台通用。

两个产品的**安全页面**都有一份完整引导（安装命令、平台差异、常见陷阱和回退命令），命令可直接复制；
macOS 那份还带「在 Finder 中显示」直接定位到脚本。

安装配置器：

```bash
# 装前想预演就加 --dry-run
./cc-project-attribution.sh install
```

Linux 的 Claude Code 若在另一台机器上运行，可从 **Sumpter Linux listener 的 Base URL** 取得配置器。
这个 URL 可以是局域网监听地址，也可以是转发该路径的 Nginx HTTPS 地址；不是发布镜像地址：

```bash
SUMPTER_LISTENER_BASE_URL='http://192.168.1.20:57878'
SUMPTER_LISTENER_BASE_URL="${SUMPTER_LISTENER_BASE_URL%/}"
curl --fail --location \
  "$SUMPTER_LISTENER_BASE_URL/__sumpter/cc-project-attribution.sh" \
  -o /tmp/cc-project-attribution.sh
bash /tmp/cc-project-attribution.sh install
```

如果 listener 配置了 `authToken`，下载时加同一个 Bearer token：

```bash
SUMPTER_LISTENER_BASE_URL="${SUMPTER_LISTENER_BASE_URL%/}"
curl --fail --location \
  -H "Authorization: Bearer $SUMPTER_LISTENER_TOKEN" \
  "$SUMPTER_LISTENER_BASE_URL/__sumpter/cc-project-attribution.sh" \
  -o /tmp/cc-project-attribution.sh
```

Nginx 反代必须把 `__sumpter/cc-project-attribution.sh` 原样转发到 proxy listener，并保留
`Authorization`/`x-api-key`；脚本只在
Claude Code 客户端本地执行，不会修改 daemon 主机。
通过 Nginx 对外提供时建议（跨机器时应）设置非空 `listener.authToken`；不要把无认证的 proxy
listener 直接暴露到公网。

安装后新开终端，在项目目录发消息即可使用项目统计。

出问题随时回退，装前状态有时间戳备份：

```bash
./cc-project-attribution.sh restore     # 还原 rc 到装前
./cc-project-attribution.sh uninstall   # 移除 wrapper,保留备份
```

配置器做了什么：往 rc 文件（zsh→`~/.zshrc`；bash 在 macOS→`~/.bash_profile`，Linux→`~/.bashrc`）
加一段带标记的 `source` 块，指向一个独立 snippet。改动最小、可精确移除，装前自动备份。它还会
**拒绝**在 `settings.json` 已写死该 header 时安装（那样装了也不生效，见下），并处理好尾随斜杠、
非 `origin` remote、非 ASCII 目录名这些边角。

**fish 用户**：自动安装只支持 zsh/bash。跑 `./cc-project-attribution.sh fish-snippet` 打印一段
等价的 fish 配置，粘进 `~/.config/fish/config.fish`。

### macOS 与 Linux 的差别

| | macOS | Linux |
|---|---|---|
| 谁在跑 CC | 本机菜单栏 App 旁边就是 CC | CC 可能在本机，也可能在别的机器上连远程 daemon |
| 配置器位置 | App 内 `Sumpter.app/Contents/Resources/`（源码构建则是 `platforms/macos/scripts/`） | 发布包解包后 `/opt/sumpter/scripts/`（源码树 `platforms/linux/scripts/`） |
| 默认 shell | 通常 zsh | 视发行版，zsh 或 bash 都常见 |
| 看统计 | 菜单栏 App 的「统计」页 | Web Admin 的统计页（`http://<daemon>:57879/admin`） |

配置器两边命令完全一样，自己会探测 shell 与平台。

### Grok Build

Grok 没有 `ANTHROPIC_CUSTOM_HEADERS`。配置器注入 `grok()`，每次启动用 `GROK_CONFIG` overlay
写入 `[models].extra_headers`，**不改** `~/.grok/config.toml`。

```bash
./grok-project-attribution.sh install
```

Linux 也可从 listener 下载 `/__sumpter/grok-project-attribution.sh`。已设置 `GROK_CONFIG_PATH`
时默认拒装（`--force` 才继续）。

### 手工配置（不想用配置器时）

```bash
export ANTHROPIC_CUSTOM_HEADERS="X-Sumpter-Project: $(basename "$PWD")
X-Sumpter-Workspace: $PWD
X-Sumpter-Git-Remote: $(git remote get-url origin 2>/dev/null)
X-Sumpter-User: $USER"
```

curl 风格 `名字: 值`，**一行一个**，三个都可选。但手工设有几个坑配置器已经替你处理，自己写要注意：

- **值必须纯 ASCII。** CC 见到含非 ASCII 的 `ANTHROPIC_CUSTOM_HEADERS` 会**直接报错退出**
  （`Invalid value for distinct header ... non-ASCII character`），整个会话起不来——不是归因失败，
  是 `claude` 用不了。中文目录名不能直接塞，得先判断跳过。
- **别写进 `settings.json` 的 `env`。** 那里的值**覆盖**进程环境变量，一旦写死 shell 里再怎么设都
  不生效；且 `env` **不做插值**（`$PWD`、`${CLAUDE_PROJECT_DIR}` 都按字面发出），只能是一个固定
  字符串，等于所有项目共用一个名字。
- **进程级、启动时读一次**，之后 `cd` 到别的目录不更新——归因的是「启动 CC 时所在的项目」。想每个
  项目自动跟着走，用配置器，或 per-project 的 `.envrc`（direnv）。

### 几个要知道的

- 工作区在界面上只显示**尾两段**（`.../claude/automode-proxy`），完整绝对路径不落盘，故意的。
- 带了工作区路径时来源是「本地项目」，有 `X-Sumpter-User` 时显示 `本地(用户名)`。只有项目名时仍是「客户端声明」。
- **这三个 header 被剥，不代表路径没外泄。** CC 每条请求的 body 里本来就带工作目录绝对路径、
  `CLAUDE.md` 全文与 `git status` 摘要，代理对 `system` / `messages` 一字不改地转发。配不配这三个
  header 对外泄面**毫无影响**，只决定代理能不能按项目统计。真在意就只能换可信上游。

---

## 9. 常见失败

| 现象 | 先查 |
|---|---|
| 进程起不来 | 配置路径对不对；是不是只有 `keys.json`；JSON 是否合法；权限是否 0600；`schemaVersion` 是否为 6（旧文件应能自动迁移） |
| Claude Code 连不上 | `ANTHROPIC_BASE_URL` 是否指向当前 `host:port` |
| 401 | `authToken` 开了但客户端没带，或带错 |
| 400 `route_planning` | 没有任何入口的 `mappings` 声明该模型，或声明它的入口全部停用；空 `mappings` 等于不接模型 |
| Codex 连不上 | `base_url` 是否指向当前 `host:port`（常见带 `/v1`）；模型名是否写在 `mappings`；`authToken` 开了但没配成 API key |
| 用量统计突然断了 | 改过 `endpoints[].id` |
| Linux 管理页 Failed to fetch | 没登录，或 Admin 不在 `57879`，或 daemon 没起来 |
| 上游 401/403 探测 | 部分中转要完整客户端指纹；菜单栏/WebUI 的「获取模型」和真实对话不是一回事 |

不要把真实 key 贴进 issue、聊天或 git。

---

## 10. Agent 操作清单（帮用户配置时）

1. 问清：macOS 还是 Linux；要接 Claude Code、Codex 还是两者。
2. 定位配置文件路径；没有就从 example 复制，不要用仓库里的 example 当生产文件原地填 key。
3. 向用户要：每个 Provider 入口的 baseURL + key，以及客户端实际会发的模型名。
4. 写入 `config.json`，确认 `schemaVersion: 6`、顶层是 `endpoints`（没有 `pools`），且至少一条入口为 `enabled: true` 并带覆盖客户端模型的 `mappings`。
5. 告诉用户怎么设 `ANTHROPIC_BASE_URL`（以及可选 `ANTHROPIC_AUTH_TOKEN`）。接 Codex 时给一份 `~/.codex/config.toml` 的 `model_providers` 片段。
6. **不要**把 key 写进回复；**不要** `git add` 配置；**不要**改源码。
