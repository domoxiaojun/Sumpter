# 使用 Docker Compose 部署 Sumpter

本教程在 Linux 部署机上执行。完成后，你会得到一个 Sumpter 服务、一个 Web 管理页，以及独立的持久化数据目录。

使用官方镜像 `ghcr.io/domoxiaojun/sumpter:latest`，支持 amd64 / arm64。需要 Docker Engine、Compose v2 插件（支持 `up --wait`）、curl、OpenSSL 和 sudo 权限。无需在部署机安装 Rust、Node.js 或数据库。

以下是首次部署流程。已有部署直接跳到“备份与升级”，不要重新复制示例覆盖实际配置。

## 1. 创建部署目录并复制示例

选择一个新的目录，例如当前用户的 `~/sumpter`：

```bash
mkdir -m 700 ~/sumpter
cd ~/sumpter

curl --proto '=https' --tlsv1.2 -fLo compose.example.yaml \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/compose.yaml
curl --proto '=https' --tlsv1.2 -fLo config.example.json \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/config.example.json

cp compose.example.yaml compose.yaml
sudo install -d -m 700 config
sudo install -m 600 config.example.json config/config.json
```

仓库中的 `compose.yaml` 就是部署模板，下载时另存为 `compose.example.yaml`，再复制成实际使用的文件，方便以后对照。手头已有源码或 Linux 发布包时，也可以直接复制其中的 `compose.yaml` 和 `config.example.json`，无需下载。

容器使用镜像默认 root 用户，且丢弃额外 capabilities。标准 Linux Docker 部署应让 `config/` 及其中的敏感文件由 root 拥有，上面的 `sudo install` 已完成这一点；宿主机查看或编辑时使用 sudo。自定义 rootless / user namespace 部署需按实际 UID 映射调整所有权。

## 2. 修改实际配置

打开刚复制的配置：

```bash
sudo vi config/config.json
```

只编辑 `config/config.json`，保留下载的示例以备对照。首先把 `listener` 改为：

```json
"listener": {
  "host": "0.0.0.0",
  "port": 57878,
  "allowedCIDRs": [],
  "authToken": "替换为你生成的代理入站 Token",
  "trustedProxyCIDRs": []
}
```

可以用 `openssl rand -hex 32` 生成随机 Token，粘贴到 `authToken`，并在之后的客户端配置中使用同一个值。

这里的 `0.0.0.0` 是**容器内部**监听地址，必须能接收 Compose 转发的连接。配置示例默认的 `127.0.0.1` 适合本机直接运行，在 bridge 容器里必须修改。容器内部保持 `57878`，宿主机端口在 Compose 中设置。

此时可以保留全部示例入口和模型组为停用，启动后通过 WebUI 填写真实上游。若手工填写，需同时替换入口地址、API Key、模型映射，并启用模型组和入口绑定。`.invalid` 示例域名无法发送真实请求。

## 3. 创建管理密码文件

管理页密码与上一节的代理入站 Token 分别使用。运行下面命令生成密码文件；`set -C` 会阻止覆盖已有文件：

```bash
sudo sh -c 'umask 077; set -C; openssl rand -hex 32 > config/admin-password'
sudo chmod 600 config/config.json config/admin-password
```

首次管理用户名为 `kkl`。在自己的终端查看初始密码：

```bash
sudo cat config/admin-password
```

保管这条密码，下一步用于登录。登录后在「安全」修改用户名和密码，文件随后变为哈希 JSON，不能再通过 `cat` 找回明文密码。

## 4. 检查 Compose 配置

部署目录应为：

```text
sumpter/
├── compose.example.yaml       # 下载的模板，供对照
├── compose.yaml               # 实际使用的 Compose 配置
├── config.example.json        # 原始 JSON 示例
└── config/                    # 唯一必须持久化的数据目录
    ├── config.json            # 已修改容器监听地址
    └── admin-password         # 已创建管理密码
```

默认端口映射是：

```yaml
ports:
  - "127.0.0.1:57878:57878"
  - "127.0.0.1:57879:57879"
```

格式为 `宿主机地址:宿主机端口:容器端口`。默认只允许从宿主机访问；例如本机端口冲突时可把第一条改为 `127.0.0.1:17878:57878`，客户端改用 `17878`，容器配置保持原值。

模板无需 `.env`。镜像、端口、时区和挂载路径直接编辑 `compose.yaml`。如需固定版本，把 `x-runtime.image` 改为 `ghcr.io/domoxiaojun/sumpter:0.4.20`，正式部署前确认该版本已经发布。

```bash
sudo docker compose config --quiet
```

