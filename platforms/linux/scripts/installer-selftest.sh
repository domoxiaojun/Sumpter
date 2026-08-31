#!/usr/bin/env bash
# Exercise user and system install transactions without touching real systemd.
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
TEST_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEST_BASE/sumpter-installer-test.XXXXXX")"
USER_HOME="$TEST_ROOT/home"
SYSTEM_ROOT="$TEST_ROOT/rootfs"
TEST_PACKAGE="$TEST_ROOT/package"
STUB_BIN="$TEST_ROOT/bin"
STUB_STATE_DIR="$TEST_ROOT/systemctl-state"
ADMIN_PASSWORD_FILE="$TEST_ROOT/admin-password"
TRUE_BIN=""
FALSE_BIN=""
REALPATH_BIN="/usr/bin/realpath"
[[ -x "$REALPATH_BIN" ]] || REALPATH_BIN="/bin/realpath"

cleanup() {
    case "$TEST_ROOT" in
        "$TEST_BASE"/sumpter-installer-test.*)
            [[ -d "$TEST_ROOT" && ! -L "$TEST_ROOT" ]] && rm -rf -- "$TEST_ROOT"
            ;;
        *)
            echo "拒绝清理非测试目录:$TEST_ROOT" >&2
            ;;
    esac
}
trap cleanup EXIT

mkdir -p "$USER_HOME" "$SYSTEM_ROOT" "$TEST_PACKAGE/scripts" "$TEST_PACKAGE/web" "$STUB_BIN" "$STUB_STATE_DIR"
for candidate in /usr/bin/true /bin/true; do
    if [[ -x "$candidate" ]]; then
        TRUE_BIN="$candidate"
        break
    fi
done
for candidate in /usr/bin/false /bin/false; do
    if [[ -x "$candidate" ]]; then
        FALSE_BIN="$candidate"
        break
    fi
done
[[ -n "$TRUE_BIN" ]] || {
    echo "找不到可执行的 true 测试夹具" >&2
    exit 1
}
[[ -n "$FALSE_BIN" ]] || {
    echo "找不到可执行的 false 测试夹具" >&2
    exit 1
}
cp "$TRUE_BIN" "$TEST_PACKAGE/sumpterd"
chmod 0755 "$TEST_PACKAGE/sumpterd"
cp "$ROOT/config.example.json" "$TEST_PACKAGE/config.example.json"
cp "$ROOT/deploy/sumpter.service" "$TEST_PACKAGE/sumpter.service"
cp "$ROOT/deploy/sumpter-system.service" "$TEST_PACKAGE/sumpter-system.service"
cp -R "$ROOT/web/." "$TEST_PACKAGE/web/"
cp "$ROOT/scripts/"*.sh "$TEST_PACKAGE/scripts/"
chmod 0755 "$TEST_PACKAGE/scripts/"*.sh
printf '%s\n' 'synthetic-admin-password' >"$ADMIN_PASSWORD_FILE"
chmod 0600 "$ADMIN_PASSWORD_FILE"

# Executable stubs shadow real tools only while the installer runs. User and
# system scope state are deliberately separate so an accidental --user is a
# failing root-test assertion rather than a false pass.
cat >"$STUB_BIN/systemctl" <<'EOF'
#!/usr/bin/env bash
set -u
scope="system"
if [[ "${1:-}" == "--user" ]]; then
    scope="user"
    shift
