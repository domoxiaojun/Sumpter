#!/usr/bin/env bash
# Install or upgrade the Linux release package as a user or systemd system service.
set -Eeuo pipefail
umask 077

PROGRAM_NAME="sumpter"
SERVICE_NAME="sumpter.service"
ADMIN_DROPIN_NAME="50-admin-listen.conf"
DEFAULT_ADMIN_HOST="127.0.0.1"
DEFAULT_ADMIN_PORT="57879"
REPOSITORY=""
VERSION=""
# CLI 覆盖（空表示本次安装未指定；未指定时保留已有 drop-in）
ADMIN_HOST_CLI=""
ADMIN_PORT_CLI=""
ADMIN_PASSWORD_FILE_CLI=""
TRANSACTION_ACTIVE=0
PROGRAM_SWAPPED=0
UNIT_SWAPPED=0
HAD_INSTALL=0
HAD_UNIT=0
WAS_ACTIVE=0
WAS_ENABLED=0
DOWNLOAD_DIR=""
STAGE_DIR=""
RESOLVED_PACKAGE_ROOT=""
UNIT_STAGE=""
SYSTEMD_SCOPE=""
SYSTEM_TEST_ROOT="${SUMPTER_SYSTEM_TEST_ROOT:-}"
SYSTEM_ROOT=""
SERVICE_USER=""
SERVICE_GROUP=""
UNIT_SOURCE=""
HOME_REAL=""
PROGRAM_PARENT=""
PROGRAM_PARENT_REAL=""
INSTALL_DIR=""
PREVIOUS_DIR=""
CONFIG_PARENT=""
CONFIG_PARENT_REAL=""
CONFIG_DIR=""
UNIT_DIR=""
UNIT_DIR_REAL=""
UNIT_PATH=""
UNIT_PREVIOUS=""
UNIT_DROPIN_DIR=""
UNIT_DROPIN_FILE=""

die() {
    echo "错误:$*" >&2
    exit 1
}

note() {
    echo "[sumpter-install] $*"
}

usage() {
    cat <<'EOF'
用法:
  install.sh [选项]
      从当前已解压的 sumpter Linux 发布包安装。

  install.sh --repo OWNER/REPO [--version vX.Y.Z] [选项]
      从 GitHub Release 下载当前架构的 tar.gz 与 SHA256SUMS，校验后安装。
      未指定 --version 时使用 latest release。

选项:
  --repo OWNER/REPO     GitHub 仓库，例如 domoxiaojun/sumpter
  --version VERSION     Release tag，例如 v1.2.3；必须与 --repo 同用
  --admin-host IP       Admin 绑定地址（写入 systemd drop-in，默认 127.0.0.1）
  --admin-port PORT     Admin 端口（写入 systemd drop-in，默认 57879）
  --admin-password-file PATH
                        覆盖默认凭据文件路径（写入 systemd drop-in）
  -h, --help            显示帮助

也可用环境变量 SUMPTER_ADMIN_HOST / SUMPTER_ADMIN_PORT /
SUMPTER_ADMIN_PASSWORD_FILE（CLI 优先）。
未指定时保留已有 Admin drop-in；指定后写入 <unit>.d/50-admin-listen.conf。
首次安装会生成 <配置目录>/admin-password（0600），升级与普通卸载均保留。
旧单行格式的初始用户名为 kkl；登录后可在 WebUI 安全页修改用户名和密码。
Admin API/SSE 使用会话 Cookie，公网入口必须使用 HTTPS。

普通用户安装（默认）:
  程序: ~/.local/share/sumpter
  上一版本: ~/.local/share/sumpter.previous
  配置: ~/.config/sumpter（已有 config.json 永不覆盖）
  unit: ~/.config/systemd/user/sumpter.service

root / sudo 安装（system service）:
  程序: /opt/sumpter
  上一版本: /opt/sumpter.previous
  配置和统计: /var/lib/sumpter（已有 config.json 永不覆盖）
  unit: /etc/systemd/system/sumpter.service
  daemon: 专用低权限 sumpter 系统用户，而非 root
EOF
}

is_positive_port() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]] && (($1 <= 65535))
}

