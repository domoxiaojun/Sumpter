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

在**运行 pi 的主机**安装 `pi-project-attribution.ts`。扩展要求当前 pi 支持
`before_provider_headers`，只为带 `X-Sumpter-Client: pi` 的 provider 追加归因；不要在直连
其他服务的 provider 上设置该标识。无需改 pi 源码或 shell 启动文件。

资源位置：Linux 包的 `scripts/pi-project-attribution.ts`；macOS App 的
`Contents/Resources/pi-project-attribution.ts`；源码的 `scripts/pi-project-attribution.ts`。

先临时加载验证（将路径替换为实际资源位置）：

```sh
pi -e /path/to/pi-project-attribution.ts --provider sumpter --model your-enabled-model
```

永久安装：

```sh
mkdir -p "$HOME/.pi/agent/extensions"
cp /path/to/pi-project-attribution.ts "$HOME/.pi/agent/extensions/pi-project-attribution.ts"
```

如目标文件已经存在，先备份再更新。重启 pi 或执行 `/reload`。
Linux listener 也提供受现有访问控制与入站认证保护的下载入口：

```sh
curl --fail --show-error \
  -H "Authorization: Bearer $SUMPTER_API_KEY" \
  http://127.0.0.1:57878/__sumpter/pi-project-attribution.ts \
  -o /tmp/pi-project-attribution.ts
```

将地址替换为实际 Sumpter 地址，再按上面的安装步骤复制下载文件。
移除扩展时只删除自己安装的文件，然后 `/reload`：

```sh
rm "$HOME/.pi/agent/extensions/pi-project-attribution.ts"
```

扩展按当前会话获取 ID、项目目录及本地用户名；Git 项目使用仓库根目录，普通目录使用当前目录。
恢复、分叉或切换会话后自动更新。Git remote 去掉用户名、密码、query 和 fragment 后才发送。
中文路径通过带 `uri-v1` 标记的编码传输，Sumpter 解码后沿用现有归因清洗与本地存储规则。
所有 `X-Sumpter-*` 归因头在出站前剥离，原生请求体、认证与协议会话头不由扩展改写。

验证时发一条请求，在“运行”检查客户端为 **pi**、项目及会话 ID 正确，再按 pi 筛选统计并查看会话导出。
统计中的提示基于当前视图：**已观察到归因 / 存在未归因请求 / 暂无可判定数据**；无流量不等于未安装。
未安装扩展时仍可根据 pi 原生身份头识别客户端；未上送的项目或会话保留“未识别”，不根据消息内容猜测。

## Linux/macOS 客户端归因脚本统一安装

Claude Code、Grok Build、Gemini CLI 共用 `client-attribution.mjs`。它们使用同一套项目名、工作区、用户、脱敏 Git remote 和 URI 编码规则；客户端的原生启动参数与会话恢复参数保持不变。Pi 仍使用 Pi 原生扩展机制。

macOS：在「设置 → 安全」的项目归因面板选择客户端与终端 Shell，自动显示本机配置状态；点击「安装配置」或「还原配置」，操作后自动复查。需要 Node.js 18+。

Linux：在运行客户端的主机执行，不要用 `sudo`。安装包内可直接运行 `bash scripts/setup-client-attribution.sh` 进入交互菜单，也可远程下载：

```bash
export SUMPTER_BASE_URL='http://127.0.0.1:57878'
export SUMPTER_AUTH_TOKEN='替换为 Sumpter 入站 Token'
curl --fail --show-error -H "Authorization: Bearer $SUMPTER_AUTH_TOKEN" \
  "${SUMPTER_BASE_URL%/}/__sumpter/setup-client-attribution.sh" \
  -o setup-client-attribution.sh

# 选择 claude、grok、gemini 或 all；自动安装支持 bash/zsh
bash setup-client-attribution.sh status all
bash setup-client-attribution.sh install all
bash setup-client-attribution.sh restore all
```

Linux 脚本自动获取配套安装器，安装和还原后显示当前状态；支持 `--shell bash|zsh` 与 `--rc 文件`。

安装器会备份 shell rc，并只替换 Sumpter 管理的对应归因标记块；重复安装幂等，`uninstall` 删除所选块，`restore` 只恢复所选客户端安装前的旧块并保留其他改动。安装、卸载、还原后新开终端。也可以不安装，直接临时运行：

```bash
node client-attribution.mjs run claude -- --help
node client-attribution.mjs run grok -- --help
```

Gemini 运行前还需设置 `SUMPTER_GEMINI_BASE_URL` 和 `SUMPTER_AUTH_TOKEN`；统一入口会设置 Gemini 的 Base URL、认证方式和归因 header。显式 `--resume`、`--session-id`、`--session-file` 或 `--list-sessions` 时不生成新会话 ID，避免改变客户端恢复语义。

三者都只把归因 header 发送到 Sumpter；代理解析后从发往上游的请求剥离。工作区路径只在 Sumpter 本地统计中保存脱敏形态，Git remote 会删除凭据、query 和 fragment。

Pi 扩展仍从 `scripts/pi-project-attribution.ts` 或 listener 的 `/__sumpter/pi-project-attribution.ts` 获取，安装到 `~/.pi/agent/extensions/` 后执行 `/reload`；必须在 Pi 的 Sumpter provider 上设置 `X-Sumpter-Client: pi`。

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

然后在“模型组”启用默认组（或新建组），添加客户端使用的模型名，从入口库添加刚才的入口，选择“全部组内模型”或勾选指定模型后保存。示例文件的入口和默认组均停用，须分别启用。

手工配置对应 `modelGroups[].models` 与 `bindings`；`endpointID` 必须引用已有入口 ID：

```json
{"id":"main","name":"主用","enabled":true,"priority":0,"models":["gpt-5.4"],"bindings":[{"endpointID":"your-endpoint-id","enabled":true,"priority":0,"models":null}]}
```

组内模型名必须和客户端实际使用的模型名一致；不确定时先用精确名称。模型名相同的多个入口可绑定到同一组，也可按用途放到不同组；客户端地址不变。

配置多个入口后，同一会话默认粘在上次成功的入口组，时长由顶层 `sessionStickyTtlHours` 控制（单位小时，默认 72；`0` 表示永不过期）。想立即改走新顺序，在统计页对应项目行点「清除粘性归属」，或在入口库把粘性时长改为更短的值——默认组的入口顺序与优先级始终跟随入口库的列表顺序和 Priority。

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

<!-- 由 scripts/sync-usage-docs.py 生成；请修改 docs/usage-onboarding.md 与 docs/usage-path-matrix.json 后同步。 -->
