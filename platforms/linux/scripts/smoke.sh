#!/usr/bin/env bash
# Rust sumpterd Linux 冒烟：临时 v3 配置、双 listener、内置登录 Admin、
# 禁用通知端点、SIGHUP、SIGTERM，以及旧 keys.json 拒绝启动。
# 默认 Admin 57879；可用 SUMPTER_SMOKE_ADMIN_PORT 覆盖（并传 --admin-port）。
set -euo pipefail
umask 077

BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${SUMPTERD_BIN:-$BASE_DIR/sumpterd}"
WEB_ROOT="${SUMPTERD_WEB_ROOT:-$BASE_DIR/web}"
CONFIG_EXAMPLE="${SUMPTERD_CONFIG_EXAMPLE:-$BASE_DIR/config.example.json}"
PROXY_PORT="${SUMPTERD_SMOKE_PROXY_PORT:-$((RANDOM % 1500 + 56000))}"
ADMIN_PORT="${SUMPTERD_SMOKE_ADMIN_PORT:-57879}"
PROXY_BASE="http://127.0.0.1:$PROXY_PORT"
ADMIN_BASE="http://127.0.0.1:$ADMIN_PORT"
ADMIN_USERNAME="kkl"
ADMIN_PASSWORD="synthetic-smoke-admin-password"
COOKIE_JAR=""
ADMIN_CSRF=""

[[ -x "$BIN" ]] || {
    echo "[smoke] 失败:找不到可执行文件 $BIN(可用 SUMPTERD_BIN 覆盖)" >&2
    exit 1
}
[[ -f "$CONFIG_EXAMPLE" ]] || {
    echo "[smoke] 失败:找不到 v3 示例配置 $CONFIG_EXAMPLE" >&2
    exit 1
}
[[ -f "$WEB_ROOT/index.html" ]] || {
    echo "[smoke] 失败:web-root 缺少 index.html:$WEB_ROOT" >&2
    exit 1
}
command -v curl >/dev/null 2>&1 || {
    echo "[smoke] 失败:需要 curl" >&2
    exit 1
}
command -v jq >/dev/null 2>&1 || {
    echo "[smoke] 失败:需要 jq" >&2
    exit 1
}

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sumpter-rust-smoke.XXXXXX")"
CONFIG_DIR="$TMP_DIR/config"
DAEMON_LOG="$TMP_DIR/sumpterd.log"
LEGACY_LOG="$TMP_DIR/legacy.log"
DAEMON_PID=""
FAILED=0

note() { echo "[smoke] $*"; }
fail() {
    echo "[smoke] 失败:$*" >&2
    FAILED=1
}

cleanup() {
    if [[ -n "$DAEMON_PID" ]]; then
        if kill -0 "$DAEMON_PID" 2>/dev/null; then
            kill -KILL "$DAEMON_PID" 2>/dev/null || true
        fi
        wait "$DAEMON_PID" 2>/dev/null || true
    fi
    if [[ "$FAILED" -ne 0 ]]; then
        echo "[smoke] daemon 日志尾部:" >&2
        tail -n 40 "$DAEMON_LOG" >&2 2>/dev/null || true
        if [[ -s "$LEGACY_LOG" ]]; then
            echo "[smoke] legacy 拒启日志尾部:" >&2
            tail -n 20 "$LEGACY_LOG" >&2 2>/dev/null || true
        fi
        echo "[smoke] 临时目录保留供排查:$TMP_DIR" >&2
    else
        rm -rf "$TMP_DIR"
    fi
}
trap cleanup EXIT

status_of() {
    local base="$1"
    local path="$2"
    curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' --max-time 5 "$base$path" 2>/dev/null || true
}

admin_status_of() {
    local path="$1"
    local cookie_args=()
    if [[ -n "$COOKIE_JAR" ]]; then
        cookie_args+=(--cookie "$COOKIE_JAR")
    fi
    curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' --max-time 5 \
        "${cookie_args[@]}" "$ADMIN_BASE$path" 2>/dev/null || true
}

