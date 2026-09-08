#!/usr/bin/env bash
# Download the current architecture package from GitHub Releases (or another
# HTTPS root) and invoke the package's transactional installer. This bootstrap
# intentionally does not verify a SHA-256 checksum; it still rejects unsafe
# archive layouts. Prefer scripts/install.sh --repo for checksum verification.
#
# Bootstrap-only options: --base-url, -h/--help.
# All other arguments (and values after --) are forwarded to package install.sh,
# e.g. --admin-password-file /absolute/path/admin-password (advanced override).
set -Eeuo pipefail
umask 077

DEFAULT_DOWNLOAD_BASE="https://github.com/domoxiaojun/sumpter/releases/latest/download"
DOWNLOAD_BASE="${SUMPTER_DOWNLOAD_BASE:-$DEFAULT_DOWNLOAD_BASE}"
WORK_DIR=""
INSTALL_ARGS=()

die() {
    echo "错误:$*" >&2
    exit 1
}

note() {
    echo "[sumpter-bootstrap] $*"
}

usage() {
    cat <<'EOF'
用法:
  bootstrap-install.sh [--base-url HTTPS_URL] [install.sh 选项...]
  bootstrap-install.sh [--base-url HTTPS_URL] -- [install.sh 选项...]

从 GitHub Release 下载当前 Linux 架构的 sumpter 发布包，再运行包内安装器。
默认地址: https://github.com/domoxiaojun/sumpter/releases/latest/download
下载路径: <base-url>/sumpter-linux-<x86_64|aarch64>.tar.gz
归因配置器也随包安装到 `/opt/sumpter/scripts/`（普通用户安装则在
`~/.local/share/sumpter/scripts/`）。客户端主机也可从仓库 raw 下载统一安装器：
https://raw.githubusercontent.com/domoxiaojun/sumpter/main/platforms/linux/scripts/setup-client-attribution.sh

需要 SHA-256 校验或钉死版本时，请使用 scripts/install.sh --repo domoxiaojun/sumpter [--version vX.Y.Z]，
不要把 --repo/--version 加在本脚本后。

引导脚本自身选项:
  --base-url HTTPS_URL  覆盖下载根目录（须提供同名 tar.gz）
  -h, --help            显示帮助

其余参数原样传给包内 scripts/install.sh，例如（高级路径覆盖）:
  --admin-password-file /absolute/path/admin-password

注意：这里的 `--base-url` 只用于下载 Linux 发布包。归因脚本默认从 GitHub 仓库 raw 获取，
与本引导安装器的发布包地址不是同一个 URL。

示例:
  bash sumpter-install.sh
  sudo bash sumpter-install.sh
  bash sumpter-install.sh --admin-password-file /absolute/path/admin-password
  SUMPTER_ADMIN_PASSWORD_FILE=/absolute/path/admin-password bash sumpter-install.sh

注意: 此引导安装器按发布方要求不校验 SHA-256。它仍会拒绝路径穿越、
符号链接和错误的发布包根目录，但 HTTPS 传输与镜像内容本身仍须由发布方负责。
安装器会生成配置目录/admin-password；首次登录用户名为 kkl，之后可在 WebUI 安全页修改。
Admin API/SSE 使用会话 Cookie；公网仍必须使用外层 HTTPS，推荐保持 Admin loopback 绑定。

密码安全:
  正常安装只显示密码文件路径，不会在终端回显密码内容。
  首次登录前可用 cat ~/.config/sumpter/admin-password（user）或
  sudo cat /var/lib/sumpter/admin-password（system）主动查看。WebUI 修改凭据后文件改为哈希 JSON。
  不要使用 bash -x、set -x 或其它 shell 跟踪方式运行安装器，以免调试输出泄露密码。
EOF
}

cleanup() {
    local status=$?

    trap - EXIT
    if [[ -n "$WORK_DIR" ]]; then
        case "$WORK_DIR" in
            "${TMPDIR:-/tmp}"/sumpter-bootstrap.*)
                if [[ -d "$WORK_DIR" && ! -L "$WORK_DIR" ]]; then
                    rm -rf -- "$WORK_DIR"
                fi
                ;;
            *)
                echo "拒绝清理非引导安装临时目录:$WORK_DIR" >&2
                ;;
        esac
    fi
    exit "$status"
}
trap cleanup EXIT

while (($# > 0)); do
    case "$1" in
        --base-url)
            (($# >= 2)) || die "--base-url 缺少 HTTPS_URL"
            DOWNLOAD_BASE="$2"
            shift 2
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        --repo | --version)
            die "引导安装器自己下载发布包，不支持 $1。请改用 scripts/install.sh $1 ..."
            ;;
        --)
            shift
            INSTALL_ARGS+=("$@")
            break
            ;;
        *)
            # 非引导选项一律交给包内 install.sh（如 --admin-password-file）。
            INSTALL_ARGS+=("$1")
            shift
            ;;
    esac
