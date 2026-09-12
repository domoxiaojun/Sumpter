# Docker：独立目录部署与迁移

目标是在任意位置创建一个 `sumpter/`，在其中运行 `docker compose`，不需要克隆源码、安装 Rust 或在部署主机编译。配置、数据库等持久化数据都留在该目录下，停机后复制整个目录即可迁移。

要求：Linux、Docker Engine 与 Docker Compose **2.24+**。默认 bridge 网络，镜像提供 amd64 / arm64 两种架构。程序与 WebUI 随镜像升级。

容器**不指定运行用户**，使用镜像默认用户（root）运行，因此不需要在部署机上推导或配置 UID/GID。数据目录由容器自行创建所需文件。

## 镜像来源

运行镜像只有 GHCR 一处来源，**不在部署主机构建**：

- 构建与推送由发布工作流完成：container job 先用 `linux/amd64,linux/arm64` 校验，再由 publish job 推送双架构 manifest，不存在单架构可用而另一架构缺失的情况。
- tag 形式：`0.4.6`、`0.4`、`0`、`latest`、`sha-<full>`；**不带 `v` 前缀**。`latest` 只在发布成功后推进，普通 main push 不会刷新镜像。
- 钉死版本：`.env` 里写 `SUMPTER_IMAGE=ghcr.io/domoxiaojun/sumpter:0.4.6`，或直接写 digest。
- 仓库或 GHCR 为私有时需先 `docker login ghcr.io`。
- `compose.yaml` 不含 `build:`；源码构建只存在于单独的 override，且使用本地 tag `sumpter:local`（见文末）。

## 首次部署

在新建的 `sumpter/` 中放入 `compose.yaml`。源码树里是 `platforms/linux/compose.yaml`；也可以直接下载：

```bash
mkdir -p sumpter/config && cd sumpter
curl --proto '=https' --tlsv1.2 -fLo compose.yaml \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/compose.yaml
# 可选：需要改端口/目录/日志时再拿参数样例
curl --proto '=https' --tlsv1.2 -fLo .env.example \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/.env.example

docker compose up -d          # 首次会拉取镜像
docker compose ps -a
docker compose logs -f sumpter
```

`init` 服务用同一镜像的 shell 在 `/config` 内生成缺失的 `config.json` 与随机 `admin-password`，成功后显示 `Exited (0)`，这是正常状态。它不会覆盖已有配置、密码或数据库，也不会读取或自动迁移仅剩 `keys.json` 的历史目录；初始化失败会阻止 daemon 启动，用 `docker compose logs init` 查看原因。

`.env` 是可选的：不创建时上面这些默认值就够用。需要自定义参数时再 `cp .env.example .env`。

Linux 二进制发布包也带 `compose.yaml`、`.env.example` 与本文。若直接在解压目录里 `docker compose up -d`，数据会落在包内的 `config/`，**升级或删除解压目录前先备份整个目录**；更稳妥的做法是把 `compose.yaml` 复制到一个独立的 `sumpter/` 目录再启动。

默认访问：

- 管理页：`http://127.0.0.1:57879/admin/`
- 代理：`http://127.0.0.1:57878`（OpenAI / Codex 客户端通常使用 `/v1`）

首次用户名 `kkl`。初始密码只在可信终端**本地查看** `config/admin-password`；登录后可在安全页改用户名与密码，改密后该文件变成 Argon2 哈希 JSON，不再有可读明文。不要把密码写进日志、Issue 或聊天记录。

初始配置没有上游入口，需要在 WebUI 中配置。代理在容器内监听 `0.0.0.0:57878`，但宿主机默认只发布到 `127.0.0.1`。这只影响容器部署，不改变原生 Linux/macOS 安装的安全默认配置。

## 精简部署：自备 `config.json`，不用 `init`

`init` 服务只做两件事：生成 `admin-password`，以及写入 `listener.host=0.0.0.0` 的 `config.json`。
如果你在部署前就用 `curl` 把正式配置准备好，可以省掉这个服务：`compose.yaml` 只剩一个服务，
也不需要 `.env`。代价是下面第 2、3 步必须你自己做完，做漏了不会自愈。

