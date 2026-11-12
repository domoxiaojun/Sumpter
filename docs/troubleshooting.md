# 故障排查

Gemini 原生入口的模型列表、分页鉴权、文件/缓存/批处理、Interactions 和 Live 接入约定见 [Gemini 原生接入](gemini-native.md)。若模型目录错误只列出 `/v1/models` 等兼容路径，应使用包含 Gemini 原生目录修复的版本。

先记录平台、版本、安装方式、客户端、访问地址和触发时间。不要把真实 API Key、Token、Cookie、密码、完整请求正文或未经脱敏的诊断捕获发到 Issue。

## 先分清三个端口

| 端口 | 用途 | 典型检查 |
| --- | --- | --- |
| 57878 | 客户端代理数据面 | 本机 `curl --noproxy '*' http://127.0.0.1:57878/__status`；已设置入站 Token 时加上 `Authorization: Bearer <token>` |
| 57879 | Linux Admin / WebUI | `curl --noproxy '*' -i http://127.0.0.1:57879/healthz` |
| SSH 隧道左侧端口 | 本机转发端口 | 检查 `ssh -L` 左侧是否被占用 |

`/healthz` 返回 204 只证明 Admin listener 存活。未登录的 `/admin/api/status` 返回 401 是正常鉴权结果；管理页能打开也不代表上游可用。

`/__status` 在未配置入站 Token 时只接受环回对端。配置 Token 后，环回访问也要带该凭据，否则返回 401。响应不含近期事件；事件详情在登录后的管理 API。公网 Nginx 示例不转发 `/__status`。

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

这些能力需要上游真实支持。Codex Live 与标准 Realtime 由入口级 `capabilities: ["live"]` 声明，保留方法、SDP、会话字段和查询参数；不要求 `gpt-live-1-codex`、`gpt-live-1.1-codex` 或 `gpt-realtime-2.1` 出现在模型 mapping、模型组或 `/v1/models`。显式 GPT-Live 模型不会被降为旧默认名；缺省模型和误带的聊天模型仍使用 Live 默认路由。普通文本 mapping 不会自动授予 Live 能力。能获取模型列表不等于能完成 WebSocket 握手或异步资源后续请求。

Codex 的 `/wham/realtime/calls` 与 `/backend-api/wham/realtime/calls` 在 Sumpter 中归入 Live 创建接口，出站规范化为上游 `/v1/realtime/calls`。创建成功后从 `Location` 或响应体提取 call ID 并绑定原入口；后续 `/v1/live/{call_id}` 或 calls 子路径继续使用该入口。入站仍使用 Sumpter 客户端 Token，出站使用入口凭据，不将 ChatGPT 登录凭据作为代理鉴权的替代品。

**410 与创建链路分离**：如果客户端日志先出现 `Created client-owned realtime voice call`，随后以 `existingCall` 启动控制连接，而 Sumpter 只有 GET、没有对应的创建 POST，说明会话创建绕过了代理。未知 call ID 在本地拒绝时，上游尝试为 0，不代表上游返回了 410；新版记录为 `client_request_rejected`、`before_response`，不填上游 HTTP 状态。旧历史记录可能仍误标为上游错误。

对于确实通过 app-server 的 `transport: webrtc` 创建会话、且支持以下实验配置的客户端，创建与控制地址应同时指向 Sumpter（凭据沿用已配置的代理 Provider）：

```toml
experimental_realtime_webrtc_call_base_url = "http://127.0.0.1:57878/v1"
experimental_realtime_ws_base_url = "http://127.0.0.1:57878/v1"
```

