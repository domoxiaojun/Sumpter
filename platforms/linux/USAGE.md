# Sumpter使用指南

给第一次安装并接入客户端的用户。当前版本 **0.4.8**，配置 **schema v7**。

**范围**：从 GitHub 安装、填写 `config.json`、接入 Claude Code / Codex / Grok Build / Gemini CLI / pi、项目归因、常见错误。  
**不包含**：改源码、编译、发版。


<!-- BEGIN SUMPTER_CANONICAL_ONBOARDING -->

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
| macOS App | `~/Library/Application Support/Sumpter/config.json`（仅在 macOS 客户端使用） |
| Linux daemon | 普通用户：`$XDG_CONFIG_HOME/sumpter/config.json`，否则 `~/.config/sumpter/config.json`；system 安装：`/var/lib/sumpter/config.json` |

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

<!-- END SUMPTER_CANONICAL_ONBOARDING -->

两端当前使用 **schema v7** 的 `config.json`。启动或热重载时自动迁移旧的 schema v3 / v4 / v5 / v6；更早的
格式和旧 `keys.json` 不会被读取，可留作人工转换的草稿。迁移会先创建带时间戳的
`config.before-schema-v7-*.json` 原始备份，成功后才原子替换配置。

本文是 Linux 发布包的使用指南。源码 monorepo 中它位于 `platforms/linux/USAGE.md`；发布阶段会把
`platforms/linux/` 提升为包根，届时本文与同目录 `README.md` 位于发布包根目录。开箱模板的真源是
仓库根 `docs/templates/usage-onboarding.md`，由 `scripts/maintenance/sync-usage-docs.py` 同步到仓库根 `USAGE.md` 和本文件。

## 模型组调度

需要多个 OpenAI、Claude、Gemini、Grok 等入口共用一个客户端地址时，在“模型组”中声明模型范围，再把入口库中的连接绑定到一个或多个组。客户端继续使用原模型名；基础候选依次按组优先级、组顺序、入口优先级和入口顺序排列。实际调度保留会话粘性与冷却规则；允许故障切换时继续尝试后续组中的同一模型，不自动更换模型。

入口库负责地址、密钥和原始映射，模型组只负责模型范围与入口绑定。原有 HTTP 500 重试、跨轮重试、冷却、退避、`Retry-After`、会话粘性、超时、raw 透传和 Live/Realtime/Video 资源绑定继续由全局调度器处理。绑定的 `models` 省略或为 `null` 表示全部组内模型，`[]` 表示不承接；`overrides` 可为某个精确模型设置 `upstreamModel` 与 `priority`。

## 文档怎么读

| 你想做什么 | 读哪份 |
|---|---|
| 开箱、接客户端、归因、排错 | 本文 |
| 每个配置字段的含义 | 本文第 5 节（schema v7）；macOS 字段完整说明仅在源码树的 `docs/configuration.md` |
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

Linux 首次 Admin 用户名是 `kkl`，初始密码在同目录 `admin-password`，只在可信本地终端读取；Docker 首次密码见 `docker compose logs init`。改密后该文件会变成 Argon2 哈希 JSON，不能再回读明文。

---

## 2. 安装

### 2.1 macOS

从 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest) 下载 `sumpter-macos-*.dmg`。打开 DMG，双击「安装 Sumpter.command」，按提示确认。安装器只处理旁边的 `Sumpter.app`，只去掉这个 App 的 quarantine 标记，不会关闭全局 Gatekeeper。

不想用安装器时，把 App 拖到「应用程序」，再在 Finder 里右键 → 打开。不要用 `spctl --master-disable`，也不要对整个磁盘执行 `xattr -dr`。macOS 安装器的完整说明在源码树 `platforms/macos/app/INSTALL.txt`；Linux 发布包不包含该安装器。

### 2.2 Linux

从 GitHub Release 安装（按架构取包并校验 SHA-256）：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/install.sh
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

`--repo` 只指定下载来源，原来的安装选项仍可一起用，例如 `--admin-host 0.0.0.0`、`--admin-port 57879`、`--admin-password-file /绝对路径`；也可用环境变量 `SUMPTER_ADMIN_HOST` / `SUMPTER_ADMIN_PORT` / `SUMPTER_ADMIN_PASSWORD_FILE`。钉死版本时加上 `--version vX.Y.Z`。

