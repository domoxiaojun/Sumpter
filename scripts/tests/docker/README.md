# docker 部署契约测试

用 Go 依赖里的**官方实现**验证 Docker / Compose 部署输入，而不是重新实现近似规则：

- `github.com/moby/patternmatcher`：Docker 自身使用的 `.dockerignore` 匹配器（含 `MatchesOrParentMatches` 与父路径状态两种 API），断言两条构建上下文的放行/排除清单。
- `github.com/compose-spec/compose-go/v2`：Compose 官方解析器，断言独立目录部署、`.env` 覆盖与透传、宿主机端口与容器内端口的一致性、数据目录挂载、环境变量清单完整性，以及源码构建 override 与 GHCR 镜像名隔离。
- `init` 服务的内联 shell 在临时目录里真实执行，覆盖首次初始化、幂等不覆盖、权限、符号链接/目录/仅剩 `keys.json` 的拒绝路径，并交给 `shellcheck` 检查。

运行：

```bash
./scripts/check.sh docker
# 等价于
(cd scripts/tests/docker && go test ./...)
```

需要 Go 工具链；本机没有 Docker 也能运行，但**不能**替代真实镜像构建、容器启动与流量验证——那些在 Linux CI 和发布工作流执行。

新增部署参数时，先在 `compose.yaml` / `.env.example` 落地，再在这里补断言；`TestEnvInventoryStaysComplete` 会强制模板引用的每个变量都出现在示例与文档里，并阻止把 `HTTP_PROXY` 等不生效的变量当可用参数写回。
