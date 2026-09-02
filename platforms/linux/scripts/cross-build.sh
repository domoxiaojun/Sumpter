#!/usr/bin/env bash
# 用 cargo-zigbuild 构建 x86_64/aarch64 两个静态 musl 包。
# 兼容两种布局：源码 monorepo 的根 workspace，以及把 platforms/linux
# 提升为根目录后的独立 Linux 发布树。
#
# 用法:
#   scripts/cross-build.sh --check  # 只报告环境/资源，不安装、不编译、不创建 dist
#   scripts/cross-build.sh          # 环境齐备后构建并打包两个架构
#
# 本脚本绝不调用 brew/apt/rustup target add/cargo install，也不使用 Docker。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ -f "$ROOT/../../Cargo.toml" && -d "$ROOT/../../crates" ]]; then
    REPO_ROOT="$(cd "$ROOT/../.." && pwd -P)"
else
    REPO_ROOT="$ROOT"
fi
DIST="$ROOT/dist"
TARGETS=(
    "x86_64-unknown-linux-musl:x86_64"
    "aarch64-unknown-linux-musl:aarch64"
)
CHECK_ONLY=0

case "${1:-}" in
    "") ;;
    --check) CHECK_ONLY=1 ;;
    *)
        echo "用法:$0 [--check]" >&2
        exit 2
        ;;
esac
[[ "$#" -le 1 ]] || {
    echo "用法:$0 [--check]" >&2
    exit 2
}

READY=1
INSTALLED_TARGETS=""

ok() { echo "[ok] $*"; }
missing() {
    echo "[缺失] $*" >&2
    READY=0
}

check_command() {
    local command_name="$1"
    local version_command="$2"
    if command -v "$command_name" >/dev/null 2>&1; then
        ok "$command_name:$(bash -c "$version_command" 2>&1 | head -n 1)"
    else
        missing "$command_name"
    fi
}

check_rust_tool_version() {
    local tool="$1"
    local minimum_minor=88
    local raw version major remainder minor
    command -v "$tool" >/dev/null 2>&1 || return 0
    raw="$("$tool" --version 2>/dev/null || true)"
    version="$(awk '{print $2}' <<<"$raw")"
    version="${version%%-*}"
    major="${version%%.*}"
    remainder="${version#*.}"
    minor="${remainder%%.*}"
    if [[ ! "$major" =~ ^[0-9]+$ || ! "$minor" =~ ^[0-9]+$ ]]; then
        missing "$tool 版本无法解析（需要 >= 1.${minimum_minor}）"
    elif ((major > 1 || (major == 1 && minor >= minimum_minor))); then
        ok "$tool 版本满足 Rust 2024/let-chains（>= 1.${minimum_minor}）"
    else
        missing "$tool $version 过旧（需要 >= 1.${minimum_minor}）"
    fi
}

check_file() {
    local path="$1"
    if [[ -f "$path" ]]; then
        ok "文件:$path"
    else
        missing "文件:$path"
    fi
}

check_directory() {
    local path="$1"
    if [[ -d "$path" ]]; then
        ok "目录:$path"
    else
        missing "目录:$path"
    fi
}

echo "== Rust musl 交叉构建环境检查 =="
check_command cargo 'cargo --version'
check_command rustc 'rustc --version'
check_rust_tool_version cargo
check_rust_tool_version rustc
check_command rustup 'rustup --version'
check_command zig 'zig version'
check_command file 'file --version || file -v'
if command -v cargo-zigbuild >/dev/null 2>&1 && cargo-zigbuild --version >/dev/null 2>&1; then
    ok "cargo-zigbuild:$(cargo-zigbuild --version 2>&1 | head -n 1)"
else
    missing "cargo-zigbuild(cargo-zigbuild --version 不可用)"
fi

if command -v rustup >/dev/null 2>&1; then
    INSTALLED_TARGETS="$(rustup target list --installed 2>/dev/null || true)"
fi
for entry in "${TARGETS[@]}"; do
    target="${entry%%:*}"
    if grep -Fxq "$target" <<<"$INSTALLED_TARGETS"; then
        ok "Rust target:$target"
    else
        missing "Rust target:$target"
    fi
done