fi
case "$*" in
    "show-environment") exit 0 ;;
    "is-active --quiet sumpter.service") test -f "$STUB_STATE_DIR/$scope.active" ;;
    "is-enabled --quiet sumpter.service") test -f "$STUB_STATE_DIR/$scope.enabled" ;;
    "stop sumpter.service") rm -f -- "$STUB_STATE_DIR/$scope.active" ;;
    "start sumpter.service")
        if [[ -f "$STUB_STATE_DIR/$scope.fail-start-once" ]]; then
            rm -f -- "$STUB_STATE_DIR/$scope.fail-start-once"
            exit 1
        fi
        touch "$STUB_STATE_DIR/$scope.active"
        ;;
    "enable sumpter.service") touch "$STUB_STATE_DIR/$scope.enabled" ;;
    "disable sumpter.service") rm -f -- "$STUB_STATE_DIR/$scope.enabled" ;;
    "disable --now sumpter.service")
        rm -f -- "$STUB_STATE_DIR/$scope.active" "$STUB_STATE_DIR/$scope.enabled"
        ;;
    "daemon-reload" | "reset-failed sumpter.service") exit 0 ;;
    *)
        echo "未预期的 systemctl 调用(scope=$scope):$*" >&2
        exit 1
        ;;
esac
EOF

cat >"$STUB_BIN/file" <<'EOF'
#!/usr/bin/env bash
case "$(uname -m)" in
    arm64 | aarch64) echo "ELF 64-bit LSB executable, ARM aarch64, statically linked" ;;
    *) echo "ELF 64-bit LSB executable, x86-64, statically linked" ;;
esac
EOF

cat >"$STUB_BIN/realpath" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == "-e" ]]; then
    shift
fi
exec "$REALPATH_BIN" "$@"
EOF
chmod 0755 "$STUB_BIN/systemctl" "$STUB_BIN/file" "$STUB_BIN/realpath"
export REALPATH_BIN STUB_STATE_DIR

hash_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

reset_package_binary() {
    cp "$1" "$TEST_PACKAGE/sumpterd"
    chmod 0755 "$TEST_PACKAGE/sumpterd"
}

run_user() {
    PATH="$STUB_BIN:$PATH" HOME="$USER_HOME" bash "$TEST_PACKAGE/scripts/$1" "${@:2}"
}

run_system() {
    PATH="$STUB_BIN:$PATH" HOME="$USER_HOME" \
        SUMPTER_INSTALLER_SELFTEST=1 SUMPTER_SYSTEM_TEST_ROOT="$SYSTEM_ROOT" \
        bash "$TEST_PACKAGE/scripts/$1" "${@:2}"
}

