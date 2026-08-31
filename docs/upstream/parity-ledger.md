# 双端对拍账本(parity ledger)

`macos/crates` 与 `linux/crates` 是同源副本。本文件逐处裁定两侧差异,每条标 **有意 / 漂移 /
格式**,供 CI 门禁白名单与修复清单直接引用。裁定日期见各节标题。

## 复现方法

```bash
# 逐文件原始差异
diff macos/crates/<path> linux/crates/<path> | grep -c '^[<>]'

# 归一化差异(去注释、压空白、去空行)= 近似语义差异
# 注意 grep 必须用 '^[[:space:]]*$' —— 只写 '^$' 会漏掉「只含空白的行」,
# 导致注释被剥后残留的缩进被当成真差异,归一化数字整体偏高。
norm() { sed -e 's|//.*$||' -e 's/[[:space:]]\+/ /g' -e 's/^ //;s/ $//' "$1" | grep -v '^[[:space:]]*$'; }
diff <(norm macos/crates/<path>) <(norm linux/crates/<path>) | grep -c '^[<>]'

# impl Engine 方法体逐个对拍(见 0.1 节结论)
awk '/^    (pub )?(async )?fn [a-z_0-9]+/ {if(n!="")close(o"/"n".body"); l=$0;
  sub(/^.*fn /,"",l); sub(/[^a-z_0-9].*$/,"",l); n=l} n!=""{print > (o"/"n".body")}' \
  o=/tmp/body-macos macos/crates/kekulv-proxy/src/engine.rs
```

## 基线(2026-08-30)

| 层 | 生产码 | 原始差异 | 归一化 | 判断 |
|---|---|---|---|---|
| 共享纯逻辑(core 8 + proxy 3 文件) | ~9000 | ~120 | **~50** | 事实一致 |
| 统计子系统(runtime_store + runtime_query) | 9928 | 20 | **1** | 事实一致,零反向依赖 |
| 端点契约(28 条 route,归一化前缀后) | — | — | **0** | 零漂移,macOS ⊆ Linux |
| 引擎/管理面(engine/admin/config_store/main/server) | ~8500 | 7101 | **6002** | 真分叉,有理由 |

逐文件归一化噪音占比:`runtime_query` 95% · `events` 87% · `config`/`request_build` 50% ·
`engine` 9% · `admin` 20% · `tests/engine` 6%。**共享文件的差异几乎全是注释;分叉文件的是真的。**

`impl Engine` = 3696 行 / 91 方法(macOS)、89(Linux)。职责分布:统计委托 26 · 诊断捕获 17 ·
刷盘 6 · 事件记录 6/5 · 配置生命周期 8/5 · **真正的请求处理仅 11/9**。
前 55 个方法两侧数量相同、本该一致 —— 但与有平台差异的转发循环同处一个 impl 块,`cmp` 用不上。

## 0.1 Engine 方法体裁定(2026-08-30,55 个本该一致的方法)

方法体对拍:**40 逐字一致 / 24 差异 / 1 单边**。24 个差异逐条裁定如下。

### 漂移(3 处,需修)

| 方法 | 差异 | 裁定 |
|---|---|---|
| `capture_attempt_started` | Linux 的 `base_bytes` 多算 `protocol_token(source_format)` 与 `route_mode` 的 "native"/"translated" 长度 | **漂移(双边不自洽)**,见下 |
| `complete_upstream` | macOS 有三行 `event.{source_format,target_format,route_mode} = …or(client.…)`,**Linux 没有** | **漂移(Linux 缺)** |
| `capture_upstream_chunk` | attempt 未找到时:macOS 已写 `truncated` 并 `refresh_capture_usage`+`sync_capture_index_record`+`capture_dirty`;Linux 直接 return | **漂移(副作用不同)** |

**`capture_attempt_started` 的证据链**:两侧 `DiagnosticAttemptCapture` 结构完全一致,均含
`source_format` / `route_mode`;但**持久化侧的 `diagnostic_attempt_size` 两侧逐字一致,且只算
`attempt.protocol.len()`,不算这两个字段**。所以 Linux 是写入预算算、持久化核算不算(内部不
自洽),macOS 是两处都不算(自洽但低估实际占用)。**需要产品决策**:两处都算(正确),还是两处
都不算(维持 macOS 现状)。影响面:诊断捕获 `max_bytes` 的实际利用率,每 attempt 约 16–20 字节。

**`complete_upstream` 的影响**:响应头之前的失败(transport / 429 / 工具兼容)由
`CompletionGuard` 之外记录,macOS 会从 in-flight client 事件补齐协议路由三字段,Linux 不补 →
**Linux 的这类 upstream 事件缺 source/target format 与 route_mode**,诊断与统计归因都看不到。
macOS 侧注释写明了这正是设计意图。**方向:macOS → Linux**。

