# 贡献指南

Sumpter 使用一个仓库维护 Linux 与 macOS。先按 [项目结构](docs/project-structure.md) 定位源码与测试，再阅读 [开发指南](docs/development.md) 和 [架构边界](docs/architecture.md)。

## 从需求到合并

1. Bug 提供版本、平台、预期与实际行为、最小复现；功能先写用户场景、范围和验收条件。
2. 从最新 `main` 建立短期分支，例如 `fix/runtime-pagination` 或 `feat/model-groups`。
3. 按最小完整范围实现。较长任务可在本地 `todos.md` 记录中文步骤；任务草稿不入库，完成后把行为写入产品文档。
4. 使用 `scripts/check.sh` 运行受影响层的检查。共享行为覆盖 Linux/macOS 两个 adapter；UI 改动核对两端的字段、状态和主要交互。
5. 更新配置、使用说明或 `CHANGELOG.md` 的 Unreleased 条目；同步生成副本。
6. 提交 PR，说明问题、结果、影响平台、实际验证及限制。CI 成功并完成审阅后合并，删除临时分支。

单人维护同样使用 PR 自查与自动门禁；不要强制一个不存在的第二审阅者。受保护分支、禁止 force push 和必需检查由仓库设置启用，提交配置文件不会自动启用这些保护。

## 提交与作者

使用 Conventional Commits，例如 `fix(runtime): preserve request-chain ordering`、`docs(repo): clarify release checks`。一次提交完成一个可描述的改动。

Git 作者使用实际贡献者身份。AI 辅助工具不写入 Author 或 `Co-Authored-By`，也不在提交正文追加自动工具署名；真实的人类共同贡献者按实际情况署名。AI 工具使用 [AGENTS.md](AGENTS.md) 的同一套规则。

## 变更规则

- core/runtime/engine 保持平台中立；平台差异经 adapter 和 `PlatformBoundary` 注入。
- 配置 schema 或 API wire 改动同步 Rust、Swift、WebUI、模板和必要迁移测试。
- 只暂存本次文件，保留无关工作区改动；不提交缓存、真实配置、运行库和诊断捕获。
- 修改 WebUI 源码后提交对应 `platforms/linux/web/` 构建产物；不手工编辑带同步标记的文档块。
- Rust 使用项目 rustfmt；Python 通过 uv 执行；不自动做全仓格式化。
- 完成目标并通过最小必要验证后停止。测试、安装、发布、部署分别报告，不能相互代替。

安全问题按照 [SECURITY.md](SECURITY.md) 私下报告；正常问题使用 Issue 模板。
