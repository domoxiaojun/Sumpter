# Docker：独立目录部署与迁移

目标是在任意位置创建一个 `sumpter/`，在其中运行 `docker compose`，不需要克隆源码、安装 Rust 或在部署主机编译。配置与数据库等持久化数据都留在该目录下，停机后复制整个目录即可迁移。

要求：Linux、Docker Engine 与 Docker Compose **2.24+**。默认 bridge 网络，镜像提供 amd64 / arm64 两种架构。容器**不指定运行用户**（用镜像默认的 root），所以不需要在部署机上推导 UID/GID，数据目录里的文件由容器自行创建。

## 镜像来源

运行镜像只有 GHCR 一处来源，**不在部署主机构建**。发布工作流推送 `linux/amd64,linux/arm64` 双架构 manifest，不会出现只有一种架构可用的情况。

- tag 形式：`0.4.8`、`0.4`、`0`、`latest`、`sha-<full>`；**不带 `v` 前缀**。`latest` 只在发布成功后推进，普通 main push 不会刷新镜像。
- 钉死版本：直接编辑 `compose.yaml` 的 `image` 行钉住版本，例如 `ghcr.io/domoxiaojun/sumpter:0.4.8`，或写 digest。
- 仓库或 GHCR 为私有时先 `docker login ghcr.io`。
- `compose.yaml` 不含 `build:`；源码构建见文末「维护者：源码构建」。

## 首次部署

在新目录里放一份 `compose.yaml` 就能启动，不需要克隆源码：

```bash
mkdir -p sumpter/config && cd sumpter
curl --proto '=https' --tlsv1.2 -fLo compose.yaml \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/compose.yaml

docker compose up -d          # 首次会拉取镜像
docker compose ps -a
docker compose logs init
```

`init` 服务用同一镜像的 shell 在 `/config` 内生成缺失的 `config.json`（监听 `0.0.0.0:57878`、入口为空）与随机 `admin-password`，成功后显示 `Exited (0)`，这是正常状态。它不会覆盖已有配置、密码或数据库，也不会迁移只剩 `keys.json` 的历史目录；初始化失败会阻止 daemon 启动，原因看 `docker compose logs init`。

**初始密码**：首次生成时，`docker compose logs init`（前台运行就是 `docker compose up` 的输出）会直接打印初始密码与凭据路径。明文只打印一次，重启不重复；登录后请立即在 WebUI「安全」页修改，改密后该文件变成 Argon2 哈希 JSON，日志里的初始密码随即失效。不想让明文进容器日志（例如日志会被采集或长期保留），就按下文「自备 `config.json` 与初始密码」先自己生成。原生 Linux/macOS 安装只显示密码文件路径、不回显密码值，也是这个原因。

启动后的地址：

- 管理页：`http://127.0.0.1:57879/admin/`，首次用户名 `kkl`
- 代理：`http://127.0.0.1:57878`（OpenAI / Codex 客户端通常使用 `/v1`）

两个端口默认只发布到 `127.0.0.1`。要给局域网客户端或远程管理使用就改宿主机发布地址，但 `0.0.0.0` 只是监听所有接口，本身不是认证：代理对外开放前先在 WebUI 设置入站 Token/CIDR，Admin 走 SSH 隧道或 HTTPS 反向代理。

登录后先添加 Provider 入口，填写 Base URL、API Key 和模型映射，再把客户端连接到代理地址。

镜像、端口、目录与日志都直接在 `compose.yaml` 中修改，见「常用配置」。

Linux 二进制发布包也带 `compose.yaml` 与本文。若直接在解压目录里 `docker compose up -d`，数据会落在包内的 `config/`，**升级或删除解压目录前先备份整个目录**；更稳妥的做法是复制到一个独立的 `sumpter/` 目录再启动。

## 目录与持久化边界

```text
sumpter/
├── compose.yaml
├── config/                   # 整目录 → 容器 /config
│   ├── config.json
│   ├── admin-password
│   ├── runtime.sqlite3
│   ├── runtime.sqlite3-wal   # SQLite 工作文件，可能出现
│   ├── runtime.sqlite3-shm
│   ├── session_affinity.json # 按实际使用创建
│   ├── resource_bindings.json
│   ├── diagnostic_capture.json
│   ├── config.before-schema-v7-*.json # 仅 schema 迁移时创建
│   └── sumpterd.pid          # 运行时创建，正常退出时清理
└── logs/                     # 可选：手工导出的日志
```

复制整个 `config/`，不要只复制 `runtime.sqlite3`：凭据更新、配置原子替换、SQLite WAL 都需要在同一目录内创建和替换文件，拆成多个单文件挂载或漏掉 WAL 都会出错。旧 `stats.json` 只作为历史文件保留，新版本不读写。

