#!/usr/bin/env python3
"""Durable GitHub delivery checkpoints: validate, render, audit and reconcile.

A checkpoint is the only state a later session (another worktree, another VM,
another harness) is allowed to resume from. It lives in one fenced
```checkpoint block inside a GitHub issue comment. This tool never writes to
GitHub, git or the claim registry: `gather` only reads (REST and local git),
every other command is offline.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
from typing import Callable, Iterable

REQUIRED = ("ISSUE", "OWNER", "ENVIRONMENT", "BRANCH", "FULL_SHA", "PR",
            "COMPLETED", "CHECKS", "DECISIONS", "BLOCKERS", "NEXT")
OPTIONAL = ("FILES",)
KEYS = REQUIRED[:6] + OPTIONAL + REQUIRED[6:]
# Fields where "none" is a real answer; everywhere else it hides missing state.
NONE_ALLOWED = {"PR", "COMPLETED", "DECISIONS", "BLOCKERS", "FILES"}
DETACHED = "(detached HEAD)"

FENCE = re.compile(r"^```checkpoint[ \t]*\n(.*?)^```[ \t]*$", re.M | re.S)
KEY_LINE = re.compile(r"^([A-Z][A-Z_]*):(?:[ \t]+(.*))?$")
ISSUE_REF = re.compile(r"^(?:[\w.-]+/[\w.-]+)?#\d+(?!\d)")
FULL_SHA = re.compile(r"^[0-9a-f]{40}$")
PR_REF = re.compile(r"^(?:none\b|#\d+(?!\d)|https://github\.com/[\w.-]+/[\w.-]+/pull/\d+(?!\d))", re.I)
BRANCH = re.compile(r"^(?!-)(?!.*\.\.)(?!.*//)[A-Za-z0-9._/-]+(?<![/.])$")
# Values a checkpoint must never carry. Kept small and literal; the repository
# secret scan stays authoritative for committed files.
LEAKS = (
    ("github token", re.compile(r"gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,}")),
    ("api key", re.compile(r"sk-(?:ant-)?[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|xox[baprs]-[A-Za-z0-9-]{20,}")),
    ("private key", re.compile(r"-----BEGIN [A-Z ]*PRIVATE KEY-----")),
    ("home directory path", re.compile(r"(?:/Users/|/home/)[a-z0-9._-]+/", re.I)),
)


class Checkpoint(dict):
    """Field name -> value, in canonical order."""


def blocks(text: str) -> list[str]:
    return [match.group(1) for match in FENCE.finditer(text)]


def parse(text: str) -> tuple[Checkpoint | None, list[str]]:
    found = blocks(text)
    if not found:
        return None, ["no ```checkpoint block found"]
    if len(found) > 1:
        return None, [f"{len(found)} ```checkpoint blocks found; post exactly one per comment"]
    fields: dict[str, list[str]] = {}
    errors: list[str] = []
    current = None
    for number, line in enumerate(found[0].splitlines(), 1):
        if not line.strip():
            continue
        if line[0] in " \t":
            if current is None:
                errors.append(f"line {number}: continuation before any field")
            else:
                fields[current].append(line.strip())
            continue
        match = KEY_LINE.match(line)
        if not match:
            errors.append(f"line {number}: expected FIELD: value, got {line[:40]!r}")
            current = None
            continue
        key, value = match.group(1), (match.group(2) or "").strip()
        if key not in KEYS:
            errors.append(f"line {number}: unknown field {key}")
            current = None
            continue
        if key in fields:
            errors.append(f"line {number}: duplicate field {key}")
            current = None
            continue
        fields[key] = [value] if value else []
        current = key
    checkpoint = Checkpoint((key, "\n".join(fields[key])) for key in KEYS if key in fields)
    return checkpoint, errors


def validate(text: str, head: str | None = None) -> tuple[Checkpoint | None, list[str], list[str]]:
    """Return (checkpoint, errors, stale). Errors make the checkpoint unusable."""
    checkpoint, errors = parse(text)
    if checkpoint is None:
        return None, errors, []
    for key in REQUIRED:
        if key not in checkpoint:
            errors.append(f"missing required field {key}")
        elif not checkpoint[key]:
            errors.append(f"{key} is empty")
        elif checkpoint[key].lower() == "none" and key not in NONE_ALLOWED:
            errors.append(f"{key} cannot be 'none'")
    value = checkpoint.get
    if value("ISSUE") and not ISSUE_REF.match(value("ISSUE")):
        errors.append("ISSUE must start with an issue reference such as #1451")
    sha = value("FULL_SHA")
    if sha and not FULL_SHA.match(sha):
        if re.fullmatch(r"[0-9a-fA-F]{4,39}", sha):
            errors.append(f"FULL_SHA is a {len(sha)}-character short SHA; record the full 40-character commit")
        else:
            errors.append("FULL_SHA must be exactly 40 lowercase hex characters")
    branch = value("BRANCH")
    if branch and branch != DETACHED and not BRANCH.match(branch):
        errors.append(f"BRANCH must be a single branch name or {DETACHED!r}")
    if value("PR") and not PR_REF.match(value("PR")):
        errors.append("PR must be none, #<number> or a pull request URL")
    body = "\n".join(checkpoint.values())
    for label, pattern in LEAKS:
        if pattern.search(body):
            errors.append(f"checkpoint contains a {label}; remove it before posting")
    stale = []
    if head and sha and FULL_SHA.match(sha) and head != sha:
        stale.append(f"stale: checkpoint FULL_SHA {sha} but the live head is {head}; "
                     "read the new commits before continuing")
    return checkpoint, errors, stale


def render(fields: dict[str, str]) -> str:
    lines = ["```checkpoint"]
    for key in KEYS:
        if key not in fields:
            continue
        first, *rest = (fields[key] or "").splitlines() or [""]
        lines.append(f"{key}: {first}".rstrip())
        lines.extend(f"  {line}" for line in rest)
    lines.append("```")
    return "\n".join(lines) + "\n"


AUDIT_ALIASES = {"FULL_SHA": ("FULL_HEAD_SHA",), "CHECKS": ("CHECKS_WITH_EXACT_RESULTS",),
                 "NEXT": ("NEXT_ACTIONS",)}
# Free-form checkpoints also write "ISSUE #1450 · OWNER x"; only these
# identity fields are recognized without a colon, and PR only before a value.
BARE = {"ISSUE", "OWNER", "ENVIRONMENT", "BRANCH", "FULL_SHA"}


def audit(text: str) -> dict:
    """Report which fields a free-form (pre-schema) checkpoint names."""
    present = []
    for key in REQUIRED:
        for name in (key,) + AUDIT_ALIASES.get(key, ()):
            labelled = re.search(rf"\b{name}\b\**\s*[:=]", text, re.I)
            bare = key in BARE and re.search(rf"\b{name}\b\**\s+\S", text)
            pr = key == "PR" and re.search(r"\bPR\b\**\s+(?:#\d|https://github\.com/|none\b)", text)
            if labelled or bare or pr:
                present.append(key)
                break
    return {
        "present": present,
        "missing": [key for key in REQUIRED if key not in present],
        "full_sha_values": sorted(set(re.findall(r"(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])", text))),
        "canonical_block": bool(blocks(text)),
    }


def mentions(text: str | None, issue: int) -> bool:
    return bool(text) and re.search(rf"(?<![\d.]){issue}(?!\d)", text) is not None


def decide(state: dict, now: dt.datetime | None = None, fresh_hours: float = 2.0) -> list[dict]:
    """Pre-edit conflict findings from read-only state. Pure; see `gather`."""
    issue = int(state["issue"]["number"])
    environment = state.get("environment", "")
    findings = []
    for pull in state.get("pulls", []):
        reasons = [where for where, text in (("branch", pull.get("head", {}).get("ref")),
                                            ("title", pull.get("title")), ("body", pull.get("body")))
                   if mentions(text, issue)]
        if reasons:
            merged = pull.get("merged_at") is not None
            findings.append({
                "kind": "open-pr" if pull.get("state") == "open" else "closed-pr",
                "blocking": pull.get("state") == "open",
                "detail": f"#{pull['number']} ({'merged' if merged else pull.get('state')}) "
                          f"{pull.get('head', {}).get('ref', '')} matches in {', '.join(reasons)}",
            })
    pr_refs = {pull.get("head", {}).get("ref") for pull in state.get("pulls", [])}
    for branch in state.get("remote_branches", []):
        name = branch["ref"].removeprefix("refs/heads/")
        if mentions(name, issue) and name not in pr_refs:
            findings.append({"kind": "remote-branch", "blocking": False,
                             "detail": f"{name} at {branch['sha']} has no PR; inspect before starting"})
    for claim in state.get("claims", []):
        if claim.get("branch") == f"issue-{issue}" and claim.get("state") == "active" \
                and claim.get("agent") != state.get("agent"):
            findings.append({"kind": "local-claim", "blocking": True,
                             "detail": f"issue-{issue} is held by {claim.get('agent')} in this checkout"})
    latest = None
    for comment in state.get("comments", []):
        checkpoint, errors, _ = validate(comment.get("body", ""))
        if checkpoint is not None and not errors:
            latest = (comment, checkpoint)
    if latest is None:
        findings.append({"kind": "no-checkpoint", "blocking": False,
                         "detail": "no valid checkpoint on the issue; the claim registry alone "
                                   "says nothing about other machines"})
    else:
        comment, checkpoint = latest
        posted = dt.datetime.fromisoformat(comment["created_at"].replace("Z", "+00:00"))
        age = ((now or dt.datetime.now(dt.timezone.utc)) - posted).total_seconds() / 3600
        elsewhere = checkpoint["ENVIRONMENT"] != environment
        findings.append({
            "kind": "recent-checkpoint-elsewhere" if elsewhere and age < fresh_hours else "checkpoint",
            "blocking": elsewhere and age < fresh_hours,
            "detail": f"{comment.get('html_url', 'comment')} ({age:.1f} h old) from "
                      f"{checkpoint['ENVIRONMENT']!r} on {checkpoint['BRANCH']} at {checkpoint['FULL_SHA']}",
        })
        live = {branch["ref"].removeprefix("refs/heads/"): branch["sha"]
                for branch in state.get("remote_branches", [])}
        head = live.get(checkpoint["BRANCH"])
        if head and head != checkpoint["FULL_SHA"]:
            findings.append({"kind": "stale-checkpoint", "blocking": False,
                             "detail": f"{checkpoint['BRANCH']} is now {head}, not {checkpoint['FULL_SHA']}; "
                                       "reconcile the new commits before resuming"})
    return findings


Runner = Callable[[list[str]], "subprocess.CompletedProcess[str]"]


def run(argv: list[str]) -> "subprocess.CompletedProcess[str]":
    return subprocess.run(argv, capture_output=True, text=True, check=False)


def default_agent(runner: Runner) -> str:
    """Coven's fallback claim identity: $USER@<worktree directory name>."""
    top = runner(["git", "rev-parse", "--show-toplevel"])
    slug = os.path.basename(top.stdout.strip()) if top.returncode == 0 else ""
    return f"{os.environ.get('USER', '')}@{slug}"


