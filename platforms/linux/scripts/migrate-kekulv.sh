#!/usr/bin/env bash
# Migrate the standard root/system Kekulv installation without deleting its data.
set -Eeuo pipefail
umask 077

VERSION=""
ADMIN_HOST=""
ADMIN_PORT=""
CHECK_ONLY=0
SYSTEM_ROOT=""
WORK_DIR=""
BACKUP_DIR=""
LOCK_DIR=""
LOCKED=0
TRANSACTION=0
NEW_STARTED=0
DATA_CREATED=0
OLD_ACTIVE=0
OLD_ENABLED=0

die() { echo "错误：$*" >&2; exit 1; }
note() { echo "[sumpter-migrate] $*"; }
exists() { [[ -e "$1" || -L "$1" ]]; }
usage() {
    cat <<'EOF'
用法：sudo bash migrate-kekulv.sh [--version vX.Y.Z] [--admin-host IP] [--admin-port PORT] [--check]

仅迁移标准 Linux system 安装：
  kekulv.service /opt/kekulv /var/lib/kekulv
  → sumpter.service /opt/sumpter /var/lib/sumpter

先从 domoxiaojun/sumpter 的 GitHub Release 下载并校验 SHA256SUMS，再停服复制。
默认下载 latest；Admin 监听继承旧安装器 drop-in（未设置则 127.0.0.1:57879）。
保留 config.json、admin-password、统计和 SQLite/WAL 文件，复制件交给新版迁移 schema。
原 /var/lib/kekulv 保持不变；旧程序和 unit 移至 /var/lib/sumpter-migration.* 备份。
新服务连续通过存活检查后才卸下旧程序；失败自动恢复原服务启停状态。
已有 Sumpter 目录或服务时拒绝覆盖；不支持 user 安装或自定义 systemd unit。
--check 只检查布局和参数，不下载、不停服、不改文件。
--admin-host 0.0.0.0 可继续使用原外网绑定；公网 Admin 仍需现有 HTTPS 反代。
EOF
}

while (($#)); do
    case "$1" in
        --version|--admin-host|--admin-port)
            (($# >= 2)) && [[ -n $2 ]] || die "$1 缺少参数"
            case "$1" in
                --version) VERSION=$2;;
                --admin-host) ADMIN_HOST=$2;;
                --admin-port) ADMIN_PORT=$2;;
            esac
            shift 2;;
        --check) CHECK_ONLY=1; shift;;
        -h|--help) usage; exit 0;;
        *) die "未知参数：$1";;
    esac
done
[[ -z $VERSION || $VERSION =~ ^v[0-9]+\.[0-9]+\.[0-9]+([.-][A-Za-z0-9.-]+)?$ ]] || die "版本须为 vX.Y.Z 格式"

# The only path override is the same isolated rootfs contract used by installer tests.
if [[ ${SUMPTER_MIGRATION_SELFTEST:-} == 1 && ${SUMPTER_INSTALLER_SELFTEST:-} == 1 ]]; then
    SYSTEM_ROOT=$(realpath -e "${SUMPTER_SYSTEM_TEST_ROOT:?}")
    test_base=$(realpath -e "${TMPDIR:-/tmp}")
    case "$SYSTEM_ROOT" in "$test_base"/sumpter-installer-test.*/rootfs) ;; *) die "无效测试 rootfs";; esac
else
    [[ -z ${SUMPTER_SYSTEM_TEST_ROOT:-} && -z ${SUMPTER_MIGRATION_SELFTEST:-} ]] || die "拒绝测试路径覆盖"
    [[ $(uname -s) == Linux ]] || die "请在旧服务所在的 Linux 服务器运行"
    [[ $EUID -eq 0 ]] || die "/var/lib/kekulv 是 system 安装，请用 sudo 或 root 运行"
fi
for command_name in awk bash chmod chown cp curl find id install mkdir mktemp mv realpath rm rmdir sha256sum sleep systemctl tar uname; do
    command -v "$command_name" >/dev/null || die "缺少命令：$command_name"
