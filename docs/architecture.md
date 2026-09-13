# 架构说明

Sumpter 的核心边界是“客户端请求进入一个地址，代理按配置选择上游并记录结果”。Linux 与 macOS 共享数据面，平台 adapter 只负责监听、Admin、生命周期和原生能力。

## 请求路径

```text
客户端
  → listener 访问检查与请求识别
  → 模型 / 能力 / featureRule 路由规划
  → 会话粘性、优先级、随机或轮询调度
  → Provider 上游认证与 raw relay
  → 响应 / 流 / WebSocket 回传
  → RuntimeEvent 与 usage 写入 SQLite
```

主请求采用 raw 透传：保留客户端方法、路径、查询、请求体和响应流；代理只做鉴权、映射、路由、重试、上游凭据注入及必要的模型路径更新。旧桥接能力仍用于明确的内部兼容调用，不能把“入口 protocol”理解成自动转换器。

HTTP、SSE 和 WebSocket 在上游响应或真实握手后才算成功。一个客户端请求可包含多次上游尝试，RuntimeEvent 用同一 request ID 关联它们，界面分别展示客户端最终结果和上游尝试链。

## 配置与调度

`AppConfig` 的 v7 JSON 是跨平台合同。入口保存地址、Key、协议标签和 mapping；模型组保存开放模型、绑定和调度策略；featureRules 负责独立子请求。配置保存使用 generation 检查，避免两个 Admin 页面互相覆盖。

会话粘性优先于新会话调度。`priority` 按数字和数组顺序排序；`randomSticky` 和 `roundRobinSticky` 只决定新会话首选，故障仍遵循重试与后备链。资源绑定文件保存 Live / Video 等后续请求的入口归属。

## 运行时存储

`sumpter-runtime` 使用 bundled SQLite、WAL 和有界后台写入。内存快照让请求热路径不等待每次数据库写入；存储退化和 backpressure 通过 Admin 状态暴露。运行统计、会话删除、清理、重置和重建是不同操作，文档不得混用。

统计只从客户端完成事件的上游 usage 聚合，pending 单独计数；缓存 Token 按协议口径保留原始值。诊断捕获独立于统计，默认关闭，可能包含未脱敏正文和凭据。

## 平台边界

Linux adapter 组合 Admin API、WebUI 静态服务、systemd 控制和 Linux 生命周期。macOS adapter 组合 sidecar、原生菜单栏 / 通知和 App 控制。两端页面保持字段、状态语义与主要交互一致，窗口、sheet、通知和系统服务可按平台适配。

Admin 监听与 proxy listener 分离。`/healthz` 只证明 Admin 存活；管理 API 由 HttpOnly Cookie 和 CSRF 保护。代理入站 Token、Admin 密码、上游 API Key 和 macOS control token 互不替代。

## 安全原则

默认监听 loopback；远程 Admin 通过 HTTPS 反代、VPN 或 SSH 隧道访问。`X-Sumpter-*` 归因头仅用于入站统计，转发上游前剥离。源码、示例、日志和测试不得包含真实凭据。