### 有意(2 处,已在 RUST_UPSTREAM.md 记录)

| 方法 | 差异 |
|---|---|
| `capture_start` | macOS 9 个独立参数 + `#[allow(clippy::too_many_arguments)]`;Linux 聚合为 `meta: &ClientMeta` |
| `record_rejected_client_with_metadata` / `_with_id_and_metadata`(仅 macOS) | 同上,macOS 另有 `PlanMeta::clone_meta` trait,参数多穿一层 |

### 单边增强(2 处,可选跟进)

| 方法 | 差异 | 建议 |
|---|---|---|
| `runtime_analytics_filtered` | Linux 响应多回显 `"endpointID": filter.endpoint_id` | macOS 可跟进(纯回显,不影响正确性) |
| `set_diagnostic_capture` | Linux 用 `CAPTURE_STOP_MANUAL`/`CAPTURE_STOP_CAPACITY` 常量,macOS 用字面量 `"manual"`/`"capacity_limit"` | 常量值已核对**完全相同**;macOS 跟进用常量,防将来拼错 |

### 应删的多余代码(1 处)

| 方法 | 差异 | 建议 |
|---|---|---|
| `record_chunk` | Linux `let started = *self.started.get_or_insert(now);` + `let _ = started;` | 语义与 macOS 的 `self.started.get_or_insert(now);` 完全相同,是绕 clippy 的空包装 → 删掉,回归 macOS 写法 |

### 格式/注释(15 处,无语义差异)

`runtime_summary_value`(`json!` 多键一行 vs 每键一行) · `diagnostic_capture_index`(`"k":v` vs
`"k": v`,差一个空格) · `diagnostic_capture_detail` / `_json`(参数名 `id` vs `request_id`) ·
`diagnostic_capture_export_file` / `diagnostic_capture_snapshot`(空行) ·
`sync_capture_index_record`(`retain` 表达式 vs 块) · `sync_capture_index_status` /
`sync_capture_index_window`(Linux 多中文注释) · `clear_diagnostic_capture`(`captured_bytes = 0`
前后移一行,无依赖) · `recreate_runtime` / `record_stream_terminal` / `complete_from_stream` /
`enqueue_runtime_change`(注释措辞) · `complete_upstream_event`(两行相邻赋值换位,无依赖)

`json!` 宏内部 rustfmt 不格式化,手写风格差异会永久留存并污染每一次 diff —— 归一化时优先处理。

### 提取噪音(1 处,非真差异)

`flush_session_affinity` 的 1 行差异是下一个方法的文档注释被方法体提取算法带入。

## 0.2 共享逻辑文件裁定 — 待做

归一化差异约 50 行:`config` 14 · `warnings` 20 · `stream_terminal` 8 · `request_build` 6 ·
`events` 2 · `runtime_query` 1。

## 0.3 测试层裁定 — 待做

已定位:4 处单边缺口(`client_declared_project_headers_reach_both_forwarded_and_rejected_events`
`ingress_preflight_never_polls_rejected_or_bodyless_request_bodies`
`initial_client_in_flight_records_preferred_translated_protocol_route`
`mixed_priority_sticky_group_warns_with_minimum_rule`,均 Linux 有 macOS 缺)、
3 对同测试改名、2 个 helper 不对称(`next_notify` 仅 macOS / `body_with_poll_flag` 仅 Linux)。

## 附:与 A 方案清单的偏差

`linux/RUST_UPSTREAM.md` 的差异表基线是 2026-08-22;`admin.rs` 当时 ~2680,今天 4290;
`engine.rs` ~1320 → 1684。**分叉在增长**,所以分叉文件也需要棘轮基线,不能只锁清单内文件。

另:两侧 `admin.rs` 各有 2 处生产环境的游离 `reqwest::Client::builder()`(macOS `289`/`320`,
Linux `2383`/`2416`),不走 `outbound.rs:161` 的唯一入口。`admin.rs` 不在 cmp 清单内,所以
**"探测鉴权同配置"那类 bug 靠 cmp 门禁挡不住** —— 需要独立的 `ast-grep` 门禁。

## 0.5 已修复(2026-08-30)

两侧 `cargo test --workspace` 全绿(macOS 324 / Linux 348)。差异变化:
`warnings.rs` 25→**0** · `tests/routing.rs` 7→**0** · `request_build.rs` 12→6 · `engine.rs` 1684→1665。