admin_login() {
    local response_file="$TMP_DIR/login.json"
    COOKIE_JAR="$TMP_DIR/admin.cookies"
    local code
    code="$(curl --noproxy '*' -sS -o "$response_file" -w '%{http_code}' --max-time 5 \
        -c "$COOKIE_JAR" -H 'Content-Type: application/json' -X POST \
        -d "$(jq -cn --arg username "$ADMIN_USERNAME" --arg password "$ADMIN_PASSWORD" '{username:$username,password:$password}')" \
        "$ADMIN_BASE/admin/api/auth/login" 2>/dev/null || true)"
    [[ "$code" == "200" ]] || return 1
    ADMIN_CSRF="$(jq -r '.csrfToken // empty' "$response_file")"
    [[ -n "$ADMIN_CSRF" ]]
}

admin_write_status_of() {
    local method="$1"
    local path="$2"
    local body="${3-}"
    [[ -n "$body" ]] || body='{}'
    curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' --max-time 5 \
        -b "$COOKIE_JAR" -H 'Content-Type: application/json' -H "X-Sumpter-CSRF: $ADMIN_CSRF" \
        -X "$method" -d "$body" "$ADMIN_BASE$path" 2>/dev/null || true
}

mkdir -p "$CONFIG_DIR"
# 示例文件的唯一 listener.port 替换为本轮随机端口，不接触真实配置。
sed "0,/\"port\": 57878/s//\"port\": $PROXY_PORT/" "$CONFIG_EXAMPLE" >"$CONFIG_DIR/config.json"
chmod 600 "$CONFIG_DIR/config.json"
printf '%s\n' "$ADMIN_PASSWORD" >"$CONFIG_DIR/admin-password"
chmod 600 "$CONFIG_DIR/admin-password"

# 选定 Admin 端口；已有服务时必须拒绝，避免把另一实例误判为本轮 daemon。
existing_admin_code="$(status_of "$ADMIN_BASE" /admin/api/auth/session)"
if [[ "$existing_admin_code" != "000" ]]; then
    fail "Admin 端口 127.0.0.1:${ADMIN_PORT} 已被 HTTP 服务占用（状态码 ${existing_admin_code}）"
    exit 1
fi

note "启动 daemon(proxy=$PROXY_PORT,admin=127.0.0.1:$ADMIN_PORT)…"
"$BIN" \
    --config-dir "$CONFIG_DIR" \
    --web-root "$WEB_ROOT" \
    --admin-host 127.0.0.1 \
    --admin-port "$ADMIN_PORT" \
    >"$DAEMON_LOG" 2>&1 &
DAEMON_PID=$!

ready=0
for _ in $(seq 1 30); do
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        break
    fi
    if [[ "$(status_of "$ADMIN_BASE" /admin/)" == "200" ]] && admin_login; then
        ready=1
        break
    fi
    sleep 0.5
done
if [[ "$ready" -ne 1 ]]; then
    fail "daemon 未在 15s 内就绪"
    exit 1
fi
note "✓ Admin 登录壳与会话登录 200"

if [[ "$(status_of "$ADMIN_BASE" /admin/api/status)" == "401" ]]; then
    note "✓ Admin API 裸访问统一 401（无 Basic challenge）"
else
    fail "Admin 本机裸访问未返回 401"
fi

if [[ "$(status_of "$ADMIN_BASE" /healthz)" == "204" ]]; then
    note "✓ /healthz 未认证存活探针 204"
else
    fail "/healthz 未返回 204"
fi

if [[ "$(status_of "$ADMIN_BASE" /admin/)" == "200" ]]; then
    note "✓ /admin/ 静态登录壳公开 200"
else
    fail "/admin/ 未返回 200"
fi
if [[ "$(admin_status_of /admin/api/config)" == "200" ]]; then
    note "✓ config API 带会话 200"
else
    fail "config API 未返回 200"
