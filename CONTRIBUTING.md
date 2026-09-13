# 贡献指南

先阅读 [项目结构](docs/project-structure.md)、[架构说明](docs/architecture.md) 和 [开发指南](docs/development.md)。Sumpter 的共享 Rust workspace 同时服务 Linux 与 macOS，平台页面和安装脚本位于 `platforms/`。

## 提交改动

1. 描述用户场景、原行为、预期行为和验收条件。
2. 从最新 `main` 建立短分支，保持一次提交一个可说明的改动。
3. 共享行为同时核对两个 adapter；WebUI 改动同时更新 `platforms/linux/web/` 生成产物。
4. 运行受影响层的最小检查：文档 `./scripts/check.sh docs`，Rust `rust`，WebUI `web`，Compose `docker`，macOS App `macos`。
5. 只暂存本次文件，保留混合工作区中的其它改动；同步配置示例和现行手册。
6. PR 说明触发条件、用户可见结果、影响平台、实际命令和剩余未验证层次。

## 代码边界

`core`、`runtime`、`engine` 不依赖平台或 UI；平台差异通过 adapter 和 `PlatformBoundary` 注入。配置 schema、API wire、统计字段和安装资源变化要同步 Rust、Swift、WebUI、模板与定向测试。

Python 通过 uv 执行；本机不安装数据库、不启动 Docker。Compose 契约测试可在无 Docker 的环境运行，真实镜像和容器行为由 Linux CI 验证。

## 作者与安全

使用真实贡献者 Git 身份，不添加 AI Author 或 `Co-Authored-By`。安全问题遵循 [安全政策](SECURITY.md) 私下报告；不要把凭据、运行数据库、诊断捕获或缓存提交到仓库。
