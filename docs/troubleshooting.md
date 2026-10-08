# 故障排查

先记录平台、版本、安装方式、客户端、访问地址和触发时间。不要把真实 API Key、Token、Cookie、密码、完整请求正文或未经脱敏的诊断捕获发到 Issue。

## 先分清三个端口

| 端口 | 用途 | 典型检查 |
| --- | --- | --- |
| 57878 | 客户端代理数据面 | `curl --noproxy '*' http://127.0.0.1:57878/__status` |
| 57879 | Linux Admin / WebUI | `curl --noproxy '*' -i http://127.0.0.1:57879/healthz` |
| SSH 隧道左侧端口 | 本机转发端口 | 检查 `ssh -L` 左侧是否被占用 |

`/healthz` 返回 204 只证明 Admin listener 存活。未登录的 `/admin/api/status` 返回 401 是正常鉴权结果；管理页能打开也不代表上游可用。

## 按现象检查

### 容器没有启动

```bash
sudo docker compose ps -a
sudo docker compose logs --tail 100 init sumpter
```

确认 `init` 为 `Exited (0)`，`config/config.json` 和 `config/admin-password` 是普通文件且权限正确。标准 Docker 目录通常由 root 拥有、目录 0700、文件 0600。仅剩旧 `keys.json` 时不会自动迁移。

### 管理页打不开

Linux systemd 查看 `journalctl`；Compose 查看 `docker compose logs`。确认 57879 未被其它进程占用、管理监听仍为默认值、远程访问使用了 SSH 隧道或 HTTPS 反代。浏览器地址填写 `127.0.0.1` 或服务器可达地址，不填写 `0.0.0.0`。

### 管理页能打开但客户端连接失败

确认客户端访问的是 57878（不是 57879），远程部署时端口映射和防火墙允许访问，Compose 配置的容器 listener 是 `0.0.0.0:57878`。入站 Token 非空时，客户端 API Key 必须完全相同。

### 返回模型不存在或 no route

入口必须启用且有 mapping；模型组必须启用该模型并绑定入口；绑定范围不能是空数组。入口「获取模型」成功只是目录提示，不会替你建立 mapping。精确模型名、通配符和 `upstreamModel` 逐一核对。

### Codex 列表里没有 CPA 的 Gemini 模型

先在对应入口重新执行「获取模型」，刷新旧缓存。目录探测区分 OpenAI 与 Anthropic 身份，并合并有效结果；旧版本探测的 Anthropic 头可能使 CPA 返回经过改名的 Claude 专用目录，不能据此认定 Gemini 不可用。

刷新后检查 Gemini 原始模型 ID 是否在入口 mapping、已启用模型组及绑定范围内。Codex 的 `/v1/models?client_version=...` 只列配置实际开放的模型，不会自动开放整个上游目录。列表可见后仍需发送一条 Responses 请求验证上游能力；不要把 CPA 私有别名手工反转或硬编码成原始 ID。显式自定义 UA 会保留，若该 UA 触发上游专用目录，请核对入口 UA 设置。

### 返回上游 401 / 403 / 404 / 429 / 5xx

在「运行」打开请求详情，区分最终客户端结果和中途上游尝试。确认对应入口的 API Key、Base URL、协议和上游实际路径。Sumpter 尽量透传最终上游状态；不要只凭某一次 failover 尝试判断。

### Live / Realtime / Video 失败

这些能力需要上游真实支持。Codex Live 与标准 Realtime 由入口级 `capabilities: ["live"]` 声明，方法、路径和负载原样透传；不要求 `gpt-live-1-codex` 或 `gpt-realtime-2.1` 出现在模型 mapping、模型组或 `/v1/models`。普通文本 mapping 不会自动授予 Live 能力。能获取模型列表不等于能完成 WebSocket 握手或异步资源后续请求。

### 只有某个会话失败

先用新会话复测，再检查运行详情中的会话粘性和上游尝试。加密推理状态跨 Provider 可能不兼容；不要把一次继续请求的错误当作所有入口都不可用。确认后可在运行页清除项目或会话粘性，再发新请求。

### 统计缺失、待定或 Token 不一致

统计按客户端完成事件和上游 usage 记录；进行中的请求计入 pending，不会算作失败。缓存 Token 的口径随协议不同，failover 尝试不会重复计入客户端汇总。检查项目归因是否安装在运行客户端的机器，而不是只运行代理的服务器。

