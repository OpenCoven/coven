#!/usr/bin/env python3
from __future__ import annotations

import argparse
from datetime import datetime
from pathlib import Path


def main() -> int:
    parser = argparse.ArgumentParser(description="Append a timeline snippet to MEMORY.md with timestamp heading")
    parser.add_argument("--workspace", required=True, help="Workspace root path")
    parser.add_argument("--snippet", required=True, help="Snippet text to promote")
    args = parser.parse_args()

    workspace = Path(args.workspace).expanduser().resolve()
    memory_file = workspace / "MEMORY.md"
    snippet = args.snippet.strip()
    if not snippet:
        raise SystemExit("Snippet is empty")

    heading = datetime.now().strftime("## Promoted from Timeline (%Y-%m-%d %H:%M:%S)")
    block = f"\n\n{heading}\n\n{snippet}\n"

    if not memory_file.exists():
        memory_file.write_text("# MEMORY.md\n", encoding="utf-8")

    current = memory_file.read_text(encoding="utf-8")
    memory_file.write_text(current.rstrip() + block, encoding="utf-8")
    print(str(memory_file))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
