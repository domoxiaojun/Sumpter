# Linux Admin API

本文描述 Linux WebUI 与 `sumpterd` 的当前管理接口。地址默认 `http://127.0.0.1:57879`，API 前缀为 `/admin/api/`。代理数据面是另一套 listener，默认 57878。

## 鉴权

`GET /healthz` 不需要登录，成功返回 204 空响应，只代表 Admin listener 存活。`/admin/` 静态页面可以加载；除登录会话接口外，API 和 SSE 需要 HttpOnly 会话 Cookie。

```text
POST /admin/api/auth/login       登录，JSON {"username":"...","password":"..."}
GET  /admin/api/auth/session     当前会话与 CSRF 值
POST /admin/api/auth/logout      退出
```

登录成功后，写请求 `POST` / `PUT` / `PATCH` / `DELETE` 必须发送 `Content-Type: application/json` 和当前会话返回的 `X-Sumpter-CSRF`。Cookie 为 `Path=/admin`、HttpOnly、SameSite=Strict；HTTPS 反代正确传递 `X-Forwarded-Proto` 后会带 Secure。

错误统一为：

```json
{"error":"error_code","message":"可读说明"}
```

未登录返回 401，CSRF 缺失或错误返回 403，JSON 缺失返回 415，参数或配置无效返回 400。Admin 不提供浏览器 Basic Auth challenge。

## 服务和配置

```text
GET  /admin/api/status                 服务状态、版本、监听摘要
GET  /admin/api/config                 脱敏配置与 generation
PUT  /admin/api/config                 generation-safe 保存配置
POST /admin/api/reload                 从磁盘重载 config.json
POST /admin/api/provider-models        从入口获取模型目录
GET  /admin/api/endpoint-secret        读取当前会话允许的入口密钥摘要
GET  /admin/api/diagnostics            诊断摘要
GET  /admin/api/autostart               systemd 自启动状态
PUT  /admin/api/autostart               设置 user scope 自启动
PUT  /admin/api/auth/credentials        修改 Admin 用户名 / 密码
```

保存配置的请求结构为 `expectedGeneration`、`config` 和可选 `secretUpdates`。`config.schemaVersion` 必须为 7；服务器校验入口、模型组、featureRules 和监听字段后才写盘。generation 不匹配时拒绝覆盖，请重新读取再保存。响应不会回传完整 API Key。

`POST /reload` 请求体即使为空也发送 `{}`。重载只影响 `config.json`；Admin 监听、密码文件和 systemd drop-in 变化需要重启进程。

## 运行事件和统计

```text
GET    /admin/api/events                         SSE runtime-change / stats-reset
GET    /admin/api/runtime/summary                当前快照
GET    /admin/api/runtime/events                 keyset 分页（beforeSeq / afterChangeSeq）
GET    /admin/api/runtime/events/:id             单条事件与上游尝试
GET    /admin/api/runtime/request-chain          请求链
GET    /admin/api/runtime/analytics              range、客户端、项目、会话和模型筛选
GET    /admin/api/runtime/facets                 可用筛选值
GET    /admin/api/runtime/trends                 趋势
GET    /admin/api/runtime/errors                 错误聚合
GET    /admin/api/runtime/dimensions             模型等维度分页
GET    /admin/api/runtime/projects               项目聚合
GET    /admin/api/runtime/sessions               会话聚合
GET    /admin/api/runtime/storage                SQLite 状态
GET    /admin/api/runtime/retention              保留策略
PUT    /admin/api/runtime/retention              设置 maxAgeDays / storageLimitBytes
GET    /admin/api/runtime/pricing                成本估算单价
PUT    /admin/api/runtime/pricing                设置成本估算单价
POST   /admin/api/runtime/cleanup/preview        预览按时间清理
POST   /admin/api/runtime/cleanup                执行按时间清理
POST   /admin/api/runtime/reset                  清空运行统计
POST   /admin/api/runtime/recreate               删除并按当前 schema 重建数据库
DELETE /admin/api/runtime/session?sessionID=...  删除完整会话及关联尝试
GET    /admin/api/runtime/session/export?...     导出脱敏会话 JSON
POST   /admin/api/runtime/projects/sticky-clear  清除项目粘性
POST   /admin/api/runtime/sessions/sticky-clear  清除会话粘性
GET    /admin/api/runtime/export                 导出筛选后的统计
GET    /admin/api/runtime/export/estimate        估算导出大小
```

带时间范围的查询接受 `today`、`1h`、`24h`、`7d`、`30d`、`all`，也可提供 `from` / `to`。筛选条件按 AND 组合。事件正文、Authorization、Cookie、API Key 和完整绝对路径不属于普通统计响应；项目和 workspace 使用清洗后的投影。

`GET /runtime/events?view=page` 使用 `page` / `pageSize` 返回已完成事件页，进行中事件由实时流单独展示。首次查询不传快照 token；后续页及关联趋势、维度、错误、导出查询同时回传响应中的 `snapshotSeq`、`snapshotChangeSeq`、`historyGeneration`。`snapshotChangeSeq` 固定首次完成水位，防止并发请求后来完成使分页重漏；旧 token 缺少该字段时返回快照失效，客户端应重新获取首页而非降级到旧游标接口。

`runtime.sqlite3` 是唯一运行统计存储。`maxAgeDays` 与 `storageLimitBytes` 任一达到即轮换已完成请求组，进行中的请求组受保护；两者都为 null 时不自动删除。`reset`、会话删除和 `recreate` 的删除范围不同，调用方必须向用户明确说明。

## 诊断捕获

```text
GET    /admin/api/diagnostic-capture
GET    /admin/api/diagnostic-capture/{requestID}
GET    /admin/api/diagnostic-capture/export
PUT    /admin/api/diagnostic-capture       {"enabled":true,"maxBytes":...}
DELETE /admin/api/diagnostic-capture
```

索引接口只返回请求摘要；详情和原始导出可能包含未脱敏 Header、Body、上游响应和 Chunk。捕获默认关闭，快照保存在配置目录并使用 0600。导出使用 `privacy=redacted` 时，current/selected/all 范围都会清除已识别的凭据字段与 URL/path 查询凭据，包括 Gemini `key`。这是规则脱敏，不保证识别任意正文中的机密，分享前仍须检查；`privacy=raw` 必须显式确认。

## 平台约束

Admin 默认绑定 loopback。远程访问使用 HTTPS 反向代理、VPN 或 SSH 隧道；反代要保留 Cookie、CSRF、SSE 和 WebSocket Upgrade。Admin API 不能启停任意进程或执行任意命令；自启动只允许固定的 `sumpter.service`，system scope 由管理员使用 sudo 操作。

代理 listener 上的旧 `/__runtime`、`/__reset-stats`、`/__notify` 和旧 `/admin/api/main/*`、`/pools/*`、`/providers/*` 不属于当前接口。旧 `keys.json` 也不是 Admin API 的配置输入。
