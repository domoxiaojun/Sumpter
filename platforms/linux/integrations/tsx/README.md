# TSX 进行中请求脚本

`sumpter-inflight.tsx` 读取 Admin 登录、状态和 `inFlight` 客户端事件，只输出代理状态、版本、在线时长、入口数量以及模型、入口、用途和等待时长。

需要 Node.js 18+ 和 `tsx`：

```bash
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx --json
```

可用环境变量注入连接信息，避免把密码写入脚本：

```bash
SUMPTER_ADMIN_URL='https://admin.example.com' \
SUMPTER_ADMIN_USERNAME='kkl' \
SUMPTER_ADMIN_PASSWORD='从密码管理器读取' \
npx tsx platforms/linux/integrations/tsx/sumpter-inflight.tsx
```

脚本只调用 GET 和登录接口，不会改变配置或服务状态。HTTP 地址会明文传输凭据，仅适合可信网络。
