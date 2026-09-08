# 仓库工作规则

适用于整个仓库。人类贡献流程见 [CONTRIBUTING.md](CONTRIBUTING.md)，具体命令见 [开发指南](docs/development.md)。

## 工程边界

- 唯一 Rust 2024 workspace 在根 `Cargo.toml`，最低 Rust 1.88。
- `crates/sumpter-core`：配置、路由、调度、事件与协议纯函数。
- `crates/sumpter-runtime`：bundled SQLite 存储与查询。
- `crates/sumpter-engine`：共享 HTTP/WebSocket 代理管线、relay、重试和回放。
- `adapters/{linux,macos}`：平台边界、Admin 与服务组合；`apps/` 为薄可执行入口。
- `platforms/` 维护 WebUI / SwiftUI / 安装与打包输入。共享库不得反向依赖平台或 UI。
- 对应页面保持信息层级、字段、状态语义与主要交互一致，布局和原生控件可按平台适配。

## 工作流程

- 用户描述模糊时先梳理用户场景、范围与验收条件，用中文给出专业表达。
- 长任务写 `plan.md`，每步完成标记 ✅。OpenSpec 与计划使用中文。
- 搜索先用 fd、rg 或 git grep；只在需要语法结构时使用 ast-grep，避免无目的全仓扫描。
- 保留无关工作区改动，精确暂存文件；不自行 push、重写历史或发布。
- 使用实际 Git 作者身份；不得追加 AI 的 Author、Co-Authored-By 或自动工具署名。
- 完成目标并通过最小必要验证后停止，不重复审查或做无明确收益的优化。

## 验证与生成文件

- 优先使用 `./scripts/check.sh docs|rust|web|macos|all` 和受影响模块的定向测试。
- 共享行为改变须覆盖两个 adapter；不要用 `--all-features` 替代平台测试。
- Python 全部通过 uv 运行；大型 Python 项目使用 venv。本机不安装外部数据库，不安装或运行 Docker；容器验证在 Linux CI 完成。
- TOML 用 taplo，Shell 用 shellcheck，GitHub Actions 用 actionlint，文档链接用 lychee。验证默认只读，不做全仓自动修复。
- 修改 WebUI 后重建并提交 `platforms/linux/web/`；同步命令见开发指南。
- 根 `config.example.json` 只放禁用的合成入口与 `.invalid` 主机；不要提交密钥、数据库、诊断捕获、target 或其他缓存。
- `docs/upstream/` 与旧审查仅作历史参考；不要把其中的旧 schema 和发布路径恢复到现行代码。

## 交付说明

报告本次行为变化、实际验证与剩余边界。源码测试、打包、签名、公证、安装、发布与生产部署分别核对。历史测试计数不得当作本次验证结果。
