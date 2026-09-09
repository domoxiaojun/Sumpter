# 开发指南

所有示例从仓库根执行。Linux 和 macOS 共用 Rust workspace；WebUI 和 Swift Package 各自使用锁文件。无需外部数据库，SQLite 随 Rust 编译。

先按 [项目结构](project-structure.md) 找到实现和测试位置，再根据下面的变更范围选择检查。共享模块的依赖约束见 [架构说明](architecture.md)。

## 环境

| 工具 | 要求 | 用途 |
| --- | --- | --- |
| Rust / Cargo | 1.88+，CI 使用 1.88.0 | 共享引擎与两个 adapter |
| Node.js / npm | Node 22，CI 使用 22.18.0 | WebUI、资源同步与脚本测试 |
| uv | 可用的稳定版，Python 3.11+ | 文档生成脚本 |
| Xcode / Swift | Xcode 26.6 (17F113)，Swift 6.3.3，macOS SDK 26.5 | macOS 14+ App，仅 macOS 主机 |
| actionlint / shellcheck / taplo / lychee | 本机或 CI 可用 | 对应文件静态检查；链接检查按需手动执行 |

macOS Homebrew rustup 若未进入 PATH，先执行 `export PATH="/opt/homebrew/opt/rustup/bin:$PATH"`。SwiftUI 宏需完整 Xcode，只有 Command Line Tools 不足；使用 `xcode-select -p` 与 `swift --version` 核对。

macOS CI 与 Release 使用 `macos-26` arm64 runner；与本地检查、打包共用 `platforms/macos/app/select-xcode.sh` 中固定的最新稳定工具链基线。脚本校验 Xcode 版本、构建号和 SDK，不会因多个版本都使用 Swift 6 而选中旧 Xcode。非默认位置可通过 `DEVELOPER_DIR` 指向同一版本；版本不匹配会停止并提示，不自动降级或安装。升级工具链时同步更新该脚本与本表，最低运行系统仍为 macOS 14。相同 SDK 可消除构建环境引入的样式差异，实际外观仍受运行系统和外观设置影响。

```bash
npm ci --prefix platforms/linux/webui
./scripts/check.sh docs
./scripts/check.sh rust
./scripts/check.sh web
# 仅 macOS
./scripts/check.sh macos
```

`./scripts/check.sh all` 执行 docs、rust、web，并在 macOS 上追加 App 检查；Linux 会明确报告跳过 App。本机不运行 Docker；多架构镜像由 Release 工作流校验并推送 GHCR。`web` 会重建静态资源，再用 `git diff` 检查产物与 Git 索引是否一致，并拒绝新增未跟踪资源。开发中源码与产物都未暂存时，这一检查会因预期差异退出，即使测试和构建已通过；应分别报告测试、构建和产物提交状态，不为通过检查而自动暂存或提交。

## 按变更选择验证

| 变更 | 最小检查 |
| --- | --- |
| 文档、模板、版本元数据 | `./scripts/check.sh docs`；含链接文档另跑 lychee |
| 共享 Rust 行为 | `./scripts/check.sh rust`，覆盖两个 adapter |
| WebUI | `./scripts/check.sh web`，必要的浏览器交互验收 |
| SwiftUI / macOS wire | `./scripts/check.sh macos`，必要的 App 界面验收 |
| Shell / workflow / TOML | shellcheck / actionlint / taplo；运行受影响脚本自测 |
| 发布配置 | 以上受影响检查，再按发布指南检查最终包 |

Rust gate 是 fmt、check、test、clippy，均不自动修复；构建与测试使用 `--locked`。先运行能证明改动正确的定向测试，变更范围需要时再跑整组：

```bash
cargo test --locked -p sumpter-core --test routing
cargo test --locked -p sumpter-engine --test replay_conformance
cargo test --locked -p sumpter-linux-adapter --test engine
cargo test --locked -p sumpter-macos-adapter --test engine
swift test --package-path platforms/macos/app --filter RuntimeV2WireContractTests
```

不要并行运行共享同一 `.build` 的 SwiftPM 命令。

## runtime ORM 开发

