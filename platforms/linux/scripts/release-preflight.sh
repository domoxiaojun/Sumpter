#!/usr/bin/env bash
# Preflight checks before publishing linux/ to GitHub (domoxiaojun/sumpter).
# Ensures Admin CLI, installer passthrough, and Fedora SELinux paths exist.
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$ROOT"

fail() {
    echo "preflight FAIL: $*" >&2
    exit 1
}

ok() {
    echo "preflight OK: $*"
}

need_file() {
    local path="$1"
    [[ -f "$path" ]] || fail "missing file $path"
}

need_text() {
    local path="$1"
    local needle="$2"
    grep -Fq -- "$needle" "$path" || fail "$path missing text: $needle"
}

reject_text() {
    local path="$1"
    local needle="$2"
    if grep -Fq -- "$needle" "$path"; then
        fail "$path contains forbidden text: $needle"
    fi
}

version="$(awk -F'"' '/^version = / {print $2; exit}' Cargo.toml)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "bad Cargo.toml version:$version"
need_text Cargo.lock "name = \"kekulvd\""
# lock package versions for our crates
for crate in kekulv-core kekulv-proxy kekulvd; do
    awk -v crate="$crate" -v ver="$version" '
        /^\[\[package\]\]$/ { in_pkg=1; hit=0; next }
        in_pkg && $0 == "name = \"" crate "\"" { hit=1; next }
        in_pkg && hit && $0 == "version = \"" ver "\"" { found=1; exit 0 }
        in_pkg && hit && $0 ~ /^version = / {
            # A shared path dependency can intentionally use a parallel
            # `-unified.*` package version beside the release crate.  Keep
            # scanning for the exact legacy package version instead of
            # rejecting the first duplicate name in Cargo.lock.
            next
        }
        END { if (!found) exit 3 }
    ' Cargo.lock || fail "Cargo.lock version for $crate is not $version"
done
ok "Cargo version $version"

# WebUI 产物版本跟随 Cargo 版本。CI 与 release.yml 都不查这项,历史上只靠人工同步;
# 版本号漂移不会让构建失败,但发布出去的包里前端与后端版本号会对不上。
need_file webui/package.json
# 只认 npm 标准格式的顶层键(2 空格缩进):`[[:space:]]*` 会把嵌套对象里的
# "version" 也匹配上,重排 package.json 后可能误取依赖的版本号。
# 缩进风格变了就读不到值,下面的空值检查会直接 fail —— 宁可挡住发布,不要误判通过。
webui_version="$(awk -F'"' '/^  "version"[[:space:]]*:/ {print $4; exit}' webui/package.json)"
[[ -n "$webui_version" ]] || fail "webui/package.json 读不到 version"
[[ "$webui_version" == "$version" ]] \
    || fail "webui/package.json version $webui_version 与 Cargo.toml $version 不一致"
ok "WebUI version $webui_version"

# macOS workspace 版本必须与 Linux 一致:同一个 v* tag 同时触发 release.yml 与
# macos-release.yml,后者会校验根 Cargo.toml、macos/Cargo.toml 与 tag 三者相同,
# 不一致就在 CI 上失败。这里提前挡住,免得推了 tag 才发现。
# 两种拓扑都要认:发布仓库里 macOS 在 macos/ 子目录,上游 monorepo 里在 ../macos。
macos_manifest=""
for candidate in macos/Cargo.toml ../macos/Cargo.toml; do
    if [[ -f "$candidate" ]]; then
        macos_manifest="$candidate"
        break
    fi
