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
- **关于**：版本、许可证与检查更新。正式发布包可在此或菜单栏图标右键检查 Sparkle 更新；本机构建没有更新源。

## 客户端归因

客户端只需配置代理地址、入站 Token 和模型，无需安装归因脚本。Sumpter 优先使用请求中的原生身份、会话和工作区信息；证据不足时保留未知。需要人工补充产品身份时，可在 App「安全 → 客户端识别」中为独立 Token 设置兜底绑定，绑定不会生成项目或会话信息。

旧 Shell 包装器、Pi 扩展和安装管理界面已经退役；升级不会自动编辑客户端机器上的配置。曾安装旧采集器的用户应在客户端主机核对备份、移除对应启动注册或显式加载的扩展，再重启客户端。详细字段、可信中继与迁移边界见 [客户端身份与请求归因](../../docs/client-attribution.md)。

## 构建与更新

开发构建从仓库根执行 `./scripts/check.sh macos`；DMG 构建执行 `./scripts/build-macos-dmg.sh --clean`。Sparkle、appcast、签名和公证见 [更新发布](app/UPDATE.md)。源码测试、DMG 生成、Apple 签名、公证和实际安装是不同验证层次。
