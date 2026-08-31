# Sumpter使用指南

给使用者，以及要帮用户「装好、配好、连上客户端」的 LLM agent。

**范围**：安装入口、`config.json`、接 Claude Code / Codex、常见错误。  
**不包含**：改源码、编译、发版、Git 远程。


<!-- BEGIN SUMPTER_CANONICAL_ONBOARDING -->

## 开箱路径（固定顺序）

安装/启动 → 找到配置 → 启用入口与 mapping → 配置 Claude/Codex → 首个成功请求 → 查看请求链

这条顺序也是 Help 页的引导顺序。每个状态只给一个主要下一步，并只依据已有的
`status`、`config`、运行统计和请求事件判断；不会要求用户粘贴密钥或 raw 诊断。

### 状态引导

| 状态 | 现有数据的判断 | 唯一下一步 |
|---|---|---|
| 未启动 | `status.running` 不是 `true`（macOS 为 sidecar 未运行） | 启动代理 |
| 未配置 | 没有可读的 `config.json` 或没有入口 | 添加并保存 Provider 入口 |
| 无 mapping | 有入口，但没有启用入口包含 `endpoints[].mappings[]` | 为客户端模型添加 mapping |
| 客户端未接入 | 有可用 mapping，但 `clientRequests == 0` | 配置 Claude/Codex 的 Base URL |
| 首次失败 | 已有客户端请求，但 `clientSuccesses == 0` 且出现失败 | 打开运行页查看请求链和失败阶段 |
| 首次成功 | `clientSuccesses > 0` | 查看成功请求链与后续 failover |

### 路径矩阵

| 项目 | macOS App | Linux daemon / 发布包 |
|---|---|---|
| 配置文件 | `~/Library/Application Support/Sumpter/config.json` | 普通用户：`$XDG_CONFIG_HOME/kekulv/config.json`，否则 `~/.config/kekulv/config.json`；system 安装：`/var/lib/kekulv/config.json` |
| 数据面代理 | `http://127.0.0.1:57878`（以运行页/配置为准） | `http://127.0.0.1:57878`（以 `listener` 为准） |
| Admin 地址 | sidecar 握手返回的本机动态端口（App 内部使用） | `http://127.0.0.1:57879/admin/`（可由启动参数覆盖） |
| 入站鉴权 | `listener.authToken`（非空才启用） | `listener.authToken`（非空才启用） |
| Admin 鉴权 | App 与 sidecar 的本机控制通道 | Admin session cookie + CSRF；不要把密码写入文档或 Issue |
| 模型 mapping | `endpoints[].mappings[]` | `endpoints[].mappings[]` |
| 配置字段参考 | [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md) | [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md) |
| 项目归因脚本 | 源码树 `platforms/linux/scripts/cc-project-attribution.sh`；发布包按其 USAGE 路径 | 源码树 `platforms/linux/scripts/cc-project-attribution.sh`；发布包按其 USAGE 路径 |

本 monorepo 的 canonical 文档是 [`USAGE.md`](USAGE.md)；Linux 发布输入位于 `platforms/linux/`，发布包构建时会将其提升为包根。

### 客户端接入与安全边界

- Claude Code 使用 `ANTHROPIC_BASE_URL`；Codex / OpenAI 兼容客户端使用 API Base，具体协议和路径见下文。
- 代理会先做 CIDR 与入站鉴权，再读取请求体；不要用大 body 测试错误 token。
- `apiKey`、`authToken`、Admin 密码、Cookie、请求体和 raw 捕获都只留在本机受限文件中；文档、截图和 Issue 只放脱敏后的请求 ID、时间和错误阶段。
- 项目归因脚本必须运行在**启动 Claude Code 的客户端机器**上，而不是远程 daemon 所在机器；每台客户端机器单独配置。

### macOS 统一通知（Claude Code + Codex CLI）

