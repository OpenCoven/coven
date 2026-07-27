#!/usr/bin/env python3
"""Fail-closed privacy guard for new Coven changes.

The classic secret scanner remains responsible for the full tree and history.
This guard scans staged or PR-changed files for Coven-specific identifiers and
local paths without printing matched values.
"""
from __future__ import annotations

import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
PNPM_SHA512_INTEGRITY_DIGEST = re.compile(
    r"(?P<prefix>(?:^\s*|[{,]\s*)integrity:\s*)"
    r"sha512-[A-Za-z0-9+/]{86}=="
    r"(?=$|[\s},#])"
)
RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    (
        "coven_session_key",
        re.compile(
            r"agent:[a-z0-9_-]+:(?:telegram|imessage|discord|whatsapp|signal|webchat):"
            r"[a-z]+:[^\s\"']+"
        ),
    ),
    (
        "messenger_chat_id",
        re.compile(r"(?:telegram|imessage|discord|whatsapp|signal):(?:direct:)?\d{6,}"),
    ),
    (
        "absolute_home_path",
        re.compile(r"(?:/Users/|/home/)[a-z0-9._-]+/", re.IGNORECASE),
    ),
    (
        "runtime_internal_path",
        re.compile(
            r"~/\.(?:openclaw|coven)/(?:agents|workspaces|credentials|sessions)"
            r"[^\s\"']*"
        ),
    ),
    ("phone_number", re.compile(r"(?<!\d)\+[1-9]\d{1,14}(?!\d)")),
    (
        "invite_or_handoff_url",
        re.compile(r"https?://[^\s\"']*(?:invite|handoff|ts\.net)[^\s\"']*token[^\s\"']*"),
    ),
)


def scannable_line(line: str, is_pnpm_lock: bool) -> str:
    if not is_pnpm_lock:
        return line
    return PNPM_SHA512_INTEGRITY_DIGEST.sub(
        r"\g<prefix><integrity-digest>",
        line,
    )


def scan_text(text: str, path: str) -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    is_pnpm_lock = pathlib.PurePath(path).name == "pnpm-lock.yaml"
    for line_number, line in enumerate(text.splitlines(), 1):
        line = scannable_line(line, is_pnpm_lock)
        session_key_hit = False
        for name, pattern in RULES:
            if name == "messenger_chat_id" and session_key_hit:
                continue
            if pattern.search(line):
                hits.append((path, line_number, name))
                session_key_hit = session_key_hit or name == "coven_session_key"
    return hits


def git(*args: str, text: bool = True) -> str | bytes:
    return subprocess.check_output(
        ["git", *args],
        cwd=ROOT,
        text=text,
    )


def nul_paths(data: bytes) -> list[str]:
    return [
        value.decode("utf-8", errors="surrogateescape")
        for value in data.split(b"\0")
        if value
    ]


def staged_files() -> list[tuple[str, bytes]]:
    names = nul_paths(
        git("diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z", text=False)
    )
    return [(name, git("show", f":{name}", text=False)) for name in names]


def changed_files(revision_range: str) -> list[tuple[str, bytes]]:
    if "..." in revision_range:
        end_rev = revision_range.rsplit("...", 1)[1]
    elif ".." in revision_range:
        end_rev = revision_range.rsplit("..", 1)[1]
    else:
        raise ValueError("--range requires START..END or START...END")
    end_rev = end_rev.strip() or "HEAD"

    names = nul_paths(
        git(
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            "-z",
            revision_range,
            text=False,
        )
    )
    return [(name, git("show", f"{end_rev}:{name}", text=False)) for name in names]


def working_files(names: list[str]) -> list[tuple[str, bytes]]:
    files: list[tuple[str, bytes]] = []
    for name in names:
        path = ROOT / name
        if not path.is_file():
            raise ValueError(f"--files path is missing or not a regular file: {name}")
        files.append((name, path.read_bytes()))
    return files


def scan_files(files: list[tuple[str, bytes]]) -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    for path, data in files:
        text = data.decode("utf-8", errors="surrogateescape")
        hits.extend(scan_text(text, path))
    return hits


def usage() -> int:
    print(
        "usage: check-coven-privacy.py --staged | "
        "--range START..END|START...END | --files PATH...",
        file=sys.stderr,
    )
    return 2


def main(argv: list[str]) -> int:
    try:
        if argv == ["--staged"]:
            files = staged_files()
            mode = "staged"
        elif len(argv) == 2 and argv[0] == "--range":
            files = changed_files(argv[1])
            mode = f"range {argv[1]}"
        elif len(argv) >= 2 and argv[0] == "--files":
            files = working_files(argv[1:])
            mode = "explicit files"
        else:
            return usage()
    except ValueError as error:
        print(f"Coven privacy guard failed: {error}", file=sys.stderr)
        return 2

    hits = scan_files(files)
    if hits:
        print("Coven privacy guard blocked publishable private data:", file=sys.stderr)
        for path, line, rule in hits:
            print(f"{path}:{line}:{rule}", file=sys.stderr)
        return 1
    print(f"Coven privacy guard passed ({mode}, {len(files)} files).")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
