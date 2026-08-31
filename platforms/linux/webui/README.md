# Sumpter Linux WebUI

这是 Linux 版唯一正式前端源码。构建产物写入 `../web/`，由 `sumpterd-linux --web-root`（发布包内二进制名为 `kekulvd`）静态托管。

```bash
npm ci
npm test
npm run build
```

`vite.config.js` 使用 `base: './'`，产物可挂载在 `/admin/`。保存前页面模型会转换成 Rust schema v6。Admin 登录使用 HttpOnly 会话 Cookie，写请求携带 CSRF，运行事件使用 `/admin/api/events` SSE 并自动重连。`?mock=1` 只用于浏览器本地验收。

契约测试在 `tests/`。改统计、运行页或布局时，对照 macOS 对应页面的信息层级和字段命名，再补平台布局差异。