镜像与 `/tmp` 无需备份。程序日志走标准错误/输出，由 Docker 的 `json-file` 驱动管理，**不在 `config/` 内，删除容器后无法靠复制 `config/` 恢复**；需要留档就在停机或删除容器前导出：

```bash
mkdir -p logs
docker compose logs --no-color > "logs/compose-$(date +%Y%m%d-%H%M%S).log"
```

## 常用配置

默认配置已经写入 `compose.yaml`，复制文件后即可启动。需要修改时直接编辑对应字段，再执行 `docker compose up -d`：

| 需求 | 修改位置 | 默认值 |
| --- | --- | --- |
| 镜像版本 | `x-runtime.image` | `ghcr.io/domoxiaojun/sumpter:latest` |
| 宿主机代理端口 | `services.sumpter.ports` 第一行 | `127.0.0.1:57878` |
| 宿主机 Admin 端口 | `services.sumpter.ports` 第二行 | `127.0.0.1:57879` |
| 数据目录 | `x-runtime.volumes` | `./config:/config` |
| 日志等级 | `services.sumpter.environment.RUST_LOG` | `info` |

例如把 Admin 改为宿主机 `18081`：

```yaml
- "127.0.0.1:18081:57879"
```

容器内端口固定为 `57878` 和 `57879`，不要修改映射右侧端口。修改后运行 `docker compose up -d`，`restart` 不会应用配置变化。

其他参数也在 `services.sumpter` 下：`environment.TZ` 为容器时区（默认 UTC，WebUI 按浏览器时区显示），`restart` 为重启策略，`logging.options` 为日志大小和保留份数。查看服务日志用 `docker compose logs --tail=100 sumpter`。

旧部署的自定义镜像、端口、目录和日志值需要先从 `.env` 手工转写到 Compose 对应字段，再启用新模板；尤其要保留原来的数据目录，避免误用一个空目录。Compose CLI 自身仍可能读取 `.env` 中的项目名等内置选项，转写后将旧文件移出部署目录；多实例使用不同目录和端口，必要时在文件顶层设置 `name` 保持原项目名。

上游入口、模型和入站认证仍在 WebUI 管理。不要用 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY` 配置上游出网：引擎显式禁用了环境代理，请使用容器内可达的上游地址。

### 宿主机侧 vs 容器内

代理服务和管理页面使用两个端口：

| 监听项 | 作用 | 容器内地址 / 端口 | 宿主机侧（Compose `ports`） |
| --- | --- | --- | --- |
| 代理（数据面） | 客户端 API 流量，OpenAI / Anthropic / Codex 等都指向这里 | `config.json` 的 `listener.host` / `listener.port`，init 写为 `0.0.0.0:57878` | `127.0.0.1:57878:57878` |
| Admin（控制面） | WebUI 与 `/admin/api`，登录会话 | Compose 的 `environment` 固定为 `0.0.0.0:57879` | `127.0.0.1:57879:57879` |

- **代理的容器内端口来自 `config.json`**，不是环境变量。不要在 WebUI 的“监听配置”里改代理端口，否则端口映射会指向无人监听的端口；确需修改时同时改 `config.json` 与 `compose.yaml` 的容器侧端口。

### 远程访问与主机加固

- 远程管理优先用 SSH 隧道或 HTTPS 反向代理；确需直接发布到局域网时修改 `ports` 中最左边的绑定地址。公网 Admin 必须置于 HTTPS 之后，代理对外开放前先设置入站 Token/CIDR。
- 容器以 root 运行只是为了让 bind mount 的读写不依赖宿主机 UID 映射；Compose 已设置 `read_only: true`、`cap_drop: ALL`、`no-new-privileges`，可写路径只有 `config/` 与 `/tmp`，`config/` 内文件为 `0700` / `0600`。
- 因此 `config/` 通常由 root 所有：非 root 用户需要读取或打包备份时用 `sudo tar` / `sudo cp -a`。不要用 `chmod 777` 放宽权限。
- SELinux 主机（Fedora/RHEL）挂载报错时，在 `compose.yaml` 的 volumes 行末追加 `:z`；不要用私有 `Z`，init 与 daemon 共用同一目录。
- 容器内没有 systemd，WebUI 安全页的“systemd 自启动”显示不可用属预期；容器级别的开机自启用 Compose 中的 `restart` 字段控制。

## 升级与迁移

升级只替换镜像，`compose.yaml` 与整个 `config/` 都保留：

```bash
docker compose pull
docker compose up -d
```

跨机器迁移：

1. 旧机先 `docker compose stop` 等正常退出，需要日志就按上文导出，再 `docker compose down`。不要在 daemon 仍在写库时只拷贝单个 SQLite 文件。
2. 复制整个 `sumpter/`（包括 `compose.yaml` 与完整 `config/`），例如在父目录运行 `sudo tar -czf sumpter-backup.tar.gz sumpter/`。备份包含凭据，应限制读取权限并安全传输。
3. 新机解压后检查端口占用、Compose 中的 `./config` 路径与镜像架构；旧机若钉了单架构 digest，换架构时改用同版本的多架构引用。
4. 在新目录执行 `docker compose pull && docker compose up -d`，确认能登录 Admin、代理请求正常。初始化不会覆盖迁入的文件；验证通过前不要删除旧备份，也不要同时运行两个实例访问同一目录。

`config/` 备份涵盖凭据、SQLite 与各类绑定文件；浏览器登录会话、进行中的请求、临时 Live 连接属于内存状态，不承诺跨进程迁移。

### 从旧 host 网络 Compose 切换

旧模板使用 `network_mode: host`，且由 `SUMPTER_ADMIN_HOST/PORT` 直接决定监听地址。按顺序切换：

1. 备份旧 `compose.yaml` 与配置，把 `config.json` 的 `listener.host` 改为 `0.0.0.0`、`listener.port` 保持 `57878`（bridge 下容器内不能监听回环）。旧容器仍是 host 网络，改动前先设置入站认证或停机离线修改，避免短暂的对外暴露。
2. 停旧服务，换用新模板，按「首次部署」启动。
3. 当前模板直接固定宿主机绑定到 127.0.0.1 和端口 57878/57879；需要对外提供服务时直接编辑 `ports` 行，并先配置认证。
4. 启动后同时验证两个端口与管理页登录：`config.json` 若仍是 `127.0.0.1`，容器内代理从宿主机访问不到。

`compose.bridge.example.yaml` 与 `docker-compose.yml` 在**源码树**中是指向 `compose.yaml` 的兼容链接（旧命令仍可用）；发布包与独立部署目录只保留一份 `compose.yaml`。

## 自备 `config.json` 与初始密码

如果你希望自己控制初始密码（例如不想让明文出现在容器日志里），或者部署前就有现成配置，只要把两个文件先放进 `./config`，再执行跟「首次部署」一模一样的 `docker compose up -d`：`init` 只创建缺失文件，发现两者都在就静默跳过，不覆盖、也不打印密码。

```bash
mkdir -p sumpter/config && cd sumpter

