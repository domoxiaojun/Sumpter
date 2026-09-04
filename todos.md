# Codex 通知状态：已信任仍显示等待（2026-09-04）

Sumpter 用「收到一次 SSE」当 /hooks 已信任的证据，启动时 rewrite 还会
`clearVerified()`。Codex 桌面开关已经打开时仍显示「等待 /hooks 信任」。

- [x] 已写入即「已配置」；启动/重新安装不清验证
- [x] 测试、提交

---

# Codex hook 显示名（2026-09-04）

Codex 用户配置里 Sumpter 条目显示成「钩子 1」，因为 hooks.json 只有
`statusMessage`，没有 `name`。补上显示名「Sumpter 通知」。

- [x] CodexHookEditing 写入 name；校验允许该字段
- [x] 测试、提交。重新安装/启动后才会改已有 hooks.json

---

# 通知页：三客户端安装 / 移除配置（2026-09-04）

总开关开着时 Grok 仍显示「未配置」，接入状态也没有安装按钮。
给 Claude Code、Codex CLI、Grok Build 各加「安装配置」「移除配置」。
启动时若 Claude/Codex 已开且用户没手动移除过 Grok，则补装 Grok hook。

- [x] AppModel：`setGrokNotifications`；三路独立安装/移除；升级补装
- [x] NotificationsPane：每行安装/移除 + 确认；Codex 提示跟在 Codex 下面
- [x] 测试、提交。不宣称已安装 App 已更新

---

# Grok Build 系统通知（2026-09-04）

macOS 通知已接 Claude Code / Codex CLI，Grok Build 缺 hook → `/__notify`。
用 `~/.grok/hooks/sumpter-notify.json`（不改 grok-build、不写 config.toml）。
Linux `/__notify` 仍 404。不替换已安装 App。

- [x] macOS `/__notify` 识别 `grok_build`：事件白名单、固定文案、camelCase、session-end Stop 过滤
- [x] adapter engine 测试
- [x] `GrokNotifyScript` + `GrokNotificationHooks`；Claude 脚本遇 `GROK_*` 改标
- [x] AppModel 总开关三路；NotificationsPane；端口刷新
- [x] macOS README（USAGE 未提通知，只改 macos README + 根 README）
- [x] Rust + Swift 测试；提交

---

# 安全页能找到 Grok 归因脚本（2026-09-04）

脚本在 `platforms/macos/scripts/grok-project-attribution.sh`，但 App 只查 Bundle
资源和一层相对路径，开发运行/未重打包的 App 会显示「配置器不可用」。

- [x] 统一脚本定位：Bundle Resources + 向上找仓库 scripts/
- [x] Swift 包打进 grok/cc 脚本资源；CC 与 Grok 面板共用定位
- [x] 测试、提交。不宣称已安装 App 已更新

---

# Grok 运行页：补上游请求链 + 归因配置入口（2026-09-04）

截图：Grok 请求已打到 CPA、有上游请求 ID，但请求链写「0 次上游尝试」；项目仍是未识别。
原因一：运行页「客户端」筛选下没拉 `/request-chain`。原因二：安全页只有 CC 归因引导。

- [x] 运行页选中事件时加载完整请求链（含上游尝试）
- [x] 双端安全页补 Grok 归因配置（与 CC 并列）
- [x] 测试、todos/plan、提交。不宣称已安装 App 已更新

---

# Codex 事件来源也显示本地(kkl)（2026-09-04）

Codex 结构化 workspace 已经是 `workspace_local`，但没有 wrapper 的 `X-Sumpter-User`，
运行页就停在「本地项目」。从 `/Users/kkl`、`/home/kkl` 这类源路径取出用户名，
和 CC/Grok 一样显示 `本地(kkl)`。用户名仍只用于展示，不进项目 identity。

- [x] core：从工作区源路径解析本机用户名
- [x] runtime：投影 `localUser`；升 projection 让历史 Codex 行回填
- [x] 双端运行页：详情「项目来源」和列表摘要用 `本地(kkl)`
- [x] 测试、todos/plan、提交。不宣称已安装 App 已更新

---

