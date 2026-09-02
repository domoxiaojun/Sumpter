#!/usr/bin/env bash
set -Eeuo pipefail
umask 022

usage() {
  cat <<'EOF'
用法:
  ./package-app.sh

通过环境变量调整本地构建：
  ARCH=arm64|x86_64             目标架构（默认 arm64）
  CONFIGURATION=release|debug   Swift/Cargo 构建配置（默认 release）
  ENGINE_PROFILE=legacy|unified 独立发布树兼容选项（monorepo 根 workspace 始终使用 shared engine）
  ARTIFACT_NAME=...             DMG/ZIP 基名（默认 unified 时带测试标记）
  DIST_DIR=...                  输出目录（unified 默认 dist-unified-engine-test）
  CLEAN_BUILD=1                 清理当前 Rust 目标与 Swift .build 后再构建
  RUN_TESTS=1                   打包前运行 Rust workspace 与 Swift 测试
  SHORT_VERSION=X.Y.Z           覆盖 macOS App 版本（也接受 VERSION）
  BUILD_VERSION=N               覆盖 Bundle 版本（也接受 BUILD_NUMBER）
  CODESIGN_IDENTITY=-           ad-hoc 签名；正式包设置 Developer ID 证书名称
  SPARKLE_FEED_URL=...          与 SPARKLE_PUBLIC_ED_KEY 一起启用 Sparkle 更新
  SPARKLE_PUBLIC_ED_KEY=...      Sparkle Ed25519 公钥
  VERBOSE=1                     打印 shell 执行轨迹

脚本只生成 macOS .app、DMG 和 Sparkle ZIP，不安装、不启动、不公证、不上传。
EOF
}

die() {
  echo "错误: $*" >&2
  exit 1
}

if [[ "$#" -gt 0 ]]; then
  case "$1" in
    -h|--help)
      [[ "$#" -eq 1 ]] \
        || die "--help 不接受其它位置参数"
      usage
      exit 0
      ;;
    *)
      die "不支持的位置参数: $1（使用 --help 查看用法）"
      ;;
  esac
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
if [[ -f "$ROOT/../../../Cargo.toml" && -d "$ROOT/../../../crates" ]]; then
  # Monorepo layout: platform inputs live under platforms/, while the Rust
  # workspace is the repository root.
  REPO_ROOT="$(cd "$ROOT/../../.." && pwd -P)"
  RUST_DIR="$REPO_ROOT"
  DEFAULT_VERSION_SOURCE="$REPO_ROOT/Cargo.toml"
  DEFAULT_CC_ATTRIBUTION_SCRIPT="$REPO_ROOT/platforms/macos/scripts/cc-project-attribution.sh"
  SHARED_WORKSPACE=1
  SIDECAR_BIN="sumpterd-macos"
else
  # Standalone release layout: macos/ remains the Rust workspace and scripts/
  # is provided by the Linux release root.
  REPO_ROOT="$(cd "$ROOT/../.." && pwd -P)"
  RUST_DIR="$ROOT/.."
  DEFAULT_VERSION_SOURCE="$RUST_DIR/Cargo.toml"
  DEFAULT_CC_ATTRIBUTION_SCRIPT="$REPO_ROOT/scripts/cc-project-attribution.sh"
  SHARED_WORKSPACE=0
  SIDECAR_BIN="sumpterd-macos"
fi
SIDECAR_NAME="${SIDECAR_NAME:-sumpterd}"
CONFIGURATION="${CONFIGURATION:-release}"
ARCH="${ARCH:-arm64}"
ENGINE_PROFILE="${ENGINE_PROFILE:-legacy}"
APP_NAME="${APP_NAME:-Sumpter}"
ARTIFACT_NAME="${ARTIFACT_NAME:-}"
BUNDLE_ID="${BUNDLE_ID:-org.kkl.sumpter}"
DIST_DIR="${DIST_DIR:-}"
VERSION_SOURCE="${VERSION_SOURCE:-$DEFAULT_VERSION_SOURCE}"
ICON_PATH="${ICON_PATH:-$ROOT/../icon.icns}"
CC_ATTRIBUTION_SCRIPT="${CC_ATTRIBUTION_SCRIPT:-}"
SHORT_VERSION="${SHORT_VERSION:-}"
BUILD_VERSION="${BUILD_VERSION:-${BUILD_NUMBER:-1}}"
SPARKLE_FEED_URL="${SPARKLE_FEED_URL:-}"
SPARKLE_PUBLIC_ED_KEY="${SPARKLE_PUBLIC_ED_KEY:-}"
CODESIGN_IDENTITY="${CODESIGN_IDENTITY:--}"
CLEAN_BUILD="${CLEAN_BUILD:-0}"
RUN_TESTS="${RUN_TESTS:-0}"
VERBOSE="${VERBOSE:-0}"

case "$APP_NAME" in
  ""|.|..|*/*|*$'\n'*|*$'\r'*)
    die "APP_NAME 必须是单个不含路径分隔符或换行的名称"
    ;;
esac
[[ "$BUNDLE_ID" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]] \
  || die "BUNDLE_ID 格式无效: $BUNDLE_ID"
[[ "$BUILD_VERSION" =~ ^[0-9]+([.][0-9]+)*$ ]] \
  || die "BUILD_VERSION 必须是数字或点分数字: $BUILD_VERSION"
case "$CONFIGURATION" in
  release)
    RUST_PROFILE="release"
    CARGO_PROFILE_ARGS=(--release)
    ;;
  debug)
    RUST_PROFILE="debug"
    CARGO_PROFILE_ARGS=()
    ;;
  *)
    die "CONFIGURATION 只支持 release 或 debug: $CONFIGURATION"
    ;;
esac
case "$ARCH" in
  arm64)
    RUST_TARGET="aarch64-apple-darwin"
    ;;
  x86_64)
    RUST_TARGET="x86_64-apple-darwin"
    ;;
  *)
    die "ARCH 只支持 arm64 或 x86_64: $ARCH"
    ;;
esac
case "$ENGINE_PROFILE" in
  legacy)
    [[ -n "$ARTIFACT_NAME" ]] || ARTIFACT_NAME="$APP_NAME"
    ;;
  unified)
    [[ -n "$ARTIFACT_NAME" ]] || ARTIFACT_NAME="${APP_NAME}-unified-engine-test"
    ;;
  *)
    die "ENGINE_PROFILE 只支持 legacy 或 unified: $ENGINE_PROFILE"
    ;;
esac
if [[ -z "$DIST_DIR" ]]; then
  if [[ "$ENGINE_PROFILE" == "unified" && "$SHARED_WORKSPACE" == "0" ]]; then
    DIST_DIR="$ROOT/dist-unified-engine-test"
  else
    DIST_DIR="$ROOT/dist"
  fi
fi
case "$ARTIFACT_NAME" in
  ""|.|..|*/*|*$'\n'*|*$'\r'*)
    die "ARTIFACT_NAME 必须是单个不含路径分隔符或换行的名称"
    ;;
