# Sumpter Linux（发布包二进制仍名为 `sumpterd`）

本文有两种阅读上下文：

1. **源码 monorepo**：本文件位于 `platforms/linux/README.md`。Rust 真源是仓库根 workspace 的 `sumpter-core` / `sumpter-runtime` / `sumpter-engine` 与 `sumpterd-linux`。
2. **独立发布包**：发布阶段把 `platforms/linux/` 提升为包根。包内二进制名为 `sumpterd`，配置目录 `~/.config/sumpter` 或 `/var/lib/sumpter`，systemd 单元 `sumpter.service`，环境变量 `SUMPTER_*`。

Linux 版以 standalone daemon 提供多协议代理、入口库、多个模型组、分流规则、failover、统计，以及与桌面 UI 信息架构对齐的本机 Web 管理界面。版本与 schema 与仓库根一致（现为 0.4.8 / schema v7）。用户安装看下文「自动安装、升级与卸载」。

Linux 专属边界：

- proxy listener：由 `config.json.listener.host/port` 决定，默认 `127.0.0.1:57878`。
- Admin/Web listener：默认 `127.0.0.1:57879`；可用启动参数或环境变量覆盖（见下文）。
- 配置：当前使用 schema v7 `config.json`；自动迁移 schema v3/v4/v5/v6，其他旧格式和旧 `keys.json`
  不读取。迁移会先创建 0600 的 `config.before-schema-v7-*.json` 原始备份，再原子写入并验证；已废弃入口字段会在迁移时删除。
- Web Admin：默认从配置目录 `admin-password` 初始化内置登录，会话使用 HttpOnly Cookie + CSRF；不提供通知页、Claude Hook 或 `/__notify`。
- 进程：默认前台运行，适合 systemd；SIGHUP 热重载，SIGTERM/SIGINT 优雅退出。
- 发行版：面向通用 systemd Linux（含 Fedora/RHEL 与 Debian/Ubuntu）；静态 musl 二进制，不依赖发行版 glibc 包。

## 验证状态

> 用户安装看「自动安装、升级与卸载」。下面是维护者边界，不是安装步骤。

源码树的 Rust 门禁在**仓库根**运行：`cargo fmt` / `check` / `test` / `clippy`，覆盖共享 crate、Linux adapter 和 `sumpterd-linux`。WebUI 在 `platforms/linux/webui/` 跑 `npm ci`、契约测试和生产构建。

源码检查统一从仓库根运行 `./scripts/check.sh`；`cross-build.sh` 从根 workspace 构建并打包。交叉编译、镜像与真实 systemd 流量分别验收，目标 Linux 的检查见本文「原生构建与打包」「真机冒烟」。

## 部署包结构

```text
sumpter-linux-<arch>/
├── sumpterd
├── config.example.json
├── compose.yaml                         # 可选：独立目录 Docker 部署
├── DOCKER.md
├── USAGE.md
├── CHANGELOG.md                         # 发布工作流写入的版本说明节选
├── web/
├── scripts/
│   ├── start.sh
│   ├── stop.sh
│   ├── smoke.sh
│   ├── install.sh
│   ├── bootstrap-install.sh
│   ├── bootstrap-uninstall.sh
│   ├── uninstall.sh
│   ├── migrate-kekulv.sh                # 旧 Kekulv 安装迁移，仅兼容已有部署
│   ├── setup-client-attribution.sh      # 统一归因安装器（推荐）
│   ├── client-attribution.mjs
│   ├── pi-project-attribution.ts
│   ├── gemini-sumpter-wrapper.mjs
│   ├── cc-project-attribution.sh        # 旧 Claude 配置器，仅兼容已有安装
│   └── grok-project-attribution.sh      # 旧 Grok 配置器，仅兼容已有安装
├── specs/admin-api.md
├── deploy/nginx-sumpter-admin.conf.example
├── sumpter.service
├── sumpter-system.service
├── LICENSE
├── logs/
└── README.md
```

`arch` 为 `x86_64` 或 `aarch64`。目标产物是对应架构的静态 musl ELF；必须在目标 Linux
机器用 `file ./sumpterd` 和 `scripts/smoke.sh` 复核，不能只凭交叉编译退出码判断。

## GitHub Actions 与发布资产

