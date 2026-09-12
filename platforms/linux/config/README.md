# Docker 运行数据目录

默认由 `compose.yaml` 挂载为 `/config`，完整步骤见 [DOCKER.md](../DOCKER.md)。也可用 `.env` 的 `SUMPTER_CONFIG_DIR` 指向其他相对目录。

容器不指定运行用户，按镜像默认用户（root）运行，因此不需要在部署机上配置 UID/GID；本目录及其中的文件由容器创建。首次启动时 `init` 服务只生成缺失的 `config.json` 与随机 `admin-password`，已有文件一律不覆盖，也不自动迁移仅剩 `keys.json` 的历史目录；直接 `docker run` 镜像则需自行准备密码文件。

```text
./config → /config
  admin-password          # 初始单行密码；改密后为 Argon2 哈希 JSON
  config.json             # 当前 schema 配置；Docker 初始代理监听 0.0.0.0:57878（端口由本文件决定，不要在 WebUI 改，否则 compose 端口映射会失效）
  runtime.sqlite3         # 持久化运行统计
  runtime.sqlite3-wal     # SQLite 工作文件，可能出现
  runtime.sqlite3-shm
  session_affinity.json   # 按实际使用维护的粘性会话归属
  resource_bindings.json  # 按实际使用维护的 Live/Video 资源绑定
  diagnostic_capture.json # 手工启用的诊断捕获快照
  config.before-schema-v7-*.json # schema 迁移时的原始备份
  sumpterd.pid            # 运行时创建，正常退出清理
```

文件权限为目录 `0700`、配置与密码 `0600`；若宿主机用户需要直接读取，请用 `sudo` 而不要用 `chmod 777`。

首次用户名 `kkl`，初始密码只在可信终端本地查看，不要复制到 Issue 或日志。停机后备份/迁移**整个目录**（含隐藏文件与可能存在的 SQLite WAL），不要只复制数据库主文件，也不要让多个实例共享同一目录。旧 `stats.json` 不由新版本读写。

程序日志走容器标准输出/错误，由 Docker 的 `json-file` 驱动管理，不会写入本目录；需要留档时按 DOCKER.md 导出。`/tmp`、登录会话与进行中的请求不属于持久化数据。

本目录除占位与说明文件外都被 Git 忽略。不要提交真实配置、密码、数据库或备份。
