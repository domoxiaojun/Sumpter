#!/usr/bin/env bash
# 为 Grok Build 配置项目归因:注入 grok() wrapper,按当前目录把
# X-Sumpter-Project / -Workspace / -Git-Remote / -User 写入 GROK_CONFIG overlay
# ([models].extra_headers)。不改 ~/.grok/config.toml。
#
# 支持 macOS 与 Linux、zsh 与 bash。改动只有两处:一个独立 snippet 文件,
# 以及 rc 文件里一段带标记的 source 行(装前自动备份,可一键还原)。
#
# 用法: grok-project-attribution.sh [install|uninstall|restore|status|snippet] [选项]
#   --shell zsh|bash  跳过自动探测
#   --rc <path>       指定 rc 文件
#   --dry-run         只打印将要做的改动
#   --force           GROK_CONFIG_PATH 已设置时仍继续
set -euo pipefail

MARK_BEGIN='# >>> sumpter grok-project-attribution >>>'
MARK_END='# <<< sumpter grok-project-attribution <<<'
SNIPPET_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/sumpter/grok-project-attribution.sh"

ACTION=install
SHELL_KIND=""
RC_FILE=""
DRY_RUN=0
FORCE=0

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }
run() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run]'
        printf ' %q' "$@"
        printf '\n'
    else
        "$@"
    fi
}

usage() { sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'; }

while [ $# -gt 0 ]; do
    case "$1" in
        install | uninstall | restore | status | snippet | fish-snippet) ACTION="$1" ;;
        --shell) SHELL_KIND="${2:-}"; shift ;;
        --rc) RC_FILE="${2:-}"; shift ;;
        --dry-run) DRY_RUN=1 ;;
        --force) FORCE=1 ;;
        -h | --help) usage; exit 0 ;;
        *) die "未知参数:$1（--help 看用法）" ;;
    esac
    shift
done

SNIPPET="${SUMPTER_GROK_SNIPPET:-$SNIPPET_DEFAULT}"

detect_shell() {
    [ -n "$SHELL_KIND" ] && { printf '%s' "$SHELL_KIND"; return; }
    case "$(basename "${SHELL:-}")" in
        zsh) printf 'zsh' ;;
        bash) printf 'bash' ;;
        fish) die "检测到 fish。自动安装只支持 zsh 与 bash。
       fish 请运行:  $0 fish-snippet
       它会打印一段等价的 fish 配置,粘进 ~/.config/fish/config.fish 即可。" ;;
        *) die "无法确定登录 shell（SHELL=${SHELL:-未设置}）。用 --shell zsh|bash 指定。" ;;
    esac
}

resolve_rc() {
    [ -n "$RC_FILE" ] && { printf '%s' "$RC_FILE"; return; }
    case "$1" in
        zsh) printf '%s/.zshrc' "${ZDOTDIR:-$HOME}" ;;
        bash)
            if [ "$(uname -s)" = "Darwin" ] && [ -f "$HOME/.bash_profile" ]; then
                printf '%s/.bash_profile' "$HOME"
            else
                printf '%s/.bashrc' "$HOME"
            fi
            ;;
        fish) die "fish 不支持自动安装。运行:  $0 fish-snippet  取得可粘贴配置。" ;;
        *) die "不支持的 shell:$1（只支持 zsh 与 bash）" ;;
    esac
}

has_block() { [ -f "$1" ] && grep -Fq "$MARK_BEGIN" "$1"; }

strip_block() {
    awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
        index($0, b) { skip = 1; next }
        index($0, e) { skip = 0; next }
        !skip { print }
    ' "$1"
}

list_backups() {
    find "$(dirname "$1")" -maxdepth 1 -type f \
        -name "$(basename "$1").sumpter-grok-bak-*" 2> /dev/null | sort -r
}

newest_backup() {
    list_backups "$1" | head -1
}

path_overlay_set() { [ -n "${GROK_CONFIG_PATH:-}" ]; }