done
if [[ -z $SYSTEM_ROOT ]] && ! id -u sumpter >/dev/null 2>&1; then
    command -v useradd >/dev/null || die "缺少创建 sumpter 系统用户的命令：useradd"
fi
DATA_PARENT="$SYSTEM_ROOT/var/lib"
PROGRAM_PARENT="$SYSTEM_ROOT/opt"
UNIT_DIR="$SYSTEM_ROOT/etc/systemd/system"
OLD_DATA="$DATA_PARENT/kekulv"
NEW_DATA="$DATA_PARENT/sumpter"
OLD_PROGRAM="$PROGRAM_PARENT/kekulv"
OLD_UNIT="$UNIT_DIR/kekulv.service"
NEW_UNIT="$UNIT_DIR/sumpter.service"

regular_tree() {
    local target=$1 bad
    [[ -d $target && ! -L $target && $(realpath -e "$target") == "$target" ]] || die "目录不是原位普通目录：$target"
    bad=$(find "$target" ! -type f ! -type d -print -quit)
    [[ -z $bad ]] || die "目录含链接或特殊文件，请先处理：$target"
}
check_layout() {
    local target fragment execution dropin
    for target in "$DATA_PARENT" "$PROGRAM_PARENT" "$UNIT_DIR"; do
        [[ -d $target && ! -L $target && $(realpath -e "$target") == "$target" ]] || die "父目录被重定向：$target"
    done
    regular_tree "$OLD_DATA"
    [[ -f $OLD_DATA/config.json && -f $OLD_DATA/admin-password ]] || die "旧目录缺少 config.json 或 admin-password，停止自动迁移"
    [[ -d $OLD_PROGRAM && ! -L $OLD_PROGRAM && -f $OLD_PROGRAM/kekulvd && ! -L $OLD_PROGRAM/kekulvd ]] || die "未找到标准 /opt/kekulv 程序"
    [[ -f $OLD_UNIT && ! -L $OLD_UNIT ]] || die "未找到普通文件 /etc/systemd/system/kekulv.service"
    for target in "$PROGRAM_PARENT/sumpter" "$PROGRAM_PARENT/sumpter.previous" "$NEW_DATA" "$NEW_UNIT" "$NEW_UNIT.previous" "$NEW_UNIT.d"; do
        ! exists "$target" || die "目标已存在，拒绝覆盖：$target"
    done
    systemctl show-environment >/dev/null || die "无法连接 systemd system manager"
    [[ $(systemctl show sumpter.service -p LoadState --value) == not-found ]] || die "已存在 Sumpter 服务，拒绝覆盖"
    fragment=$(systemctl show kekulv.service -p FragmentPath --value)
    [[ $fragment == "$OLD_UNIT" ]] || die "旧服务不是标准 system unit"
    execution=$(systemctl show kekulv.service -p ExecStart --value)
    [[ $execution == *'/opt/kekulv/kekulvd --systemd-scope system --config-dir /var/lib/kekulv --web-root /opt/kekulv/web ;'* ]] || die "旧 ExecStart 已自定义，需先核对迁移路径"
    [[ $(systemctl show kekulv.service -p User --value) == kekulv ]] || die "旧服务用户不是 kekulv"
    dropin="$OLD_UNIT.d/50-admin-listen.conf"
    if exists "$OLD_UNIT.d"; then
        regular_tree "$OLD_UNIT.d"
        [[ -z $(find "$OLD_UNIT.d" -mindepth 1 ! -path "$dropin" -print -quit) ]] || die "旧服务含自定义 drop-in，需先核对迁移"
    fi
    local dropins
    dropins=$(systemctl show kekulv.service -p DropInPaths --value)
    [[ -z $dropins || $dropins == "$dropin" ]] || die "旧服务含其他位置的 drop-in，需先核对迁移"
}

