# Docker / Compose 契约测试

这些测试验证部署输入，不启动 Docker 容器。需要 Go 工具链：

```bash
./scripts/check.sh docker
```

测试使用 Compose 官方解析器和 Docker 的 pattern matcher，覆盖：

- 根源码构建上下文与 Linux runtime 上下文的 `.dockerignore` 隔离。
- `compose.yaml` 在没有 `.env` 时仍能解析，默认 bridge 网络、回环端口、共享 `/config` 挂载和 `init` 成功依赖。
- `compose.build.example.yaml` 使用独立的 `sumpter:local` 镜像名，不覆盖官方 GHCR 镜像。
- `init` 首次创建、幂等运行、权限、符号链接 / 目录拒绝和旧 `keys.json` 路径。
- Dockerfile 自带健康探针，Compose 不重复声明；版本、tzdata 和部署文档保持一致。

这不能替代真实镜像构建、双架构 manifest、容器启动、登录或模型请求。真实容器由 Linux CI 和发布工作流验证；部署步骤见 [Compose 教程](../../../platforms/linux/DOCKER.md)。
