#!/usr/bin/env python3
"""Offline publication preflight contracts: every classified state, its handoff, no writes, no leaks."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
FIXTURE = json.loads((HERE / "fixtures" / "preflight-states.json").read_text())
SHA, MERGE_BASE = FIXTURE["sha"], FIXTURE["merge_base"]
BRANCH = "feat/1452-publication-preflight"
# Built at run time so no credential-shaped literal is committed.
TOKEN = "".join(["gh", "o_", "R" * 36])
# Every command the preflight may run: reads only, plus the patch and bundle
# that go to an explicit --handoff-dir.
READ_ONLY = (
    ("git", "--version"), ("git", "rev-parse"), ("git", "symbolic-ref"), ("git", "remote", "get-url"),
    ("git", "merge-base"), ("git", "diff", "--stat"), ("git", "format-patch", "--stdout"),
    ("git", "bundle", "create"), ("gh", "--version"), ("gh", "auth", "status"), ("gh", "api"),
)


def load():
    spec = importlib.util.spec_from_file_location("delivery_preflight", HERE / "preflight.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


preflight = load()


class Fake:
    def __init__(self, override=None):
        self.responses = dict(FIXTURE["baseline"])
        for key, value in (override or {}).items():
            if value is None:
                self.responses.pop(key, None)
            else:
                self.responses[key] = value
        self.calls = []

    def __call__(self, argv):
        self.calls.append(argv)
        joined = " ".join(argv)
        if joined not in self.responses:
            return subprocess.CompletedProcess(argv, 127, "", f"{argv[0]}: command not found {TOKEN}")
        code, stdout, stderr = self.responses[joined]
        return subprocess.CompletedProcess(argv, code, stdout.replace("{TOKEN}", TOKEN),
                                           stderr.replace("{TOKEN}", TOKEN) + f" {TOKEN}")


def execute(scenario, *extra, runner=None):
    runner = runner or Fake(scenario.get("override"))
    paths = []
    with tempfile.TemporaryDirectory() as temp:
        for index, text in enumerate(scenario.get("observed", [])):
            path = Path(temp) / f"observed-{index}.txt"
            path.write_text(text + TOKEN)
            paths += ["--observed", str(path)]
        env = {"USER": "fixture", "COVEN_ENVIRONMENT": "fixture runner", **scenario.get("env", {})}
        out = io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
            code = preflight.main(["--issue", "#1452", "--json", *paths, *extra], runner=runner, env=env)
        text = out.getvalue()
        return code, (json.loads(text) if text.startswith("{") else text), runner


class StateTests(unittest.TestCase):
    def test_every_fixture_state_is_classified_with_its_exact_recovery(self):
        for scenario in FIXTURE["scenarios"]:
            with self.subTest(scenario=scenario["name"]):
                code, report, _ = execute(scenario)
                expect = scenario["expect"]
                self.assertEqual(report["delivery"], expect["delivery"])
                self.assertEqual(code, 5 if expect["delivery"] == "handoff" else 0)
                levels = {s["state"]: s["level"] for s in report["states"]}
                # Info states only change no command; the fixture names those it checks.
                self.assertEqual({k: v for k, v in levels.items() if v != "info"},
                                 {k: v for k, v in expect["states"].items() if v != "info"})
                for name, level in expect["states"].items():
                    self.assertEqual(levels.get(name), level, name)
                commands = "\n".join(report["next"])
                for wanted in expect.get("commands", []):
                    self.assertIn(wanted.format(sha=SHA, branch=BRANCH), commands)
                for unwanted in expect.get("absent", []):
                    self.assertNotIn(unwanted, commands)
                for path in expect.get("paths_absent", []):
                    self.assertNotIn(path, report["facts"].get("paths", []))

    def test_each_non_clean_state_carries_a_valid_handoff_bundle(self):
        for scenario in FIXTURE["scenarios"]:
            if scenario["expect"]["delivery"] == "pr" or scenario["name"] == "not-a-repository":
                continue
            with self.subTest(scenario=scenario["name"]):
                _, report, _ = execute(scenario)
                bundle = report["handoff"]
                self.assertEqual(bundle["checkpoint_errors"], [])
                self.assertEqual(bundle["full_sha"], SHA)
                if bundle["merge_base"]:
                    self.assertIn(f"git format-patch --stdout {MERGE_BASE}..{SHA}", bundle["patch_command"])
                else:
                    self.assertIsNone(bundle["patch_command"])
                    self.assertIn("--base-ref", bundle["patch_omitted"])
                self.assertIn("git bundle create", bundle["bundle_command"])
                self.assertIn(f"FULL_SHA: {SHA}", bundle["checkpoint"])
                self.assertIn(f"NEXT: {report['next'][0]}", bundle["checkpoint"])
                for item in report["states"]:
                    if item["level"] != "info":
                        self.assertIn(item["state"], bundle["checkpoint"])

    def test_a_clean_delivery_emits_no_handoff(self):
        _, report, _ = execute(FIXTURE["scenarios"][0])
        self.assertNotIn("handoff", report)

    def test_observed_failures_classify_without_echoing_their_text(self):
        cases = {
            "gh: Merge must be made through the merge queue (HTTP 405)": "merge-queue-405",
            "GraphQL: API rate limit already exceeded for user ID 1.": "graphql-rate-limit",
            "API rate limit exceeded for user ID 1.": "rest-rate-limit",
            "fatal: Authentication failed for 'https://github.com/OpenCoven/coven.git/'": "missing-auth",
            "ERROR: Permission to OpenCoven/coven.git denied to someone.": "missing-auth",
            "fatal: 'origin' does not appear to be a git repository": "missing-remote",
            "ssh: Could not resolve host github.com": "network",
            "zsh: command not found: gh": "missing-tool",
            " ! [rejected]        feat/x -> feat/x (non-fast-forward)": "non-fast-forward",
        }
        for text, state in cases.items():
            with self.subTest(state=state):
                self.assertEqual(preflight.classify(text), state)


class SafetyTests(unittest.TestCase):
    def test_nothing_but_reads_is_ever_run_and_graphql_is_never_called(self):
        for scenario in FIXTURE["scenarios"]:
            _, _, runner = execute(scenario)
            for argv in runner.calls:
                with self.subTest(scenario=scenario["name"], argv=argv):
                    self.assertTrue(any(tuple(argv[:len(prefix)]) == prefix for prefix in READ_ONLY), argv)
                    self.assertNotIn("graphql", " ".join(argv))
                    if argv[:2] == ["gh", "api"]:
                        self.assertEqual(len(argv), 3, "gh api calls are plain REST GETs")
                    self.assertNotEqual(argv[:2], ["git", "bundle"], "bundle only with --handoff-dir")

    def test_credentials_in_tool_output_never_reach_the_report(self):
        for scenario in FIXTURE["scenarios"]:
            with self.subTest(scenario=scenario["name"]):
                _, report, _ = execute(scenario)
                self.assertNotIn(TOKEN, json.dumps(report))
                code, text, _ = execute(scenario, runner=Fake(scenario.get("override")))
                self.assertNotIn(TOKEN, json.dumps(text))

    def test_no_command_pushes_the_base_merges_by_rest_or_deletes_a_branch(self):
        for scenario in FIXTURE["scenarios"]:
            with self.subTest(scenario=scenario["name"]):
                _, report, _ = execute(scenario)
                commands = "\n".join(report["next"])
                for forbidden in ("push origin main", "push -u origin main", "HEAD:main", "-X PUT",
                                  "--delete", "push origin :", "--force", "branch -D"):
                    self.assertNotIn(forbidden, commands)

    def test_handoff_dir_writes_only_outside_the_worktree(self):
        scenario = next(s for s in FIXTURE["scenarios"] if s["name"] == "no-remote")
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "checkout"
            override = dict(scenario["override"], **{"git rev-parse --show-toplevel": [0, f"{root}\n", ""]})
            code, text, runner = execute({"override": override}, "--handoff-dir", str(root / "out"))
            self.assertEqual(code, 2)
            self.assertIn("refusing --handoff-dir inside the worktree", text)
            self.assertFalse((root / "out").exists())
            self.assertFalse(any(argv[:2] in (["git", "format-patch"], ["git", "bundle"]) for argv in runner.calls))


class RealGitTests(unittest.TestCase):
    """The git half against a real repository; gh is absent from the fake world."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        self.git("init", "-q", "-b", "feat/1452-real")
        for name in ("one", "two"):
            (self.root / name).write_text(name + "\n")
            self.git("add", name)
            self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@users.noreply.github.com",
                     "-c", "commit.gpgsign=false", "commit", "-qm", name)
        self.head = self.git("rev-parse", "HEAD").strip()

    def git(self, *args):
        return subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True, text=True).stdout

    def runner(self, argv):
        if argv[0] == "gh":
            return subprocess.CompletedProcess(argv, 127, "", "gh: command not found")
        if argv[0] == "git":
            argv = ["git", "-C", str(self.root), *argv[1:]]
        return subprocess.run(argv, capture_output=True, text=True, check=False)

    def status(self):
        return self.git("status", "--porcelain", "--ignored"), self.git("for-each-ref"), self.git("config", "--list", "--local")

    def test_a_checkout_without_a_remote_hands_off_a_verifiable_patch_and_bundle(self):
        before = self.status()
        with tempfile.TemporaryDirectory() as out:
            report = preflight.Preflight("OpenCoven/coven", "main", self.runner).probe([])
            self.assertEqual(report["delivery"], "handoff")
            self.assertEqual({s["state"] for s in report["states"]}, {"missing-remote", "missing-tool"})
            report["facts"]["merge_base"] = self.git("rev-parse", "HEAD~1").strip()
            bundle = preflight.handoff(report, self.runner, Path(out), {
                "issue": "#1452", "owner": "fixture", "environment": "fixture checkout"})
            self.assertEqual(bundle["checkpoint_errors"], [])
            patch = Path(bundle["patch"]["path"])
            self.assertEqual(bundle["patch"]["sha256"], hashlib.sha256(patch.read_bytes()).hexdigest())
            self.assertIn("+two", patch.read_text())
            clone = Path(out) / "clone"
            subprocess.run(["git", "clone", "-q", str(self.root), str(clone)], check=True)
            subprocess.run(["git", "-C", str(clone), "fetch", "-q", bundle["bundle"]["path"],
                            "HEAD:refs/heads/handed-off"], check=True)
            fetched = subprocess.run(["git", "-C", str(clone), "rev-parse", "handed-off"],
                                     check=True, capture_output=True, text=True).stdout.strip()
            self.assertEqual(fetched, self.head)
        self.assertEqual(self.status(), before)

    def test_without_a_merge_base_the_bundle_still_carries_head_and_the_patch_says_why(self):
        with tempfile.TemporaryDirectory() as out:
            report = preflight.Preflight("OpenCoven/coven", "main", self.runner).probe([])
            self.assertIsNone(report["facts"]["merge_base"])
            bundle = preflight.handoff(report, self.runner, Path(out), {"owner": "fixture", "environment": "fixture"})
            self.assertNotIn("patch", bundle)
            self.assertIn("--base-ref", bundle["patch_omitted"])
            clone = Path(out) / "clone"
            subprocess.run(["git", "init", "-q", str(clone)], check=True)
            subprocess.run(["git", "-C", str(clone), "fetch", "-q", bundle["bundle"]["path"],
                            "HEAD:refs/heads/handed-off"], check=True)
            fetched = subprocess.run(["git", "-C", str(clone), "rev-parse", "handed-off"],
                                     check=True, capture_output=True, text=True).stdout.strip()
            self.assertEqual(fetched, self.head)

    def test_a_base_ref_cuts_the_patch_when_no_base_branch_exists(self):
        first = self.git("rev-parse", "HEAD~1").strip()
        report = preflight.Preflight("OpenCoven/coven", "main", self.runner, base_ref=first).probe([])
        self.assertEqual(report["facts"]["merge_base"], first)

    def test_the_command_leaves_a_checkout_byte_for_byte_unchanged(self):
        scripts = self.root / "scripts" / "delivery"
        scripts.mkdir(parents=True)
        for name in ("preflight.py", "checkpoint.py"):
            (scripts / name).write_text((HERE / name).read_text())
        self.git("add", "scripts")
        self.git("-c", "user.name=Fixture", "-c", "user.email=fixture@users.noreply.github.com",
                 "-c", "commit.gpgsign=false", "commit", "-qm", "scripts")
        before = self.status()
        bin_dir = Path(self.temp.name) / "bin"
        bin_dir.mkdir()
        git = subprocess.run(["sh", "-c", "command -v git"], capture_output=True, text=True).stdout.strip()
        (bin_dir / "git").symlink_to(git)
        # Some interpreters cache bytecode outside the source tree; pin that
        # cache here so a write is caught wherever it would land.
        prefix = Path(self.temp.name) / "pycache"
        result = subprocess.run([sys.executable, "-X", f"pycache_prefix={prefix}",
                                 "scripts/delivery/preflight.py", "--issue", "#1452"],
                                cwd=self.root, capture_output=True, text=True,
                                env={"PATH": str(bin_dir), "HOME": self.temp.name, "USER": "fixture"})
        self.assertEqual(result.returncode, 5, result.stderr)
        self.assertIn("BLOCK    missing-remote", result.stdout)
        self.assertEqual(self.status(), before)
        written = [path.name for path in prefix.rglob("*.pyc") if path.name.startswith(("checkpoint.", "preflight."))]
        self.assertEqual(written, [])

    def test_a_detached_head_is_refused_as_a_pr_source(self):
        self.git("checkout", "-q", "--detach")
        report = preflight.Preflight("OpenCoven/coven", "main", self.runner).probe([])
        self.assertIn("detached-head", {s["state"] for s in report["states"]})
        self.assertIn(f"git switch -c <task-branch> {self.head}", report["next"])


if __name__ == "__main__":
    unittest.main()
