# 配置参考（schema v7）

Sumpter 使用一份 JSON 配置文件，Linux 和 macOS 共用格式。新用户应先在 WebUI 配置；只有需要审阅、备份或自动化时才手工编辑。

- Linux 用户服务：`${XDG_CONFIG_HOME:-~/.config}/sumpter/config.json`
- Linux system 服务：`/var/lib/sumpter/config.json`
- macOS：`~/Library/Application Support/Sumpter/config.json`
- Compose：部署目录 `config/config.json`

真实 API Key、入站 Token 和密码只能放在仓库外，文件权限为 0600。完整占位模板是 [config.example.json](../config.example.json)；两平台副本由同步脚本维护。

## 顶层字段

| 字段 | 类型 | 用途 |
| --- | --- | --- |
| `schemaVersion` | `7` | 当前格式版本，保存时必须明确为 7 |
| `listener` | 对象 | 代理数据面监听和入站访问控制 |
| `retry` | 对象 | 超时、重试、退避和失败响应行为 |
| `endpoints` | 数组 | Provider 地址、凭据、协议标签和入口映射 |
| `modelGroups` | 数组或省略 | 统一地址开放哪些模型，以及入口绑定和调度 |
| `featureRules` | 数组 | WebSearch、WebFetch、安全分类器等子请求分流 |
| `sessionStickyTtlHours` | 数字 | 会话归属保留时长；0 表示不按时间淘汰 |

`pools`、`globalModels`、`main_accounts`、`feature_routes`、`secretRef` 和 `target.poolID` 不属于 v7。旧 v3–v6 文件会在加载时兼容迁移；仅有旧 `keys.json` 不会自动转换。

## listener

```json
"listener": {
  "host": "127.0.0.1",
  "port": 57878,
  "allowedCIDRs": [],
  "authToken": ""
}
```

`host` 和 `port` 是客户端访问的代理数据面。空 `authToken` 表示不校验入站 Token；生产环境建议设置随机非空值。`allowedCIDRs` 非空时只允许列出的 IP 网段。

Compose 特例：容器内 `host` 应为 `0.0.0.0`，容器端口保持 57878；宿主机暴露地址和端口由 Compose `ports` 决定。Linux systemd / macOS 本机通常使用 `127.0.0.1`。

Linux Admin 是独立监听，默认 `127.0.0.1:57879`，由 CLI 参数或环境变量设置，不写入 `listener`：`--admin-host`、`--admin-port`、`SUMPTER_ADMIN_HOST`、`SUMPTER_ADMIN_PORT`。管理密码默认是配置目录的 `admin-password`，可由 `--admin-password-file` 或 `SUMPTER_ADMIN_PASSWORD_FILE` 覆盖。

## endpoints

```json
{
  "id": "provider-main",
  "name": "主入口",
  "baseURL": "https://api.example.com",
  "protocol": "auto",
  "enabled": true,
  "apiKey": "只放本机文件",
  "priority": 0,
  "stickyGroup": "provider-main",
  "mappings": [
    {
      "clientPattern": "claude-opus-*",
      "upstreamModel": "",
      "thinking": "adaptive",
      "context": "standard"
    }
  ]
}
```

| 字段 | 说明 |
| --- | --- |
| `id` | 唯一 ID，也是统计聚合键；修改会切断历史入口统计 |
| `baseURL` | Provider API 根地址；不要填控制台网页地址 |
| `apiKey` | 发往该 Provider 的凭据，绝不与配置示例一起提交 |
| `protocol` | `auto`、`anthropic`、`openai`、`openai-responses` 或 `gemini`；只描述入口能力 |
| `enabled` | false 时不参与路由 |
| `priority` | 非负整数，数字越小越优先；同级按数组顺序 |
| `stickyGroup` | 相同值的入口共享会话调度组；省略时使用自身 ID |
| `mappings` | 入口明确承接的模型；空数组不承接普通模型 |
| `keepAlive` | 可选出站连接复用，省略或 false 为关闭 |
| `catalog` | 「获取模型」的展示缓存，不会自动开放模型 |

映射中的 `clientPattern` 支持精确名称和尾部 `*` 通配；精确匹配优先，通配按最长前缀。`upstreamModel` 为空表示同名。`thinking`、`context`、`effort` 和 `failoverTimeoutSeconds` 保存路由策略与兼容信息；raw 透传不会凭这些字段重写客户端正文或普通协议头，只有明确的模型映射才会替换可安全识别的模型字段。

## modelGroups

模型组把模型范围与入口绑定分开管理。组 `priority` 决定组顺序，绑定 `priority` 决定组内入口顺序。绑定的 `models` 省略或为 null 表示承接入口已添加的全部组内模型，空数组表示不承接，填写数组则进一步收紧范围。

`schedulingStrategy` 默认为 `priority`，可选 `randomSticky` 或 `roundRobinSticky`。随机和轮询只影响新会话；已有会话继续使用有效粘性归属。轮询游标在进程内维护，重启或重载后从配置顺序开始。

`bindings[].overrides` 只对当前绑定和模型覆盖上游模型名或入口优先级，不改变入口库的原始映射。实际开放范围是入口映射、组模型和绑定选择的交集。

## retry

| 字段 | 说明 |
| --- | --- |
| `responseTimeoutSeconds` | 首响应总截止；null 由客户端决定 |
| `streamIdleTimeoutSeconds` | 流式两次输出之间的最长空闲；null 不限制 |
| `max500Retries` | 当前入口收到 500 后的额外重试次数 |
| `failoverOn500` | 500 重试耗尽后是否切换入口 |
| `retryDelaySeconds` | 最终失败响应使用的 retry delay |
| `passThroughRetryDelay` | 是否透传 `Retry-After` / `retry_delay` |
| `sessionStickyRetries` | 粘性调度组遇到可重试故障时的额外尝试次数 |
| `maxDeferredRounds` | 可重试故障最多跨越的轮数；0 不限 |
| `maxRetryDurationSeconds` | 可重试故障的总时长；0 不限 |

客户端断开会取消上游请求和等待。有限的轮数或总时长通常比两个值都为 0 更容易排障。代理尽量返回最终实际的上游状态和错误内容；尚未产生上游响应时，才会返回本地访问、路由、配置或连接错误。

## featureRules

内建规则 ID 为 `websearch`、`webfetch`、`classifier`，默认关闭。启用后可指定目标模型、协议、入口和 reasoning effort。规则识别 Claude Code 的独立子请求，不扫描主会话的全部历史。`endpointID` 固定入口时会绕过普通映射筛选；入口不可用时才回到候选序列。

## 迁移与保存

加载 v3–v6 时，程序先生成 0600 的 `config.before-schema-v7-时间.json`，再原子写入并复读校验。失败会保留旧文件。迁移不会读取或改写只存在的 `keys.json`；请人工建立 v7 配置。

WebUI 保存使用 generation-safe PUT，防止两个页面互相覆盖。手工编辑后，Linux 执行 `systemctl reload`，Compose 可在管理页重载或重启容器；管理监听和密码路径变化需要重启。配置目录和运行数据库不要让多个实例共享。

## 凭据边界

`endpoints[].apiKey` 只发给对应上游；`listener.authToken` 只用于客户端到 Sumpter；Linux `admin-password` 只用于管理页登录；`X-Sumpter-*` 项目归因头只在入站统计使用，转发上游前会剥离。诊断捕获可能含原始正文和 Header，默认关闭，导出前必须自行脱敏。