esac
case "$CLEAN_BUILD" in 0|1) ;; *) die "CLEAN_BUILD 只能是 0 或 1" ;; esac
case "$RUN_TESTS" in 0|1) ;; *) die "RUN_TESTS 只能是 0 或 1" ;; esac
case "$VERBOSE" in 0|1) ;; *) die "VERBOSE 只能是 0 或 1" ;; esac

if [[ -z "$SHORT_VERSION" && -n "${VERSION:-}" ]]; then
  SHORT_VERSION="$VERSION"
fi

if [[ "$DIST_DIR" != /* ]]; then DIST_DIR="$ROOT/$DIST_DIR"; fi
if [[ "$VERSION_SOURCE" != /* ]]; then VERSION_SOURCE="$ROOT/$VERSION_SOURCE"; fi
if [[ "$ICON_PATH" != /* ]]; then ICON_PATH="$ROOT/$ICON_PATH"; fi
if [[ -z "$CC_ATTRIBUTION_SCRIPT" ]]; then
  CC_ATTRIBUTION_SCRIPT="$DEFAULT_CC_ATTRIBUTION_SCRIPT"
elif [[ "$CC_ATTRIBUTION_SCRIPT" != /* ]]; then
  CC_ATTRIBUTION_SCRIPT="$ROOT/$CC_ATTRIBUTION_SCRIPT"
fi
[[ "$DIST_DIR" != "/" && "$DIST_DIR" != "$ROOT" ]] \
  || die "DIST_DIR 不能是文件系统根或源码根目录: $DIST_DIR"

if [[ -z "$SHORT_VERSION" ]]; then
  [[ -f "$VERSION_SOURCE" ]] || die "版本源不存在: $VERSION_SOURCE"
  SHORT_VERSION="$(awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && $1 == "version" && $2 == "=" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  ' "$VERSION_SOURCE")"
fi
[[ "$SHORT_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || die "版本号必须是 X.Y.Z: $SHORT_VERSION"

if [[ -n "$SPARKLE_FEED_URL" || -n "$SPARKLE_PUBLIC_ED_KEY" ]]; then
  [[ -n "$SPARKLE_FEED_URL" && -n "$SPARKLE_PUBLIC_ED_KEY" ]] \
    || die "SPARKLE_FEED_URL 与 SPARKLE_PUBLIC_ED_KEY 必须同时设置"
  [[ "$SPARKLE_FEED_URL" == https://* ]] \
    || die "SPARKLE_FEED_URL 必须使用 HTTPS"
  [[ "$SPARKLE_PUBLIC_ED_KEY" =~ ^[A-Za-z0-9+/_=-]+$ ]] \
    || die "SPARKLE_PUBLIC_ED_KEY 含有非法字符或换行"
fi

APP_DIR="$DIST_DIR/$APP_NAME.app"
DMG_PATH="$DIST_DIR/$ARTIFACT_NAME.dmg"
ZIP_PATH="$DIST_DIR/$ARTIFACT_NAME-$SHORT_VERSION-macos.zip"
INSTALL_COMMAND="$ROOT/install.command"
INSTALL_README="$ROOT/INSTALL.txt"

DEVELOPER_DIR_SELECTED=""
SDKROOT=""
SWIFT_BIN=""
SWIFTC_BIN=""
SWIFT_VERSION_OUTPUT=""
SWIFT_MAJOR=0
CARGO_BIN=""
RUSTC_BIN=""
RUSTUP_BIN=""
RUST_TARGET_DIR=""
BUILD_DIR=""
SIDECAR_BIN_PATH=""
STAGING_DIR=""
DMG_TMP_PATH=""
DMG_MOUNT_DIR=""
DMG_MOUNTED=0
DISKUTIL_IMAGE_SUPPORTED=0
DISKUTIL_BIN=""
HDIUTIL_BIN=""
DMG_SOURCE_DIR=""
LOCK_DIR="$DIST_DIR/.package-app.lock"
LOCK_HELD=0
COMMIT_STARTED=0
COMMIT_COMPLETE=0
BACKUP_DIR=""
TARGET_PATHS=()
STAGED_PATHS=()
BACKUP_PATHS=()
MOVED_TARGETS=()

if [[ "$VERBOSE" == "1" ]]; then
  set -x
fi

on_error() {
  local status=$?
  local line="${BASH_LINENO[0]:-?}"
  local command="${BASH_COMMAND:-unknown}"
  trap - ERR
  echo "错误: macOS 构建在第 ${line} 行失败（退出码 ${status}）: ${command}" >&2
  exit "$status"
}
trap on_error ERR

rollback_artifacts() {
  local i target backup
  i=$(( ${#TARGET_PATHS[@]} - 1 ))
  while (( i >= 0 )); do
    target="${TARGET_PATHS[$i]}"
    if [[ "${MOVED_TARGETS[$i]:-0}" == "1" ]]; then
      if [[ -e "$target" || -L "$target" ]]; then
        rm -rf "$target"
      fi
    fi
    backup="${BACKUP_PATHS[$i]:-}"
    if [[ -n "$backup" && ( -e "$backup" || -L "$backup" ) ]]; then
      mv "$backup" "$target"
    fi
    i=$(( i - 1 ))
  done
}

detach_dmg_mount() {
  [[ "${DMG_MOUNTED:-0}" == "1" && -n "${DMG_MOUNT_DIR:-}" ]] || return 0
  if [[ "${DISKUTIL_IMAGE_SUPPORTED:-0}" == "1" ]]; then
    "${DISKUTIL_BIN:?}" eject force "$DMG_MOUNT_DIR" >/dev/null 2>&1
  else
    "${HDIUTIL_BIN:?}" detach -quiet "$DMG_MOUNT_DIR" -force >/dev/null 2>&1
  fi
}

cleanup() {
  local status=$?
  trap - ERR
  set +e
  if [[ "$COMMIT_STARTED" == "1" && "$COMMIT_COMPLETE" != "1" ]]; then
    rollback_artifacts
  fi
  if [[ "$DMG_MOUNTED" == "1" && -n "$DMG_MOUNT_DIR" ]]; then
    detach_dmg_mount || true
  fi
  if [[ -n "$STAGING_DIR" && -d "$STAGING_DIR" ]]; then
    rm -rf "$STAGING_DIR"
  fi
  if [[ "$LOCK_HELD" == "1" && -f "$LOCK_DIR/pid" ]]; then
    if [[ "$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null)" == "$$" ]]; then
      rm -f "$LOCK_DIR/pid"
      rmdir "$LOCK_DIR" 2>/dev/null || true
    fi
  fi
  return "$status"
}
trap cleanup EXIT

acquire_lock() {
  mkdir -p "$DIST_DIR"
  if ! mkdir "$LOCK_DIR" 2>/dev/null; then
    local existing_pid=""
    if [[ -f "$LOCK_DIR/pid" ]]; then
      existing_pid="$(sed -n '1p' "$LOCK_DIR/pid" 2>/dev/null || true)"
    fi
    if [[ "$existing_pid" =~ ^[0-9]+$ ]] && kill -0 "$existing_pid" 2>/dev/null; then
      die "已有另一个 macOS 打包进程在运行（PID $existing_pid）"
    fi
    if [[ -f "$LOCK_DIR/pid" ]]; then
      rm -f "$LOCK_DIR/pid"
    fi
    rmdir "$LOCK_DIR" 2>/dev/null \
      || die "发现无法确认归属的构建锁: ${LOCK_DIR}；请确认没有构建进程后手动移除该目录"
    mkdir "$LOCK_DIR" \
      || die "无法创建构建锁: $LOCK_DIR"
  fi
  LOCK_HELD=1
  printf '%s\n' "$$" > "$LOCK_DIR/pid"
}

normalize_developer_dir() {
  local candidate="$1"
  candidate="${candidate%/}"
  case "$candidate" in
    *.app) candidate="$candidate/Contents/Developer" ;;
  esac
  printf '%s' "$candidate"
}

swift_binary_for() {
  printf '%s/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift' "$1"
}

swiftc_binary_for() {
  printf '%s/Toolchains/XcodeDefault.xctoolchain/usr/bin/swiftc' "$1"
}

xcode_candidate_valid() {
  local candidate="$1"
  [[ -d "$candidate" \
    && -x "$(swift_binary_for "$candidate")" \
    && -x "$(swiftc_binary_for "$candidate")" ]]
}

swift_major_for() {
  local candidate="$1"
  local output
  output="$(DEVELOPER_DIR="$candidate" "$(swift_binary_for "$candidate")" --version 2>&1 || true)"
  printf '%s\n' "$output" \
    | awk 'match($0, /Swift version [0-9][0-9]*/) && value == "" { value = substr($0, RSTART + 14, RLENGTH - 14) } END { print value }'
}

