# Sumpter

Sumpter 是一个跨平台、本地优先的多协议代理引擎。Rust 数据面只有一份共享实现，
Linux 与 macOS 通过各自的 adapter 和应用入口接入；平台 UI、安装器和发布输入集中在
`platforms/`，不混入共享 workspace。

源码仓库：[domoxiaojun/sumpter](https://github.com/domoxiaojun/sumpter)

## 当前结构

```text
crates/                 共享 core / runtime / engine
adapters/linux/         Linux 平台边界与 Admin facade
adapters/macos/         macOS 平台边界与 sidecar facade
apps/linux/             sumpterd-linux 可执行入口
apps/macos/             sumpterd-macos 可执行入口
platforms/linux/        WebUI、静态资源、systemd、安装与 Linux 发布输入
platforms/macos/        SwiftUI App、图标、Sparkle 与 DMG 发布输入
docs/                   当前架构、审查总结与历史对照资料
scripts/                仓库级维护脚本
```

依赖方向固定为：

```text
apps/<platform> → adapters/<platform> → sumpter-engine → sumpter-runtime → sumpter-core
```

共享 crate 不得反向依赖 adapter、操作系统或 UI；平台差异通过
`crates/sumpter-engine/src/boundary.rs` 的边界接口注入。

## 开发与验证

在仓库根目录运行：

```bash
PATH="/opt/homebrew/opt/rustup/bin:$PATH" cargo fmt --all -- --check
PATH="/opt/homebrew/opt/rustup/bin:$PATH" cargo check --workspace --locked
PATH="/opt/homebrew/opt/rustup/bin:$PATH" cargo test --workspace --locked
uv run scripts/sync-usage-docs.py --check
```

本地构建 macOS 测试 DMG：

```bash
PATH="/opt/homebrew/opt/rustup/bin:$PATH" ./scripts/build-macos-dmg.sh --clean
```

脚本默认运行 Rust/Swift 测试并生成 `platforms/macos/app/dist/Sumpter-local.dmg`；
`--skip-tests` 可用于后续增量打包。该产物使用 ad-hoc 签名，仅用于本机测试，不包含
公证、上传或发布流程。

Rust workspace 当前包含 7 个 package。Swift Package 位于
`platforms/macos/app/`；Linux WebUI 源码位于 `platforms/linux/webui/`，构建后的静态资源
位于 `platforms/linux/web/`。

## 文档入口

- [架构与变更归属](docs/architecture.md)
- [深度代码审查与维护总结](docs/code-review-2026-08-31.md)
- [使用与接入指南](USAGE.md)
- [文档索引](docs/README.md)
- [当前执行计划](plan.md)

## 发布边界

`platforms/linux/` 和 `platforms/macos/` 保留各自的安装、systemd、SwiftUI、DMG 与
Linux 发布输入。独立 Linux 发布包会在发布阶段将 `platforms/linux/` 提升为包根；这不
改变源码树的目录规范。现有交叉构建、release preflight 和 macOS 打包脚本仍有独立发布
树、旧 profile 或 sidecar 命名假设，不能仅凭源码树检查通过就宣称 DMG/Linux 发布已恢复。
