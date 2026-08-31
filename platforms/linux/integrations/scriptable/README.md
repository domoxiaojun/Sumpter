# Scriptable 状态小组件

注意：Scriptable 直接运行 JavaScript，不能直接运行 `.ts`/`.tsx`；因此可粘贴文件是 `.js`。
`.tsx` 属于 React/TypeScript 页面组件，不是 Scriptable 的脚本格式。

`sumpter-status.js` 是一个只读的 iOS Scriptable 小组件，只显示 Linux 当前的进行中请求：

- 进行中的客户端请求数量
- 每个请求的模型、入口、客户端/用途和已等待时长
- proxy 是否运行、版本、在线时长与入口数量（作为上下文）

内部 upstream 重试事件和已经完成的请求不会单独显示。

## 使用前提

Linux 默认只把 Admin 绑定到 `127.0.0.1:57879`，iPhone 无法直接访问。请先通过 VPN/Tailscale
或 HTTPS 反向代理提供一个 iPhone 可达的管理地址，例如 `https://admin.example.com`。不要把
57879 明文暴露到公网；反向代理应保留 `Set-Cookie`，并将 HTTPS 转发到本机的 Admin 端口。

## 安装

1. 将 `sumpter-status.js` 的内容复制到 Scriptable 的新脚本。
2. 在 Scriptable 内手动运行，填写 HTTPS 地址、用户名和管理员密码。
3. 添加 Scriptable 小组件并选择 `Sumpter Linux Status`，建议先使用中号确认布局。
4. 手动运行脚本时可选择“修改连接配置”或“清除本机配置”。

密码和会话 Cookie 使用 iOS Keychain 保存；脚本不调用启停、配置写入、重载或统计重置接口。
小组件约每 5 分钟刷新一次，实际刷新时间仍受 iOS 调度影响。
