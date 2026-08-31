#!/usr/bin/env bash
# Exercise the static-mirror bootstrap without downloading or installing a real package.
set -Eeuo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
BOOTSTRAP="$ROOT/scripts/bootstrap-install.sh"
TEST_BASE="${TMPDIR:-/tmp}"
TEST_ROOT="$(mktemp -d "$TEST_BASE/kekulv-bootstrap-test.XXXXXX")"
STUB_BIN="$TEST_ROOT/bin"
ARCHIVE_DIR="$TEST_ROOT/archive"
GOOD_ARCHIVE="$TEST_ROOT/good.tar.gz"
BAD_ARCHIVE="$TEST_ROOT/bad.tar.gz"
MARKER="$TEST_ROOT/inner-installer-ran"
REQUEST_URL="$TEST_ROOT/request-url"
BOOTSTRAP_OUTPUT="$TEST_ROOT/bootstrap-output"
BASE_URL="https://mirror.example.invalid/kekulv"

cleanup() {
    local status=$?

    trap - EXIT
    case "$TEST_ROOT" in
        "$TEST_BASE"/kekulv-bootstrap-test.*)
            [[ -d "$TEST_ROOT" && ! -L "$TEST_ROOT" ]] && rm -rf -- "$TEST_ROOT"
            ;;
        *)
            echo "拒绝清理非测试目录:$TEST_ROOT" >&2
            ;;
    esac
    exit "$status"
}
trap cleanup EXIT

case "$(uname -m)" in
    x86_64 | amd64) ARCH="x86_64" ;;
    aarch64 | arm64) ARCH="aarch64" ;;
    *)
        echo "不支持的测试架构:$(uname -m)" >&2
        exit 1
        ;;
esac

PACKAGE_NAME="kekulv-linux-$ARCH"
PACKAGE_ROOT="$ARCHIVE_DIR/$PACKAGE_NAME"
mkdir -p "$PACKAGE_ROOT/scripts" "$STUB_BIN"
cat >"$PACKAGE_ROOT/scripts/install.sh" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail
printf 'package installer ran\n' >"${KEKULV_BOOTSTRAP_MARKER:?}"
if (($# > 0)); then
    printf '%s\n' "$*" >"${KEKULV_BOOTSTRAP_INSTALL_ARGS:?}"
else
    : >"${KEKULV_BOOTSTRAP_INSTALL_ARGS:?}"
fi
EOF
chmod 0755 "$PACKAGE_ROOT/scripts/install.sh"
cat >"$PACKAGE_ROOT/CHANGELOG.md" <<'EOF'
# v9.8.7

* bootstrap changelog display fixture
EOF
tar -C "$ARCHIVE_DIR" -czf "$GOOD_ARCHIVE" "$PACKAGE_NAME"
INSTALL_ARGS_FILE="$TEST_ROOT/install-args"

cat >"$STUB_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -Eeuo pipefail

destination=""
url=""
while (($# > 0)); do
    case "$1" in
        --output)
            destination="$2"
            shift 2
            ;;
        --proto | --proto-redir | --retry | --connect-timeout | --max-time)
            shift 2
            ;;
        --fail | --location | --silent | --show-error | --tlsv1.2)
            shift
            ;;
        https://*)
            url="$1"
            shift
            ;;
        *)
            echo "curl stub 收到未预期参数:$1" >&2
            exit 1
            ;;
    esac
done
[[ -n "$destination" && -n "$url" ]] || exit 1
printf '%s\n' "$url" >"${KEKULV_BOOTSTRAP_REQUEST_URL:?}"
cp -- "${KEKULV_BOOTSTRAP_ARCHIVE:?}" "$destination"
EOF
chmod 0755 "$STUB_BIN/curl"

PATH="$STUB_BIN:$PATH" \
    KEKULV_BOOTSTRAP_ARCHIVE="$GOOD_ARCHIVE" \
    KEKULV_BOOTSTRAP_MARKER="$MARKER" \
    KEKULV_BOOTSTRAP_INSTALL_ARGS="$INSTALL_ARGS_FILE" \
    KEKULV_BOOTSTRAP_REQUEST_URL="$REQUEST_URL" \
    bash "$BOOTSTRAP" --base-url "$BASE_URL" >"$BOOTSTRAP_OUTPUT"
