# 0.3.4 清理与发布准备

- ✅ 核对远端：origin 为 domoxiaojun/sumpter，公开空仓库，无现有 tag；Sparkle 两个 Secret 已配置。
- ✅ 清理：将 .audit、旧 plan.md、todos.md 备份到仓库外；移出无引用的 ProjectAttributionInstallerPanel.swift，保留文档索引引用的迁移记录与发布输入。
- ✅ 更新：Compose 版本示例、0.3.4 变更记录、macOS 安装器名称、Docker 健康检查变量和源码构建上下文、安装器自测脚本资源夹具。
- ✅ 当前快照验证：Rust 638 tests + fmt/clippy；WebUI 135 tests/build；Swift 194 XCTest + 34 Swift Testing；归因脚本 10 tests；actionlint/shellcheck/hadolint、文档同步、离线链接检查；历史与暂存快照 gitleaks 脱敏扫描通过。
- ✅ 归因安装器 UI 已完成，分发副本同步及相关增量测试通过，详见下方记录。
- [ ] 同步归因资源并针对最终文件验证，整理暂存区，提交并 push main；核对远端 SHA。
- [ ] 待编译任务全部完成后清理 Rust/Swift 缓存（约 59 GB）；本轮未删除，以免干扰并行任务。

备份：/Users/kkl/Documents/claude/sumpter-cleanup-backup-20260908/
Docker 构建与 Linux 安装器事务自测需在 Linux/CI 验证，本机不运行 Docker。
正式 tag/Release、Linux 静态镜像同步、DMG 和 GHCR 镜像发布均尚未执行。

# 客户端归因配置操作（本任务完成）

- ✅ 确认 macOS 命令面板、统一安装器及 Linux 下载与打包入口。
- ✅ macOS 自动检测归因配置，提供安装、更新和还原按钮，操作后自动复查。
- ✅ Linux 新增 scripts/setup-client-attribution.sh，支持交互菜单与 status/install/restore；接入 listener 下载、打包检查和 WebUI。
- ✅ 修复新建 rc 文件位于路径别名下时还原记录不一致的问题；统一分发副本已同步。
- ✅ 验证：Node 归因脚本 11 项，Swift UnifiedAttributionInstallerTests 2 项，Linux listener 下载访问控制 1 项，WebUI 135 项与构建，ShellCheck 和文档同步通过。

本任务仅完成源码和本地构建验证，未安装 App、提交、push 或发布；本机真实客户端配置未修改。

# v0.3.4 提交与正式发布

- ✅ 核对本地 main、空远端、版本号与发布 Secrets；没有需取消的旧构建。
- [ ] 合并发布准备与归因配置改动，验证最终资源并提交。
- [ ] 推送 main 与不可变 v0.3.4 标签，确认 Release 构建启动。
- [ ] 跟进构建和发布结果，核对 Release 资产与远端提交。