check_layout
old_host=127.0.0.1
old_port=57879
if [[ -f $OLD_UNIT.d/50-admin-listen.conf ]]; then
    while IFS= read -r line || [[ -n $line ]]; do
        case "$line" in
            ''|'#'*|'[Service]') ;;
            Environment=KEKULV_ADMIN_HOST=*) old_host=${line#Environment=KEKULV_ADMIN_HOST=};;
            Environment=KEKULV_ADMIN_PORT=*) old_port=${line#Environment=KEKULV_ADMIN_PORT=};;
            Environment=KEKULV_ADMIN_PASSWORD_FILE=/var/lib/kekulv/admin-password) ;;
            *) die "旧 Admin drop-in 含自定义指令或外部密码路径，停止自动迁移";;
        esac
    done <"$OLD_UNIT.d/50-admin-listen.conf"
fi
ADMIN_HOST=${ADMIN_HOST:-$old_host}
ADMIN_PORT=${ADMIN_PORT:-$old_port}
if [[ $ADMIN_HOST == localhost || $ADMIN_HOST =~ ^[a-fA-F0-9:]+:[a-fA-F0-9:]*$ ]]; then
    :
elif [[ $ADMIN_HOST =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]]; then
    IFS=. read -r -a octets <<<"$ADMIN_HOST"
    for octet in "${octets[@]}"; do ((10#$octet <= 255)) || die "Admin IPv4 地址无效"; done
else
    die "Admin host 必须是 IP 或 localhost"
fi
if [[ ! $ADMIN_PORT =~ ^[0-9]{1,5}$ ]] || ((10#$ADMIN_PORT < 1 || 10#$ADMIN_PORT > 65535)); then
    die "Admin port 必须在 1–65535"
fi
ADMIN_PORT=$((10#$ADMIN_PORT))
note "迁移 /var/lib/kekulv → /var/lib/sumpter；Admin ${ADMIN_HOST}:${ADMIN_PORT}；版本 ${VERSION:-latest}"
if ((CHECK_ONLY)); then note "检查通过；未下载、停服或修改文件"; exit 0; fi

# A mkdir lock also works on minimal systems without flock. Never remove another run's lock.
LOCK_DIR="$DATA_PARENT/.sumpter-kekulv-migration.lock"
mkdir -- "$LOCK_DIR" 2>/dev/null || die "迁移锁已存在：${LOCK_DIR}；确认没有迁移进程后才可移除空锁目录"
LOCKED=1

rollback() {
    local ok=1 target label
    note "迁移失败，恢复旧服务；新版本失败现场保留在 $BACKUP_DIR"
    if ((NEW_STARTED)) && [[ $(systemctl show sumpter.service -p LoadState --value) != not-found ]]; then
        systemctl stop sumpter.service || return 1
        if systemctl is-active --quiet sumpter.service; then return 1; fi
        systemctl disable sumpter.service || ok=0
    fi
    mkdir -p "$BACKUP_DIR/failed-sumpter" || return 1
    for target in "$PROGRAM_PARENT/sumpter" "$NEW_UNIT" "$NEW_UNIT.d"; do
        if exists "$target"; then mv -- "$target" "$BACKUP_DIR/failed-sumpter/" || ok=0; fi
    done
    if ((DATA_CREATED)) && exists "$NEW_DATA"; then
        mv -- "$NEW_DATA" "$BACKUP_DIR/failed-data" || ok=0
    fi
    for label in program previous unit unit.previous dropins; do
        case "$label" in
            program) target=$OLD_PROGRAM;; previous) target=$OLD_PROGRAM.previous;;
            unit) target=$OLD_UNIT;; unit.previous) target=$OLD_UNIT.previous;; dropins) target=$OLD_UNIT.d;;
        esac
        if exists "$BACKUP_DIR/$label"; then
            if exists "$target"; then ok=0
            else mv -- "$BACKUP_DIR/$label" "$target" || ok=0; fi
        fi
    done
    systemctl daemon-reload || ok=0
    if ((OLD_ENABLED)); then systemctl enable kekulv.service || ok=0; fi
    if ((ok && OLD_ACTIVE)); then
        systemctl start kekulv.service || ok=0
        systemctl is-active --quiet kekulv.service || ok=0
    fi
    ((ok))
}
cleanup() {
    local status=$?
    trap - EXIT INT TERM
    if ((TRANSACTION)); then
        if rollback; then note "已恢复旧服务状态，旧数据仍在 /var/lib/kekulv"
        else echo "自动回滚未完全成功；请检查 systemctl status kekulv sumpter 与 $BACKUP_DIR，勿删除旧数据" >&2; fi
        status=1
    fi
    if [[ -n $WORK_DIR ]]; then rm -rf -- "$WORK_DIR"; fi
    if ((LOCKED)); then rmdir -- "$LOCK_DIR" || true; fi
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

WORK_DIR=$(mktemp -d "${TMPDIR:-/tmp}/sumpter-migrate.XXXXXX")
WORK_DIR=$(realpath -e "$WORK_DIR")
case $(uname -m) in x86_64|amd64) arch=x86_64;; aarch64|arm64) arch=aarch64;; *) die "只支持 x86_64 和 aarch64";; esac
package_name="sumpter-linux-$arch"
asset="$package_name.tar.gz"
base_url=https://github.com/domoxiaojun/sumpter/releases/latest/download
if [[ -n $VERSION ]]; then base_url="https://github.com/domoxiaojun/sumpter/releases/download/$VERSION"; fi
for resource in "$asset" SHA256SUMS; do
    note "下载 $resource"
    curl --fail --location --silent --show-error --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --retry 3 --connect-timeout 15 --max-time 600 "$base_url/$resource" --output "$WORK_DIR/$resource"
