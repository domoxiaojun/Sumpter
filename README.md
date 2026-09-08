# Sumpter

本地优先的 AI 请求代理，支持 **Linux 服务端与 macOS 桌面端**。客户端连接一个地址，Sumpter 负责入口选择、模型映射、会话粘性、重试与运行记录。

采用一份 Rust 共享引擎和独立平台适配器。当前版本以 [Cargo.toml](Cargo.toml) 为准（0.3.4），配置格式为 schema v7，使用 [MIT 许可证](LICENSE)。

## 平台支持

| 平台 | 产品形态 | 交付目标 | 管理入口 |
| --- | --- | --- | --- |
| Linux | Rust daemon + React WebUI | x86_64 / aarch64 静态 musl 包；多架构容器 | 默认 `http://127.0.0.1:57879/admin/` |
| macOS 14+ | SwiftUI 菜单栏 App + Rust sidecar | 当前自动发布为 Apple Silicon；脚本另支持 x86_64 构建 | App 设置窗口 |

Windows 当前没有产品适配与发布支持。Linux 安装可接入 systemd；macOS 由 App 管理 sidecar 生命周期。

## 开始使用

1. 按 [使用指南](USAGE.md) 安装并启动对应平台产品。
2. 在入口库添加 Provider 地址与密钥，配置模型映射并绑定模型组。
3. 将客户端连接到默认代理地址 `http://127.0.0.1:57878`；Codex 使用带 `/v1` 的 Base URL。
4. 发一条请求，在运行页核对上游状态和统计。客户端模型必须被启用的映射或模型组接住。

入站鉴权启用时，客户端 API key 应填写 `listener.authToken`。完整字段和安全模板见 [配置说明](docs/configuration.md) 与 [config.example.json](config.example.json)。真实配置放在仓库外。

支持 Claude Code、Codex、Grok Build、Gemini CLI 及 OpenAI 兼容客户端。通常保留原始 HTTP 路径、查询、正文和 WebSocket 帧，只进行鉴权、调度与必要模型映射；本地模型目录、已配置协议转换和 Codex Live bootstrap 有明确特例。具体范围以 [协议与路径](USAGE.md#4-协议与路径) 为准，上游能力需要实际请求验证。

## 开发入口

先安装 Rust 1.88+、Node.js 22、uv；开发 macOS App 另需完整 Xcode 与 Swift 6+。所有命令从仓库根执行：

```bash
npm ci --prefix platforms/linux/webui
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
# macOS 主机另执行
./scripts/check.sh macos
```

第一次参与开发，可先读 [项目结构](docs/project-structure.md) 定位源码与测试，再按 [开发指南](docs/development.md) 配置环境和运行检查。PR 流程见 [贡献指南](CONTRIBUTING.md)。

## 项目地图

| 目录 | 责任 |
| --- | --- |
| `crates/` | 共享配置、路由、运行存储和代理引擎 |
| `adapters/linux/`、`adapters/macos/` | 平台权限、Admin 与服务组装 |
| `apps/` | Linux daemon / macOS sidecar 可执行入口 |
| `platforms/linux/` | WebUI、systemd、安装与打包输入 |
| `platforms/macos/` | SwiftUI、通知、资源与 DMG / Sparkle 打包 |
| `tests/contracts/` | Linux/macOS adapter 共用的行为测试 |
| `scripts/`、`.github/` | 统一检查、资源同步、CI 与发布流程 |
| `docs/` | 现行指南、`templates/` 生成输入、`archive/` 与 `upstream/` 历史资料 |

完整目录与维护源见 [项目结构](docs/project-structure.md)。依赖从 app 到 adapter，再到共享引擎及底层库；共享库不反向依赖平台，具体约束见 [架构说明](docs/architecture.md)。

## 维护与分发

- [文档索引](docs/README.md)：使用、配置、开发及平台参考。
- [版本记录](CHANGELOG.md)：两个平台共用版本与发布说明。
- [发布流程](docs/releasing.md)：质量门禁、版本、签名、产物和发布验收。
- [仓库公开流程](docs/publishing.md)：首次公开源码的准备与验收。
- [新仓库迁移](docs/repository-migration.md)：导出源码、重新建立作者历史及迁移发布设置。
- [安全政策](SECURITY.md)：敏感信息处理与漏洞报告。

GitHub Release、容器发布与静态镜像同步是独立结果；构建通过不表示已安装或部署。
