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

`?mock=1` 用于不连接 daemon 的本地界面验收，只在 `npm run dev` 或显式 `VITE_SUMPTER_MOCK=1 npm run build` 的构建中生效；提交的 `web/` 产物不包含演示数据。真实 Admin 使用 HttpOnly Cookie 登录、CSRF 保护和 `/admin/api/events` SSE。改动页面时保持与 macOS 相同的字段、状态语义和主要交互；布局、sheet 与原生控件可按平台适配。

daemon 对 `/admin/` 下发 `style-src 'self'` 且不放行 `data:` 的 CSP，而 Vite 开发服务不带这个响应头。应用代码不要在运行时注入 `<style>`，动态颜色用元素 `style` 上的 CSS 变量。Paper Shaders、Radix 滚动锁及选择器的已知注入样式由 `shadcn.css` 提供静态版本；滚动锁沿用组件库的嵌套计数，根元素保留滚动条槽位。

改动图表、字体、页面外壳或交互组件后运行 `npm run test:browser`：它用演示数据构建，按 daemon 的 CSP 提供静态文件，再用本机 Chrome（可用 `CHROME_BIN` 指定）逐页检查 CSP 违规、未捕获异常、图表颜色和分包丢失时的恢复。共享的 `tests/browser/interaction-checks.mjs` 还验证弹窗、嵌套弹窗、菜单、选择器与移动端抽屉的锁定和释放、收起导航名称与焦点提示，以及命令面板的方向键、循环、筛选、中文输入法和小窗口行为。

提交前从仓库根运行 `./scripts/check.sh web`，它会测试、构建并检查生成目录差异。不要手工编辑 `web/` 或提交本地 node_modules。