截至 2026-11-09 检查的桌面版 26.1103.60933（随附 core 0.167.0-alpha.3.2），语音界面通过自己的 ChatGPT HTTP 客户端调用 `/wham/realtime/calls`，然后才通知 app-server 接入 `existingCall`；这个分支不读取上述创建地址覆盖。仅增加这两行或升级 Sumpter 不能改变其创建目标。[Codex 上游同类问题](https://github.com/openai/codex/issues/53463)和 [DSCodex 的关闭说明](https://github.com/fish2lab/DSCodex/pull/21#issuecomment-5748970448)也记录了创建请求绕过代理的情况；第三方报告的版本不能替代当前安装版本的核对。

桌面包另有 `CODEX_API_BASE_URL` 启动环境变量，[其他代理的桌面接入](https://github.com/nicosuave/comradex#optional-desktop-backend-compatibility)使用过这个入口。但它覆盖整个桌面后端，并非语音专用配置。当前包的 `performDesktopFetch` 还会读取 `account/read` 返回的 `workspaceRouting`，用其中的 `backendOrigin` 再次替换目标 origin。对已安装包路由函数使用合成输入验证：即使初始地址为本地 Sumpter，只要 workspace 后端仍是 ChatGPT，最终地址仍会回到 ChatGPT。这是静态函数验证，不是抓取真实账户路由或实机语音成功证据；不能把设置环境变量直接写成已验证的修复方案。

要让语音使用 CPA 账户，客户端接入必须让创建请求携带 Sumpter Token 进入代理，并将该创建结果用于 SDP 协商及后续控制连接。可由客户端增加语音专用路由，或由经过完整验证的桌面后端网关处理；后者还涉及账户路由、非语音 API 及 WebSocket 的兼容。Sumpter 的 Live 路径兼容本身没有实现这个桌面网关。不要取消未知会话校验或伪造绑定，CPA 也需要拥有同一会话。仅删除 WS 覆盖也不是等价方案：Provider 的显式代理凭据仍可能被控制连接沿用，且语音不再使用 CPA 账户。源码合同测试不代表已完成真实桌面语音、CPA 账户与媒体链路验收。

后续已增加独立的 [Codex 桌面网关原型](codex-desktop-gateway.md)：对于官方账户发现明确返回无约束的账户，可继续验证 HTTPS 网关与两项启动地址同时配置的路线，无需改客户端安装包。原型没有集成进已安装 Sumpter；证书信任、客户端重启、真实语音和非语音功能仍需验收。

### 只有某个会话失败

先用新会话复测，再检查运行详情中的会话粘性和上游尝试。加密推理状态跨 Provider 可能不兼容；不要把一次继续请求的错误当作所有入口都不可用。确认后可在运行页清除项目或会话粘性，再发新请求。

### Claude Code 跨会话工具出现 Invalid tool parameters

Claude Code 的普通视图可能把工具未加载、JSON 解析失败和 schema 校验失败统一显示为 `Invalid tool parameters`。先展开第一条失败的详细输出（例如 2.1.322 使用 Ctrl+O），记录工具名、脱敏后的参数和 `InputValidationError` 原因，再核对该请求实际使用的 Sumpter 入口协议；界面的模型名称不能证明走了 Anthropic 原生路径。

转换链路保留本轮声明的真实工具、可选参数和并发约束；历史压缩不会隐藏仍已声明的延迟工具，占位符不会作为可执行工具发送。Responses 的闭合参数分片会暂缓，用结束快照补齐 `{}` 或较短对象中的缺少字段；真实字段冲突仍会失败。遇到未知 `tool_reference`、无法保持的约束、关联冲突或无效参数时，查看 Sumpter 返回的具体字段或桥接失败原因。若走 Anthropic 原生路径，工具定义、引用和响应流直接保留，需继续核对客户端与上游实际参数。消息送达或空闲通知成功只能证明对应操作成功，不能替代监控工具的参数校验。转换边界见 [架构说明](architecture.md)。

### 统计缺失、待定或 Token 不一致

统计按客户端完成事件和上游 usage 记录；进行中的请求计入 pending，不会算作失败。缓存 Token 的口径随协议不同，failover 尝试不会重复计入客户端汇总。客户端显示未知时，检查请求原生标记和独立入站 Token 的身份绑定；项目和会话缺少原生证据时仍保持未知。

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

Responses 仅支持 HTTP/SSE。`/v1/responses`、`/responses`、`/backend-api/codex/responses` 及其子路径的 WS 升级探测在通过入口鉴权后返回 HTTP 426，错误码为 `websocket_not_supported`，提示使用 POST 与 SSE；不返回 101，不连接上游。三个根路径的普通 GET 返回 405 与 `Allow: POST`。支持回退的 Codex 客户端收到 426 后改用 HTTP；其他客户端需选择 HTTP/SSE 传输。不支持的任意路径不会进入通用 WS 转发。

426 是本地传输协商，不计入模型请求数、失败数、请求历史和失败通知。运行页只展示实际请求，已移除 Responses 连接面板和由空闲连接合成的进行中行；运行汇总 API 保持 v1，停止返回可选字段 `responsesWebSocketConnections`。HTTP/SSE 仍使用既有逐请求记录、usage、分页和请求链。

HTTP 200 不代表最终业务成功；应检查 SSE 的协议终态。`client_cancelled` 表示客户端侧断开或取消，不能仅凭它断言 Codex 主动停止，也可能涉及反代。Realtime/Live 继续支持原生 WS，保留文本、二进制、Ping/Pong、关闭和空闲超时处理；进入 relay 只表示连接建立，不代表模型正在生成。

旧 Responses WS 记录仍可查询、展示和导出，包括 `websocketTrace.requests`、首消息等待与 Guardian/预热关闭诊断。这些是历史连接级记录，不会改写为逐请求记录，也不回填缺失 Token、重算历史成功率或清空数据库。历史记录的连接数与内部 create 计数不能相加。

Realtime WS 的 `relayError` 标明读写/控制 flush 位置，`transportErrorKind` 标明传输错误类型；`lastEventType` 是有界诊断，不能单独证明业务成功。空闲超时仍记为 `stream_idle_timeout`；无 Close 的 EOF/RST 会向另一侧发送受控 1011 Close。

本地回归覆盖两端 adapter、大帧期间的心跳、协议帧透明转发、关闭和空闲超时；不证明真实 CPA/网络已修复。线上归因仍需同一请求与同时间的客户端、Sumpter、CPA 和反代日志。近月差异与剩余边界见 [Codex 流式兼容性审计](codex-streaming-audit.md)。
