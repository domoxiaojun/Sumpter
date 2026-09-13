# Compose 数据目录

部署时先创建这个目录，再复制配置示例并准备管理密码。完整命令见 [Compose 教程](../DOCKER.md)。默认挂载关系为宿主机 `./config` → 容器 `/config`。

| 文件 | 用途 | 何时创建 |
| --- | --- | --- |
| `config.json` | 上游地址、密钥、模型组、监听与重试 | 首次部署从示例复制 |
| `admin-password` | 初始单行密码；改密后为 Argon2 哈希 JSON | 首次部署生成 |
| `runtime.sqlite3` | 运行统计数据库 | 服务自动创建 |
| `runtime.sqlite3-wal` / `runtime.sqlite3-shm` | SQLite 工作文件 | 运行时按需产生 |
| `session_affinity.json` | 会话粘性归属 | 按实际请求维护 |
| `resource_bindings.json` | Live / Video 等资源所属入口 | 按实际请求维护 |
| `diagnostic_capture.json` | 主动开启的原始诊断捕获 | 启用捕获后产生 |
| `config.before-schema-v7-*.json` | 兼容配置迁移前备份 | 迁移时产生 |

不需要预先创建空数据库、日志目录或其它运行文件。容器日志由 Docker 管理；浏览器登录会话和 `/tmp` 不持久化。

标准 Docker 下目录由 root 拥有，权限 0700，配置与密码 0600。容器代理监听应为 `0.0.0.0:57878`，宿主机端口在 Compose 中修改。SELinux 共享挂载使用 `:z`。

一个目录只能供一个运行实例使用。备份时停止服务，复制整个目录并保留权限；配置与备份都含敏感信息。本目录运行数据已被 Git 忽略。
