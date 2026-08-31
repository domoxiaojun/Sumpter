#!/usr/bin/env bash
# 按 pidfile 优雅停止 Linux sumpterd；超时后才 SIGKILL。
# 环境变量:SUMPTERD_BIN(默认 ../sumpterd)、SUMPTERD_PID_FILE(默认 ../sumpterd.pid)、
# SUMPTERD_STOP_TIMEOUT(秒,默认 15)。
set -euo pipefail

BASE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="${SUMPTERD_BIN:-$BASE_DIR/sumpterd}"
PID_FILE="${SUMPTERD_PID_FILE:-$BASE_DIR/sumpterd.pid}"
TIMEOUT="${SUMPTERD_STOP_TIMEOUT:-15}"
BIN_REAL="$(readlink -f "$BIN" 2>/dev/null || readlink -m "$BIN")"
[[ -n "$BIN_REAL" ]] || {
    echo "错误:无法解析 SUMPTERD_BIN 规范路径:$BIN" >&2
    exit 1
}

[[ "$TIMEOUT" =~ ^[1-9][0-9]*$ ]] || {
    echo "错误:SUMPTERD_STOP_TIMEOUT 必须是正整数" >&2
    exit 1
}

if [[ ! -f "$PID_FILE" ]]; then
    echo "未找到 pidfile($PID_FILE),sumpterd 可能未通过 start.sh 运行。"
    exit 0
fi

pid="$(head -n 1 "$PID_FILE" 2>/dev/null || true)"
if [[ ! "$pid" =~ ^[1-9][0-9]*$ ]] || ! kill -0 "$pid" 2>/dev/null; then
    echo "pidfile 里的进程(${pid:-?})已不存在,清理残留 pidfile。"
    rm -f "$PID_FILE"
    exit 0
fi

if [[ -r "/proc/$pid/exe" ]]; then
    running_exe="$(readlink "/proc/$pid/exe" 2>/dev/null || true)"
    running_exe="${running_exe% (deleted)}"
    if [[ "$running_exe" != "$BIN_REAL" ]]; then
        echo "错误:pidfile 指向另一进程或另一份 sumpterd(pid $pid,exe $running_exe),拒绝发送信号。" >&2
        exit 1
    fi
else
    # 非 Linux /proc 环境只能退回命令名，无法排除另一份同名 binary。
    command_name="$(ps -p "$pid" -o comm= 2>/dev/null | awk '{print $1}')"
    if [[ "$command_name" != "sumpterd" ]]; then
        echo "错误:pidfile 指向非 sumpterd 进程(pid $pid,comm ${command_name:-?}),拒绝发送信号。" >&2
        exit 1
    fi
fi

echo "向 sumpterd(pid $pid)发送 SIGTERM,等待优雅退出(最多 ${TIMEOUT}s)…"
kill -TERM "$pid"

for ((second = 0; second < TIMEOUT; second += 1)); do
    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f "$PID_FILE"
        echo "sumpterd 已停止。"
        exit 0
    fi
    sleep 1
done

echo "SIGTERM 超时,向 sumpterd(pid $pid)发送 SIGKILL。" >&2
kill -KILL "$pid" 2>/dev/null || true
for _ in 1 2 3 4 5; do
    if ! kill -0 "$pid" 2>/dev/null; then
        rm -f "$PID_FILE"
        echo "sumpterd 已强制停止。"
        exit 0
    fi
    sleep 1
done

echo "错误:SIGKILL 后进程仍存在;保留 pidfile 供人工检查:$PID_FILE" >&2
exit 1
