# 脚本目录

在仓库根目录执行命令。顶层只保留日常检查和本机打包入口；源码与平台目录的整体关系见 [项目结构](../docs/project-structure.md)。

| 位置 | 职责 |
| --- | --- |
| `check.sh` | 统一检查入口：`docs`、`rust`、`web`、`macos`、`all` |
| `build-macos-dmg.sh` | 本机 macOS DMG 构建入口，调用平台打包脚本 |
| `clients/` | 客户端归因程序的维护源，以及自动生成的 Gemini 兼容入口 |
| `maintenance/` | 文档、元数据、客户端副本同步，以及源码快照导出 |
| `tests/` | Node 测试，由检查入口和 CI 调用 |

## 常用命令

```bash
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
./scripts/check.sh macos
./scripts/build-macos-dmg.sh --help
```

`web` 检查会重建受版本控制的 WebUI 产物。DMG 脚本当前默认跳过测试；需要打包前测试时设置 `RUN_TESTS=1`。本机打包不代表签名公证、安装或发布完成。

## 维护工具

```bash
node scripts/maintenance/sync-project-metadata.mjs --write
uv run scripts/maintenance/sync-usage-docs.py --write
node scripts/maintenance/sync-client-attribution.mjs
node scripts/maintenance/export-source.mjs /absolute/path/to/new-source
node --test scripts/tests/*.test.mjs
```

三个同步工具都支持 `--check` 只读检查。元数据和文档同步默认检查；客户端同步默认写入副本。完整的维护源与目标对应关系见 [开发指南](../docs/development.md#单一维护源与生成副本)。

`clients/client-attribution.mjs` 是 Claude、Grok、Gemini、pi 的共用安装器；前三者还使用其中的启动包装逻辑。不要单独编辑 `clients/gemini-sumpter-wrapper.mjs`。`clients/pi-project-attribution.ts` 是 pi 扩展，修改后运行同一个客户端同步命令更新三个平台资源副本，检查入口和测试会检查一致性。

平台 Shell 安装器、`cc-project-attribution.sh` 和 `grok-project-attribution.sh` 保留在平台目录；它们不由上述客户端同步工具生成，修改时应核对 Linux/macOS 脚本与 Swift App 内置资源。

`platforms/linux/scripts/`、`platforms/macos/scripts/` 和 macOS App 内置资源属于安装包布局，保留原来的文件名和位置。用户文档中的远程安装命令指向 GitHub 仓库 raw 与 Release；listener 的 `/__sumpter/` 仍可作为已运行代理的备用下载。
