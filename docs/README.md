# 文档索引

按任务选择入口。当前产品版本以根 `Cargo.toml` 为准（0.4.6），磁盘格式为 schema v7，安装入口为 [GitHub Releases](https://github.com/domoxiaojun/sumpter/releases/latest) 与仓库 raw 脚本。行为以源码、平台契约测试和现行文档共同核对。

## 使用与运维

| 用途 | 文档 |
| --- | --- |
| 产品、版本、GitHub 安装 | [README](../README.md) |
| 开箱、客户端接入、归因、排错 | [使用指南](../USAGE.md) |
| 两端共用 schema v7 字段 | [配置说明](configuration.md) |
| Linux 安装、服务管理与部署包 | [Linux 指南](../platforms/linux/README.md) |
| macOS App、sidecar 与通知 | [macOS 指南](../platforms/macos/README.md) |

## 开发与协作

| 用途 | 文档 |
| --- | --- |
| 目录职责、源码定位、测试归属 | [项目结构](project-structure.md) |
| 模块边界、请求处理与运行时约束 | [架构说明](architecture.md) |
| 环境、构建、测试、生成资源 | [开发指南](development.md) |
| 仓库脚本分类与维护入口 | [脚本目录](../scripts/README.md) |
| Linux 前端开发与接口 | [WebUI](../platforms/linux/webui/README.md)、[Admin API](../platforms/linux/specs/admin-api.md) |
| Issue、分支、提交、PR | [贡献指南](../CONTRIBUTING.md) |
| AI 工具的仓库规则 | [AGENTS](../AGENTS.md) |

## 版本与仓库维护

| 用途 | 文档 |
| --- | --- |
| 两端版本变化 | [CHANGELOG](../CHANGELOG.md) |
| 版本、CI、签名、发布验收 | [发布指南](releasing.md) |
| 漏洞报告与密钥处理 | [安全政策](../SECURITY.md) |

## 平台补充

- Linux：[包内使用指南](../platforms/linux/USAGE.md)、[Docker 独立部署与迁移](../platforms/linux/DOCKER.md)、[配置目录](../platforms/linux/config/README.md)、[Scriptable](../platforms/linux/integrations/scriptable/README.md)、[TSX 集成](../platforms/linux/integrations/tsx/README.md)。
- macOS：[安装说明](../platforms/macos/app/INSTALL.txt)、[Sparkle 更新](../platforms/macos/app/UPDATE.md)。

## 文档维护

- 当前公共文档放在 `docs/`；平台安装、运维与接口细节保留在对应平台目录。
- 配置字段维护在 `configuration.md`，目录地图维护在 `project-structure.md`，验证与同步命令维护在 `development.md`；其他文档通过链接引用。
- [templates](templates/README.md) 保存生成输入；两份 USAGE 的标记块由工具同步。维护源与副本关系见 [开发指南](development.md#单一维护源与生成副本)。
