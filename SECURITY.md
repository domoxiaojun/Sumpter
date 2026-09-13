# 安全政策

## 报告漏洞

请使用 GitHub 仓库的 **Security → Advisories → Report a vulnerability** 私下提交影响版本、复现步骤和脱敏证据。不要在公开 Issue 附上可利用细节、真实请求体、令牌、密码、Cookie、数据库或诊断捕获。

如果私密报告入口不可用，先提交不含漏洞细节的 Issue，请维护者提供安全联系渠道。当前没有固定响应时间承诺；修复目标是最新稳定版本和 `main`。

## 凭据和部署

- `endpoints[].apiKey`、`listener.authToken`、Linux Admin 密码、macOS control token 分属不同边界，不要混用。
- 真实配置、数据库、日志、Cookie、诊断捕获和 Sparkle 私钥放在仓库外；示例入口必须停用并使用 `.invalid` 域名。
- 配置目录使用 0700，配置与密码文件使用 0600。
- 默认 listener 绑定 loopback。远程 Admin 使用 HTTPS 反代、VPN 或 SSH 隧道，不直接暴露无保护的 57879。
- 诊断捕获默认关闭，可能包含原始 Header、Body 和上游响应；分享前逐字段脱敏。

安全修复、签名、公证、发布和生产部署分别验证。安全问题不要通过普通 PR 公开提交。
