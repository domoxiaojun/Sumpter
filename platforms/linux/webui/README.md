# Linux Gemini WebUI

这是 Linux 版唯一正式前端源码。界面采用 Gemini 方案的六页视觉与交互，构建产物直接写入
`../web/`，由 `kekulvd --web-root` 静态托管。

```bash
npm ci
npm test
npm run build
```

`vite.config.js` 使用 `base: './'`，产物可挂载在 `/admin/`。真实配置在保存前会从页面模型
转换为 Rust schema v6，Admin 登录使用 HttpOnly 会话 Cookie，写请求携带 CSRF，运行事件使用
`/admin/api/events` SSE 并自动重连。`?mock=1` 只用于浏览器本地验收。
