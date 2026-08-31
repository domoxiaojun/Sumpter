#!/usr/bin/env bash
# Assemble a Linux release/staging tree that contains the shared engine.
#
# The source checkout remains untouched. In the assembled tree Linux Cargo
# manifests point at ./shared, while the macOS checkout keeps its own
# ../shared topology. This script also accepts the standalone mirror layout
# used by `sumpter`, where the shared workspace is the source root itself.
# It only prepares a staging tree; it does not publish, tag, deploy, or invoke
# Docker.
set -euo pipefail

LINUX_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
SOURCE_ROOT="$(cd -- "$LINUX_ROOT/.." && pwd -P)"
if [[ -d "$SOURCE_ROOT/shared/crates/kekulv-engine" ]]; then
    SHARED_ROOT="$SOURCE_ROOT/shared"
    SHARED_LAYOUT="nested"
elif [[ -d "$SOURCE_ROOT/crates/kekulv-engine" ]]; then
    # `/Users/kkl/Documents/claude/sumpter` is itself the shared workspace.
    SHARED_ROOT="$SOURCE_ROOT"
    SHARED_LAYOUT="root"
else
    SHARED_ROOT="$SOURCE_ROOT/shared"
    SHARED_LAYOUT="nested"
fi

CHECK_ONLY=0
VERIFY=0
OUTPUT=""

usage() {
    cat <<'EOF'
用法: scripts/assemble-shared-tree.sh [--check] [--verify] [--output DIR]

  --check       只检查共享目录、清单工具和路径拓扑，不创建输出目录
  --verify      组装后在 staging tree 运行 Linux unified-engine check
  --output DIR  输出新的 staging tree（目录必须不存在）
EOF
}

while [[ "$#" -gt 0 ]]; do
    case "$1" in
        --check)
            CHECK_ONLY=1
            shift
            ;;
        --verify)
            VERIFY=1
            shift
            ;;
        --output)
            [[ "$#" -ge 2 ]] || {
                echo "--output 缺少目录" >&2
                exit 2
            }
            OUTPUT="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "未知参数:$1" >&2
            usage >&2
            exit 2
            ;;
    esac
done

fail() {
    echo "assemble-shared-tree FAIL: $*" >&2
    exit 1
}

ok() {
    echo "assemble-shared-tree OK: $*"
}

[[ -d "$LINUX_ROOT/crates" ]] || fail "Linux crates 目录不存在:$LINUX_ROOT/crates"
[[ -f "$LINUX_ROOT/Cargo.toml" ]] || fail "Linux Cargo.toml 不存在"
[[ -f "$LINUX_ROOT/Cargo.lock" ]] || fail "Linux Cargo.lock 不存在"
[[ -d "$SHARED_ROOT/crates/kekulv-engine" ]] || fail "共享引擎目录不存在:$SHARED_ROOT"
[[ -f "$SHARED_ROOT/Cargo.toml" ]] || fail "shared/Cargo.toml 不存在"
command -v git >/dev/null 2>&1 || fail "git 不可用"
command -v shasum >/dev/null 2>&1 || fail "shasum 不可用"

SOURCE_COMMIT="$(git -C "$SOURCE_ROOT" rev-parse HEAD 2>/dev/null || true)"
if [[ -z "$SOURCE_COMMIT" && -f "$SOURCE_ROOT/SOURCE_COMMIT" ]]; then
    SOURCE_COMMIT="$(tr -d '[:space:]' < "$SOURCE_ROOT/SOURCE_COMMIT")"
fi
[[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]] || fail "无法读取有效的源提交"
ok "source commit $SOURCE_COMMIT"
ok "shared source $SHARED_ROOT"

if [[ "$CHECK_ONLY" -eq 1 ]]; then
    [[ -z "$OUTPUT" ]] || fail "--check 不应同时指定 --output"
    echo "--check 结束:未创建 staging tree，未修改源 checkout。"
    exit 0
fi

