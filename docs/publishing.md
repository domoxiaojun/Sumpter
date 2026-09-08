# 仓库公开流程

本流程用于首次公开源码或迁移到新仓库。产品版本发版另见 [发布流程](releasing.md)。按阶段完成并记录提交 SHA、检查结果和外部设置，不能仅以目录整齐或本地编译成功认定公开完成。

## 1. 明确发布范围

- 确认仓库名称、所有者、公开可见性和是否保留原 Git 历史。
- 审阅 `git status --short`：每个新增、修改和删除都应属于待发布范围。
- 根目录保留项目入口、许可证、贡献/安全政策、清单和配置示例；业务按 `crates/`、`adapters/`、`apps/`、`platforms/` 分层。
- 当前文档放在 `docs/`，生成输入在 `docs/templates/`，历史资料在 `docs/archive/` 和 `docs/upstream/`；平台资源副本必须通过同步检查。
- `plan.md` 和 `.claude/` 是本地工作资料。已跟踪文件不会因加入 `.gitignore` 自动退出版本管理；创建无历史源码快照时由导出工具排除，保留历史时须显式审阅索引。

## 2. 许可与敏感数据

核对根 LICENSE、依赖许可证和复制内容的归属。保留必要第三方声明，不能用根 MIT 声明替代上游授权。

示例必须使用禁用入口和合成地址。运行数据、令牌、证书私钥、构建缓存不得进入源码；不要输出疑似密钥原文。导出工具按路径排除常见危险文件，但不代替内容检查。

```bash
gitleaks dir --redact /absolute/path/to/source-candidate
```

若保留旧 Git 历史，另检查历史：`gitleaks git --redact`。发现已泄露凭据应先撤销或轮换，再处理历史。重新建立历史的操作见 [迁移指南](repository-migration.md)。

## 3. 在最终源码上验证

```bash
npm ci --prefix platforms/linux/webui
./scripts/check.sh all
node --test scripts/tests/*.test.mjs
shellcheck scripts/check.sh scripts/build-macos-dmg.sh
actionlint
```

检查现行文档链接与已修改平台安装脚本自测。Linux 和 macOS CI 都必须成功；macOS 本地通过不能证明 Linux 安装成功。多架构容器由 Release 工作流校验并推送 GHCR，不在普通 CI 里发布。

生成源码候选时执行 `node scripts/maintenance/export-source.mjs /absolute/path/to/source-candidate`。在候选目录重新运行文档检查并核对旁置 SHA-256 清单。候选生成后继续修改源仓库会使其过时，应创建新候选，不覆盖旧目录。

## 4. 仓库设置与公开

创建仓库或推送前确认目标 URL、可见性、历史方案与最终提交范围。配置默认分支、Actions 权限、私密漏洞报告、分支保护及必需检查；参照 [迁移指南](repository-migration.md) 设置更新密钥与版本编号。

发布源码后核对远端 SHA、默认分支、匿名访问和 CI 结果。若提供二进制版本，继续执行发布流程，核对 Release、校验和、签名及安装验收。

## 完成记录

记录候选路径、最终提交、目标 URL、实际运行的检查、许可审阅、脱敏扫描、远端验证及未完成事项。未运行的检查不能沿用旧计数，不能将源码公开与安装、发布版本或部署混为一谈。