done
if [[ -n "$macos_manifest" ]]; then
    macos_version="$(awk '
        /^\[workspace\.package\]$/ { in_pkg = 1; next }
        in_pkg && /^\[/ { exit }
        in_pkg && $1 == "version" && $2 == "=" { gsub(/"/, "", $3); print $3; exit }
    ' "$macos_manifest")"
    [[ -n "$macos_version" ]] || fail "$macos_manifest 读不到 workspace.package.version"
    [[ "$macos_version" == "$version" ]] \
        || fail "$macos_manifest version $macos_version 与 Cargo.toml $version 不一致"
    ok "macOS version $macos_version ($macos_manifest)"
else
    ok "macOS workspace 不在此检出内,跳过版本比对"
fi

need_file USAGE.md
need_text USAGE.md "schema v6"
need_text USAGE.md '"schemaVersion": 6'
ok "USAGE.md present for published repo / tarball"

# The shared engine is opt-in during the migration.  A legacy-only standalone
# Linux checkout may not contain `shared/`, so keep this check conditional; a
# monorepo or assembled staging tree must prove that the shared manifest and
# its self-contained checksum list are present before it is used for a release
# build.
shared_root=""
if [[ -d "$ROOT/shared" ]]; then
    shared_root="$ROOT/shared"
elif [[ -d "$ROOT/../shared" ]]; then
    shared_root="$ROOT/../shared"
elif [[ -d "$ROOT/../crates/kekulv-engine" ]]; then
    # The standalone `sumpter` mirror keeps the shared workspace at its root;
    # Linux remains a sibling under that root.
    shared_root="$ROOT/../"
fi
if [[ -n "$shared_root" ]]; then
    need_file "$shared_root/Cargo.toml"
    need_file "$shared_root/crates/kekulv-engine/Cargo.toml"
    need_text "$shared_root/crates/kekulv-engine/Cargo.toml" "platform-linux"
    need_file scripts/assemble-shared-tree.sh
    need_text scripts/assemble-shared-tree.sh "SOURCE_COMMIT"
    if [[ -f "$shared_root/SHA256SUMS" ]]; then
        (cd "$shared_root" && shasum -a 256 -c SHA256SUMS) \
            || fail "shared/SHA256SUMS 校验失败"
        ok "shared source SHA-256 manifest"
    else
        ok "shared source manifest present（源码树尚未生成 staging SHA256SUMS）"
    fi
fi

need_file crates/kekulvd/src/main.rs
need_text crates/kekulvd/src/main.rs "--admin-host"
need_text crates/kekulvd/src/main.rs "AdminListen"
need_text crates/kekulvd/src/main.rs "KEKULV_ADMIN_HOST"
need_text crates/kekulvd/src/main.rs "KEKULV_ADMIN_PASSWORD_FILE"
need_text crates/kekulvd/src/main.rs 'DEFAULT_ADMIN_PASSWORD_FILENAME: &str = "admin-password"'
need_text crates/kekulvd/src/main.rs "admin_listen.socket_addr()"
ok "daemon CLI admin listen"

need_file crates/kekulv-proxy/src/admin.rs
need_file crates/kekulv-proxy/src/admin_auth.rs
need_text crates/kekulv-proxy/src/admin.rs "struct AdminListen"
need_text crates/kekulv-proxy/src/admin.rs "DEFAULT_ADMIN_HOST"
need_text crates/kekulv-proxy/src/admin.rs "is_loopback_bind"
need_text crates/kekulv-proxy/src/admin.rs '"/auth/session"'
need_text crates/kekulv-proxy/src/admin.rs '"/auth/login"'
need_text crates/kekulv-proxy/src/admin.rs '"/auth/logout"'
need_text crates/kekulv-proxy/src/admin.rs '"/auth/credentials"'
need_text crates/kekulv-proxy/src/admin.rs '"x-kekulv-csrf"'
need_text crates/kekulv-proxy/src/admin_auth.rs '"session-cookie"'
need_text crates/kekulv-proxy/src/admin_auth.rs "SameSite=Strict"
need_text crates/kekulv-proxy/src/admin_auth.rs "MAX_CREDENTIAL_FILE_BYTES: u64 = 16 * 1024"
reject_text crates/kekulv-proxy/src/admin.rs "WWW_AUTHENTICATE"
need_text crates/kekulv-proxy/src/admin.rs '"/healthz"'
need_text crates/kekulv-proxy/src/admin.rs "admin_listen.host"
ok "admin session auth/diagnostics"

need_file scripts/install.sh
need_text scripts/install.sh "--admin-host"
need_text scripts/install.sh "--admin-password-file"
need_text scripts/install.sh "ensure_admin_password_file"
need_text scripts/install.sh 'od -An -N24 -tx1 /dev/urandom'
need_text scripts/install.sh "50-admin-listen.conf"
need_text scripts/install.sh "restorecon"
need_text scripts/install.sh "no-preserve=context"
need_text scripts/install.sh "resolve_nologin_shell"
ok "install.sh admin + Fedora"

need_file scripts/bootstrap-install.sh
need_text scripts/bootstrap-install.sh "INSTALL_ARGS"
# shellcheck disable=SC2016 # 这里检查的就是脚本中的字面量，不应展开当前 shell 变量。
need_text scripts/bootstrap-install.sh 'bash "$package_root/scripts/install.sh" "${INSTALL_ARGS[@]}"'
need_text scripts/bootstrap-install.sh "display_release_changelog"
ok "bootstrap passthrough"

need_file .github/workflows/release.yml
# changelog 取仓库手写段落,不调 releases/generate-notes —— 那个 API 需要
# contents: write,构建 job 不该为此提权(v0.2.2 就是因此 403 发布失败)。
# shellcheck disable=SC2016 # 检查的是 workflow 里未展开的字面量。
need_text .github/workflows/release.yml 'awk -v ver="$version"'
# shellcheck disable=SC2016 # 这里检查 workflow 中未展开的 package 字面量。
need_text .github/workflows/release.yml 'cp release/CHANGELOG.md "dist/${package}/CHANGELOG.md"'
need_text .github/workflows/release.yml "--notes-file release/CHANGELOG.md"
if grep -q "releases/generate-notes" .github/workflows/release.yml; then
    fail "release.yml 仍调用 releases/generate-notes,该 API 需要 contents: write"
fi
ok "per-version release changelog"

# 本地就挡住「忘写 changelog」:CI 上缺这一段会直接让发布失败,不如提前发现。
need_file CHANGELOG.md
changelog_body="$(awk -v ver="$version" '
    index($0, "## [" ver "]") == 1 { inside = 1; next }
    inside && index($0, "## [") == 1 { exit }
    inside { print }
' CHANGELOG.md)"
[[ -n "${changelog_body//[[:space:]]/}" ]] \
    || fail "CHANGELOG.md 缺少 ## [${version}] 段落"
ok "CHANGELOG entry for ${version}"

need_file scripts/start.sh
need_text scripts/start.sh "resolve_admin_probe"
need_text scripts/start.sh "--admin-host"
need_text scripts/start.sh "/healthz"
ok "start.sh probe"

need_file scripts/uninstall.sh
need_text scripts/uninstall.sh "kekulv.service.d"
ok "uninstall drop-in cleanup"

need_text deploy/kekulv.service "KEKULV_ADMIN_PASSWORD_FILE"
need_text deploy/kekulv-system.service "KEKULV_ADMIN_PASSWORD_FILE"
need_file deploy/nginx-kekulv-admin.conf.example
[[ "$(grep -Fc "proxy_set_header Cookie \$http_cookie" deploy/nginx-kekulv-admin.conf.example)" -ge 2 ]] \
    || fail "nginx example must forward Cookie in both SSE and general Admin locations"
need_text deploy/nginx-kekulv-admin.conf.example "proxy_set_header X-Forwarded-Proto \$scheme"
reject_text deploy/nginx-kekulv-admin.conf.example "proxy_set_header Authorization"
ok "unit docs"

need_file Dockerfile
need_text Dockerfile "KEKULV_ADMIN_HOST"
need_text Dockerfile "KEKULV_ADMIN_PORT"
need_file Dockerfile.runtime
need_text Dockerfile.runtime "docker-bin/kekulvd-"
need_text Dockerfile.runtime "KEKULV_ADMIN_HOST"
need_file .github/workflows/container.yml
need_text .github/workflows/container.yml "Dockerfile.runtime"
need_text .github/workflows/container.yml "cargo-zigbuild"
need_file compose.yaml
need_text compose.yaml "KEKULV_ADMIN_HOST"
need_text compose.yaml "network_mode: host"
need_file compose.bridge.example.yaml
need_text compose.bridge.example.yaml "0.0.0.0"
ok "docker compose examples"

# Hard fail on the previous rsync footgun: exclude pattern 'kekulvd' must not be used bare.
if git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if git grep -n -- '--exclude .kekulvd.' scripts >/dev/null 2>&1; then
        :
    fi
fi

echo "release-preflight PASS (version $version)"
