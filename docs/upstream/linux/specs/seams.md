# Linux Rust 组件边界

旧 SwiftNIO `LocalHTTPServing` 协议缝已经退役。Rust Linux 版按以下边界维护：

- `kekulv-core`：schema v6（迁移 v3/v4/v5）、四态入口路由、调度、Native Adapter、Translator、
  事件与校验；不包含 WebUI 或进程生命周期。
- `kekulv-proxy`：axum/reqwest/rustls 代理引擎、出站连接、统计与运行事件。
- `kekulvd`：XDG 路径、双 listener、Admin API、静态 Web、信号处理、配置写入与监听重绑。
- `webui/`：React 19 + Vite 6 前端源码，Linux 版唯一正式前端；只通过 `/admin/api/*`
  管理 daemon。
- `web/`：`webui/` 的构建产物，随仓库入库并由 `kekulvd --web-root` 静态托管；不手工编辑，
  改前端一律改 `webui/` 后重新构建。

Proxy listener 按 `config.listener` 绑定；Admin listener 默认 loopback
`127.0.0.1:57879`，可由 `--admin-host`/`--admin-port` 或 `KEKULV_ADMIN_*` 覆盖（不进
config.json）；配置目录 `admin-password` 初始化 WebUI 内置登录，API/SSE 使用会话 Cookie +
CSRF，静态登录壳可公开加载。两者生命周期解耦，确保 proxy 停止或重绑时 WebUI 仍可诊断和恢复。

Linux 版复制 macOS Rust backend 后允许的有意差异仅限：XDG 路径、standalone signal 生命周期、
Admin/Web、配置写入、systemd、自启动以及明确移除通知。路由、协议桥接、重试、统计口径等行为应
继续由共享 fixtures 和差分测试约束。
