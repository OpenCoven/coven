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
EXCLUDED_PATHS = {"scripts/check-secrets.py", "scripts/check-secrets-test.py"}
LOCKFILE_NAMES = ("pnpm-lock.yaml", "package-lock.json", "yarn.lock")
LOCKFILE_PACKAGE_KEY = re.compile(r"^\s*(?:['\"]?/?@?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)?(?:@[A-Za-z0-9][^:'\"]*)?['\"]?)\s*:\s*(?:\{\})?\s*$")
LOCKFILE_NODE_MODULE_KEY = re.compile(r'''^\s*["']?node_modules/(?:@?[A-Za-z0-9_.-]+/)?[A-Za-z0-9_.-]+["']?\s*:\s*\{?\s*$''')
LOCKFILE_PACKAGE_VERSION_ENTRY = re.compile(r"^\s*['\"]?@?[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)?['\"]?\s*:\s*\d+\.\d+\.\d+(?:[-+][A-Za-z0-9_.-]+)?\s*$")
LOCKFILE_INTEGRITY_LINE = re.compile(r'''["']?\bintegrity\b["']?\s*:\s*["']?(?:sha256|sha384|sha512)-[A-Za-z0-9+/=]+["']?''')
LOCKFILE_RESOLVED_LINE = re.compile(r'''["']?\bresolved\b["']?\s*:\s*["']?https://registry\.npmjs\.org/[A-Za-z0-9_+/@.,~%:-]+\.tgz["']?''')
OPENCOVEN_GITHUB_URL = re.compile(r"https://github\.com/OpenCoven/coven/(?:blob|tree)/[A-Za-z0-9_./@%+-]+")
OPENCOVEN_LOCAL_WORKTREE = re.compile(
    r"/Users/[A-Za-z0-9_.-]+/Documents/GitHub/OpenCoven/coven/\.worktrees/[A-Za-z0-9_.-]+"
)
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
ENV_SECRET_READ = re.compile(
    r"(?i)\b(?:api[_-]?key|secret|token|password|private[_-]?key)\b\s*[:=]\s*"
    r"(?:os\.environ(?:\.get)?\(|std::env::var\(|env::var\(|process\.env\.)"
)


def sh(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True, stderr=subprocess.DEVNULL)


def entropy(value: str) -> float:
    if not value:
        return 0.0
    counts = collections.Counter(value)
    return -sum((count / len(value)) * math.log2(count / len(value)) for count in counts.values())


def is_lockfile_path(path: str) -> bool:
    normalized = path.replace("\\", "/")
    return any(normalized == lockfile or normalized.endswith(f"/{lockfile}") for lockfile in LOCKFILE_NAMES)


def is_excluded_path(path: str) -> bool:
    normalized = path.replace("\\", "/")
    return normalized in EXCLUDED_PATHS


def is_known_safe_lockfile_line(path: str, line: str) -> bool:
    if not is_lockfile_path(path):
        return False
    stripped = line.strip()
    return bool(
        LOCKFILE_INTEGRITY_LINE.search(stripped)
        or LOCKFILE_RESOLVED_LINE.search(stripped)
        or LOCKFILE_NODE_MODULE_KEY.match(stripped)
        or LOCKFILE_PACKAGE_KEY.match(stripped)
        or LOCKFILE_PACKAGE_VERSION_ENTRY.match(stripped)
    )


def is_local_path_like_token(token: str) -> bool:
    normalized = token.strip("/")
    parts = normalized.split("/")
    if len(parts) < 4:
        return False
    if parts[0] in {"Users", "home", "private", "var", "tmp", "Volumes"}:
        return True
    return ".worktrees" in parts or "worktrees" in parts


def is_public_repo_url_like_token(token: str) -> bool:
    normalized = token.strip("/")
    return normalized.startswith("github.com/OpenCoven/coven/") and (
        "/blob/" in normalized or "/tree/" in normalized
    )


def is_github_advisory_url_like_token(token: str) -> bool:
    normalized = token.strip("/")
    return normalized.startswith("github.com/advisories/GHSA-")


_GITHUB_ACTION_SHA_REF = re.compile(
    r"^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+@[0-9a-f]{40}$"
)


