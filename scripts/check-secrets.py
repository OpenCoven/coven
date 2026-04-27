#!/usr/bin/env python3
"""Small repo-local secret guard for public-release checks.

The scanner intentionally prints rule names and file locations only. It never
prints matching values.
"""
from __future__ import annotations

import collections
import math
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
EXCLUDED_PARTS = {".git", "target", "node_modules", ".coven", ".comux", ".comux-hooks"}
EXCLUDED_PATHS = {"scripts/check-secrets.py"}
SECRET_RULES: list[tuple[str, re.Pattern[str]]] = [
    ("private_key", re.compile(r"-----BEGIN (?:RSA |DSA |EC |OPENSSH |PGP )?PRIVATE KEY-----")),
    ("aws_access_key", re.compile(r"AKIA[0-9A-Z]{16}")),
    ("github_token", re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}")),
    ("openai_key", re.compile(r"sk-[A-Za-z0-9]{32,}")),
    ("anthropic_key", re.compile(r"sk-ant-[A-Za-z0-9_-]{20,}")),
    ("slack_token", re.compile(r"xox[baprs]-[A-Za-z0-9-]{20,}")),
    (
        "generic_assignment",
        re.compile(
            r"(?i)\b(api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*[\"']?[^\"'\s]{12,}"
        ),
    ),
]
ALLOW_LINE = re.compile(
    r"(?i)(example|placeholder|your_|<.*>|op://|secret scanning|secret guard|missing|expected|description|readme|docs/|abcdefghijklmnopqrstuvwxyz|custom-coven-home)"
)


def sh(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True, stderr=subprocess.DEVNULL)


def entropy(value: str) -> float:
    if not value:
        return 0.0
    counts = collections.Counter(value)
    return -sum((count / len(value)) * math.log2(count / len(value)) for count in counts.values())


def scan_text(text: str, path: str) -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    for line_number, line in enumerate(text.splitlines(), 1):
        allow = bool(ALLOW_LINE.search(line))
        for name, pattern in SECRET_RULES:
            if pattern.search(line) and not (allow and name != "private_key"):
                hits.append((path, line_number, name))
        if allow:
            continue
        for match in re.finditer(r"\b[A-Za-z0-9_+/@.-]{32,}\b", line):
            token = match.group(0)
            if re.fullmatch(r"[0-9a-f]{32,64}", token):
                continue
            if entropy(token) >= 4.3:
                hits.append((path, line_number, "high_entropy"))
    return hits


def scan_bytes(data: bytes, path: str) -> list[tuple[str, int, str]]:
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return []
    return scan_text(text, path)


def tracked_file_hits() -> list[tuple[str, int, str]]:
    files = sh("git", "ls-files").splitlines()
    hits: list[tuple[str, int, str]] = []
    for rel in files:
        if rel in EXCLUDED_PATHS:
            continue
        path = ROOT / rel
        if any(part in EXCLUDED_PARTS for part in path.relative_to(ROOT).parts):
            continue
        if path.is_file():
            hits.extend(scan_bytes(path.read_bytes(), rel))
    return hits


def history_blob_hits() -> list[tuple[str, str, int, str]]:
    rows = sh("git", "rev-list", "--objects", "--all").splitlines()
    hits: list[tuple[str, str, int, str]] = []
    seen: set[str] = set()
    for row in rows:
        parts = row.split(" ", 1)
        sha = parts[0]
        rel = parts[1] if len(parts) > 1 else "<unknown>"
        if rel in EXCLUDED_PATHS:
            continue
        if sha in seen:
            continue
        seen.add(sha)
        if any(part in EXCLUDED_PARTS for part in pathlib.PurePosixPath(rel).parts):
            continue
        if sh("git", "cat-file", "-t", sha).strip() != "blob":
            continue
        data = subprocess.check_output(["git", "cat-file", "-p", sha], cwd=ROOT)
        for path, line, rule in scan_bytes(data, rel):
            hits.append((sha[:12], path, line, rule))
    return hits


def main() -> int:
    current = tracked_file_hits()
    history = history_blob_hits()
    if current or history:
        print("Secret guard found possible sensitive values. Values are intentionally redacted.", file=sys.stderr)
        for path, line, rule in current:
            print(f"current:{path}:{line}:{rule}", file=sys.stderr)
        for sha, path, line, rule in history:
            print(f"history:{sha}:{path}:{line}:{rule}", file=sys.stderr)
        return 1
    print("Secret guard passed: no current-tree or history findings.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
