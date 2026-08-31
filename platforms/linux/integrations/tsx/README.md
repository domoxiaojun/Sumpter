# Sumpter 进行中请求 TSX 脚本

`sumpter-inflight.tsx` 使用 TypeScript 读取 Linux Admin API，只输出：

- proxy 是否运行、版本、在线时长和入口数量
- `kind=client` 且 `phase=inFlight` 的请求
- 每个请求的模型、入口、请求用途和等待时长

## 单文件运行

只需要 Node.js 18+ 和 `tsx` 运行器。直接编辑 `sumpter-inflight.tsx` 顶部的 `CONFIG`，然后运行：

```bash
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx
```

也可以使用环境变量覆盖单文件配置（更适合避免密码落盘）：

```bash
SUMPTER_ADMIN_URL='https://admin.example.com' \
SUMPTER_ADMIN_USERNAME='kkl' \
SUMPTER_ADMIN_PASSWORD='从安全凭据管理器注入' \
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx
```

加 `--json` 可输出机器可读 JSON：

```bash
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx --json
```

脚本只调用登录、状态和运行事件 GET 接口，不会启停代理、修改配置或重置统计。地址支持
`http://` 和 `https://`；HTTP 会明文传输密码和 Cookie，只适合可信局域网、VPN 或 Tailscale，
不要把 Linux 的 57879 端口直接暴露到公网。
