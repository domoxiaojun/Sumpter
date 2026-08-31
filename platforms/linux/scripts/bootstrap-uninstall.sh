#!/usr/bin/env bash
# Invoke the installed package's scope-aware uninstaller from a static mirror.
set -Eeuo pipefail
umask 077

PURGE=0
UNINSTALL_SCRIPT=""
SYSTEMD_SCOPE=""

die() {
    echo "错误:$*" >&2
    exit 1
}

note() {
    echo "[kekulv-bootstrap-uninstall] $*"
}

usage() {
    cat <<'EOF'
用法: bootstrap-uninstall.sh [--purge]

普通用户运行时，调用 ~/.local/share/kekulv/scripts/uninstall.sh。
root 或 sudo 运行时，调用 /opt/kekulv/scripts/uninstall.sh。
默认保留配置；--purge 才永久删除当前 scope 的配置和统计数据。

选项:
  --purge      同时永久删除当前 scope 的配置目录
  -h, --help   显示帮助
EOF
}

while (($# > 0)); do
    case "$1" in
        --purge)
            PURGE=1
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            die "未知参数:$1"
            ;;
    esac
done

if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    SYSTEMD_SCOPE="system"
    UNINSTALL_SCRIPT="/opt/kekulv/scripts/uninstall.sh"
else
    [[ -n "${HOME:-}" && "$HOME" == /* && "$HOME" != "/" && "$HOME" != *$'\n'* ]] \
        || die "HOME 必须是非根目录的绝对路径且不能包含换行"
    command -v realpath >/dev/null 2>&1 || die "缺少命令:realpath"
    home_real="$(realpath "$HOME")" || die "无法解析 HOME:$HOME"
    SYSTEMD_SCOPE="user"
    UNINSTALL_SCRIPT="$home_real/.local/share/kekulv/scripts/uninstall.sh"
fi

[[ -f "$UNINSTALL_SCRIPT" && ! -L "$UNINSTALL_SCRIPT" ]] \
    || die "未找到可用的 $SYSTEMD_SCOPE 卸载器:$UNINSTALL_SCRIPT"

note "调用 $SYSTEMD_SCOPE 卸载器:$UNINSTALL_SCRIPT"
if [[ "$PURGE" -eq 1 ]]; then
    exec bash "$UNINSTALL_SCRIPT" --purge
fi
exec bash "$UNINSTALL_SCRIPT"
