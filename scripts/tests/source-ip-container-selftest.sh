#!/usr/bin/env bash
# 真实容器验证：事件 sourceIP 在 Docker bridge 部署下的三种来源路径。
#   A. 同一容器网络直连（地址保留）      → 记录客户端容器 IP，伪造头被忽略
#   B. 经可信 Nginx 反代（传递 X-Forwarded-For） → 记录客户端容器 IP，客户端自带的伪造头被代理覆盖
#   C. 宿主机访问发布端口（Docker NAT）    → 记录网关地址，不伪造真实 IP
#   D. 转发头不改变 /__status 的环回限制
#   E. 重启容器后历史事件仍带解析结果
# 只在 Linux CI 运行，需要 docker compose v2、curl、jq 与已构建的镜像（SUMPTER_TEST_IMAGE）。
set -Eeuo pipefail

IMAGE="${SUMPTER_TEST_IMAGE:?需要 SUMPTER_TEST_IMAGE}"
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PROXY_EXAMPLE="$REPO_ROOT/platforms/linux/deploy/nginx-sumpter-proxy.conf.example"
NGINX_IMAGE="${SUMPTER_TEST_NGINX_IMAGE:-nginx:1.27-alpine}"
CLIENT_IMAGE="${SUMPTER_TEST_CLIENT_IMAGE:-curlimages/curl:8.11.1}"
SUBNET="172.29.0.0/24"
GATEWAY="172.29.0.1"
NGINX_IP="172.29.0.10"
CLIENT_IP="172.29.0.20"
SPOOF="198.51.100.99"
ADMIN_USER="kkl"
PROXY_PORT="${SUMPTER_TEST_PROXY_PORT:-$((RANDOM % 1000 + 47000))}"
ADMIN_PORT="${SUMPTER_TEST_ADMIN_PORT:-$((PROXY_PORT + 1))}"
ADMIN_PASSWORD="synthetic-source-ip-selftest-password"

for tool in docker curl jq; do
    command -v "$tool" >/dev/null 2>&1 || { echo "[source-ip] 需要 $tool" >&2; exit 1; }