ensure_xcode_toolchain() {
  local requested="${DEVELOPER_DIR:-}"
  local active=""
  local candidate=""
  local selected=""
  local selected_major=0
  local major=0
  local sdk=""
  local sdk_root=""
  local sdk_candidate=""
  local app=""
  local candidates=()

  if [[ -n "$requested" ]]; then
    requested="$(normalize_developer_dir "$requested")"
    [[ "$requested" != *CommandLineTools* ]] \
      || die "DEVELOPER_DIR 指向 Command Line Tools；SwiftUI 宏需要完整 Xcode"
    xcode_candidate_valid "$requested" \
      || die "DEVELOPER_DIR 不是可用的完整 Xcode: $requested"
    major="$(swift_major_for "$requested")"
    if [[ ! "$major" =~ ^[0-9]+$ ]] || (( major < 6 )); then
      die "当前 Xcode 的 Swift 版本低于 6.0，Package.swift 无法编译: $requested"
    fi
    selected="$requested"
    selected_major="$major"
  else
    active="$(xcode-select -p 2>/dev/null || true)"
    if [[ -n "$active" ]]; then
      active="$(normalize_developer_dir "$active")"
      if [[ "$active" != *CommandLineTools* ]] && xcode_candidate_valid "$active"; then
        candidates+=("$active")
      fi
    fi
    for app in /Applications/Xcode.app /Applications/Xcode-beta.app /Applications/Xcode*.app; do
      candidate="$(normalize_developer_dir "$app")"
      if xcode_candidate_valid "$candidate"; then
        candidates+=("$candidate")
      fi
    done
    for candidate in "${candidates[@]}"; do
      major="$(swift_major_for "$candidate")"
      if [[ "$major" =~ ^[0-9]+$ ]] && (( major > selected_major )); then
        selected="$candidate"
        selected_major="$major"
      fi
    done
    [[ -n "$selected" && "$selected_major" -ge 6 ]] \
      || die "找不到带 Swift 6.0+ 的完整 Xcode；当前 Package.swift 的 swift-tools-version 是 6.0"
  fi

  DEVELOPER_DIR_SELECTED="$selected"
  export DEVELOPER_DIR="$selected"
  SWIFT_BIN="$(swift_binary_for "$selected")"
  SWIFTC_BIN="$(swiftc_binary_for "$selected")"
  export PATH="$selected/Toolchains/XcodeDefault.xctoolchain/usr/bin:$selected/usr/bin:$PATH"

  XCRUN_BIN="$(command -v xcrun 2>/dev/null || true)"
  [[ -n "$XCRUN_BIN" ]] || die "找不到 xcrun"
  sdk="$("$XCRUN_BIN" --sdk macosx --show-sdk-path 2>/dev/null || true)"
  if [[ -z "$sdk" || "$sdk" == *CommandLineTools* || ! -d "$sdk" ]]; then
    # BSD find（macOS 自带版本）没有 GNU 的 -maxdepth；用有序 glob
    # 选 SDK，确保在 xcrun 异常时仍能从完整 Xcode 回退。
    sdk_root="$selected/Platforms/MacOSX.platform/Developer/SDKs"
    for sdk_candidate in "$sdk_root"/MacOSX*.sdk; do
      if [[ -d "$sdk_candidate" ]]; then
        sdk="$sdk_candidate"
      fi
    done
  fi
  [[ -n "$sdk" && -d "$sdk" && "$sdk" != *CommandLineTools* ]] \
    || die "找不到完整 Xcode 的 macOS SDK"
  SDKROOT="$sdk"
  export SDKROOT
  SWIFT_VERSION_OUTPUT="$("$SWIFT_BIN" --version 2>&1)"
  SWIFT_MAJOR="$(printf '%s\n' "$SWIFT_VERSION_OUTPUT" \
    | awk 'match($0, /Swift version [0-9][0-9]*/) && value == "" { value = substr($0, RSTART + 14, RLENGTH - 14) } END { print value }')"
  if [[ ! "$SWIFT_MAJOR" =~ ^[0-9]+$ ]] || (( SWIFT_MAJOR < 6 )); then
    die "Swift 版本解析失败或低于 6.0: $SWIFT_VERSION_OUTPUT"
  fi
  echo "使用 Xcode: $DEVELOPER_DIR (Swift ${SWIFT_MAJOR}.x)"
  echo "使用 SDK: $SDKROOT"
}