# Grok 事件详情：采样客户端/会话字段（2026-09-04）

Grok Build 推理请求的身份在 `x-grok-*` header，不在 Codex `client_metadata`。
cwd/git 仍不进推理请求。事件详情要展示会话/对话/请求/客户端，不能再把
OTel `traceparent` 当成空 Codex「代理身份未确定」。

- [x] core：`GrokMetadata::from_headers`，挂到 `RuntimeEvent.grokMetadata`
- [x] engine：ClientMeta / 拒绝路径 / websocket 贯通；`x-grok-session-id` 进 sessionID
- [x] Grok 请求若只有 redacted OTel、没有 Codex 身份，不落空的 `codexMetadata`
- [x] 双端运行页详情展示 Grok 字段；列表摘要不再写空 Codex 代理
- [x] 测试、todos/plan、提交。不宣称已安装 App 已更新

---

# Grok / Claude Code 项目归因：本地(kkl)（2026-09-04）

CC 和 Grok 都用 wrapper 按启动目录分项目。有工作区路径时来源升格为 `workspace_local`，
运行页显示 `sumpter 本地(kkl)`。出站只认 `X-Sumpter-*`，不再保留 `x-kekulv-*`。

- [x] core：`X-Sumpter-User` → `ClientDeclaredMetadata.user`
- [x] runtime：声明了 workspace 则 `workspace_local`；列表投影带 `localUser`
- [x] engine：出站剥离 `x-sumpter-user`；去掉 `x-kekulv-*` 黑名单
- [x] `cc-project-attribution.sh` 补 User；新增 grok wrapper / 自测 / Linux 端点
- [x] 发布包与 DMG 带 grok 脚本
- [x] 双端 UI / USAGE / 引导
- [x] 测试、门禁、提交。不宣称已安装 App 已更新

---

# 修复运行页历史事件表只剩表头（2026-09-04）

截图：进行中两行正常，下面「请求 / 模型 / 路由 / 结果 / 说明」表头还在，行是空白。
SwiftUI `Table` 底层是 NSTableView，实时 SSE / 进行中 overlay 更新后经常只挂表头、行被裁掉。
改成与进行中请求相同的 SwiftUI 行，不再用原生 Table。

- [x] 历史事件列表改用自定义四列行 + 表头，去掉 `Table(visibleEvents)`
- [x] 窄窗仍走 compact 列表；保留分页高度和选中高亮
- [x] Swift 测试；更新 todos/plan；提交。不宣称已安装 App 已更新

---

# 修复 Codex Desktop 语音 400 invalid_architecture（2026-09-04）


运行页：`passthrough realtime` → CPA `ccc.domob.org`，`gpt-live-1-codex`，HTTP 400，
229 字节 `architecture="avas" is only supported for quicksilver Realtime WebRTC sessions.`
上一轮把 `intent=quicksilver&architecture=avas` 加在了 POST `/v1/realtime` 根上，
没有改到 `/v1/realtime/calls`。OpenAI/CPA 兼容面只允许 avas 出现在 WebRTC `/calls`。

- [x] POST `/v1/realtime` WebRTC bootstrap 出站改写到 `/v1/realtime/calls?intent=quicksilver&architecture=avas`
- [x] POST `/v1/live`、`/v1/realtime/calls` 保持原路径并补齐 avas
- [x] 标准 Realtime GET/WS 剥掉 `intent`/`architecture`，不向非 WebRTC 面泄漏 avas
- [x] 补 request_build / engine / linux websocket 回归；fmt/check/clippy/focused tests
- [x] 更新 USAGE / README / architecture / plan.md；提交。不宣称已安装 App 已更新

---

# 审查后续：意图 / 归因 / 分流全部收口（2026-09-04）


对照 CPA：POST `/v1/live`、`/v1/realtime`、`/v1/realtime/calls` 都是 Quicksilver；
只有无 `call_id` 的 GET `/v1/realtime` 才是标准 Realtime。Sumpter 按 mapping 选入口，
泄漏的聊天模型必须改写成 `gpt-live-1-codex`，不能按名字打到 xiao。