```bash
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter --admin-host 0.0.0.0
sudo bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter --admin-host 0.0.0.0 --admin-port 57879
```

`sudo` 会装成 system 服务（daemon 仍以低权限 `sumpter` 用户运行）。已有 `config.json` 与 `admin-password` 不会被覆盖。

已解压本发布包时，在包内运行 `./scripts/install.sh`。Docker、systemd、Admin HTTPS 反代、卸载见 [`README.md`](README.md)。

### 2.3 卸载

卸载默认停用服务并删除程序、上一版本和 unit，保留 `config.json`、登录凭据与统计数据。已安装包内直接运行：

```bash
~/.local/share/sumpter/scripts/uninstall.sh        # 普通用户安装
sudo /opt/sumpter/scripts/uninstall.sh            # root / sudo 安装
```

包内脚本已不在时，用引导卸载器按当前身份调用同一个卸载器：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-uninstall.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/bootstrap-uninstall.sh
bash /tmp/sumpter-uninstall.sh        # system 安装改用 sudo bash /tmp/sumpter-uninstall.sh
```

只有明确要永久删除 `config.json`、`admin-password`、`runtime.sqlite3`、`stats.json` 和运行数据时，才在所选命令后加 `--purge`。macOS App 的卸载不在本发布包范围内，本发布包只含 Linux 组件。完整布局与参数见 [`README.md`](README.md)。

---

## 3. 最小开箱

目标：Claude Code / Codex 的请求由本机代理按入口映射、优先级和粘性分组调度到多个上游。

### 3.1 从 config.example.json 复制出 config.json

`config.example.json` 是**结构模板**，不是能直接跑的配置：域名全是 `.invalid`、入口全部
`enabled: false`、secret 是合成的 `sk-test-…`。先复制它、再替换一处真实入口，比从零手写 JSON
稳妥得多。

配置目录由运行方式决定，先认准自己那一行：

| 运行方式 | 配置目录 | 说明 |
| --- | --- | --- |
| 普通用户安装（XDG） | `$XDG_CONFIG_HOME/sumpter`；未设置时 `~/.config/sumpter` | 安装器与 WebUI 都用这个目录 |
| `sudo` 安装的 system 服务 | `/var/lib/sumpter` | 安装器显式指定，不要用 root 的 `$HOME` |
| 源码开发实例 | 任意仓库外目录，下文用 `/tmp/sumpter-dev` | 避免与已安装服务的端口冲突 |
| Docker 独立部署 | 宿主机 `config/`（容器内 `/config`） | **不要抄模板**，见本节末尾 |

普通用户安装：

```bash
config_base="${XDG_CONFIG_HOME:-$HOME/.config}"
install -d -m 700 "$config_base/sumpter"
install -m 600 ./config.example.json "$config_base/sumpter/config.json"
```

`sudo` 安装的 system 服务：

```bash
sudo install -d -m 700 /var/lib/sumpter
sudo install -m 600 ./config.example.json /var/lib/sumpter/config.json
```

源码开发只适用于完整 monorepo，步骤见仓库的开发指南；Linux 发布包没有 Rust workspace。手动运行 daemon 时必须先准备非空 `admin-password`。

上面三个命令引用的 `config.example.json` 是同一份内容：发布包根目录与仓库根的模板一致。

Docker 新部署不需要复制配置模板或准备密码。下载一份 Compose，启动时会在 `./config` 生成缺失的配置与初始密码：

```bash
mkdir -p sumpter/config && cd sumpter
curl --proto '=https' --tlsv1.2 -fLo compose.yaml \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/compose.yaml
docker compose up -d
docker compose logs init
```

管理页为部署主机的 `http://127.0.0.1:57879/admin/`，用户名 `kkl`，密码看 init 日志。默认只允许本机访问；远程管理可用 SSH 隧道。登录后配置入口映射和模型组。镜像、宿主机端口、数据目录与日志均直接编辑 Compose；容器内代理保持 `0.0.0.0:57878`。已有配置不会被覆盖，迁入配置时必须核对监听地址。详细步骤见 [Docker 部署说明](DOCKER.md)。

非 Docker 安装复制模板后，按下面顺序配置：