exercise_user_scope() {
    reset_package_binary "$TRUE_BIN"
    if run_user install.sh --admin-host 999.999.999.999; then
        echo "非法 IPv4 应被拒绝" >&2
        exit 1
    fi
    run_user install.sh
    [[ -x "$USER_HOME/.local/share/sumpter/sumpterd" ]]
    [[ -f "$USER_HOME/.config/sumpter/config.json" ]]
    [[ -s "$USER_HOME/.config/sumpter/admin-password" ]]
    [[ "$(wc -l <"$USER_HOME/.config/sumpter/admin-password")" -eq 1 ]]
    [[ -f "$USER_HOME/.config/systemd/user/sumpter.service" ]]
    [[ ! -e "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf" ]]
    CONFIG_SUM="$(hash_file "$USER_HOME/.config/sumpter/config.json")"
    DEFAULT_PASSWORD_SUM="$(hash_file "$USER_HOME/.config/sumpter/admin-password")"

    # 默认密码文件让自定义监听无需再重复配置密码路径。
    run_user install.sh --admin-host 0.0.0.0
    grep -Fq 'Environment=SUMPTER_ADMIN_HOST=0.0.0.0' \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"
    [[ "$(hash_file "$USER_HOME/.config/sumpter/admin-password")" == "$DEFAULT_PASSWORD_SUM" ]]

    # 捕获安装结束摘要，确认自定义绑定会打印出来（0.0.0.0 的访问 URL 回落 127.0.0.1）。
    user_install_log="$TEST_ROOT/user-install-admin.log"
    PATH="$STUB_BIN:$PATH" HOME="$USER_HOME" \
        bash "$TEST_PACKAGE/scripts/install.sh" --admin-host 0.0.0.0 --admin-port 57901 \
        --admin-password-file "$ADMIN_PASSWORD_FILE" \
        | tee "$user_install_log"
    [[ -f "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf" ]]
    grep -Fq 'Environment=SUMPTER_ADMIN_HOST=0.0.0.0' \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"
    grep -Fq 'Environment=SUMPTER_ADMIN_PORT=57901' \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"
    grep -Fq "Environment=SUMPTER_ADMIN_PASSWORD_FILE=$ADMIN_PASSWORD_FILE" \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"
    grep -Fq 'Admin 绑定:0.0.0.0:57901' "$user_install_log"
    grep -Fq 'Web 管理:http://127.0.0.1:57901/admin/' "$user_install_log"
    DROPIN_SUM="$(hash_file "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf")"

    # 只改 host 时应保留已有 port。
    run_user install.sh --admin-host 192.168.1.50
    grep -Fq 'Environment=SUMPTER_ADMIN_HOST=192.168.1.50' \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"
    grep -Fq 'Environment=SUMPTER_ADMIN_PORT=57901' \
        "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf"

    # 升级未带 Admin 参数时应保留 drop-in。
    DROPIN_SUM="$(hash_file "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf")"
    run_user install.sh
    [[ -d "$USER_HOME/.local/share/sumpter.previous" ]]
    [[ "$(hash_file "$USER_HOME/.config/sumpter/config.json")" == "$CONFIG_SUM" ]]
    [[ "$(hash_file "$USER_HOME/.config/sumpter/admin-password")" == "$DEFAULT_PASSWORD_SUM" ]]
    [[ "$(hash_file "$USER_HOME/.config/systemd/user/sumpter.service.d/50-admin-listen.conf")" == "$DROPIN_SUM" ]]

    INSTALLED_SUM="$(hash_file "$USER_HOME/.local/share/sumpter/sumpterd")"
    UNIT_SUM="$(hash_file "$USER_HOME/.config/systemd/user/sumpter.service")"
    reset_package_binary "$FALSE_BIN"
    touch "$STUB_STATE_DIR/user.fail-start-once"
    if run_user install.sh; then
        echo "user 新版本启动失败时安装器应返回非零" >&2
        exit 1
    fi
    [[ "$(hash_file "$USER_HOME/.local/share/sumpter/sumpterd")" == "$INSTALLED_SUM" ]]
    [[ "$(hash_file "$USER_HOME/.config/systemd/user/sumpter.service")" == "$UNIT_SUM" ]]
    [[ -f "$STUB_STATE_DIR/user.active" && -f "$STUB_STATE_DIR/user.enabled" ]]
    [[ "$(hash_file "$USER_HOME/.config/sumpter/config.json")" == "$CONFIG_SUM" ]]
    [[ "$(hash_file "$USER_HOME/.config/sumpter/admin-password")" == "$DEFAULT_PASSWORD_SUM" ]]

    run_user uninstall.sh
    [[ ! -e "$USER_HOME/.local/share/sumpter" ]]
    [[ ! -e "$USER_HOME/.local/share/sumpter.previous" ]]
    [[ -f "$USER_HOME/.config/sumpter/config.json" ]]
    [[ "$(hash_file "$USER_HOME/.config/sumpter/admin-password")" == "$DEFAULT_PASSWORD_SUM" ]]
    [[ ! -e "$USER_HOME/.config/systemd/user/sumpter.service" ]]
    [[ ! -e "$USER_HOME/.config/systemd/user/sumpter.service.d" ]]

    reset_package_binary "$TRUE_BIN"
    run_user install.sh
    run_user uninstall.sh --purge
    [[ ! -e "$USER_HOME/.config/sumpter" ]]
}

