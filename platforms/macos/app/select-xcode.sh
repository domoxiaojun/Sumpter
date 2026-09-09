#!/usr/bin/env bash
# CI、Release、本地检查与打包共用的稳定工具链基线；stdout 只输出 Developer 路径。
set -euo pipefail

required_version=26.6
required_build=17F113
required_sdk=26.5

if [[ -n "${DEVELOPER_DIR:-}" ]]; then
  candidates=("$DEVELOPER_DIR")
else
  candidates=(
    "/Applications/Xcode_${required_version}.app/Contents/Developer"
    "/Applications/Xcode.app/Contents/Developer"
    "$(xcode-select -p 2>/dev/null || true)"
  )
fi

for candidate in "${candidates[@]}"; do
  candidate="${candidate%/}"
  case "$candidate" in
    *.app) candidate="$candidate/Contents/Developer" ;;
  esac
  [[ -x "$candidate/usr/bin/xcodebuild" ]] || continue
  [[ -x "$candidate/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift" ]] || continue
  version_output="$(DEVELOPER_DIR="$candidate" "$candidate/usr/bin/xcodebuild" -version 2>/dev/null)" || continue
  version="$(printf '%s\n' "$version_output" | awk '$1 == "Xcode" { print $2; exit }')"
  build="$(printf '%s\n' "$version_output" | awk '$1 == "Build" && $2 == "version" { print $3; exit }')"
  [[ "$version" == "$required_version" && "$build" == "$required_build" ]] || continue
  sdk="$(DEVELOPER_DIR="$candidate" xcrun --sdk macosx --show-sdk-version 2>/dev/null)" || continue
  [[ "$sdk" == "$required_sdk" ]] || continue
  printf '%s\n' "$candidate"
  exit 0
done

echo "需要 Xcode ${required_version} (${required_build}) / macOS SDK ${required_sdk}；请安装该稳定版工具链。" >&2
if [[ -n "${DEVELOPER_DIR:-}" ]]; then
  echo "DEVELOPER_DIR=$DEVELOPER_DIR 未匹配；请指向该版本的 Xcode，或取消覆盖以使用默认安装位置。" >&2
fi
exit 1
