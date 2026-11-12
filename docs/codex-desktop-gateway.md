# Codex 桌面网关原型

这是 Sumpter 仓库内的独立实验脚本，还没有集成到 Sumpter App、安装包或自动启动服务。它用于验证新版 Codex 桌面语音的完整代理接入，无需修改 Codex 安装包。

## 适用条件

- 当前 Codex 桌面使用 `CODEX_API_BASE_URL` 解析桌面 API 地址，并支持 `CODEX_APP_SERVER_CHATGPT_BASE_URL` 向 app-server 传入后端地址覆盖。2026-11-09 已在桌面版 26.1103.60933 的安装包中核对这两个入口，尚未启动真实客户端进行网关接入验收。
- 当前 ChatGPT 登录保存在 `~/.codex/auth.json`；选中的 Sumpter Provider 在 `~/.codex/config.toml`，使用 `experimental_bearer_token` 或 `env_key`。原型暂不支持仅存储于系统钥匙串的登录、命令式凭据和其他 Provider 鉴权形式。
- Provider 的 `base_url` 指向环回 Sumpter；Sumpter 已启用支持 Live 的 CPA 入口。网关不读取 CPA 账户密钥。
- 官方账户发现结果中的 `workspace_backend_origin` 和 `account_routing_override` 都必须为 `NO_CONSTRAINT`。原型转发真实发现结果，不伪造账户、权限、额度或区域信息；遇到约束时拒绝接入。
- 本地 HTTPS 证书必须包含 `127.0.0.1` 的 IP SAN，并被桌面 Electron 和 app-server 信任。脚本不生成正式证书、不安装 CA、不修改系统信任、不关闭证书验证。

官方的[网关接入说明](https://learn.chatgpt.com/docs/enterprise/connect-to-a-gateway#voice-through-a-gateway)指出，模型 Provider 和语音使用不同路由。本原型补的是桌面 API 网关，不是给模型 Provider 增加一个普通接口别名。

## 路由行为

| 流量 | 路径与凭据 |
| --- | --- |
| 桌面语音创建 | `/backend-api/wham/realtime/calls` → 已配置 Sumpter 的 `/realtime/calls`；使用同一个 Provider 的 Sumpter Token；去掉桌面 OAuth、账户、Cookie 和 attestation 等凭据；保留 SDP、会话正文与查询参数 |
| 语音控制 | 沿用 `experimental_realtime_ws_base_url` 指向 Sumpter；使用 CPA 返回的 call ID，走既有绑定及账户选择 |
| 其他桌面 HTTP / WS | 固定转发到 `https://chatgpt.com`，保留桌面登录身份；不跟随上游跳转，不接受客户端指定任意上游 |
| 账户发现 | 只读校验当前账户及无约束路由，原样转发成功响应；身份变化会重新校验 |

网关仅监听 `127.0.0.1`，除健康检查外校验当前登录 Bearer，存在账户头时还检查账户一致。它不会新增或降级 Sumpter 的入站认证，也不会接纳没有创建绑定的外部 call ID。普通后端流量不进入 Sumpter 推理统计，语音创建和控制仍由 Sumpter 记录。

## 只读预检

在仓库根运行：

```bash
uv run --no-build scripts/clients/codex_desktop_gateway.py --check
```

它读取现有配置，并向官方账户发现接口发起一次只读请求，不创建语音、不启动监听、不打印凭据、不改文件。成功只代表当前账户满足路由前提。

## 候选接入步骤

以下步骤必须等证书与客户端重启安排完成后再执行；它们不是已经完成的实机验收。

1. 使用已有受信任的证书启动网关。路径替换为自己的证书和私钥；首次 `uv run` 会在隔离缓存安装脚本声明的依赖，`--no-build` 禁止从源码构建依赖。

```bash
uv run --no-build scripts/clients/codex_desktop_gateway.py \
  --cert /absolute/path/desktop-gateway-cert.pem \
  --key /absolute/path/desktop-gateway-key.pem
```

2. 保持模型 Provider 仍指向 Sumpter，并确认现有顶层设置：

```toml
experimental_realtime_ws_base_url = "http://127.0.0.1:57878/v1"
```

3. 完全退出桌面客户端后，从单次启动环境启动。以下可执行路径已在本机安装包核对；其他安装位置自行替换。环境变量只作用于本次启动，不写入 `launchctl` 或配置文件：

```bash
CODEX_API_BASE_URL=https://127.0.0.1:57880/backend-api \
CODEX_APP_SERVER_CHATGPT_BASE_URL=https://127.0.0.1:57880/backend-api \
/Applications/ChatGPT.app/Contents/MacOS/ChatGPT
```

两项地址必须一致。仅设置桌面地址时，后续 `workspaceRouting.backendOrigin` 仍可能将请求送回官方。需要同时核对重启后 app-server 的有效后端地址；后台 daemon 若复用旧配置，此次启动不能算接入成功。app-server 支持 `CODEX_CA_CERTIFICATE` 等自定义 CA 输入，但这不能代替 Electron 的证书信任。

4. 新开语音会话，确认 Sumpter 先收到创建 POST 并返回成功及 call ID，再收到同 ID 的控制连接并返回 101。随后验证双向声音、语音触发任务、侧栏、模型列表、普通聊天和插件读取。旧会话可能仍带有创建时的后端状态，需要新会话验收。

## 回滚

结束本次网关进程，退出由上述环境启动的桌面客户端，再从正常应用入口打开即可。单次环境不会改变常规启动。若手动修改过 TOML、系统信任或全局启动环境，须恢复那些单独的改动；脚本本身不做这些操作。停止网关前先结束语音，避免打断正在使用它的桌面请求。

## 验证与边界

定向回归命令：

```bash
uv run --no-build scripts/tests/test_codex_desktop_gateway.py
ruff check --target-version py311 scripts/clients/codex_desktop_gateway.py scripts/tests/test_codex_desktop_gateway.py
uv run --no-build --with aiohttp==3.14.5 --with basedpyright basedpyright --level error scripts/clients/codex_desktop_gateway.py scripts/tests/test_codex_desktop_gateway.py
```

测试使用合成登录、假 Sumpter 与假官方后端；临时证书仅在测试进程中显式信任，不写系统信任。覆盖创建正文、SDP/Location、同 ID 控制连接、账户原文、约束拒绝、登录变化、HTTP 错误与压缩/SSE、WS 文本/二进制/Ping/Pong、跳转拒绝和 HTTPS。

真实桌面接入、CPA 音频及全部非语音 API 尚未验收。后端请求经 Python HTTP 客户端转发，与桌面的原网络实现不同；如果真实接口出现新的认证、Cookie 或网络错误，需要按同一时间的客户端与网关结果定位，不能以本地测试通过替代产品验收。不要把这个原型配置成默认启动或作为已发布的 Sumpter 功能。
