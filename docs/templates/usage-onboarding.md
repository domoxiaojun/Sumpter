## pi 客户端

pi 使用现有代理协议入口。编辑 `~/.pi/agent/models.json`，将以下 provider 合并到已有
`providers`，不要覆盖其他配置。`id` 填模型组已启用的客户端模型名：

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

`SUMPTER_API_KEY` 设置为 Sumpter **入站 Token**；入站未启用认证时可用非空占位值 `sumpter`，
以满足 pi 的模型可用性检查。使用 `pi --provider sumpter --model your-enabled-model` 启动；
编辑后打开 `/model` 可重新加载配置。模型的上下文长度、输出上限和 reasoning 能力按实际模型填写。

| pi 的 `api` | Sumpter `baseUrl` 示例 |
|---|---|
| `openai-responses` / `openai-completions` | `http://127.0.0.1:57878/v1` |
| `anthropic-messages` | `http://127.0.0.1:57878` |
| `google-generative-ai` | `http://127.0.0.1:57878/v1beta` |

同一代理地址下可以配置多个 provider，分别选择协议；上游入口须能承接对应协议。
Codex 专用请求沿用现有入口和认证配置，本扩展不替代 pi 登录或修改 OAuth。

### pi 项目与会话归因扩展

在**运行 pi 的主机**通过统一安装器配置 zsh/bash wrapper，私有扩展自动安装和加载，
无需手动安装到 pi 全局目录。扩展要求当前 pi 支持 `before_provider_headers`。wrapper 负责加载扩展，
不修改 provider 配置。每个连接 Sumpter 的 provider 必须设置 `headers: { "X-Sumpter-Client": "pi" }`。

资源位置：Linux 包的 `scripts/pi-project-attribution.ts`；macOS App 的
`Contents/Resources/pi-project-attribution.ts`；源码的 `scripts/clients/pi-project-attribution.ts`。

先临时加载验证（将路径替换为实际资源位置）：

```sh
pi -e /path/to/pi-project-attribution.ts --provider sumpter --model your-enabled-model
```

也可使用同目录的统一 wrapper，自动加载扩展并保留原始 Pi 参数：

```sh
node /path/to/client-attribution.mjs run pi -- --provider sumpter --model your-enabled-model
```

wrapper 与 `pi-project-attribution.ts` 需放在同一目录；可通过 `SUMPTER_PI_BIN` 指定 Pi 可执行文件。
无需手动导出项目或用户名环境变量；扩展在每次请求时读取当前目录、系统用户名和真实会话 ID。
扩展仅对显式标记 `X-Sumpter-Client: pi` 的 provider 添加归因；通过 wrapper 启动也不会给未标记的直连 provider 添加项目、用户或会话信息。

macOS：在「设置 → 安全」或「帮助」的归因面板选择 **pi**，点击「安装配置」。选择终端 Shell，App 使用内置安装器配置 wrapper 和私有扩展，显示 shell 配置路径；「还原配置」恢复该客户端的终端归因块。需要 Node.js 18+ 和 bash/zsh。

Linux：使用下节的仓库脚本，在运行 pi 的主机管理 shell wrapper 与配套扩展：

```sh
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh
bash setup-client-attribution.sh status pi
bash setup-client-attribution.sh install pi
bash setup-client-attribution.sh restore pi
```

pi 与其它客户端一样，默认按当前 bash/zsh 安装 shell wrapper，可用 `--shell zsh`（或 `bash`）指定。配套扩展保存在 `${XDG_DATA_HOME:-~/.local/share}/sumpter/attribution/pi-project-attribution.ts`，由 wrapper 通过 `pi -e` 加载，不再写入 `~/.pi/agent/extensions/`。首次安装前备份终端配置；还原仅处理该 wrapper，不修改 provider 凭据。私有资源作为共享缓存保留，避免影响另一种 shell 中仍在使用的 wrapper。安装或还原后新开终端并重新启动 pi；`/reload` 不会加载 shell 配置。

升级时，安装器仅迁移有旧版还原记录且内容仍匹配分发资源的全局扩展：恢复原始文件，或在原先没有文件时移除。用户修改过的文件、符号链接及无还原记录的文件保持原样；原始备份保留，界面提示已修改的旧版文件。

