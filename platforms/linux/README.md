# Sumpter Linux Rust 版（kekulvd）

在源码 monorepo 中，Linux 版复用根 `crates/` 的共享代理核心，以 standalone daemon 形式提供 Anthropic 兼容
代理、扁平 Provider 入口、分流规则、pinned IP、failover、统计，以及与桌面 UI 信息架构对齐的
本机 Web 管理界面。

Linux 专属边界：

- proxy listener：由 `config.json.listener.host/port` 决定，默认 `127.0.0.1:57878`。
- Admin/Web listener：默认 `127.0.0.1:57879`；可用启动参数或环境变量覆盖（见下文）。
- 配置：当前使用 schema v6 `config.json`；自动迁移 schema v3/v4/v5，其他旧格式和旧 `keys.json`
  不读取。迁移会先创建 0600 的 `config.before-schema-v6-*.json` 原始备份，再原子写入并验证；已废弃入口字段会在迁移时删除。
- Web Admin：默认从配置目录 `admin-password` 初始化内置登录，会话使用 HttpOnly Cookie + CSRF；不提供通知页、Claude Hook 或 `/__notify`。
- 进程：默认前台运行，适合 systemd；SIGHUP 热重载，SIGTERM/SIGINT 优雅退出。
- 发行版：面向通用 systemd Linux（含 Fedora/RHEL 与 Debian/Ubuntu）；静态 musl 二进制，不依赖发行版 glibc 包。

## 验证状态

> 时效：这段是 2026-08-10 开发迁移时的验证边界，**不是安装步骤**。用户安装看「自动安装、升级与卸载」。

2026-08-10 这一批迁移**不在本机编译或测试 Rust workspace，也不在本机构建 Docker 镜像**。
GitHub Actions 负责 Rust fmt/check/test、双架构 Release 与多架构 GHCR 镜像；提交后的
Actions 结果才是该提交的构建证据。Gemini WebUI 构建/契约测试、Shell 语法/ShellCheck 与安装器的隔离
事务自测已经覆盖 user 与 system 两种布局，但都不能替代真实 Linux systemd、真实二进制或代理
流量验收。正式交付前仍须按本文「构建与验收」在目标 Linux 环境补齐验证。

## 部署包结构

```text
kekulv-linux-<arch>/
├── kekulvd
├── config.example.json
├── USAGE.md
├── web/
├── scripts/
│   ├── start.sh
│   ├── stop.sh
│   ├── smoke.sh
│   ├── install.sh
│   ├── bootstrap-install.sh
│   ├── bootstrap-uninstall.sh
│   ├── uninstall.sh
│   └── cc-project-attribution.sh   # Claude Code 项目统计配置器(可选)
├── specs/admin-api.md
├── kekulv.service
├── kekulv-system.service
├── LICENSE
├── logs/
└── README.md
```

`arch` 为 `x86_64` 或 `aarch64`。目标产物是对应架构的静态 musl ELF；必须在目标 Linux
机器用 `file ./kekulvd` 和 `scripts/smoke.sh` 复核，不能只凭交叉编译退出码判断。

## GitHub Actions 与发布资产

本仓库按**独立仓库根目录**组织，工作流位于 `.github/workflows/`。如果把它继续放在更大
monorepo 的子目录里，GitHub 不会发现这里的工作流；此时必须把工作流移到仓库根目录并同步调整路径。

- `ci.yml`：pull request 或手动触发；使用 Rust 1.88.0 执行 fmt/check/test，并运行 Gemini WebUI 的 `npm ci`、契约测试与生产构建。普通 push 不会自动消耗 CI 额度。
- `release.yml`：手动触发只生成 Actions artifact；推送 `v*` tag 时同时创建 GitHub Release。
- `container.yml`：pull request 只构建不推送；手动触发只构建二进制；推送 `v*` tag 时发布 amd64/arm64 GHCR manifest。
- `macos-release.yml`：已有统一 `v*` tag 的 macOS DMG、Sparkle ZIP、签名 appcast 和 checksum；也可手动指定已有 tag。

Release 固定提供：

```text
kekulv-linux-x86_64.tar.gz
kekulv-linux-aarch64.tar.gz
SHA256SUMS
```

