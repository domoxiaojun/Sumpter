#!/usr/bin/env bash
# Generate and sign a Sparkle appcast from versioned .zip/.dmg archives.
set -Eeuo pipefail
umask 077

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ARCHIVES_DIR="${1:-}"
GENERATE_APPCAST="${SPARKLE_GENERATE_APPCAST:-$ROOT/.build/artifacts/sparkle/Sparkle/bin/generate_appcast}"
DOWNLOAD_URL_PREFIX="${SPARKLE_DOWNLOAD_URL_PREFIX:-}"

die() {
    echo "错误:$*" >&2
    exit 1
}

usage() {
    cat <<'EOF'
用法:
  SPARKLE_PRIVATE_KEY='不要回显的私钥' \
  SPARKLE_DOWNLOAD_URL_PREFIX='https://github.com/domoxiaojun/sumpter/releases/download/v<version>/' \
  ./generate-appcast.sh /path/to/release-archives

archives 目录应放置 package-app.sh 生成的版本化 .zip，且可选放置同名 .html/.md/.txt 更新说明。
脚本会在该目录生成或更新 appcast.xml。未设置 SPARKLE_PRIVATE_KEY 时，Sparkle 会从本机默认钥匙串读取 ed25519 私钥；CI 环境再通过 Secret 注入该变量。
EOF
}

[[ -n "$ARCHIVES_DIR" ]] || { usage; exit 2; }
[[ -d "$ARCHIVES_DIR" ]] || die "archives 目录不存在:$ARCHIVES_DIR"
[[ -x "$GENERATE_APPCAST" ]] || die "找不到 Sparkle generate_appcast:$GENERATE_APPCAST"
[[ -n "$DOWNLOAD_URL_PREFIX" ]] || die "必须通过环境变量提供 SPARKLE_DOWNLOAD_URL_PREFIX"

args=(
    --download-url-prefix "$DOWNLOAD_URL_PREFIX"
)
if [[ -n "${SPARKLE_PRIVATE_KEY:-}" ]]; then
    args=(--ed-key-file - "${args[@]}")
fi
if [[ -n "${SPARKLE_RELEASE_LINK:-}" ]]; then
    args+=(--link "$SPARKLE_RELEASE_LINK")
fi
if [[ -n "${SPARKLE_MAXIMUM_VERSIONS:-}" ]]; then
    args+=(--maximum-versions "$SPARKLE_MAXIMUM_VERSIONS")
fi

# 本机开发优先使用钥匙串；CI 才通过 stdin 注入 Secret，避免私钥进入命令行参数。
if [[ -n "${SPARKLE_PRIVATE_KEY:-}" ]]; then
    printf '%s' "$SPARKLE_PRIVATE_KEY" | "$GENERATE_APPCAST" "${args[@]}" "$ARCHIVES_DIR"
else
    "$GENERATE_APPCAST" "${args[@]}" "$ARCHIVES_DIR"
fi
echo "已生成:$ARCHIVES_DIR/appcast.xml"