def is_github_action_sha_ref_token(token: str) -> bool:
    """Whether `token` is a GitHub Actions `uses:` reference pinned to a 40-char
    commit SHA, e.g. ``actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd``.
    SHA-pinned refs are an OpenSSF best practice (they prevent action authors
    from silently moving a version tag onto a malicious commit) but the trailing
    40-hex SHA otherwise pushes the workflow line over the entropy threshold.
    """
    return bool(_GITHUB_ACTION_SHA_REF.match(token))


def is_known_fake_private_key_fixture(line: str) -> bool:
    return (
        "-----BEGIN PRIVATE KEY-----" in line
        and "\\nfakefakefake" in line
        and "\\n-----END PRIVATE KEY-----" in line
    )


def is_programming_identifier_token(token: str) -> bool:
    """Whether `token` looks like a snake_case / SCREAMING_SNAKE_CASE identifier
    (optionally suffixed with a `.method` call), a workflow-style relative file
    path (e.g. `github/workflows/release-npm.yml`), or a Rust target triple
    (`target/x86_64-pc-windows-msvc/release`). None of these shapes are ever a
    credential.

    The guard keeps the rest of the high-entropy path strict by requiring the
    token to be composed only of `[A-Za-z0-9_./-]`, to be split into at least
    three segments by `_`/`.`/`/`/`-`, for at least one segment to contain
    letters (so a token of pure-digit segments still trips the entropy rule),
    and for every letter-bearing segment to be uniformly single-case while
    allowing digits inside those identifier/path/triple segments. Real API tokens
    lack separators, mix case within a segment, or contain base64-only characters
    such as `+`/`=`, so they continue to fail at least one of these checks and
    trip the entropy rule as before.
    """
    if not re.fullmatch(r"[A-Za-z0-9_./-]+", token):
        return False
    segments = [seg for seg in re.split(r"[._/-]", token) if seg]
    if len(segments) < 3:
        return False
    has_letter_segment = False
    for seg in segments:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9]*", seg):
            return False
        if len(seg) > 24:
            return False
        letters = "".join(ch for ch in seg if ch.isalpha())
        if letters:
            has_letter_segment = True
            if not (letters.islower() or letters.isupper()):
                return False
    return has_letter_segment


def scan_text(text: str, path: str) -> list[tuple[str, int, str]]:
    hits: list[tuple[str, int, str]] = []
    for line_number, line in enumerate(text.splitlines(), 1):
        allow = bool(ALLOW_LINE.search(line))
        for name, pattern in SECRET_RULES:
            if name == "private_key" and is_known_fake_private_key_fixture(line):
                continue
            if name == "generic_assignment" and ENV_SECRET_READ.search(line):
                continue
            if pattern.search(line) and not (allow and name != "private_key"):
                hits.append((path, line_number, name))
        if allow:
            continue
        if (
            OPENCOVEN_GITHUB_URL.search(line)
            or OPENCOVEN_LOCAL_WORKTREE.search(line)
        ):
            continue
        if is_known_safe_lockfile_line(path, line):
            continue
        for match in re.finditer(r"\b[A-Za-z0-9_+/@.-]{32,}\b", line):
            token = match.group(0)
            if re.fullmatch(r"[0-9a-f]{32,64}", token):
                continue
            if (
                is_local_path_like_token(token)
                or is_public_repo_url_like_token(token)
                or is_github_advisory_url_like_token(token)
                or is_github_action_sha_ref_token(token)
                or is_programming_identifier_token(token)
            ):
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
        if is_excluded_path(rel):
            continue
        path = ROOT / rel
        if any(part in EXCLUDED_PARTS for part in path.relative_to(ROOT).parts):
            continue
        if path.is_file():
            hits.extend(scan_bytes(path.read_bytes(), rel))
    return hits


def history_blob_hits(ref: str = "HEAD") -> list[tuple[str, str, int, str]]:
    rows = sh("git", "rev-list", "--objects", ref).splitlines()
    hits: list[tuple[str, str, int, str]] = []
    seen: set[str] = set()
    for row in rows:
        parts = row.split(" ", 1)
        sha = parts[0]
        rel = parts[1] if len(parts) > 1 else "<unknown>"
        if is_excluded_path(rel):
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
        if history:
            print(f"history findings: {len(history)} entries (details redacted)", file=sys.stderr)
        return 1
    print("Secret guard passed: no current-tree or history findings.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
