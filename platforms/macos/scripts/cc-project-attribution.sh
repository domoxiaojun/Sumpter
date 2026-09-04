#!/usr/bin/env bash
# 为 Claude Code 配置项目归因:注入一个 claude() wrapper,按当前目录逐次设置
# ANTHROPIC_CUSTOM_HEADERS(X-Sumpter-Project / -Workspace / -Git-Remote)。
#
# 支持 macOS 与 Linux、zsh 与 bash。改动只有两处:一个独立 snippet 文件,
# 以及 rc 文件里一段带标记的 source 行(装前自动备份,可一键还原)。
#
# 用法: cc-project-attribution.sh [install|uninstall|restore|status|snippet] [选项]
#   --shell zsh|bash  跳过自动探测
#   --rc <path>       指定 rc 文件
#   --dry-run         只打印将要做的改动
#   --force           settings.json 里已写死该 header 时仍继续(装了也不会生效)
set -euo pipefail

MARK_BEGIN='# >>> sumpter cc-project-attribution >>>'
MARK_END='# <<< sumpter cc-project-attribution <<<'
SNIPPET_DEFAULT="${XDG_DATA_HOME:-$HOME/.local/share}/sumpter/cc-project-attribution.sh"
SETTINGS="$HOME/.claude/settings.json"

ACTION=install
SHELL_KIND=""
RC_FILE=""
DRY_RUN=0
FORCE=0

die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
info() { printf '%s\n' "$*"; }
run() { if [ "$DRY_RUN" -eq 1 ]; then printf '  [dry-run] %s\n' "$*"; else eval "$*"; fi; }

usage() { sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; }

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

SNIPPET="${SUMPTER_CC_SNIPPET:-$SNIPPET_DEFAULT}"

# ---- shell 探测:$SHELL 决定哪个 rc 会被读,比脚本自身的 shell 可靠 ----
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

# ---- rc 路径:macOS 的 bash 登录 shell 读 .bash_profile 而不是 .bashrc ----
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

strip_block() { # $1=rc  → stdout 去掉标记块后的内容
    awk -v b="$MARK_BEGIN" -v e="$MARK_END" '
        index($0, b) { skip = 1; next }
        index($0, e) { skip = 0; next }
        !skip { print }
    ' "$1"
}

# 备份名带 YYYYmmdd-HHMMSS,字典序即时间序,不需要按 mtime 排。
list_backups() {
    find "$(dirname "$1")" -maxdepth 1 -type f \
        -name "$(basename "$1").sumpter-bak-*" 2> /dev/null | sort -r
}

newest_backup() {
    list_backups "$1" | head -1
}

# settings.json 的 env 一旦写死该 header,就会覆盖进程环境变量,wrapper 永久失效。
settings_has_header() {
    [ -f "$SETTINGS" ] || return 1
    if command -v jq > /dev/null 2>&1; then
        [ "$(jq -r '.env.ANTHROPIC_CUSTOM_HEADERS // empty' "$SETTINGS" 2> /dev/null)" != "" ]
    else
        grep -q 'ANTHROPIC_CUSTOM_HEADERS' "$SETTINGS"
    fi
}

