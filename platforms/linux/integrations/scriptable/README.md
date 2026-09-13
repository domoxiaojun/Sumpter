# Scriptable 状态小组件

`sumpter-status.js` 只读取 Linux Admin 的登录、状态和运行事件，用于在 iPhone 查看代理和进行中请求。它不启动、停止、重载、修改配置或重置统计。

1. 将脚本复制到 Scriptable 新脚本。
2. 使用 HTTPS、VPN 或 Tailscale 的 Admin 地址，不要把 57879 明文暴露公网。
3. 填写管理员用户名和密码，运行一次完成登录。
4. 添加 `Sumpter Linux Status` 小组件，先用中号确认布局。

Cookie 和密码保存在 iOS Keychain。远程 Admin 必须保留 Cookie、正确转发 HTTPS 和 SSE；脚本不会替你配置反向代理。
