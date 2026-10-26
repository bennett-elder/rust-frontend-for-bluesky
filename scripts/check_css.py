#!/usr/bin/env python3
"""Cheap CSS sanity check.

A stray `}` or an orphaned declaration block makes the browser silently skip the
next rule, which is how a missing `display: flex` turns into a layout bug that
looks fine in the source. This catches that class of mistake in one second.

Usage: python3 scripts/check_css.py
"""

from __future__ import annotations

import pathlib
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
FILES = [ROOT / "app/assets/style.css"]


def check(path: pathlib.Path) -> list[str]:
    problems: list[str] = []
    depth = 0
    for number, line in enumerate(path.read_text().splitlines(), start=1):
        depth += line.count("{") - line.count("}")
        if depth < 0:
            problems.append(f"{path.name}:{number}: unexpected '}}' (depth went negative)")
            depth = 0
    if depth != 0:
        problems.append(f"{path.name}: unbalanced braces, final depth {depth}")

    # An orphaned declaration block: a line outside any rule that looks like a property.
    depth = 0
    for number, line in enumerate(path.read_text().splitlines(), start=1):
        stripped = line.strip()
        if depth == 0 and stripped.endswith(";") and ":" in stripped:
            problems.append(f"{path.name}:{number}: declaration outside a rule: {stripped}")
        depth += line.count("{") - line.count("}")
    return problems


def main() -> int:
    problems: list[str] = []
    for path in FILES:
        if path.exists():
            problems.extend(check(path))

    if problems:
        print("\n".join(problems))
        return 1

    print(f"css ok ({len([p for p in FILES if p.exists()])} files)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
