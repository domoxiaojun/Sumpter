#!/usr/bin/env bash
# Build and verify a local Sumpter macOS DMG.
set -Eeuo pipefail
umask 022

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
PACKAGE_SCRIPT="$ROOT/platforms/macos/app/package-app.sh"

usage() {
  cat <<'EOF'
用法:
  ./scripts/build-macos-dmg.sh [选项]

默认行为:
  - 构建 arm64 Release 版本
  - 打包前运行 Rust workspace 与 Swift 测试
  - 使用 ad-hoc 签名，仅适合本机测试
  - 输出到 platforms/macos/app/dist/
  - 产物名为 Sumpter-local

选项:
  --clean                 清理本次 macOS Rust 目标与 Swift 缓存后再构建
  --skip-tests            跳过打包前测试
  --arch ARCH             目标架构: arm64 或 x86_64（默认 arm64）
  --artifact-name NAME   DMG/ZIP 基名（默认 Sumpter-local）
  --build-version VALUE  Bundle 版本（默认 1）
  -h, --help              显示帮助

也可通过环境变量覆盖:
  DEVELOPER_DIR, CODESIGN_IDENTITY, DIST_DIR, BUILD_VERSION,
  ARTIFACT_NAME, RUN_TESTS, CLEAN_BUILD, VERBOSE

正式分发请直接使用 platforms/macos/app/package-app.sh，并配置 Developer ID
签名、Sparkle 参数及 notarization 流程；本脚本不会上传或公证。
EOF
}

die() {
  echo "错误: $*" >&2
  exit 1
}

ARCH="${ARCH:-arm64}"
ARTIFACT_NAME="${ARTIFACT_NAME:-Sumpter-local}"
BUILD_VERSION="${BUILD_VERSION:-1}"
RUN_TESTS="${RUN_TESTS:-1}"
CLEAN_BUILD="${CLEAN_BUILD:-0}"
CONFIGURATION="${CONFIGURATION:-release}"
VERBOSE="${VERBOSE:-0}"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --clean)
      CLEAN_BUILD=1
      ;;
    --skip-tests)
      RUN_TESTS=0
      ;;
    --arch)
      [[ "$#" -ge 2 ]] || die "--arch 缺少值"
      ARCH="$2"
      shift
      ;;
    --artifact-name)
      [[ "$#" -ge 2 ]] || die "--artifact-name 缺少值"
      ARTIFACT_NAME="$2"
      shift
      ;;
    --build-version)
      [[ "$#" -ge 2 ]] || die "--build-version 缺少值"
      BUILD_VERSION="$2"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "不支持的参数: $1（使用 --help 查看用法）"
      ;;
  esac
  shift
done

[[ "$(uname -s)" == "Darwin" ]] || die "此脚本只能在 macOS（Darwin）上运行"
[[ -x "$PACKAGE_SCRIPT" ]] || die "找不到 macOS 打包脚本: $PACKAGE_SCRIPT"

case "$ARCH" in
  arm64|x86_64) ;;
  *) die "ARCH 只支持 arm64 或 x86_64: $ARCH" ;;
esac
case "$RUN_TESTS" in
  0|1) ;;
  *) die "RUN_TESTS 只能是 0 或 1: $RUN_TESTS" ;;
esac
case "$CLEAN_BUILD" in
  0|1) ;;
  *) die "CLEAN_BUILD 只能是 0 或 1: $CLEAN_BUILD" ;;
esac
case "$CONFIGURATION" in
  release|debug) ;;
  *) die "CONFIGURATION 只支持 release 或 debug: $CONFIGURATION" ;;
esac

DIST_DIR="${DIST_DIR:-$ROOT/platforms/macos/app/dist}"
if [[ "$DIST_DIR" != /* ]]; then
  DIST_DIR="$ROOT/$DIST_DIR"
fi

echo "开始构建 Sumpter macOS DMG"
echo "  架构: $ARCH"
echo "  配置: $CONFIGURATION"
echo "  测试: $([[ "$RUN_TESTS" == "1" ]] && echo 启用 || echo 跳过)"
echo "  输出: $DIST_DIR"

export ARCH ARTIFACT_NAME BUILD_VERSION RUN_TESTS CLEAN_BUILD CONFIGURATION DIST_DIR VERBOSE
"$PACKAGE_SCRIPT"

DMG_PATH="$DIST_DIR/$ARTIFACT_NAME.dmg"
APP_PATH="$DIST_DIR/Sumpter.app"
[[ -f "$DMG_PATH" ]] || die "DMG 产物缺失: $DMG_PATH"
[[ -d "$APP_PATH" ]] || die "App 产物缺失: $APP_PATH"

echo "本地 DMG 构建完成"
echo "  App: $APP_PATH"
echo "  DMG: $DMG_PATH"
echo "  SHA-256:"
shasum -a 256 "$DMG_PATH"
