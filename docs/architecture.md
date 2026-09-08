# Sumpter 当前架构

Linux 和 macOS 共用同一套数据面、请求透传、模型映射、重试和运行时存储；平台能力由 adapter、app 和 `platforms/` 组合。

本文说明依赖方向、请求处理和运行时约束。完整目录、源码定位与测试归属统一见 [项目结构](project-structure.md)，执行命令见 [开发指南](development.md)。

## 依赖方向

```text
apps/<platform>/sumpterd
    └── adapters/<platform>
            └── sumpter-engine
                    ├── sumpter-runtime
                    │       └── sumpter-core
                    └── sumpter-core
```

约束：

1. `sumpter-core` 不依赖网络客户端、SQLite、平台 adapter 或 UI；`config_store` 仍负责配置文件读写、权限和迁移，不是完全无文件 I/O 的纯函数库。
2. `sumpter-runtime` 负责 bundled SQLite 存储与查询，依赖方向指向 core；不依赖 adapter 或 app。
3. `sumpter-engine` 只通过 `PlatformBoundary`、`EngineServices` 接收平台能力和上游传输；不直接引用 Linux / macOS crate。
4. adapter 可以依赖 shared crate，shared crate 不得反向依赖 adapter。
5. app 只负责参数解析、配置目录、监听启动、信号 / EOF 生命周期和平台组合，不复制请求处理逻辑。

## 请求处理与协议

HTTP 请求经平台服务组装进入共享引擎，依次完成访问检查、协议识别与路由准备、候选选择和上游转发，再由 relay 和完成记账记录结果。WebSocket 在完成上游握手后才向客户端返回升级响应。

