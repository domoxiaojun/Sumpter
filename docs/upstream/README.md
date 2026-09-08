> 历史快照：仅用于追溯，不是当前使用、架构或发布契约。现行入口见 [文档索引](../README.md)。

# 历史快照（不是当前仓库说明）

本文以及同目录其它文件是重构前源码树的副本，路径仍写 `macos/`、`linux/`、`kekulvd`。
**当前**产品说明、目录和构建命令以仓库根 [`README.md`](../../README.md) 和
[`docs/architecture.md`](../architecture.md) 为准。不要按下面的 `cd macos` / `cd linux`
或 `kekulv` remote 流程操作现在的 monorepo。

---

# Sumpter（当时的产品说明）

本地分流代理：在 Claude Code / Codex 与多个上游之间做智能分流。当时的仓库只保留两套
Rust 产品。

| 产品 | 目录 | 形态 | 产物 |
|---|---|---|---|
| macOS | `macos/` | SwiftUI 菜单栏 App + sidecar `kekulvd` | `macos/app/dist/Sumpter.app`、`Sumpter.dmg` |
| Linux | `linux/` | standalone daemon + Web Admin | GitHub Release 静态包、GHCR 镜像 |

配置均为 schema v6 `config.json`（自动迁移 v3/v4/v5）。开箱试用与填配置见 **[USAGE.md](USAGE.md)**。
字段逐项说明见 `macos/CONFIG.md`。

## API 兼容范围

三种对话协议由入口四态 `protocol`（默认 `auto`）选择 Native Adapter 或安全 Translator；
macOS、Linux 还通过独立 Adapter 支持 Images Generations / Edits（含 multipart）、Legacy
Completions、Claude Count Tokens、Responses Compact 与 Codex Alpha Search。原生请求保留未知字段；
WebSearch 不需要 Provider 级能力开关：严格 RequestPurpose 配合最终 TargetFormat 自动保留
Anthropic 原生 `web_search`、为 OpenAI Chat 使用 `web_search_options`，或为 Responses 使用内建
`web_search`；Grok 检索仍要求 Responses。
Responses WebSocket、Realtime / Live、Videos、Files 尚不在当前 relay 生命周期内。完整路径、
协议与别名矩阵见 [USAGE.md](USAGE.md#4-协议与路径)。

## 诊断捕获

macOS“诊断”页和 Linux Web Admin“系统与服务诊断”页都提供手动开始/停止的完整捕获。
捕获内容不脱敏，持久化到配置目录的 `diagnostic_capture.json`（0600）；总容量可在开始前按 MB
设置，默认 512 MB。达到上限后捕获会自动停止并保留已有记录；手动停止和重启都不会清空，
只有显式“清空”才会移除。页面刷新只读取轻量索引，用户选择具体请求后才按需读取明文详情。

macOS 读 `~/Library/Application Support/kekulv/`，Linux 读 `$XDG_CONFIG_HOME/kekulv` 或 `~/.config/kekulv`。

## macOS

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"
cd macos
cargo test
cd app
./package-app.sh    # 需要完整 Xcode → dist/Sumpter.app + Sumpter.dmg
```

Claude Code：`ANTHROPIC_BASE_URL` 指向菜单栏 App 里显示的监听地址（默认
`http://127.0.0.1:57878`）。

## Linux

独立远程仓库：[domoxiaojun/sumpter](https://github.com/domoxiaojun/sumpter)。
本 monorepo 的 `linux/` 就是那份仓库的根目录内容。

```bash
cd linux
cargo test --workspace --locked
```

安装、Docker、发布资产见 `linux/README.md`。

### 向 Linux 远程发布（不要推错）

`kekulv` remote 指向 Linux **独立仓库根**，不是这个 monorepo。

- 要用 `linux/` 的 tree 生成提交，父节点接 `kekulv/main`，再推那颗提交。
- **禁止** `git push kekulv main`：会把整个 monorepo 覆盖到 Linux 仓库上。

```bash
# 正确口径（示意）：tree 必须是 HEAD:linux
TREE=$(git rev-parse HEAD:linux)
PARENT=$(git rev-parse kekulv/main)
COMMIT=$(git commit-tree "$TREE" -p "$PARENT" -m "release(linux): …")
git push kekulv "$COMMIT:main"
```

## 安全

- 密钥只放本机 `config.json`（0600），不入库。
- 仓库根目录不再保留任何密钥文件（旧 `keys.json` 已于 2026-08-22 删除）；
  `.gitignore` 仍拦着 `keys.json` 与 `.control_token`。