`capture_attempt_started` 的决策依据(此前标为待定):`diagnostic_attempt_size` 两侧逐字一致,
它只累加**变长字段**(String / Vec)的字节,不算 `started_at_ms`、`response_status` 等定长字段。
枚举序列化后是有界短字符串(最长 `translated` = 10 字节),属同一类可忽略项。Linux 的加法
既漏了 `target_format`、又让写入预算与持久化核算口径不一致,因此**两处都不算**,删掉那 6 行。

## 入站 body 读取时机 —— 真实架构缺陷(macOS,2026-08-30 发现)

**这不是测试缺口,是当前就存在的资源耗尽面。** 起因是移植
`ingress_preflight_never_polls_rejected_or_bodyless_request_bodies` 时发现两侧
`handle_request` 签名不同:

| | 签名 | body 读取位置 |
|---|---|---|
| macOS | `body: Bytes` | **`server.rs:46` —— 进引擎之前**就 `to_bytes(…, 64 MiB)` |
| Linux | `body: impl Into<Body>` | `engine.rs` 三处,均在 `inbound_auth_required` 401 **之后** |

后果:向 macOS 版发「大 body + 错误 token」,服务端会先把最多 64 MiB 读进内存,才返回 401。
Linux 版在读 body 前就拒掉。Linux 侧那个测试正是修完之后加上去钉住此行为的,未同步回 macOS。

修复路径(未做,需单独一轮):
1. macOS `handle_request` 签名改 `body: impl Into<Body>`;
2. `MAX_BODY_BYTES` 从 `server.rs` 移入 `engine.rs`,在三个入站分支鉴权后各自 `to_bytes`;
3. `server.rs` 直接传 `request.into_body()`;
4. 然后才能移植那个测试。

**签名改动向后兼容**(`Bytes: Into<Body>`),现有测试调用点无需改写 —— 风险主要在入站路径的
三个分支要逐一对齐 Linux 的错误记账(`record_rejected_client_with_metadata` 的 400 分支)。

## 0.3 测试层裁定与处理(2026-08-30)

`tests/engine.rs` 差异 1929 → **1789**;测试名共有 121 个。

### 「3 对改名」的真实性质 —— 只有 1 对是改名

裁定前当成"同一测试改了名"的三对,逐一比过函数体后结论不同:

| 对 | 真实性质 | 处理 |
|---|---|---|
| `mixed_retryable_http_and_connection_failure_recovers_next_round`(macOS 用 401) ↔ `http_502_mixed_with_connection_error_retries_and_recovers`(Linux 用 502) | **两个不同场景**。502 走 deferred 路径,与 401 的 failover 语义不同 | **两侧各补对方场景**,现在两侧都有这两个测试,逐字一致 |
| `webfetch_and_websearch_rules_bridge_via_openai`(macOS 87 行) ↔ `webfetch_rule_bridges_on_openai`(Linux 42 行) | **组织差异**:macOS 一个测试覆盖 webfetch + websearch 两段,Linux 拆开 | **保留双方**,进白名单(见下) |
| `websearch_auto_adapts_to_target_protocol_and_injects_upstream_search` ↔ `websearch_target_protocol_bridges_and_injects_upstream_search` | **纯改名**,函数体只差 3 行注释措辞 | 统一为 Linux 名,该测试**差异归零** |

> 教训:先比函数体再断言"这是改名"。中间一次比错了对象(拿 macOS 合并测试的后半段去比 Linux
> 的独立 websearch 测试),得出过"Linux 覆盖严格更强"的错误结论;正确对比对象是 macOS 自己的
> `websearch_auto_adapts_...`,两者只差注释。

### 不可复刻清单(环境/架构决定,**不是**漏改)

这些差异**不应**被"对称化",强行复刻会得到错误的测试:

| 项 | 为什么不可复刻 |
|---|---|
| `linux_data_plane_has_no_control_or_notify_write_endpoints`(仅 Linux) | Linux 独有的 404 契约 |
| `control_endpoints_require_token_and_status_masks_auth`(仅 macOS) | macOS 独有控制端点 |
| `notify_classifies_client_actions_and_suppresses_duplicate_hooks`、`notify_enriches_hook_payloads`、`stop_failure_adds_http_status_only_from_matching_claude_runtime_event`、`stop_failure_uses_safe_error_summary_without_inventing_http_status`(仅 macOS) | Linux 无通知链路与 Claude Hook |
| `webfetch_and_websearch_rules_bridge_via_openai`(macOS) / `webfetch_rule_bridges_on_openai`(Linux) | 有意的组织差异:macOS 合并覆盖,且其 websearch 段用 `qwen-ws` 夹具(Linux 独立测试用 `gpt-search`)。行为覆盖等价,保留双方以多一个 mapping 夹具变体 |