fish_snippet_body() {
    cat <<'FISH_EOF'
# Sumpter · Grok Build 项目归因(fish)。粘进 ~/.config/fish/config.fish。
#
# !! 未经实测 !! 作者机器上没有 fish。粘之前先 `fish -n` 校验语法。
# 注意:若已设置 GROK_CONFIG_PATH,本 wrapper 设置的 GROK_CONFIG 会挡住它。
function __sumpter_grok_is_ascii
    string match -qr '^[ -~]+$' -- $argv[1]
end

function __sumpter_grok_local_user
    set -l u $USER
    test -n "$u"; or set u $LOGNAME
    test -n "$u"; or set u (command id -un 2>/dev/null)
    string match -qr '^[A-Za-z0-9._-]+$' -- $u; and echo $u
end

function grok
    set -l dir (string trim -r -c / -- $PWD)
    test -z "$dir"; and set dir /
    set -l proj (basename $dir)
    set -l remote (command git remote get-url origin 2>/dev/null)
    if test -z "$remote"
        set -l first (command git remote 2>/dev/null)[1]
        test -n "$first"; and set remote (command git remote get-url $first 2>/dev/null)
    end
    set -l user (__sumpter_grok_local_user)
    set -l pairs
    __sumpter_grok_is_ascii "$proj"; and set -a pairs "X-Sumpter-Project" "$proj"
    __sumpter_grok_is_ascii "$dir"; and set -a pairs "X-Sumpter-Workspace" "$dir"
    __sumpter_grok_is_ascii "$remote"; and set -a pairs "X-Sumpter-Git-Remote" "$remote"
    test -n "$user"; and set -a pairs "X-Sumpter-User" "$user"
    if test (count $pairs) -eq 0
        command grok $argv
        return
    end
    set -l overlay (python3 -c 'import json,sys
pairs=sys.argv[1:]
headers={pairs[i]:pairs[i+1] for i in range(0,len(pairs),2)}
print(json.dumps({"models":{"extra_headers":headers}},separators=(",",":")))
' $pairs)
    if test -n "$GROK_CONFIG"
        set -x GROK_CONFIG (SUMPTER_GROK_OVERLAY="$overlay" python3 -c 'import json,os
overlay=json.loads(os.environ["SUMPTER_GROK_OVERLAY"])
try:
    cfg=json.loads(os.environ.get("GROK_CONFIG") or "{}")
except json.JSONDecodeError:
    cfg={}
if not isinstance(cfg, dict):
    cfg={}
models=cfg.get("models")
if not isinstance(models, dict):
    models={}
    cfg["models"]=models
headers=models.get("extra_headers")
if not isinstance(headers, dict):
    headers={}
    models["extra_headers"]=headers
headers.update(overlay.get("models",{}).get("extra_headers",{}))
print(json.dumps(cfg,separators=(",",":")))
')
    else
        set -x GROK_CONFIG $overlay
    end
    command grok $argv
end
FISH_EOF
}

snippet_body() {
    cat <<'SNIPPET_EOF'
# Sumpter · Grok Build 项目归因(自动生成,勿手改)
# 由 grok-project-attribution.sh 管理。bash 与 zsh 通用。

sumpter_grok_is_ascii() {
    local LC_ALL=C
    case "$1" in
        "") return 1 ;;
        *[!\ -~]*) return 1 ;;
        *) return 0 ;;
    esac
}

sumpter_grok_local_user() {
    local LC_ALL=C u
    u="${USER:-${LOGNAME:-}}"
    [ -n "$u" ] || u="$(command id -un 2>/dev/null || true)"
    case "$u" in
        "") return 1 ;;
        *[!A-Za-z0-9._-]*) return 1 ;;
        *) printf '%s' "$u" ;;
    esac
}

