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
| 配置文件 | {{macos_config_path}} | {{linux_config_path}} |
| 数据面代理 | `http://127.0.0.1:57878`（以运行页/配置为准） | `http://127.0.0.1:57878`（以 `listener` 为准） |
| Admin 地址 | {{macos_admin_address}} | {{linux_admin_address}} |
| 入站鉴权 | `listener.authToken`（非空才启用） | `listener.authToken`（非空才启用） |
| Admin 鉴权 | App 与 sidecar 的本机控制通道 | Admin session cookie + CSRF；不要把密码写入文档或 Issue |
| 模型 mapping | `endpoints[].mappings[]` | `endpoints[].mappings[]` |
| 配置字段参考 | {{macos_config_reference}} | {{linux_config_reference}} |
| 项目归因脚本 | {{attribution_location}} | {{attribution_location}} |

{{topology_note}}

### 客户端接入与安全边界

- Claude Code 使用 `ANTHROPIC_BASE_URL`；Codex / OpenAI 兼容客户端使用 API Base，具体协议和路径见下文。
- 代理会先做 CIDR 与入站鉴权，再读取请求体；不要用大 body 测试错误 token。
- `apiKey`、`authToken`、Admin 密码、Cookie、请求体和 raw 捕获都只留在本机受限文件中；文档、截图和 Issue 只放脱敏后的请求 ID、时间和错误阶段。
- 项目归因脚本必须运行在**启动 Claude Code 的客户端机器**上，而不是远程 daemon 所在机器；每台客户端机器单独配置。

<!-- 由 scripts/sync-usage-docs.py 生成；请修改 docs/usage-onboarding.md 与 docs/usage-path-matrix.json 后同步。 -->
