#!/usr/bin/env bash
# CI、Release、本地检查与打包共用的完整 Xcode 选择器；stdout 只输出 Developer 路径。
set -euo pipefail

if [[ -n "${DEVELOPER_DIR:-}" ]]; then
  candidates=("$DEVELOPER_DIR")
else
  candidates=()
  # 直接沿用 xcode-select 当前选中的完整 Xcode；若当前选择是
  # CommandLineTools 或路径失效，再从 /Applications 下的 Xcode 兜底。
  selected_by_xcode_select="$(xcode-select -p 2>/dev/null || true)"
  [[ -n "$selected_by_xcode_select" ]] && candidates+=("$selected_by_xcode_select")
  # 允许并行安装任意版本的 Xcode。未匹配的 glob 会保留字面量，
  # 后续的 -d 检查会将其过滤掉。
  for app in /Applications/Xcode*.app; do
    [[ -d "$app" ]] && candidates+=("$app/Contents/Developer")
  done
fi

for candidate in "${candidates[@]}"; do
  candidate="${candidate%/}"
  case "$candidate" in
    *.app) candidate="$candidate/Contents/Developer" ;;
  esac
  [[ -x "$candidate/usr/bin/xcodebuild" ]] || continue
  [[ -x "$candidate/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" ]] || continue
  sdk_path="$(DEVELOPER_DIR="$candidate" xcrun --sdk macosx --show-sdk-path 2>/dev/null)" || continue
  [[ -n "$sdk_path" && -d "$sdk_path" && "$sdk_path" != *CommandLineTools* ]] || continue
  printf '%s\n' "$candidate"
  exit 0
done

echo "找不到可用的完整 Xcode 工具链；请安装 Xcode，或通过 DEVELOPER_DIR 指向其 Contents/Developer 目录。" >&2
if [[ -n "${DEVELOPER_DIR:-}" ]]; then
  echo "DEVELOPER_DIR=$DEVELOPER_DIR 未匹配；请指向完整 Xcode，或取消覆盖以使用 /Applications 下的安装。" >&2
fi
exit 1