**上表即 Tier B 测试名对称门禁的白名单,每条都带理由。** 白名单只应因新的平台功能而增长;
任何没有理由的新条目都要当成漏改处理。

### 移植时验过的环境依赖(可复刻的前提)

移植前逐项确认过,任一不成立则期望值需重新推导:`two_endpoint_config()` 两侧**逐字一致** ·
`runtime_of` 一致 · `call`/`loopback`/`body`/`sse_ok`/`push`/`requests` 签名一致 ·
macOS 支持 `/v1/responses` 入站 · `Outcome::Hang` 两侧都有。
(`engine_with` 内部不同 —— macOS 的 `Engine::new` 多一个 control token 参数 —— 但对调用方透明。)

## 0.6 入站正文读取时机 —— 已修复(2026-08-30)

上面「不可复刻清单」原有两条已**删除**:`ingress_preflight_never_polls_rejected_or_bodyless_request_bodies`
与 `body_with_poll_flag` 现在两侧都有。

**教训:有些「不可复刻」是架构缺陷的症状,不是平台事实。** 该测试原本判定不可复刻,理由是
macOS 的 `handle_request(body: Bytes)` 让"不读 body"无法断言 —— 但那个签名本身就是缺陷。
修掉之后测试自然可复刻了。判定不可复刻前要先问:这个差异是平台强加的,还是一侧的实现偏差?

更关键的是:**macOS 的实现此前违反了自己的 spec。** `specs/spec-engine.md` §3 早就规定
「1. CIDR → 2. 入站 auth → 3. …再解析 body」,而代码在 `server.rs:46` 于步骤 1 之前就
`to_bytes` 读满了。Linux 符合 spec,macOS 不符合,而 spec 没把「不提前读取」写成显式不变量,
所以两侧都没测试钉它(Linux 那条测试是实现之后补的,未回写 spec)。现已在两侧 spec 显式化。

改动(macOS,对齐 Linux):
1. `MAX_BODY_BYTES` 移入 `engine.rs` 并 `pub`,`server.rs` 引用它(与 Linux 同形);
2. `handle_request` 签名 `body: Bytes` → `body: impl Into<Body>`,`server.rs` 直接传
   `request.into_body()`,不再进引擎前读满;
3. `handle_messages` / `handle_openai_inbound` / `handle_native_openai_passthrough` 三个入站
   处理器各自在 `inbound_auth_ok` 之后 `to_bytes`,并采用 Linux 的两阶段归因:鉴权前
   `CodexMetadata::from_request(&headers, None)`(仅 header),读完再算正文派生的完整版;
4. `/__notify` 是 macOS 独有控制端点(Linux 恒 404,无参照),在 match arm 内读 body。

行为变化(可观察):macOS 的 401/400 事件不再包含正文派生的 Codex 元数据,只保留 header 归因。
对鉴权失败的请求,正文本来也不可信。`rejected_requests_preserve_codex_metadata` 仍通过。

移植后的测试期望值按 macOS 语义调整:`(external, "/__notify", 403)` —— macOS 的 CIDR 校验在
路径分发之前,且该端点真实存在;Linux 是 404(端点不存在)。这一条差异仍是平台事实。

签名改动向后兼容(`Bytes: Into<Body>`),现有测试调用点无需改写。

## 本轮最终状态(2026-08-30)

| 文件 | 开始 | 现在 |
|---|---|---|
| `kekulv-core/src/warnings.rs` | 25 | **0** |
| `kekulv-core/tests/routing.rs` | 7 | **0** |
| `kekulv-proxy/src/request_build.rs` | 12 | 6 |
| `kekulv-proxy/src/engine.rs` | 1684 | 1626 |
| `kekulv-proxy/tests/engine.rs` | 1929 | 1754 |

**逐字节一致的文件:11 → 13 / 27。** 测试:macOS 328 passed / Linux 349 passed,10 个套件全通过。

`json!` 宏内 `"k":v` 无空格写法只剩 0 处(原 macOS 5 处,贡献 28 行 diff)—— rustfmt 不格式化
宏内部,这类手写风格差异一旦出现就会永久污染 diff,应在 Tier A 门禁里一并锁住。

### 下一步(阶段 1+,尚未做)

1. 抽 `kekulv-runtime` crate:`runtime_store.rs` + `runtime_query.rs` 共 9928 行生产码,
   占 kekulv-proxy 的 54%,实测零反向依赖(两者都不 `use` engine/server/outbound/request_build),
   归一化差异仅 1 行。一次性把一半代码移出对拍范围。