原始请求的透传、必要模型映射、本地模型目录和已配置协议转换均由共享实现负责。客户端路径与能力范围见 [使用指南](../USAGE.md#4-协议与路径)。

Codex Live 的 SDP/multipart bootstrap（`POST /v1/live`、`POST /v1/realtime`、`POST /v1/realtime/calls`）是共享引擎内的 quicksilver 封装特例；出站把 POST `/v1/realtime` 改写到 `/v1/realtime/calls?intent=quicksilver&architecture=avas`（avas 只允许出现在 WebRTC `/calls`）。无 `call_id` 的 `GET /v1/realtime` 仍是公开 Realtime WebSocket 原生透传，并剥掉这些 query。

## 共享引擎内部

`engine/mod.rs` 只定义可克隆的 `Engine` 句柄、模块声明和既有公开类型的重导出。
实现仍属于同一个 crate，各职责使用明确的模块导入，内部类型最多在 `engine`
范围可见。`Engine`、`EngineServices` 和 adapter 调用的公开方法保持原有合同。

| 职责 | 实现模块 | 状态与交接 |
| --- | --- | --- |
| 构造和生命周期 | `state`、`lifecycle` | 组合共享状态、恢复持久化数据、替换配置并编排后台 flush |
| HTTP 入站和协议提示 | `inbound`、`context`、`protocol`、`payload`、`catalog` | 先检查访问权限，再消费惰性 body；保留原始报文和既有 Live/模型转换规则 |
| HTTP 调度和转发 | `dispatch`、`http_relay`、`http_response` | 保留入口排序、粘性、冷却和重试；响应被接纳后将完成保护对象交给 relay |
| 完成和事件 | `completion`、`events`、`failure` | `CompletionGuard` 管理 HTTP 完成与 Drop 取消；统一失败描述和 client/upstream 计数 |
| WebSocket | `websocket`、`websocket_relay` | 保留先完成上游握手再升级的入口；帧转发与关闭指标使用独立上下文 |
| 会话、抓包和统计 | `sessions`、`capture`、`runtime_api` | 各自管理绑定、诊断和查询/存储 API；SQLite 实现继续属于 `sumpter-runtime` |

`forward` 保留上游传输的兼容重导出。模块拆分不增加配置字段、crate 或平台专属引擎。

Runtime 内部也按数据生命周期分层：`runtime_store.rs` 只保留共享类型、
`RuntimeStore` 状态定义和模块声明；`runtime_store/store_api.rs` 实现公开存储方法，
`runtime_store/schema.rs` 负责 schema、投影与
rollup，`worker.rs` 负责写入 worker，`maintenance.rs` 负责保留策略/清理，
`analytics.rs` 负责投影聚合，`export.rs` 负责会话导出，测试位于同目录
`tests.rs`。只读查询以 `runtime_query.rs` 的公共模型和过滤器为边界，具体实现
分布在 `runtime_query/events.rs`、`trends.rs`、`analytics.rs`、`facets.rs`、
`errors.rs`、`dimensions.rs`、`export.rs` 与 `storage.rs`；这些模块共享过滤器、
快照和 SQL 辅助函数，但不改变 crate 的公开查询函数合同。

并发和持久化约束：runtime 写操作保持 `runtime_write → state` 锁顺序；
抓包 flush/clear 保持 `capture_flush → capture → capture_index` 顺序；
会话在释放内存锁后落盘。加载损坏的绑定或诊断文件时继续禁止隐式覆盖，过期资源绑定
在启动时裁剪并写回。HTTP 的完成保护对象沿 `dispatch → http_relay` 唯一移交，
WebSocket 继续使用自身的完成记账流程。

## 模型组与统一地址

schema v7 将配置分为入口库、模型组和组内绑定。`endpoints` 保存地址、凭据、协议和原始映射；`modelGroups` 声明模型范围与组优先级；`bindings` 引用入口 ID，并声明全部/指定模型、组内优先级与局部模型覆盖。客户端继续请求同一监听地址并发送原模型名。

`core/model_groups.rs` 从配置生成临时路由投影，原入口配置不会被改写。投影继承原映射参数与能力，精确映射和最长前缀优先；多个组可以引用同一入口。普通模型、媒体能力路由与本地模型目录使用投影；已有固定入口功能规则和资源所有者绑定继续保留原语义。

基础候选按组优先级、组数组顺序、绑定优先级、绑定数组顺序排列。已有调度器继续处理会话粘性、冷却、入口内 500 重试、非 500 粘性重试、跨轮重试、退避、Retry-After、超时和不限时语义。模型组不创建另一层重试预算；失败仍围绕同一有效模型切换。同一入口与相同实际调用跨组去重，不同上游映射保留。

运行事件及 SQLite 列表投影增加 `modelGroupID/modelGroupName`，用于识别实际尝试来自哪个组。旧 schema v3/v4/v5/v6 文件先备份后迁移，默认组保留旧入口顺序、优先级、映射与粘性标识。顶层模型组缺省兼容旧路由，空数组关闭自动模型路由；删除入口后两端编辑事务清理组绑定及功能规则引用。

## 平台边界

`crates/sumpter-engine/src/boundary.rs` 是唯一边界入口：

- `authorize_status`：控制面 `/__status` 的平台访问策略；
- `platform_action`：声明平台专属动作（macOS 通知 / reload，Linux 保持 404）；
- `handle_platform_action`：执行通知、reload 等副作用；
- `validate_opened_capture`：由平台提供文件身份校验；
- `EngineServices`：注入上游传输和 `PlatformBoundary`。

共享引擎默认使用 `NoopPlatform`，因此 core / runtime / engine 可以在没有操作系统控制面时独立测试。实际二进制由对应 adapter 注入具体 `Platform`。

对应页面优先保持同一信息层级、字段命名、状态语义和主要交互；只有原生控件、窗口形态或平台生命周期确有差异时才保留平台化表现。运行统计存储统一使用 `maxAgeDays` 与 `storageLimitBytes` 的 OR 轮换语义，进行中的请求组整体保护；低频技术字段进入详情，策略编辑使用 Linux 弹窗 / macOS sheet。

Linux `admin.rs` 保留 listener / 配置事务状态、路由组装和 SSE 生命周期，
路由处理按职责放在 `admin_auth_routes.rs`、`admin_config.rs`、`admin_runtime.rs`、
`admin_diagnostics.rs` 与 `admin_autostart.rs`。既有 `admin_auth.rs` 继续管理
登录会话与凭据；配置和模型探测属于平台 Admin，公开 `admin::validate_config`
入口保持不变。

macOS `main.swift` 保留应用入口、AppModel 状态/初始化和原生通知支持类型。
AppModel 方法分布在 `AppModel+Lifecycle.swift`、`AppModel+RuntimeStatus.swift`、
`AppModel+Analytics.swift`、`AppModel+Diagnostics.swift`、`AppModel+Config.swift`、
`AppModel+Providers.swift` 与 `AppModel+Notifications.swift`。这些 extension
继续共享同一个 `@MainActor` AppModel 和 `@Published` 状态；跨文件使用的内部成员
为模块内可见，异步响应顺序保护、sidecar 生命周期和通知行为保持原有语义。

## 变更归属

按 [源码职责表](project-structure.md#按需求找源码) 定位实现。共享行为放在 core、runtime 或 engine，平台权限和生命周期放在 adapter / app，界面与安装打包输入放在 `platforms/`。保持上述依赖方向，避免在两个平台复制一套共享逻辑。

HTTP header、环境变量、服务单元名、导出格式标识和 sticky domain 属于兼容协议或运行时数据字段。改工程包名或目录时不要机械替换这些字段。

## 构建与验证

从仓库根运行统一检查，具体命令及最小验证范围见 [开发指南](development.md#按变更选择验证)。共享行为覆盖两个 adapter；WebUI、SwiftUI、安装脚本和发布包分别验证。

当前工作流统一位于根 `.github/workflows/`，版本发布入口为 `release.yml`。本机 DMG 构建见 [脚本目录](../scripts/README.md)，平台产物、签名和发布验收见 [发布指南](releasing.md)。

## 品牌与运行时名字

工程包、macOS App、Linux 配置目录、systemd 单元、发布包二进制、环境变量、入站 header 和导出格式统一为 `sumpter` / `Sumpter`：

- 配置：`~/.config/sumpter`、`/var/lib/sumpter`、`/opt/sumpter`
- 二进制与单元：`sumpterd`、`sumpter.service`
- 环境变量 `SUMPTER_*` / `SUMPTERD_*`
- 入站 header `X-Sumpter-*`
- 导出格式 `sumpter-session-export-v1`
- 粘性域 `sumpter-sticky-v3`

旧的 `kekulv` 路径、单元、header 和环境变量不再识别。已有安装需要卸载后重装，不要做原地双读迁移。