done
expected=$(awk -v wanted="$asset" '{ name=$2; sub(/^\*/, "", name); sub(/^\.\//, "", name); sub(/\r$/, "", name); if (name==wanted) print tolower($1) }' "$WORK_DIR/SHA256SUMS")
[[ $expected =~ ^[a-f0-9]{64}$ ]] || die "SHA256SUMS 中缺少唯一有效校验值"
actual=$(sha256sum "$WORK_DIR/$asset" | awk '{print tolower($1)}')
[[ $actual == "$expected" ]] || die "SHA-256 校验失败；旧服务未停止"
tar -tzf "$WORK_DIR/$asset" >"$WORK_DIR/entries"
tar -tvzf "$WORK_DIR/$asset" | awk 'substr($1,1,1)!="-" && substr($1,1,1)!="d" { bad=1 } END { exit bad }' || die "发布包含链接或特殊文件"
while IFS= read -r entry; do
    entry=${entry#./}
    [[ $entry == "$package_name/"* && /$entry/ != */../* ]] || die "发布包路径不安全"
done <"$WORK_DIR/entries"
mkdir "$WORK_DIR/extracted"
tar --no-same-owner --no-same-permissions -xzf "$WORK_DIR/$asset" -C "$WORK_DIR/extracted"
package_root="$WORK_DIR/extracted/$package_name"
regular_tree "$package_root"
for required in sumpterd scripts/install.sh config.example.json sumpter-system.service web/index.html; do
    [[ -f $package_root/$required ]] || die "发布包缺少 $required"
done
bash -n "$package_root/scripts/install.sh"
note "发布包 SHA-256 与路径检查通过"

# Re-check after downloads; another normal installer might have run in the meantime.
check_layout
enabled_state=$(systemctl is-enabled kekulv.service) || true
case "$enabled_state" in enabled) OLD_ENABLED=1;; disabled) ;; *) die "旧服务启用状态不是 enabled/disabled，停止自动迁移";; esac
systemctl is-active --quiet kekulv.service && OLD_ACTIVE=1
BACKUP_DIR=$(mktemp -d "$DATA_PARENT/sumpter-migration.XXXXXX")
printf 'old_active=%s\nold_enabled=%s\nversion=%s\n' "$OLD_ACTIVE" "$OLD_ENABLED" "${VERSION:-latest}" >"$BACKUP_DIR/state.txt"
TRANSACTION=1
systemctl disable kekulv.service
systemctl stop kekulv.service
if systemctl is-active --quiet kekulv.service; then die "旧服务仍在运行，未复制数据"; fi
[[ $(systemctl show kekulv.service -p MainPID --value) == 0 ]] || die "旧服务进程未退出，未复制数据"
regular_tree "$OLD_DATA"
note "旧服务已停止；复制完整数据目录（包括 SQLite WAL），原数据保持原位"
mkdir "$NEW_DATA"
DATA_CREATED=1
cp -a "$OLD_DATA/." "$NEW_DATA/"
regular_tree "$NEW_DATA"

# The package installer only chowns config/password; all copied SQLite files also need the new owner.
if [[ -z $SYSTEM_ROOT ]]; then
    if ! id -u sumpter >/dev/null 2>&1; then
        useradd --system --user-group --home-dir /var/lib/sumpter --no-create-home --shell /usr/sbin/nologin sumpter
    fi
    [[ $(id -gn sumpter) == sumpter && $(id -u sumpter) != 0 ]] || die "现有 sumpter 用户或主组不符合安装要求"
    chown -R sumpter:sumpter "$NEW_DATA"
fi
find "$NEW_DATA" -type d -exec chmod 0700 {} +
find "$NEW_DATA" -type f -exec chmod 0600 {} +
if [[ -z $SYSTEM_ROOT ]] && command -v restorecon >/dev/null; then restorecon -RF "$NEW_DATA"; fi

NEW_STARTED=1
(
    unset SUMPTER_ADMIN_HOST SUMPTER_ADMIN_PORT SUMPTER_ADMIN_PASSWORD_FILE
    bash "$package_root/scripts/install.sh" --admin-host "$ADMIN_HOST" --admin-port "$ADMIN_PORT"
)
probe_host=$ADMIN_HOST
case "$probe_host" in 0.0.0.0) probe_host=127.0.0.1;; ::) probe_host='[::1]';; *:*) probe_host="[$probe_host]";; esac
main_pid=$(systemctl show sumpter.service -p MainPID --value)
[[ $main_pid =~ ^[1-9][0-9]*$ ]] || die "新服务没有运行中的进程"
for ((attempt=0; attempt<5; attempt++)); do
    sleep 1
    systemctl is-active --quiet sumpter.service || die "新服务退出"
    [[ $(systemctl show sumpter.service -p MainPID --value) == "$main_pid" ]] || die "新服务反复重启"
    code=$(curl --silent --show-error --noproxy '*' --connect-timeout 2 --max-time 5 --output /dev/null \
        --write-out '%{http_code}' "http://$probe_host:$ADMIN_PORT/healthz")
    [[ $code == 204 ]] || die "新服务 /healthz 检查失败"
done

# Retire, rather than purge, the old installation only after the new service is healthy.
for label in program previous unit unit.previous dropins; do
    case "$label" in
        program) target=$OLD_PROGRAM;; previous) target=$OLD_PROGRAM.previous;;
        unit) target=$OLD_UNIT;; unit.previous) target=$OLD_UNIT.previous;; dropins) target=$OLD_UNIT.d;;
    esac
    if exists "$target"; then mv -- "$target" "$BACKUP_DIR/$label"; fi
done
systemctl daemon-reload
TRANSACTION=0
note "迁移成功：sumpter.service 已运行，kekulv.service 已停用并卸下"
note "新数据：/var/lib/sumpter；旧数据保留：/var/lib/kekulv"
note "旧程序与服务备份：${BACKUP_DIR}；未删除旧系统用户"
note "检查服务：sudo systemctl status sumpter.service；管理页使用原登录凭据"