这一步只验证 Compose 配置，尚未验证 JSON 内容、上游或实际请求。

## 5. 启动并打开管理页

```bash
sudo docker compose pull
sudo docker compose up -d --wait
sudo docker compose ps -a
curl --noproxy '*' -i http://127.0.0.1:57879/healthz
```

正常情况下，`init` 完成后退出，状态为 `Exited (0)`；`sumpter` 持续运行并显示 healthy。`/healthz` 返回 `204 No Content` 表示管理监听存活，尚不代表上游配置可用。

在部署机打开 `http://127.0.0.1:57879/admin/`，用 `kkl` 和准备的管理密码登录。

如果部署在远程服务器，在**自己的电脑**另开终端建立隧道，替换 SSH 用户和主机名：

```bash
ssh -N \
  -L 57879:127.0.0.1:57879 \
  -L 57878:127.0.0.1:57878 \
  your-user@your-server
```

保持这个终端运行，然后在自己的电脑打开同一管理地址。客户端也可通过隧道使用 `127.0.0.1:57878`。若本地端口已被占用，改 `-L` 左侧端口，并相应调整浏览器或客户端地址。

## 6. 完成第一条真实请求

1. 在「安全」修改默认管理用户名和密码。
2. 在「入口库」填写真实上游与模型，启用入口。
3. 在「模型组」启用对应模型和入口绑定。
4. 在客户端设置代理地址和第 2 步的入站 Token。
5. 发送一句简单问候，在「运行」确认最终成功及实际使用的入口。

详细客户端配置见 [使用手册](USAGE.md)。管理页能打开、容器 healthy、模型目录可获取，都不能单独证明一条模型请求已经成功。

## 网络与常用设置

| 需要调整的项目 | 修改位置 |
| --- | --- |
| 宿主机端口或绑定 IP | `compose.yaml` 的 `services.sumpter.ports` |
| 容器代理监听 | `config/config.json` 的 `listener`，默认保持 `0.0.0.0:57878` |
| 容器管理监听 | Compose 中 `SUMPTER_ADMIN_HOST=0.0.0.0`、`SUMPTER_ADMIN_PORT=57879` |
| 时区 | Compose 的 `TZ`，例如 `Asia/Shanghai`；镜像包含 tzdata |
| 日志级别 | Compose 的 `RUST_LOG`，默认 `info` |
| 数据目录 | `x-runtime.volumes` 的宿主机路径，两个服务共用这个挂载 |
| 镜像版本 | `x-runtime.image`，同时用于初始化和主服务 |

修改 Compose 后运行 `sudo docker compose up -d --wait` 使容器配置生效。仅修改 JSON 后，可在 WebUI 重载配置，或执行 `sudo docker compose restart sumpter`；WebUI 保存会通过服务校验并应用配置。

需要局域网或 VPN 访问时，可将端口映射的宿主机地址改成相应网卡 IP，并配置防火墙和入站 Token。公开管理页应通过 HTTPS 反向代理，通常保留宿主机管理端口为回环地址。仓库及 Linux 包提供 `deploy/nginx-sumpter-admin.conf.example`；只有两份模板的独立部署目录需另外下载该反代示例。

反代要保留 Cookie，设置 `X-Forwarded-Proto`，对事件流关闭 buffering。另行反代代理端口时，还需支持长响应及 WebSocket Upgrade；仓库提供 `deploy/nginx-sumpter-proxy.conf.example` 作为数据面反代示例。

### 事件中的客户端 IP

运行事件的「请求源 IP」按 `X-Real-IP`、`X-Forwarded-For`、TCP 对端的顺序取第一个合法值；没有转发头时就是 Sumpter 看到的 TCP 对端。默认 bridge 网络下，直接访问宿主机发布端口的客户端会经过 Docker 的 NAT：宿主机本机访问通常显示为网关地址（如 `172.18.0.1`），来自其它机器的连接在多数 Linux 发行版上仍保留原始地址。经反向代理但代理没有传递地址时，显示的是代理容器或代理进程的地址。

只要代理传递 `X-Real-IP` 或 `X-Forwarded-For`，事件就直接显示其中的客户端 IP，不需要预先登记代理地址；示例里的边界 Nginx 会用 `$remote_addr` 覆盖客户端自带的头。多层代理时把已知代理填进 `listener.trustedProxyCIDRs`（WebUI「安全」页的「可信代理 IP / CIDR」），`X-Forwarded-For` 链会从右向左跳过它们。该设置只改变事件记录，入站 Token 和 `allowedCIDRs` 仍按 TCP 对端判定。