- [x] `gpt-4o`/`gpt-4o-mini` 不再从名字推断为 Live；通配 `*` 不能靠请求模型名冒充 video/live
- [x] POST `/v1/realtime` 与 `/calls` 对齐 CPA 为 Codex Live；GET 无 call_id 才是标准 Realtime
- [x] 标准 Realtime WS 去掉 `OpenAI-Alpha`；侧带 `?call_id=` 走 Live
- [x] OpenAI `/v1/models` 只保留 id/object/created/owned_by；跳过 `*`；Codex 不隐藏 gpt-4o
- [x] 目录按 UA 分流：client_version→Codex，grok-shell→Grok，claude-cli/Anthropic-Version→Anthropic
- [x] query 值做百分号解码；聊天/Responses 拒绝 image/video-only 模型
- [x] 补 routing/engine/websocket 回归；fmt/check/clippy/focused tests；提交

---

# 修复 Codex `/v1/models` 被透传到 xiao（2026-09-04）

对照 `/Users/kkl/.claude/automode-proxy`：原来 `GET /v1/models` 是未知路径 404，不会打上游。
Sumpter 后来按 CPA 资源面把 Models 原样透传到「第一个非 Anthropic 入口」，Codex Desktop
探测目录就会变成 `__sumpter_resource__` / `passthrough models` 打到 `xiao`/`anyrouter.top`。
对话本身（`gpt-5.6-sol` → CPA）没串台，但目录内容和运行页被改坏了。

- [x] 本地拦截 `GET /v1/models`（含 `/models`、`/openai/v1/models` 与 id 子路径），按 mapping 生成目录，不再 `plan_for_resource` 转发
- [x] 普通客户端返回 OpenAI `{object,data}`；带 `client_version` 时返回 Codex `{models:[{slug,...}]}`，并按版本过滤 reasoning levels
- [x] `?cursor=` 也走本地目录，不再留给上游分页
- [x] 回归：engine 形状、adapter 不打 FakeTransport、websocket 资源测试、Files capability 选路
- [x] 更新 USAGE / 双端 README / plan.md；fmt/check/focused tests；提交

---

# 硬切品牌名 kekulv → sumpter

旧安装不兼容，用户会卸载重装。不保留双读。

映射：
- 配置/数据目录 `kekulv` → `sumpter`
- 系统用户/单元 `kekulv` → `sumpter`
- 发布包二进制 `kekulvd` → `sumpterd`
- 环境变量 `KEKULV_*` / `KEKULVD_*` → `SUMPTER_*` / `SUMPTERD_*`
- Header `X-Kekulv-*` → `X-Sumpter-*`
- Cookie / CSRF / 导出格式 / sticky domain / localStorage 同步改名
- 不改 `docs/upstream/`、历史 CHANGELOG 条目、工作日志里的当时路径

- [x] 盘点并锁定映射
- [x] core / runtime / engine
- [x] Linux adapter、daemon、安装器、Docker
- [x] WebUI、macOS、归因脚本
- [x] 现行文档
- [x] 测试并提交

---

# 出站桥接补全与意图识别加固（审查后续）

审查对照 CLIProxyAPI 后确认：出站 Anthropic → OpenAI/Responses 的翻译只做纯文本，
工具调用被静默丢弃；`bridge.rs` 里那套 `validate_anthropic_request` /
`try_make_*` 从未接到数据面。按严重性分阶段修。

## P0 出站桥接补全（工具调用双向映射）

- [x] 请求侧 chat：`tools` / `tool_choice` 映射，`tool_use` → `assistant.tool_calls`，`tool_result` → `role:"tool"` 消息
- [x] 请求侧 Responses：`tools` flat 形状，`tool_use` → `function_call`，`tool_result` → `function_call_output`
- [x] 响应侧 chat：累积 `delta.tool_calls` → `tool_use` 块 + `input_json_delta`
- [x] 响应侧 Responses：`function_call` 输出项 → `tool_use` 块 + `input_json_delta`
- [x] 重设计校验器边界并接到数据面（现版本 `tools` 非空即拒，接线会让 CC 主对话全挂）