2. 三层门禁:Tier A `cmp` 清单(现可覆盖 13 个文件)+ Tier B 测试名对称(白名单见上)+
   Tier C 分叉棘轮(engine 1626 / admin 4290 / config_store 464 / main 565 / server 98)。
3. `Client` 构造收敛:两侧 `admin.rs` 各有 2 处生产环境游离 `reqwest::Client::builder()`
   (macOS `289`/`320`,Linux `2383`/`2416`),不走 `outbound.rs:161` 唯一入口。加 `ast-grep` 门禁。
4. 补 macOS PR CI:`ci.yml` 在仓库根(=Linux)跑 `cargo check --workspace`,`macos/` 子目录只有
   推 tag 时才被 `macos-release.yml` 碰到 —— macOS Rust 在 PR 阶段零 CI 覆盖。
   `macos/crates` 零平台专属依赖,ubuntu runner 就能跑。
5. 拆 `Engine` God impl(3696 行 / 91 方法,真正的请求处理只有 11 个)—— 必须在门禁之后。
6. 剩余未裁定:`0.2` 共享逻辑文件的约 50 行归一化差异、`config_store.rs` 迁移覆盖对比
   (`migrate_legacy_value(source_schema)` vs `migrate_v3_value`+`migrate_v4_value`)、
   `x-kekulv-request-id` 覆盖面(macOS 11 个调用点 vs Linux 内联 2 处)。

## 0.7 修正:归一化口径与门禁分层(2026-08-30)

**上文各处「归一化差异」数字曾整体偏高**,原因是 `norm()` 用 `grep -v '^$'` 只滤完全空行,
没滤「只含空白的行」;注释被剥掉后残留的缩进因此被算成真差异。修正为
`grep -v '^[[:space:]]*$'` 后,`stream_terminal.rs`(原记 8)、`events.rs`(原记 2)、
`runtime_query.rs`(原记 1)的归一化差异实际都是 **0**。

### 准确分层(本轮收尾时)

| 层 | 文件数 | 文件 |
|---|---|---|
| **A1 逐字节一致** | **13** | `access` `bridge_in` `core/lib` `model_name` `scheduler` `warnings` `tests/bridge_in` `tests/golden` `tests/retry_policy` `tests/routing` `health` `outbound` `runtime_store` |
| **A2 归一化一致**(仅注释/空白不同) | **7** | `bridge` `config` `routing` `request_build` `stream_terminal` `events` `runtime_query` |
| 仍有逻辑差异 | 7 | `admin` 3395 · `tests/engine` 1615 · `engine` 1406 · `main` 518 · `config_store` 430 · `server` 70 · `proxy/lib` 1 |

**门禁可覆盖 20 / 27 个文件(74%)。**

### 门禁必须分两级 —— A1 之外还要 A2

原方案只设「Tier A 逐字节 cmp」,这是不够的:**有些文件永远无法逐字节一致,因为注释必须描述
各自平台**。`config.rs` 是典型 —— 它的 14 行差异里 9 行是有意的:

- 文件头:macOS 写「config.json 由 SwiftUI 壳负责编辑与写盘」;Linux 写「Admin PUT 是唯一远程
  写入口,reload/SIGHUP 只读磁盘」。**两段各自准确,不该统一。**
- `/// 兼容旧 SwiftUI/测试命名` vs `/// 兼容旧 WebUI/测试命名`。同理。

强行统一这类注释,等于让文档变得不准确来迁就门禁。所以:

- **A1 逐字节 `cmp`** —— 用于无平台特定注释的文件(13 个)
- **A2 归一化 `cmp`**(去注释、压空白后比对)—— 用于逻辑必须相同、但注释各自描述平台的文件(7 个)

A2 同样能挡住全部逻辑漂移,这才是门禁的目的;逐字节只是手段。
`kekulv-proxy/src/lib.rs` 的唯一差异是 Linux 的 `mod admin_auth;`,进白名单后也归 A2。

### 本轮对 0.2 的处理

- `routing.rs`:删掉 Linux 的冗余中间变量 `mapped_upstream_model`(纯包装,已删)。
- `config.rs`:`session_sticky_retries` 在 struct 与 `Default` 中移到**字母序位**(macOS 原先
  插在 `pinned_ip_concurrency` 之前打断了序;Linux 是完整字母序,故以 Linux 为准),
  `normalized()` 的赋值顺序反向对齐 macOS,文档措辞统一。两侧 golden 各 8 passed —— 字段声明
  顺序不影响磁盘格式(serde 按 `rename` + 字母序输出)。
- 其余(`stream_terminal`/`events`/`runtime_query`)本就归一化一致,**不动注释措辞** —— 见上。
