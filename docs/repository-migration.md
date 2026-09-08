# 新仓库迁移

目标是用当前整理后的源码创建新仓库，重新建立提交历史。当前仓库、Git 历史与远端保持原状，由维护者决定何时删除旧远端。

## 为什么 Claude 会显示在 Contributors

本次盘点发现旧提交 `32c67dd`、`8d53d39` 正文包含 Claude 的 `Co-Authored-By`，主作者都是 domoxiaojun。GitHub 根据提交作者及共同作者生成 Contributors；修改 README 或删除 CLAUDE.md 不会清除历史署名。

新仓库只导入源码并建立全新首提交，不推旧 `.git`、分支、tag 或 mirror。以后按贡献指南使用实际作者，不再自动追加 AI 署名。产品中的 Claude Code 支持不受影响。GitHub 展示可能有缓存，首次推送后核对新仓库提交与贡献者页面。

## 1. 准备源码快照

在当前仓库根执行，目标必须在仓库外且不存在：

```bash
node scripts/maintenance/export-source.mjs /absolute/path/to/sumpter-new
```

工具读取当前工作区中 Git 跟踪的文件和未忽略的新文件，包含尚未提交的整理改动；跳过已删除文件、plan.md、根 SOURCE_COMMIT 和 .claude/，保留目标也在快照内的相对符号链接，拒绝外部链接、敏感配置、运行库与构建缓存。不会复制 `.git`，不会提交或创建远端。文件 SHA-256 清单（符号链接校验链接文本）写在目标目录旁边的 `<目标名>.manifest.json`，不随源码进入新仓库。

导出前确认 `git status --short` 的范围；工具的路径检查不是内容级密钥扫描。迁移前对导出目录运行 `gitleaks dir --redact <目录>` 并人工检查待提交文件，勿提交真实配置或诊断数据。

## 2. 建立全新历史

在导出目录执行：

```bash
git init -b main
git config user.name domoxiaojun
git config user.email 48934312+domoxiaojun@users.noreply.github.com
./scripts/check.sh docs
git add .
git diff --cached --stat
git commit -m "chore(repo): initialize Sumpter multiplatform project"
git log --format=fuller -1
```

确认只有预期首提交、没有 AI 共同作者。保留 MIT 和所需第三方版权声明。若存在真正人类共同贡献者，按事实保留署名与授权记录。

在 GitHub 创建空仓库后添加其实际 URL，再 push main。不要导入旧仓库历史，也不要把原目录的 origin 直接当成新仓库从旧分支推送。当前源码身份仍是 `domoxiaojun/sumpter`；如改名，先定位引用：

```bash
rg -n 'domoxiaojun/sumpter|ghcr.io/|sf.domob.org' Cargo.toml README.md docs platforms .github
```

按含义更新 repository/homepage/bugs、镜像、feed 与安装器地址；历史快照不做机械替换。

## 3. 恢复仓库设置

这些设置不会随源码迁移：

- 默认分支 main，允许 Actions；启用 Security 的私密漏洞报告。
- 保护 main：PR 合并、禁止删除和 force push，必需检查选择新 CI 实际生成的 Rust 1.88、WebUI、macOS App、Repository contracts。单人项目可不要求第二人批准。
- 配置 Sparkle 两个 Secrets；需要 Apple 签名时另配置证书环境。Secret 只通过安全渠道重新设置，不能从旧仓库读出明文。
- 保留已有 Sparkle 更新 key，核对旧 feed 地址和 build number；设置 `MACOS_BUILD_NUMBER_BASE` 防止新仓库计数回退。
- 核对 GHCR package 权限、静态镜像与旧下载地址，按 [发布指南](releasing.md) 做首发验收。

先确认本地历史备份、源码快照和外部分发迁移准备好，再由维护者删除旧仓库。旧 Release、Issue、Actions 设置与统计不属于源码快照；需要保留的项目资料应另行备份。
