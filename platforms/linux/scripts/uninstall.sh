#!/usr/bin/env bash
# Uninstall the user or system Linux release while preserving configuration by default.
set -Eeuo pipefail
umask 077

PURGE=0
SYSTEMD_SCOPE=""
SYSTEM_TEST_ROOT="${KEKULV_SYSTEM_TEST_ROOT:-}"
SYSTEM_ROOT=""
HOME_REAL=""
PROGRAM_PARENT=""
PROGRAM_PARENT_REAL=""
PROGRAM_PARENT_EXPECTED=""
INSTALL_DIR=""
PREVIOUS_DIR=""
CONFIG_PARENT=""
CONFIG_PARENT_REAL=""
CONFIG_PARENT_EXPECTED=""
CONFIG_DIR=""
UNIT_DIR=""
UNIT_DIR_REAL=""
UNIT_DIR_EXPECTED=""
UNIT_PATH=""
UNIT_PREVIOUS=""

die() {
    echo "错误:$*" >&2
    exit 1
}

note() {
    echo "[kekulv-uninstall] $*"
}

usage() {
    cat <<'EOF'
用法: uninstall.sh [--purge]

默认停用 kekulv.service，并删除程序、上一版本和对应 scope 的 unit；保留配置。
普通用户配置位于 ~/.config/kekulv，root/system 配置位于 /var/lib/kekulv。
只有显式传入 --purge 才删除 config.json、stats.json 和运行时文件。

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

validate_system_test_root() {
    local temp_root=""
    [[ "${KEKULV_INSTALLER_SELFTEST:-}" == "1" ]] \
        || die "KEKULV_SYSTEM_TEST_ROOT 仅允许 installer-selftest 使用"
    [[ "$SYSTEM_TEST_ROOT" == /* && "$SYSTEM_TEST_ROOT" != "/" && -d "$SYSTEM_TEST_ROOT" && ! -L "$SYSTEM_TEST_ROOT" ]] \
        || die "测试 rootfs 必须是已存在的非链接绝对目录"
    SYSTEM_ROOT="$(realpath -e "$SYSTEM_TEST_ROOT")" || die "无法解析测试 rootfs:$SYSTEM_TEST_ROOT"
    temp_root="$(realpath -e "${TMPDIR:-/tmp}")" || die "无法解析 TMPDIR"
    case "$SYSTEM_ROOT" in
        "$temp_root"/kekulv-installer-test.*/rootfs) ;;
        *) die "测试 rootfs 必须位于 $temp_root/kekulv-installer-test.*/rootfs" ;;
    esac
}

configure_scope() {
    if [[ -n "$SYSTEM_TEST_ROOT" ]]; then
        validate_system_test_root
        SYSTEMD_SCOPE="system"
        PROGRAM_PARENT="$SYSTEM_ROOT/opt"
        CONFIG_PARENT="$SYSTEM_ROOT/var/lib"
        UNIT_DIR="$SYSTEM_ROOT/etc/systemd/system"
    elif [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
        SYSTEMD_SCOPE="system"
        PROGRAM_PARENT="/opt"
        CONFIG_PARENT="/var/lib"
        UNIT_DIR="/etc/systemd/system"
    else
        [[ -n "${HOME:-}" && "$HOME" == /* && "$HOME" != "/" && "$HOME" != *$'\n'* ]] \
            || die "HOME 必须是非根目录的绝对路径且不能包含换行"
        SYSTEMD_SCOPE="user"
        HOME_REAL="$(realpath -e "$HOME")" || die "无法解析 HOME:$HOME"
        PROGRAM_PARENT="$HOME/.local/share"
        CONFIG_PARENT="$HOME/.config"
        UNIT_DIR="$CONFIG_PARENT/systemd/user"
    fi
    INSTALL_DIR="$PROGRAM_PARENT/kekulv"
    PREVIOUS_DIR="$PROGRAM_PARENT/kekulv.previous"
    CONFIG_DIR="$CONFIG_PARENT/kekulv"
    UNIT_PATH="$UNIT_DIR/kekulv.service"
    UNIT_PREVIOUS="$UNIT_PATH.previous"
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        PROGRAM_PARENT_EXPECTED="$HOME_REAL/.local/share"
        CONFIG_PARENT_EXPECTED="$HOME_REAL/.config"
        UNIT_DIR_EXPECTED="$HOME_REAL/.config/systemd/user"
    else
        PROGRAM_PARENT_EXPECTED="$PROGRAM_PARENT"
        CONFIG_PARENT_EXPECTED="$CONFIG_PARENT"
        UNIT_DIR_EXPECTED="$UNIT_DIR"
    fi
}

run_systemctl() {
    # Avoid empty-array expansion under Bash 3.2 plus `set -u`.
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        systemctl --user "$@"
    else
        systemctl "$@"
    fi
}

resolve_existing_directory() {
    local directory="$1"
    local expected="$2"
    local label="$3"
    if [[ -e "$directory" || -L "$directory" ]]; then
        [[ -d "$directory" && ! -L "$directory" ]] \
            || die "$label 必须是非链接目录:$directory"
        local resolved
        resolved="$(realpath -e "$directory")" || die "无法解析$label:$directory"
        [[ "$resolved" == "$expected" ]] || die "$label 不能是重定向路径:$directory"
        printf '%s\n' "$resolved"
    fi
}

configure_scope
for command_name in dirname realpath rm systemctl; do
    command -v "$command_name" >/dev/null 2>&1 || die "缺少命令:$command_name"
done
PROGRAM_PARENT_REAL="$(resolve_existing_directory "$PROGRAM_PARENT" "$PROGRAM_PARENT_EXPECTED" "程序父目录")"
CONFIG_PARENT_REAL="$(resolve_existing_directory "$CONFIG_PARENT" "$CONFIG_PARENT_EXPECTED" "配置父目录")"
UNIT_DIR_REAL="$(resolve_existing_directory "$UNIT_DIR" "$UNIT_DIR_EXPECTED" "unit 目录")"
if ! run_systemctl show-environment >/dev/null 2>&1; then
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        die "无法连接 systemd user manager（为避免留下运行进程，本次未删除任何文件）。无桌面/SSH 会话可先 loginctl enable-linger \"\$USER\" 后重新登录，或用 sudo 卸载 system 安装"
    fi
    die "无法连接 systemd system manager；为避免留下运行进程，本次未删除任何文件"
fi

is_managed_install() {
    local candidate="$1"
    [[ -d "$candidate" && ! -L "$candidate" \
        && -f "$candidate/kekulvd" && ! -L "$candidate/kekulvd" \
        && -f "$candidate/web/index.html" && ! -L "$candidate/web/index.html" ]]
}

validate_program_tree() {
    local candidate="$1"
    local expected="$2"
    local parent_real=""

    [[ "$candidate" == "$expected" && -n "$candidate" && "$candidate" != "/" \
        && "$candidate" != "$PROGRAM_PARENT" && "$candidate" != "$CONFIG_PARENT" \
        && "$candidate" != "$UNIT_DIR" ]] \
        || die "拒绝删除未授权程序路径:$candidate"
    [[ -e "$candidate" || -L "$candidate" ]] || return 0
    if [[ -L "$candidate" ]]; then
        return 0
    fi
    parent_real="$(realpath -e "$(dirname "$candidate")")" \
        || die "无法解析程序父目录:$candidate"
    [[ -n "$PROGRAM_PARENT_REAL" && "$parent_real" == "$PROGRAM_PARENT_REAL" ]] \
        || die "程序目录不在预期程序目录，拒绝递归删除:$candidate"
    is_managed_install "$candidate" \
        || die "目录不像 kekulv 安装，拒绝递归删除:$candidate"
}

safe_remove_program_tree() {
    local candidate="$1"
    local expected="$2"

    validate_program_tree "$candidate" "$expected"
    [[ -e "$candidate" || -L "$candidate" ]] || return 0
    if [[ -L "$candidate" ]]; then
        note "目标是符号链接，仅删除链接本身:$candidate"
        rm -f -- "$candidate"
        return
    fi
    note "删除程序目录:$candidate"
    rm -rf -- "$candidate"
}

validate_config_tree() {
    local candidate="$1"
    local parent_real=""

    [[ "$PURGE" -eq 1 ]] || die "内部错误:未指定 --purge"
    [[ "$candidate" == "$CONFIG_DIR" && -n "$candidate" \
        && "$candidate" != "/" && "$candidate" != "$CONFIG_PARENT" && "$candidate" != "$UNIT_DIR" ]] \
        || die "拒绝清除未授权配置路径:$candidate"
    [[ -e "$candidate" || -L "$candidate" ]] || return 0
    if [[ -L "$candidate" ]]; then
        return 0
    fi
    [[ -d "$candidate" ]] || die "配置路径不是目录，拒绝删除:$candidate"
    parent_real="$(realpath -e "$(dirname "$candidate")")" \
        || die "无法解析配置父目录:$candidate"
    [[ -n "$CONFIG_PARENT_REAL" && "$parent_real" == "$CONFIG_PARENT_REAL" ]] \
        || die "配置目录不在预期配置目录，拒绝递归删除:$candidate"
}

safe_remove_config_tree() {
    local candidate="$1"

    validate_config_tree "$candidate"
    [[ -e "$candidate" || -L "$candidate" ]] || return 0
    if [[ -L "$candidate" ]]; then
        note "配置目标是符号链接，仅删除链接本身，不跟随目标:$candidate"
        rm -f -- "$candidate"
        return
    fi
    note "--purge 永久删除配置目录:$candidate"
    rm -rf -- "$candidate"
}

# 在停止服务或删除任何内容前一次性验证所有递归删除目标，避免半卸载。
validate_program_tree "$INSTALL_DIR" "$INSTALL_DIR"
validate_program_tree "$PREVIOUS_DIR" "$PREVIOUS_DIR"
if [[ "$PURGE" -eq 1 ]]; then
    validate_config_tree "$CONFIG_DIR"
fi
if [[ -e "$UNIT_PATH" || -L "$UNIT_PATH" || -e "$UNIT_PREVIOUS" || -L "$UNIT_PREVIOUS" ]]; then
    [[ -d "$UNIT_DIR" && ! -L "$UNIT_DIR" && -n "$UNIT_DIR_REAL" \
        && "$(realpath -e "$UNIT_DIR")" == "$UNIT_DIR_REAL" ]] \
        || die "unit 父目录不在预期目录，拒绝删除"
fi

note "停用并停止 kekulv.service"
if ! run_systemctl disable --now kekulv.service; then
    if run_systemctl is-active --quiet kekulv.service; then
        die "服务仍在运行；为避免删除运行中的程序，本次未删除任何文件"
    fi
    note "unit 可能尚未启用；服务当前不在运行，继续卸载"
fi
if run_systemctl is-active --quiet kekulv.service; then
    die "服务仍在运行；本次未删除任何文件"
fi

safe_remove_program_tree "$INSTALL_DIR" "$INSTALL_DIR"
safe_remove_program_tree "$PREVIOUS_DIR" "$PREVIOUS_DIR"

for unit_file in "$UNIT_PATH" "$UNIT_PREVIOUS"; do
    [[ "$unit_file" == "$UNIT_PATH" || "$unit_file" == "$UNIT_PREVIOUS" ]] \
        || die "拒绝删除未授权 unit 路径:$unit_file"
    if [[ -e "$unit_file" || -L "$unit_file" ]]; then
        note "删除 $SYSTEMD_SCOPE unit:$unit_file"
        rm -f -- "$unit_file"
    fi
done
# 安装器写入的 Admin 监听 drop-in（以及用户 systemctl edit 产生的同名目录）。
UNIT_DROPIN_DIR="$UNIT_DIR/kekulv.service.d"
if [[ -e "$UNIT_DROPIN_DIR" || -L "$UNIT_DROPIN_DIR" ]]; then
    [[ -d "$UNIT_DIR" && ! -L "$UNIT_DIR" && -n "$UNIT_DIR_REAL" \
        && "$(realpath -e "$UNIT_DIR")" == "$UNIT_DIR_REAL" ]] \
        || die "unit 父目录不在预期目录，拒绝删除 drop-in"
    if [[ -L "$UNIT_DROPIN_DIR" ]]; then
        note "删除 unit drop-in 符号链接:$UNIT_DROPIN_DIR"
        rm -f -- "$UNIT_DROPIN_DIR"
    elif [[ -d "$UNIT_DROPIN_DIR" ]]; then
        note "删除 unit drop-in 目录:$UNIT_DROPIN_DIR"
        rm -rf -- "$UNIT_DROPIN_DIR"
    else
        die "unit drop-in 路径类型异常:$UNIT_DROPIN_DIR"
    fi
fi
run_systemctl daemon-reload
run_systemctl reset-failed kekulv.service >/dev/null 2>&1 || true

if [[ "$PURGE" -eq 1 ]]; then
    safe_remove_config_tree "$CONFIG_DIR"
else
    note "保留配置:$CONFIG_DIR"
    note "如需永久删除配置，请从发布包或仓库重新运行 scripts/uninstall.sh --purge"
fi

note "卸载完成"