check_file "$REPO_ROOT/Cargo.toml"
check_file "$REPO_ROOT/Cargo.lock"
check_file "$ROOT/config.example.json"
check_file "$ROOT/README.md"
check_file "$ROOT/USAGE.md"
check_file "$ROOT/specs/admin-api.md"
check_file "$ROOT/deploy/sumpter.service"
check_file "$ROOT/deploy/sumpter-system.service"
check_file "$ROOT/deploy/nginx-sumpter-admin.conf.example"
check_file "$ROOT/scripts/start.sh"
check_file "$ROOT/scripts/stop.sh"
check_file "$ROOT/scripts/smoke.sh"
check_file "$ROOT/scripts/install.sh"
check_file "$ROOT/scripts/uninstall.sh"
check_file "$ROOT/scripts/cc-project-attribution.sh"
check_directory "$ROOT/web"
check_file "$ROOT/web/index.html"

if [[ "$CHECK_ONLY" -eq 1 ]]; then
    echo
    echo "--check 结束:未安装任何工具、未编译、未创建或改动 dist。"
    if [[ "$READY" -eq 1 ]]; then
        echo "环境齐备。去掉 --check 才会开始双架构构建。"
        exit 0
    fi
    echo "环境尚未齐备；请人工安装上面列出的组件后重试。" >&2
    exit 1
fi

if [[ "$READY" -ne 1 ]]; then
    echo "错误:构建环境不完整；脚本不会自动安装依赖。先运行 --check 查看清单。" >&2
    exit 1
fi

mkdir -p "$DIST"
timestamp="$(date +%Y%m%d-%H%M%S)"

for entry in "${TARGETS[@]}"; do
    target="${entry%%:*}"
    arch="${entry##*:}"
    echo
    echo "== cargo zigbuild --release --locked --target $target =="
    (
        cd "$REPO_ROOT"
        cargo zigbuild --release --locked --target "$target" --bin sumpterd-linux
    )

    binary="$REPO_ROOT/target/$target/release/sumpterd-linux"
    [[ -x "$binary" ]] || {
        echo "错误:未找到构建产物 $binary" >&2
        exit 1
    }

    description="$(file "$binary")" || {
        echo "错误:file 无法检查 $target 产物" >&2
        exit 1
    }
    echo "$description"
    [[ "$description" == *"ELF"* ]] || {
        echo "错误:$target 产物不是 ELF" >&2
        exit 1
    }
    if [[ "$description" != *"statically linked"* && "$description" != *"static-pie linked"* ]]; then
        echo "错误:$target 产物未被 file 识别为静态链接" >&2
        exit 1
    fi

    stage="$(mktemp -d "$DIST/.stage-sumpter-linux-${arch}.XXXXXX")"
    final="$DIST/sumpter-linux-$arch"
    mkdir -p "$stage/deploy" "$stage/scripts" "$stage/specs" "$stage/web" "$stage/logs"
    cp "$binary" "$stage/sumpterd"
    chmod 0755 "$stage/sumpterd"
    cp -R "$ROOT/web/." "$stage/web/"
    for script in start.sh stop.sh smoke.sh install.sh bootstrap-install.sh uninstall.sh \
        bootstrap-uninstall.sh cc-project-attribution.sh; do
        cp "$ROOT/scripts/$script" "$stage/scripts/$script"
        chmod 0755 "$stage/scripts/$script"
    done
    cp "$ROOT/deploy/sumpter.service" "$stage/sumpter.service"
    cp "$ROOT/deploy/sumpter-system.service" "$stage/sumpter-system.service"
    cp "$ROOT/deploy/nginx-sumpter-admin.conf.example" \
        "$stage/deploy/nginx-sumpter-admin.conf.example"
    cp "$ROOT/specs/admin-api.md" "$stage/specs/admin-api.md"
    cp "$ROOT/config.example.json" "$stage/config.example.json"
    cp "$ROOT/README.md" "$stage/README.md"
    cp "$ROOT/USAGE.md" "$stage/USAGE.md"
    for legal_file in LICENSE NOTICE THIRD_PARTY_LICENSES; do
        if [[ -f "$ROOT/$legal_file" && ! -L "$ROOT/$legal_file" ]]; then
            cp "$ROOT/$legal_file" "$stage/$legal_file"
        fi
    done

    if [[ -e "$final" ]]; then
        previous="$final.prev-$timestamp"
        mv "$final" "$previous"
        echo "旧包已可恢复地保留:$previous"
    fi
    mv "$stage" "$final"
    echo "打包完成:$final"
done

echo
echo "双架构打包完成。必须把每个包放到对应 Linux 架构后运行 scripts/smoke.sh；"
echo "交叉构建成功不等同于 systemd、浏览器或真实代理流量已经验收。"