done

for forwarded in "${INSTALL_ARGS[@]+"${INSTALL_ARGS[@]}"}"; do
    case "$forwarded" in
        --repo | --version | --repo=* | --version=*)
            die "引导安装器自己下载发布包，不支持 ${forwarded}。请改用 scripts/install.sh --repo domoxiaojun/sumpter"
            ;;
    esac
done

[[ "$DOWNLOAD_BASE" == https://* && "$DOWNLOAD_BASE" != *$'\n'* ]] \
    || die "--base-url 必须是无换行的 HTTPS URL"
DOWNLOAD_BASE="${DOWNLOAD_BASE%/}"
[[ "$DOWNLOAD_BASE" != "https:" && "$DOWNLOAD_BASE" != "https://" ]] \
    || die "--base-url 缺少主机名"

for command_name in curl find grep mktemp tar uname; do
    command -v "$command_name" >/dev/null 2>&1 || die "远程安装缺少命令:$command_name"
done

detect_arch() {
    case "$(uname -m)" in
        x86_64 | amd64) echo "x86_64" ;;
        aarch64 | arm64) echo "aarch64" ;;
        *) die "不支持的架构:$(uname -m)（仅支持 x86_64/aarch64）" ;;
    esac
}

validate_archive_paths() {
    local archive="$1"
    local expected_root="$2"
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
        [[ "$entry" == "$expected_root" || "$entry" == "$expected_root/"* ]] \
            || die "发布包根目录必须是 $expected_root:$entry"
        IFS='/' read -r -a components <<<"$entry"
        for component in "${components[@]}"; do
            [[ "$component" != ".." ]] || die "压缩包包含路径穿越:$entry"
        done
    done < <(tar -tzf "$archive")
}

arch="$(detect_arch)"
package_name="sumpter-linux-${arch}"
archive_url="$DOWNLOAD_BASE/${package_name}.tar.gz"
WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/sumpter-bootstrap.XXXXXX")"
archive="$WORK_DIR/${package_name}.tar.gz"

note "下载:$archive_url"
note "按配置跳过 SHA-256 校验"
curl --fail --location --silent --show-error \
    --proto '=https' --proto-redir '=https' --tlsv1.2 --retry 3 --connect-timeout 15 --max-time 600 \
    --output "$archive" "$archive_url"
validate_archive_paths "$archive" "$package_name"

mkdir -p "$WORK_DIR/extracted"
tar --no-same-owner --no-same-permissions -xzf "$archive" -C "$WORK_DIR/extracted"
if find "$WORK_DIR/extracted" -type l -print -quit | grep -q .; then
    die "发布包包含符号链接，拒绝安装"
fi

package_root="$WORK_DIR/extracted/$package_name"
[[ -d "$package_root" && ! -L "$package_root" ]] \
    || die "发布包缺少顶层目录:$package_name"
[[ -f "$package_root/scripts/install.sh" && ! -L "$package_root/scripts/install.sh" ]] \
    || die "发布包缺少包内安装器:scripts/install.sh"
[[ -f "$package_root/scripts/cc-project-attribution.sh" && ! -L "$package_root/scripts/cc-project-attribution.sh" ]] \
    || die "发布包缺少 Claude Code 项目归因配置器:scripts/cc-project-attribution.sh"
[[ -f "$package_root/scripts/grok-project-attribution.sh" && ! -L "$package_root/scripts/grok-project-attribution.sh" ]] \
    || die "发布包缺少 Grok Build 项目归因配置器:scripts/grok-project-attribution.sh"

# 首次安装时，包内安装器会先探测旧 unit 的状态。Fedora/systemd 对不存在的
# unit 会把这条正常探测结果写到 stderr；只过滤这一条固定文案，保留安装器的
# 其余诊断和原始退出码。
filter_installer_stderr() {
    sed '/^Failed to get unit file state for sumpter\.service: No such file or directory$/d' >&2
}

display_release_changelog() {
    local changelog="$package_root/CHANGELOG.md"
    local line=""

    if [[ ! -f "$changelog" || -L "$changelog" ]]; then
        note "当前发布包未包含 changelog（旧版静态镜像包可能出现此提示）"
        return 0
    fi

    echo
    note "安装成功，当前版本 changelog:"
    echo "============================================================"
    while IFS= read -r line || [[ -n "$line" ]]; do
        printf '%s\n' "$line"
    done <"$changelog"
    echo "============================================================"
}

if ((${#INSTALL_ARGS[@]} > 0)); then
    note "执行发布包内安装器（透传: ${INSTALL_ARGS[*]}）"
    bash "$package_root/scripts/install.sh" "${INSTALL_ARGS[@]}" \
        2> >(filter_installer_stderr)
else
    note "执行发布包内安装器"
    bash "$package_root/scripts/install.sh" \
        2> >(filter_installer_stderr)
fi

display_release_changelog
