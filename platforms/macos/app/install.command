#!/usr/bin/env bash
# Install the adjacent Sumpter app for users who do not have a Developer ID build.
set -Eeuo pipefail

APP_NAME="Sumpter.app"
EXPECTED_BUNDLE_ID="org.kkl.sumpter"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_APP="$SCRIPT_DIR/$APP_NAME"

die() {
    echo
    echo "安装失败：$*" >&2
    echo "请按回车关闭窗口。"
    read -r _ || true
    exit 1
}

echo "Sumpter macOS 安装器"
echo "=================="
echo
[[ -d "$SOURCE_APP" ]] || die "找不到同目录的 $APP_NAME"
[[ -f "$SOURCE_APP/Contents/Info.plist" ]] || die "App bundle 不完整"
[[ -x "$SOURCE_APP/Contents/MacOS/Sumpter" ]] || die "App 主程序缺失"

BUNDLE_ID="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$SOURCE_APP/Contents/Info.plist" 2>/dev/null || true)"
[[ "$BUNDLE_ID" == "$EXPECTED_BUNDLE_ID" ]] || die "Bundle ID 不匹配，已停止处理"

if ! codesign --verify --deep --strict "$SOURCE_APP" >/dev/null 2>&1; then
    die "App 签名校验失败，未删除任何安全属性"
fi

echo "将安装：$SOURCE_APP"
echo
echo "此安装器只会移除这个 App 的下载隔离标记，不会关闭 macOS Gatekeeper。"
echo "按回车继续，按 Control-C 取消。"
read -r _

INSTALL_ROOT="/Applications"
if [[ ! -d "$INSTALL_ROOT" || ! -w "$INSTALL_ROOT" ]]; then
    INSTALL_ROOT="$HOME/Applications"
    mkdir -p "$INSTALL_ROOT"
    echo "没有 /Applications 写权限，将安装到：$INSTALL_ROOT"
fi

DEST_APP="$INSTALL_ROOT/$APP_NAME"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/sumpter-install.XXXXXX")"
BACKUP_APP=""
cleanup() {
    rm -rf "$TEMP_ROOT"
}
trap cleanup EXIT

ditto --rsrc --extattr --acl "$SOURCE_APP" "$TEMP_ROOT/$APP_NAME"
if [[ -e "$DEST_APP" ]]; then
    BACKUP_APP="$INSTALL_ROOT/$APP_NAME.previous.$(date +%Y%m%d%H%M%S)"
    mv "$DEST_APP" "$BACKUP_APP"
fi
if ! mv "$TEMP_ROOT/$APP_NAME" "$DEST_APP"; then
    if [[ -n "$BACKUP_APP" && -e "$BACKUP_APP" ]]; then
        mv "$BACKUP_APP" "$DEST_APP" || true
    fi
    die "无法写入 $DEST_APP"
fi

# 只对我们刚校验过、刚复制的明确 App 路径处理 quarantine。
xattr -dr com.apple.quarantine "$DEST_APP" 2>/dev/null || true
if ! codesign --verify --deep --strict "$DEST_APP" >/dev/null 2>&1; then
    if [[ -n "$BACKUP_APP" && -e "$BACKUP_APP" ]]; then
        rm -rf "$DEST_APP"
        mv "$BACKUP_APP" "$DEST_APP"
    fi
    die "安装后的 App 校验失败，已尝试恢复旧版本"
fi

echo
echo "安装完成：$DEST_APP"
echo "正在启动Sumpter……"
open "$DEST_APP"
echo "可以关闭此窗口。"
read -r _ || true
