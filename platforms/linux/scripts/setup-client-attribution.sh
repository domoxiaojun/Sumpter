#!/usr/bin/env bash
# Run on the client host as its regular user, not on behalf of a remote browser.
set -euo pipefail
umask 077

usage() {
  cat <<'HELP'
用法：bash setup-client-attribution.sh [status|install|restore] [claude|grok|gemini|codex|pi|all] [--shell bash|zsh] [--rc 文件]
不带参数进入交互菜单；操作后自动检查当前状态。
优先使用同目录 client-attribution.mjs 与 pi-project-attribution.ts。
缺失时默认从 GitHub 仓库 raw 下载：
  https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/
可用 SUMPTER_RESOURCE_BASE 覆盖该目录 URL。
已运行的代理可选 SUMPTER_BASE_URL（代理根地址）从 /__sumpter/ 下载；启用认证时设置 SUMPTER_AUTH_TOKEN。
需要 Node.js 18+，请在运行客户端的主机执行，不要使用 sudo。
HELP
}

if [[ ${1:-} == --help || ${1:-} == -h ]]; then usage; exit 0; fi
if [[ ${EUID} -eq 0 ]]; then echo '请以运行客户端的普通用户执行，不要使用 sudo。' >&2; exit 1; fi
command -v node >/dev/null 2>&1 || { echo '未找到 Node.js，请先安装 Node.js 18+。' >&2; exit 1; }
node -e 'if (Number(process.versions.node.split(".")[0]) < 18) process.exit(1)' || { echo '需要 Node.js 18+。' >&2; exit 1; }

action=${1:-}
client=${2:-all}
if [[ -z $action ]]; then
  [[ -t 0 ]] || { usage >&2; exit 2; }
  printf '客户端：1) Claude Code  2) Grok Build  3) Gemini CLI  4) Codex  5) pi  6) 全部\n'
  read -r -p '请选择 [1-6，默认 6]：' choice
  case ${choice:-6} in 1) client=claude;; 2) client=grok;; 3) client=gemini;; 4) client=codex;; 5) client=pi;; 6) client=all;; *) exit 2;; esac
  printf '操作：1) 检查状态  2) 安装配置  3) 还原配置\n'
  read -r -p '请选择 [1-3，默认 1]：' choice
  case ${choice:-1} in 1) action=status;; 2) action=install;; 3) action=restore;; *) exit 2;; esac
else
  shift
  if [[ $# -gt 0 ]]; then shift; fi
fi
case $action in status|install|restore) ;; *) usage >&2; exit 2;; esac
case $client in claude|grok|gemini|codex|pi|all) ;; *) usage >&2; exit 2;; esac
options=("$@")
# Validate before attempting a download or changing configuration.
while [[ $# -gt 0 ]]; do
  case $1 in --shell|--rc) [[ $# -ge 2 && -n $2 ]] || { usage >&2; exit 2; }; shift 2;; *) usage >&2; exit 2;; esac
done
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
installer="$script_dir/client-attribution.mjs"
task_tmp=$(mktemp -d "${TMPDIR:-/tmp}/sumpter-attribution.XXXXXX")
trap 'rm -rf -- "$task_tmp"' EXIT
DEFAULT_RESOURCE_BASE='https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts'
download_resource() {
  local name=$1 destination=$2 url=''
  command -v curl >/dev/null 2>&1 || { echo '未找到 curl。' >&2; exit 1; }
  headers="$task_tmp/headers"
  if [[ -n ${SUMPTER_BASE_URL:-} ]]; then
    case $SUMPTER_BASE_URL in http://*|https://*) ;; *) echo '代理根地址必须使用 http:// 或 https://' >&2; exit 2;; esac
    url="${SUMPTER_BASE_URL%/}/__sumpter/$name"
    if [[ -n ${SUMPTER_AUTH_TOKEN:-} ]]; then
      [[ $SUMPTER_AUTH_TOKEN != *$'\n'* && $SUMPTER_AUTH_TOKEN != *$'\r'* ]] || { echo 'Token 包含无效换行。' >&2; exit 2; }
      printf 'Authorization: Bearer %s\n' "$SUMPTER_AUTH_TOKEN" > "$headers"
    else
      : > "$headers"
    fi
  else
    local base="${SUMPTER_RESOURCE_BASE:-$DEFAULT_RESOURCE_BASE}"
    case $base in http://*|https://*) ;; *) echo '资源地址必须使用 http:// 或 https://' >&2; exit 2;; esac
    url="${base%/}/$name"
    : > "$headers"
  fi
  curl --fail --silent --show-error --connect-timeout 10 --max-time 60 \
    -H "@$headers" "$url" -o "$destination"
}
if [[ ! -f $installer ]]; then
  installer="$task_tmp/client-attribution.mjs"
  download_resource client-attribution.mjs "$installer"
fi
if [[ $client == pi || $client == all ]]; then
  # Stage both files together without writing into the downloaded script's directory.
  extension="$script_dir/pi-project-attribution.ts"
  if [[ $installer != "$task_tmp/client-attribution.mjs" ]]; then
    cp "$installer" "$task_tmp/client-attribution.mjs"
    installer="$task_tmp/client-attribution.mjs"
  fi
  if [[ -f $extension ]]; then
    cp "$extension" "$task_tmp/pi-project-attribution.ts"
  else
    download_resource pi-project-attribution.ts "$task_tmp/pi-project-attribution.ts"
  fi
fi
node "$installer" "$action" "$client" "${options[@]}" > "$task_tmp/result.json"
if [[ $action != status ]]; then
  if [[ $action == install ]]; then echo '归因配置已安装。'; else echo '归因配置已还原；没有还原记录的客户端保持原样。'; fi
  case $client in
    pi) echo '请在 pi 中执行 /reload；provider 需要设置 X-Sumpter-Client: pi。';;
    all) echo '其他客户端请新开终端；pi 请执行 /reload，并设置 provider 的 X-Sumpter-Client: pi。';;
    *) echo '请新开终端。';;
  esac
  node "$installer" status "$client" "${options[@]}" > "$task_tmp/result.json"
fi
node - "$task_tmp/result.json" <<'NODE'
const fs = require('node:fs');
const labels = { installed: '已安装', absent: '未安装', legacy: '旧版配置，建议更新', broken: '配置不完整，需要修复', outdated: '已安装，需要更新' };
for (const item of JSON.parse(fs.readFileSync(process.argv[2], 'utf8'))) {
  console.log(`${item.client}：${labels[item.status] || item.status} · ${item.shell} · ${item.rc} · ${item.canRestore ? '可还原' : '无待还原配置'}`);
}
NODE