required_tool() {
  local name="$1"
  local path
  path="$(command -v "$name" 2>/dev/null || true)"
  [[ -n "$path" ]] || die "未找到必需工具: $name"
  printf '%s' "$path"
}

prepare_swift_cache() {
  local stamp_path="$ROOT/.build/.sumpter-build-environment"
  local current_stamp=""
  local previous_stamp=""
  local pcm_probe=""
  [[ -d "$ROOT/.build" ]] || return 0
  current_stamp="$(printf 'developer=%s\nsdk=%s\nswift=%s\narch=%s\nconfiguration=%s\n' \
    "$DEVELOPER_DIR_SELECTED" "$SDKROOT" "$SWIFT_VERSION_OUTPUT" "$ARCH" "$CONFIGURATION")"
  if [[ -f "$stamp_path" ]]; then
    previous_stamp="$(cat "$stamp_path")"
    if [[ "$previous_stamp" != "$current_stamp" ]]; then
      echo "Swift 工具链或目标变化，清理过期 .build 缓存..."
      rm -rf "$ROOT/.build"
    fi
    return 0
  fi
  # 旧脚本没有环境标记。只有确认存在 PCM 且没有当前源码路径时才清理，
  # 避免每次普通增量构建都被迫重新抓取依赖。
  pcm_probe="$(find "$ROOT/.build" -type f -name '*.pcm' -print -quit 2>/dev/null)"
  if [[ -n "$pcm_probe" ]] && ! grep -R -a -F -q "$ROOT" "$ROOT/.build" 2>/dev/null; then
    echo "检测到搬迁后的 Swift 模块缓存，清理 .build..."
    rm -rf "$ROOT/.build"
  fi
}

create_info_plist() {
  local plist="$1"
  cat > "$plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>zh-Hans</string>
  <key>CFBundleExecutable</key>
  <string></string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundleIconName</key>
  <string>AppIcon</string>
  <key>CFBundleIdentifier</key>
  <string></string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string></string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string></string>
  <key>CFBundleVersion</key>
  <string></string>
  <key>LSMinimumSystemVersion</key>
  <string>14.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>SumpterEngineProfile</key>
  <string></string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright © 2026 kkl</string>
  <key>NSUserNotificationAlertStyle</key>
  <string>alert</string>
</dict>
</plist>
PLIST
  "$PLUTIL_BIN" -replace CFBundleExecutable -string "$APP_NAME" "$plist"
  "$PLUTIL_BIN" -replace CFBundleIdentifier -string "$BUNDLE_ID" "$plist"
  "$PLUTIL_BIN" -replace CFBundleName -string "$APP_NAME" "$plist"
  "$PLUTIL_BIN" -replace CFBundleShortVersionString -string "$SHORT_VERSION" "$plist"
  "$PLUTIL_BIN" -replace CFBundleVersion -string "$BUILD_VERSION" "$plist"
  "$PLUTIL_BIN" -replace SumpterEngineProfile -string "$ENGINE_PROFILE" "$plist"
  if [[ -n "$SPARKLE_FEED_URL" ]]; then
    "$PLUTIL_BIN" -insert SUFeedURL -string "$SPARKLE_FEED_URL" "$plist"
    "$PLUTIL_BIN" -insert SUPublicEDKey -string "$SPARKLE_PUBLIC_ED_KEY" "$plist"
    "$PLUTIL_BIN" -insert SUEnableAutomaticChecks -bool true "$plist"
    "$PLUTIL_BIN" -insert SUScheduledCheckInterval -integer 86400 "$plist"
  fi
  "$PLUTIL_BIN" -lint "$plist" >/dev/null
}

contains_arch() {
  local wanted="$1"
  local actual="$2"
  case " $actual " in
    *" $wanted "*) return 0 ;;
    *) return 1 ;;
  esac
}

verify_macho_arch() {
  local path="$1"
  local actual
  actual="$("$LIPO_BIN" -archs "$path" 2>/dev/null || true)"
  contains_arch "$ARCH" "$actual" \
    || die "架构不匹配: ${path}（期望 ${ARCH}，实际 ${actual:-未知}）"
}

