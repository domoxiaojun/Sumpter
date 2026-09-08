# 文档生成输入

这里存放维护输入，不是用户使用指南。

- `usage-onboarding.md`：两端共同的接入说明模板。
- `usage-path-matrix.json`：根仓库与 Linux 包的路径差异。

模板中的路径占位符由矩阵按根仓库和 Linux 发布包分别替换。工具只更新两份 USAGE 的 `BEGIN SUMPTER_CANONICAL_ONBOARDING` / `END SUMPTER_CANONICAL_ONBOARDING` 标记块；其余正文仍在各自文档维护。

修改后按 [开发指南](../development.md#单一维护源与生成副本) 执行同步和文档检查。链接检查针对生成后的 [根使用指南](../../USAGE.md) 与 [Linux 包内指南](../../platforms/linux/USAGE.md)，不直接检查含占位符的模板。