**第 1 步：拿配置模板**。下载的是仓库里的安全模板：入口全部停用、域名是 `.invalid`、secret 是合成的。
配置目录已挂载到宿主机 `./config`（`volumes: ./config:/config`），所以配置只要落在宿主机这个目录里，
不需要 `docker cp`；已有填好的 `config.json` 时直接拷进去即可：

```bash
mkdir -p sumpter/config && cd sumpter
cp 你的/config.json config/config.json          # 已有配置：直接拷到挂载目录
# 或者拿模板：
curl --proto '=https' --tlsv1.2 -fLo config/config.json \
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/config.example.json
```

**第 2 步：改两处**。把 `config/config.json` 的 `listener.host` 从 `127.0.0.1` 改成 `0.0.0.0`，
否则容器内代理只监听回环，宿主机的 `57878` 映射会指向没人监听的端口；再启用一个真实入口
（填 `baseURL`、`apiKey`，把 `enabled` 改成 `true`，并在 `mappings` 里写下客户端实际会发的模型名）。
改完先验证 JSON 语法：

```bash
python3 -m json.tool config/config.json > /dev/null && echo OK
```

**第 3 步：生成 Admin 凭据**。这一步不能省：`admin-password` 缺失时 daemon 会在启动前退出，
日志为 `读取 Admin 凭据文件 /config/admin-password 失败`。daemon 不会自己造这个文件。

```bash
od -An -N32 -tx1 /dev/urandom | tr -d ' \n' > config/admin-password
printf '\n' >> config/admin-password
chmod 700 config
chmod 600 config/config.json config/admin-password
```

**第 4 步：写 `compose.yaml` 并启动**。容器内监听、只读加固、日志轮转与官方模板一致；
存活探针来自镜像的 `HEALTHCHECK`，这里与官方模板一样不重复声明。

```yaml
services:
  sumpter:
    image: ghcr.io/domoxiaojun/sumpter:latest   # 建议钉版本，例如 :0.4.6
    restart: unless-stopped
    init: true
    read_only: true
    cap_drop:
      - ALL
    security_opt:
      - no-new-privileges:true
    stop_grace_period: 30s
    volumes:
      - ./config:/config
    environment:
      TZ: Asia/Shanghai
      RUST_LOG: info
      SUMPTER_ADMIN_HOST: "0.0.0.0"
      SUMPTER_ADMIN_PORT: "57879"
      SUMPTER_ADMIN_PASSWORD_FILE: /config/admin-password
    ports:
      - "0.0.0.0:57878:57878"
      - "127.0.0.1:57879:57879"
    tmpfs:
      - "/tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777"
    logging:
      driver: json-file
      options:
        max-size: 10m
        max-file: "3"
```

```bash
docker compose up -d && docker compose ps
sudo cat config/admin-password      # 初始密码，只在可信终端本地查看
```

登录方式与默认部署相同：管理页 `http://127.0.0.1:57879/admin/`（远程用 SSH 隧道），代理是
`http://<宿主机地址>:57878`。

与官方模板的差别只有初始化这一层：

| 情形 | 官方模板 | 精简版 |
| --- | --- | --- |
| 已有 `config.json` | 不动 | 不动 |
| 缺 `config.json` | `init` 写入 `0.0.0.0` bootstrap | daemon 自建**回环**配置，宿主机映射打空 |
| 缺 `admin-password` | `init` 生成随机密码 | daemon 拒启 |
| 只有旧 `keys.json` | `init` 拒绝并阻止 daemon 启动 | daemon 自己报错 |

也就是说，精简版把 `init` 的护栏换成了你的手工步骤；备份、升级与迁移仍照本文其余章节执行，
升级镜像时不需要改这份 `compose.yaml`。

## 目录与持久化边界