扩展按当前会话获取 ID、项目目录及本地用户名；Git 项目使用仓库根目录，普通目录使用当前目录。
恢复、分叉或切换会话后自动更新。Git remote 去掉用户名、密码、query 和 fragment 后才发送。
远程仓库优先使用 `origin`，没有 `origin` 时使用 Git 列出的第一个 remote。
中文路径通过带 `uri-v1` 标记的编码传输，Sumpter 解码后沿用现有归因清洗与本地存储规则。
所有 `X-Sumpter-*` 归因头在出站前剥离，原生请求体、认证与协议会话头不由扩展改写。

后台插件需通过 pi 的会话请求入口调用模型，才能继承归因钩子。当前本地配套修复为 pi 新增
`ctx.streamSimple`，并让 pi-observational-memory 的 Observer、Reflector、Dropper 使用此入口。
两份修复必须配套使用；原版 pi 0.85.1 尚无此入口，仅更新 Sumpter 扩展不能补齐插件后台请求。
配套插件上报 `memory` 角色与阶段名称；路由/缓存使用阶段独立 ID，归因仍使用所属 pi 会话 ID，不虚构父子会话关系。

验证时发一条请求，在“运行”检查客户端为 **pi**、项目及会话 ID 正确，再按 pi 筛选统计并查看会话导出。
统计中的提示基于当前视图：**已观察到归因 / 存在未归因请求 / 暂无可判定数据**；无流量不等于未安装。
未安装扩展时仍可根据 pi 原生身份头识别客户端；未上送的项目或会话保留“未识别”，不根据消息内容猜测。

## Linux/macOS 客户端归因脚本统一安装

Claude Code、Grok Build、Gemini CLI、Codex CLI/TUI、pi 共用 `client-attribution.mjs` 安装器。五个客户端都管理 shell 启动包装器，pi 的私有配套扩展随包装器自动安装和加载；客户端的原生启动参数与会话恢复参数保持不变。

必须在**启动客户端的主机**执行，不要装到只跑 Sumpter daemon 的 Linux 上。本机 macOS App 可在「设置 → 安全」或「帮助」选择客户端后点「安装配置」。其它机器从仓库下载，不要用 `sudo`：

```bash
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh

# 选择 claude、grok、gemini、codex、pi 或 all（全部五个客户端）
bash setup-client-attribution.sh status all
bash setup-client-attribution.sh install all
bash setup-client-attribution.sh restore all
bash setup-client-attribution.sh uninstall all
```

只需下载上述 setup 脚本，无需手动准备 mjs。脚本优先使用同目录资源；缺失时从 GitHub raw 自动获取所选客户端需要的安装器和扩展，下载失败时不执行安装。无法访问 GitHub 且代理已运行时，可改设 `SUMPTER_BASE_URL`（及入站 Token）从 `/__sumpter/` 下载。安装和还原后显示当前状态；`--shell bash|zsh` 与 `--rc 文件` 影响所有客户端，pi 自动安装 shell wrapper 和私有配套资源，无需另行安装全局扩展。

安装器会备份 shell rc，并只替换 Sumpter 管理的对应归因标记块；重复安装幂等，`uninstall` 删除所选块，`restore` 只恢复所选客户端安装前的旧块并保留其他改动。所有客户端安装、卸载、还原后新开终端并重新启动；pi 的 `/reload` 不会加载 shell 配置。

`pi install`、`remove`、`uninstall`、`update`、`list`、`config` 和 `auth` 是 Pi 自己的顶层命令。Sumpter 的 pi 包装器会原样透传这些命令，不会在前面插入扩展参数；例如 `pi install npm:@czottmann/pi-automode` 会进入 Pi 包管理器。`pi list` 查看 Pi 包，`status pi` 查看 Sumpter 归因配置，两者不是同一状态。

Claude 的 `settings.json` 中若写死 `env.ANTHROPIC_CUSTOM_HEADERS`，会覆盖启动时的动态值；
安装器会提示先移除该冲突。Grok 的 `GROK_CONFIG_PATH` 同样会触发冲突提示。
不要把项目路径写死在全局配置里；切换项目后应从对应目录重新启动客户端。

