#!/usr/bin/env bash
# Linux 后台启动器:nohup + 私有 pidfile + 日志轮转 + Admin 就绪探测。
#
# 环境变量:
#   SUMPTERD_BIN          可执行文件(默认:部署目录/sumpterd)
#   SUMPTERD_PID_FILE     pidfile(默认:部署目录/sumpterd.pid)
#   SUMPTERD_LOG_DIR      日志目录(默认:部署目录/logs)
#   SUMPTERD_CONFIG_DIR   非空时追加 --config-dir
#   SUMPTERD_WEB_ROOT     Web 根目录(默认:部署目录/web；可用 CLI --no-web 禁用)
#   SUMPTERD_START_TIMEOUT  就绪等待秒数(默认 15)
#   SUMPTER_ADMIN_HOST    Admin 绑定地址(默认 127.0.0.1；透传给 daemon)
#   SUMPTER_ADMIN_PORT    Admin 端口(默认 57879；透传给 daemon)
#   SUMPTER_ADMIN_PASSWORD_FILE 高级凭据路径覆盖；默认读取配置目录/admin-password
# 其余参数原样透传给 sumpterd（含 --admin-host / --admin-port / --admin-password-file）。
set -euo pipefail
umask 077

BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${SUMPTERD_BIN:-$BASE_DIR/sumpterd}"
PID_FILE="${SUMPTERD_PID_FILE:-$BASE_DIR/sumpterd.pid}"
LOG_DIR="${SUMPTERD_LOG_DIR:-$BASE_DIR/logs}"
LOG_FILE="$LOG_DIR/sumpterd.log"
WEB_ROOT="${SUMPTERD_WEB_ROOT:-$BASE_DIR/web}"
START_TIMEOUT="${SUMPTERD_START_TIMEOUT:-15}"
DEFAULT_ADMIN_HOST="127.0.0.1"
DEFAULT_ADMIN_PORT="57879"

die() {
    echo "错误:$*" >&2
    exit 1
}

is_positive_integer() {
    [[ "$1" =~ ^[1-9][0-9]*$ ]]
}

pid_is_running() {
    local pid="$1"
    [[ "$pid" =~ ^[1-9][0-9]*$ ]] && kill -0 "$pid" 2>/dev/null
}

pid_is_sumpterd() {
    local pid="$1"
    local running_exe=""
    if [[ -r "/proc/$pid/exe" ]]; then
        running_exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
        running_exe="${running_exe% (deleted)}"
        [[ "$running_exe" == "$BIN_REAL" ]]
        return
    fi
    # 非 Linux /proc 环境只能退回命令名，无法排除另一份同名 binary。
    [[ "$(ps -p "$pid" -o comm= 2>/dev/null | awk '{print $1}')" == "sumpterd" ]]
}

terminate_spawned_pid() {
    local pid="$1"
    if ! pid_is_running "$pid" || ! pid_is_sumpterd "$pid"; then
        return
    fi
    kill -TERM "$pid" 2>/dev/null || true
    for _ in 1 2 3 4 5; do
        if ! pid_is_running "$pid"; then
            wait "$pid" 2>/dev/null || true
            return
        fi
        sleep 1
    done
    if pid_is_running "$pid" && pid_is_sumpterd "$pid"; then
        kill -KILL "$pid" 2>/dev/null || true
    fi
    wait "$pid" 2>/dev/null || true
}

# 从 CLI / 环境变量解析 Admin 监听；CLI 优先。探测地址对全接口绑定回落 127.0.0.1。
resolve_admin_probe() {
    local host="$DEFAULT_ADMIN_HOST"
    local port="$DEFAULT_ADMIN_PORT"
    local expect_host=0
    local expect_port=0
    local arg

    if [[ -n "${SUMPTER_ADMIN_HOST:-}" ]]; then
        host="$SUMPTER_ADMIN_HOST"
    fi
    if [[ -n "${SUMPTER_ADMIN_PORT:-}" ]]; then
        port="$SUMPTER_ADMIN_PORT"
    fi

    for arg in "$@"; do
        if [[ "$expect_host" -eq 1 ]]; then
            host="$arg"
            expect_host=0
            continue
        fi
        if [[ "$expect_port" -eq 1 ]]; then
            port="$arg"
            expect_port=0
            continue
        fi
        case "$arg" in
            --admin-host)
                expect_host=1
                ;;
            --admin-host=*)
                die "请使用 --admin-host <ip>，daemon 不接受 --admin-host=<ip>"
                ;;
            --admin-port)
                expect_port=1
                ;;
            --admin-port=*)
                die "请使用 --admin-port <port>，daemon 不接受 --admin-port=<port>"
                ;;
        esac
    done
    [[ "$expect_host" -eq 0 ]] || die "--admin-host 缺少地址值"
    [[ "$expect_port" -eq 0 ]] || die "--admin-port 缺少端口值"
    is_positive_integer "$port" || die "Admin 端口必须是 1...65535 的正整数:$port"
    [[ "$port" -le 65535 ]] || die "Admin 端口必须是 1...65535 的正整数:$port"

    local probe_host="$host"
    case "$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')" in
        ""|0.0.0.0|::|localhost)
            probe_host="127.0.0.1"
            ;;
    esac
    # IPv6 探测 URL 需要方括号。
    local probe_authority="$probe_host"
    if [[ "$probe_host" == *:* && "$probe_host" != \[* ]]; then
        probe_authority="[$probe_host]"
    fi
    ADMIN_PROBE_URL="http://${probe_authority}:${port}/healthz"
    # 展示：绑定地址用真实 host；访问 URL 用 probe（0.0.0.0 → 127.0.0.1）
    ADMIN_DISPLAY_URL="http://${probe_authority}:${port}/admin/"
    ADMIN_BIND_DESC="${host}:${port}"
    case "$(printf '%s' "$host" | tr '[:upper:]' '[:lower:]')" in
        ""|0.0.0.0|::)
            ADMIN_LAN_HINT="http://<主机IP>:${port}/admin/"
            ;;
        *)
            ADMIN_LAN_HINT=""
            ;;
    esac
}

