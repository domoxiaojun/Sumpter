#!/usr/bin/env bash
# grok-project-attribution.sh 的自测。全部在临时 HOME 与临时 PATH 里跑。
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
INSTALLER="${SUMPTER_GROK_ATTRIBUTION_SCRIPT:-$SCRIPT_DIR/grok-project-attribution.sh}"
[ -x "$INSTALLER" ] || chmod +x "$INSTALLER"
[ -x "$INSTALLER" ] || {
    printf 'selftest FAIL: 找不到可执行的 %s\n' "$INSTALLER" >&2
    exit 1
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FAKE_HOME="$WORK/home"
RC="$FAKE_HOME/.zshrc"
SNIP="$FAKE_HOME/.local/share/sumpter/grok-project-attribution.sh"
SEED='export SELFTEST_SENTINEL=1'

pass=0
fail=0
ok() {
    printf 'ok   %s\n' "$1"
    pass=$((pass + 1))
}
bad() {
    printf 'FAIL %s\n' "$1"
    [ -n "${2:-}" ] && printf '     %s\n' "$2"
    fail=$((fail + 1))
}
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1" "期望[$3] 实际[$2]"; fi; }

reset_home() {
    rm -rf "$FAKE_HOME"
    mkdir -p "$FAKE_HOME"
    printf '%s\n' "$SEED" > "$RC"
}

inst() { inst_as /bin/zsh "$@"; }
inst_as() {
    local sh="$1"
    shift
    HOME="$FAKE_HOME" XDG_DATA_HOME="$FAKE_HOME/.local/share" SHELL="$sh" \
        "$INSTALLER" "$@"
}

blocks() {
    [ -f "$RC" ] || {
        printf '0'
        return
    }
    grep -c '>>> sumpter grok-project-attribution >>>' "$RC" 2> /dev/null || true
}
backups() {
    find "$FAKE_HOME" -maxdepth 1 -type f -name '.zshrc.sumpter-grok-bak-*' | wc -l | tr -d ' '
}

mkdir -p "$WORK/bin"
cat > "$WORK/bin/grok" <<'STUB'
#!/bin/sh
printf '%s' "${GROK_CONFIG:-}"
STUB
chmod +x "$WORK/bin/grok"

run_snip() {
    local sh="$1" dir="$2" preset="${3:-}" runner="$WORK/runner.sh"
    printf 'cd "%s" || exit 1\n. "%s"\ngrok\n' "$dir" "$SNIP" > "$runner"
    GROK_CONFIG="$preset" USER=kkl LOGNAME=kkl HOME="$FAKE_HOME" \
        PATH="$WORK/bin:$PATH" GIT_CEILING_DIRECTORIES="$WORK" "$sh" "$runner"
}

header_val() { # $1=json $2=key
    python3 -c 'import json,sys
cfg=json.loads(sys.argv[1] or "{}")
print(cfg.get("models",{}).get("extra_headers",{}).get(sys.argv[2],""))
' "$1" "$2"
}

printf '== 安装器 ==\n'

reset_home
inst install --rc "$RC" > /dev/null
check 'T1 装出恰好一个标记块' "$(blocks)" 1
check 'T1 snippet 已生成' "$([ -f "$SNIP" ] && echo yes || echo no)" yes
check 'T1 保留用户原有内容' "$(grep -c "$SEED" "$RC")" 1

inst install --rc "$RC" > /dev/null
check 'T2 重装后仍只有一个块' "$(blocks)" 1

reset_home
inst install --rc "$RC" --dry-run > /dev/null
check 'T3 dry-run 未改 rc' "$(blocks)" 0
check 'T3 dry-run 未写 snippet' "$([ -f "$SNIP" ] && echo yes || echo no)" no
check 'T3 dry-run 未建备份' "$(backups)" 0

reset_home
inst install --rc "$RC" > /dev/null
check 'T4 已建备份' "$(backups)" 1
printf 'JUNK_LINE\n' >> "$RC"
inst restore --rc "$RC" > /dev/null
check 'T4 还原后 JUNK 消失' "$(grep -c JUNK_LINE "$RC" || true)" 0
check 'T4 还原后标记块消失' "$(blocks)" 0

reset_home
inst install --rc "$RC" > /dev/null
inst uninstall --rc "$RC" > /dev/null
check 'T5 卸载后无标记块' "$(blocks)" 0
check 'T5 卸载后 snippet 删除' "$([ -f "$SNIP" ] && echo yes || echo no)" no
check 'T5 卸载保留备份' "$(backups)" 1

reset_home
if GROK_CONFIG_PATH=/tmp/x inst install --rc "$RC" > /dev/null 2>&1; then
    bad 'T6 应拒装 GROK_CONFIG_PATH'
else
    ok 'T6 检出 GROK_CONFIG_PATH 并拒装'
fi
check 'T6 拒装后未改 rc' "$(blocks)" 0
if GROK_CONFIG_PATH=/tmp/x inst install --rc "$RC" --force > /dev/null 2>&1; then
    ok 'T6 --force 可强行安装'
else
    bad 'T6 --force 仍失败'
fi

reset_home
check 'T7 装前 status 报未安装' \
    "$(inst status --rc "$RC" | grep -c '未安装')" 1
inst install --rc "$RC" > /dev/null
check 'T7 装后 status 报已安装' \
    "$(inst status --rc "$RC" | grep -c '已安装')" 1

if inst_as /usr/bin/fish install --rc "$RC" > /dev/null 2>&1; then
    bad 'T8 fish 应被拒绝'
else
    ok 'T8 fish 被明确拒绝'
fi

printf '\n== snippet 语义 ==\n'
reset_home
inst install --rc "$RC" > /dev/null

GITDIR="$WORK/proj-git"
mkdir -p "$GITDIR"
(
    cd "$GITDIR"
    git init -q
    git remote add origin https://example.invalid/probe.git
)
PLAINDIR="$WORK/proj-plain"
mkdir -p "$PLAINDIR"
CJKDIR="$WORK/中文项目"
mkdir -p "$CJKDIR"

IFS=' ' read -ra TEST_SHELLS <<< "${SUMPTER_TEST_SHELLS:-/bin/bash /bin/zsh}"
for sh in "${TEST_SHELLS[@]}"; do
    [ -x "$sh" ] || {
        ok "跳过 $sh(不存在)"
        continue
    }
    name="$(basename "$sh")"
    out="$(run_snip "$sh" "$GITDIR")"
    check "[$name] git 项目名" "$(header_val "$out" X-Sumpter-Project)" proj-git
    check "[$name] git workspace" "$(header_val "$out" X-Sumpter-Workspace)" "$GITDIR"
    check "[$name] git remote" "$(header_val "$out" X-Sumpter-Git-Remote)" \
        'https://example.invalid/probe.git'
    check "[$name] 用户名" "$(header_val "$out" X-Sumpter-User)" kkl

    out="$(run_snip "$sh" "$PLAINDIR")"
    check "[$name] 非 git 无 remote" "$(header_val "$out" X-Sumpter-Git-Remote)" ''
    check "[$name] 非 git 项目名" "$(header_val "$out" X-Sumpter-Project)" proj-plain

    out="$(run_snip "$sh" "$CJKDIR")"
    check "[$name] 中文目录无 Project" "$(header_val "$out" X-Sumpter-Project)" ''
    check "[$name] 中文目录无 Workspace" "$(header_val "$out" X-Sumpter-Workspace)" ''
    check "[$name] 中文目录仍有用户名" "$(header_val "$out" X-Sumpter-User)" kkl

    out="$(run_snip "$sh" "$GITDIR" '{"models":{"extra_headers":{"X-Foo":"keep"}}}')"
    check "[$name] 合并保留外部 header" "$(header_val "$out" X-Foo)" keep
    check "[$name] 合并后仍有项目名" "$(header_val "$out" X-Sumpter-Project)" proj-git
done

printf '\n通过 %d,失败 %d\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
printf 'selftest OK\n'
