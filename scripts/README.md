# 脚本目录

脚本按“检查、同步、客户端、平台安装、构建”分组。默认不安装依赖、不启动 Docker、不修改真实配置。

## 常用检查

```bash
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
./scripts/check.sh docker
./scripts/check.sh macos
```

Python 脚本统一用 `uv run`：

```bash
uv run scripts/maintenance/sync-usage-docs.py --check
uv run scripts/maintenance/sync-usage-docs.py --write
```

`sync-project-metadata.mjs` 同步配置、许可证和更新记录；`sync-client-attribution.mjs` 检查客户端归因资源副本。WebUI 改动后必须构建并审阅 `platforms/linux/web/`。

## 目录说明

- `clients/`：归因维护源和 Gemini wrapper。
- `maintenance/`：同步、源码导出和仓库维护工具。
- `tests/`：Node 与 Compose / Docker 契约测试；`tests/source-ip-container-selftest.sh` 在 Linux CI 用真实容器验证事件源 IP 的直连、反代与 NAT 路径，本机不运行。
- `build-macos-dmg.sh`：调用 macOS App 打包脚本。

Linux 安装、卸载、systemd 和迁移脚本随 `platforms/linux/scripts/` 发布；使用步骤见 [Linux 指南](../platforms/linux/README.md)。Compose 只在 Linux CI 做真实镜像验证，本机 `check.sh docker` 只解析输入和运行契约测试。
