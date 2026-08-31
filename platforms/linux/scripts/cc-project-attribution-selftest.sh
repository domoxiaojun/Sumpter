#!/usr/bin/env bash
# cc-project-attribution.sh 的自测。全部在临时 HOME 与临时 PATH 里跑,
# 不读写真实 ~/.zshrc、~/.claude 或 ~/.local/share。
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
INSTALLER="$SCRIPT_DIR/cc-project-attribution.sh"
[ -x "$INSTALLER" ] || {
    printf 'selftest FAIL: 找不到可执行的 %s\n' "$INSTALLER" >&2
    exit 1
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

FAKE_HOME="$WORK/home"
RC="$FAKE_HOME/.zshrc"
SNIP="$FAKE_HOME/.local/share/sumpter/cc-project-attribution.sh"
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

# 隔离调用安装器:HOME/XDG 都指向临时目录
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
    grep -c '>>> sumpter cc-project-attribution >>>' "$RC" 2> /dev/null || true
}
backups() {
    find "$FAKE_HOME" -maxdepth 1 -type f -name '.zshrc.sumpter-bak-*' | wc -l | tr -d ' '
}

# 假 claude:把它实际收到的 ANTHROPIC_CUSTOM_HEADERS 原样打出来,
# 这样每条 snippet 断言都是穿过 wrapper 的端到端验证。
mkdir -p "$WORK/bin"
cat > "$WORK/bin/claude" <<'STUB'
#!/bin/sh
printf '%s' "${ANTHROPIC_CUSTOM_HEADERS:-}"
STUB
chmod +x "$WORK/bin/claude"

run_snip_pwd() { # $1=shell $2=cd目标 $3=伪造的 PWD(复现父进程传入带尾随斜杠的 cwd)
    local sh="$1" dir="$2" fake="$3" runner="$WORK/runner.sh"
    printf 'cd "%s" || exit 1\nPWD="%s"\n. "%s"\nclaude\n' "$dir" "$fake" "$SNIP" > "$runner"
    HOME="$FAKE_HOME" PATH="$WORK/bin:$PATH" GIT_CEILING_DIRECTORIES="$WORK" "$sh" "$runner"
}

run_snip() { # $1=shell $2=cwd $3=预置 ANTHROPIC_CUSTOM_HEADERS
    local sh="$1" dir="$2" preset="${3:-}" runner="$WORK/runner.sh"
    printf 'cd "%s" || exit 1\n. "%s"\nclaude\n' "$dir" "$SNIP" > "$runner"
    ANTHROPIC_CUSTOM_HEADERS="$preset" HOME="$FAKE_HOME" PATH="$WORK/bin:$PATH" \
        GIT_CEILING_DIRECTORIES="$WORK" "$sh" "$runner"
}

printf '== 安装器 ==\n'

# T1 全新安装
reset_home
inst install --rc "$RC" > /dev/null
check 'T1 装出恰好一个标记块' "$(blocks)" 1
check 'T1 snippet 已生成' "$([ -f "$SNIP" ] && echo yes || echo no)" yes
check 'T1 保留用户原有内容' "$(grep -c "$SEED" "$RC")" 1

# T2 幂等:重复安装不重复追加
inst install --rc "$RC" > /dev/null
check 'T2 重装后仍只有一个块' "$(blocks)" 1

# T3 dry-run 不落任何改动
reset_home
inst install --rc "$RC" --dry-run > /dev/null
check 'T3 dry-run 未改 rc' "$(blocks)" 0
check 'T3 dry-run 未写 snippet' "$([ -f "$SNIP" ] && echo yes || echo no)" no
check 'T3 dry-run 未建备份' "$(backups)" 0

# T4 备份与还原
reset_home
inst install --rc "$RC" > /dev/null
check 'T4 已建备份' "$(backups)" 1
printf 'JUNK_LINE\n' >> "$RC"
inst restore --rc "$RC" > /dev/null
check 'T4 还原后 JUNK 消失' "$(grep -c JUNK_LINE "$RC" || true)" 0
check 'T4 还原后标记块消失' "$(blocks)" 0
check 'T4 还原后原有内容在' "$(grep -c "$SEED" "$RC")" 1