[[ -n "$OUTPUT" ]] || {
    echo "非 --check 模式必须指定 --output DIR" >&2
    usage >&2
    exit 2
}
[[ ! -e "$OUTPUT" ]] || fail "输出目录已存在，为避免覆盖而停止:$OUTPUT"

stage_parent="$(dirname -- "$OUTPUT")"
mkdir -p "$stage_parent"
stage="$(mktemp -d "$stage_parent/.kekulv-shared-stage.XXXXXX")"
cleanup() {
    if [[ -d "$stage" ]]; then
        rm -rf -- "$stage"
    fi
}
trap cleanup EXIT

# Copy the Linux source tree, then discard only generated/build material from
# the temporary staging copy. The source checkout is never touched.
cp -R "$LINUX_ROOT/." "$stage/"
rm -rf -- "$stage/.git" "$stage/target" "$stage/dist" "$stage/webui/node_modules"

# A nested Cargo workspace is otherwise discovered as part of the copied
# Linux workspace and its `workspace = true` dependencies inherit the wrong
# root manifest.  Excluding the sibling directory keeps shared/Cargo.toml as
# its own workspace while still allowing the path dependency below.
if ! grep -Eq '^exclude[[:space:]]*=.*shared' "$stage/Cargo.toml"; then
    temporary="$stage/Cargo.toml.tmp"
    awk '
        /^\[workspace\]$/ { print; print "exclude = [\"shared\"]"; next }
        { print }
    ' "$stage/Cargo.toml" > "$temporary"
    mv -- "$temporary" "$stage/Cargo.toml"
fi

# The standalone Linux release root contains crates/ and shared/ as siblings;
# the monorepo path is one directory deeper. Rewrite only the staged Linux
# manifests. No macOS manifest is copied or rewritten here. The second
# replacement handles the standalone mirror's direct `../../../crates` path.
while IFS= read -r -d '' manifest; do
    temporary="$manifest.tmp"
    sed \
        -e 's#\.\./\.\./\.\./shared/#../../shared/#g' \
        -e 's#\.\./\.\./\.\./crates/#../../shared/crates/#g' \
        "$manifest" > "$temporary"
    mv -- "$temporary" "$manifest"
done < <(find "$stage" -path '*/crates/*/Cargo.toml' -type f -print0)

if [[ "$SHARED_LAYOUT" == "root" ]]; then
    mkdir -p "$stage/shared"
    cp "$SHARED_ROOT/Cargo.toml" "$stage/shared/Cargo.toml"
    [[ -f "$SHARED_ROOT/Cargo.lock" ]] && cp "$SHARED_ROOT/Cargo.lock" "$stage/shared/Cargo.lock"
    [[ -f "$SHARED_ROOT/config.example.json" ]] && cp "$SHARED_ROOT/config.example.json" "$stage/shared/config.example.json"
    cp -R "$SHARED_ROOT/crates" "$stage/shared/crates"
else
    cp -R "$SHARED_ROOT" "$stage/shared"
fi
rm -rf -- "$stage/shared/target"

printf '%s\n' "$SOURCE_COMMIT" > "$stage/SOURCE_COMMIT"
{
    echo "source_commit=$SOURCE_COMMIT"
    echo "shared_root=shared"
    echo "manifest_path=shared/Cargo.toml"
} > "$stage/shared/ASSEMBLY_METADATA"

# Keep the manifest self-contained: every path is relative to the shared
# directory, so release-preflight and downstream packagers can verify it from
# any staging-tree parent without reconstructing the monorepo layout.
(
    cd "$stage/shared"
    find . -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 shasum -a 256
) > "$stage/shared/SHA256SUMS"

if [[ "$VERIFY" -eq 1 ]]; then
    command -v cargo >/dev/null 2>&1 || fail "--verify 需要 cargo"
    (
        cd "$stage"
        cargo check --manifest-path Cargo.toml --locked --features unified-engine
    )
fi

mv -- "$stage" "$OUTPUT"
trap - EXIT
ok "assembled staging tree $OUTPUT"
ok "shared manifest $OUTPUT/shared/SHA256SUMS"
