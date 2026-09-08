#!/usr/bin/env bash
# 仓库根开发入口；除 web 构建产物外，不自动修复或改写源码。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
mode="${1:-docs}"
if [[ "$#" -gt 1 ]]; then
    echo "用法: $0 [docs|rust|web|macos|all]" >&2
    exit 2
fi
check_docs() {
    uv run scripts/maintenance/sync-usage-docs.py --check
    node scripts/maintenance/sync-project-metadata.mjs --check
    node scripts/maintenance/sync-client-attribution.mjs --check
    node --test scripts/tests/repository-contracts.test.mjs scripts/tests/export-source.test.mjs
}
check_rust() {
    cargo fmt --all -- --check
    cargo check --workspace --all-targets --locked
    cargo test --workspace --locked
    cargo clippy --workspace --all-targets --locked -- -D warnings
}
check_web() {
    npm test --prefix platforms/linux/webui
    node --test scripts/tests/pi-project-attribution.test.mjs scripts/tests/client-attribution.test.mjs
    npm run build --prefix platforms/linux/webui
    git diff --exit-code -- platforms/linux/web
    if [[ -n "$(git ls-files --others --exclude-standard -- platforms/linux/web)" ]]; then
        echo "WebUI 构建产生了新的未跟踪资源，请审阅并提交" >&2
        return 1
    fi
}
check_macos() {
    if [[ "$(uname -s)" != Darwin ]]; then
        echo "macos 检查必须在 macOS 主机执行" >&2
        return 1
    fi
    swift build --package-path platforms/macos/app --product SumpterApp
    swift test --package-path platforms/macos/app
}
case "$mode" in
    docs) check_docs ;;
    rust) check_rust ;;
    web) check_web ;;
    macos) check_macos ;;
    all)
        check_docs
        check_rust
        check_web
        if [[ "$(uname -s)" == Darwin ]]; then check_macos
        else echo "跳过 macOS App：请由 macOS CI 验证"; fi
        ;;
    *) echo "用法: $0 [docs|rust|web|macos|all]" >&2; exit 2 ;;
esac
