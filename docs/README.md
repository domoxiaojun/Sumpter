# 文档目录

这套文档对应 Sumpter **0.4.16 / schema v7**。第一次接触项目先看 [项目介绍](../README.md)，安装后按 [使用手册](../USAGE.md) 完成第一条请求。

## 安装与部署

| 文档 | 内容 |
| --- | --- |
| [Docker Compose](../platforms/linux/DOCKER.md) | 复制示例、准备目录与配置、首次登录、网络、备份与升级 |
| [Linux](../platforms/linux/README.md) | systemd 安装、服务管理、密码恢复和卸载 |
| [macOS](../platforms/macos/README.md) | App 安装、启动、退出、通知和更新 |

## 使用与配置

| 文档 | 内容 |
| --- | --- |
| [使用手册](../USAGE.md) | 从添加上游到客户端接入，模型组、归因和日常管理 |
| [配置参考](configuration.md) | 当前 JSON 字段、模型范围、重试与凭据边界 |
| [故障排查](troubleshooting.md) | 按症状检查连接、认证、路由和存储 |
| [Linux 包内手册](../platforms/linux/USAGE.md) | 随 Linux 发布包提供的独立使用说明 |
| [容器数据目录](../platforms/linux/config/README.md) | 持久化文件的用途和备份范围 |
| [Admin API](../platforms/linux/specs/admin-api.md) | Linux 管理接口、Cookie、CSRF 和配置并发控制 |
| [Scriptable 小组件](../platforms/linux/integrations/scriptable/README.md) | 在 iPhone 查看进行中请求 |
| [TSX 状态脚本](../platforms/linux/integrations/tsx/README.md) | 在终端读取状态与进行中请求 |

## 开发与维护

| 文档 | 内容 |
| --- | --- |
| [项目结构](project-structure.md) | 按需求查找源码、测试和生成文件 |
| [架构](architecture.md) | 依赖方向、请求处理、平台边界与持久化 |
| [开发指南](development.md) | 环境、开发实例、验证和资源同步 |
| [WebUI 开发](../platforms/linux/webui/README.md) | 前端运行、模拟数据、构建和 Admin 连接 |
| [脚本目录](../scripts/README.md) | 检查、同步、安装和打包入口 |
| [文档生成](templates/README.md) | 两份使用手册的单一维护源 |
| [部署契约测试](../scripts/tests/docker/README.md) | 无需运行 Docker 的模板验证 |
| [发布指南](releasing.md) | 版本、CI、资产与发布验收 |
| [macOS 更新机制](../platforms/macos/app/UPDATE.md) | Sparkle、签名和更新源 |
| [贡献指南](../CONTRIBUTING.md) | 提交改动和 PR 要求 |
| [安全政策](../SECURITY.md) | 凭据、诊断数据与漏洞报告 |

历史版本事实保存在 [CHANGELOG](../CHANGELOG.md)。安装、使用和开发操作以这些现行指南为准。