# 1) 配置：已有就拷进去，否则先拿模板再改（模板入口全停用、域名是 .invalid）
cp 你的/config.json config/config.json
# 或者：curl --proto '=https' --tlsv1.2 -fLo config/config.json \
#   https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/config.example.json

# 2) 初始密码：自己生成，daemon 缺这个文件会拒绝启动
od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > config/admin-password
printf '\n' >> config/admin-password
chmod 700 config
chmod 600 config/config.json config/admin-password

docker compose up -d
```

两条容易踩的约束：

- **`listener.host` 必须是 `0.0.0.0`**。写成 `127.0.0.1`（模板默认值）时容器内代理只监听回环，宿主机的 `57878` 映射指向没人监听的端口，而且 daemon 不报错、健康检查照常通过，只是局域网连不上。
- **`admin-password` 必须存在且非空**。缺失时 daemon 在启动前退出，日志为 `读取 Admin 凭据文件 /config/admin-password 失败`；它不会自己生成这个文件。

连那一次多余的 `init` 容器都不想要，就把 `compose.yaml` 里的 `init` 服务与 `sumpter` 的 `depends_on` 删掉，其余不用改；代价是上面两条约束失去兜底，漏了就直接起不来或映射打空。

## 维护者：源码构建

只有完整源码树支持本机源码构建；Linux 二进制发布包不包含 Rust workspace，也不打包这个 override：

```bash
cd platforms/linux
SUMPTER_LOCAL_IMAGE=sumpter:local docker compose -f compose.yaml -f compose.build.example.yaml up -d --build
```

两个服务共用同一个构建镜像。`Dockerfile` 以仓库根为构建上下文；本机没有双架构构建器时，构建结果是**当前架构**的镜像，不能当作双架构验证。

本机构建使用独立的 `SUMPTER_LOCAL_IMAGE`（默认 `sumpter:local`），不会覆盖 Compose 中的官方镜像配置。构建成功不代表权限、登录、真实流量或迁移已经验收，容器验证在 Linux CI 执行。

发布用的 `Dockerfile.runtime` 同样以 `platforms/linux/` 为上下文，但要求目录内已有 CI 生成的 `docker-bin/sumpterd-amd64`、`docker-bin/sumpterd-arm64`，**不适合本机直接使用**；本机验证请用上面的源码构建路径。
