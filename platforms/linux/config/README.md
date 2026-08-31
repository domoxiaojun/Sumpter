# Docker 运行数据目录

由 `compose.yaml` / `docker-compose.yml` 挂载为容器内 `/config`。

```text
./config  →  /config
  admin-password          # Web Admin 登录凭据，**启动前必须自己创建**（见下）
  config.json             # 主配置（首次启动由 daemon 自动生成 bootstrap）
  runtime.sqlite3         # 当前运行统计（首次启动后创建）
  session_affinity.json   # 粘性会话归属 v2（自动维护，TTL 30 天）
  diagnostic_capture.json # 手动诊断捕获（默认停止，0600 原子写入）
  stats.json              # 旧版运行统计只读归档，新版本不读取或写入
  kekulvd.pid             # 容器内 pid 文件（daemon 自己写，退出时清理）
```

`admin-password` 不会自动生成：不创建它 daemon 会拒绝启动，也就登不进 WebUI。首登用户名
`kkl`，改密后该文件被原子迁移为 Argon2 哈希 JSON。

首次：

```bash
mkdir -p config
docker compose pull
docker compose up -d
```

Compose 默认以 `user: "0:0"` 运行，**不必** `chown 10001`。  
配置文件会由容器直接写在本目录。

可选：放入示例配置后再启动：

```bash
cp config.example.json config/config.json
chmod 600 config/config.json
```

**不要**把含密钥的 `config.json` 提交进 git。