共享 runtime 使用 SeaORM 1.1.20 与内嵌 SQLite，保持 Rust 1.88 支持；SeaORM 2.x
要求更高的 Rust 版本，升级依赖时须一起核对 workspace 和 CI 工具链。
实体维护源是 `crates/sumpter-runtime/src/entities/`，新库结构由实体生成，检查约束
与查询索引在同层显式定义。事件投影和常规数据操作使用 ORM，复杂统计保留参数化 SQL。
所有读写经 `database.rs` 管理的 SeaORM 连接、事务与有界流式通道执行；不要新增平行数据库驱动。

新增或删减事件字段时，同时检查实体、`runtime_store/models.rs`、相关查询结果模型、
统计 SQL 与两个 adapter 的 wire 契约。修改实体不会自动改写现有数据库；旧版本库
继续在只读检查后阻断，清空重建仍需用户明确操作。不要将 ORM 接入视为旧库迁移授权。

先运行 `cargo +1.88.0 test --locked -p sumpter-runtime`，覆盖临时数据库上的约束、事务
回滚、流式取消、只读访问、重启恢复和十万行查询；随后运行两个 adapter 的定向测试。
测试数据库使用临时目录或独立内存连接，不指向正在使用的配置目录。

## 运行开发实例

复制根 `config.example.json` 到仓库外的专用目录，保留文件权限 `0600`。避免与已安装服务的端口冲突。以下使用 `/tmp/sumpter-dev` 作为专用测试目录：

```bash
cargo run --locked -p sumpterd-linux -- --config-dir /tmp/sumpter-dev --no-web
# macOS sidecar 手工运行时关闭 stdin EOF 监视
cargo run --locked -p sumpterd-macos -- --config-dir /tmp/sumpter-dev --foreground
```

模板入口默认关闭。需要真实流量时再在本地配置启用入口；两条示例命令择一运行。Linux WebUI 开发见 [前端说明](../platforms/linux/webui/README.md)。

## 单一维护源与生成副本

| 修改源 | 同步命令 | 同步目标 |
| --- | --- | --- |
| 根 `LICENSE`、`CHANGELOG.md`、`config.example.json` | `node scripts/maintenance/sync-project-metadata.mjs --write` | Linux 包内许可证/版本记录、两端配置模板 |
| `docs/templates/usage-onboarding.md`、`docs/templates/usage-path-matrix.json` | `uv run scripts/maintenance/sync-usage-docs.py --write` | 根 `USAGE.md` 与 `platforms/linux/USAGE.md` 的 BEGIN/END 标记块 |
| `scripts/clients/client-attribution.mjs`、`scripts/clients/pi-project-attribution.ts` | `node scripts/maintenance/sync-client-attribution.mjs` | 两个平台的 `scripts/`、Swift App `Resources/` 及各处 Gemini 兼容入口 |
| `platforms/linux/webui/` | `npm run build --prefix platforms/linux/webui` | `platforms/linux/web/` |

USAGE 标记块以外的正文仍需直接维护。`scripts/check.sh docs` 只检查同步，不写副本，同时运行版本/示例/导出工具契约测试。Node 脚本无需额外依赖。

先修改维护源，再执行对应同步命令，并一起审阅源文件与生成副本。平台 Shell 安装器与归因脚本不在上述 Node/TypeScript 同步范围内，修改前核对各平台脚本和 App 内置资源的调用关系。脚本分类见 [scripts/README.md](../scripts/README.md)。

## 文档链接检查

对本次修改的现行 Markdown 文件运行 `lychee --offline --include-fragments <文件...>` 验证本地路径与 Markdown 章节锚点；外部链接使用 `lychee --exclude-loopback --exclude-link-local <文件...>`。开发服务地址不属于公网链接检查。`docs/templates/usage-onboarding.md` 含未替换的路径占位符，应检查生成后的两份 USAGE。

新增现行文档时，在 [文档索引](README.md) 加入入口，并按需使用 lychee 检查该文档的链接。

CI 在 PR、main push 和手动触发时运行 Rust、WebUI、macOS App 与仓库契约检查。容器镜像只在发布工作流中验证和发布。分支保护在 GitHub 仓库设置中启用，不随源码自动生效。