verify_app() {
  local app="$1"
  local info="$app/Contents/Info.plist"
  local app_bin="$app/Contents/MacOS/$APP_NAME"
  local sidecar="$app/Contents/MacOS/$SIDECAR_NAME"
  local sparkle_bin="$app/Contents/Frameworks/Sparkle.framework/Versions/Current/Sparkle"
  local executable_name=""
  local bundle_id=""
  local short_version=""
  local build_version=""
  local engine_profile=""
  [[ -x "$app_bin" ]] || die "App 主程序缺失或不可执行: $app_bin"
  [[ -x "$sidecar" ]] || die "Sumpter sidecar 缺失或不可执行: $sidecar"
  [[ -f "$info" ]] || die "Info.plist 缺失: $info"
  [[ -f "$app/Contents/Resources/AppIcon.icns" ]] || die "App 图标缺失"
  [[ -f "$app/Contents/Resources/cc-project-attribution.sh" ]] \
    || die "归因配置器未打进 App"
  [[ -f "$sparkle_bin" ]] || die "Sparkle 主二进制缺失: $sparkle_bin"
  "$PLUTIL_BIN" -lint "$info" >/dev/null
  executable_name="$("$PLUTIL_BIN" -extract CFBundleExecutable raw -o - "$info")"
  bundle_id="$("$PLUTIL_BIN" -extract CFBundleIdentifier raw -o - "$info")"
  short_version="$("$PLUTIL_BIN" -extract CFBundleShortVersionString raw -o - "$info")"
  build_version="$("$PLUTIL_BIN" -extract CFBundleVersion raw -o - "$info")"
  engine_profile="$("$PLUTIL_BIN" -extract SumpterEngineProfile raw -o - "$info")"
  [[ "$executable_name" == "$APP_NAME" ]] \
    || die "App 可执行文件字段不匹配: $executable_name"
  [[ "$bundle_id" == "$BUNDLE_ID" ]] || die "Bundle ID 不匹配: $bundle_id"
  [[ "$short_version" == "$SHORT_VERSION" ]] || die "App 版本不匹配: $short_version"
  [[ "$build_version" == "$BUILD_VERSION" ]] || die "Bundle 版本不匹配: $build_version"
  [[ "$engine_profile" == "$ENGINE_PROFILE" ]] || die "引擎 profile 不匹配: $engine_profile"
  verify_macho_arch "$app_bin"
  verify_macho_arch "$sidecar"
  verify_macho_arch "$sparkle_bin"
  "$CODESIGN_BIN" --verify --deep --strict "$app" >/dev/null
}

commit_artifacts() {
  local i target staged backup
  COMMIT_STARTED=1
  BACKUP_DIR="$STAGING_DIR/previous"
  mkdir -p "$BACKUP_DIR"
  for i in 0 1 2; do
    target="${TARGET_PATHS[$i]}"
    staged="${STAGED_PATHS[$i]}"
    MOVED_TARGETS[i]=0
    BACKUP_PATHS[i]=""
    if [[ -e "$target" || -L "$target" ]]; then
      backup="$BACKUP_DIR/$i-$(basename "$target")"
      mv "$target" "$backup"
      BACKUP_PATHS[i]="$backup"
    fi
    mv "$staged" "$target"
    MOVED_TARGETS[i]=1
  done
  COMMIT_COMPLETE=1
}

[[ "$(uname -s)" == "Darwin" ]] || die "此脚本只能在 macOS（Darwin）上运行"
cd "$ROOT"
[[ -f "$ICON_PATH" ]] || die "未找到图标文件: $ICON_PATH"
[[ -f "$INSTALL_COMMAND" && -f "$INSTALL_README" ]] \
  || die "DMG 安装说明或安装脚本缺失"
[[ -f "$CC_ATTRIBUTION_SCRIPT" ]] \
  || die "归因配置器缺失: $CC_ATTRIBUTION_SCRIPT"
[[ -f "$RUST_DIR/Cargo.toml" ]] || die "macOS Rust workspace 缺失: $RUST_DIR/Cargo.toml"
[[ -f "$ROOT/Package.swift" ]] || die "Swift Package.swift 缺失: $ROOT/Package.swift"

ensure_xcode_toolchain
DISKUTIL_BIN="$(required_tool diskutil)"
HDIUTIL_BIN="$(command -v hdiutil 2>/dev/null || true)"
DITTO_BIN="$(required_tool ditto)"
INSTALL_NAME_TOOL_BIN="$(required_tool install_name_tool)"
CODESIGN_BIN="$(required_tool codesign)"
PLUTIL_BIN="$(required_tool plutil)"
LIPO_BIN="$(required_tool lipo)"
FILE_BIN="$(required_tool file)"
SHASUM_BIN="$(required_tool shasum)"
UNZIP_BIN="$(required_tool unzip)"
OTOOL_BIN="$(required_tool otool)"
SETFILE_BIN="$(command -v SetFile 2>/dev/null || true)"
BLESS_BIN="$(command -v bless 2>/dev/null || true)"
CHFLAGS_BIN="$(command -v chflags 2>/dev/null || true)"

# 较新的 macOS 提供 diskutil image create/attach/info；旧版 macOS 仍只有
# hdiutil，因此保留兼容分支，避免把最低支持系统的打包链切断。
if "$DISKUTIL_BIN" image create from --help >/dev/null 2>&1 \
  && "$DISKUTIL_BIN" image attach --help >/dev/null 2>&1; then
  DISKUTIL_IMAGE_SUPPORTED=1
  echo "DMG 工具: diskutil image"
else
  echo "提示: 当前系统的 diskutil 不支持 image create，使用 hdiutil 兼容路径" >&2
  [[ -n "$HDIUTIL_BIN" ]] || die "当前系统缺少 hdiutil，无法创建 DMG"
fi