```text
sumpter/
├── compose.yaml
├── .env                      # 可选：本机部署参数，不入库
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

不要把配置、密码、数据库拆成多个单文件挂载：凭据更新、配置原子替换、SQLite WAL 都需要在同一目录内创建和替换文件。备份/迁移时复制整个 `config/`，不要只复制 `runtime.sqlite3` 而漏掉可能存在的 WAL。旧 `stats.json` 只作为历史文件保留，新版本不读写。

镜像内的 `/usr/local/bin/sumpterd` 与 `/opt/sumpter/web/` 无需备份，`/tmp` 是临时内存文件系统。程序日志走标准错误/输出，由 Docker 的 `json-file` 驱动管理，**不在 `config/` 内，删除容器后无法靠复制 `config/` 恢复**。需要留档时在停机或删除容器前导出：

```bash
mkdir -p logs
docker compose logs --no-color > "logs/compose-$(date +%Y%m%d-%H%M%S).log"
```

## `.env` 参数

`.env` 会被 Compose 用来展开部署参数，同时通过 `env_file` 原样透传给 daemon。不会执行其中的 Shell 代码；不要 `source .env`。

| 参数 | 默认 / 作用 |
| --- | --- |
| `COMPOSE_PROJECT_NAME` | `sumpter`；同一主机运行多个实例时必须使用不同项目名、目录与端口 |
| `SUMPTER_IMAGE` | `ghcr.io/domoxiaojun/sumpter:latest`；官方 GHCR 双架构镜像，可钉版本或 digest |
| `SUMPTER_CONFIG_DIR` | `./config`；相对于 Compose 项目目录（即 `compose.yaml` 所在目录） |
| `SUMPTER_PROXY_BIND_HOST` / `SUMPTER_ADMIN_BIND_HOST` | `127.0.0.1`；宿主机发布地址 |
| `SUMPTER_PROXY_PORT` / `SUMPTER_ADMIN_PORT` | `57878` / `57879`；宿主机端口 |
| `RUST_LOG` | `info`；日志等级或过滤器（daemon 读取） |
| `TZ` | `UTC`；容器时区，镜像已装 tzdata（v0.4.5 及更早的发布镜像没有）。程序自身的统计与时间戳仍按 Unix 时间（UTC）存储，日志时间戳也为 UTC，WebUI 按浏览器时区展示 |
| `SUMPTER_RESTART_POLICY` | `unless-stopped`；只作用于 daemon，init 不会循环重启 |
| `SUMPTER_STOP_GRACE_PERIOD` | `30s`；迁移前等待正常关停 |
| `SUMPTER_TMPFS_SIZE` | `64m`；容器 `/tmp` 容量，不持久化 |
| `SUMPTER_LOG_MAX_SIZE` / `SUMPTER_LOG_MAX_FILES` | `10m` / `3`；Docker 日志轮转 |
| `RUST_BACKTRACE` | 不设；设为 `1` 时 panic 输出会附上回溯，便于提交问题时排查 |

改成宿主机 18080 / 18081：

```dotenv
SUMPTER_PROXY_PORT=18080
SUMPTER_ADMIN_PORT=18081
```

容器内端口仍固定为 **57878 / 57879**，健康检查与端口映射都按容器内端口工作，因此改宿主机端口不会造成错位。Compose 显式设置的 `SUMPTER_ADMIN_HOST=0.0.0.0`、`SUMPTER_ADMIN_PORT=57879`、`SUMPTER_ADMIN_PASSWORD_FILE=/config/admin-password` 优先于 `env_file` 中的同名值。

其他变量会被透传，但只能是程序或其网络库确实支持的变量；**`config.json` 中的入口、模型组、Token/CIDR 仍通过 WebUI 或配置文件管理，不会因为 `.env` 里出现同名字段就被覆盖**。修改 `.env` 后要 `docker compose up -d` 才会重建容器；`restart` 不会重新加载容器环境。宿主机 shell 里已 `export` 的同名变量优先级高于 `.env`，排错时先确认这一点。不要公开 `docker compose config` 的完整输出，它可能包含用户填写的凭据。

daemon 实际读取的环境变量只有这些：`RUST_LOG`、`RUST_BACKTRACE`、`SUMPTER_ADMIN_HOST`、`SUMPTER_ADMIN_PORT`、`SUMPTER_ADMIN_PASSWORD_FILE`、`SUMPTER_WEB_ROOT`（外加默认配置目录推导用的 `HOME` / `XDG_CONFIG_HOME`）。代理的监听地址与端口属于 `config.json`，**没有**对应的环境变量。

### 不要设置的变量

| 变量 | 为什么无效 |
| --- | --- |
| `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`（含小写） | **上游转发不走环境代理**：数据面与模型目录探测都显式 `.no_proxy()`。设了只会在目录探测失败时被识别出并提示“不走代理”，不会让请求真的能出去；既然没有代理可用，`NO_PROXY` 也没有可豁免的对象。确需代理出网时请在容器外做网络层转发，或让 `baseURL` 直接指向容器内可达的地址。 |
| `SUMPTER_WEB_ROOT` | CMD 的 `--web-root /opt/sumpter/web` 优先，环境变量被忽略；WebUI 已内置在镜像里。 |
| `SUMPTER_ADMIN_HOST` / `SUMPTER_ADMIN_PORT` | 模板已固定为容器内 `0.0.0.0` / `57879`，健康检查与端口映射都依赖这两个值；宿主机地址与端口用 `*_BIND_HOST` / `*_PORT`。 |
| `SUMPTER_ADMIN_PASSWORD_FILE` | 固定为 `/config/admin-password`，改路径会让登录凭据与数据目录脱节。 |
| `HOME` / `XDG_CONFIG_HOME` | CMD 已用 `--config-dir /config` 固定配置目录，不再参与推导。 |

### 宿主机侧 vs 容器内

模板里出现 proxy 与 admin 两组变量，它们**不是重复配置**：daemon 有两个独立监听项。

| 监听项 | 作用 | 容器内地址 / 端口 | 宿主机侧（`.env`） |
| --- | --- | --- | --- |
| 代理（数据面） | 客户端 API 流量，OpenAI / Anthropic / Codex 等都指向这里 | `config.json` 的 `listener.host` / `listener.port`，init 写为 `0.0.0.0:57878` | `SUMPTER_PROXY_BIND_HOST` / `SUMPTER_PROXY_PORT` |
| Admin（控制面） | WebUI 与 `/admin/api`，登录会话 | Compose 的 `environment` 固定为 `0.0.0.0:57879` | `SUMPTER_ADMIN_BIND_HOST` / `SUMPTER_ADMIN_PORT` |

宿主机侧变量只决定“发布到宿主机的哪个地址与端口”，改它们不会改变容器内监听；映射里的容器侧端口写死为 `57878` / `57879`，所以宿主机换到 18080/18081 也不会让健康检查或映射错位。

两个需要留意的耦合：

- **代理的容器内端口来自 `config.json`**，不是环境变量。不要在 WebUI 的“监听配置”里改代理端口，否则端口映射会指向无人监听的端口；确需修改时同时改 `config.json` 与 `compose.yaml` 的容器侧端口。宿主机换端口只用 `SUMPTER_PROXY_PORT`。
- `SUMPTER_ADMIN_PORT` 在 `.env` 里表示**宿主机端口**，而 Compose 的 `environment` 又把容器内的同名变量钉成 `57879`（显式 `environment` 优先于 `env_file`）。这是有意保留的：从旧 bridge 示例来的 `.env` 值继续表示宿主机端口，同时容器内的 Admin 契约不会被误改。

### 远程访问与主机加固

- 默认管理与代理端口都只在本机可见。远程管理优先用 SSH 隧道或 HTTPS 反向代理。确需直接发布到局域网时改 `*_BIND_HOST`；公网 Admin 必须置于 HTTPS 之后，代理对外开放前先设置入站 Token/CIDR。`0.0.0.0` 表示监听所有宿主机接口，本身不是认证。
- 容器以 root 运行，仅用于让 bind mount 的读写不依赖宿主机 UID 映射；Compose 已同时设置 `read_only: true`、`cap_drop: ALL`、`no-new-privileges`，可写路径只有 `config/` 与 `/tmp`。`config/` 中的文件由容器创建，权限为 `0700` / `0600`。
- 因此 `config/` 与其中的数据库通常由 root 所有。非 root 用户需要读取或打包备份时，用 `sudo tar`/`sudo cp -a` 或先 `docker compose stop` 后再由管理员操作，不要用 `chmod 777` 放宽权限。
- SELinux 主机（Fedora/RHEL）如果挂载报错，在 `compose.yaml` 的 volumes 行末追加 `:z`；init 与 daemon 共用同一目录，不要使用私有 `Z` 标签。
- 容器内没有 systemd：WebUI 安全页的“systemd 自启动”会显示不可用（`systemctl` 不存在），属预期。容器级别的开机自启请用 `.env` 的 `SUMPTER_RESTART_POLICY` 控制。

## 升级与迁移

升级只替换镜像，保留 `.env` 与整个 `config/`：

```bash
docker compose pull
docker compose up -d
```

跨机器迁移：

1. 在旧机 `docker compose stop` 等待正常退出；需要日志则先按上文导出，再 `docker compose down`。不要在 daemon 仍在写库时只拷贝单个 SQLite 文件。
2. 复制整个 `sumpter/`（包括隐藏的 `.env` 与完整 `config/`）。例如在父目录运行 `sudo tar -czf sumpter-backup.tar.gz sumpter/`。备份包含凭据，应限制读取权限并安全传输。
3. 在新机解压，检查端口占用、`SUMPTER_CONFIG_DIR` 相对路径与镜像架构；旧机若钉了单架构 digest，换架构时改用同版本的多架构引用。
4. 在新目录执行 `docker compose pull && docker compose up -d`，确认 `init` 成功、能登录 Admin、代理请求正常。初始化不会覆盖迁入的文件。旧的备份在验证通过前不要删除，也不要同时运行两个实例访问同一目录。

`config/` 备份涵盖凭据、SQLite 与各类绑定文件；浏览器登录会话、进行中的请求、临时 Live 连接等内存状态不承诺跨进程迁移。

### 从旧 host 网络 Compose 切换

旧模板使用 `network_mode: host` 且由 `SUMPTER_ADMIN_HOST/PORT` 直接决定监听地址。不要直接覆盖模板后假设行为不变：

1. 备份旧 `compose.yaml`、`.env` 与配置。把 `config.json` 的 `listener.host` 改为 `0.0.0.0`、`listener.port` 保持 `57878`（bridge 下容器内必须监听非回环地址）。旧容器仍是 host 网络，改动前请先设置入站认证或停机离线修改，避免短暂的对外暴露。
2. 停旧服务，换用新模板，按首次部署流程启动。
3. `.env` 中 `SUMPTER_ADMIN_PORT` / `SUMPTER_PROXY_PORT` 现在只控制宿主机映射，宿主机绑定地址改用 `*_BIND_HOST`；旧的 `SUMPTER_ADMIN_HOST` 不再决定宿主机暴露范围（它会被 Compose 覆盖为容器内的 `0.0.0.0`）。
4. 若 `config.json` 仍是 `127.0.0.1`，容器内代理无法从宿主机访问；启动后要同时验证两个端口与管理页登录。

`compose.bridge.example.yaml` 与 `docker-compose.yml` 在**源码树**中是指向 `compose.yaml` 的兼容链接（旧命令仍可用）；发布包与独立部署目录只保留一份 `compose.yaml`。

## 维护者：源码构建

只有完整源码树支持本机源码构建；Linux 二进制发布包不包含 Rust workspace，也不打包这个 override：

```bash
cd platforms/linux
SUMPTER_LOCAL_IMAGE=sumpter:local docker compose -f compose.yaml -f compose.build.example.yaml up -d --build
```

两个服务共用同一个构建镜像。`Dockerfile` 以仓库根为构建上下文；本机没有双架构构建器时，源码构建出来的是**当前架构**的镜像，不能当作双架构验证。

本机构建使用独立的 `SUMPTER_LOCAL_IMAGE`（默认 `sumpter:local`），**不读 `.env` 里的 `SUMPTER_IMAGE`**——否则源码构建结果会被打进 `ghcr.io/...` 正式 tag，覆盖本地缓存的官方镜像。构建成功不代表权限、登录、真实流量或迁移已经验收，容器验证在 Linux CI 执行。

发布用的 `Dockerfile.runtime` 同样以 `platforms/linux/` 为上下文，但要求目录内已有 CI 生成的 `docker-bin/sumpterd-amd64`、`docker-bin/sumpterd-arm64`，**不适合本机直接使用**；本机验证请用上面的源码构建路径。