转发头由请求方声明，Sumpter 不校验其来源：能直接访问 Sumpter 端口的客户端可以自行声明来源地址。若需要事件里的地址可信，让该端口只对代理开放（独立的 Compose 网络或防火墙），并像示例那样由边界代理覆盖客户端自带的头。

如果只有纯 NAT 而没有任何代理传递地址，Sumpter 记录的就是网关地址，不会伪造真实 IP。可选做法：一是保留源地址的网络路径，例如让流量走保留原地址的接口而不是宿主机回环，或改用 `network_mode: host`（需自行处理端口冲突，模板不默认切换）；二是在直接接收客户端的位置放一层可信反代传递地址。

在 SELinux 主机上，将共享挂载改为 `./config:/config:z`，为两个服务共享的数据目录设置容器标签。

引擎的数据转发和模型探测显式使用 `no_proxy()`。向容器添加 `HTTP_PROXY`、`HTTPS_PROXY`、`ALL_PROXY`、`NO_PROXY` 不会让这些请求经过系统 HTTP 代理；应保证容器网络能直接访问上游。

## 备份与升级

以下命令在实际部署目录执行。备份整个 `config/`，包含数据库及可能存在的 WAL，不要在服务运行时只复制 `runtime.sqlite3`：

```bash
sudo docker compose stop sumpter
sudo tar -czf "sumpter-backup-$(date +%Y%m%d-%H%M%S).tar.gz" compose.yaml config
sudo docker compose start sumpter
```

备份含密钥和运行数据，存放到你控制的位置。需要恢复时，先停服务，把备份解到新的目录检查，再切换挂载路径或目录；不要直接覆盖仍在使用的数据。

升级时先备份。使用固定版本的部署先修改 `x-runtime.image`，使用 `latest` 的部署可直接拉取：

```bash
sudo docker compose pull
sudo docker compose up -d --wait
sudo docker compose ps -a
```

再次登录，检查版本、原有配置、历史统计，并发送一条实际请求。回退旧镜像前核对配置和数据库兼容性；必要时配套恢复升级前备份。

`sudo docker compose down` 停止并移除容器与网络，绑定挂载的 `config/` 仍保留。不要为了重启或升级删除数据目录。

## 故障排查

先查看状态和最近日志：

```bash
sudo docker compose ps -a
sudo docker compose logs --tail 100 init sumpter
```

| 现象 | 检查 |
| --- | --- |
| `init` 为 `Exited (0)` | 正常，它是一次性初始化服务 |
| `init` 非零退出 | 配置与密码应是普通文件，目录所有权正确；仅有旧 `keys.json` 的目录需先整理 |
| 管理页可用，代理连不上 | `listener.host` 是否仍为示例的 `127.0.0.1`；容器代理端口与映射是否一致 |
| healthy，但客户端报模型错误 | 示例仍停用，或入口映射、模型组、绑定范围不一致 |
| 远程浏览器打不开 | 默认只发布到回环地址，先使用 SSH 隧道；浏览器地址不能写 `0.0.0.0` |
| `permission denied` | 标准 Docker 下 `config/` 应由 root 拥有、目录 0700；SELinux 检查 `:z` |
| 登录失败 | 代理 Token 不是管理密码；改密后的初始密码失效；重启后需要重新登录 |
| 新端口不生效 | 修改的是宿主机映射还是容器监听；Compose 改动需重新 `up` |
| 事件源 IP 都是 `172.x` 网关或代理地址 | 见「事件中的客户端 IP」：让代理传递 `X-Real-IP` 或 `X-Forwarded-For`，或改用保留源地址的网络路径 |

模板里的 `init` 只创建缺失文件，不覆盖已有配置或密码。按本教程准备好文件时，它不会生成新密码；若未准备密码而依赖它自动初始化，初始密码会输出到 init 日志，仅在可信终端查看日志。

忘记管理密码时，先停止 `sumpter`，备份 `config/admin-password`，将它改为新的非空单行密码并保持 root 所有、0600 权限，再启动服务。用户名会恢复为 `kkl`；登录后重新设置凭据。

## 维护者从源码构建

需要完整源码树和部署机的构建环境，Linux 二进制发布包不包含 Rust 源码。在源码树 `platforms/linux/` 准备同样的 `config/` 后执行：

```bash
sudo docker compose -f compose.yaml -f compose.build.example.yaml up -d --build --wait
```

构建覆盖文件使用独立的 `sumpter:local` 镜像名，避免覆盖本地官方镜像标签。镜像构建、容器启动与真实请求应分别验证。