### 存储降级或数据库错误

先检查磁盘空间、配置目录所有者和 SQLite 的 `-wal` / `-shm` 文件。停止服务后备份整个目录。只有确认无需保留历史时才使用「重建数据库」；该操作会删除原运行统计，不能用作普通重启。

## 收集最小证据

```bash
# Linux systemd
systemctl status sumpter.service --no-pager
journalctl -u sumpter.service -n 100 --no-pager

# Compose
sudo docker compose ps -a
sudo docker compose logs --tail 200 sumpter

# 只检查监听，不带凭据
curl --noproxy '*' -i http://127.0.0.1:57879/healthz
```

在 WebUI「诊断」中按需开启捕获，复现后立即关闭并导出脱敏副本。诊断捕获默认可能包含 Header、Body 和上游响应，未经检查不要分享。

### Codex 流式断开与客户端取消

运行页的“当前 Responses WebSocket”显示当前进程内存中的连接快照：等待首条业务消息（含明确 Guardian 子集）、连接上游中、已进入转发，以及最长首消息等待。三个阶段之和为当前连接总数；没有等待连接时最长等待显示“—”。进入转发只表示上游连接已建立，不代表模型正在生成；带 query model 的连接直接进入上游准备阶段。等待首消息时 Ping/Pong 不改变阶段，`generate:false` 则是业务消息。此快照独立于历史事件，不写入数据库，清空统计不清除存活连接，重启后重新计数；不增加主动心跳、回收或连接数量限制。

两端运行汇总 API v1 增量返回可选对象 `responsesWebSocketConnections`，包含 `total`、`awaitingFirstMessage`、`guardianAwaitingFirstMessage`、`connectingUpstream`、`relaying`、`oldestFirstMessageWaitMS`；无人等待时时长为 `null`。运行页沿用刷新间隔，即使没有新事件也更新；关闭自动刷新会冻结连接区域。旧 daemon 缺少字段时显示“当前版本未提供”；读取失败保留上次值并提示“更新失败”，不能把历史事件条数或读取失败解释成实时零连接。

先区分 HTTP/SSE 请求和可复用的 Responses WebSocket 连接。HTTP 200、WebSocket 101 都不代表最终业务成功；`client_cancelled` 表示客户端侧断开或取消，不能仅凭它断言 Codex 主动停止，也可能涉及反代。WebSocket 的正常 Close 与异常断开分别记录；正常客户端 Close 记取消，正常上游 Close 记连接成功，异常 Close、EOF/RST 原则上仍记失败。

Guardian 未使用预热连接是一个窄范围例外：仅在 Responses 客户端已升级、有明确且无冲突的 Guardian 标记、尚未收到首条业务消息且未连接上游时，正常 Close 或无 Close 的 EOF 记为 `cancelled`，消息为 `websocket_unused_guardian_connection_closed`。Linux/macOS 显示“Guardian 未使用连接已关闭（预热阶段，未发送业务请求）”；保留连接记录与请求总数，不增加成功或失败计数、不触发失败通知。EOF 仍保留 `abnormalClose=true`、`relayError=client_eof_without_close` 和 `transportErrorKind=eof_without_close`，不能据此断定客户端主动回收或网络正常。RST、异常 Close、无效首帧、服务停止、普通非 Guardian 连接及已发送业务帧后的断流仍保留原分类；`generate:false` 也是业务帧，不属于未使用连接。旧记录不追溯改写。

WebSocket 诊断保留 `stage`、`firstMessageWaitMS`、`attemptCount` 与两侧握手状态；`relayError` 标明读写/控制 flush 位置，`transportErrorKind` 标明安全错误类型。`lastEventType` 只观察不超过 64 KiB 的上游业务帧中已知的事件类型；未知、过大或无效业务帧后留空。它不是响应完成的证明，更不能把复用连接最后一次异常算成此前每个响应均失败。`idleTimeoutMS` 只在配置的双向帧空闲截止触发时记录。

本地回归覆盖两端 adapter、大帧期间的心跳、协议帧透明转发、关闭和空闲超时；不证明真实 CPA/网络已修复。线上归因仍需同一请求与同时间的客户端、Sumpter、CPA 和反代日志。近月差异与剩余边界见 [Codex 流式兼容性审计](codex-streaming-audit.md)。
