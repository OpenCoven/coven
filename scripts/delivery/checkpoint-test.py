#!/usr/bin/env python3
"""Offline checkpoint contracts: schema, legacy audit and pre-edit conflicts."""
import contextlib
import datetime as dt
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
SHA = "6a26f5116d3a61b720cd7e984a11eb8a3c10ac94"
OTHER_SHA = "a8dde08d182bdd364ed0a557e7c748080819cf34"


def load():
    spec = importlib.util.spec_from_file_location("delivery_checkpoint", HERE / "checkpoint.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


checkpoint = load()


def fields(**overrides):
    base = {
        "ISSUE": "#1451", "OWNER": "@BunsDev; claim issue-1451", "ENVIRONMENT": "local macOS worktree",
        "BRANCH": "feat/1451-cloud-maintainer-skill", "FULL_SHA": SHA, "PR": "none",
        "COMPLETED": "none", "CHECKS": "`bash scripts/dev-verify.sh --quick` -> Local quick result: 0",
        "DECISIONS": "none", "BLOCKERS": "none", "NEXT": "open the PR",
    }
    base.update(overrides)
    return {key: value for key, value in base.items() if value is not None}


def errors_for(**overrides):
    return checkpoint.validate(checkpoint.render(fields(**overrides)))[1]


def cli(*args, stdin=None):
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        code = checkpoint.main(list(args))
    return code, out.getvalue()


class SchemaTests(unittest.TestCase):
    def test_real_checkpoint_restated_in_the_schema_validates(self):
        text = (FIXTURES / "canonical-1450-b3-merged.md").read_text()
        parsed, errors, stale = checkpoint.validate(text)
        self.assertEqual((errors, stale), ([], []))
        self.assertEqual(parsed["FULL_SHA"], SHA)
        self.assertEqual(parsed["NEXT"].splitlines()[1], "feed the matrix result into the #1443 parity audit")

    def test_render_round_trips_multiline_values_in_canonical_order(self):
        text = checkpoint.render(fields(CHECKS="first -> 0\nsecond -> 1", FILES="a.py"))
        parsed, errors, _ = checkpoint.validate(text)
        self.assertEqual(errors, [])
        self.assertEqual(parsed["CHECKS"], "first -> 0\nsecond -> 1")
        self.assertEqual(list(parsed), list(checkpoint.KEYS))

    def test_every_required_field_is_named_when_missing(self):
        for key in checkpoint.REQUIRED:
            with self.subTest(key=key):
                self.assertIn(f"missing required field {key}", errors_for(**{key: None}))

    def test_short_and_noncanonical_shas_are_rejected(self):
        self.assertIn("FULL_SHA is a 8-character short SHA; record the full 40-character commit",
                      errors_for(FULL_SHA=SHA[:8]))
        self.assertIn("FULL_SHA must be exactly 40 lowercase hex characters", errors_for(FULL_SHA=SHA.upper()))
        self.assertIn("FULL_SHA must be exactly 40 lowercase hex characters", errors_for(FULL_SHA=SHA + "0"))

    def test_none_is_an_answer_only_where_absence_is_real_state(self):
        for key in ("OWNER", "ENVIRONMENT", "CHECKS", "NEXT", "ISSUE"):
            with self.subTest(key=key):
                self.assertIn(f"{key} cannot be 'none'", errors_for(**{key: "none"}))
        self.assertEqual(errors_for(PR="none", COMPLETED="none", DECISIONS="none", BLOCKERS="none"), [])

    def test_identity_fields_reject_free_text(self):
        self.assertIn("ISSUE must start with an issue reference such as #1451", errors_for(ISSUE="C1 skill"))
        self.assertIn("PR must be none, #<number> or a pull request URL", errors_for(PR="pending"))
        self.assertIn("BRANCH must be a single branch name or '(detached HEAD)'", errors_for(BRANCH="feat/x pushed"))
        self.assertEqual(errors_for(BRANCH="(detached HEAD)", PR="https://github.com/OpenCoven/coven/pull/1482"), [])

    def test_block_structure_is_closed(self):
        one = checkpoint.render(fields())
        self.assertEqual(checkpoint.validate("no block here")[1], ["no ```checkpoint block found"])
        self.assertIn("2 ```checkpoint blocks found", checkpoint.validate(one + one)[1][0])
        self.assertIn("unknown field STATUS", " ".join(checkpoint.validate(one.replace("PR:", "STATUS: x\nPR:"))[1]))
        self.assertIn("duplicate field PR", " ".join(checkpoint.validate(one.replace("PR:", "PR: #1\nPR:"))[1]))

    def test_credentials_and_home_paths_are_refused_without_being_echoed(self):
        sample = "".join(["gh", "p_", "Q" * 36])
        errors = errors_for(BLOCKERS=f"push failed for {sample}")
        self.assertIn("checkpoint contains a github token; remove it before posting", errors)
        self.assertNotIn(sample, " ".join(errors))
        self.assertIn("checkpoint contains a home directory path; remove it before posting",
                      errors_for(OWNER="@BunsDev in " + "/" + "Users/someone/coven"))

    def test_stale_sha_is_reported_against_the_live_head(self):
        text = checkpoint.render(fields())
        _, errors, stale = checkpoint.validate(text, head=OTHER_SHA)
        self.assertEqual(errors, [])
        self.assertEqual(stale, [f"stale: checkpoint FULL_SHA {SHA} but the live head is {OTHER_SHA}; "
                                 "read the new commits before continuing"])
        with tempfile.NamedTemporaryFile("w", suffix=".md") as handle:
            handle.write(text)
            handle.flush()
            self.assertEqual(cli("validate", handle.name, "--head", OTHER_SHA)[0], 3)
            self.assertEqual(cli("validate", handle.name, "--head", SHA)[0], 0)

    def test_blocked_handoff_checkpoint_carries_its_recovery(self):
        handoff = fields(PR="none (publication blocked)",
                         BLOCKERS="GraphQL rate limit; REST still available",
                         NEXT="gh api -X POST repos/OpenCoven/coven/pulls -f head=feat/x -f base=main -F body=@pr.md")
        self.assertEqual(checkpoint.validate(checkpoint.render(handoff))[1], [])
        self.assertIn("NEXT cannot be 'none'", errors_for(BLOCKERS="no remote", NEXT="none"))


class LegacyAuditTests(unittest.TestCase):
    # The fixtures are real issue comments with their field labels intact;
    # run links are reduced to run IDs, which the secret scan's entropy rule
    # would otherwise flag.
    def test_real_free_form_checkpoints_report_exactly_what_is_missing(self):
        expected = {
            "legacy-1450-b3-merged.md": (["COMPLETED", "CHECKS", "DECISIONS", "BLOCKERS"], [SHA, OTHER_SHA]),
            "legacy-1446-policy-pass.md": (["COMPLETED", "CHECKS", "DECISIONS", "BLOCKERS"],
                                           ["3124745f90827648482802334f9e41141fc2c4f1"]),
        }
        for name, (missing, shas) in expected.items():
            with self.subTest(name=name):
                text = (FIXTURES / name).read_text()
                report = checkpoint.audit(text)
                self.assertEqual(report["missing"], missing)
                self.assertEqual(report["full_sha_values"], shas)
                self.assertFalse(report["canonical_block"])
                self.assertEqual(checkpoint.validate(text)[1], ["no ```checkpoint block found"])


NOW = dt.datetime(2026, 10, 24, 12, 0, tzinfo=dt.timezone.utc)


def state(**overrides):
    base = {
        "issue": {"number": 311, "state": "open", "assignees": ["BunsDev"]},
        "environment": "local macOS worktree",
        "agent": "buns@coven-311-output-polish",
        "comments": [], "pulls": [], "remote_branches": [], "claims": [],
    }
    base.update(overrides)
    return base


def kinds(findings):
    return [(finding["kind"], finding["blocking"]) for finding in findings]


def posted(hours_ago, **overrides):
    when = (NOW - dt.timedelta(hours=hours_ago)).isoformat().replace("+00:00", "Z")
    return {"created_at": when, "html_url": "https://github.com/OpenCoven/coven/issues/311#c",
            "body": "progress\n\n" + checkpoint.render(fields(ISSUE="#311", BRANCH="fix/311-output-polish", **overrides))}


class ConflictTests(unittest.TestCase):
    def test_duplicate_prs_are_found_behind_divergent_branch_names(self):
        pulls = [
            {"number": 1, "state": "open", "title": "fix: output polish", "body": "", "head": {"ref": "fix/output-polish-311"}},
            {"number": 2, "state": "open", "title": "Polish output (#311)", "body": "", "head": {"ref": "fix/output-polish"}},
            {"number": 3, "state": "closed", "merged_at": None, "title": "x", "body": "Refs #311", "head": {"ref": "fix/x"}},
            {"number": 4, "state": "open", "title": "unrelated #3110 and 1311", "body": "", "head": {"ref": "fix/3112"}},
        ]
        findings = checkpoint.decide(state(pulls=pulls), now=NOW)
        self.assertEqual(kinds(findings)[:3], [("open-pr", True), ("open-pr", True), ("closed-pr", False)])
        self.assertNotIn("#4", " ".join(finding["detail"] for finding in findings))
        self.assertIn("matches in branch", findings[0]["detail"])
        self.assertIn("matches in title", findings[1]["detail"])

    def test_a_pushed_branch_without_a_pr_is_surfaced(self):
        branches = [{"ref": "refs/heads/fix/311-output-polish", "sha": SHA}, {"ref": "refs/heads/main", "sha": OTHER_SHA}]
        findings = checkpoint.decide(state(remote_branches=branches), now=NOW)
        self.assertIn(("remote-branch", False), kinds(findings))

    def test_local_claims_block_only_other_agents_in_this_checkout(self):
        other = [{"branch": "issue-311", "agent": "buns@coven-other", "state": "active"}]
        mine = [{"branch": "issue-311", "agent": "buns@coven-311-output-polish", "state": "active"}]
        expired = [{"branch": "issue-311", "agent": "buns@coven-other", "state": "expired"}]
        self.assertIn(("local-claim", True), kinds(checkpoint.decide(state(claims=other), now=NOW)))
        for claims in (mine, expired):
            self.assertNotIn("local-claim", [kind for kind, _ in kinds(checkpoint.decide(state(claims=claims), now=NOW))])

    def test_an_empty_local_registry_never_means_unowned(self):
        # Another VM shares no git common directory, so its claim is invisible
        # here; its recent checkpoint is the only ownership signal.
        elsewhere = [posted(0.5, ENVIRONMENT="Codex Cloud task, managed VM")]
        self.assertEqual(kinds(checkpoint.decide(state(comments=elsewhere), now=NOW))[0],
                         ("recent-checkpoint-elsewhere", True))
        older = [posted(30, ENVIRONMENT="Codex Cloud task, managed VM")]
        self.assertEqual(kinds(checkpoint.decide(state(comments=older), now=NOW))[0], ("checkpoint", False))
        same = [posted(0.5)]
        self.assertEqual(kinds(checkpoint.decide(state(comments=same), now=NOW))[0], ("checkpoint", False))
        none = checkpoint.decide(state(), now=NOW)
        self.assertEqual(kinds(none), [("no-checkpoint", False)])
        self.assertIn("says nothing about other machines", none[0]["detail"])

    def test_resuming_reconciles_a_moved_branch_head(self):
        comments = [posted(30)]
        branches = [{"ref": "refs/heads/fix/311-output-polish", "sha": OTHER_SHA}]
        findings = checkpoint.decide(state(comments=comments, remote_branches=branches), now=NOW)
        self.assertIn(("stale-checkpoint", False), kinds(findings))
        self.assertIn(f"is now {OTHER_SHA}, not {SHA}", findings[-1]["detail"])

    def test_only_the_newest_valid_checkpoint_counts(self):
        broken = posted(0.1, FULL_SHA=SHA[:7], ENVIRONMENT="Codex Cloud task")
        findings = checkpoint.decide(state(comments=[posted(30), broken]), now=NOW)
        self.assertEqual(kinds(findings)[0], ("checkpoint", False))

    def test_conflicts_cli_exits_nonzero_on_a_blocking_finding(self):
        blocking = state(pulls=[{"number": 9, "state": "open", "title": "x", "body": "Closes #311", "head": {"ref": "y"}}])
        with tempfile.NamedTemporaryFile("w", suffix=".json") as handle:
            json.dump(blocking, handle)
            handle.flush()
            code, output = cli("conflicts", "--state", handle.name)
        self.assertEqual(code, 4)
        self.assertIn("BLOCK open-pr: #9 (open) y matches in body", output)


class Fake:
    def __init__(self, responses):
        self.responses = responses
        self.calls = []

    def __call__(self, argv):
        self.calls.append(argv)
        code, stdout, stderr = self.responses.get(" ".join(argv), (1, "", "unexpected"))
        return subprocess.CompletedProcess(argv, code, stdout, stderr)


class GatherTests(unittest.TestCase):
    def responses(self):
        return {
            "gh api repos/OpenCoven/coven/issues/311": (0, json.dumps({"state": "open", "assignees": [{"login": "BunsDev"}]}), ""),
            "gh api repos/OpenCoven/coven/issues/311/comments?per_page=100": (0, "[]", ""),
            "gh api repos/OpenCoven/coven/pulls?state=all&per_page=100": (0, "[]", ""),
            "git rev-parse --show-toplevel": (0, "/tmp/coven-311-output-polish\n", ""),
            "git ls-remote --heads origin": (0, f"{SHA}\trefs/heads/fix/311-x\n", ""),
            "coven claim status": (0, "branch agent state expires\nissue-311 buns@coven-a active 2026-10-24T08:00:00Z\n", ""),
        }

    def test_gather_reads_rest_only_and_parses_local_state(self):
        fake = Fake(self.responses())
        result = checkpoint.gather("OpenCoven/coven", 311, "local", runner=fake)
        self.assertEqual(result["issue"]["assignees"], ["BunsDev"])
        self.assertEqual(result["remote_branches"], [{"sha": SHA, "ref": "refs/heads/fix/311-x"}])
        self.assertEqual(result["claims"], [{"branch": "issue-311", "agent": "buns@coven-a", "state": "active"}])
        self.assertTrue(result["agent"].endswith("@coven-311-output-polish"))
        for argv in fake.calls:
            self.assertNotIn("graphql", argv)
            self.assertNotIn("-X", argv)

    def test_a_failed_read_names_the_path_but_not_its_stderr(self):
        sample = "".join(["gh", "o_", "Z" * 36])
        responses = self.responses()
        responses["gh api repos/OpenCoven/coven/issues/311"] = (1, "", f"HTTP 401 for {sample}")
        with self.assertRaises(RuntimeError) as raised:
            checkpoint.gather("OpenCoven/coven", 311, "local", runner=Fake(responses))
        self.assertEqual(str(raised.exception), "gh api repos/OpenCoven/coven/issues/311 failed (exit 1)")

    def test_render_from_git_records_a_detached_head(self):
        with tempfile.TemporaryDirectory() as root:
            def git(*args):
                return subprocess.run(["git", "-C", root, *args], check=True, capture_output=True, text=True).stdout
            git("init", "-q")
            git("-c", "user.name=Fixture", "-c", "user.email=fixture@users.noreply.github.com",
                "-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "fixture")
            head = git("rev-parse", "HEAD").strip()
            git("checkout", "-q", "--detach")
            argv = ["python3", str(HERE / "checkpoint.py"), "render", "--from-git"] + [
                f"{key}={value}" for key, value in fields(BRANCH=None, FULL_SHA=None).items()]
            result = subprocess.run(argv, cwd=root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("BRANCH: (detached HEAD)", result.stdout)
        self.assertIn(f"FULL_SHA: {head}", result.stdout)


if __name__ == "__main__":
    unittest.main()