1. 留一个真实 Provider 入口，填它的 `baseURL`、`apiKey`，把 `enabled` 改成 `true`。
2. 在该入口的 `mappings[]` 里添加客户端模型名；若保留模板的 `modelGroups`，还必须启用对应组和绑定，并将同一模型加入组内范围。只启用入口不会自动启用模型组。
3. 本机自用保持 `listener.host: "127.0.0.1"`、`authToken: ""`、`allowedCIDRs: []`；要限定来源再动这三项。
4. `schemaVersion` 保持 `7`；新入口的 `protocol` 用 `auto`。
5. 启动服务：用户安装用 `systemctl --user start sumpter`，system 安装用 `sudo systemctl start sumpter`。

字段逐项含义见本文第 5 节；只想要最小骨架也可以直接抄第 6 节。

**复制模板时别做这几件事：**

- 不要把 Rust / 桌面版的 `config.toml` 改名成 `config.json`，daemon 只解析 schema v7 JSON，TOML 内容会让它在 Admin 端口启动前退出。
- 不要把权限放宽：`config.json` 含明文 `apiKey`，必须 `0600`，所在目录必须 `0700`。
- 不要在起服务之前指望 WebUI 报语法错：JSON 不合法时 daemon 直接退出，页面只会显示“Failed to fetch”。可以先自查 `python3 -m json.tool "$config_base/sumpter/config.json"`。
- 不要把填好的 `config.json` 或真实 key 提交进仓库；仓库里的模板只放停用的合成入口。

没有 `config.json`、只有旧 `keys.json` 时，进程会拒启，且不会改你的旧文件。如果配置目录里只有
`admin-password`，daemon 会自行创建一份空的 schema v7 bootstrap 配置；先用模板装一份的好处是
能直接看到字段结构和那些停用的合成入口。

**模型必须写在入口的 `mappings` 里。** 普通模型路由只使用声明了该模型的入口；配置模型组时还要满足组和绑定范围。显式固定入口的分流规则与已有资源绑定另有规则，见下文。旧 schema v5 的池级 `globalModels` 只在迁移时复制到当时还没有显式映射的入口，现行配置里已经没有这个字段。

