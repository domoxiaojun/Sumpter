# 使用手册模板

`usage-onboarding.md` 是根 `USAGE.md` 和 Linux 发布包 `USAGE.md` 的共同正文维护源。路径差异写在 `usage-path-matrix.json`；占位符由 `uv run scripts/maintenance/sync-usage-docs.py --write` 替换。

不要直接编辑两份生成手册的标记块。修改模板后运行同步，再执行 `./scripts/check.sh docs`。标记块外的标题、平台说明和链接属于各自文件，可单独维护。