fi
if [[ "$(admin_write_status_of POST /admin/api/auth/logout)" == "200" ]]; then
    note "✓ logout 通过会话 + CSRF 200"
else
    fail "logout 未通过会话 + CSRF 校验"
fi
admin_login || fail "logout 后重新登录失败"
if [[ "$(status_of "$PROXY_BASE" /__status)" == "200" ]]; then
    note "✓ proxy /__status 200"
else
    fail "proxy /__status 未返回 200"
fi
if [[ "$(status_of "$PROXY_BASE" /v1/does-not-exist)" == "404" ]]; then
    note "✓ proxy 未知路径 404"
else
    fail "proxy 未知路径未返回 404"
fi

notify_code="$(curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' --max-time 5 \
    -X POST -H 'Content-Type: application/json' -d '{}' "$PROXY_BASE/__notify" 2>/dev/null || true)"
if [[ "$notify_code" == "404" ]]; then
    note "✓ /__notify 已禁用(404)"
else
    fail "/__notify 期望 404,实际 ${notify_code:-000}"
fi

note "发送 SIGHUP 并确认双 listener 继续服务…"
kill -HUP "$DAEMON_PID"
sleep 1
[[ "$(admin_status_of /admin/api/status)" == "200" ]] \
    || fail "SIGHUP 后 Admin 未恢复"
[[ "$(status_of "$PROXY_BASE" /__status)" == "200" ]] \
    || fail "SIGHUP 后 proxy 未恢复"

note "发送 SIGTERM,等待优雅退出…"
kill -TERM "$DAEMON_PID"
stopped=0
for _ in $(seq 1 20); do
    if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
        stopped=1
        break
    fi
    sleep 0.5
done
if [[ "$stopped" -ne 1 ]]; then
    fail "SIGTERM 后 10s 未退出"
    exit 1
else
    daemon_exit=0
    wait "$DAEMON_PID" || daemon_exit=$?
    if [[ "$daemon_exit" -eq 0 ]]; then
        note "✓ SIGTERM 优雅退出(exit 0)"
    else
        fail "SIGTERM 退出码非零:$daemon_exit"
    fi
fi
DAEMON_PID=""

# Rust Linux 版不做 keys.json 自动迁移；旧配置必须显式阻断，且原文件不能被改写。
legacy_dir="$TMP_DIR/legacy-only"
mkdir -p "$legacy_dir"
printf '{"port":57878}\n' >"$legacy_dir/keys.json"
printf '%s\n' "$ADMIN_PASSWORD" >"$legacy_dir/admin-password"
chmod 600 "$legacy_dir/admin-password"
legacy_before="$(cksum "$legacy_dir/keys.json")"
"$BIN" --config-dir "$legacy_dir" --no-web \
    >"$LEGACY_LOG" 2>&1 &
legacy_pid=$!
legacy_running=1
for _ in $(seq 1 10); do
    if ! kill -0 "$legacy_pid" 2>/dev/null; then
        legacy_running=0
        break
    fi
    sleep 0.2
done
if [[ "$legacy_running" -eq 1 ]]; then
    kill -KILL "$legacy_pid" 2>/dev/null || true
    wait "$legacy_pid" 2>/dev/null || true
    fail "仅有 keys.json 时 daemon 未拒绝启动"
else
    legacy_exit=0
    wait "$legacy_pid" || legacy_exit=$?
    if [[ "$legacy_exit" -ne 0 ]]; then
        note "✓ legacy keys.json-only 配置被拒绝"
    else
        fail "legacy keys.json-only 意外 exit 0"
    fi
fi
[[ ! -e "$legacy_dir/config.json" ]] || fail "拒绝旧配置时不应生成 config.json"
[[ "$(cksum "$legacy_dir/keys.json")" == "$legacy_before" ]] || fail "keys.json 被改写"

if [[ "$FAILED" -ne 0 ]]; then
    exit 1
fi
note "全部通过。公网 HTTPS 反代仍需 Linux/VPS 真机补验。"