Codex CLI/TUI 使用与 Claude 相同的本地采集逻辑，支持 `-C / --cd` 和已有 profile。
安装器自动读取当前连接配置，仅在本次启动参数中挂载归因 header；不修改 Codex 配置文件、
模型或凭据，也不从 provider 名称判定项目。需先有指向 Sumpter 的自定义连接配置；
Codex 内置连接不支持这一注入方式。通过新终端的 `codex` 命令启动才会加载包装器，
Codex Desktop 和已运行的会话不会加载它；原生 workspace metadata 仍优先于脚本声明。

如只需临时启动，可使用发布包内的 `node /path/to/client-attribution.mjs run claude --`，
把 `claude` 换为对应客户端，`--` 后传原始参数；不修改 rc。pi 的临时启动需同目录带扩展，
具体见上面的 pi 说明。普通安装无需先做临时启动。

Gemini 运行前还需设置 `SUMPTER_GEMINI_BASE_URL` 和 `SUMPTER_AUTH_TOKEN`；统一入口会设置 Gemini 的 Base URL、认证方式和归因 header。显式 `--resume`、`--session-id`、`--session-file` 或 `--list-sessions` 时不生成新会话 ID，避免改变客户端恢复语义。

仅在已经连接 Sumpter 的客户端上启用；包装器随该客户端请求附加归因头，Sumpter 在上游转发前剥离。统计投影使用脱敏路径，Codex 源元数据和诊断捕获可能包含原始路径。Git remote 会删除凭据、query 和 fragment。

pi 扩展由统一安装器从同目录资源、GitHub 仓库 raw 或 listener 的 `/__sumpter/pi-project-attribution.ts` 获取；shell wrapper 加载私有扩展，扩展按 provider 的 `X-Sumpter-Client: pi` 标记决定是否添加归因，安装器不会修改 provider 凭据。

## 开箱路径（固定顺序）

第一次使用只做 3 件事：找到配置 → 配置入口与模型组→ 启动 Sumpter 并接入客户端。
备用入口、重试和功能规则等高级配置，等基本使用后再设置。

### 1. 找到配置文件

| 平台 | 配置文件 |
|---|---|
| macOS App | {{macos_config_path}} |
| Linux daemon | {{linux_config_path}} |

优先复制对应平台的 `config.example.json`，不要从零创建 JSON。`listener` 默认保持
`127.0.0.1:57878`，先保存配置，再启动 Sumpter。

### 2. 只填一个上游服务入口

在 `endpoints[]`（上游服务列表）中先只启用一个入口，填 `baseURL`（上游服务地址）、`apiKey`、
`enabled: true`。需要改上游模型名或设置模型参数时填写 `mappings`。例如：

```json
{ "clientPattern": "gpt-5.4", "upstreamModel": "gpt-5.4" }
```

然后在“模型组”启用默认组（或新建组），添加客户端使用的模型名，从入口库添加刚才的入口，选择“全部可用模型”或勾选指定模型后保存。示例文件的入口和默认组均停用，须分别启用。

手工配置对应 `modelGroups[].models` 与 `bindings`；`endpointID` 必须引用已有入口 ID：

```json
{"id":"main","name":"主用","enabled":true,"priority":0,"models":["gpt-5.4"],"bindings":[{"endpointID":"your-endpoint-id","enabled":true,"priority":0,"models":null}]}
```

组内模型名必须和客户端实际使用的模型名一致；不确定时先用精确名称。模型名相同的多个入口可绑定到同一组，也可按用途放到不同组；客户端地址不变。

配置多个入口后，同一会话默认粘在上次成功的入口组，时长由顶层 `sessionStickyTtlHours` 控制（单位小时，默认 72；`0` 表示永不过期）。想立即改走新顺序，在统计页对应项目行点「清除粘性归属」，或在入口库把粘性时长改为更短的值——默认组的入口顺序与优先级始终跟随入口库的列表顺序和 Priority。

需要同优先级入口按顺序分配新会话时，将模型组的 `schedulingStrategy` 设为 `roundRobinSticky`；
分配后的会话仍保持粘性，故障时继续按现有重试和故障转移规则处理。

### 3. 接入客户端

Claude Code：

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878
```

Codex：**Base URL 必须带 `/v1`**。

```toml
model = "gpt-5.4"                 # 改成你模型组里的模型名
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

<!-- 由 scripts/maintenance/sync-usage-docs.py 生成；请修改 docs/templates/usage-onboarding.md 与 docs/templates/usage-path-matrix.json 后同步。 -->