def gather(repo: str, issue: int, environment: str, runner: Runner = run) -> dict:
    """Read-only: GitHub REST (never GraphQL), `git ls-remote` and claim status."""
    def api(path: str):
        result = runner(["gh", "api", path])
        if result.returncode != 0:
            raise RuntimeError(f"gh api {path} failed (exit {result.returncode})")
        return json.loads(result.stdout)
    data = api(f"repos/{repo}/issues/{issue}")
    state = {
        "issue": {"number": issue, "state": data.get("state"),
                  "assignees": [user["login"] for user in data.get("assignees", [])]},
        "comments": api(f"repos/{repo}/issues/{issue}/comments?per_page=100"),
        "pulls": api(f"repos/{repo}/pulls?state=all&per_page=100"),
        "environment": environment,
        "agent": os.environ.get("COVEN_AGENT_ID") or default_agent(runner),
        "remote_branches": [],
        "claims": [],
    }
    remote = runner(["git", "ls-remote", "--heads", "origin"])
    if remote.returncode == 0:
        state["remote_branches"] = [{"sha": sha, "ref": ref} for sha, ref in
                                    (line.split("\t", 1) for line in remote.stdout.splitlines() if "\t" in line)]
    claims = runner(["coven", "claim", "status"])
    if claims.returncode == 0:
        for line in claims.stdout.splitlines()[1:]:
            parts = line.split()
            if len(parts) >= 3:
                state["claims"].append({"branch": parts[0], "agent": parts[1], "state": parts[2]})
    return state