# T5 卸载
reset_home
inst install --rc "$RC" > /dev/null
inst uninstall --rc "$RC" > /dev/null
check 'T5 卸载后无标记块' "$(blocks)" 0
check 'T5 卸载后 snippet 删除' "$([ -f "$SNIP" ] && echo yes || echo no)" no
check 'T5 卸载后原有内容在' "$(grep -c "$SEED" "$RC")" 1
check 'T5 卸载保留备份' "$(backups)" 1

# T6 settings.json 写死该键时必须拒装(否则装了永远不生效)
reset_home
mkdir -p "$FAKE_HOME/.claude"
printf '{"env":{"ANTHROPIC_CUSTOM_HEADERS":"X-Sumpter-Project: pinned"}}\n' \
    > "$FAKE_HOME/.claude/settings.json"
if inst install --rc "$RC" > /dev/null 2>&1; then
    bad 'T6 应拒装但成功了'
else
    ok 'T6 检出 settings.json 覆盖陷阱并拒装'
fi
check 'T6 拒装后未改 rc' "$(blocks)" 0
if inst install --rc "$RC" --force > /dev/null 2>&1; then
    ok 'T6 --force 可强行安装'
else
    bad 'T6 --force 仍失败'
fi

# T7 status 文案
reset_home
check 'T7 装前 status 报未安装' \
    "$(inst status --rc "$RC" | grep -c '未安装')" 1
inst install --rc "$RC" > /dev/null
check 'T7 装后 status 报已安装' \
    "$(inst status --rc "$RC" | grep -c '已安装')" 1

# T8 shell 探测
if inst_as /usr/bin/fish install --rc "$RC" > /dev/null 2>&1; then
    bad 'T8 fish 应被拒绝'
else
    ok 'T8 fish 被明确拒绝'
fi
# fish 不支持自动安装,但必须给出 fish-snippet 指引(两条触发路径都要)
check 'T8 fish(探测) 指向 fish-snippet' \
    "$(inst_as /usr/bin/fish install 2>&1 | grep -c 'fish-snippet')" 1
check 'T8 fish(--shell) 指向 fish-snippet' \
    "$(inst install --shell fish 2>&1 | grep -c 'fish-snippet')" 1
check 'T8 fish-snippet 输出 claude 函数' \
    "$(inst fish-snippet | grep -c '^function claude')" 1
check 'T8 fish-snippet 标注未实测' \
    "$(inst fish-snippet | grep -c '未经实测')" 1

if inst_as /bin/false install --rc "$RC" > /dev/null 2>&1; then
    bad 'T8 未知 shell 应被拒绝'
else
    ok 'T8 未知 shell 被拒绝'
fi

# T9 macOS 的 bash 优先 .bash_profile
if [ "$(uname -s)" = Darwin ]; then
    reset_home
    printf '%s\n' "$SEED" > "$FAKE_HOME/.bash_profile"
    check 'T9 Darwin+bash 选 .bash_profile' \
        "$(inst_as /bin/bash status | grep -c 'bash_profile')" 1
else
    ok 'T9 跳过(非 Darwin)'
fi

# T10 两个 OS 分支都用假 uname 覆盖,这样在任何平台上跑结论都一样
#（本机跑不了真 Linux,CI 的 Linux runner 上也跑不了 Darwin）。
reset_home
printf '%s\n' "$SEED" > "$FAKE_HOME/.bash_profile"
for os in Linux Darwin; do
    mkdir -p "$WORK/fakeos-$os"
    printf '#!/bin/sh\necho %s\n' "$os" > "$WORK/fakeos-$os/uname"
    chmod +x "$WORK/fakeos-$os/uname"
done
check 'T10 假 Linux + bash 选 .bashrc' \
    "$(PATH="$WORK/fakeos-Linux:$PATH" inst_as /bin/bash status | grep -c '\.bashrc')" 1
check 'T10 假 Darwin + bash 选 .bash_profile' \
    "$(PATH="$WORK/fakeos-Darwin:$PATH" inst_as /bin/bash status | grep -c 'bash_profile')" 1

printf '\n== snippet 语义(穿过 wrapper 端到端) ==\n'

reset_home
inst install --rc "$RC" > /dev/null

