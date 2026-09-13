# Linux WebUI 开发

`platforms/linux/webui/` 是 WebUI 源码，`platforms/linux/web/` 是由 Vite 生成并提交的静态文件。daemon 从 `/admin/` 提供生成目录。

```bash
npm ci
npm test
npm run build
```

开发预览：

```bash
npm run dev
```

`?mock=1` 用于不连接 daemon 的本地界面验收。真实 Admin 使用 HttpOnly Cookie 登录、CSRF 保护和 `/admin/api/events` SSE。改动页面时保持与 macOS 相同的字段、状态语义和主要交互；布局、sheet 与原生控件可按平台适配。

提交前从仓库根运行 `./scripts/check.sh web`，它会测试、构建并检查生成目录差异。不要手工编辑 `web/` 或提交本地 node_modules。