附带修掉一个既有 bug：两个桥的 `render_non_stream` 在 terminal 还是 `Pending`
时就把 `json_emitted` 置位，导致任何跨多个 SSE block 的非流式响应渲染成 0 字节。
- [x] 两个桥的非流式渲染只在 terminal 定型后置位 `json_emitted`

## P1

- [x] `has_tool_type` 把 `type:"custom"` 也算客户端工具（先落，独立小改动）
- [x] Translated 降级可见信号（配置期警告：某模型只能落到非 Anthropic 协议入口时提示）

## P2

- [x] `RequestPurpose` 注释与实现对齐（它参与 `server_retrieval` body 改写）
- [x] title 指纹加固：加 `output_config` json_schema 判据；`is_unmatched_no_tools` 改判「除 CC 身份外还有无专用指令」
- [x] ~~`plan_for_resource` 按 capability 过滤；`Live/Files` 接线~~ → 审查误判：`Live` 已接线（`engine/mod.rs:6745`），`plan_for_resource` 按 provider 面路由是有意设计（同 CPA）。只补注释说明 `Files` 属 wire 契约但不参与路由

## P3

- [x] WebFetch 注入的工具类型提为 `SERVER_WEB_SEARCH_TOOL_TYPE` 常量，注明为何无法从请求继承
- [x] 未知路径兜底：Raw 且 query 无 model → 404（不鉴权、不读 body），修复 2 个既有失败测试

---

# 待办：OpenAI 入站被误判为原生透传（会话前改动引入，本轮只定位未修）

`adapters/{linux,macos}/tests/engine.rs` 各有 7 个失败，两端同名同因。

## 根因

`engine/mod.rs:3374` 给 chat/Responses 入站（`dialect: Some(...)`）也设了
`passthrough: Some(body)`，于是：

- `passthrough_intent`（:3786）恒为真
- → 走 `plan_for_passthrough`（:3856）→ `route_mode` 恒 `Native`
- → `bridging` 为 false → content-type 走 `:5402` 的 event-stream 分支、
  出站路径不改写、route_mode 记成 Native

`:4148` 那段「Translated 时清 `client.passthrough`」是**死逻辑**：它排在 plan
之后，而 plan 的选择依赖 `passthrough_intent`，到那里 `route_mode` 已是 `Native`，
条件永不成立。

## 修法

dialect 入站有两种归宿：落到同协议上游可按字节透传，落到 Anthropic 上游必须走
翻译面。所以不能一看到 passthrough body 就认透传，要先按协议 gate 规划一次探
真实 `route_mode`，只有首选确实是 `Native` 时才透传：

```rust
let passthrough_intent = client_out.as_ref().is_some_and(|client| {
    client.passthrough.is_some()
        && (client.dialect.is_none()
            || RoutePlanner::plan_for_source(&request, config, source_format)
                .ok()
                .and_then(|plan| plan.endpoints.first().map(|e| e.route_mode))
                == Some(RouteMode::Native))
});
```

多一次纯计算的 plan，无 IO。改完可删 `:4148` 那段死逻辑。

- [x] 按上述修法调整 `passthrough_intent`，并把 passthrough body 一并清掉
      （`bridging` 直接读 `client.passthrough`，只改 plan 判定不够），删除死逻辑
- [x] `images_generation_alias_...` 是独立根因：capability 按 mapping 的 **pattern**
      推断，而 `grok-imagine-*` 的 stem 丢了 image/video 区分（`is_image_stem` 要求
      含 `imagine-image`）→ 推断成 Text 被过滤。改为按**请求的实际模型**推断
      （`routing.rs` 的 `plan_for_capability`）

## 顺带修绿的全仓门禁

- [x] `cargo clippy --workspace --all-targets -- -D warnings`：修掉 30+ 处
      （collapse if / needless borrow / `?` 重写 / `sort_by_key` / 无效 struct update
      / 重复分支），签名类的按项目既有惯例加 `allow` 并写明理由，复杂返回类型提
      `ExpiredEventRow` 别名
- [x] `cargo fmt --all -- --check`
- [x] `cargo test --workspace --locked`：0 失败
- [x] `uv run scripts/sync-usage-docs.py --check`