expect_web_root=0
web_option_present=0
for arg in "$@"; do
    if [[ "$expect_web_root" -eq 1 ]]; then
        expect_web_root=0
    elif [[ "$arg" == "--web-root" ]]; then
        web_option_present=1
        expect_web_root=1
    elif [[ "$arg" == --web-root=* ]]; then
        die "请使用 --web-root <dir>，daemon 不接受 --web-root=<dir>"
    elif [[ "$arg" == "--no-web" ]]; then
        web_option_present=1
    fi
done
[[ "$expect_web_root" -eq 0 ]] || die "--web-root 缺少目录值"
is_positive_integer "$START_TIMEOUT" || die "SUMPTERD_START_TIMEOUT 必须是正整数"

resolve_admin_probe "$@"

[[ -x "$BIN" ]] || die "找不到可执行文件 $BIN(可用 SUMPTERD_BIN 覆盖)"
BIN_REAL="$(readlink -f "$BIN" 2>/dev/null || readlink -m "$BIN")"
[[ -n "$BIN_REAL" ]] || die "无法解析 sumpterd 规范路径:$BIN"

if [[ -f "$PID_FILE" ]]; then
    old_pid="$(head -n 1 "$PID_FILE" 2>/dev/null || true)"
    if pid_is_running "$old_pid"; then
        if pid_is_sumpterd "$old_pid"; then
            echo "sumpterd 已在运行(pid $old_pid),无需重复启动。"
            exit 0
        fi
        die "pidfile 指向另一进程或另一份 sumpterd(pid $old_pid);为避免误伤,请人工核对 $PID_FILE"
    fi
    echo "发现残留 pidfile(进程 ${old_pid:-?} 已不存在),本次启动将覆盖。"
fi

if command -v curl >/dev/null 2>&1; then
    existing_admin_code="$(curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' \
        --max-time 2 "$ADMIN_PROBE_URL" 2>/dev/null || true)"
    if [[ "$existing_admin_code" != "000" ]]; then
        die "Admin 地址 ${ADMIN_BIND_DESC} 已被 HTTP 服务占用（状态码 ${existing_admin_code}）"
    fi
fi

mkdir -p "$LOG_DIR" "$(dirname "$PID_FILE")"
if [[ -s "$LOG_FILE" ]]; then
    mv "$LOG_FILE" "$LOG_DIR/sumpterd-$(date +%Y%m%d-%H%M%S).log"
fi

daemon_args=()
if [[ -n "${SUMPTERD_CONFIG_DIR:-}" ]]; then
    daemon_args+=(--config-dir "$SUMPTERD_CONFIG_DIR")
fi
if [[ "$web_option_present" -eq 0 ]]; then
    [[ -f "$WEB_ROOT/index.html" ]] \
        || die "Web 根目录缺少 index.html:${WEB_ROOT}（确需无 Web 时传 --no-web）"
    daemon_args+=(--web-root "$WEB_ROOT")
fi
daemon_args+=("$@")

nohup "$BIN" "${daemon_args[@]}" >>"$LOG_FILE" 2>&1 &
pid=$!
printf '%s\n' "$pid" >"$PID_FILE"

ready=0
for ((second = 0; second < START_TIMEOUT; second += 1)); do
    if ! pid_is_running "$pid" || ! pid_is_sumpterd "$pid"; then
        break
    fi
    if command -v curl >/dev/null 2>&1; then
        code="$(curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' \
            --max-time 2 "$ADMIN_PROBE_URL" 2>/dev/null || true)"
        # 新版 /healthz 返回 204；兼容旧包的 200/401 Admin 探针结果。
        if [[ "$code" == "204" || "$code" == "200" || "$code" == "401" ]]; then
            ready=1
            break
        fi
    elif [[ "$second" -ge 1 ]]; then
        # 最小部署没有 curl 时只能确认进程持续存活。
        ready=1
        break
    fi
    sleep 1
done

if [[ "$ready" -ne 1 ]]; then
    terminate_spawned_pid "$pid"
    rm -f "$PID_FILE"
    echo "错误:sumpterd 未在 ${START_TIMEOUT}s 内就绪(Admin $ADMIN_BIND_DESC)。最近日志:" >&2
    tail -n 30 "$LOG_FILE" >&2 || true
    exit 1
fi

echo "sumpterd 已后台启动(pid $pid)"
echo "  Admin 绑定:$ADMIN_BIND_DESC"
echo "  Web 管理:$ADMIN_DISPLAY_URL"
if [[ -n "${ADMIN_LAN_HINT:-}" ]]; then
    echo "  局域网访问:$ADMIN_LAN_HINT"
fi
echo "  日志:$LOG_FILE"
echo "  停止:$BASE_DIR/scripts/stop.sh"
echo "  重载:kill -HUP $pid"