- macOS App 的「通知」页分别管理 Claude Code Hook 与 Codex CLI Stop Hook；两者都由 Sumpter 的 `/__notify` 接收并投递系统通知。
- Codex 默认关闭。启用后 App 会在 `CODEX_HOME/hooks.json`（未设置时为 `~/.codex/hooks.json`）写入 `Stop` Hook，并把脚本放在同目录的 `hooks/sumpter-codex-notify.zsh`；脚本只转发 Stop JSON，最多等待 2 秒，Sumpter 未运行也不会阻断 Codex。
- 启用时会直接移除 Codex 顶层 `config.toml` 中已知的 `SkyComputerUseClient … turn-ended` legacy `notify`，不生成备份、不自动恢复。无法安全识别的自定义 `notify` 会保留并显示冲突，需手动删除后再启用。
- 写入后在 Codex CLI 执行 `/hooks` 并信任 Sumpter Hook；设置页只有收到一次真实 Codex SSE 通知后才显示「已验证」。通知使用固定安全文案，不包含 transcript、完整 prompt、`last_assistant_message` 或原始错误详情。
- Linux daemon 不提供通知 Hook，也不提供 `/__notify`；上述统一通知仅适用于 macOS App。

<!-- 由 scripts/sync-usage-docs.py 生成；请修改 docs/usage-onboarding.md 与 docs/usage-path-matrix.json 后同步。 -->

<!-- END SUMPTER_CANONICAL_ONBOARDING -->

两端当前使用 **schema v6** 的 `config.json`。启动或热重载时自动迁移旧的 schema v3 / v4 / v5；更早的
格式和旧 `keys.json` 不会被读取，可留作人工转换的草稿。迁移会先创建带时间戳的
`config.before-schema-v6-*.json` 原始备份，成功后才原子替换配置。

