# Sumpter Linux

Linux 版以后台进程运行，通过 WebUI 管理入口、模型组、请求与统计。本文对应 **0.4.23 / schema v7**，支持 x86_64 / aarch64 的静态 musl 发布包。

容器用户直接阅读 [Docker Compose 部署教程](DOCKER.md)。下面介绍在 Linux 上直接安装 systemd 服务；两种方式共用 [使用手册](USAGE.md)。

## 安装 systemd 服务

在需要运行代理的 Linux 主机执行：

```bash
curl --proto '=https' --tlsv1.2 -fLo /tmp/sumpter-install.sh \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/install.sh
bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter
```

安装器下载当前架构的最新 Release，校验 `SHA256SUMS`，安装程序和 WebUI，并准备配置目录与初始管理密码。首次生成的随机密码不回显，安装末尾会显示读取位置。

按执行身份选择一种安装方式：

| 方式 | 安装命令 | 程序目录 | 配置目录 |
| --- | --- | --- | --- |
| 当前用户 | 上面的 `bash` 命令 | `~/.local/share/sumpter` | `${XDG_CONFIG_HOME:-~/.config}/sumpter` |
| 系统服务 | `sudo bash /tmp/sumpter-install.sh --repo domoxiaojun/sumpter` | `/opt/sumpter` | `/var/lib/sumpter` |

system 服务由专用低权限 `sumpter` 用户运行。无桌面会话的服务器通常更适合 system 服务；user 服务依赖该用户的 systemd session bus。

需要固定版本时追加 `--version v0.4.23`。安装器也接受 `--admin-host`、`--admin-port`、`--admin-password-file`，将管理设置写入 systemd drop-in。升级时原有配置与密码会保留。

如果已下载并解压官方 Linux 包，在包目录执行 `bash scripts/install.sh` 或 `sudo bash scripts/install.sh`，无需再次指定 `--repo`。

## 首次登录与接入

管理页默认是 `http://127.0.0.1:57879/admin/`，代理默认是 `http://127.0.0.1:57878`。

读取首次密码，按安装方式择一：

```bash
cat "${XDG_CONFIG_HOME:-$HOME/.config}/sumpter/admin-password"
```

```bash
sudo cat /var/lib/sumpter/admin-password
```

首次用户名为 `kkl`。登录后在「安全」修改管理凭据，随后添加真实入口、配置模型组和代理入站 Token，按 [使用手册](USAGE.md) 接入客户端。管理密码、代理 Token 和上游 Key 各自独立。

远程服务器可从自己的电脑建立隧道：

```bash
ssh -N -L 57879:127.0.0.1:57879 -L 57878:127.0.0.1:57878 your-user@your-server
```

然后从本机访问上述地址。长期公开管理访问使用 HTTPS 反代，示例见 [Nginx 配置](deploy/nginx-sumpter-admin.conf.example)；反代代理数据面（含 SSE 与 WebSocket）见 [数据面 Nginx 配置](deploy/nginx-sumpter-proxy.conf.example)，并把代理地址填进 `listener.trustedProxyCIDRs` 让运行事件显示真实客户端 IP。局域网访问需调整对应监听与防火墙，并使用服务器实际 IP。

## 服务管理

| 操作 | 当前用户安装 | system 安装 |
| --- | --- | --- |
| 状态 | `systemctl --user status sumpter.service` | `sudo systemctl status sumpter.service` |
| 最近日志 | `journalctl --user -u sumpter.service -n 100 --no-pager` | `sudo journalctl -u sumpter.service -n 100 --no-pager` |
| 重载 JSON | `systemctl --user reload sumpter.service` | `sudo systemctl reload sumpter.service` |
| 重启进程 | `systemctl --user restart sumpter.service` | `sudo systemctl restart sumpter.service` |
| 停止 | `systemctl --user stop sumpter.service` | `sudo systemctl stop sumpter.service` |