GITDIR="$WORK/proj-git"
mkdir -p "$GITDIR"
(
    cd "$GITDIR"
    git init -q
    git remote add origin https://example.invalid/probe.git
)
ALTDIR="$WORK/proj-altremote"
mkdir -p "$ALTDIR"
(
    cd "$ALTDIR"
    git init -q
    git remote add sumpter https://example.invalid/alt.git
)
PLAINDIR="$WORK/proj-plain"
mkdir -p "$PLAINDIR"
CJKDIR="$WORK/中文项目"
mkdir -p "$CJKDIR"

# 待测 shell 可覆盖:CI 的 Linux runner 通常没有 zsh,只跑 bash 分支。
IFS=' ' read -ra TEST_SHELLS <<< "${SUMPTER_TEST_SHELLS:-/bin/bash /bin/zsh}"
for sh in "${TEST_SHELLS[@]}"; do
    [ -x "$sh" ] || {
        ok "跳过 $sh(不存在)"
        continue
    }
    name="$(basename "$sh")"

    out="$(run_snip "$sh" "$GITDIR")"
    check "[$name] git 仓库发 3 条" "$(printf '%s\n' "$out" | grep -c '^X-Sumpter-')" 3
    check "[$name] 项目名取目录末段" \
        "$(printf '%s\n' "$out" | grep '^X-Sumpter-Project:')" 'X-Sumpter-Project: proj-git'
    check "[$name] git remote 正确" \
        "$(printf '%s\n' "$out" | grep '^X-Sumpter-Git-Remote:')" \
        'X-Sumpter-Git-Remote: https://example.invalid/probe.git'

    out="$(run_snip "$sh" "$PLAINDIR")"
    check "[$name] 非 git 目录发 2 条" "$(printf '%s\n' "$out" | grep -c '^X-Sumpter-')" 2
    check "[$name] 非 git 不发 Git-Remote" \
        "$(printf '%s\n' "$out" | grep -c 'Git-Remote' || true)" 0

    # 非 ASCII:一条都不能发,否则 CC 会拒绝启动
    out="$(run_snip "$sh" "$CJKDIR")"
    check "[$name] 中文目录名不发任何 header" "$out" ''

    # 合并:保留外部 header
    out="$(run_snip "$sh" "$GITDIR" 'X-Foo: keep-me')"
    check "[$name] 保留外部 header" "$(printf '%s\n' "$out" | grep -c '^X-Foo: keep-me')" 1
    check "[$name] 外部+自身共 4 条" "$(printf '%s\n' "$out" | grep -c '^X-')" 4

    # PWD 带尾随斜杠时项目名不能变空(实测踩到过:只发 workspace 不发 project)
    out="$(run_snip_pwd "$sh" "$GITDIR" "$GITDIR/")"
    check "[$name] 尾随斜杠仍取到项目名" \
        "$(printf '%s\n' "$out" | grep '^X-Sumpter-Project:')" 'X-Sumpter-Project: proj-git'
    check "[$name] 尾随斜杠被规范化掉" \
        "$(printf '%s\n' "$out" | grep '^X-Sumpter-Workspace:')" "X-Sumpter-Workspace: $GITDIR"

    # 没有 origin 的仓库退回第一个 remote
    out="$(run_snip "$sh" "$ALTDIR")"
    check "[$name] 非 origin remote 也能取到" \
        "$(printf '%s\n' "$out" | grep '^X-Sumpter-Git-Remote:')" \
        'X-Sumpter-Git-Remote: https://example.invalid/alt.git'

    # 非 ASCII 目录下也不能把继承的过期归因透传出去
    out="$(run_snip "$sh" "$CJKDIR" 'X-Sumpter-Project: stale')"
    check "[$name] 中文目录+继承过期值 → 清除" "$out" ''
    out="$(run_snip "$sh" "$CJKDIR" 'X-Foo: keep-me')"
    check "[$name] 中文目录仍保留外部 header" "$out" 'X-Foo: keep-me'

    # upsert:替换而非叠加已有的 X-Sumpter-*
    out="$(run_snip "$sh" "$GITDIR" 'X-Sumpter-Project: stale')"
    check "[$name] 旧 X-Sumpter 值被替换" "$(printf '%s\n' "$out" | grep -c 'stale' || true)" 0
    check "[$name] Project 不重复" "$(printf '%s\n' "$out" | grep -c '^X-Sumpter-Project:')" 1
done

printf '\n通过 %d,失败 %d\n' "$pass" "$fail"
[ "$fail" -eq 0 ] || exit 1
printf 'selftest OK\n'