# fish 的等价配置。fish 不支持自动安装(语法与 POSIX shell 不同),打印供手工粘贴。
# 行为与 POSIX 版一致:纯 ASCII 才发、合并保留外部 header、非 origin 退回首个 remote。
fish_snippet_body() {
    cat <<'FISH_EOF'
# Sumpter · Claude Code 项目归因(fish)。粘进 ~/.config/fish/config.fish。
#
# !! 未经实测 !! 作者机器上没有 fish,这段没跑过,自测也覆盖不到(zsh/bash 版有 55 条
# 断言钉死)。粘之前先 `fish -n` 校验语法,粘之后先跑一次 `claude --version` 确认没把
# shell 弄坏。有问题请以 zsh/bash 版的行为为准自行调整。
# 注意:不要把 ANTHROPIC_CUSTOM_HEADERS 写进 ~/.claude/settings.json 的 env,
# 那会覆盖这里设的值且不做插值。
function __sumpter_cc_is_ascii
    string match -qr '^[ -~]+$' -- $argv[1]
end

function __sumpter_cc_local_user
    set -l u $USER
    test -n "$u"; or set u $LOGNAME
    test -n "$u"; or set u (command id -un 2>/dev/null)
    string match -qr '^[A-Za-z0-9._-]+$' -- $u; and echo $u
end

function claude
    set -l dir (string trim -r -c / -- $PWD)
    test -z "$dir"; and set dir /
    set -l proj (basename $dir)
    set -l remote (command git remote get-url origin 2>/dev/null)
    if test -z "$remote"
        set -l first (command git remote 2>/dev/null)[1]
        test -n "$first"; and set remote (command git remote get-url $first 2>/dev/null)
    end

    set -l hdrs
    # 保留调用方已有的非 X-Sumpter-* 行
    if set -q ANTHROPIC_CUSTOM_HEADERS
        for line in (string split \n -- $ANTHROPIC_CUSTOM_HEADERS)
            test -z "$line"; and continue
            string match -qir '^x-sumpter-' -- $line; and continue
            set -a hdrs $line
        end
    end
    __sumpter_cc_is_ascii "$proj"; and set -a hdrs "X-Sumpter-Project: $proj"
    __sumpter_cc_is_ascii "$dir"; and set -a hdrs "X-Sumpter-Workspace: $dir"
    __sumpter_cc_is_ascii "$remote"; and set -a hdrs "X-Sumpter-Git-Remote: $remote"
    set -l user (__sumpter_cc_local_user)
    test -n "$user"; and set -a hdrs "X-Sumpter-User: $user"

    if test (count $hdrs) -gt 0
        ANTHROPIC_CUSTOM_HEADERS=(string join \n -- $hdrs) command claude $argv
    else if set -q ANTHROPIC_CUSTOM_HEADERS
        # 过滤后为空但继承了旧值:显式清除,不透传过期归因
        set -e ANTHROPIC_CUSTOM_HEADERS
        command claude $argv
    else
        command claude $argv
    end
end
FISH_EOF
}

snippet_body() {
    cat <<'SNIPPET_EOF'
# Sumpter · Claude Code 项目归因(自动生成,勿手改)
# 由 cc-project-attribution.sh 管理。bash 与 zsh 通用。

# 值必须是纯 ASCII:Claude Code 见到非 ASCII 的 ANTHROPIC_CUSTOM_HEADERS 会
# 直接报错退出(整个会话起不来),所以这里宁可不发也不能让它拒启。
sumpter_cc_is_ascii() {
    # 必须锁 C:bash 3.2 在 en_*.UTF-8 下把 [ -~] 按排序序求值,连 "abc" 都会被
    # 判成含非 ASCII 字符(守卫全程假阴性)。local 只在本函数内生效,不污染外层。
    local LC_ALL=C
    case "$1" in
        "") return 1 ;;
        *[!\ -~]*) return 1 ;;
        *) return 0 ;;
    esac
}

sumpter_cc_local_user() {
    local LC_ALL=C u
    u="${USER:-${LOGNAME:-}}"
    [ -n "$u" ] || u="$(command id -un 2>/dev/null || true)"
    case "$u" in
        "") return 1 ;;
        *[!A-Za-z0-9._-]*) return 1 ;;
        *) printf '%s' "$u" ;;
    esac
}

sumpter_cc_headers() {
    local out="" line dir proj ws remote first_remote user
    # 合并而非覆盖:保留调用方已有的非 X-Sumpter-* 行
    if [ -n "${ANTHROPIC_CUSTOM_HEADERS:-}" ]; then
        while IFS= read -r line; do
            [ -z "$line" ] && continue
            case "$line" in
                [Xx]-[Ss][Uu][Mm][Pp][Tt][Ee][Rr]-*) continue ;;
            esac
            out="${out}${line}"$'\n'
        done <<SUMPTER_EOF
