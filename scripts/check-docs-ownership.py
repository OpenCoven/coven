#!/usr/bin/env python3
from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parents[1]
PUBLIC_DOC_DIRS = (
    "docs/install/",
    "docs/platforms/",
    "docs/start/",
    "docs/help/",
    "docs/harnesses/",
    "docs/models/",
    "docs/memory/",
    "docs/guides/",
    "docs/reference/",
    "docs/daemon/",
)
CANONICAL_PREFIX = "https://docs.opencoven.ai/docs/"
SOURCE_ADJACENT_FIELD = "source_adjacent_reason:"
MAX_POINTER_LINES = 25


def is_public_doc(path: str) -> bool:
    normalized = path.replace("\\", "/")
    return normalized.endswith(".md") and normalized.startswith(PUBLIC_DOC_DIRS)


def validate_page(path: str, content: str) -> list[str]:
    if not is_public_doc(path):
        return []

    errors = []
    line_count = len(content.splitlines())
    if CANONICAL_PREFIX in content and line_count <= MAX_POINTER_LINES:
        return []

    source_reason = next(
        (
            line.partition(":")[2].strip().strip("\"'")
            for line in content.splitlines()
            if line.strip().lower().startswith(SOURCE_ADJACENT_FIELD)
        ),
        "",
    )
    if source_reason:
        return []

    errors.append(
        f"{path}: public docs must be a short canonical pointer "
        f"(at most {MAX_POINTER_LINES} lines with {CANONICAL_PREFIX}) or declare "
        "a non-empty source_adjacent_reason in frontmatter"
    )
    return errors


def changed_paths(revision_range: str) -> list[str]:
    result = subprocess.run(
        [
            "git",
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            revision_range,
            "--",
            "docs",
        ],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return [line for line in result.stdout.splitlines() if line]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Enforce canonical-pointer or source-adjacent ownership for changed public docs."
    )
    parser.add_argument(
        "--range",
        required=True,
        dest="revision_range",
        help="Git revision range to inspect, for example BASE...HEAD.",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    errors = []
    for path in changed_paths(args.revision_range):
        if not is_public_doc(path):
            continue
        full_path = ROOT / path
        if not full_path.is_file():
            continue
        errors.extend(validate_page(path, full_path.read_text(encoding="utf-8")))

    if errors:
        print("Documentation ownership check failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
