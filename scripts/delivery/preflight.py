#!/usr/bin/env python3
"""Read-only publication preflight with a deterministic blocked-delivery handoff.

Run it before long work and again before publishing. It answers one question:
can this session turn its commits into a pull request, and if not, exactly
what does a maintainer need to finish the delivery? It never pushes, creates,
merges, deletes or edits anything, never calls GraphQL, and never prints tool
output, so credentials cannot leak through it. The only files it writes are
the optional patch and bundle in a --handoff-dir outside the worktree.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
import platform
import re
import subprocess
import sys
from pathlib import Path
from typing import Callable

Runner = Callable[[list[str]], "subprocess.CompletedProcess[str]"]

# Observed failure text -> state. Order matters: the first match wins.
CLASSIFIERS = (
    ("merge-queue-405", re.compile(r"merge queue|HTTP 405|405 Method Not Allowed", re.I)),
    ("graphql-rate-limit", re.compile(r"GraphQL:? API rate limit|graphql.*rate limit", re.I)),
    ("rest-rate-limit", re.compile(r"API rate limit (?:already )?exceeded|HTTP 403.*rate limit", re.I)),
    ("missing-workflow-scope", re.compile(r"refusing to allow an OAuth App to create or update workflow", re.I)),
    ("missing-auth", re.compile(r"not logged in|gh auth login|Bad credentials|HTTP 401|Authentication failed"
                                r"|Permission to \S+ denied|Permission denied \(publickey\)|could not read Username", re.I)),
    ("missing-remote", re.compile(r"does not appear to be a git repository|No such remote|no upstream", re.I)),
    ("network", re.compile(r"Could not resolve host|Failed to connect|Connection timed out|network is unreachable", re.I)),
    ("missing-tool", re.compile(r"command not found|No such file or directory|not recognized as an internal", re.I)),
    ("non-fast-forward", re.compile(r"non-fast-forward|\[rejected\].*fetch first", re.I)),
)
# block: no pull request can be opened from here, so the delivery is a handoff.
# degraded: a pull request can still be opened, but only by the listed fallback.
# info: changes no command. Anything unrecognized blocks.
LEVELS = {
    "graphql-rate-limit": "degraded", "merge-queue-405": "degraded",
    "merge-queue-required": "info", "existing-pr": "info",
}


def load_checkpoint():
    # Importing must not leave __pycache__ in the checkout being inspected.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("delivery_checkpoint", Path(__file__).with_name("checkpoint.py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(argv: list[str]) -> "subprocess.CompletedProcess[str]":
    try:
        return subprocess.run(argv, capture_output=True, text=True, check=False, timeout=60)
    except FileNotFoundError:
        return subprocess.CompletedProcess(argv, 127, "", f"{argv[0]}: command not found")
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(argv, 124, "", "Connection timed out")


def classify(text: str) -> str:
    for state, pattern in CLASSIFIERS:
        if pattern.search(text):
            return state
    return "unknown-failure"


class Preflight:
    def __init__(self, repo: str, base: str, runner: Runner = run, integration: str = "", base_ref: str = ""):
        self.repo, self.base, self.runner, self.integration = repo, base, runner, integration
        self.base_ref = base_ref
        self.facts: dict = {"repo": repo, "base": base}
        self.states: list[dict] = []

    def ok(self, *argv: str) -> str | None:
        result = self.runner(list(argv))
        return result.stdout if result.returncode == 0 else None

    def state(self, name: str, detail: str, level: str | None = None) -> None:
        if name not in {s["state"] for s in self.states}:
            self.states.append({"state": name, "level": level or LEVELS.get(name, "block"), "detail": detail})

    def probe(self, observed: list[str]) -> dict:
        facts = self.facts
        if self.ok("git", "--version") is None:
            self.state("missing-git", "git is not installed; nothing can be published or handed off")
            return self.report()
        top = self.ok("git", "rev-parse", "--show-toplevel")
        if top is None:
            self.state("not-a-repository", "the working directory is not inside a git checkout")
            return self.report()
        facts["root"] = top.strip()
        facts["full_sha"] = (self.ok("git", "rev-parse", "HEAD") or "").strip()
        branch = self.ok("git", "symbolic-ref", "--quiet", "--short", "HEAD")
        facts["branch"] = branch.strip() if branch else None
        if facts["branch"] is None:
            self.state("detached-head", "HEAD is detached; a pull request needs a branch")
        elif facts["branch"] == self.base:
            self.state("on-default-branch", f"on {self.base}; work is published from a task branch, never {self.base}")
        remote = self.ok("git", "remote", "get-url", "origin")
        facts["remote"] = bool(remote)
        if remote is None:
            self.state("missing-remote", "no origin remote; the branch cannot be pushed from here")
        facts["merge_base"] = None
        for ref in [self.base_ref] if self.base_ref else [f"origin/{self.base}", self.base]:
            found = self.ok("git", "merge-base", "HEAD", ref)
            if found:
                facts["merge_base"], facts["merge_base_ref"] = found.strip(), ref
                break
        self.github()
        for text in observed:
            name = classify(text)
            self.state(name, f"observed while publishing: {name}")
        return self.report()

    def github(self) -> None:
        facts = self.facts
        pushable = ["git-push"] if facts.get("remote") else []
        if self.ok("gh", "--version") is None:
            # A missing gh alone does not stop a session whose harness publishes
            # through its own connected integration.
            if self.integration:
                self.state("missing-tool", f"gh CLI is not installed; publish through {self.integration}", "degraded")
            else:
                self.state("missing-tool", "gh CLI is not installed and no publication integration is declared")
            facts["paths"] = pushable + (["integration"] if self.integration else [])
            return
        status = self.runner(["gh", "auth", "status"])
        if status.returncode != 0:
            self.state("missing-auth", "gh is not authenticated for github.com")
            return
        limits = self.ok("gh", "api", "rate_limit")
        if limits is None:
            self.state("network", "GitHub REST is unreachable")
            return
        resources = json.loads(limits).get("resources", {})
        core, graphql = resources.get("core", {}), resources.get("graphql", {})
        facts["rate"] = {"core_remaining": core.get("remaining"), "graphql_remaining": graphql.get("remaining"),
                         "graphql_reset": graphql.get("reset"), "core_reset": core.get("reset")}
        if core.get("remaining") == 0:
            self.state("rest-rate-limit", f"REST quota exhausted until epoch {core.get('reset')}")
            return
        if graphql.get("remaining") == 0:
            self.state("graphql-rate-limit",
                       f"GraphQL quota exhausted until epoch {graphql.get('reset')}; REST still works")
        repo = self.ok("gh", "api", f"repos/{self.repo}")
        if repo is not None:
            permissions = json.loads(repo).get("permissions", {})
            facts["push_permission"] = bool(permissions.get("push"))
            if not permissions.get("push"):
                self.state("missing-push-permission", f"this account cannot push to {self.repo}")
        rules = self.ok("gh", "api", f"repos/{self.repo}/rules/branches/{self.base}")
        if rules is not None:
            types = {rule.get("type") for rule in json.loads(rules)}
            facts["merge_queue"] = "merge_queue" in types
            if facts["merge_queue"]:
                self.state("merge-queue-required", f"{self.base} merges only through the merge queue; "
                           "a REST merge returns 405")
            facts["required_checks"] = sorted(
                check.get("context") for rule in json.loads(rules) if rule.get("type") == "required_status_checks"
                for check in rule.get("parameters", {}).get("required_status_checks", []))
        branch = facts.get("branch")
        if branch and branch != self.base:
            owner = self.repo.split("/")[0]
            pulls = self.ok("gh", "api", f"repos/{self.repo}/pulls?head={owner}:{branch}&state=all")
            if pulls is not None:
                facts["pulls"] = [{"number": p["number"], "state": p["state"], "merged": p.get("merged_at") is not None,
                                   "draft": p.get("draft", False), "head_sha": p.get("head", {}).get("sha")}
                                  for p in json.loads(pulls)]
                for pull in facts["pulls"]:
                    if pull["state"] == "open":
                        self.state("existing-pr", f"#{pull['number']} is open for {branch}; reuse it")
        graphql = [] if any(s["state"] == "graphql-rate-limit" for s in self.states) else ["gh-graphql"]
        facts["paths"] = pushable + ["gh-rest"] + graphql + (["integration"] if self.integration else [])

    def report(self) -> dict:
        levels = {s["level"] for s in self.states}
        delivery = "handoff" if "block" in levels else "pr-degraded" if "degraded" in levels else "pr"
        return {"facts": self.facts, "states": self.states, "delivery": delivery,
                "next": next_commands(self.facts, self.states, self.repo, self.base)}


def next_commands(facts: dict, states: list[dict], repo: str, base: str) -> list[str]:
    """Exact commands for whoever finishes the delivery. Never a push to the base,
    never a REST merge, never a branch delete chained to a merge."""
    branch = facts.get("branch") or "<task-branch>"
    sha = facts.get("full_sha") or "<full-sha>"
    open_prs = [p for p in facts.get("pulls", []) if p["state"] == "open"]
    pr = f"{open_prs[0]['number']}" if open_prs else "<pr-number>"
    names = [s["state"] for s in states if s["level"] != "info"]
    commands = []
    if "detached-head" in names or "on-default-branch" in names:
        commands.append(f"git switch -c <task-branch> {sha}")
    if "missing-remote" in names:
        commands.append(f"git remote add origin https://github.com/{repo}.git   # maintainer: or apply the handoff patch/bundle")
    if "missing-auth" in names or "missing-push-permission" in names:
        commands.append("gh auth login   # a maintainer with push access; never inject a broad token")
    if "missing-workflow-scope" in names:
        commands.append("gh auth refresh -h github.com -s workflow   # or push this branch over SSH")
    if "missing-tool" in names:
        commands.append(f"git push -u origin {branch}")
        commands.append(f"# then open https://github.com/{repo}/compare/{base}...{branch}?expand=1")
    if "rest-rate-limit" in names:
        commands.append(f"gh api rate_limit --jq .resources.core.reset   # wait for the reset, then rerun this preflight")
    if "graphql-rate-limit" in names:
        reset = facts.get("rate", {}).get("graphql_reset")
        if not open_prs:
            commands.append(f"gh api -X POST repos/{repo}/pulls -f head={branch} -f base={base} "
                            "-f title='<conventional title>' -F body=@pr-body.md")
        commands.append(f"gh api repos/{repo}/commits/{sha}/check-runs?per_page=100 "
                        "--jq '.check_runs[] | [.name, .conclusion] | @tsv'")
        commands.append(f"# gh pr ready / gh pr merge need GraphQL: wait for reset epoch {reset}")
    if "non-fast-forward" in names:
        commands.append(f"git fetch origin {branch} && git log --oneline HEAD..origin/{branch}   # reconcile, never force over it")
    if "network" in names:
        commands.append("# network: retry once the host resolves; publication is unchanged")
    if not names or names == ["merge-queue-405"]:
        commands.append(f"git push -u origin {branch}" if not open_prs else f"git push origin {branch}")
        if not open_prs:
            commands.append(f"gh pr create --base {base} --head {branch} --body-file pr-body.md")
    if "merge-queue-405" in names or facts.get("merge_queue"):
        commands.append(f"gh pr merge {pr} --match-head-commit {sha}   # enters the merge queue once CI is green")
        commands.append("# never PUT pulls/<n>/merge (405 under the queue); delete the branch only after the merge lands")
    return commands


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def handoff(report: dict, runner: Runner, directory: Path | None, fields: dict) -> dict:
    """Patch, bundle and checkpoint for a blocked delivery. Files only in `directory`."""
    facts = report["facts"]
    sha, base_sha = facts.get("full_sha"), facts.get("merge_base")
    name = (sha or "head")[:12]
    revision = f"{base_sha}..{sha}" if base_sha else None
    # A bundle records refs, not bare commits, so it is cut from HEAD. Without a
    # merge base it carries the whole history: complete, but large.
    tips = f"{base_sha}..HEAD" if base_sha else "HEAD"
    bundle = {"full_sha": sha, "branch": facts.get("branch"), "merge_base": base_sha,
              "patch_command": f"git format-patch --stdout {revision} > {name}.patch" if revision else None,
              "bundle_command": f"git bundle create {name}.bundle {tips}"}
    if revision is None:
        bundle["patch_omitted"] = "no merge base with the base branch here; rerun with --base-ref <commit>"
    stat = runner(["git", "diff", "--stat", revision]) if revision else None
    bundle["diffstat"] = stat.stdout.strip() if stat is not None and stat.returncode == 0 else None
    if directory is not None and sha:
        directory.mkdir(parents=True, exist_ok=True)
        patch = runner(["git", "format-patch", "--stdout", revision]) if revision else None
        if patch is not None and patch.returncode == 0:
            path = directory / f"{sha[:12]}.patch"
            path.write_text(patch.stdout)
            bundle["patch"] = {"path": str(path), "sha256": sha256(path)}
        target = directory / f"{sha[:12]}.bundle"
        made = runner(["git", "bundle", "create", str(target), tips])
        if made.returncode == 0 and target.exists():
            bundle["bundle"] = {"path": str(target), "sha256": sha256(target),
                                "fetch": f"git fetch {target} HEAD:refs/heads/{facts.get('branch') or '<task-branch>'}"}
    open_prs = [p for p in facts.get("pulls", []) if p["state"] == "open"]
    states = [s for s in report["states"] if s["level"] != "info"]
    checkpoint = load_checkpoint()
    values = {
        "ISSUE": fields.get("issue") or "#<issue>",
        "OWNER": fields.get("owner"),
        "ENVIRONMENT": fields.get("environment"),
        "BRANCH": facts.get("branch") or checkpoint.DETACHED,
        "FULL_SHA": sha or "",
        "PR": f"#{open_prs[0]['number']}" if open_prs else
              "none (publication blocked)" if report["delivery"] == "handoff" else "none",
        "COMPLETED": fields.get("completed") or "none",
        "CHECKS": fields.get("checks") or f"publication preflight -> {report['delivery']}",
        "DECISIONS": "none",
        "BLOCKERS": "\n".join(f"{s['state']}: {s['detail']}" for s in states) or "none",
        "NEXT": "\n".join(report["next"]),
    }
    bundle["checkpoint"] = checkpoint.render(values)
    bundle["checkpoint_errors"] = checkpoint.validate(bundle["checkpoint"])[1]
    return bundle


def default_identity(env) -> tuple[str, str]:
    owner = env.get("COVEN_AGENT_ID") or env.get("USER", "") or "unknown agent"
    environment = env.get("COVEN_ENVIRONMENT") or f"{platform.system()} {platform.machine()} (undeclared)"
    return owner, environment


def render_text(report: dict) -> str:
    facts = report["facts"]
    lines = [f"delivery: {report['delivery']}",
             f"branch: {facts.get('branch') or '(detached HEAD)'} at {facts.get('full_sha') or 'unknown'}",
             f"paths: {', '.join(facts.get('paths', [])) or 'none'}"]
    for item in report["states"]:
        lines.append(f"{item['level'].upper():8} {item['state']}: {item['detail']}")
    lines.append("next:")
    lines.extend(f"  {command}" for command in report["next"])
    if "handoff" in report:
        bundle = report["handoff"]
        lines.append("handoff:")
        for key in ("patch", "bundle"):
            if key in bundle:
                lines.append(f"  {key}: {bundle[key]['path']} sha256 {bundle[key]['sha256']}")
            elif bundle.get(key + "_command"):
                lines.append(f"  {key}: {bundle[key + '_command']}")
            else:
                lines.append(f"  {key}: omitted ({bundle[key + '_omitted']})")
        lines.append(bundle["checkpoint"].rstrip())
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None, runner: Runner = run, env=None) -> int:
    env = os.environ if env is None else env
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--repo", default="OpenCoven/coven")
    parser.add_argument("--base", default="main")
    parser.add_argument("--base-ref", default="", help="commit or ref to cut the patch from (default: origin/<base>, then <base>)")
    parser.add_argument("--issue", help="issue reference for the handoff checkpoint, e.g. #1452")
    parser.add_argument("--owner", help="checkpoint OWNER (default: COVEN_AGENT_ID or USER)")
    parser.add_argument("--environment", help="checkpoint ENVIRONMENT (default: COVEN_ENVIRONMENT or platform)")
    parser.add_argument("--checks", help="checkpoint CHECKS: commands already run and their results")
    parser.add_argument("--observed", action="append", default=[], metavar="FILE",
                        help="text of a failed publication attempt to classify (repeatable)")
    parser.add_argument("--handoff-dir", type=Path, help="write the patch and bundle here (outside the worktree)")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    integration = env.get("COVEN_PUBLICATION_INTEGRATION", "")
    observed = [Path(path).read_text(encoding="utf-8", errors="replace") for path in args.observed]
    preflight = Preflight(args.repo, args.base, runner, integration, args.base_ref)
    report = preflight.probe(observed)
    root = report["facts"].get("root")
    if args.handoff_dir is not None and root:
        target = args.handoff_dir.resolve()
        if target == Path(root).resolve() or Path(root).resolve() in target.parents:
            print("refusing --handoff-dir inside the worktree: preflight must not change the checkout", file=sys.stderr)
            return 2
    if report["delivery"] != "pr":
        owner, environment = default_identity(env)
        report["handoff"] = handoff(report, runner, args.handoff_dir, {
            "issue": args.issue, "owner": args.owner or owner, "environment": args.environment or environment,
            "checks": args.checks})
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else render_text(report), end="" if not args.json else "\n")
    return 5 if report["delivery"] == "handoff" else 0


if __name__ == "__main__":
    sys.exit(main())
