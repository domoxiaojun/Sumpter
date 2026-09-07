#!/usr/bin/env python3
"""Render the canonical onboarding block into both user-facing USAGE files.

Run with ``uv run scripts/sync-usage-docs.py --write`` when the template or
path matrix changes.  ``--check`` is the read-only CI/preflight mode.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
TEMPLATE = ROOT / "docs" / "usage-onboarding.md"
MATRIX = ROOT / "docs" / "usage-path-matrix.json"
START = "<!-- BEGIN SUMPTER_CANONICAL_ONBOARDING -->"
END = "<!-- END SUMPTER_CANONICAL_ONBOARDING -->"


def render(kind: str) -> str:
    template = TEMPLATE.read_text(encoding="utf-8").strip()
    values = json.loads(MATRIX.read_text(encoding="utf-8"))[kind]
    for key, value in values.items():
        template = template.replace("{{" + key + "}}", value)
    if "{{" in template or "}}" in template:
        raise ValueError(f"未解析的路径占位符: {kind}")
    return f"{START}\n\n{template}\n\n{END}"


def with_block(document: str, block: str) -> str:
    start = document.find(START)
    end = document.find(END)
    if start >= 0 or end >= 0:
        if start < 0 or end < 0 or end < start:
            raise ValueError("USAGE 文档的 canonical onboarding 标记不完整")
        end += len(END)
        return document[:start] + block + "\n\n" + document[end:].lstrip("\n")

    anchor = "\n两端当前使用 **schema v7**"
    position = document.find(anchor)
    if position < 0:
        raise ValueError("找不到 USAGE 文档插入点")
    return document[:position] + "\n\n" + block + "\n\n" + document[position:].lstrip("\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--write", action="store_true", help="写入两个 USAGE.md")
    mode.add_argument("--check", action="store_true", help="只检查两个 USAGE.md（默认）")
    args = parser.parse_args()

    targets = [(ROOT / "USAGE.md", "root"), (ROOT / "platforms" / "linux" / "USAGE.md", "linux")]
    changed = False
    for path, kind in targets:
        current = path.read_text(encoding="utf-8")
        expected = with_block(current, render(kind))
        if expected != current:
            changed = True
            if args.write:
                path.write_text(expected, encoding="utf-8")
                print(f"已更新 {path.relative_to(ROOT)}")
            else:
                print(f"需要同步 {path.relative_to(ROOT)}")
    if changed and not args.write:
        print("USAGE 文档与 canonical 模板不一致；运行 uv run scripts/sync-usage-docs.py --write")
        return 1
    print("USAGE canonical onboarding 同步完成" if args.write else "USAGE canonical onboarding 检查通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
