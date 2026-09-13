# Sumpter macOS

macOS App 提供菜单栏状态、原生管理界面、sidecar 代理和通知。支持 macOS 14+；当前自动发布包面向 Apple Silicon。App 与 Linux 共用 schema v7、入口映射、模型组、重试和运行统计。

## 安装与启动

从 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest) 下载 DMG，打开后运行「安装 Sumpter.command」，或将 App 拖到“应用程序”并在 Finder 右键选择“打开”。当前发布包为 ad-hoc 签名，可能没有 Apple 公证；首次提示无法验证开发者时，只对确认来源的 App 允许打开，不要关闭全局 Gatekeeper。

启动 App 后，sidecar 自动使用 `~/Library/Application Support/Sumpter/` 保存 `config.json`、运行数据库和控制状态。进入入口库添加上游、在模型组开放模型，再按 [使用手册](../../USAGE.md) 接入客户端。代理默认 `http://127.0.0.1:57878`；管理控制由 App 内部端口处理，不使用 Linux 的固定 57879 页面。

## App 中的页面

- **运行**：服务状态、进行中请求、请求链和最终结果。
- **入口库**：Provider 地址、API Key、协议和模型映射。
- **模型组**：开放模型、入口绑定和优先级 / 随机 / 轮询策略。
- **Claude Code 路由**：WebSearch、WebFetch、分类器等独立子请求。
- **统计**：耗时、Token、缓存、项目、会话和入口维度。
- **安全**：入站访问、客户端归因和敏感操作。
- **诊断**：存储状态和按需捕获；捕获可能含原始凭据，导出前必须脱敏。

## 客户端归因

在 App 的「安全」页选择 Claude、Grok、Gemini、Codex 或 pi，点击安装配置。安装动作只修改所选客户端的 shell 启动配置；pi 的私有扩展保存在用户数据目录并由 wrapper 加载。安装后新开终端并重新启动客户端，`/reload` 不会加载 shell 配置。

归因必须装在运行客户端的 Mac，而不是只运行远程 Linux 代理的服务器。Codex Desktop 和已运行的图形会话不会加载 shell wrapper；原生 workspace metadata 仍可参与统计。

## 构建与更新

开发构建从仓库根执行 `./scripts/check.sh macos`；DMG 构建执行 `./scripts/build-macos-dmg.sh --clean`。Sparkle、appcast、签名和公证见 [更新发布](app/UPDATE.md)。源码测试、DMG 生成、Apple 签名、公证和实际安装是不同验证层次。