### 3.2 接 Claude Code

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878
# 若 listener.authToken 非空，必须完全一致：
export ANTHROPIC_AUTH_TOKEN='填 config 里的 authToken'
```

模型名必须能被已启用入口的 `mappings.clientPattern` 接住（精确名或 `prefix-*` 通配），并在已配置模型组的启用范围内。没有任何入口声明该模型时返回 **400**。

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

`endpoints[].protocol` 参与协议候选选择与转换，五种取值和路径说明如下。

---

## 4. 协议与路径

入口协议为 `auto`、`anthropic`、`openai`、`openai-responses`、`gemini` 五态。`auto` 根据入站协议选择原生转发；固定协议参与候选选择。SourceFormat 与 TargetFormat 相同时使用 Native Adapter，异协议时只有已注册的 Translator 才能转换；不能无损表达的请求会被拒绝，不能假设任意请求都能转换。

| 路径族 | 用途与范围 |
| --- | --- |
| `/v1/messages` | Anthropic Messages |
| `/v1/chat/completions` | OpenAI Chat Completions |
| `/v1/responses` | OpenAI Responses，含流式响应和 WebSocket |
| `/v1/responses/compact` | 原生 Responses Compact |
| `/v1/completions` | 原生 Legacy Completions |
| `/v1/messages/count_tokens` | Claude Token 计数，不纳入生成用量汇总 |
| `/v1/images/generations`、`/v1/images/edits` | 图片生成与编辑，支持的能力由上游决定 |
| `/v1/alpha/search` | 原生 Alpha Search |
| `/v1beta/models/{model}:generateContent`、`:streamGenerateContent?alt=sse` | Gemini Developer API 原生 REST |
| `/v1/files`、`/v1/videos` 及资源子路径 | 文件、视频与后续资源访问 |
| `/v1/live`、`/v1/realtime`、`/v1/realtime/calls` | 按方法、模型和资源标识识别 Live / Realtime 意图 |

普通模型候选是入口映射、启用模型组和绑定模型范围的交集；未配置模型组时使用入口映射。原生转发保留所支持路径的请求、响应和流，按配置完成鉴权与必要的上游模型替换。Host、连接级 Header、入站凭据和 Sumpter 私有归因 Header 不原样转发。异协议转换会构造目标协议的请求和响应，因此不能将原生透传的字节保留承诺套用于 Translator。

`GET /v1/models`（及支持的模型目录别名）按当前可路由模型生成本地目录，不等于入口“获取模型”拉回的上游 catalog。普通客户端收到 `{object:"list",data:[...]}`；Codex 带 `client_version` 时收到 `{models:[...]}`。上游 catalog 仅辅助配置，获取成功不等于该模型已经启用。

Gemini 入口使用 `gemini` 或 `auto`。Provider 的 key 默认发送为 `x-goog-api-key`；以 `Bearer ` 开头时使用 Authorization。Gemini CLI 包装器通过 `X-Sumpter-*` 显式声明项目和会话，Vertex、OAuth、Service Account 和 Code Assist 不在当前接入范围内。

Codex Live bootstrap 会按 Live 意图选择 `gpt-live-1-codex` 精确映射，普通文本模型不会被当成语音模型。Sumpter 不把 `/v1/realtime` 改为 `/v1/realtime/calls`，也不代替 CPA 封装 Quicksilver；会话模型映射和短期凭据绑定仍可能修改模型、会话字段。无 `call_id` 的 `GET /v1/realtime` 属于公开 Realtime WebSocket；带资源标识的后续请求按已记录的入口归属转发。

WebSocket 先完成上游握手，再向客户端返回 `101`；握手失败保留相应 HTTP 错误。短期 `ek_…` 凭据的会话配置可在建连后通过 `session.update` 发给上游，不能描述为所有帧均无条件不解析。模型权限、媒体、Live/Realtime 能力及端到端响应仍须使用真实 Provider 验证。

---

## 5. 配置文件怎么填

顶层包括 `schemaVersion`、`listener`、`retry`、`endpoints`、可选 `modelGroups`、`sessionStickyTtlHours` 和 `featureRules`。
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

Linux Web Admin 独立于此字段，默认 `127.0.0.1:57879`，可由启动参数覆盖；Docker 宿主机地址由 Compose 的端口发布配置决定。

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

未配置模型组时，入口按 `priority` 从小到大调度；配置模型组后使用组与绑定的优先级，同级默认保持数组顺序；同一 `stickyGroup` 内的线路连续尝试。已有稳定会话优先复用原分组，发生可重试故障后再 failover 到其它分组。

模型组可将 `schedulingStrategy` 设为 `randomSticky`，让新会话在同优先级入口调度组中随机
选择首选并保持后续请求粘性；省略或设为 `priority` 时保持按顺序调度。随机策略不改变更高
优先级入口、故障切换和重试规则。

设为 `roundRobinSticky` 时，新会话按顺序取得同优先级入口调度组，分配后仍保持会话粘性；
轮询游标只存在于当前进程，重启或重新加载配置后从配置顺序重新开始，已有有效会话归属保留。

入口必填：

| 字段 | 注意 |
|---|---|
| `id` | 唯一。改 id 会让用量统计断成两截 |
| `baseURL` | 上游根，不要抄错路径 |
| `apiKey` | 明文，仅本机文件 |
| `protocol` | 必填五态：`auto`（默认）/ `anthropic` / `openai` / `openai-responses` / `gemini` |
| `enabled` | `false` 的入口不参与 |
| `priority` | 非负整数，越小越优先；同级按数组顺序；`0` 可不写 |
| `mappings` | **必须**声明本入口承接的客户端模型；空数组 = 不接任何模型 |
| `stickyGroup` | 空 = 用 id 当独立组；同名组共享会话 |

可选：`keepAlive`（省略 = 关闭；界面里新建入口默认打开）、`catalog`（「获取模型」缓存，纯展示，不参与路由）。

### 5.4 mappings[]

- `clientPattern`：客户端发来的模型名（可通配）
- `upstreamModel`：空 = 同名转给上游
- `thinking` / `context`：保留在配置中的兼容字段；raw 透传不会据此改写客户端 body 或 header
- `failoverTimeoutSeconds`：可选的映射级首响应截止；与全局 `responseTimeoutSeconds` 同时配置时取较小值，到期且尚未收到响应时尝试下一个入口

同一入口同时命中精确模型名和 `prefix-*` 通配时，精确映射优先；同级按配置顺序。

Codex Desktop 语音需要在可用 Provider（例如 CPA）额外声明
`{"clientPattern":"gpt-live-1-codex","upstreamModel":"gpt-live-1-codex"}`。
`/v1/live` 会固定按这个 Live 模型选入口，不会继承当前文本会话的模型。
Live/Realtime 不使用 `*` 通配 mapping，也不会选择 Anthropic 文本入口；未声明精确
Live mapping 时返回 `no_live_provider`，避免语音请求误发到普通模型（例如 `claude-fable-5`）。
即使全局 `responseTimeoutSeconds` 保持 `null`，原生 Live/Realtime 启动仍有 15 秒响应头保护；
当前入口候选耗尽后直接结束，不会按普通请求的无限跨轮策略继续挂起。

### 5.5 featureRules — 分流（可先全关）

模板里三条内建规则默认 `enabled: false`：

- `websearch` / `webfetch` / `classifier`：只识别 Claude Code 那种独立子请求的完整形状，不会扫主会话全文。

分流规则先选择目标模型和入口；实际协议处理遵循第 4 节。`target.effort` 可覆盖出站推理等级，`target.protocol` 参与目标协议选择，不是纯展示字段。

启用时设置 `enabled: true` 和 `target.model`。`endpointID` 显式固定入口时会绕过普通映射筛选；被停用或删除时才退回候选入口序列，未固定时目标模型需可路由。前端会清理删除入口后的悬空引用。

开箱可以全部保持关闭，先保证主对话能通。`featureRules` 也可以写成 `[]`（进程会补三条内建规则，默认停用）。

---

## 6. 一份可抄的骨架

此骨架省略 `modelGroups`，使用入口映射路由。把 URL、key、模型名换成你的；`.invalid` 域名不能用于真实流量。若改用完整模板，需额外启用模型组与绑定。

```json
{
  "schemaVersion": 7,
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
    "maxRetryDurationSeconds": 0
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

更完整的 Linux 模板见 `config.example.json`；macOS 模板和字段逐项说明只在源码树 `platforms/macos/config.example.json` 与 `docs/configuration.md` 提供。

### 旧配置怎么迁

- schema v3 / v4 / v5 / v6 会在启动时迁到 v7，备份名为 `config.before-schema-v7-*.json`。
- 旧 `pools` 会按原顺序展平为顶层 `endpoints`；非主池入口会拿到更高的连续 `priority`。
- 旧池级 `globalModels` 只复制给当时 `mappings` 为空的入口，然后删除。
- 旧 `featureRules[].target.poolID` 删除；规则目标改走入口序列 / 可选 `endpointID`。
- 旧 `listener.inboundDialectPassthrough=true` 会把当时的入口改成 `protocol: "auto"`；为 `false` 或缺失时保留三种固定协议；缺少 `protocol` 的旧入口按历史默认迁为 `anthropic`。
- 迁移失败不会覆盖旧文件。已经是 v7 的配置不会再迁一遍。

---

## 7. 改完怎么生效

| 平台 | 做法 |
|---|---|
| macOS App | 设置里保存；需要时点重启。监听地址/端口变了会重绑 |
| Linux WebUI | 登录后改并保存 |
| Linux 手改文件 | 对进程 `SIGHUP`，或 `systemctl --user reload sumpter` / `sudo systemctl reload sumpter` |

---

## 8. 让 Claude Code / Grok Build 按项目统计（可选）

本节同样适用于 Codex CLI/TUI、Gemini CLI 和 pi。统计依赖客户端明确上送的项目证据：
Codex 部分请求自带 workspace metadata，但并非每条请求都有。缺少证据时保留“未识别项目”，
不会从提示词、消息正文或 provider 名称推测本地目录。

安装、检查和还原统一见前面的[客户端归因安装](#linuxmacos-客户端归因脚本统一安装)。
只需在运行客户端的主机下载 setup 脚本，所需 mjs 和 pi 扩展由它自动获取；本机 macOS App
可直接在“安全”或“帮助”页使用内置资源。Linux 发布包也带有同一 setup 脚本。

Claude、Grok、Gemini、Codex 的包装器按每次启动目录读取 Git 根目录（非 Git 目录使用当前目录）、
项目名、用户和 remote；Codex 支持 -C / --cd。安装后新开终端，通过对应客户端命令启动。
pi 包装器加载扩展，在请求时读取当前工作区和会话；安装或还原后从新终端重新启动 pi。

Codex Desktop 图形进程和已经运行的会话不会加载 shell 包装器；桌面的 system 等内部请求
可能没有项目上下文。请在新请求的运行详情中核对项目、来源和会话，安装状态不代表已经观察到归因。

归因字段使用 X-Sumpter-Project、X-Sumpter-Workspace、X-Sumpter-Git-Remote 和
X-Sumpter-User。带工作区时显示“本地项目”，只有项目名时显示“客户端声明”；这些专用头
在上游转发前剥离。Git remote 会移除凭据、query 和 fragment，中文路径用 uri-v1 编码。
统计使用脱敏后的工作区路径；Codex 的 sourceWorkspacePaths 及启用的诊断捕获可能保留原始路径，
不能把界面脱敏理解为所有存储都不含绝对路径。客户端正文中的工作目录也不由归因脚本清除。

---

## 9. 常见失败

| 现象 | 先查 |
|---|---|
| 进程起不来 | 配置路径对不对；是不是只有 `keys.json`；JSON 是否合法；权限是否 0600；`schemaVersion` 是否为 7（旧文件应能自动迁移） |
| Claude Code 连不上 | `ANTHROPIC_BASE_URL` 是否指向当前 `host:port` |
| 401 | `authToken` 开了但客户端没带，或带错 |
| 400 `route_planning` | 启用的模型组未声明该客户端模型，或组内没有启用入口；旧配置则看入口 `mappings`。空 mappings / 空模型组都不接模型 |
| Codex 连不上 | `base_url` 是否指向当前 `host:port`（常见带 `/v1`）；模型名是否同时满足入口映射和已配置模型组的范围；`authToken` 开了但没配成 API key |
| 用量统计突然断了 | 改过 `endpoints[].id` |
| Linux 管理页 Failed to fetch | 没登录，或 Admin 不在 `57879`，或 daemon 没起来 |
| 上游 401/403 探测 | 部分中转要完整客户端指纹；菜单栏/WebUI 的「获取模型」和真实对话不是一回事 |

不要把真实 key 贴进 issue、聊天或 git。

---

## 10. Agent 操作清单（帮用户配置时）

1. 问清平台（macOS / Linux）和客户端（Claude Code、Codex、Grok Build、Gemini CLI、pi）。
2. 安装走 GitHub：macOS 下 Releases 的 DMG；Linux 用 `install.sh --repo domoxiaojun/sumpter`，需要时加 `--admin-host`。
3. 定位配置文件路径；没有就从 example 复制，不要在仓库 example 里填真实 key。
4. 写入 `config.json`，确认 `schemaVersion: 7`、顶层是 `endpoints` 与可选 `modelGroups`（没有 `pools`），至少一条入口 `enabled: true`，客户端模型有入口映射；配置了模型组时还要启用对应组、模型和入口绑定。
5. 告诉用户对应客户端的 Base URL：Claude Code 用根地址；Codex 必须带 `/v1`；pi 要设 `X-Sumpter-Client: pi`。
6. 需要项目统计时，在**启动客户端的主机**处理，不要装到只跑 daemon 的 Linux。下载 setup 脚本后执行 `bash setup-client-attribution.sh install all`。
7. **不要**把 key 写进回复；**不要** `git add` 配置；**不要**改源码。

#### Gemini CLI 客户端 wrapper

Gemini 已纳入统一安装器。本发布包内已有包装脚本，也可以从仓库取最新副本：

```bash
# 包内直接使用
node scripts/gemini-sumpter-wrapper.mjs --model gemini-2.5-pro

# 或从仓库下载
curl --proto '=https' --tlsv1.2 -fLo gemini-sumpter-wrapper.mjs \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/gemini-sumpter-wrapper.mjs
export SUMPTER_GEMINI_BASE_URL='http://127.0.0.1:57878'
export SUMPTER_AUTH_TOKEN='替换为 Sumpter 入站 Token'
node gemini-sumpter-wrapper.mjs --model gemini-2.5-pro
```

wrapper 会让 Gemini CLI 使用 Developer API Gateway（`GOOGLE_GEMINI_BASE_URL` + `GEMINI_API_KEY`），把新会话 UUID 通过 CLI 的 `--session-id` 与 `X-Sumpter-Session-Id` 同时固定，并声明安全的 `X-Sumpter-Project`。使用 `--resume`、`--session-file` 或显式 `--session-id` 时，wrapper 不猜测会话。它会清除 Vertex/GCA/ADC 环境变量；本接入不支持 Vertex、OAuth、Service Account 或 Code Assist/Cloud Code 协议。