WebUI 保存配置会校验并应用。手工编辑 `config.json` 后使用重载；管理监听地址或密码文件变化需要重启。停止代理监听不会关闭 Admin 页面，停止 systemd 服务则会结束整个进程。

system 服务的开机启动由管理员通过 `sudo systemctl enable sumpter.service` 管理。WebUI 不会为此提权；user 服务的可用控制范围以界面提示为准。

## 配置和数据

每个实例使用一个独立配置目录，包含 `config.json`、`admin-password`、`runtime.sqlite3`，以及按实际使用生成的会话与资源绑定、诊断捕获。数据库随程序提供，不需要另装数据库服务。

配置与密码权限为 0600，目录为 0700，所有者应是服务运行用户。示例配置见 [config.example.json](config.example.json)，其中入口和模型组默认停用。

代理监听保存在 `config.json.listener`。管理监听通过启动参数或环境变量设置，优先级为 CLI、环境变量、默认值：

| 环境变量 | 默认 |
| --- | --- |
| `SUMPTER_ADMIN_HOST` | `127.0.0.1` |
| `SUMPTER_ADMIN_PORT` | `57879` |
| `SUMPTER_ADMIN_PASSWORD_FILE` | 配置目录的 `admin-password` |
| `SUMPTER_WEB_ROOT` | 自动查找 WebUI；安装器提供正确位置 |

## 升级、备份与卸载

升级前停止服务，备份整个配置目录，再启动并重跑原安装命令。备份包括可能存在的 SQLite WAL，不要只复制数据库主文件。升级后检查版本、原有入口、历史统计和一条真实请求。

普通卸载保留配置和数据，按安装方式执行：

```bash
bash ~/.local/share/sumpter/scripts/uninstall.sh
```

```bash
sudo bash /opt/sumpter/scripts/uninstall.sh
```

`--purge` 才会永久删除对应配置目录，仅在明确不再需要这些数据时使用。

若来源是旧 Kekulv system 安装，使用包内 `scripts/migrate-kekulv.sh --check` 先检查适用布局，再按脚本提示迁移。自定义目录和运行中的数据库不能直接当作新安装覆盖。

## 故障排查

先确认进程状态，再检查监听：

```bash
curl --noproxy '*' -i http://127.0.0.1:57879/healthz
```

204 表示管理监听存活。未登录访问 `/admin/api/status` 返回 401 是正常的；WebUI 登录成功后才能访问管理 API。

| 现象 | 处理方向 |
| --- | --- |
| 服务无法启动 | 查看 journal，核对密码文件、配置 JSON、目录权限和端口占用 |
| SSH 中 user 服务连接 bus 失败 | 检查用户会话环境；无人值守主机可选择 system 安装 |
| 管理页正常但请求失败 | 检查代理地址、Token、入口映射和启用的模型组 |
| 忘记管理凭据 | 停服务，备份密码文件，写入新的非空单行密码，保持原所有者与 0600 后重启；用户名恢复为 `kkl` |
| 磁盘或存储错误 | 先检查剩余空间与数据库目录权限，再按诊断页处理；重建会丢失原库数据 |

需要更细的现象分类时，阅读包内 [Compose 排障](DOCKER.md#故障排查)。管理 API 详见 [接口参考](specs/admin-api.md)。

## 发布包与源码

发布包中的可执行文件名为 `sumpterd`，同时提供 `web/`、`scripts/`、Compose 模板、配置示例和本地手册。直接前台运行时，须先准备配置目录和管理密码，然后执行：

```bash
./sumpterd --config-dir /path/to/sumpter-config --web-root ./web
```

源码位于完整仓库根 workspace；对应构建目标名为 `sumpterd-linux`。开发和交叉打包见 [在线开发指南](https://github.com/domoxiaojun/sumpter/blob/main/docs/development.md)。许可证见 [LICENSE](LICENSE)，版本变化见 [CHANGELOG](CHANGELOG.md)。