def read(path: str) -> str:
    return sys.stdin.read() if path == "-" else open(path, encoding="utf-8").read()


def main(argv: Iterable[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("validate", help="validate the one checkpoint block in a comment body")
    check.add_argument("path", help="markdown file, or - for stdin")
    check.add_argument("--head", help="live branch head; a different FULL_SHA is reported stale (exit 3)")
    make = sub.add_parser("render", help="print a checkpoint block (validated) from FIELD=value pairs")
    make.add_argument("fields", nargs="+", metavar="FIELD=value")
    make.add_argument("--from-git", action="store_true", help="fill BRANCH and FULL_SHA from HEAD")
    legacy = sub.add_parser("audit", help="report which fields a free-form checkpoint names")
    legacy.add_argument("path")
    conflicts = sub.add_parser("conflicts", help="pre-edit conflict findings (exit 4 if blocking)")
    conflicts.add_argument("--state", help="state JSON from `gather`; default: gather live")
    conflicts.add_argument("--repo", default="OpenCoven/coven")
    conflicts.add_argument("--issue", type=int)
    conflicts.add_argument("--environment", default="")
    conflicts.add_argument("--fresh-hours", type=float, default=2.0)
    collect = sub.add_parser("gather", help="print read-only conflict state as JSON")
    collect.add_argument("--repo", default="OpenCoven/coven")
    collect.add_argument("--issue", type=int, required=True)
    collect.add_argument("--environment", required=True)
    args = parser.parse_args(list(argv) if argv is not None else None)

    if args.command == "validate":
        checkpoint, errors, stale = validate(read(args.path), args.head)
        for line in errors + stale:
            print(line)
        if errors:
            return 1
        print(f"checkpoint ok: {checkpoint['ISSUE']} {checkpoint['BRANCH']} {checkpoint['FULL_SHA']}")
        return 3 if stale else 0
    if args.command == "render":
        fields = {}
        for pair in args.fields:
            key, sep, value = pair.partition("=")
            if not sep:
                parser.error(f"expected FIELD=value, got {pair!r}")
            fields[key] = value
        if args.from_git:
            fields["FULL_SHA"] = run(["git", "rev-parse", "HEAD"]).stdout.strip()
            branch = run(["git", "symbolic-ref", "--quiet", "--short", "HEAD"])
            fields["BRANCH"] = branch.stdout.strip() if branch.returncode == 0 else DETACHED
        text = render(fields)
        _, errors, _ = validate(text)
        for line in errors:
            print(line, file=sys.stderr)
        if errors:
            return 1
        print(text, end="")
        return 0
    if args.command == "audit":
        print(json.dumps(audit(read(args.path)), indent=2))
        return 0
    if args.command == "gather":
        print(json.dumps(gather(args.repo, args.issue, args.environment), indent=2))
        return 0
    if args.state:
        state = json.loads(read(args.state))
    elif args.issue is not None:
        state = gather(args.repo, args.issue, args.environment)
    else:
        parser.error("conflicts needs --state or --issue")
    findings = decide(state, fresh_hours=args.fresh_hours)
    for finding in findings:
        print(f"{'BLOCK' if finding['blocking'] else 'INFO '} {finding['kind']}: {finding['detail']}")
    return 4 if any(finding["blocking"] for finding in findings) else 0


if __name__ == "__main__":
    sys.exit(main())
