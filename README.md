# Sumpter

Sumpter 把多个 AI 服务入口集中到一个代理地址。客户端连接 Sumpter，由它选择上游、映射模型、保持会话归属、处理故障切换，并记录请求和用量。

适合同时使用多个 Provider、希望统一管理客户端连接，或需要按项目查看 AI 用量的个人与小团队。你需要自行准备可用的上游服务和 API Key。

Linux 提供后台服务和 Web 管理界面；macOS 提供原生 App。两端共用 Rust 引擎和配置格式。本文对应 **0.4.20 / schema v7**，版本以 [Cargo.toml](Cargo.toml) 为准。

## 可以做什么

- **管理上游**：在入口库保存地址、密钥和模型映射，按模型组分配主用与后备入口。
- **保持会话稳定**：支持优先级、随机粘性和轮询粘性调度；故障时按配置重试或切换入口。
- **接入现有客户端**：通过 HTTP、流式响应和 WebSocket 转发 Claude Code、Codex、Grok Build、Gemini CLI、pi 等客户端请求。
- **查看运行情况**：区分客户端请求与上游尝试，查看耗时、结果、Token、缓存、项目和会话统计。
- **控制访问和存储**：设置入站 Token、管理页凭据、统计保留策略；需要排障时再开启诊断捕获。

会话请求按协议择路：入口协议与客户端一致时按字节原生转发；不一致时在模型映射范围内经协议转换桥适配，响应再转回客户端方言。Anthropic Messages、OpenAI Chat Completions、OpenAI Responses 与 Gemini `generateContent`/`streamGenerateContent` 之间两两可转。Compact、countTokens、embedContent、图片生成、Realtime 等没有会话语义的接口仍只走原生路径。

转换不等于全功能兼容：历史推理不回放，服务端工具、文件引用以及目标协议没有等价参数的项会被明确拒绝，而不是静默丢弃。Sumpter 也不提供模型账号或代办上游登录。

## 选择安装方式

| 使用场景 | 入口 |
| --- | --- |
| Linux 服务器，使用 Docker Compose | [Compose 部署教程](platforms/linux/DOCKER.md) |
| Linux 服务器，直接使用 systemd | [Linux 安装与维护](platforms/linux/README.md) |
| macOS 14+，使用原生界面 | [macOS 安装与使用](platforms/macos/README.md) |

安装包从 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest) 下载。Linux 包支持 x86_64 / aarch64，容器镜像支持 amd64 / arm64；当前自动发布的 macOS 包面向 Apple Silicon。

Compose 教程从新建部署目录开始，依次完成**复制 Compose 模板、复制配置示例、创建密码文件、启动、首次登录和请求验证**。数据保存在部署目录的 `config/` 中。

## 第一次使用

1. 安装并打开 macOS App 或 Linux WebUI。
2. 在「入口库」添加一个真实上游，填写地址、API Key 和可用模型。
3. 在「模型组」启用该模型，并绑定刚添加的入口。
4. 在「安全」设置代理入站 Token，再将客户端指向代理地址。
5. 发出一条简单请求，在「运行」确认最终成功、实际入口和模型。

默认代理地址为 `http://127.0.0.1:57878`；Linux WebUI 为 `http://127.0.0.1:57879/admin/`。客户端的 Base URL 是否带 `/v1` 取决于客户端协议，具体配置见 [使用手册](USAGE.md)。

`config.example.json` 中的入口和模型组均为停用示例，域名不可连接。启动成功后仍须完成上游配置，才能转发真实请求。

## 继续阅读

| 你要完成的事情 | 文档 |
| --- | --- |
| 配置入口、模型组、客户端和归因，理解运行页面 | [使用手册](USAGE.md) |
| 手工编辑 JSON，查询字段和默认行为 | [配置参考](docs/configuration.md) |
| 排查连接、认证、模型与存储问题 | [故障排查](docs/troubleshooting.md) |
| 找源码、运行检查、提交改动 | [开发指南](docs/development.md) · [贡献指南](CONTRIBUTING.md) |
| 打包与发布新版本 | [发布指南](docs/releasing.md) |
| 查找全部专题 | [文档目录](docs/README.md) |

项目采用 [MIT 许可证](LICENSE)。版本变化见 [更新记录](CHANGELOG.md)，安全问题按 [安全政策](SECURITY.md) 私下反馈。
