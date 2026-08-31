#!/usr/bin/env bash
# Exercise the user-scope static uninstaller bootstrap without touching systemd.
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BOOTSTRAP="$ROOT/scripts/bootstrap-uninstall.sh"
TEST_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEST_BASE/sumpter-bootstrap-uninstall-test.XXXXXX")"
USER_HOME="$TEST_ROOT/home"
INSTALLED_SCRIPT="$USER_HOME/.local/share/sumpter/scripts/uninstall.sh"
MARKER="$TEST_ROOT/forwarded-arguments"

cleanup() {
    local status=$?

    trap - EXIT
    case "$TEST_ROOT" in
        "$TEST_BASE"/sumpter-bootstrap-uninstall-test.*)
            [[ -d "$TEST_ROOT" && ! -L "$TEST_ROOT" ]] && rm -rf -- "$TEST_ROOT"
            ;;
        *)
            echo "拒绝清理非测试目录:$TEST_ROOT" >&2
            ;;
    esac
    exit "$status"
}
trap cleanup EXIT

mkdir -p "$(dirname "$INSTALLED_SCRIPT")"
cat >"$INSTALLED_SCRIPT" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail
printf '%s\n' "$*" >"${SUMPTER_BOOTSTRAP_UNINSTALL_MARKER:?}"
EOF
chmod 0755 "$INSTALLED_SCRIPT"

HOME="$USER_HOME" \
    SUMPTER_BOOTSTRAP_UNINSTALL_MARKER="$MARKER" \
    bash "$BOOTSTRAP"
[[ "$(cat "$MARKER")" == "" ]]

HOME="$USER_HOME" \
    SUMPTER_BOOTSTRAP_UNINSTALL_MARKER="$MARKER" \
    bash "$BOOTSTRAP" --purge
[[ "$(cat "$MARKER")" == "--purge" ]]

rm -f -- "$INSTALLED_SCRIPT"
if HOME="$USER_HOME" \
    SUMPTER_BOOTSTRAP_UNINSTALL_MARKER="$MARKER" \
    bash "$BOOTSTRAP"; then
    echo "缺少包内卸载器时引导脚本应返回非零" >&2
    exit 1
fi

echo "static mirror uninstall bootstrap self-test: PASS"