所有第三方 Actions 固定到完整 commit SHA；Release 发布 job 才有 `contents:write`，容器发布 job
才有 `packages:write`。源码仓库为 [`domoxiaojun/sumpter`](https://github.com/domoxiaojun/sumpter)，
可以设为私有；面向用户的二进制分发由下文的静态镜像承担。本项目采用 [MIT License](LICENSE)，
Cargo workspace 与仓库根目录的 `LICENSE` 保持一致。
推送与 `kekulvd` Cargo 版本一致的 `v<version>` tag 会创建 GitHub Release；许可证不是 GitHub
Release 的技术前置条件，但它明确下游可获得的使用权。

## 自动安装、升级与卸载

安装器按调用身份选择 scope：普通用户安装为 systemd user 服务；root 或 `sudo` 安装为 system
service，但 daemon 始终使用专用的低权限 `kekulv` 用户运行。源码仓库可以保持私有，发布包改由
静态镜像 `https://sf.domob.org/kkl` 提供。镜像目录必须同时提供：

```text
kekulv-install.sh
kekulv-uninstall.sh
kekulv-linux-x86_64.tar.gz
kekulv-linux-aarch64.tar.gz
```

把仓库内的 `scripts/bootstrap-install.sh` 和 `scripts/bootstrap-uninstall.sh` 原样上传为镜像根目录的
`kekulv-install.sh` 和 `kekulv-uninstall.sh` 后，普通用户使用下面命令安装最新包：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/kekulv-install.sh https://sf.domob.org/kkl/kekulv-install.sh
bash /tmp/kekulv-install.sh
```

首次安装会在配置目录生成 0600 随机 `admin-password`，不在终端回显，升级与普通卸载都会保留。
首次登录用户名为 `kkl`；登录后可在“安全”页修改用户名和密码，文件会迁移为 Argon2 哈希 JSON。
监听参数和高级凭据路径覆盖会写入 systemd drop-in；
HTTPS 反代通常无需改默认 loopback：

```bash
bash /tmp/kekulv-install.sh --admin-password-file /absolute/path/admin-password
bash /tmp/kekulv-install.sh --admin-host 0.0.0.0 --admin-port 57879 \
  --admin-password-file /absolute/path/admin-password
# 或环境变量
KEKULV_ADMIN_PASSWORD_FILE=/absolute/path/admin-password bash /tmp/kekulv-install.sh
```

如需先审查已下载的脚本，可在安装命令前单独运行下面这条非交互命令：

```bash
sed -n '1,$p' /tmp/kekulv-install.sh
```

引导安装器根据当前机器架构下载同名压缩包，不需要指定版本。root/system 安装使用同一份脚本，
但必须明确通过 `sudo` 运行：

```bash
sudo bash /tmp/kekulv-install.sh
```

静态镜像路径按你的要求**不校验 SHA-256**；它只校验 HTTPS、归档结构、路径与符号链接，并由包内
安装器继续检查布局和可执行文件架构。镜像被替换、TLS 被错误终止或发布包被篡改时，安装器无法证明
内容真实性。**静态镜像需发布方手工替换**，可能落后于 GitHub Release。

`--repo` / `--version` 只属于包内 `scripts/install.sh`（公开 GitHub Release + SHA-256），
**不能**加在 `kekulv-install.sh` / `bootstrap-install.sh` 后面（已从静态镜像取包）。

已手动下载并解压 Release 包时，普通用户直接在包内执行；root/system 则加 `sudo`：

```bash
./scripts/install.sh
sudo ./scripts/install.sh
./scripts/install.sh --admin-password-file /absolute/path/admin-password
```

两种安装布局如下；已有 `config.json` 与 `admin-password` 永不覆盖，首次安装才复制停用的示例
配置并生成初始密码。安装失败会恢复旧程序、unit 和服务启停状态。

| 调用方式 | 程序 | 上一版本 | 配置/统计 | unit |
| --- | --- | --- | --- | --- |
| 普通用户 | `~/.local/share/kekulv` | `~/.local/share/kekulv.previous` | `~/.config/kekulv` | `~/.config/systemd/user/kekulv.service` |
| root / sudo | `/opt/kekulv` | `/opt/kekulv.previous` | `/var/lib/kekulv` | `/etc/systemd/system/kekulv.service` |

首次登录前可在可信终端查看安装器生成的 Admin 密码（不要把输出贴到日志或聊天）。在安全页
修改凭据后，该文件保存的是哈希 JSON，不能再用于回读明文密码：

```bash
cat ~/.config/kekulv/admin-password                 # user 安装
sudo cat /var/lib/kekulv/admin-password             # system 安装
```

卸载默认保留配置。已安装包内的卸载器可直接运行：

```bash
~/.local/share/kekulv/scripts/uninstall.sh
```

root/system 安装则运行：

```bash
sudo /opt/kekulv/scripts/uninstall.sh
```

也可下载静态镜像的引导卸载器；它会按当前身份调用对应的包内卸载器。普通用户：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/kekulv-uninstall.sh https://sf.domob.org/kkl/kekulv-uninstall.sh
bash /tmp/kekulv-uninstall.sh
```

root/system 安装：

```bash
sudo bash /tmp/kekulv-uninstall.sh
```

只有明确要永久删除 `config.json`、`runtime.sqlite3`、旧 `stats.json` 归档和其他运行数据时，才在所选卸载命令后加上
`--purge`，例如：

```bash
bash /tmp/kekulv-uninstall.sh --purge
```

静态镜像引导安装器不依赖 GitHub 仓库可见性，适合源码私有、二进制公开分发。当前已确认
`https://sf.domob.org/kkl/kekulv-linux-x86_64.tar.gz` 和
`https://sf.domob.org/kkl/kekulv-linux-aarch64.tar.gz` 均可访问；还需上传引导脚本
`kekulv-install.sh` 和 `kekulv-uninstall.sh` 后才能分享上面的安装和卸载命令。若改回
`scripts/install.sh --repo domoxiaojun/sumpter`，该路径只支持 `github.com` 的公开 Release，并会下载
`SHA256SUMS` 校验资产。

## 首次配置

普通用户默认配置目录遵循 XDG：

- 设置了 `XDG_CONFIG_HOME`：`$XDG_CONFIG_HOME/kekulv`
- 否则：`~/.config/kekulv`
- 也可用 `--config-dir <dir>` 覆盖

root/system service 的安装器显式传入 `/var/lib/kekulv`；不要在该路线把配置放进 root 的 HOME。

建议显式安装合成示例，再通过 WebUI 或编辑器填写真实入口：

```bash
config_base="${XDG_CONFIG_HOME:-$HOME/.config}"
install -d -m 700 "$config_base/kekulv"
install -m 600 ./config.example.json "$config_base/kekulv/config.json"
```

> 不要用桌面/Rust 版本的 `config.toml` 内容覆盖这里的 `config.json`，也不要把它直接改名为
> `config.json`。Linux daemon 只解析 schema v6 JSON；名为 `config.toml` 的文件不会被读取，
> TOML 内容一旦放进 `config.json` 会让 daemon 在 Admin 端口启动前退出，WebUI 随后只能显示
> “无法连接管理接口 / Failed to fetch”。请以 `config.example.json` 为结构，通过 WebUI 或人工把
> Provider、模型和路由值转换到 JSON。

若配置目录已有 `admin-password`，但既没有 `config.json` 也没有旧 `keys.json`，daemon 会创建
安全的 schema v6 bootstrap 空配置；显式安装示例的好处是能直接看到字段结构和停用的合成入口。

示例入口全部 `enabled:false`，域名使用 `.invalid`，secret 为合成的 `sk-test-...`；替换真实
地址和 secret 后再启用。`config.json` 含明文 secret，权限必须保持 0600。

> 旧 Linux Swift 版的 `keys.json` 不会自动迁移。配置目录只有 `keys.json` 时，新 daemon
> 必须报错并非零退出，且不能改写原文件。请先备份旧文件，再人工转换为 v5 `config.json`；
> 不要通过删除旧文件来掩盖未完成的转换。

## 启动与连接

不使用安装器时，先在默认配置目录创建初始单行密码文件（输入不会回显）：

```bash
install -d -m 700 ~/.config/kekulv
read -rsp 'Admin password: ' KEKULV_NEW_ADMIN_PASSWORD
printf '%s\n' "$KEKULV_NEW_ADMIN_PASSWORD" > ~/.config/kekulv/admin-password
unset KEKULV_NEW_ADMIN_PASSWORD
printf '\n'
chmod 600 ~/.config/kekulv/admin-password
```

前台运行：

```bash
./kekulvd
```

后台运行：

```bash
./scripts/start.sh
./scripts/stop.sh
```

`start.sh` 使用 nohup、私有 pidfile 和日志轮转，并探测 Admin 是否就绪。常用覆盖：

| 环境变量 | 默认 | 作用 |
| --- | --- | --- |
| `KEKULVD_BIN` | `../kekulvd` | daemon 路径 |
| `KEKULVD_CONFIG_DIR` | XDG 默认 | 追加 `--config-dir` |
| `KEKULVD_WEB_ROOT` | `../web` | Web 静态资源目录 |
| `KEKULVD_PID_FILE` | `../kekulvd.pid` | start/stop 共用 pidfile |
| `KEKULVD_LOG_DIR` | `../logs` | nohup 日志目录 |
| `KEKULVD_START_TIMEOUT` | `15` | 启动等待秒数 |
| `KEKULVD_STOP_TIMEOUT` | `15` | 优雅停止等待秒数 |
| `KEKULV_ADMIN_HOST` | `127.0.0.1` | Admin 绑定地址（systemd `Environment=` 可用） |
| `KEKULV_ADMIN_PORT` | `57879` | Admin 端口 |
| `KEKULV_ADMIN_PASSWORD_FILE` | `<config-dir>/admin-password` | 高级 Admin 凭据文件路径覆盖 |

其它 daemon 参数可直接追加到 `start.sh`。daemon CLI：

| 参数 | 作用 |
| --- | --- |
| `--config-dir <dir>` | 指定 `config.json` / `runtime.sqlite3` 目录 |
| `--web-root <dir>` | 指定静态 Web 目录；部署脚本显式传包内 `web/` |
| `--no-web` | 禁用静态 Web；Admin API 仍可用 |
| `--admin-host <ip>` | Admin 绑定地址（`0.0.0.0`/`::` = 全接口；默认 `127.0.0.1`） |
| `--admin-port <port>` | Admin 端口（默认 `57879`） |
| `--admin-password-file <path>` | 覆盖默认凭据路径；旧单行格式初始用户名为 `kkl` |
| `--foreground` | 旧脚本兼容 no-op；Linux daemon 始终前台运行 |
| `--version` | 输出版本 |

优先级：Admin 监听为 CLI > 环境变量 > 默认；密码路径为 CLI > 环境变量 >
`<config-dir>/admin-password`。这些启动项都**不**写入 `config.json`，避免 SIGHUP 改管理口自锁。

Claude Code：

```bash
export ANTHROPIC_BASE_URL=http://127.0.0.1:57878
# config.listener.authToken 非空时，还需设置为相同值：
export ANTHROPIC_AUTH_TOKEN='replace-with-your-inbound-token'
```

健康探测：

```bash
curl --noproxy '*' http://127.0.0.1:57878/__status
curl --noproxy '*' -i http://127.0.0.1:57879/healthz
# /admin/ 静态登录壳可直接加载；未登录 API 返回 401：
curl --noproxy '*' -i http://127.0.0.1:57879/admin/
curl --noproxy '*' -i http://127.0.0.1:57879/admin/api/status
```

## Web 管理

默认浏览器打开 [http://127.0.0.1:57879/admin/](http://127.0.0.1:57879/admin/)。

- 默认只监听 loopback；`/admin/` 静态壳公开加载，API 与 SSE 必须登录。
- 初始用户名为 `kkl`，浏览器在内置登录页提交用户名和密码；服务端签发 24 小时 HttpOnly、
  SameSite=Strict 会话 Cookie，写请求同时校验 CSRF。前端不把密码或会话令牌保存进 localStorage。
- “安全”页可修改用户名和密码；成功后其它旧会话立即失效，当前页面获得新会话。
- 配置 GET 会脱敏 endpoint API key 和入站 token；secret 通过单独的更新字段提交。
- 打开入口编辑框时，界面会经 `GET /admin/api/endpoint-secret?endpointID=<id>` 单条拉取该入口的
  API Key 明文并预填（默认遮挡，点眼睛显示），方便核对和改回；入站 token 与 admin 密码
  没有读取端点。
- WebUI 可以启停 proxy listener，但不会退出 daemon/Admin。
- 通知功能已移除：没有通知页或 Hook，proxy `/__notify` 恒为 404。

### 改 Admin 监听（含 systemd）

只在可信终端临时管理时，可继续使用 SSH 转发；转发后的本机 URL 仍会显示登录页：

```bash
ssh -L 57879:127.0.0.1:57879 user@server
```

VPS 长期远程管理推荐：daemon 仍只监听 `127.0.0.1:57879`，daemon 自己管理登录会话，
Nginx/OpenResty 只做 HTTPS 和转发。不要再在 Nginx 配第二层 `auth_basic`，也不要改写 Cookie；
反代必须传递 `X-Forwarded-Proto $scheme`，让 HTTPS 会话 Cookie 带 `Secure`。可直接参考
[`deploy/nginx-kekulv-admin.conf.example`](deploy/nginx-kekulv-admin.conf.example)。

安装器已经创建默认密码文件，不需要 systemd drop-in。system 安装路径为
`/var/lib/kekulv/admin-password`；查看时注意不要把内容复制到日志或聊天：

```bash
sudo cat /var/lib/kekulv/admin-password
```

正常修改用户名和密码应在 WebUI“安全”页完成，不需要重启。若忘记凭据，可在可信终端把文件
覆盖为新的非空单行密码并重启；这会恢复初始用户名 `kkl`：

```bash
sudoedit /var/lib/kekulv/admin-password
sudo chown kekulv:kekulv /var/lib/kekulv/admin-password
sudo chmod 600 /var/lib/kekulv/admin-password
sudo systemctl restart kekulv.service
```

user 安装路径为 `~/.config/kekulv/admin-password`，修改后执行
`systemctl --user restart kekulv.service`。手动前台/包内脚本同样默认读取所选配置目录中的
`admin-password`；高级自定义路径才使用：

```bash
./kekulvd --admin-password-file /absolute/path/admin-password ...
# 或
KEKULV_ADMIN_PASSWORD_FILE=/absolute/path/admin-password ./scripts/start.sh
```

Admin 公开 HTML、模块 JS、字体和图标以呈现登录页；未登录 API/SSE 返回普通 JSON 401，
不会触发浏览器原生鉴权弹窗。daemon 不再校验 Host/Origin/peer，因此反代可原样传递公网域名和
HTTPS Origin，不再出现旧的 `untrusted_host` / `untrusted_origin` 403。登录请求在明文 HTTP
上会暴露凭据，公网入口必须使用 HTTPS；daemon 的 57879 端口仍不应直接暴露。

修改 Nginx 后执行 `nginx -t` 再 reload，并验证：

```bash
curl -I https://admin.example.com/admin/                    # 应返回登录壳 200
curl -i https://admin.example.com/admin/api/status           # 未登录应返回 JSON 401
```

浏览器登录后，fetch 与 EventSource 自动携带同源 Cookie；“退出登录”会同时撤销服务端会话并
清理 Cookie。daemon 重启后内存会话失效，需要重新登录。SIGHUP 只重载 `config.json`，不重读
凭据文件。

若确实把 Admin 改为非 loopback，仍要配置防火墙与外层 TLS。通常没有必要这样做，
loopback + HTTPS 反代更简单。

### Admin 鉴权信任模型

- `/admin/` 静态登录壳公开；除 session/login 外，API 与 SSE 必须提供有效会话 Cookie。旧单行
  文件初始用户名为 `kkl`；安全页修改后保存 Argon2 哈希 JSON。默认文件缺失时 daemon 拒绝启动。
- 会话 Cookie 为 HttpOnly、SameSite=Strict、24 小时有效；HTTPS 反代下带 Secure。写请求还需
  `X-Kekulv-CSRF`，凭据修改会撤销全部旧会话。
- `GET /healthz` 是唯一免鉴权入口，只返回 204 空响应，不暴露版本、配置或运行状态。
- Admin 禁止 CORS，写请求都必须是 `application/json`，并保留 CSP、`nosniff`、
  `X-Frame-Options: DENY` 与 API `no-store`。
- user scope 可操作固定的 systemd user unit；system scope 的 WebUI 只读显示 unit 状态，切换
  自启动仍返回 403，必须由管理员执行 `sudo systemctl enable|disable kekulv.service`。

完整接口见 [`specs/admin-api.md`](specs/admin-api.md)。

## 重载与 listener 重绑

编辑 `config.json` 后，根据启动方式选择对应入口：

1. 通过 `scripts/start.sh` 启动且没有覆盖 `KEKULVD_PID_FILE`：在部署包根目录读取脚本自己的
   `./kekulvd.pid`。

   ```bash
   kill -HUP "$(head -n 1 ./kekulvd.pid)"
   ```

   若设置过 `KEKULVD_PID_FILE`，必须改为读取那个明确路径。

2. 直接运行 daemon：daemon 自身把 PID 写入 `<config-dir>/kekulvd.pid`。默认 XDG 配置目录可用：

   ```bash
   kill -HUP "$(head -n 1 "${XDG_CONFIG_HOME:-$HOME/.config}/kekulv/kekulvd.pid")"
   ```

   使用过 `--config-dir <dir>` 时，应读取 `<dir>/kekulvd.pid`。`start.sh` 启动时 daemon 也会写这份
   配置目录 PID，但脚本自己的 pidfile 仍由 `KEKULVD_PID_FILE` 独立决定。

3. 由 systemd unit 管理：不要依赖部署目录的 `./kekulvd.pid`，直接让对应 unit 执行 ExecReload。

   ```bash
   systemctl --user reload kekulv
   sudo systemctl reload kekulv
   ```

也可调用 `POST /admin/api/reload`。上述入口使用同一条 reload 路径：先读取并验证完整 v6 配置，
再替换 Engine；proxy host/port 变化时重绑 proxy listener，Admin 57879 保持在线。失败时保留
上一份运行配置，并通过日志/diagnostics 报错。

## systemd 服务 unit

普通用户安装时，自动安装器会把 unit 放到 `~/.config/systemd/user/kekulv.service`：

```bash
systemctl --user status kekulv
journalctl --user -u kekulv -f
```

程序位于 `~/.local/share/kekulv`，配置位于 `~/.config/kekulv`。无桌面会话的服务器若需要
user unit 持续运行，`loginctl enable-linger` 涉及系统级状态，请由管理员审核后执行。

root/system 安装时，unit 位于 `/etc/systemd/system/kekulv.service`，程序在 `/opt/kekulv`，配置在
`/var/lib/kekulv`，daemon 以低权限 `kekulv` 用户运行：

```bash
sudo systemctl status kekulv
sudo journalctl -u kekulv -f
sudo systemctl enable --now kekulv
```

unit 默认 Admin `127.0.0.1:57879`（可用 drop-in/`Environment=`/`ExecStart` 覆盖）、UMask 0077、
`Restart=on-failure`、SIGHUP reload 和 SIGTERM 优雅停止。WebUI 的自启动开关只允许操作固定的
`kekulv.service`；system scope 即使启用 Admin 密码也不会授予 daemon root 权限，开关显示为
root 管理且禁用。

## Fedora / RHEL 说明

安装器与二进制按通用 systemd Linux 设计，**不是** Debian-only；在 Fedora 上需注意：

1. **SELinux**：从 `/tmp` 解压再用 `cp -a` 可能把 `tmp_t` 带进 `/opt`，导致 `status=203/EXEC`。
   安装器在 SELinux Enforcing/Permissive 下会 `cp --no-preserve=context` 并对安装树
   `restorecon -RF`。若仍失败：`sudo restorecon -RF /opt/kekulv` 或
   `sudo chcon -t bin_t /opt/kekulv/kekulvd`（需 `policycoreutils`）。
2. **user 服务 + SSH**：无桌面会话时 `systemctl --user` 常连不上 bus。可
   `loginctl enable-linger $USER` 后重登，或改用 `sudo` 装 system 服务。
3. **nologin 路径**：安装器自动选择 `/usr/sbin/nologin` 或 `/sbin/nologin`。
4. **firewalld**：仅当把 proxy/Admin 绑到非 loopback 时需要放行端口，例如
   `sudo firewall-cmd --add-port=57878/tcp --permanent && sudo firewall-cmd --reload`。
5. **二进制**：发布包为静态 musl ELF，不依赖 Fedora glibc 版本。

## Docker 安装（仅 Linux 主机）

**默认：拉 GHCR 已构建镜像，不在本机编译；数据在当前目录 `./config`。**

| 文件 | 说明 |
| --- | --- |
| `docker-compose.yml` | 同 `compose.yaml`（符号链接） |
| `compose.yaml` | 默认：GHCR 镜像 + host 网络 + `./config` 绑定 |
| `compose.bridge.example.yaml` | bridge + ports（Admin=`0.0.0.0`） |
| `compose.build.example.yaml` | 可选：叠加后本机 `docker build`（开发用） |
| `config/` | 宿主机数据目录 → 容器 `/config` |

仅 Linux host network；不面向 Docker Desktop。容器不会生成凭据，启动前必须在绑定目录创建
`admin-password`；首次打开 `/admin/` 后使用内置登录页。

### 首次启动（推荐）

```bash
git clone https://github.com/domoxiaojun/sumpter.git
cd kekulv

mkdir -p config          # 运行数据，无需 chown
umask 077
openssl rand -base64 32 > config/admin-password
docker compose pull      # 默认 ghcr.io/domoxiaojun/sumpter:latest
docker compose up -d

# 面板 http://127.0.0.1:57879/admin/
docker compose ps
docker compose logs -f kekulv
```

目录布局：

```text
.
├── docker-compose.yml   # → compose.yaml
├── compose.yaml
└── config/              # 绑定到 /config
    ├── config.json      # 首次启动自动 bootstrap
    ├── admin-password   # 启动前为单行初始密码；修改后为哈希 JSON，始终 0600
    ├── runtime.sqlite3  # 运行统计（首次启动后创建，WAL）
    ├── stats.json       # 旧版运行统计只读归档，新版本不读取或写入
```

### 升级（pull + up，配置不丢）

```bash
docker compose pull
docker compose up -d
```

默认 `latest`，**不用改版本号**。`./config` 在宿主机上，升级只换镜像。

`latest` 是 **最近一次成功的稳定版本 tag Container 构建**；普通 `main` push 不会构建镜像，
因此它不会因为文档或未发布的分支变化而刷新。要严格对齐发行版请钉 `v*` tag。

若要钉死某一版（可选）：

```bash
export KEKULV_VERSION='<version>'
export KEKULV_IMAGE="ghcr.io/domoxiaojun/sumpter:${KEKULV_VERSION}"
docker compose pull && docker compose up -d
```

### 镜像名

```text
ghcr.io/domoxiaojun/sumpter:latest    # 默认，最近成功的稳定版本 tag
ghcr.io/domoxiaojun/sumpter:<version> # 可选：钉死版本
```

仓库或 GHCR 为私有时必须先登录，否则 `pull` 会 401/denied：

```bash
echo "$GITHUB_TOKEN" | docker login ghcr.io -u YOUR_GITHUB_USER --password-stdin
```

### 自定义 Admin / bridge

`compose.yaml` 已把 `KEKULV_ADMIN_HOST` / `KEKULV_ADMIN_PORT` 传入容器（默认 127.0.0.1:57879）。
daemon 会按 `--config-dir /config` 自动读取 `./config/admin-password`，无需额外环境变量：

```bash
umask 077
openssl rand -base64 32 > config/admin-password
# 正常改凭据使用 WebUI 安全页；忘记凭据时才覆盖此文件并重启容器
```

bridge + 端口：

```bash
mkdir -p config
docker compose -f compose.bridge.example.yaml pull
docker compose -f compose.bridge.example.yaml up -d
```

### 可选：本机编译镜像

```bash
docker compose -f compose.yaml -f compose.build.example.yaml up -d --build
```

### 运维

```bash
docker compose restart kekulv
docker compose logs --tail=200 kekulv
docker compose down          # 保留 ./config
```

容器内无 systemd。为避免 chown，Compose 默认 `user: "0:0"` + host 网络，权限弱于
systemd 的 `kekulv` 系统用户。Proxy 绑 `0.0.0.0` 须配 Token/CIDR；Admin 已强制密码，
但非 loopback 公网仍必须置于 HTTPS 之后。

## 配置中的重试语义

schema v6 的全局 `retry`：

- `responseTimeoutSeconds:null`：代理不额外限制收到完整响应头的时间。
- `streamIdleTimeoutSeconds:null`：流式响应可无限空闲。
- `sessionStickyRetries:2`：同一次请求先在当前粘性调度组完成首次尝试，再额外重试 2 次；
  三次都遇到可重试故障后才访问其它调度组。其它组成功后立即把会话改绑到成功组；0 表示首次失败后立即切换。
  WebUI 事件里的「本次已故障转移」表示该请求换过入口，成功组会立即成为后续请求的粘性归属。
- `maxDeferredRounds:0`：所有可重试故障不设轮数上限（字段名为历史兼容保留）。
- `maxRetryDurationSeconds:0`：所有可重试故障不设跨轮总时长上限；示例默认 0（无限）。
- `pinnedIPConcurrency`：pinned IP 并发竞速数，必须大于 0。
- 备用映射的 `failoverTimeoutSeconds` 可省略；存在时必须大于 0，并与全局首响应超时取较小值。

可跨轮的 HTTP 状态为 `401/402/403/429/502/503/504/520-527/529/530`，另含首响应前 Timeout
和 ConnectionFailed。只有轮数与总时长**同时为 0**才真正无限。轮间按 `0.5s × 1.7` 指数
退避，数字 `Retry-After` 与退避取较大值，均封顶 30 秒。客户端断开会取消 sleep 和上游任务树；
499 不计成功或失败。自动测试已覆盖真实 TCP 断开，真实供应商长流仍需在目标 Linux 主机验收。

Claude Code 的 `/v1/messages` 与 Codex/OpenAI 客户端的 Chat、Responses、Legacy
Completions、Images、Alpha Search 和 Claude Count Tokens 共用上述路由、粘性、failover 与
无限重试状态机。入口协议是每个 Provider 的四态能力声明（默认 `auto`），完整路径与模式如下：

| 路径族 | 行为 |
|---|---|
| `/v1/messages` | Anthropic SourceFormat；Auto 入口使用原生 Native Adapter |
| `/v1/chat/completions` | OpenAI Chat SourceFormat；固定异协议入口才使用 Translator |
| `/v1/responses` | OpenAI Responses SourceFormat；固定异协议入口才使用 Translator |
| `/v1/responses/compact` | 独立 Responses Native Adapter；入口必须为 `auto` 或 `openai-responses` |
| `/v1/completions` | 独立 Legacy Completions Native Adapter；入口必须为 `auto` 或 `openai` |
| `/v1/messages/count_tokens` | 独立 Claude Count Tokens Native Adapter；入口必须为 `auto` 或 `anthropic` |
| `/v1/images/generations`、`/v1/images/edits` | 独立图片 Adapter；Edits 支持 JSON 与 multipart |
| `/v1/alpha/search` | 独立 Codex Alpha Search Native Adapter |

这些路径支持无 `/v1` 别名；Responses、Compact、Images 与 Alpha Search 另有
`/backend-api/codex/...` 直连别名。原生 JSON 请求除路由后的 `model` 外保留全部参数，Grok
图片的 `aspect_ratio` / `resolution` 等字段同样不会丢失；multipart 图片编辑不重建 body 或
`Content-Type`。WebUI 在 Provider 编辑器中选择入口协议；Security 页不再提供全局透传开关。
`auto` 只表示入口支持三种协议，不会发给上游；SourceFormat 只由路径决定，UA 和请求体形状
不参与协议选择。Provider 不再声明独立 WebSearch 能力；严格 WebSearch RequestPurpose 按最终
TargetFormat 自动保留 Anthropic 原生 `web_search`、为 OpenAI Chat 使用 `web_search_options`，
或为 Responses 使用内建 `web_search`。Grok 检索仍要求 Responses，固定 OpenAI Chat 入口不会
被隐式升级。

Responses WebSocket、Realtime / Live、Videos、Files 和 `/v1/models` 尚未接入；其中视频还
涉及创建后的查询、下载和凭据绑定，不能按普通 HTTP body 透传冒充支持。更完整的使用说明见
根目录 [`USAGE.md`](USAGE.md#4-协议与路径)。

想让 Web Admin 的「项目 Token 排行」按项目区分 Claude Code 请求（默认全堆在「未识别项目」），
在**跑 CC 的机器**上运行 `scripts/cc-project-attribution.sh install`。Web Admin 的**安全**页
有一份完整引导：当前是否已生效、三步命令（可直接复制）、macOS/Linux 与 shell 差异、三个实测
陷阱、回退命令。细节与原理见 [`USAGE.md` §8](USAGE.md#8-让-claude-code-按项目统计可选)。

## 原生构建与打包

不依赖 Docker 的双架构 musl 脚本需要人工准备：

- Rust/Cargo 1.88 或更新版本（Rust 2024 edition 与源码中的 let-chains）及 rustup
- `x86_64-unknown-linux-musl` target
- `aarch64-unknown-linux-musl` target
- Zig
- cargo-zigbuild
- `file`（打包前强制验证 ELF 与静态链接属性）

只检查，不安装、不编译、不创建 `dist/`：

```bash
./scripts/cross-build.sh --check
```

环境齐备后才执行：

```bash
./scripts/cross-build.sh
```

脚本分别运行 `cargo zigbuild --release --locked`，检查产物是静态 ELF，再把 binary、Web、
脚本（包括安装/卸载器）、systemd unit、README 和示例配置放入：

```text
dist/kekulv-linux-x86_64/
dist/kekulv-linux-aarch64/
```

脚本不会自动安装任何工具；旧同名产物会改名为带时间戳的 `.prev-*` 目录，不会直接删除。

## 真机冒烟

在对应架构 Linux 部署包内运行：

```bash
./scripts/smoke.sh
```

smoke 使用临时目录和随机端口，不碰真实配置，并验证：

- proxy 与独立 Admin listener 就绪，未认证 `/healthz` 返回 204
- `/admin/` 静态登录壳 200、未登录 API 401、登录会话下 status/config 200，写请求需 CSRF
- proxy 未知路径 404，`/__notify` 404
- SIGHUP 后双 listener 仍可用
- SIGTERM 优雅退出
- 仅有旧 `keys.json` 时拒绝启动，原文件不变且不生成 `config.json`

本机脚本不能完成 HTTPS 反代；VPS TLS、systemd、自启动、连接真实 daemon 的浏览器交互、
musl 兼容和真实上游请求必须另行验收。

## 常见问题

- **提示存在旧 keys.json**：这是预期的硬阻断，不会自动迁移。备份后人工转换为 v3。
- **Admin 连接失败**：默认 `127.0.0.1:57879`；若改过 `--admin-port` / `KEKULV_ADMIN_PORT`
  请用新端口。检查 daemon 日志和端口占用；proxy 57878 正常不代表 Admin 已启动。
- **Admin API 返回 401**：会话不存在或已过期，刷新 `/admin/` 后重新登录。旧单行凭据初始
  用户名为 `kkl`；若曾在安全页修改过，以修改后的用户名为准。确认反代保留 Cookie，并传递
  `X-Forwarded-Proto: https`。`/admin/` 静态资源本身不应返回鉴权 401。
- **HTTPS 反代仍返回 `untrusted_host` / `untrusted_origin` 403**：运行的仍是旧版本。
  检查实际二进制/镜像版本并重启，不要继续伪造上游 Host/Origin。
- **proxy 端口连接失败**：Admin 仍可打开时查看 status/diagnostics，确认 proxy 是否被停止、
  listener 是否绑定失败。
- **Web 返回 404 或空白**：检查 `web/` 与 `--web-root`；`--no-web` 只关闭静态页，不关闭 API。
- **代理 401**：若 `listener.authToken` 非空，客户端必须提供相同 token。
- **上游一直重试**：`maxDeferredRounds` 与 `maxRetryDurationSeconds` 都为 0 就是预期的无限模式；
  如需有限重试，至少把其中一项设为正数。客户端断开后应立即停止并记录 499。
- **交叉构建环境缺失**：运行 `cross-build.sh --check` 查看清单；脚本不会替你安装依赖。