${ANTHROPIC_CUSTOM_HEADERS}
SUMPTER_EOF
    fi

    # PWD 可能带尾随斜杠(父进程传入的 cwd 常见如此),不先剥掉的话 ${dir##*/}
    # 会得到空字符串,项目名整条被丢掉 —— 只发 workspace 不发 project。
    dir="$PWD"
    while :; do
        case "$dir" in
            ?*/) dir="${dir%/}" ;;
            *) break ;;
        esac
    done
    proj="${dir##*/}"
    ws="$dir"

    # 优先 origin;没有 origin 的仓库(例如只有一个自定义 remote)退回第一个。
    remote="$(command git remote get-url origin 2> /dev/null || true)"
    if [ -z "$remote" ]; then
        first_remote="$(command git remote 2> /dev/null | head -1)"
        if [ -n "$first_remote" ]; then
            remote="$(command git remote get-url "$first_remote" 2> /dev/null || true)"
        fi
    fi

    sumpter_cc_is_ascii "$proj" && out="${out}X-Sumpter-Project: ${proj}"$'\n'
    sumpter_cc_is_ascii "$ws" && out="${out}X-Sumpter-Workspace: ${ws}"$'\n'
    sumpter_cc_is_ascii "$remote" && out="${out}X-Sumpter-Git-Remote: ${remote}"$'\n'
    user="$(sumpter_cc_local_user || true)"
    [ -n "$user" ] && out="${out}X-Sumpter-User: ${user}"$'\n'

    # 去掉尾随换行
    printf '%s' "${out%$'\n'}"
}

claude() {
    local sumpter_hdrs
    sumpter_hdrs="$(sumpter_cc_headers)"
    if [ -n "$sumpter_hdrs" ]; then
        ANTHROPIC_CUSTOM_HEADERS="$sumpter_hdrs" command claude "$@"
    elif [ -n "${ANTHROPIC_CUSTOM_HEADERS:-}" ]; then
        # 过滤后什么都不剩(例如目录名非 ASCII 被跳过),而继承值里还留着旧的
        # X-Sumpter-*:显式清除,不能把过期归因带给 CC。
        command env -u ANTHROPIC_CUSTOM_HEADERS claude "$@"
    else
        command claude "$@"
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

    if settings_has_header; then
        if [ "$FORCE" -eq 0 ]; then
            die "$SETTINGS 的 env 里已写有 ANTHROPIC_CUSTOM_HEADERS。
       它会覆盖进程环境变量,wrapper 装了也永远不生效。
       请先从 settings.json 删掉该键,或加 --force 强行安装。"
        fi
        info "警告:settings.json 已写死该 header,安装后不会生效（--force）"
    fi

    if [ "$kind" = bash ] && [ "$(uname -s)" = Darwin ] && [ "$rc" = "$HOME/.bashrc" ]; then
        info "提示:macOS 的 bash 登录 shell 读 .bash_profile;写到 .bashrc 可能不被加载。"
    fi

    # 已有同名 claude 函数/别名(非本脚本所装)时给出警告,不擅自覆盖判断
    if [ -f "$rc" ] && ! has_block "$rc" \
        && grep -Eq '^[[:space:]]*(alias[[:space:]]+claude=|claude[[:space:]]*\(\))' "$rc"; then
        info "警告:$rc 里已存在 claude 别名或函数,本 wrapper 会在其后定义并生效。"
    fi

    info "shell: $kind"
    info "rc:    $rc"
    info "snippet: $SNIPPET"

    if [ -f "$rc" ]; then
        backup="$rc.sumpter-bak-$(date +%Y%m%d-%H%M%S)"
        run "cp -p \"$rc\" \"$backup\""
        info "已备份: $backup"
    else
        info "rc 不存在,将新建"
        run "mkdir -p \"$(dirname "$rc")\""
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
        printf '[ -f "%s" ] && . "%s"\n' "$SNIPPET" "$SNIPPET"
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
        run "rm -f \"$SNIPPET\""
        info "已删除 snippet: $SNIPPET"
    fi
    info "备份保留未动（restore 可还原到装前状态）"
}

do_restore() {
    local kind rc backup
    kind="$(detect_shell)"
    rc="$(resolve_rc "$kind")"
    backup="$(newest_backup "$rc" || true)"
    [ -n "$backup" ] || die "找不到 $rc 的备份（$rc.sumpter-bak-*）"
    info "还原: $backup → $rc"
    if [ "$DRY_RUN" -eq 1 ]; then
        printf '  [dry-run] 先把当前 rc 存为 .sumpter-prerestore-*,再覆盖\n'
        return
    fi
    [ -f "$rc" ] && cp -p "$rc" "$rc.sumpter-prerestore-$(date +%Y%m%d-%H%M%S)"
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
    printf 'settings.json 键: %s\n' \
        "$(settings_has_header && printf '存在(会覆盖 wrapper,必须删)' || printf '无(正确)')"
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
