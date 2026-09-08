# 安全政策

## 支持范围

安全修复以最新稳定版本和当前 `main` 为目标，旧版本不承诺持续回补。尚未发布的修复会先进入主分支，再通过正式版本交付。

## 报告漏洞

请使用仓库 **Security → Advisories → Report a vulnerability** 私下提交复现步骤、影响版本和脱敏证据。维护者需要在新仓库启用 Private vulnerability reporting。

如果该入口未启用，只提交不包含漏洞细节或敏感信息的 Issue，请维护者提供私密联系渠道。不要在公开 Issue 附上可直接利用的细节、真实请求体、访问令牌或运行数据库。当前没有固定响应时间承诺。

## 数据处理

真实 `apiKey`、`authToken`、Admin 密码、Cookie、抓包与运行数据库留在仓库外。示例入口必须禁用并使用 `.invalid` 主机；配置权限使用 `0600`。分享诊断前检查脱敏内容。

默认代理与管理监听使用 loopback。远程 Admin 应使用 HTTPS 反代与访问限制；不要把无保护的管理端口直接暴露到公网。入站 API token、Linux Admin Cookie/CSRF 和 macOS control token 是不同的鉴权边界。

Sparkle 私钥只放系统钥匙串或 GitHub Actions Secret。新仓库的更新签名、Apple 签名与公证、GitHub 发布权限分别配置，详见 [发布指南](docs/releasing.md)。