# Homebrew rustup 是 keg-only；同时兼容 Intel/Apple Silicon 与标准 rustup 安装位置。
for rust_bin_dir in /opt/homebrew/opt/rustup/bin /usr/local/opt/rustup/bin "${HOME:-}/.cargo/bin"; do
  if [[ -d "$rust_bin_dir" ]]; then
    PATH="$rust_bin_dir:$PATH"
  fi
done
export PATH
CARGO_BIN="$(required_tool cargo)"
RUSTC_BIN="$(required_tool rustc)"
RUSTUP_BIN="$(command -v rustup 2>/dev/null || true)"
RUST_HOST="$("$RUSTC_BIN" -vV | awk '/^host: / && value == "" { value = substr($0, 7) } END { print value }')"
if [[ "$RUST_HOST" != "$RUST_TARGET" ]]; then
  [[ -n "$RUSTUP_BIN" ]] \
    || die "当前 Rust host 是 ${RUST_HOST}，缺少交叉编译目标 ${RUST_TARGET} 和 rustup"
  # 不使用 grep -q：在 pipefail 下，grep 提前退出会让 rustup 收到
  # SIGPIPE，把“目标已安装”误判成失败。
  if ! "$RUSTUP_BIN" target list --installed \
    | grep -E "^${RUST_TARGET}([[:space:]]|$)" >/dev/null; then
    die "Rust 未安装目标 ${RUST_TARGET}；请先执行 rustup target add ${RUST_TARGET}"
  fi
fi

acquire_lock

# Cargo 的 target 目录可能由 workspace 配置或 CARGO_TARGET_DIR 改写，不能
# 根据脚本位置猜路径。先读取真实目录，再按目标三元组精确清理，避免误删
# 另一架构的缓存或 workspace 外的目录。
CARGO_METADATA="$("$CARGO_BIN" metadata \
  --manifest-path "$RUST_DIR/Cargo.toml" --no-deps --locked --format-version 1)"
RUST_TARGET_DIR="$(printf '%s\n' "$CARGO_METADATA" \
  | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')"