exercise_system_scope() {
    reset_package_binary "$TRUE_BIN"
    run_system install.sh
    [[ -x "$SYSTEM_ROOT/opt/sumpter/sumpterd" ]]
    [[ -f "$SYSTEM_ROOT/var/lib/sumpter/config.json" ]]
    [[ -s "$SYSTEM_ROOT/var/lib/sumpter/admin-password" ]]
    [[ "$(wc -l <"$SYSTEM_ROOT/var/lib/sumpter/admin-password")" -eq 1 ]]
    [[ -f "$SYSTEM_ROOT/etc/systemd/system/sumpter.service" ]]
    grep -Fq 'WorkingDirectory=/opt/sumpter' "$SYSTEM_ROOT/etc/systemd/system/sumpter.service"
    grep -Fq 'User=sumpter' "$SYSTEM_ROOT/etc/systemd/system/sumpter.service"
    SYSTEM_CONFIG_SUM="$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/config.json")"
    SYSTEM_PASSWORD_SUM="$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/admin-password")"

    run_system install.sh --admin-host 192.168.1.10 --admin-port 58000 \
        --admin-password-file "$ADMIN_PASSWORD_FILE"
    [[ -f "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d/50-admin-listen.conf" ]]
    grep -Fq 'Environment=SUMPTER_ADMIN_HOST=192.168.1.10' \
        "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d/50-admin-listen.conf"
    grep -Fq 'Environment=SUMPTER_ADMIN_PORT=58000' \
        "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d/50-admin-listen.conf"
    grep -Fq "Environment=SUMPTER_ADMIN_PASSWORD_FILE=$ADMIN_PASSWORD_FILE" \
        "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d/50-admin-listen.conf"

    run_system install.sh
    [[ -d "$SYSTEM_ROOT/opt/sumpter.previous" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/config.json")" == "$SYSTEM_CONFIG_SUM" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/admin-password")" == "$SYSTEM_PASSWORD_SUM" ]]
    grep -Fq 'Environment=SUMPTER_ADMIN_HOST=192.168.1.10' \
        "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d/50-admin-listen.conf"

    SYSTEM_INSTALLED_SUM="$(hash_file "$SYSTEM_ROOT/opt/sumpter/sumpterd")"
    SYSTEM_UNIT_SUM="$(hash_file "$SYSTEM_ROOT/etc/systemd/system/sumpter.service")"
    reset_package_binary "$FALSE_BIN"
    touch "$STUB_STATE_DIR/system.fail-start-once"
    if run_system install.sh; then
        echo "system 新版本启动失败时安装器应返回非零" >&2
        exit 1
    fi
    [[ "$(hash_file "$SYSTEM_ROOT/opt/sumpter/sumpterd")" == "$SYSTEM_INSTALLED_SUM" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/etc/systemd/system/sumpter.service")" == "$SYSTEM_UNIT_SUM" ]]
    [[ -f "$STUB_STATE_DIR/system.active" && -f "$STUB_STATE_DIR/system.enabled" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/config.json")" == "$SYSTEM_CONFIG_SUM" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/admin-password")" == "$SYSTEM_PASSWORD_SUM" ]]

    run_system uninstall.sh
    [[ ! -e "$SYSTEM_ROOT/opt/sumpter" ]]
    [[ ! -e "$SYSTEM_ROOT/opt/sumpter.previous" ]]
    [[ -f "$SYSTEM_ROOT/var/lib/sumpter/config.json" ]]
    [[ "$(hash_file "$SYSTEM_ROOT/var/lib/sumpter/admin-password")" == "$SYSTEM_PASSWORD_SUM" ]]
    [[ ! -e "$SYSTEM_ROOT/etc/systemd/system/sumpter.service" ]]
    [[ ! -e "$SYSTEM_ROOT/etc/systemd/system/sumpter.service.d" ]]

    reset_package_binary "$TRUE_BIN"
    run_system install.sh
    run_system uninstall.sh --purge
    [[ ! -e "$SYSTEM_ROOT/var/lib/sumpter" ]]
}

exercise_user_scope
exercise_system_scope
echo "installer transaction self-test (user + system): PASS"
