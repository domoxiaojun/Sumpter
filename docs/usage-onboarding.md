## 开箱路径（固定顺序）

第一次使用只做 3 件事：找到配置 → 填一个上游服务（Provider）→ 启动 Sumpter 并接入客户端。
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