validate_ipv4_octets() {
    local ip="$1"
    local IFS=.
    local -a octets
    # shellcheck disable=SC2206
    octets=($ip)
    [[ "${#octets[@]}" -eq 4 ]] || return 1
    local octet
    for octet in "${octets[@]}"; do
        [[ "$octet" =~ ^[0-9]+$ ]] || return 1
        ((10#$octet <= 255)) || return 1
    done
    return 0
}

validate_admin_host_value() {
    local host="$1"
    local lowered
    lowered="$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')"
    case "$lowered" in
        "" | 0.0.0.0 | :: | localhost | 127.0.0.1 | ::1 | "[::1]")
            return 0
            ;;
    esac
    if [[ "$host" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] && validate_ipv4_octets "$host"; then
        return 0
    fi
    # 含冒号视为 IPv6 字面量（daemon 侧会再校验）
    if [[ "$host" == *:* ]]; then
        return 0
    fi
    die "Admin 监听地址无效:${host}（需要 IP、localhost、0.0.0.0 或 ::）"
}

validate_admin_password_file_value() {
    local path="$1"
    [[ "$path" =~ ^/[A-Za-z0-9._/@:+-]+(/[A-Za-z0-9._@:+-]+)*$ ]] \
        || die "Admin 密码文件必须是无空白/转义字符的绝对路径:$path"
}

while (($# > 0)); do
    case "$1" in
        --repo)
            (($# >= 2)) || die "--repo 缺少 OWNER/REPO"
            REPOSITORY="$2"
            shift 2
            ;;
        --version)
            (($# >= 2)) || die "--version 缺少 release tag"
            VERSION="$2"
            shift 2
            ;;
        --admin-host)
            (($# >= 2)) || die "--admin-host 缺少地址"
            ADMIN_HOST_CLI="$2"
            shift 2
            ;;
        --admin-port)
            (($# >= 2)) || die "--admin-port 缺少端口"
            ADMIN_PORT_CLI="$2"
            shift 2
            ;;
        --admin-password-file)
            (($# >= 2)) || die "--admin-password-file 缺少路径"
            ADMIN_PASSWORD_FILE_CLI="$2"
            shift 2
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

if [[ -n "$ADMIN_HOST_CLI" ]]; then
    validate_admin_host_value "$ADMIN_HOST_CLI"
fi
if [[ -n "$ADMIN_PORT_CLI" ]]; then
    is_positive_port "$ADMIN_PORT_CLI" || die "Admin 端口必须是 1...65535:$ADMIN_PORT_CLI"
fi
if [[ -n "$ADMIN_PASSWORD_FILE_CLI" ]]; then
    validate_admin_password_file_value "$ADMIN_PASSWORD_FILE_CLI"
fi
if [[ -z "$ADMIN_HOST_CLI" && -n "${SUMPTER_ADMIN_HOST:-}" ]]; then
    validate_admin_host_value "$SUMPTER_ADMIN_HOST"
fi
if [[ -z "$ADMIN_PORT_CLI" && -n "${SUMPTER_ADMIN_PORT:-}" ]]; then
    is_positive_port "$SUMPTER_ADMIN_PORT" || die "SUMPTER_ADMIN_PORT 必须是 1...65535:$SUMPTER_ADMIN_PORT"
fi
if [[ -z "$ADMIN_PASSWORD_FILE_CLI" && -n "${SUMPTER_ADMIN_PASSWORD_FILE:-}" ]]; then
    validate_admin_password_file_value "$SUMPTER_ADMIN_PASSWORD_FILE"
fi

[[ -z "$VERSION" || -n "$REPOSITORY" ]] || die "--version 必须与 --repo 同用"
if [[ -n "$REPOSITORY" ]]; then
    [[ "$REPOSITORY" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] \
        || die "--repo 必须是 OWNER/REPO，且只能包含字母、数字、点、下划线和连字符"
    owner="${REPOSITORY%%/*}"
    repository_name="${REPOSITORY##*/}"
    [[ "$owner" != "." && "$owner" != ".." && "$repository_name" != "." && "$repository_name" != ".." ]] \
        || die "--repo 不能包含 . 或 .. 路径分量"
fi
if [[ -n "$VERSION" ]]; then
    [[ "$VERSION" =~ ^[vV]?[A-Za-z0-9][A-Za-z0-9._-]*$ ]] \
        || die "--version 含不安全字符"
fi

validate_system_test_root() {
    local temp_root=""
    [[ "${SUMPTER_INSTALLER_SELFTEST:-}" == "1" ]] \
        || die "SUMPTER_SYSTEM_TEST_ROOT 仅允许 installer-selftest 使用"
    [[ "$SYSTEM_TEST_ROOT" == /* && "$SYSTEM_TEST_ROOT" != "/" && -d "$SYSTEM_TEST_ROOT" && ! -L "$SYSTEM_TEST_ROOT" ]] \
        || die "测试 rootfs 必须是已存在的非链接绝对目录"
    SYSTEM_ROOT="$(realpath -e "$SYSTEM_TEST_ROOT")" || die "无法解析测试 rootfs:$SYSTEM_TEST_ROOT"
    temp_root="$(realpath -e "${TMPDIR:-/tmp}")" || die "无法解析 TMPDIR"
    case "$SYSTEM_ROOT" in
        "$temp_root"/sumpter-installer-test.*/rootfs) ;;
        *) die "测试 rootfs 必须位于 $temp_root/sumpter-installer-test.*/rootfs" ;;
    esac
}

configure_scope() {
    if [[ -n "$SYSTEM_TEST_ROOT" ]]; then
        validate_system_test_root
        SYSTEMD_SCOPE="system"
        SERVICE_USER="$(id -un)"
        SERVICE_GROUP="$(id -gn)"
        PROGRAM_PARENT="$SYSTEM_ROOT/opt"
        CONFIG_PARENT="$SYSTEM_ROOT/var/lib"
        UNIT_DIR="$SYSTEM_ROOT/etc/systemd/system"
    elif [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
        SYSTEMD_SCOPE="system"
        SERVICE_USER="sumpter"
        SERVICE_GROUP="sumpter"
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

    INSTALL_DIR="$PROGRAM_PARENT/$PROGRAM_NAME"
    PREVIOUS_DIR="$PROGRAM_PARENT/$PROGRAM_NAME.previous"
    CONFIG_DIR="$CONFIG_PARENT/$PROGRAM_NAME"
    UNIT_PATH="$UNIT_DIR/$SERVICE_NAME"
    UNIT_PREVIOUS="$UNIT_PATH.previous"
    UNIT_DROPIN_DIR="$UNIT_DIR/${SERVICE_NAME}.d"
    UNIT_DROPIN_FILE="$UNIT_DROPIN_DIR/$ADMIN_DROPIN_NAME"
    if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
        UNIT_SOURCE="sumpter-system.service"
    else
        UNIT_SOURCE="sumpter.service"
    fi
}

# 解析本次 CLI/环境变量给出的 Admin 覆盖（空=本次未指定该键）。
resolve_admin_listen_for_unit() {
    if [[ -n "$ADMIN_HOST_CLI" ]]; then
        ADMIN_HOST_WRITE="$ADMIN_HOST_CLI"
    elif [[ -n "${SUMPTER_ADMIN_HOST:-}" ]]; then
        ADMIN_HOST_WRITE="$SUMPTER_ADMIN_HOST"
    else
        ADMIN_HOST_WRITE=""
    fi
    if [[ -n "$ADMIN_PORT_CLI" ]]; then
        ADMIN_PORT_WRITE="$ADMIN_PORT_CLI"
    elif [[ -n "${SUMPTER_ADMIN_PORT:-}" ]]; then
        ADMIN_PORT_WRITE="$SUMPTER_ADMIN_PORT"
    else
        ADMIN_PORT_WRITE=""
    fi
    if [[ -n "$ADMIN_PASSWORD_FILE_CLI" ]]; then
        ADMIN_PASSWORD_FILE_WRITE="$ADMIN_PASSWORD_FILE_CLI"
    elif [[ -n "${SUMPTER_ADMIN_PASSWORD_FILE:-}" ]]; then
        ADMIN_PASSWORD_FILE_WRITE="$SUMPTER_ADMIN_PASSWORD_FILE"
    else
        ADMIN_PASSWORD_FILE_WRITE=""
    fi
}

# 从已有 drop-in 读出 host/port/password file（无则空串）。
read_admin_dropin_values() {
    ADMIN_DROPIN_HOST=""
    ADMIN_DROPIN_PORT=""
    ADMIN_DROPIN_PASSWORD_FILE=""
    local line=""
    [[ -f "$UNIT_DROPIN_FILE" ]] || return 0
    while IFS= read -r line || [[ -n "$line" ]]; do
        case "$line" in
            Environment=SUMPTER_ADMIN_HOST=*)
                ADMIN_DROPIN_HOST="${line#Environment=SUMPTER_ADMIN_HOST=}"
                ;;
            Environment=SUMPTER_ADMIN_PORT=*)
                ADMIN_DROPIN_PORT="${line#Environment=SUMPTER_ADMIN_PORT=}"
                ;;
            Environment=SUMPTER_ADMIN_PASSWORD_FILE=*)
                ADMIN_DROPIN_PASSWORD_FILE="${line#Environment=SUMPTER_ADMIN_PASSWORD_FILE=}"
                ;;
        esac
    done <"$UNIT_DROPIN_FILE"
}

# 生效中的绑定：默认 <- drop-in <- 本次 CLI/env（后者覆盖）。
# 结果写入 ADMIN_EFFECTIVE_HOST / ADMIN_EFFECTIVE_PORT。
resolve_admin_effective_listen() {
    resolve_admin_listen_for_unit
    read_admin_dropin_values
    ADMIN_EFFECTIVE_HOST="$DEFAULT_ADMIN_HOST"
    ADMIN_EFFECTIVE_PORT="$DEFAULT_ADMIN_PORT"
    ADMIN_EFFECTIVE_PASSWORD_FILE=""
    if [[ -n "$ADMIN_DROPIN_HOST" ]]; then
        ADMIN_EFFECTIVE_HOST="$ADMIN_DROPIN_HOST"
    fi
    if [[ -n "$ADMIN_DROPIN_PORT" ]]; then
        ADMIN_EFFECTIVE_PORT="$ADMIN_DROPIN_PORT"
    fi
    if [[ -n "$ADMIN_DROPIN_PASSWORD_FILE" ]]; then
        ADMIN_EFFECTIVE_PASSWORD_FILE="$ADMIN_DROPIN_PASSWORD_FILE"
    fi
    if [[ -n "$ADMIN_HOST_WRITE" ]]; then
        ADMIN_EFFECTIVE_HOST="$ADMIN_HOST_WRITE"
    fi
    if [[ -n "$ADMIN_PORT_WRITE" ]]; then
        ADMIN_EFFECTIVE_PORT="$ADMIN_PORT_WRITE"
    fi
    if [[ -n "$ADMIN_PASSWORD_FILE_WRITE" ]]; then
        ADMIN_EFFECTIVE_PASSWORD_FILE="$ADMIN_PASSWORD_FILE_WRITE"
    fi
}

validate_admin_password_override() {
    resolve_admin_effective_listen
    if [[ -n "$ADMIN_PASSWORD_FILE_WRITE" ]]; then
        [[ -f "$ADMIN_PASSWORD_FILE_WRITE" ]] \
            || die "Admin 密码文件不存在或不是普通文件:$ADMIN_PASSWORD_FILE_WRITE"
    fi
}

# 本机可打开的 URL 主机（0.0.0.0/:: 回落 127.0.0.1；指定 IP 原样）。
admin_access_host() {
    local host="$1"
    case "$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')" in
        "" | 0.0.0.0 | :: | localhost) printf '%s\n' "127.0.0.1" ;;
        *) printf '%s\n' "$host" ;;
    esac
}

admin_format_url() {
    local host="$1"
    local port="$2"
    local authority
    authority="$(admin_access_host "$host")"
    if [[ "$authority" == *:* && "$authority" != \[* ]]; then
        authority="[$authority]"
    fi
    printf 'http://%s:%s/admin/\n' "$authority" "$port"
}

# 有指定时写入 drop-in；只改 host 或 port 时合并已有另一项，避免冲掉。
# 未指定则完整保留已有 drop-in。
apply_admin_listen_unit_override() {
    resolve_admin_listen_for_unit
    if [[ -z "$ADMIN_HOST_WRITE" && -z "$ADMIN_PORT_WRITE" && -z "$ADMIN_PASSWORD_FILE_WRITE" ]]; then
        if [[ -f "$UNIT_DROPIN_FILE" ]]; then
            note "保留已有 Admin 监听覆盖:$UNIT_DROPIN_FILE"
        fi
        return 0
    fi

    read_admin_dropin_values
    local write_host="$ADMIN_HOST_WRITE"
    local write_port="$ADMIN_PORT_WRITE"
    local write_password_file="$ADMIN_PASSWORD_FILE_WRITE"
    # 只传一侧时，另一侧从旧 drop-in 继承（没有则不写该键，daemon 用默认）。
    if [[ -z "$write_host" && -n "$ADMIN_DROPIN_HOST" ]]; then
        write_host="$ADMIN_DROPIN_HOST"
    fi
    if [[ -z "$write_port" && -n "$ADMIN_DROPIN_PORT" ]]; then
        write_port="$ADMIN_DROPIN_PORT"
    fi
    if [[ -z "$write_password_file" && -n "$ADMIN_DROPIN_PASSWORD_FILE" ]]; then
        write_password_file="$ADMIN_DROPIN_PASSWORD_FILE"
    fi

    ensure_directory "$UNIT_DROPIN_DIR" 0755
    {
        echo "# Managed by sumpter install.sh. Re-run install with --admin-host/--admin-port/--admin-password-file to update."
        echo "[Service]"
        if [[ -n "$write_host" ]]; then
            printf 'Environment=SUMPTER_ADMIN_HOST=%s\n' "$write_host"
        fi
        if [[ -n "$write_port" ]]; then
            printf 'Environment=SUMPTER_ADMIN_PORT=%s\n' "$write_port"
        fi
        if [[ -n "$write_password_file" ]]; then
            printf 'Environment=SUMPTER_ADMIN_PASSWORD_FILE=%s\n' "$write_password_file"
        fi
    } >"$UNIT_DROPIN_FILE"
    chmod 0644 "$UNIT_DROPIN_FILE"
    note "已写入 Admin 监听覆盖:$UNIT_DROPIN_FILE"
    if [[ -n "$write_host" ]]; then
        note "  SUMPTER_ADMIN_HOST=$write_host"
    fi
    if [[ -n "$write_port" ]]; then
        note "  SUMPTER_ADMIN_PORT=$write_port"
    fi
    if [[ -n "$write_password_file" ]]; then
        note "  SUMPTER_ADMIN_PASSWORD_FILE=$write_password_file"
    fi
}

# 打印安装结束时的 Admin 绑定与访问提示（多行 note）。
note_admin_access_summary() {
    resolve_admin_effective_listen
    local bind="${ADMIN_EFFECTIVE_HOST}:${ADMIN_EFFECTIVE_PORT}"
    local password_file="${ADMIN_EFFECTIVE_PASSWORD_FILE:-$CONFIG_DIR/admin-password}"
    local url
    url="$(admin_format_url "$ADMIN_EFFECTIVE_HOST" "$ADMIN_EFFECTIVE_PORT")"
    note "Admin 绑定:$bind"
    note "Web 管理:$url"
    note "Admin 登录:WebUI 内置登录（初始用户名 kkl；凭据文件 ${password_file}）"
    note "正常改用户名/密码请使用 WebUI 安全页；忘记凭据时覆盖为新的单行密码并重启。"
    case "$(printf '%s' "$ADMIN_EFFECTIVE_HOST" | tr '[:upper:]' '[:lower:]')" in
        "" | 0.0.0.0 | ::)
            note "提示:绑定全接口时本机请用上述 127.0.0.1 URL；局域网用 http://<主机IP>:${ADMIN_EFFECTIVE_PORT}/admin/"
            ;;
    esac
}

run_systemctl() {
    # Keep the two contracts explicit instead of expanding an empty array. Bash
    # 3.2 with `set -u` treats an empty indexed-array expansion as unbound.
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        systemctl --user "$@"
    else
        systemctl "$@"
    fi
}

ensure_directory() {
    local directory="$1"
    local mode="$2"
    if [[ -e "$directory" || -L "$directory" ]]; then
        [[ -d "$directory" && ! -L "$directory" ]] \
            || die "路径必须是非链接目录:$directory"
    else
        install -d -m "$mode" "$directory"
    fi
}

ensure_admin_password_file() {
    resolve_admin_effective_listen
    local password_file="${ADMIN_EFFECTIVE_PASSWORD_FILE:-$CONFIG_DIR/admin-password}"
    local generated_password=""
    local staged_password=""

    # 显式 CLI/env/drop-in 路径由管理员管理；安装器只确认它已经是普通文件。
    if [[ -n "$ADMIN_EFFECTIVE_PASSWORD_FILE" ]]; then
        if [[ "$password_file" == /* ]]; then
            [[ -f "$password_file" ]] \
                || die "Admin 密码文件不存在或不是普通文件:$password_file"
        fi
        note "使用自定义 Admin 密码文件:$password_file"
        return 0
    fi

    if [[ -e "$password_file" || -L "$password_file" ]]; then
        [[ -f "$password_file" && ! -L "$password_file" ]] \
            || die "默认 Admin 密码文件必须是非链接普通文件:$password_file"
        chmod 0600 "$password_file"
        if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
            chown "$SERVICE_USER:$SERVICE_GROUP" "$password_file"
        fi
        note "保留已有 Admin 密码文件:$password_file"
        return 0
    fi

    generated_password="$(od -An -N24 -tx1 /dev/urandom | tr -d '[:space:]')"
    [[ "${#generated_password}" -eq 48 ]] || die "无法生成 Admin 随机密码"
    staged_password="$(mktemp "$CONFIG_DIR/.admin-password.XXXXXX")"
    printf '%s\n' "$generated_password" >"$staged_password"
    generated_password=""
    chmod 0600 "$staged_password"
    if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
        chown "$SERVICE_USER:$SERVICE_GROUP" "$staged_password"
    fi
    mv -- "$staged_password" "$password_file"
    note "已生成 Admin 密码文件:$password_file"
}

# Fedora/RHEL 常见 /sbin/nologin，Debian/Ubuntu 常见 /usr/sbin/nologin。
resolve_nologin_shell() {
    local candidate=""
    for candidate in /usr/sbin/nologin /sbin/nologin /usr/bin/nologin; do
        if [[ -x "$candidate" ]]; then
            printf '%s\n' "$candidate"
            return 0
        fi
    done
    # 最后回退：useradd 仍可能接受该路径字面量。
    printf '%s\n' "/usr/sbin/nologin"
}

selinux_enabled() {
    command -v getenforce >/dev/null 2>&1 || return 1
    case "$(getenforce 2>/dev/null || true)" in
        Enforcing | Permissive) return 0 ;;
        *) return 1 ;;
    esac
}

# 从 /tmp 下载或 cp -a 时可能带上 tmp_t；在 Fedora enforcing 下会导致 203/EXEC。
relabel_path_for_selinux() {
    local target="$1"
    [[ -z "$SYSTEM_TEST_ROOT" ]] || return 0
    selinux_enabled || return 0
    [[ -e "$target" ]] || return 0
    if command -v restorecon >/dev/null 2>&1; then
        note "SELinux: restorecon $target"
        restorecon -RF -- "$target" >/dev/null 2>&1 \
            || note "警告:restorecon 未完全成功，服务若 203/EXEC 请手动: restorecon -RF $target"
    else
        note "警告:SELinux 已启用但缺少 restorecon（安装 policycoreutils）。若服务无法启动: chcon -t bin_t $INSTALL_DIR/sumpterd"
    fi
}

copy_tree_item() {
    local src="$1"
    local dst_parent="$2"
    # 避免把下载目录的 SELinux 标签（常为 tmp_t）带进 /opt 或 ~/.local。
    if selinux_enabled && [[ -z "$SYSTEM_TEST_ROOT" ]]; then
        cp -a --no-preserve=context -- "$src" "$dst_parent/"
    else
        cp -a -- "$src" "$dst_parent/"
    fi
}

dump_service_failure_hints() {
    note "服务未能进入 active，最近日志:"
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        journalctl --user -u "$SERVICE_NAME" -n 40 --no-pager 2>/dev/null || true
    else
        journalctl -u "$SERVICE_NAME" -n 40 --no-pager 2>/dev/null || true
    fi
    if selinux_enabled; then
        note "SELinux=$(getenforce 2>/dev/null || echo unknown)。若日志含 status=203/EXEC 或 Permission denied:"
        note "  restorecon -RF $INSTALL_DIR"
        note "  # 或: chcon -t bin_t $INSTALL_DIR/sumpterd"
        if command -v ausearch >/dev/null 2>&1; then
            ausearch -m avc -ts recent 2>/dev/null | tail -n 15 || true
        fi
    fi
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        note "user 服务还需要可用的用户 systemd 会话；无桌面/SSH 无 linger 时可:"
        note "  loginctl enable-linger \"\$USER\"  # 需管理员策略允许，然后重新登录"
    fi
}

ensure_system_account() {
    [[ "$SYSTEMD_SCOPE" == "system" && -z "$SYSTEM_TEST_ROOT" ]] || return 0
    local nologin_shell=""
    if id -u "$SERVICE_USER" >/dev/null 2>&1; then
        [[ "$(id -gn "$SERVICE_USER")" == "$SERVICE_GROUP" ]] \
            || die "已有系统用户 ${SERVICE_USER} 的主组不是 ${SERVICE_GROUP}，拒绝复用"
        return 0
    fi
    nologin_shell="$(resolve_nologin_shell)"
    if command -v useradd >/dev/null 2>&1; then
        # shadow-utils（Fedora/RHEL/Debian 通用）：系统用户 + 同名主组，不强制创建 home。
        useradd --system --user-group --home-dir /var/lib/sumpter --shell "$nologin_shell" "$SERVICE_USER"
    elif command -v adduser >/dev/null 2>&1; then
        adduser --system --group --no-create-home --home /var/lib/sumpter --shell "$nologin_shell" "$SERVICE_USER"
    else
        die "root 安装需要 useradd 或 adduser 来创建低权限系统用户 $SERVICE_USER"
    fi
    [[ "$(id -gn "$SERVICE_USER")" == "$SERVICE_GROUP" ]] \
        || die "无法创建主组为 $SERVICE_GROUP 的系统用户 $SERVICE_USER"
}

configure_scope
validate_admin_password_override
for command_name in chmod cp dirname find grep id install mktemp mv od realpath rm systemctl tr; do
    command -v "$command_name" >/dev/null 2>&1 || die "缺少命令:$command_name"
done
if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
    command -v chown >/dev/null 2>&1 || die "root/system 安装缺少命令:chown"
fi
ensure_system_account

if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
    ensure_directory "$HOME/.local" 0755
    ensure_directory "$PROGRAM_PARENT" 0755
    ensure_directory "$CONFIG_PARENT" 0755
    ensure_directory "$CONFIG_PARENT/systemd" 0755
    ensure_directory "$UNIT_DIR" 0755
else
    ensure_directory "$PROGRAM_PARENT" 0755
    ensure_directory "$CONFIG_PARENT" 0755
    ensure_directory "$UNIT_DIR" 0755
fi
PROGRAM_PARENT_REAL="$(realpath -e "$PROGRAM_PARENT")" || die "无法解析程序父目录:$PROGRAM_PARENT"
CONFIG_PARENT_REAL="$(realpath -e "$CONFIG_PARENT")" || die "无法解析配置父目录:$CONFIG_PARENT"
UNIT_DIR_REAL="$(realpath -e "$UNIT_DIR")" || die "无法解析 unit 目录:$UNIT_DIR"
if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
    [[ "$PROGRAM_PARENT_REAL" == "$HOME_REAL/.local/share" ]] \
        || die "程序父目录必须位于真实 HOME 内，拒绝跟随重定向路径:$PROGRAM_PARENT"
    [[ "$CONFIG_PARENT_REAL" == "$HOME_REAL/.config" ]] \
        || die "配置父目录必须位于真实 HOME 内，拒绝跟随重定向路径:$CONFIG_PARENT"
    [[ "$UNIT_DIR_REAL" == "$HOME_REAL/.config/systemd/user" ]] \
        || die "unit 目录必须位于真实 HOME 内，拒绝跟随重定向路径:$UNIT_DIR"
else
    [[ "$PROGRAM_PARENT_REAL" == "$PROGRAM_PARENT" && "$CONFIG_PARENT_REAL" == "$CONFIG_PARENT" && "$UNIT_DIR_REAL" == "$UNIT_DIR" ]] \
        || die "system 安装目录不能是重定向路径"
fi
if ! run_systemctl show-environment >/dev/null 2>&1; then
    if [[ "$SYSTEMD_SCOPE" == "user" ]]; then
        die "无法连接 systemd user manager（本次未修改程序文件）。Fedora/RHEL 无桌面 SSH 会话常见此问题：先 loginctl enable-linger \"\$USER\" 后重新登录，或改用 sudo 安装 system 服务"
    fi
    die "无法连接 systemd system manager；本次未修改程序文件"
fi

is_managed_install() {
    local candidate="$1"
    [[ -d "$candidate" && ! -L "$candidate" \
        && -f "$candidate/sumpterd" && ! -L "$candidate/sumpterd" \
        && -f "$candidate/web/index.html" && ! -L "$candidate/web/index.html" ]]
}

safe_remove_tree() {
    local candidate="$1"
    local expected="$2"
    local label="$3"
    local parent_real=""

    [[ "$candidate" == "$expected" && -n "$candidate" && "$candidate" != "/" \
        && "$candidate" != "$PROGRAM_PARENT" && "$candidate" != "$CONFIG_PARENT" \
        && "$candidate" != "$UNIT_DIR" ]] \
        || die "拒绝删除未授权路径:$candidate"
    [[ -e "$candidate" || -L "$candidate" ]] || return 0
    [[ ! -L "$candidate" ]] || die "拒绝递归删除符号链接:$candidate"
    parent_real="$(realpath -e "$(dirname "$candidate")")" || die "无法解析删除目标父目录:$candidate"
    [[ "$parent_real" == "$PROGRAM_PARENT_REAL" ]] \
        || die "删除目标不在预期程序目录:$candidate"
    is_managed_install "$candidate" || die "目录不像 sumpter 安装，拒绝删除:$candidate"
    note "删除$label:$candidate"
    rm -rf -- "$candidate"
}

safe_remove_temp() {
    local candidate="$1"
    local allowed_prefix="$2"
    [[ -n "$candidate" && "$candidate" == "$allowed_prefix"* && "$candidate" != "$allowed_prefix" \
        && "$candidate" != "/" ]] || return 0
    if [[ -d "$candidate" && ! -L "$candidate" ]]; then
        rm -rf -- "$candidate"
    fi
}

rollback() {
    local rollback_ok=1
    set +e
    note "安装未完成，开始恢复上一状态"

    if [[ "$UNIT_SWAPPED" -eq 1 ]]; then
        rm -f -- "$UNIT_PATH"
        if [[ "$HAD_UNIT" -eq 1 && -f "$UNIT_PREVIOUS" && ! -L "$UNIT_PREVIOUS" ]]; then
            mv -- "$UNIT_PREVIOUS" "$UNIT_PATH" || rollback_ok=0
        fi
    fi

    if [[ "$PROGRAM_SWAPPED" -eq 1 ]]; then
        if [[ -e "$INSTALL_DIR" ]]; then
            safe_remove_tree "$INSTALL_DIR" "$INSTALL_DIR" "失败的新版本" || rollback_ok=0
        fi
        if [[ "$HAD_INSTALL" -eq 1 && -d "$PREVIOUS_DIR" && ! -L "$PREVIOUS_DIR" ]]; then
            mv -- "$PREVIOUS_DIR" "$INSTALL_DIR" || rollback_ok=0
        fi
    fi

    run_systemctl daemon-reload >/dev/null 2>&1 || rollback_ok=0
    if [[ "$WAS_ENABLED" -eq 1 ]]; then
        run_systemctl enable "$SERVICE_NAME" >/dev/null 2>&1 || rollback_ok=0
    else
        run_systemctl disable "$SERVICE_NAME" >/dev/null 2>&1 || true
    fi
    if [[ "$WAS_ACTIVE" -eq 1 ]]; then
        run_systemctl start "$SERVICE_NAME" >/dev/null 2>&1 || rollback_ok=0
    elif [[ "$HAD_INSTALL" -eq 0 && "$HAD_UNIT" -eq 0 ]]; then
        run_systemctl stop "$SERVICE_NAME" >/dev/null 2>&1 || true
    fi

    TRANSACTION_ACTIVE=0
    if [[ "$rollback_ok" -eq 1 ]]; then
        echo "已恢复安装前状态。" >&2
    else
        echo "警告:自动恢复不完整，请检查 ${INSTALL_DIR}、${UNIT_PATH} 和 systemd ${SYSTEMD_SCOPE} 状态。" >&2
    fi
    set -e
}

finish() {
    local status=$?
    trap - EXIT
    if [[ "$status" -ne 0 && "$TRANSACTION_ACTIVE" -eq 1 ]]; then
        rollback
    fi
    if [[ -n "$STAGE_DIR" ]]; then
        safe_remove_temp "$STAGE_DIR" "$PROGRAM_PARENT/.sumpter.stage."
    fi
    if [[ -n "$UNIT_STAGE" && "$UNIT_STAGE" == "$UNIT_DIR/.sumpter.service."* ]]; then
        rm -f -- "$UNIT_STAGE"
    fi
    if [[ -n "$DOWNLOAD_DIR" ]]; then
        safe_remove_temp "$DOWNLOAD_DIR" "${TMPDIR:-/tmp}/sumpter-download."
    fi
    exit "$status"
}
trap finish EXIT

detect_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo "x86_64" ;;
        aarch64 | arm64) echo "aarch64" ;;
        *) die "不支持的架构:$(uname -m)（仅支持 x86_64/aarch64）" ;;
    esac
}

download_file() {
    local url="$1"
    local destination="$2"
    curl --fail --location --silent --show-error \
        --proto '=https' --tlsv1.2 --retry 3 --connect-timeout 15 --max-time 600 \
        --output "$destination" "$url"
}

validate_archive_paths() {
    local archive="$1"
    local entry=""
    local component=""
    local -a components=()

    tar -tzf "$archive" >/dev/null || die "tar.gz 无法读取:$archive"
    if tar -tvzf "$archive" | awk '
        {
            type = substr($1, 1, 1)
            if (type != "-" && type != "d") invalid = 1
        }
        END { exit invalid ? 0 : 1 }
    '; then
        die "发布包只能包含普通文件和目录，拒绝链接、设备或 FIFO"
    fi
    while IFS= read -r entry; do
        entry="${entry#./}"
        [[ -n "$entry" ]] || continue
        [[ "$entry" != /* ]] || die "压缩包包含绝对路径:$entry"
        IFS='/' read -r -a components <<<"$entry"
        for component in "${components[@]}"; do
            [[ "$component" != ".." ]] || die "压缩包包含路径穿越:$entry"
        done
    done < <(tar -tzf "$archive")
}

resolve_remote_package() {
    local arch=""
    local asset=""
    local base_url=""
    local archive=""
    local sums=""
    local expected_hash=""
    local actual_hash=""
    local -a hashes=()
    local -a roots=()

    for command_name in awk curl sha256sum tar uname; do
        command -v "$command_name" >/dev/null 2>&1 || die "远程安装缺少命令:$command_name"
    done
    arch="$(detect_arch)"
    asset="sumpter-linux-${arch}.tar.gz"
    if [[ -n "$VERSION" ]]; then
        base_url="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
    else
        base_url="https://github.com/${REPOSITORY}/releases/latest/download"
    fi

    DOWNLOAD_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sumpter-download.XXXXXX")"
    archive="$DOWNLOAD_DIR/$asset"
    sums="$DOWNLOAD_DIR/SHA256SUMS"
    note "下载:$base_url/$asset"
    download_file "$base_url/$asset" "$archive"
    download_file "$base_url/SHA256SUMS" "$sums"

    mapfile -t hashes < <(awk -v wanted="$asset" '
        {
            name = $2
            sub(/^\*/, "", name)
            sub(/^\.\//, "", name)
            sub(/\r$/, "", name)
            if (name == wanted) print $1
        }
    ' "$sums")
    [[ "${#hashes[@]}" -eq 1 && "${hashes[0]}" =~ ^[0-9A-Fa-f]{64}$ ]] \
        || die "SHA256SUMS 必须且只能包含一条 $asset 的 64 位校验值"
    expected_hash="${hashes[0],,}"
    actual_hash="$(sha256sum "$archive" | awk '{print tolower($1)}')"
    [[ "$actual_hash" == "$expected_hash" ]] \
        || die "SHA-256 校验失败:$asset"
    note "SHA-256 校验通过:$actual_hash"

    validate_archive_paths "$archive"
    install -d -m 0700 "$DOWNLOAD_DIR/extracted"
    tar --no-same-owner --no-same-permissions -xzf "$archive" -C "$DOWNLOAD_DIR/extracted"
    if find "$DOWNLOAD_DIR/extracted" -type l -print -quit | grep -q .; then
        die "发布包包含符号链接，拒绝安装"
    fi

    if [[ -f "$DOWNLOAD_DIR/extracted/sumpterd" ]]; then
        RESOLVED_PACKAGE_ROOT="$DOWNLOAD_DIR/extracted"
        return
    fi
    mapfile -d '' -t roots < <(find "$DOWNLOAD_DIR/extracted" -mindepth 1 -maxdepth 1 -type d -print0)
    [[ "${#roots[@]}" -eq 1 && -f "${roots[0]}/sumpterd" ]] \
        || die "发布包必须直接包含文件，或仅包含一个顶层目录"
    RESOLVED_PACKAGE_ROOT="${roots[0]}"
}

validate_package() {
    local package_root="$1"
    local required=""
    local description=""
    local arch=""

    for required in sumpterd config.example.json sumpter.service sumpter-system.service web/index.html scripts/install.sh scripts/uninstall.sh scripts/cc-project-attribution.sh scripts/grok-project-attribution.sh scripts/pi-project-attribution.ts scripts/gemini-sumpter-wrapper.mjs scripts/client-attribution.mjs scripts/setup-client-attribution.sh; do
        [[ -f "$package_root/$required" && ! -L "$package_root/$required" ]] \
            || die "发布包缺少普通文件:$required"
    done
    [[ -x "$package_root/sumpterd" ]] || die "发布包中的 sumpterd 不可执行"
    for required in sumpterd web scripts config.example.json sumpter.service sumpter-system.service; do
        if find "$package_root/$required" -type l -print -quit | grep -q .; then
            die "发布包项目包含符号链接:$required"
        fi
    done
    grep -Fq 'WorkingDirectory=%h/.local/share/sumpter' "$package_root/sumpter.service" \
        || die "sumpter.service 的 WorkingDirectory 与安装器布局不一致"
    grep -Fq 'ExecStart=%h/.local/share/sumpter/sumpterd --systemd-scope user --config-dir %h/.config/sumpter --web-root %h/.local/share/sumpter/web' "$package_root/sumpter.service" \
        || die "sumpter.service 的 ExecStart 与安装器布局不一致"
    grep -Fq 'WorkingDirectory=/opt/sumpter' "$package_root/sumpter-system.service" \
        || die "sumpter-system.service 的 WorkingDirectory 与安装器布局不一致"
    grep -Fq 'User=sumpter' "$package_root/sumpter-system.service" \
        || die "sumpter-system.service 必须以低权限 sumpter 用户运行"
    grep -Fq 'ExecStart=/opt/sumpter/sumpterd --systemd-scope system --config-dir /var/lib/sumpter --web-root /opt/sumpter/web' "$package_root/sumpter-system.service" \
        || die "sumpter-system.service 的 ExecStart 与安装器布局不一致"

    if command -v file >/dev/null 2>&1; then
        description="$(file -b "$package_root/sumpterd")"
        [[ "$description" == *ELF* ]] || die "sumpterd 不是 Linux ELF:$description"
        arch="$(detect_arch)"
        if [[ "$arch" == "x86_64" ]]; then
            [[ "$description" == *"x86-64"* || "$description" == *"x86_64"* ]] \
                || die "sumpterd 架构与本机 x86_64 不匹配:$description"
        else
            [[ "$description" == *"aarch64"* || "$description" == *"ARM64"* ]] \
                || die "sumpterd 架构与本机 aarch64 不匹配:$description"
        fi
    fi
}

copy_package_to_stage() {
    local package_root="$1"
    local item=""

    STAGE_DIR="$(mktemp -d "$PROGRAM_PARENT/.sumpter.stage.XXXXXX")"
    for item in sumpterd web scripts config.example.json sumpter.service sumpter-system.service; do
        copy_tree_item "$package_root/$item" "$STAGE_DIR"
    done
    for item in README.md LICENSE NOTICE THIRD_PARTY_LICENSES specs; do
        if [[ -e "$package_root/$item" && ! -L "$package_root/$item" ]]; then
            if find "$package_root/$item" -type l -print -quit | grep -q .; then
                die "发布包可选项目包含符号链接:$item"
            fi
            copy_tree_item "$package_root/$item" "$STAGE_DIR"
        fi
    done
    find "$STAGE_DIR" -type d -exec chmod 0755 {} +
    find "$STAGE_DIR" -type f -exec chmod 0644 {} +
    chmod 0755 "$STAGE_DIR/sumpterd"
    find "$STAGE_DIR/scripts" -type f -name '*.sh' -exec chmod 0755 {} +
    # 在 mv 进最终路径前先按目标父目录策略打标签，避免 Fedora 上带 tmp_t。
    relabel_path_for_selinux "$STAGE_DIR"
}

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
PACKAGE_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd -P)"
if [[ -n "$REPOSITORY" ]]; then
    resolve_remote_package
    PACKAGE_ROOT="$RESOLVED_PACKAGE_ROOT"
fi
validate_package "$PACKAGE_ROOT"
copy_package_to_stage "$PACKAGE_ROOT"

if [[ -e "$INSTALL_DIR" || -L "$INSTALL_DIR" ]]; then
    is_managed_install "$INSTALL_DIR" \
        || die "目标已存在但不是可识别的 sumpter 安装，拒绝覆盖:$INSTALL_DIR"
    HAD_INSTALL=1
fi
if [[ -e "$PREVIOUS_DIR" || -L "$PREVIOUS_DIR" ]]; then
    safe_remove_tree "$PREVIOUS_DIR" "$PREVIOUS_DIR" "更早的恢复版本"
fi
if [[ -L "$UNIT_PATH" || -L "$UNIT_PREVIOUS" ]]; then
    die "unit 路径不能是符号链接:$UNIT_PATH"
fi
if [[ -e "$UNIT_PATH" ]]; then
    [[ -f "$UNIT_PATH" ]] || die "unit 路径不是普通文件:$UNIT_PATH"
    HAD_UNIT=1
fi
if [[ -e "$UNIT_PREVIOUS" ]]; then
    [[ -f "$UNIT_PREVIOUS" ]] || die "unit 备份路径不是普通文件:$UNIT_PREVIOUS"
    note "删除更早的 unit 备份:$UNIT_PREVIOUS"
    rm -f -- "$UNIT_PREVIOUS"
fi

run_systemctl is-active --quiet "$SERVICE_NAME" && WAS_ACTIVE=1 || true
run_systemctl is-enabled --quiet "$SERVICE_NAME" && WAS_ENABLED=1 || true
if [[ "$WAS_ACTIVE" -eq 1 ]]; then
    note "停止当前服务"
    run_systemctl stop "$SERVICE_NAME" \
        || die "无法停止当前服务；程序文件尚未更改"
fi

TRANSACTION_ACTIVE=1
if [[ "$HAD_INSTALL" -eq 1 ]]; then
    mv -- "$INSTALL_DIR" "$PREVIOUS_DIR"
fi
PROGRAM_SWAPPED=1
mv -- "$STAGE_DIR" "$INSTALL_DIR"
STAGE_DIR=""

if [[ "$HAD_UNIT" -eq 1 ]]; then
    mv -- "$UNIT_PATH" "$UNIT_PREVIOUS"
fi
UNIT_SWAPPED=1
UNIT_STAGE="$(mktemp "$UNIT_DIR/.sumpter.service.XXXXXX")"
    install -m 0644 "$INSTALL_DIR/$UNIT_SOURCE" "$UNIT_STAGE"
mv -f -- "$UNIT_STAGE" "$UNIT_PATH"
UNIT_STAGE=""
# 在 daemon-reload 前写入/保留 Admin 监听 drop-in。
apply_admin_listen_unit_override

if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
    install -d -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0700 "$CONFIG_DIR"
else
    install -d -m 0700 "$CONFIG_DIR"
fi
if [[ -e "$CONFIG_DIR/config.json" || -L "$CONFIG_DIR/config.json" ]]; then
    [[ -f "$CONFIG_DIR/config.json" && ! -L "$CONFIG_DIR/config.json" ]] \
        || die "已有 config.json 不是普通文件，拒绝启动:$CONFIG_DIR/config.json"
    chmod 0600 "$CONFIG_DIR/config.json"
    if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
        chown "$SERVICE_USER:$SERVICE_GROUP" "$CONFIG_DIR/config.json"
    fi
    note "保留已有配置:$CONFIG_DIR/config.json"
else
    if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
        install -o "$SERVICE_USER" -g "$SERVICE_GROUP" -m 0600 "$INSTALL_DIR/config.example.json" "$CONFIG_DIR/config.json"
    else
        install -m 0600 "$INSTALL_DIR/config.example.json" "$CONFIG_DIR/config.json"
    fi
    note "初始化停用示例配置:$CONFIG_DIR/config.json"
fi
ensure_admin_password_file

# 最终路径再 restorecon 一次（mv 后 inode 路径变化，策略按路径匹配）。
relabel_path_for_selinux "$INSTALL_DIR"
if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
    # 确保 sumpter 用户能遍历程序目录读 web 静态资源。
    chmod 0755 "$INSTALL_DIR"
    find "$INSTALL_DIR" -type d -exec chmod 0755 {} +
    find "$INSTALL_DIR" -type f -exec chmod 0644 {} +
    chmod 0755 "$INSTALL_DIR/sumpterd"
    find "$INSTALL_DIR/scripts" -type f -name '*.sh' -exec chmod 0755 {} + 2>/dev/null || true
fi

run_systemctl daemon-reload
run_systemctl enable "$SERVICE_NAME"
if ! run_systemctl start "$SERVICE_NAME"; then
    dump_service_failure_hints
    die "无法启动 $SERVICE_NAME"
fi
if ! run_systemctl is-active --quiet "$SERVICE_NAME"; then
    dump_service_failure_hints
    die "新版本未进入 active 状态"
fi

TRANSACTION_ACTIVE=0
note "安装完成:$INSTALL_DIR"
note "服务已启用并运行:$SERVICE_NAME"
note "配置:$CONFIG_DIR/config.json"
if [[ "$HAD_INSTALL" -eq 1 ]]; then
    note "上一版本保留于:$PREVIOUS_DIR"
fi
note_admin_access_summary
if [[ "$SYSTEMD_SCOPE" == "system" ]]; then
    note "system 服务管理: sudo systemctl status|restart|stop $SERVICE_NAME"
else
    note "user 服务管理: systemctl --user status|restart|stop $SERVICE_NAME"
fi
note "改登录凭据:使用 WebUI 安全页；忘记凭据时覆盖 ${ADMIN_EFFECTIVE_PASSWORD_FILE:-$CONFIG_DIR/admin-password} 为新单行密码并重启"
note "改 Admin 监听/高级密码路径:重装时加 --admin-host/--admin-port/--admin-password-file，或用 systemctl edit 写 Environment=SUMPTER_ADMIN_*"