源码仓库 [`domoxiaojun/sumpter`](https://github.com/domoxiaojun/sumpter) 是这份 monorepo。发布阶段会把
`platforms/linux/` 提升为 Linux 发布包根，因此发布包里的指南位于包根；源码树里的 Linux 指南是
[`platforms/linux/USAGE.md`](platforms/linux/USAGE.md)。改开箱模板时编辑 `docs/usage-onboarding.md`，
由 `scripts/sync-usage-docs.py` 同步根 `USAGE.md` 与 Linux 那一份。

## 文档怎么读

| 你想做什么 | 读哪份 |
|---|---|
| 产品定位与仓库地图 | [`README.md`](README.md) |
| 开箱、接客户端、排错 | 本文 |
| 每个配置字段的含义 | [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md)（两端同一份 schema） |
| Linux 安装、systemd、Docker、Admin 反代 | [`platforms/linux/README.md`](platforms/linux/README.md) |
| macOS 首次打开被拦截 | [`platforms/macos/app/INSTALL.txt`](platforms/macos/app/INSTALL.txt) |
| 改代码 / 架构 | [`docs/architecture.md`](docs/architecture.md)、[`AGENTS.md`](AGENTS.md) |

Agent：只改**本机配置目录**里的 `config.json`，不要把填好 key 的文件提交进 git，不要读取后写进聊天记录。

---

## 1. 先搞清你在用哪一端

| | macOS App | Linux daemon |
|---|---|---|
| 配置文件 | `~/Library/Application Support/Sumpter/config.json` | 普通用户：`$XDG_CONFIG_HOME/kekulv/config.json`，否则 `~/.config/kekulv/config.json`。system 安装：`/var/lib/kekulv/config.json` |
| 权限 | `0600` | `0600` |
| 代理端口 | `listener.port`，默认 `57878` | 同左 |
| 管理界面 | 菜单栏 App 设置窗 | 浏览器 `http://127.0.0.1:57879/admin/` |
| 改配置后 | App 里保存 / 重启 | WebUI 保存，或 `SIGHUP` 热重载 |

模板：

- **推荐抄** `platforms/linux/config.example.json`：域名是 `.invalid`，入口全部 `enabled: false`，secret 是合成的 `sk-test-…`。
- `platforms/macos/config.example.json` 更像本机草稿，可能带习惯用的上游域名、且入口是启用的。复制后务必换成你的地址和 key，不要原样拿去打真实流量。

Linux 首次 Admin 用户名是 `kkl`，密码在同目录 `admin-password`（不要在终端 `cat` 出去）。改密后该文件会变成 Argon2 哈希 JSON，不能再回读明文。

---

## 2. 安装

### 2.1 macOS

打开 DMG，双击「安装Sumpter.command」，按提示确认。安装器只处理旁边的 `Sumpter.app`，只去掉这个 App 的 quarantine 标记，不会关闭全局 Gatekeeper。

不想用安装器时，把 App 拖到「应用程序」，再在 Finder 里右键 → 打开。不要用 `spctl --master-disable`，也不要对整个磁盘执行 `xattr -dr`。细节见 [`platforms/macos/app/INSTALL.txt`](platforms/macos/app/INSTALL.txt)。

### 2.2 Linux

静态镜像一键安装（按架构自动取包）：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/kekulv-install.sh https://sf.domob.org/kkl/kekulv-install.sh
bash /tmp/kekulv-install.sh
```

`sudo bash /tmp/kekulv-install.sh` 会装成 system 服务（daemon 仍以低权限 `kekulv` 用户运行）。已有 `config.json` 与 `admin-password` 不会被覆盖。

Docker、systemd、Admin HTTPS 反代、卸载见 [`platforms/linux/README.md`](platforms/linux/README.md)。

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

把客户端的 API Base 指到 `http://127.0.0.1:57878`（或你改过的 host/port）。Codex 通常要带 `/v1` 后缀；没有 `/v1` 的别名也可以，因为代理同时认 `/responses` 和 `/v1/responses`。

`~/.codex/config.toml` 可以写成：

```toml
model = "gpt-5.4"                 # 必须能被某个入口 mappings 接住
model_provider = "kekulv"

[model_providers.kekulv]
name = "Sumpter"
base_url = "http://127.0.0.1:57878/v1"
wire_api = "responses"
```

其它 OpenAI SDK：

```bash
export OPENAI_BASE_URL=http://127.0.0.1:57878/v1
```

若 `listener.authToken` 非空，把它配成该客户端的 API key（Codex 用 provider 的 `env_key`，其它客户端用 `OPENAI_API_KEY`）。

入口协议能力在每个 `endpoints[]` 上单独声明，只有四个值：

| `protocol` | 含义 |
|---|---|
| `auto` | 自动（三协议），按入站路径选择 Anthropic、OpenAI Chat 或 OpenAI Responses；新入口默认值 |
| `anthropic` | 只声明 Anthropic Messages |
| `openai` | 只声明 OpenAI Chat Completions |
| `openai-responses` | 只声明 OpenAI Responses |

`auto` 是入口能力模式，不会作为出站协议发送给上游。普通对话没有规则目标覆盖时，入口按入站路径原生发送；固定协议与入站协议不同时才进入 Translator。

### 3.4 本机自检

```bash
curl --noproxy '*' http://127.0.0.1:57878/__status
```

Linux 还可看 Admin 是否起来：

```bash
curl --noproxy '*' -i http://127.0.0.1:57879/healthz
curl --noproxy '*' -i http://127.0.0.1:57879/admin/
```

---

## 4. 协议与路径

SourceFormat 只由路径决定，不再依据 User-Agent 或 body 形状猜测协议。请求路径与 body 结构不匹配时直接返回 `invalid_request`。User-Agent 只用于客户端识别、统计和 Provider 特殊 Header。

原生路径：`/v1/messages` 是 Anthropic，`/v1/chat/completions` 是 OpenAI Chat，`/v1/responses` 及 Codex Responses 别名是 OpenAI Responses。

规则里的 `target.protocol`（或 API 返回的 `protocolOverride`）仍只能写三个真实协议，不能写 `auto`。Auto 入口可按规则目标解析为指定协议，固定入口只有声明的协议匹配时才参与；没有安全的协议候选时返回 `NoCompatibleProtocol`。原生候选存在时，桥接候选不会混入本次重试列表。

当前 HTTP 支持面：

| 客户端路径 | 处理 |
|---|---|
| `/v1/messages` | Anthropic 原生 Adapter |
| `/v1/messages/count_tokens` | Claude Count Tokens 独立原生 Adapter；入口须为 `auto` 或 `anthropic` |
| `/v1/chat/completions` | OpenAI Chat 原生，或按固定目标协议安全桥接 |
| `/v1/responses` | OpenAI Responses 原生，或按固定目标协议安全桥接 |
| `/v1/responses/compact` | Responses Compact 独立原生 Adapter；入口须为 `auto` 或 `openai-responses` |
| `/v1/completions` | Legacy Completions 独立原生 Adapter；入口须为 `auto` 或 `openai` |
| `/v1/images/generations`、`/v1/images/edits` | 独立图片 Adapter；multipart body 与 boundary 保持原样 |
| `/v1/alpha/search` | Codex Alpha Search 独立原生 Adapter |

需要 Responses 才能执行的 Grok 检索只选择 `auto` 或 `openai-responses` 入口；固定 `openai` Chat 入口不会隐式升级。Translator 无法安全表达工具、reasoning、引用或未知内容块时会拒绝请求，不静默丢字段。

除 Claude `/v1/messages` 外，表中业务路径支持无 `/v1` 别名；Responses、Compact、Alpha Search 和 Images 还支持 `/backend-api/codex/...` 直连别名。原生 JSON 请求只重写路由后的 `model`，其它字段（包括 Grok 的 `aspect_ratio`、`resolution`、`tools`、`reasoning` 和未来未知字段）都会保留；Alpha Search 会按 Codex 行为移除 `prompt_cache_key` 与 `prompt_cache_retention`。multipart 编辑为保护 boundary 与二进制图片，body 和 `Content-Type` 字节级保持不变；未填图片模型时默认 `gpt-image-2`。

当前**不支持** Responses WebSocket、Realtime / Live、Videos 创建后的查询与下载、Files，以及 `/v1/models`。这些不能当作普通一次性 HTTP body 透传；命中会明确 404。

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
- `sessionStickyRetries` 默认 `2`：当前线路组失败后再整组多试 2 次，仍不行才换组
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
- `thinking`：`disabled` / `passthrough` / `adaptive`
- `context`：`standard`（不补不剥 1M）/ `oneMillion`（强制开 1M）/ `strip`（剥离 1M 标记）
- `failoverTimeoutSeconds`：可选的映射级首响应截止；与全局 `responseTimeoutSeconds` 同时配置时取较小值，到期且尚未收到响应时尝试下一个入口

同一入口同时命中精确模型名和 `prefix-*` 通配时，精确映射优先；同级按配置顺序。

### 5.5 featureRules — 分流（可先全关）

模板里三条内建规则默认 `enabled: false`：

- `websearch` / `webfetch` / `classifier`：只识别 Claude Code 那种独立子请求的完整形状，不会扫主会话全文。

Provider 不需要另填 WebSearch 能力字段。严格识别为 WebSearch 后，代理按最终目标协议自动选择：Anthropic 保留原生 `web_search`，OpenAI Chat 使用 `web_search_options`，OpenAI Responses 使用内建 `web_search`。Grok 检索仍只允许最终目标为 Responses。

要启用：设 `enabled: true`，`target.model` 必须能被某个入口的映射承接。`endpointID` 可选，钉住后只走该入口；入口失效时退回整列候选。`target.effort` 可选，支持 `none` / `auto` / `minimal` / `low` / `medium` / `high` / `xhigh` / `max`；省略时跟随客户端原请求。`target.protocol` 可选，只能写三个真实协议，不能写 `auto`。

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
    "responseTimeoutSeconds": null,
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

更完整的模板见 `platforms/macos/config.example.json` / `platforms/linux/config.example.json`。字段逐项说明见 [`platforms/macos/CONFIG.md`](platforms/macos/CONFIG.md)。

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
| Linux WebUI | 登录后改并保存（带 generation 对账） |
| Linux 手改文件 | 对进程 `SIGHUP`，或 `systemctl --user reload kekulv` / `sudo systemctl reload kekulv` |

手改 JSON 后语法坏了：进程拒载，**不会覆盖**你磁盘上的原文件。可先用 `jq empty config.json` 检查语法。

---

## 8. 让 Claude Code 按项目统计（可选）

统计页有「项目 Token 排行」。Codex 会自己上行工作区信息，天生分好项目；**Claude Code 不会**
——它的 `cwd` / `project_dir` 只给本机 statusLine 和 hook 用，不进发给代理的请求，所以默认
所有 CC 请求都堆在「未识别项目」里。会话维度不受影响：CC 无条件发 `X-Claude-Code-Session-Id`，
**会话统计零配置就有**，这里配的只是项目维度。

要分项目，就让 CC 把项目名随请求带上。Sumpter认三个入站 header：`X-Kekulv-Project` /
`X-Kekulv-Workspace` / `X-Kekulv-Git-Remote`（代理读完即从出站剥离，中转站看不到）。不用改 CC、
不用装东西——一个 shell 配置器按你**当前目录**自动生成这三个值。

> **在哪台机器配？** 在**跑 Claude Code 的那台机器**上，不是跑 daemon 的那台。daemon 常在远程
> 或容器里（比如你连的是 `192.168.0.8`），但归因 header 是 CC 进程的环境变量，只能在 CC 本地设。
> 每台跑 CC 的机器各配一次。

### 一键配置

配置器 `cc-project-attribution.sh` 随发布包分发（Linux 解包后在 `/opt/kekulv/scripts/`；macOS
打包进 App 内的 `Sumpter.app/Contents/Resources/`，从源码构建则在 clone 仓库的 `platforms/linux/scripts/`
下）。支持 zsh 与 bash，两个平台通用。

两个产品的**安全页面**都有一份完整引导（当前是否已生效、三步命令、平台差异、三个陷阱、
回退命令），命令可直接复制；macOS 那份还带「在 Finder 中显示」直接定位到脚本。

**先体检，再安装，最后新开终端验证**——三步：

```bash
# 1) 只读体检:看当前 shell、rc 路径、有没有 settings.json 覆盖陷阱
./cc-project-attribution.sh status

# 2) 安装(装前想预演就加 --dry-run)
./cc-project-attribution.sh install

# 3) 新开一个终端窗口,进任意项目发一条消息,去 Admin 统计页看「项目 Token 排行」
```

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
等价的 fish 配置，粘进 `~/.config/fish/config.fish`（该段未经实测，粘前先 `fish -n` 校验）。

### macOS 与 Linux 的差别

| | macOS | Linux |
|---|---|---|
| 谁在跑 CC | 本机菜单栏 App 旁边就是 CC | CC 可能在本机，也可能在别的机器上连远程 daemon |
| 配置器位置 | App 内 `Sumpter.app/Contents/Resources/`（源码构建则是 clone 仓库的 `platforms/linux/scripts/`） | 发布包解包后 `/opt/kekulv/scripts/` |
| 默认 shell | 通常 zsh | 视发行版，zsh 或 bash 都常见 |
| 看统计 | 菜单栏 App 的「统计」页 | Web Admin 的统计页（`http://<daemon>:57879/admin`） |

配置器两边命令完全一样，自己会探测 shell 与平台。

### 手工配置（不想用配置器时）

```bash
export ANTHROPIC_CUSTOM_HEADERS="X-Kekulv-Project: $(basename "$PWD")
X-Kekulv-Workspace: $PWD
X-Kekulv-Git-Remote: $(git remote get-url origin 2>/dev/null)"
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
- 项目来源标成「客户端声明」，和 Codex 的「本地项目」分开显示——这值是客户端自己说的，可信度不同。
- **这三个 header 被剥，不代表路径没外泄。** CC 每条请求的 body 里本来就带工作目录绝对路径、
  源项目开发规则与 `git status` 摘要，代理对 `system` / `messages` 一字不改地转发。配不配这三个
  header 对外泄面**毫无影响**，只决定代理能不能按项目统计。真在意就只能换可信上游。

---

## 9. 常见失败

| 现象 | 先查 |
|---|---|
| 进程起不来 | 配置路径对不对；是不是只有 `keys.json`；JSON 是否合法；权限是否 0600；`schemaVersion` 是否为 6（旧文件应能自动迁移） |
| Claude Code 连不上 | `ANTHROPIC_BASE_URL` 是否指向当前 `host:port`；本机 `curl __status` 通不通 |
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
6. 让用户跑 `curl --noproxy '*' http://127.0.0.1:57878/__status`。
7. **不要**把 key 写进回复；**不要** `git add` 配置；**不要**改源码。