[[ -n "$RUST_TARGET_DIR" && "$RUST_TARGET_DIR" == /* && "$RUST_TARGET_DIR" != "/" ]] \
  || die "无法从 cargo metadata 解析安全的 target 目录"

if [[ "$CLEAN_BUILD" == "1" ]]; then
  echo "清理 macOS Rust 目标（${RUST_TARGET}）与 Swift .build..."
  RUST_TARGET_BUILD_DIR="$RUST_TARGET_DIR/$RUST_TARGET"
  if [[ -d "$RUST_TARGET_BUILD_DIR" ]]; then
    rm -rf -- "$RUST_TARGET_BUILD_DIR"
  fi
  if [[ -d "$ROOT/.build" ]]; then rm -rf -- "$ROOT/.build"; fi
fi
prepare_swift_cache

if [[ "$RUN_TESTS" == "1" ]]; then
  echo "运行 Rust workspace 测试..."
  rust_test_args=(test --manifest-path "$RUST_DIR/Cargo.toml" --workspace --locked)
  if [[ "$ENGINE_PROFILE" == "unified" ]]; then
    rust_test_args+=(--features unified-engine)
  fi
  "$CARGO_BIN" "${rust_test_args[@]}"
  echo "运行 Swift 测试..."
  "$SWIFT_BIN" test --package-path "$ROOT"
fi

# sidecar 依赖同一 workspace 的 shared crates；Cargo 会在
# 依赖源码变化时一并重编译它们，并把最终链接结果放进 sidecar。
echo "构建 macOS Rust workspace（${SIDECAR_BIN}，${RUST_TARGET}, ${CONFIGURATION}, engine=${ENGINE_PROFILE}）..."
cargo_build_args=(
  build
  --manifest-path "$RUST_DIR/Cargo.toml"
  --locked
  --target "$RUST_TARGET"
  --bin "$SIDECAR_BIN"
)
cargo_build_args+=("${CARGO_PROFILE_ARGS[@]}")
if [[ "$ENGINE_PROFILE" == "unified" && "$SHARED_WORKSPACE" == "0" ]]; then
  cargo_build_args+=(--features unified-engine)
fi
"$CARGO_BIN" "${cargo_build_args[@]}"
SIDECAR_BIN_PATH="$RUST_TARGET_DIR/$RUST_TARGET/$RUST_PROFILE/$SIDECAR_BIN"
[[ -x "$SIDECAR_BIN_PATH" ]] || die "$SIDECAR_BIN 构建产物缺失: $SIDECAR_BIN_PATH"

echo "构建 Swift App ($ARCH, $CONFIGURATION)..."
swift_build_args=(
  build
  --package-path "$ROOT"
  -c "$CONFIGURATION"
  --arch "$ARCH"
  --product SumpterApp
)
"$SWIFT_BIN" "${swift_build_args[@]}"
BUILD_DIR="$("$SWIFT_BIN" "${swift_build_args[@]}" --show-bin-path)"
SWIFT_APP_BIN="$BUILD_DIR/SumpterApp"
SPARKLE_FRAMEWORK="$BUILD_DIR/Sparkle.framework"
[[ -x "$SWIFT_APP_BIN" ]] || die "Swift App 构建产物缺失: $SWIFT_APP_BIN"
[[ -d "$SPARKLE_FRAMEWORK" ]] || die "Sparkle.framework 构建产物缺失: $SPARKLE_FRAMEWORK"

if [[ -d "$ROOT/.build" ]]; then
  printf 'developer=%s\nsdk=%s\nswift=%s\narch=%s\nconfiguration=%s\n' \
    "$DEVELOPER_DIR_SELECTED" "$SDKROOT" "$SWIFT_VERSION_OUTPUT" "$ARCH" "$CONFIGURATION" \
    > "$ROOT/.build/.sumpter-build-environment"
fi

STAGING_DIR="$(mktemp -d "$DIST_DIR/.sumpter-package.XXXXXX")"
STAGED_APP="$STAGING_DIR/$APP_NAME.app"
STAGED_DMG="$STAGING_DIR/$ARTIFACT_NAME.dmg"
STAGED_ZIP="$STAGING_DIR/$ARTIFACT_NAME-$SHORT_VERSION-macos.zip"
# `diskutil image create from` derives the volume name from the source
# directory and does not accept a `--volumeName` option on current macOS.
# Keep the source directory basename equal to the product name so the mounted
# image remains user-facing, while the legacy hdiutil path remains unchanged.
DMG_SOURCE_DIR="$STAGING_DIR/$ARTIFACT_NAME"
DMG_MOUNT_DIR="$STAGING_DIR/dmg-mount"
TARGET_PATHS=("$APP_DIR" "$DMG_PATH" "$ZIP_PATH")
STAGED_PATHS=("$STAGED_APP" "$STAGED_DMG" "$STAGED_ZIP")

STAGED_CONTENTS="$STAGED_APP/Contents"
STAGED_MACOS="$STAGED_CONTENTS/MacOS"
STAGED_FRAMEWORKS="$STAGED_CONTENTS/Frameworks"
STAGED_RESOURCES="$STAGED_CONTENTS/Resources"
mkdir -p "$STAGED_MACOS" "$STAGED_FRAMEWORKS" "$STAGED_RESOURCES"
"$DITTO_BIN" "$SWIFT_APP_BIN" "$STAGED_MACOS/$APP_NAME"
"$DITTO_BIN" "$SIDECAR_BIN_PATH" "$STAGED_MACOS/$SIDECAR_NAME"
chmod 0755 "$STAGED_MACOS/$APP_NAME" "$STAGED_MACOS/$SIDECAR_NAME"
"$DITTO_BIN" "$SPARKLE_FRAMEWORK" "$STAGED_FRAMEWORKS/Sparkle.framework"

app_links="$("$OTOOL_BIN" -L "$STAGED_MACOS/$APP_NAME")"
if [[ "$app_links" == *"Sparkle.framework"* ]]; then
  app_load_commands="$("$OTOOL_BIN" -l "$STAGED_MACOS/$APP_NAME")"
  if [[ "$app_load_commands" != *"@executable_path/../Frameworks"* ]]; then
    "$INSTALL_NAME_TOOL_BIN" -add_rpath \
      "@executable_path/../Frameworks" "$STAGED_MACOS/$APP_NAME"
  fi
else
  die "App 未链接 Sparkle.framework"
fi

"$DITTO_BIN" "$ICON_PATH" "$STAGED_RESOURCES/AppIcon.icns"
"$DITTO_BIN" "$CC_ATTRIBUTION_SCRIPT" "$STAGED_RESOURCES/cc-project-attribution.sh"
chmod 0755 "$STAGED_RESOURCES/cc-project-attribution.sh"
create_info_plist "$STAGED_CONTENTS/Info.plist"

if [[ "$CODESIGN_IDENTITY" == "-" ]]; then
  "$CODESIGN_BIN" --force --deep --sign - "$STAGED_APP"
else
  "$CODESIGN_BIN" --force --deep --options runtime --timestamp \
    --sign "$CODESIGN_IDENTITY" "$STAGED_APP"
fi
verify_app "$STAGED_APP"
echo "App 架构: $("$FILE_BIN" "$STAGED_MACOS/$APP_NAME")"
echo "sidecar 架构: $("$FILE_BIN" "$STAGED_MACOS/$SIDECAR_NAME")"

mkdir -p "$DMG_SOURCE_DIR"
"$DITTO_BIN" "$STAGED_APP" "$DMG_SOURCE_DIR/$APP_NAME.app"
ln -s /Applications "$DMG_SOURCE_DIR/Applications"
"$DITTO_BIN" "$INSTALL_COMMAND" "$DMG_SOURCE_DIR/安装 Sumpter.command"
"$DITTO_BIN" "$INSTALL_README" "$DMG_SOURCE_DIR/安装说明.txt"
chmod 0755 "$DMG_SOURCE_DIR/安装 Sumpter.command"
"$DITTO_BIN" "$ICON_PATH" "$DMG_SOURCE_DIR/.VolumeIcon.icns"
if [[ -n "$SETFILE_BIN" ]]; then
  "$SETFILE_BIN" -a C "$DMG_SOURCE_DIR"
  "$SETFILE_BIN" -a V "$DMG_SOURCE_DIR/.VolumeIcon.icns"
else
  echo "提示: 未找到 SetFile，跳过 DMG Finder 自定义图标（不影响内容）" >&2
fi
if [[ -n "$CHFLAGS_BIN" ]]; then
  "$CHFLAGS_BIN" hidden "$DMG_SOURCE_DIR/.VolumeIcon.icns" 2>/dev/null || true
fi

if [[ "$DISKUTIL_IMAGE_SUPPORTED" == "1" ]]; then
  # 直接从已组装的目录生成压缩镜像，避免 macOS 27 对 hdiutil 的
  # create/attach/convert/detach 弃用警告；diskutil 会在创建过程中
  # 临时挂载并在命令结束时卸载。
  "$DISKUTIL_BIN" image create from --format UDZO \
    "$DMG_SOURCE_DIR" "$STAGED_DMG" >/dev/null
else
  # macOS 14/15 兼容路径：保留 HFS+ DMG 和 Finder 展示属性。
  DMG_TMP_PATH="$STAGING_DIR/$APP_NAME-rw.dmg"
  mkdir -p "$DMG_MOUNT_DIR"
  APP_SIZE_KB="$(du -sk "$STAGED_APP" | awk '{print $1}')"
  DMG_SIZE_KB=$(( APP_SIZE_KB + APP_SIZE_KB / 4 + 32768 ))
  if (( DMG_SIZE_KB < 65536 )); then DMG_SIZE_KB=65536; fi
  "$HDIUTIL_BIN" create -quiet -size "${DMG_SIZE_KB}k" -fs HFS+ \
    -volname "$APP_NAME" "$DMG_TMP_PATH" >/dev/null
  DMG_MOUNTED=1
  "$HDIUTIL_BIN" attach -quiet -readwrite -noverify -noautoopen \
    -mountpoint "$DMG_MOUNT_DIR" "$DMG_TMP_PATH" >/dev/null
  "$DITTO_BIN" "$DMG_SOURCE_DIR/." "$DMG_MOUNT_DIR"
  if [[ -n "$SETFILE_BIN" ]]; then
    "$SETFILE_BIN" -a C "$DMG_MOUNT_DIR"
    "$SETFILE_BIN" -a V "$DMG_MOUNT_DIR/.VolumeIcon.icns"
  fi
  if [[ -n "$CHFLAGS_BIN" ]]; then
    "$CHFLAGS_BIN" hidden "$DMG_MOUNT_DIR/.VolumeIcon.icns" 2>/dev/null || true
    if [[ -d "$DMG_MOUNT_DIR/.fseventsd" ]]; then
      "$CHFLAGS_BIN" hidden "$DMG_MOUNT_DIR/.fseventsd" 2>/dev/null || true
    fi
  fi
  if [[ -n "$BLESS_BIN" ]]; then
    "$BLESS_BIN" --folder "$DMG_MOUNT_DIR" --openfolder "$DMG_MOUNT_DIR" \
      2>/dev/null || true
  fi
  detach_dmg_mount
  DMG_MOUNTED=0
  "$HDIUTIL_BIN" convert -quiet "$DMG_TMP_PATH" -format UDZO \
    -imagekey zlib-level=9 -o "$STAGED_DMG" -ov >/dev/null
fi

"$DITTO_BIN" -c -k --sequesterRsrc --keepParent "$STAGED_APP" "$STAGED_ZIP"
"$UNZIP_BIN" -tqq "$STAGED_ZIP"
# 只读镜像结构检查。现代系统走 diskutil，旧系统回退到 hdiutil。
if [[ "$DISKUTIL_IMAGE_SUPPORTED" == "1" ]]; then
  "$DISKUTIL_BIN" image info "$STAGED_DMG" >/dev/null
else
  # 旧版 imageinfo 不接受 -quiet；回退分支保留其原生诊断输出。
  "$HDIUTIL_BIN" imageinfo "$STAGED_DMG" >/dev/null
fi
if [[ -n "$HDIUTIL_BIN" ]]; then
  # -quiet 保留校验动作但隐藏 macOS 27 的弃用提示；退出码仍决定校验是否通过。
  "$HDIUTIL_BIN" verify -quiet "$STAGED_DMG"
else
  echo "提示: 未找到 hdiutil，跳过 UDIF 内部校验（已完成 diskutil 结构校验和挂载验证）" >&2
fi

# 不只检查容器校验和，还要验证 DMG/ZIP 里真正交付的 App 与安装入口。
DMG_VERIFY_DIR="$STAGING_DIR/dmg-verify"
mkdir -p "$DMG_VERIFY_DIR"
DMG_MOUNT_DIR="$DMG_VERIFY_DIR"
DMG_MOUNTED=1
if [[ "$DISKUTIL_IMAGE_SUPPORTED" == "1" ]]; then
  "$DISKUTIL_BIN" image attach --readOnly --nobrowse \
    --mountPoint "$DMG_VERIFY_DIR" "$STAGED_DMG" >/dev/null
else
  "$HDIUTIL_BIN" attach -quiet -readonly -noverify -noautoopen \
    -mountpoint "$DMG_VERIFY_DIR" "$STAGED_DMG" >/dev/null
fi
verify_app "$DMG_VERIFY_DIR/$APP_NAME.app"
[[ -L "$DMG_VERIFY_DIR/Applications" \
  && "$(readlink "$DMG_VERIFY_DIR/Applications")" == "/Applications" ]] \
  || die "DMG 的 Applications 快捷方式缺失或指向错误"
[[ -x "$DMG_VERIFY_DIR/安装 Sumpter.command" ]] || die "DMG 安装脚本缺失或不可执行"
[[ -f "$DMG_VERIFY_DIR/安装说明.txt" ]] || die "DMG 安装说明缺失"
detach_dmg_mount
DMG_MOUNTED=0

ZIP_VERIFY_DIR="$STAGING_DIR/zip-verify"
mkdir -p "$ZIP_VERIFY_DIR"
"$DITTO_BIN" -x -k "$STAGED_ZIP" "$ZIP_VERIFY_DIR"
verify_app "$ZIP_VERIFY_DIR/$APP_NAME.app"

# 只给 DMG 文件设置 Finder 图标；失败不应使已验证的 App/DMG 失效。
ICON_SETTER_DIR="$STAGING_DIR/iconsetter"
mkdir -p "$ICON_SETTER_DIR"
cat > "$ICON_SETTER_DIR/iconsetter.swift" <<'SWIFT'
import AppKit

let iconPath = CommandLine.arguments[1]
let dmgPath = CommandLine.arguments[2]

guard let image = NSImage(contentsOfFile: iconPath) else {
    fatalError("读不到图标: \(iconPath)")
}
if !NSWorkspace.shared.setIcon(image, forFile: dmgPath, options: []) {
    fatalError("设 DMG 文件图标失败: \(dmgPath)")
}
SWIFT
if "$SWIFTC_BIN" -sdk "$SDKROOT" "$ICON_SETTER_DIR/iconsetter.swift" \
  -framework AppKit -o "$ICON_SETTER_DIR/iconsetter"; then
  "$ICON_SETTER_DIR/iconsetter" "$ICON_PATH" "$STAGED_DMG" \
    || echo "提示: 设置 DMG 文件图标失败（不影响 .app/.dmg 内容）" >&2
else
  echo "提示: 编译 DMG 图标设置器失败（不影响 .app/.dmg 内容）" >&2
fi

commit_artifacts
echo "已生成: $APP_DIR"
echo "已生成: $DMG_PATH"
echo "已生成: $ZIP_PATH"
echo "SHA-256（App 主程序）:"
"$SHASUM_BIN" -a 256 "$APP_DIR/Contents/MacOS/$APP_NAME"
for artifact in "$DMG_PATH" "$ZIP_PATH"; do
  "$SHASUM_BIN" -a 256 "$artifact"
done