[[ "$(cat "$MARKER")" == "package installer ran" ]]
[[ "$(cat "$REQUEST_URL")" == "$BASE_URL/$PACKAGE_NAME.tar.gz" ]]
[[ -f "$INSTALL_ARGS_FILE" && ! -s "$INSTALL_ARGS_FILE" ]]
grep -Fq '# v9.8.7' "$BOOTSTRAP_OUTPUT"
grep -Fq '* bootstrap changelog display fixture' "$BOOTSTRAP_OUTPUT"

rm -f -- "$MARKER" "$REQUEST_URL" "$INSTALL_ARGS_FILE"
PATH="$STUB_BIN:$PATH" \
    KEKULV_BOOTSTRAP_ARCHIVE="$GOOD_ARCHIVE" \
    KEKULV_BOOTSTRAP_MARKER="$MARKER" \
    KEKULV_BOOTSTRAP_INSTALL_ARGS="$INSTALL_ARGS_FILE" \
    KEKULV_BOOTSTRAP_REQUEST_URL="$REQUEST_URL" \
    bash "$BOOTSTRAP" --base-url "$BASE_URL" --admin-host 0.0.0.0 --admin-port 57900 \
    --admin-password-file /tmp/kekulv-test-password
[[ "$(cat "$MARKER")" == "package installer ran" ]]
[[ "$(cat "$INSTALL_ARGS_FILE")" == "--admin-host 0.0.0.0 --admin-port 57900 --admin-password-file /tmp/kekulv-test-password" ]]

rm -f -- "$MARKER" "$REQUEST_URL" "$INSTALL_ARGS_FILE"
PATH="$STUB_BIN:$PATH" \
    KEKULV_BOOTSTRAP_ARCHIVE="$GOOD_ARCHIVE" \
    KEKULV_BOOTSTRAP_MARKER="$MARKER" \
    KEKULV_BOOTSTRAP_INSTALL_ARGS="$INSTALL_ARGS_FILE" \
    KEKULV_BOOTSTRAP_REQUEST_URL="$REQUEST_URL" \
    bash "$BOOTSTRAP" --base-url "$BASE_URL" -- --admin-host 10.0.0.1
[[ "$(cat "$INSTALL_ARGS_FILE")" == "--admin-host 10.0.0.1" ]]

if PATH="$STUB_BIN:$PATH" \
    KEKULV_BOOTSTRAP_ARCHIVE="$GOOD_ARCHIVE" \
    KEKULV_BOOTSTRAP_MARKER="$MARKER" \
    KEKULV_BOOTSTRAP_INSTALL_ARGS="$INSTALL_ARGS_FILE" \
    KEKULV_BOOTSTRAP_REQUEST_URL="$REQUEST_URL" \
    bash "$BOOTSTRAP" --base-url "$BASE_URL" --repo owner/kekulv; then
    echo "bootstrap 不应接受 --repo" >&2
    exit 1
fi

rm -f -- "$MARKER" "$REQUEST_URL"
BAD_DIR="$TEST_ROOT/bad/$PACKAGE_NAME"
mkdir -p "$BAD_DIR"
ln -s /etc/passwd "$BAD_DIR/not-allowed"
tar -C "$TEST_ROOT/bad" -czf "$BAD_ARCHIVE" "$PACKAGE_NAME"
if PATH="$STUB_BIN:$PATH" \
    KEKULV_BOOTSTRAP_ARCHIVE="$BAD_ARCHIVE" \
    KEKULV_BOOTSTRAP_MARKER="$MARKER" \
    KEKULV_BOOTSTRAP_INSTALL_ARGS="$INSTALL_ARGS_FILE" \
    KEKULV_BOOTSTRAP_REQUEST_URL="$REQUEST_URL" \
    bash "$BOOTSTRAP" --base-url "$BASE_URL"; then
    echo "包含符号链接的发布包应被拒绝" >&2
    exit 1
fi
[[ ! -e "$MARKER" ]]

echo "static mirror bootstrap self-test: PASS"