sumpter_grok_json_escape() {
    local s=$1
    s=${s//\\/\\\\}
    s=${s//\"/\\\"}
    printf '%s' "$s"
}

sumpter_grok_overlay() {
    local dir proj remote user json="" sep=""
    dir="$PWD"
    while :; do
        case "$dir" in
            ?*/) dir="${dir%/}" ;;
            *) break ;;
        esac
    done
    [ -n "$dir" ] || dir="/"
    proj="${dir##*/}"
    remote="$(command git remote get-url origin 2> /dev/null || true)"
    if [ -z "$remote" ]; then
        local first_remote
        first_remote="$(command git remote 2> /dev/null | head -1)"
        if [ -n "$first_remote" ]; then
            remote="$(command git remote get-url "$first_remote" 2> /dev/null || true)"
        fi
    fi
    user="$(sumpter_grok_local_user || true)"
    json='{"models":{"extra_headers":{'
    if sumpter_grok_is_ascii "$proj"; then
        json="${json}${sep}\"X-Sumpter-Project\":\"$(sumpter_grok_json_escape "$proj")\""
        sep=","
    fi
    if sumpter_grok_is_ascii "$dir"; then
        json="${json}${sep}\"X-Sumpter-Workspace\":\"$(sumpter_grok_json_escape "$dir")\""
        sep=","
    fi
    if sumpter_grok_is_ascii "$remote"; then
        json="${json}${sep}\"X-Sumpter-Git-Remote\":\"$(sumpter_grok_json_escape "$remote")\""
        sep=","
    fi
    if [ -n "$user" ]; then
        json="${json}${sep}\"X-Sumpter-User\":\"$(sumpter_grok_json_escape "$user")\""
    fi
    json="${json}}}}"
    if [ "$json" = '{"models":{"extra_headers":{}}}' ]; then
        return 1
    fi
    printf '%s' "$json"
}

sumpter_grok_merge_config() {
    local overlay=$1
    if [ -z "${GROK_CONFIG:-}" ]; then
        printf '%s' "$overlay"
        return 0
    fi
    if ! command -v python3 >/dev/null 2>&1; then
        printf 'ERROR: GROK_CONFIG 已设置但找不到 python3,无法合并 overlay\n' >&2
        return 1
    fi
    SUMPTER_GROK_OVERLAY="$overlay" python3 -c '
import json, os
overlay = json.loads(os.environ["SUMPTER_GROK_OVERLAY"])
try:
    cfg = json.loads(os.environ.get("GROK_CONFIG") or "{}")
except json.JSONDecodeError:
    cfg = {}
if not isinstance(cfg, dict):
    cfg = {}
models = cfg.get("models")
if not isinstance(models, dict):
    models = {}
    cfg["models"] = models
headers = models.get("extra_headers")
if not isinstance(headers, dict):
    headers = {}
    models["extra_headers"] = headers
headers.update((overlay.get("models") or {}).get("extra_headers") or {})
print(json.dumps(cfg, separators=(",", ":"), ensure_ascii=True))
'
}

grok() {
    local overlay merged
    if overlay="$(sumpter_grok_overlay)"; then
        if ! merged="$(sumpter_grok_merge_config "$overlay")"; then
            command grok "$@"
            return
        fi
        GROK_CONFIG="$merged" command grok "$@"
    else
        command grok "$@"
    fi
}
SNIPPET_EOF
}

write_snippet() {
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run] 写 snippet: %s\n' "$SNIPPET"
        return
    fi
    mkdir -p "$(dirname "$SNIPPET")"
    snippet_body > "$SNIPPET"
    chmod 0644 "$SNIPPET"
}