done
docker compose version >/dev/null 2>&1 || { echo "[source-ip] 需要 docker compose v2" >&2; exit 1; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/sumpter-source-ip.XXXXXX")"
PROJECT="sumpter-source-ip-$$"
FAILED=0
note() { echo "[source-ip] $*"; }
fail() { echo "[source-ip] 失败：$*" >&2; FAILED=1; }
# 运行镜像以 root + cap_drop ALL 启动，没有 CAP_DAC_OVERRIDE。
# 与 DOCKER.md「自备 config.json 与初始密码」一样，文件必须属于容器内 root，
# 否则宿主机用户的 0600 凭据会 Permission denied。
own_config() {
    if [[ "$(id -u)" -eq 0 ]]; then
        chown -R root:root "$WORK/config"
    else
        sudo -n chown -R root:root "$WORK/config"
    fi
}
remove_work() {
    if [[ "$(id -u)" -eq 0 ]]; then
        rm -rf "$WORK"
    elif sudo -n rm -rf "$WORK" 2>/dev/null; then
        :
    else
        rm -rf "$WORK"
    fi
}
cleanup() {
    docker compose -p "$PROJECT" -f "$WORK/compose.yaml" logs --no-color sumpter nginx 2>/dev/null | tail -n 80 || true
    docker compose -p "$PROJECT" -f "$WORK/compose.yaml" down -v --remove-orphans >/dev/null 2>&1 || true
    remove_work
}
trap cleanup EXIT

mkdir -p "$WORK/config" "$WORK/nginx"
chmod 700 "$WORK/config"
# 可信代理只填 Nginx 的固定地址：不信任整个网桥，才能证明直连伪造头被忽略。
cat >"$WORK/config/config.json" <<EOF
{"schemaVersion":7,"endpoints":[],"listener":{"host":"0.0.0.0","port":57878,"authToken":"","allowedCIDRs":[],"trustedProxyCIDRs":["$NGINX_IP"]}}
EOF
printf '%s\n' "$ADMIN_PASSWORD" >"$WORK/config/admin-password"
chmod 600 "$WORK/config/config.json" "$WORK/config/admin-password"
own_config

# Nginx 配置直接来自仓库示例：map 留在 http 级，location 包进 server；后端改为服务名。
awk '
    /^location \/ \{/ && !wrapped { print "server {"; print "    listen 80;"; wrapped = 1 }
    { print }
    END { if (wrapped) print "}" }
' "$PROXY_EXAMPLE" | sed 's#proxy_pass http://127.0.0.1:57878#proxy_pass http://sumpter:57878#' >"$WORK/nginx/sumpter.conf"
# shellcheck disable=SC2016 # 字面量 $remote_addr 正是要匹配的内容
grep -Fq 'proxy_set_header X-Forwarded-For $remote_addr' "$WORK/nginx/sumpter.conf" \
    || { echo "[source-ip] 反代示例必须用 \$remote_addr 覆盖 X-Forwarded-For" >&2; exit 1; }

cat >"$WORK/compose.yaml" <<EOF
networks:
  edge:
    driver: bridge
    ipam:
      config:
        - subnet: $SUBNET
          gateway: $GATEWAY
services:
  sumpter:
    image: $IMAGE
    read_only: true
    cap_drop: [ALL]
    security_opt: ["no-new-privileges:true"]
    init: true
    environment:
      RUST_LOG: info
      SUMPTER_ADMIN_HOST: "0.0.0.0"
      SUMPTER_ADMIN_PORT: "57879"
      SUMPTER_ADMIN_PASSWORD_FILE: /config/admin-password
    volumes:
      - ./config:/config
    tmpfs:
      - /tmp:rw,noexec,nosuid,nodev,size=64m,mode=1777
    ports:
      - "127.0.0.1:$PROXY_PORT:57878"
      - "127.0.0.1:$ADMIN_PORT:57879"
    networks: [edge]
  nginx:
    image: $NGINX_IMAGE
    depends_on: [sumpter]
    volumes:
      - ./nginx/sumpter.conf:/etc/nginx/conf.d/default.conf:ro
    networks:
      edge:
        ipv4_address: $NGINX_IP
  client:
    image: $CLIENT_IMAGE
    entrypoint: ["/bin/sh", "-c", "sleep 3600"]
    networks:
      edge:
        ipv4_address: $CLIENT_IP
EOF

note "启动 Compose 项目 $PROJECT"
docker compose -p "$PROJECT" -f "$WORK/compose.yaml" up -d --wait --wait-timeout 120
ADMIN_BASE="http://127.0.0.1:$ADMIN_PORT"
note "宿主机端口：proxy=$PROXY_PORT admin=$ADMIN_PORT"

# 客户端容器发起请求；每个场景用独立会话 ID 标记，便于按事件回查。
client_request() {
    local url="$1" session="$2"; shift 2
    docker compose -p "$PROJECT" -f "$WORK/compose.yaml" exec -T client \
        curl -sS -o /dev/null -w '%{http_code}' --max-time 10 \
        -H 'Content-Type: application/json' \
        -H "x-claude-code-session-id: $session" \
        -H "X-Forwarded-For: $SPOOF" -H "X-Real-IP: $SPOOF" \
        "$@" -X POST --data '{"model":"claude-opus-5","messages":[{"role":"user","content":"hi"}]}' "$url"
}

COOKIES="$WORK/admin.cookies"
admin_login() {
    rm -f "$COOKIES"
    local code
    code="$(curl --noproxy '*' -sS -o "$WORK/login.json" -w '%{http_code}' --max-time 10 -c "$COOKIES" \
        -H 'Content-Type: application/json' -X POST \
        -d "$(jq -cn --arg u "$ADMIN_USER" --arg p "$ADMIN_PASSWORD" '{username:$u,password:$p}')" \
        "$ADMIN_BASE/admin/api/auth/login")"
    [[ "$code" == "200" ]] || { fail "Admin 登录返回 $code"; return 1; }
}
# 事件写入经有界后台队列，回查时轮询直到出现。
event_source_ip() {
    local session="$1" ip
    for _ in $(seq 1 50); do
        ip="$(curl --noproxy '*' -sS --max-time 10 -b "$COOKIES" "$ADMIN_BASE/admin/api/runtime/events?limit=100" \
            | jq -r --arg s "$session" '[.events[] | select(.kind=="client" and .sessionID==$s) | .sourceIP // "null"] | first // empty')"
        [[ -n "$ip" ]] && { printf '%s' "$ip"; return 0; }
        sleep 0.2
    done
    printf 'missing'
}
expect_ip() {
    local session="$1" expected="$2" label="$3" actual
    actual="$(event_source_ip "$session")"
    if [[ "$actual" == "$expected" ]]; then
        note "✓ $label：sourceIP=$actual"
    else
        fail "$label：期望 sourceIP=$expected，实际 $actual"
    fi
}

admin_login

note "A. 容器网络直连（非可信对端，伪造头应被忽略）"
code="$(client_request "http://sumpter:57878/v1/messages" "direct-spoofed")"
[[ "$code" =~ ^4 ]] || fail "直连请求应被拒绝为 4xx（无入口），实际 $code"
expect_ip "direct-spoofed" "$CLIENT_IP" "直连保留客户端容器地址"

note "B. 经可信 Nginx 反代（代理覆盖客户端伪造头）"
code="$(client_request "http://nginx/v1/messages" "via-nginx")"
[[ "$code" =~ ^4 ]] || fail "反代请求应透传为 4xx，实际 $code"
expect_ip "via-nginx" "$CLIENT_IP" "反代传递真实客户端地址"

note "C. 宿主机访问发布端口（NAT 抹去源地址，记录网关而非伪造值）"
code="$(curl --noproxy '*' -sS -o /dev/null -w '%{http_code}' --max-time 10 \
    -H 'Content-Type: application/json' -H 'x-claude-code-session-id: host-nat' \
    -H "X-Forwarded-For: $SPOOF" -X POST --data '{"model":"claude-opus-5","messages":[]}' \
    "http://127.0.0.1:$PROXY_PORT/v1/messages")"
[[ "$code" =~ ^4 ]] || fail "宿主机请求应为 4xx，实际 $code"
nat_ip="$(event_source_ip host-nat)"
if [[ "$nat_ip" == "$GATEWAY" ]]; then
    note "✓ NAT 回退到网关地址：sourceIP=$nat_ip"
elif [[ "$nat_ip" != "$SPOOF" && "$nat_ip" != "missing" && "$nat_ip" != "null" ]]; then
    # 部分 runner 的 docker-proxy/NAT 组合会呈现其它网桥地址；只要不是伪造值即可。
    note "✓ NAT 回退到对端地址（非网关 $GATEWAY）：sourceIP=$nat_ip"
else
    fail "NAT 场景 sourceIP=$nat_ip，不应等于伪造值或缺失"
fi

note "D. 转发头不改变 /__status 的环回限制"
for target in "http://sumpter:57878/__status" "http://nginx/__status"; do
    code="$(docker compose -p "$PROJECT" -f "$WORK/compose.yaml" exec -T client \
        curl -sS -o /dev/null -w '%{http_code}' --max-time 10 -H 'X-Forwarded-For: 127.0.0.1' "$target")"
    if [[ "$code" == "403" ]]; then
        note "✓ 伪造环回地址访问 $target 仍 403"
    else
        fail "$target 应 403，实际 $code"
    fi
done

note "E. 重启容器后历史事件保留解析结果"
docker compose -p "$PROJECT" -f "$WORK/compose.yaml" restart sumpter
docker compose -p "$PROJECT" -f "$WORK/compose.yaml" up -d --wait --wait-timeout 120 sumpter
admin_login
expect_ip "via-nginx" "$CLIENT_IP" "重启后反代事件仍为客户端地址"
expect_ip "direct-spoofed" "$CLIENT_IP" "重启后直连事件仍为客户端地址"

if [[ "$FAILED" -ne 0 ]]; then
    echo "[source-ip] 存在失败项" >&2
    exit 1
fi
note "全部通过"