GitHub 实际执行的 workflow 位于仓库根 `.github/workflows/`，已按当前 monorepo 的根 workspace 和
`platforms/linux/` 输入适配。旧嵌套工作流已移除，Linux 与 macOS 共用唯一 Release 工作流；本机测试
DMG 用仓库根 `scripts/build-macos-dmg.sh`。

源码仓库的统一工作流：

- `ci.yml`：main push、pull request 或手动触发；Rust fmt/check/test/clippy、WebUI 构建与测试、macOS App 构建与测试、文档与资源同步检查。
- `release.yml`：使用同一个已有的 `vX.Y.Z` tag 同时构建 Linux 包、校验多架构容器并发布 GHCR 镜像和 macOS 包；所有构建成功后才运行唯一的 publish job。

Release 固定提供 Linux 包，并推送 GHCR：

```text
sumpter-linux-x86_64.tar.gz
sumpter-linux-aarch64.tar.gz
SHA256SUMS
ghcr.io/domoxiaojun/sumpter:<version>   # 同时打 latest / 大版本标签
```

所有第三方 Actions 固定到完整 commit SHA；Release 发布 job 才有 `contents:write` 和
`packages:write`。源码仓库为 [`domoxiaojun/sumpter`](https://github.com/domoxiaojun/sumpter)。
面向用户的二进制和镜像来自 GitHub Release 与 GHCR。许可证是 MIT，发布包内见本目录
[LICENSE](LICENSE)；源码树的 Cargo workspace 也声明 `license = "MIT"`。
推送与 Cargo workspace 版本一致的 `v<version>` tag 会创建 GitHub Release。

## 自动安装、升级与卸载

安装器按调用身份选择 scope：普通用户安装为 systemd user 服务；root 或 `sudo` 安装为 system
service，但 daemon 始终使用专用的低权限 `sumpter` 用户运行。公开安装从
[`domoxiaojun/sumpter`](https://github.com/domoxiaojun/sumpter) 的 GitHub Release 取包，并校验
`SHA256SUMS`：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/install.sh
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

首次安装会在配置目录生成 0600 随机 `admin-password`，不在终端回显，升级与普通卸载都会保留。
首次登录用户名为 `kkl`；登录后可在“安全”页修改用户名和密码，文件会迁移为 Argon2 哈希 JSON。
监听参数和高级凭据路径覆盖会写入 systemd drop-in；
HTTPS 反代通常无需改默认 loopback：

```bash
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter \
  --admin-password-file /absolute/path/admin-password
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter --admin-host 0.0.0.0 --admin-port 57879 \
  --admin-password-file /absolute/path/admin-password
# 或环境变量
SUMPTER_ADMIN_PASSWORD_FILE=/absolute/path/admin-password \
  bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

如需先审查已下载的脚本，可在安装命令前单独运行下面这条非交互命令：

```bash
sed -n '1,$p' /tmp/sumpter-install.sh
```

root/system 安装使用同一份 `install.sh`，但必须明确通过 `sudo` 运行：

```bash
sudo bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

钉死版本时加上 `--version vX.Y.Z`。`install.sh --repo` 会下载 `SHA256SUMS` 并校验当前架构的
tar.gz。`bootstrap-install.sh` 也可从
`https://github.com/domoxiaojun/sumpter/releases/latest/download` 取同名压缩包，并强制验证 `SHA256SUMS`，
校验失败时停止安装，同时检查 HTTPS、归档结构和符号链接。
同源校验用于验证完整性，不替代发布方身份签名。

可选静态镜像仍须手工同步，可能落后于 GitHub Release。仅在无法访问 GitHub 时把
`bootstrap-install.sh` 的 `--base-url` 指到镜像根目录。

### 从旧 Kekulv system 安装迁移

只适用于标准 root 安装：`kekulv.service`、`/opt/kekulv`、`/var/lib/kekulv`。user 安装、自定义
unit 或数据目录不在范围内。

在**旧服务器**上只要这一份脚本。它会按机器架构从 GitHub Release 下载
`sumpter-linux-x86_64.tar.gz` 或 `sumpter-linux-aarch64.tar.gz`，并核对 `SHA256SUMS`。
不必事先把发布包拷到服务器。

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-migrate-kekulv.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/migrate-kekulv.sh
sudo bash /tmp/sumpter-migrate-kekulv.sh --check
# 省略 --version 即下载 latest；钉死版本再加 --version v0.4.8
sudo bash /tmp/sumpter-migrate-kekulv.sh --admin-host 0.0.0.0
```

| 参数 | 作用 |
| --- | --- |
| （默认） | 下载 `https://github.com/domoxiaojun/sumpter/releases/latest/download/` 下当前架构包 |
| `--version vX.Y.Z` | 改为该 tag 的 Release 资产，例如 `.../download/v0.4.8/` |
| `--admin-host` / `--admin-port` | 写入新服务的 Admin 监听，与下载无关；省略则沿用旧 drop-in |
| `--check` | 只检查布局和参数，**不下载、不停服、不改文件** |

`--check` 通过后才会下载。下载并校验成功后才停旧服务，把 `/var/lib/kekulv` 完整复制到
`/var/lib/sumpter`（含配置、登录凭据、SQLite/WAL），所有者改为 `sumpter`。原
`/var/lib/kekulv` 不动。新服务同一 PID 且 `/healthz` 连续五次通过后，旧程序和 unit 才挪到
`/var/lib/sumpter-migration.*`。失败会停新服务并尽量恢复旧服务启停状态。

已有 `/opt/sumpter` 或 `sumpter.service`、自定义 ExecStart/drop-in、外部密码路径或链接数据
会在停服前拒绝。公网 Admin 仍走原来的 HTTPS 反代。`/healthz` 通过不等于代理请求已验证。

迁移改的是服务器上的 daemon，**不会**改笔记本上的 Claude / Grok / Gemini / Codex / pi。客户端继续把
Base URL 指到这台机器；项目统计要在**启动客户端的电脑**上处理，见下一节，不要在这台
Linux 上对 shell 做归因安装。

### 从已解压发布包安装与卸载

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
| 普通用户 | `~/.local/share/sumpter` | `~/.local/share/sumpter.previous` | `~/.config/sumpter` | `~/.config/systemd/user/sumpter.service` |
| root / sudo | `/opt/sumpter` | `/opt/sumpter.previous` | `/var/lib/sumpter` | `/etc/systemd/system/sumpter.service` |

首次登录前可在可信终端查看安装器生成的 Admin 密码（不要把输出贴到日志或聊天）。在安全页
修改凭据后，该文件保存的是哈希 JSON，不能再用于回读明文密码：

```bash
cat ~/.config/sumpter/admin-password                 # user 安装
sudo cat /var/lib/sumpter/admin-password             # system 安装
```

卸载默认保留配置。已安装包内的卸载器可直接运行：

```bash
~/.local/share/sumpter/scripts/uninstall.sh
```

root/system 安装则运行：

```bash
sudo /opt/sumpter/scripts/uninstall.sh
```

也可从仓库下载引导卸载器；它会按当前身份调用已安装包内的卸载器。普通用户：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-uninstall.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/bootstrap-uninstall.sh
bash /tmp/sumpter-uninstall.sh
```

root/system 安装：

```bash
sudo bash /tmp/sumpter-uninstall.sh
```

只有明确要永久删除 `config.json`、`runtime.sqlite3`、旧 `stats.json` 归档和其他运行数据时，才在所选卸载命令后加上
`--purge`，例如：

```bash
bash /tmp/sumpter-uninstall.sh --purge
```

GitHub Release 安装路径要求仓库公开（或已登录可下载资产）。私有源码、单独托管二进制时，把
`bootstrap-install.sh --base-url` 指到自备 HTTPS 目录，目录内需提供
`sumpter-linux-x86_64.tar.gz`、`sumpter-linux-aarch64.tar.gz` 和对应的 `SHA256SUMS`。

## 首次配置

普通用户默认配置目录遵循 XDG：

- 设置了 `XDG_CONFIG_HOME`：`$XDG_CONFIG_HOME/sumpter`
- 否则：`~/.config/sumpter`
- 也可用 `--config-dir <dir>` 覆盖

root/system service 的安装器显式传入 `/var/lib/sumpter`；不要在该路线把配置放进 root 的 HOME。

建议显式安装合成示例，再通过 WebUI 或编辑器填写真实入口：

```bash
config_base="${XDG_CONFIG_HOME:-$HOME/.config}"
install -d -m 700 "$config_base/sumpter"
install -m 600 ./config.example.json "$config_base/sumpter/config.json"
```

> 不要用桌面/Rust 版本的 `config.toml` 内容覆盖这里的 `config.json`，也不要把它直接改名为
> `config.json`。Linux daemon 只解析 schema v7 JSON；名为 `config.toml` 的文件不会被读取，
> TOML 内容一旦放进 `config.json` 会让 daemon 在 Admin 端口启动前退出，WebUI 随后只能显示
> “无法连接管理接口 / Failed to fetch”。请以 `config.example.json` 为结构，通过 WebUI 或人工把
> Provider、模型和路由值转换到 JSON。

若配置目录已有 `admin-password`，但既没有 `config.json` 也没有旧 `keys.json`，daemon 会创建
安全的 schema v7 bootstrap 空配置；显式安装示例的好处是能直接看到字段结构和停用的合成入口。

示例入口全部 `enabled:false`，域名使用 `.invalid`，secret 为合成的 `sk-test-...`；替换真实
地址和 secret 后再启用。`config.json` 含明文 secret，权限必须保持 0600。

> 旧 Linux Swift 版的 `keys.json` 不会自动迁移。配置目录只有 `keys.json` 时，新 daemon
> 必须报错并非零退出，且不能改写原文件。请先备份旧文件，再人工转换为 schema v7 `config.json`；
> 不要通过删除旧文件来掩盖未完成的转换。

## 启动与连接

不使用安装器时，先在默认配置目录创建初始单行密码文件（输入不会回显）：

```bash
install -d -m 700 ~/.config/sumpter
read -rsp 'Admin password: ' SUMPTER_NEW_ADMIN_PASSWORD
printf '%s\n' "$SUMPTER_NEW_ADMIN_PASSWORD" > ~/.config/sumpter/admin-password
unset SUMPTER_NEW_ADMIN_PASSWORD
printf '\n'
chmod 600 ~/.config/sumpter/admin-password
```

前台运行：

```bash
./sumpterd
```

后台运行：

```bash
./scripts/start.sh
./scripts/stop.sh
```

`start.sh` 使用 nohup、私有 pidfile 和日志轮转，并探测 Admin 是否就绪。常用覆盖：

| 环境变量 | 默认 | 作用 |
| --- | --- | --- |
| `SUMPTERD_BIN` | `../sumpterd` | daemon 路径 |
| `SUMPTERD_CONFIG_DIR` | XDG 默认 | 追加 `--config-dir` |
| `SUMPTERD_WEB_ROOT` | `../web` | Web 静态资源目录 |
| `SUMPTERD_PID_FILE` | `../sumpterd.pid` | start/stop 共用 pidfile |
| `SUMPTERD_LOG_DIR` | `../logs` | nohup 日志目录 |
| `SUMPTERD_START_TIMEOUT` | `15` | 启动等待秒数 |
| `SUMPTERD_STOP_TIMEOUT` | `15` | 优雅停止等待秒数 |
| `SUMPTER_ADMIN_HOST` | `127.0.0.1` | Admin 绑定地址（systemd `Environment=` 可用） |
| `SUMPTER_ADMIN_PORT` | `57879` | Admin 端口 |
| `SUMPTER_ADMIN_PASSWORD_FILE` | `<config-dir>/admin-password` | 高级 Admin 凭据文件路径覆盖 |

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
[`deploy/nginx-sumpter-admin.conf.example`](deploy/nginx-sumpter-admin.conf.example)。

安装器已经创建默认密码文件，不需要 systemd drop-in。system 安装路径为
`/var/lib/sumpter/admin-password`；查看时注意不要把内容复制到日志或聊天：

```bash
sudo cat /var/lib/sumpter/admin-password
```

正常修改用户名和密码应在 WebUI“安全”页完成，不需要重启。若忘记凭据，可在可信终端把文件
覆盖为新的非空单行密码并重启；这会恢复初始用户名 `kkl`：

```bash
sudoedit /var/lib/sumpter/admin-password
sudo chown sumpter:sumpter /var/lib/sumpter/admin-password
sudo chmod 600 /var/lib/sumpter/admin-password
sudo systemctl restart sumpter.service
```

user 安装路径为 `~/.config/sumpter/admin-password`，修改后执行
`systemctl --user restart sumpter.service`。手动前台/包内脚本同样默认读取所选配置目录中的
`admin-password`；高级自定义路径才使用：

```bash
./sumpterd --admin-password-file /absolute/path/admin-password ...
# 或
SUMPTER_ADMIN_PASSWORD_FILE=/absolute/path/admin-password ./scripts/start.sh
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
  `X-Sumpter-CSRF`，凭据修改会撤销全部旧会话。
- `GET /healthz` 是唯一免鉴权入口，只返回 204 空响应，不暴露版本、配置或运行状态。
- Admin 禁止 CORS，写请求都必须是 `application/json`，并保留 CSP、`nosniff`、
  `X-Frame-Options: DENY` 与 API `no-store`。
- user scope 可操作固定的 systemd user unit；system scope 的 WebUI 只读显示 unit 状态，切换
  自启动仍返回 403，必须由管理员执行 `sudo systemctl enable|disable sumpter.service`。

完整接口见 [`specs/admin-api.md`](specs/admin-api.md)。

## 重载与 listener 重绑

编辑 `config.json` 后，根据启动方式选择对应入口：

1. 通过 `scripts/start.sh` 启动且没有覆盖 `SUMPTERD_PID_FILE`：在部署包根目录读取脚本自己的
   `./sumpterd.pid`。

   ```bash
   kill -HUP "$(head -n 1 ./sumpterd.pid)"
   ```

   若设置过 `SUMPTERD_PID_FILE`，必须改为读取那个明确路径。

2. 直接运行 daemon：daemon 自身把 PID 写入 `<config-dir>/sumpterd.pid`。默认 XDG 配置目录可用：

   ```bash
   kill -HUP "$(head -n 1 "${XDG_CONFIG_HOME:-$HOME/.config}/sumpter/sumpterd.pid")"
   ```

   使用过 `--config-dir <dir>` 时，应读取 `<dir>/sumpterd.pid`。`start.sh` 启动时 daemon 也会写这份
   配置目录 PID，但脚本自己的 pidfile 仍由 `SUMPTERD_PID_FILE` 独立决定。

3. 由 systemd unit 管理：不要依赖部署目录的 `./sumpterd.pid`，直接让对应 unit 执行 ExecReload。

   ```bash
   systemctl --user reload sumpter
   sudo systemctl reload sumpter
   ```

也可调用 `POST /admin/api/reload`。上述入口使用同一条 reload 路径：先读取并验证完整 v7 配置，
再替换 Engine；proxy host/port 变化时重绑 proxy listener，Admin 57879 保持在线。失败时保留
上一份运行配置，并通过日志/diagnostics 报错。

## systemd 服务 unit

普通用户安装时，自动安装器会把 unit 放到 `~/.config/systemd/user/sumpter.service`：

```bash
systemctl --user status sumpter
journalctl --user -u sumpter -f
```

程序位于 `~/.local/share/sumpter`，配置位于 `~/.config/sumpter`。无桌面会话的服务器若需要
user unit 持续运行，`loginctl enable-linger` 涉及系统级状态，请由管理员审核后执行。

root/system 安装时，unit 位于 `/etc/systemd/system/sumpter.service`，程序在 `/opt/sumpter`，配置在
`/var/lib/sumpter`，daemon 以低权限 `sumpter` 用户运行：

```bash
sudo systemctl status sumpter
sudo journalctl -u sumpter -f
sudo systemctl enable --now sumpter
```

unit 默认 Admin `127.0.0.1:57879`（可用 drop-in/`Environment=`/`ExecStart` 覆盖）、UMask 0077、
`Restart=on-failure`、SIGHUP reload 和 SIGTERM 优雅停止。WebUI 的自启动开关只允许操作固定的
`sumpter.service`；system scope 即使启用 Admin 密码也不会授予 daemon root 权限，开关显示为
root 管理且禁用。

## Fedora / RHEL 说明

安装器与二进制按通用 systemd Linux 设计，**不是** Debian-only；在 Fedora 上需注意：

1. **SELinux**：从 `/tmp` 解压再用 `cp -a` 可能把 `tmp_t` 带进 `/opt`，导致 `status=203/EXEC`。
   安装器在 SELinux Enforcing/Permissive 下会 `cp --no-preserve=context` 并对安装树
   `restorecon -RF`。若仍失败：`sudo restorecon -RF /opt/sumpter` 或
   `sudo chcon -t bin_t /opt/sumpter/sumpterd`（需 `policycoreutils`）。
2. **user 服务 + SSH**：无桌面会话时 `systemctl --user` 常连不上 bus。可
   `loginctl enable-linger $USER` 后重登，或改用 `sudo` 装 system 服务。
3. **nologin 路径**：安装器自动选择 `/usr/sbin/nologin` 或 `/sbin/nologin`。
4. **firewalld**：仅当把 proxy/Admin 绑到非 loopback 时需要放行端口，例如
   `sudo firewall-cmd --add-port=57878/tcp --permanent && sudo firewall-cmd --reload`。
5. **二进制**：发布包为静态 musl ELF，不依赖 Fedora glibc 版本。

## Docker 安装（独立目录）

推荐创建一个独立 `sumpter/`，其中放 `compose.yaml` 和 `config/`。配置、密码、SQLite、会话/资源绑定和诊断捕获统一挂载在 `./config:/config`；停机后复制整个目录即可迁移，不依赖源码仓库路径。

完整步骤、环境变量、权限、升级和迁移见 [Docker 部署说明](DOCKER.md)。要求 Linux 与 Docker Compose 2.24+。

- 默认拉 GHCR 镜像，使用 **bridge 网络**，宿主机端口默认只绑定 `127.0.0.1`。
- 不指定容器用户，使用镜像默认用户（root）运行，不需要配置 UID/GID；Compose 同时限制为只读根文件系统、`cap_drop: ALL` 与 `no-new-privileges`，仅 `config/` 与 `/tmp` 可写。
- 默认值已经写入 `compose.yaml`，需要改镜像、端口、数据目录或日志参数时直接编辑该文件。
- `init` 服务只创建缺失的初始配置和随机密码，不覆盖旧文件；首次生成时会把初始密码直接打印到 init 日志（只打印一次），方便首次登录，登录后请立即在 WebUI 改密。不想让明文进容器日志时，先按 [Docker 部署说明](DOCKER.md) 的「自备 `config.json` 与初始密码」自己生成凭据即可，init 不会重复打印。
- 新 Linux 发布包携带 `compose.yaml`、`DOCKER.md`；旧包缺少时需单独下载。
- 旧 host 网络部署升级模板前，必须按 [切换说明](DOCKER.md#从旧-host-网络-compose-切换) 核对代理内部监听；数据目录由容器创建，非 root 用户备份时需用 `sudo`。
- 容器日志由 Docker 管理，不在 `config/` 中；需要迁移日志时先导出。首次生成的初始密码会出现在 init 日志里，改密后即失效，但导出日志前仍需确认。密码和备份不要入库。

## 配置中的重试语义

schema v7 的全局 `retry`：

- `responseTimeoutSeconds:null`：代理不额外限制收到完整响应头的时间。
- `streamIdleTimeoutSeconds:null`：流式响应可无限空闲。
- `max500Retries:0`：当前入口收到 HTTP 500 后的额外重试次数；0 表示不额外重试。
- `failoverOn500:true`：HTTP 500 重试耗尽后切换到下一个入口；设为 `false` 则在当前入口直接返回 500。
- `retryDelaySeconds:null`：不配置透传秒数；设置正数后由 `passThroughRetryDelay` 决定是否返回该秒数并附带 `Retry-After`。
- `passThroughRetryDelay:true`：将最终失败响应中的 `retry_delay` 与 `Retry-After` 透传给客户端；关闭则隐藏这两个字段。
- `sessionStickyRetries:2`：同一次请求在当前粘性调度组遇到非 500 可重试故障后，再额外重试 2 次；
  三次都遇到可重试故障后才访问其它调度组。其它组成功后立即把会话改绑到成功组；0 表示首次失败后立即切换。
  WebUI 事件里的「本次已故障转移」表示该请求换过入口，成功组会立即成为后续请求的粘性归属。
- `maxDeferredRounds:0`：所有可重试故障不设轮数上限（字段名为历史兼容保留）。
- `maxRetryDurationSeconds:0`：所有可重试故障不设跨轮总时长上限；示例默认 0（无限）。
- 备用映射的 `failoverTimeoutSeconds` 可省略；存在时必须大于 0，并与全局首响应超时取较小值。

可跨轮的 HTTP 状态为 `401/402/403/429/502/503/504/520-527/529/530`；HTTP 500 仅按 `max500Retries`
在当前入口内重试，是否切换入口由 `failoverOn500` 控制，不进入跨轮无限重试。另含首响应前 Timeout
和 ConnectionFailed。只有轮数与总时长**同时为 0**才真正无限。轮间按 `0.5s × 1.7` 指数
退避，数字 `Retry-After` 与退避取较大值，均封顶 30 秒。客户端断开会取消 sleep 和上游任务树；
499 不计成功或失败。自动测试已覆盖真实 TCP 断开，真实供应商长流仍需在目标 Linux 主机验收。

Claude Code 的 `/v1/messages` 与 Codex/OpenAI 客户端的 Chat、Responses、Legacy
Completions、Images、Alpha Search 和 Claude Count Tokens 共用上述路由、粘性、failover 与
无限重试状态机。入口协议是每个 Provider 的五态能力声明（默认 `auto`），完整路径与模式如下：

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
`auto` 只表示入口支持四种协议，不会发给上游；SourceFormat 只由路径决定，UA 和请求体形状
不参与协议选择。Provider 不再声明独立 WebSearch 能力；严格 WebSearch RequestPurpose 按最终
TargetFormat 自动保留 Anthropic 原生 `web_search`、为 OpenAI Chat 使用 `web_search_options`，
或为 Responses 使用内建 `web_search`。Grok 检索仍要求 Responses，固定 OpenAI Chat 入口不会
被隐式升级。

Responses WebSocket、Realtime / Live、Files、Videos 与 `/v1/models` 已接入共享 engine。`GET /v1/models` 按本地 mapping 生成目录（Codex `client_version` 返回 `{models:[...]}`），不转发到上游。其余资源 HTTP 与 WebSocket 由 engine 做统一鉴权、按 mapping 选择 Provider、必要的上游模型名替换、failover 和连接 relay；原始 path/query、请求与响应、二进制内容，以及两类 WebSocket 的 path/query 与文本/二进制/关闭帧都交给上游，不在本地重建协议或改写路径别名。Provider 的实际权限和媒体/Realtime 能力仍需目标上游实测。更完整的使用说明见同目录 [`USAGE.md`](USAGE.md#4-协议与路径)（源码树里对应仓库根 `USAGE.md`）。

想按项目统计时，在**启动 Claude / Grok / Gemini / Codex / pi 的那台电脑**操作。
需要 Node.js 18+，请以客户端的普通用户执行：

```bash
curl --proto '=https' --tlsv1.2 -fLo setup-client-attribution.sh https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh
bash setup-client-attribution.sh install all
```

setup 自动获取所选客户端需要的安装器和扩展。重复 install 可更新；status 检查本机配置，
restore 恢复所选客户端安装前状态。所有客户端均需新开终端并重新启动；pi 的 `/reload` 不会加载 shell 包装器配置。
无法访问 GitHub 时，可设置 SUMPTER_BASE_URL 从已运行的代理下载资源。
客户端连接和 Codex Desktop 的适用边界见[归因安装](USAGE.md#linuxmacos-客户端归因脚本统一安装)。

## 原生构建与打包

`cross-build.sh` 需要包含 Rust workspace 的源码树；仅有二进制的独立 Linux 发布包不能重新编译。在当前 monorepo 根目录运行
`./platforms/linux/scripts/cross-build.sh` 会构建根 workspace 的 `sumpterd-linux`，并把发布包放到
`platforms/linux/dist/`；其它安装/卸载脚本仍以打包后的 Linux 发布树为运行根目录。

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
dist/sumpter-linux-x86_64/
dist/sumpter-linux-aarch64/
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

- **提示存在旧 keys.json**：这是预期的硬阻断，不会自动迁移。备份后人工转换为当前 schema v7。
- **Admin 连接失败**：默认 `127.0.0.1:57879`；若改过 `--admin-port` / `SUMPTER_ADMIN_PORT`
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

### Gemini CLI

发布包的 `scripts/gemini-sumpter-wrapper.mjs`（源码为 `platforms/linux/scripts/gemini-sumpter-wrapper.mjs`）
将 Gemini CLI 配置为 Developer API Gateway，并清除 Vertex/GCA/ADC 环境变量。
设置 `SUMPTER_GEMINI_BASE_URL`、`SUMPTER_AUTH_TOKEN`，可选 `SUMPTER_GEMINI_PROJECT` 与
`SUMPTER_GEMINI_SESSION_ID` 后通过 `node` 运行 wrapper；请求使用原生 Gemini JSON/SSE，
项目和会话归因只写入 Sumpter 事件，不发送给 Provider。

包装脚本也可从
[仓库 raw](https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/gemini-sumpter-wrapper.mjs)
下载；运行中的 listener 仍提供 `/__sumpter/gemini-sumpter-wrapper.mjs`。Claude Code、Grok、Gemini
和 pi 的安装命令见 [`USAGE.md` 的客户端脚本安装说明](USAGE.md#linuxmacos-客户端归因脚本统一安装)。