do_install() {
    local kind rc backup tmp
    kind="$(detect_shell)"
    rc="$(resolve_rc "$kind")"

    if path_overlay_set; then
        if [ "$FORCE" -eq 0 ]; then
            die "当前环境已设置 GROK_CONFIG_PATH。
       grok() 写入的 GROK_CONFIG 会挡住这份 path overlay。
       请先确认不需要 GROK_CONFIG_PATH,或加 --force 强行安装。"
        fi
        info "警告:GROK_CONFIG_PATH 已设置,GROK_CONFIG overlay 会优先于它（--force）"
    fi

    if [ "$kind" = bash ] && [ "$(uname -s)" = Darwin ] && [ "$rc" = "$HOME/.bashrc" ]; then
        info "提示:macOS 的 bash 登录 shell 读 .bash_profile;写到 .bashrc 可能不被加载。"
    fi

    if [ -f "$rc" ] && ! has_block "$rc" \
        && grep -Eq '^[[:space:]]*(alias[[:space:]]+grok=|grok[[:space:]]*\(\))' "$rc"; then
        info "警告:$rc 里已存在 grok 别名或函数,本 wrapper 会在其后定义并生效。"
    fi

    info "shell: $kind"
    info "rc:    $rc"
    info "snippet: $SNIPPET"

    if [ -f "$rc" ]; then
        backup="$rc.sumpter-grok-bak-$(date +%Y%m%d-%H%M%S)"
        run cp -p -- "$rc" "$backup"
        info "已备份: $backup"
    else
        info "rc 不存在,将新建"
        run mkdir -p -- "$(dirname "$rc")"
    fi

    write_snippet

    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run] 在 %s 追加带标记的 source 块\n' "$rc"
        return
    fi

    tmp="$(mktemp)"
    if [ -f "$rc" ]; then strip_block "$rc" > "$tmp"; fi
    {
        printf '%s\n' "$MARK_BEGIN"
        printf '[ -f %q ] && . %q\n' "$SNIPPET" "$SNIPPET"
        printf '%s\n' "$MARK_END"
    } >> "$tmp"
    cat "$tmp" > "$rc"
    rm -f "$tmp"
    info "已安装。新开一个终端,或执行: . $rc"
}

do_uninstall() {
    local kind rc tmp
    kind="$(detect_shell)"
    rc="$(resolve_rc "$kind")"
    if ! has_block "$rc"; then
        info "未安装（$rc 里没有标记块）"
    elif [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run] 从 %s 移除标记块\n' "$rc"
    else
        tmp="$(mktemp)"
        strip_block "$rc" > "$tmp"
        cat "$tmp" > "$rc"
        rm -f "$tmp"
        info "已从 $rc 移除标记块"
    fi
    if [ -f "$SNIPPET" ]; then
        run rm -f -- "$SNIPPET"
        info "已删除 snippet: $SNIPPET"
    fi
    info "备份保留未动（restore 可还原到装前状态）"
}

do_restore() {
    local kind rc backup
    kind="$(detect_shell)"
    rc="$(resolve_rc "$kind")"
    backup="$(newest_backup "$rc" || true)"
    [ -n "$backup" ] || die "找不到 $rc 的备份（$rc.sumpter-grok-bak-*）"
    info "还原: $backup → $rc"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run] 先把当前 rc 存为 .sumpter-grok-prerestore-*,再覆盖\n'
        return
    fi
    [ -f "$rc" ] && cp -p "$rc" "$rc.sumpter-grok-prerestore-$(date +%Y%m%d-%H%M%S)"
    cat "$backup" > "$rc"
    info "已还原。snippet 未删除,如需一并清理请先跑 uninstall。"
}

do_status() {
    local kind rc
    kind="$(detect_shell)"
    rc="$(resolve_rc "$kind")"
    printf 'shell:            %s\n' "$kind"
    printf 'rc:               %s%s\n' "$rc" "$([ -f "$rc" ] || printf ' (不存在)')"
    printf 'rc 内标记块:      %s\n' "$(has_block "$rc" && printf '已安装' || printf '未安装')"
    printf 'snippet:          %s%s\n' "$SNIPPET" "$([ -f "$SNIPPET" ] || printf ' (不存在)')"
    printf 'GROK_CONFIG_PATH: %s\n' \
        "$(path_overlay_set && printf '已设置(会与 GROK_CONFIG overlay 冲突)' || printf '无(正确)')"
    printf '备份:\n'
    if [ -n "$(list_backups "$rc")" ]; then
        list_backups "$rc" | sed 's/^/  /'
    else
        printf '  (无)\n'
    fi
}

case "$ACTION" in
    install) do_install ;;
    uninstall) do_uninstall ;;
    restore) do_restore ;;
    status) do_status ;;
    snippet) snippet_body ;;
    fish-snippet) fish_snippet_body ;;
esac
