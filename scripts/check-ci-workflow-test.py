#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import re
import unittest

CI_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'ci.yml'
RELEASE_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-npm.yml'
RELEASE_GITHUB_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-github.yml'
RELEASE_STRESS_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-stress.yml'
CACHE_SHA = "55cc8345863c7cc4c66a329aec7e433d2d1c52a9"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"
CI_TEXT = CI_WORKFLOW.read_text(encoding='utf-8')
RELEASE_TEXT = RELEASE_WORKFLOW.read_text(encoding='utf-8')
RELEASE_GITHUB_TEXT = RELEASE_GITHUB_WORKFLOW.read_text(encoding='utf-8')


def ci_job_block(job: str) -> str:
    """Return the ci.yml text belonging to one top-level job.

    Kept deliberately text-based: none of the repository's guards depend on
    PyYAML, and adding that dependency for one assertion would make the whole
    policy job harder to run.
    """
    marker = f"\n  {job}:\n"
    if marker not in CI_TEXT:
        raise AssertionError(f'ci.yml has no job named {job!r}')
    remainder = CI_TEXT[CI_TEXT.index(marker) + 1 :]
    following_job = re.search(r"\n  [A-Za-z0-9_-]+:\n", remainder)
    return remainder[: following_job.start()] if following_job else remainder


class CheckCiWorkflowTests(unittest.TestCase):
    def test_ci_routes_pull_requests_through_gate(self) -> None:
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", CI_TEXT)
        self.assertIn('git diff --name-only --diff-filter=ACMRD "$range"', CI_TEXT)
        self.assertIn("\n  changes:\n", CI_TEXT)
        self.assertIn("\n  pr-gate:\n", CI_TEXT)
        self.assertIn("name: PR gate", CI_TEXT)
        self.assertIn(
            "if: ${{ always() && !cancelled() && github.event_name == 'pull_request' }}",
            CI_TEXT,
        )

    def test_ci_contains_expected_jobs(self) -> None:
        for job_name in [
            'rust-lint-linux',
            'rust-test-linux',
            'rust-test-windows',
            'rust-test-macos',
            'afs-mount-linux',
            'afs-mount-macos',
        ]:
            self.assertIn(f"\n  {job_name}:\n", CI_TEXT)
        self.assertNotIn("\n  rust:\n", CI_TEXT)

    def test_ci_uses_expected_timeouts_and_cache_policy(self) -> None:
        self.assertGreaterEqual(CI_TEXT.count('timeout-minutes: 20'), 10)
        self.assertIn(f"actions/cache@{CACHE_SHA}", CI_TEXT)
        self.assertNotIn('actions/cache@v', CI_TEXT)

    def test_ci_runs_expected_workflow_checks(self) -> None:
        for needle in [
            'python3 scripts/classify-ci-changes-test.py',
            'python3 scripts/check-workflows-test.py',
            'python3 scripts/check-ci-workflow-test.py',
            'python3 scripts/check-docs-ownership-test.py',
            'python3 scripts/check-docs-ownership.py --range',
            'scripts/check-workflows.sh',
            'node --test scripts/package-github-release-test.mjs',
            'node --test scripts/release-stress-test.mjs',
            "needs.changes.outputs.docs_only != 'true'",
            'npm-onboarding-pr',
            "github.event_name == 'push'",
            'performance-baseline',
            'name: CLI performance baseline',
            "if: ${{ github.event_name == 'push' }}",
        ]:
            self.assertIn(needle, CI_TEXT)
        self.assertIn("\n  npm-onboarding-pr:\n", CI_TEXT)
        self.assertIn("\n  npm-onboarding-main:\n", CI_TEXT)
        self.assertIn(
            "if: ${{ github.event_name == 'pull_request' && needs.changes.outputs.npm_packaging == 'true' }}",
            CI_TEXT,
        )

    def test_benchmark_harness_tests_run_outside_the_push_only_job(self) -> None:
        command = (
            'node --test scripts/benchmark-cli.test.mjs '
            'scripts/benchmark-chaos.test.mjs'
        )
        baseline = ci_job_block('performance-baseline')
        policy_guard = ci_job_block('policy-guard')

        # `performance-baseline` runs the harness tests too, but it is gated on
        # push, so its copy cannot fail a pull request that breaks the harness.
        self.assertIn(command, baseline)
        self.assertIn("if: ${{ github.event_name == 'push' }}", baseline)

        # `policy-guard` is the coverage that actually reaches a pull request,
        # so the command must be there and the job must carry no event gate of
        # its own. Asserting the command against the whole file would pass on
        # the push-only copy alone and prove nothing.
        self.assertIn(command, policy_guard)
        job_header = policy_guard.split('    steps:')[0]
        self.assertNotIn('if:', job_header)
        pull_request_job = CI_TEXT.split("\n  npm-onboarding-pr:\n", 1)[1].split(
            "\n  npm-onboarding-main:\n", 1
        )[0]
        self.assertIn("npm-target: linux-x64", pull_request_job)
        self.assertIn("npm-target: windows", pull_request_job)


    def test_ci_sets_up_node_for_release_workflow_policy_tests(self) -> None:
        self.assertIn(f"actions/setup-node@{SETUP_NODE_SHA}", CI_TEXT)

    def test_release_github_workflow_has_expected_trigger_and_permissions(self) -> None:
        self.assertIn("workflow_run:", RELEASE_GITHUB_TEXT)
        self.assertIn("Release npm packages", RELEASE_GITHUB_TEXT)
        self.assertIn("workflow_dispatch:", RELEASE_GITHUB_TEXT)
        self.assertIn("source_run_attempt:", RELEASE_GITHUB_TEXT)
        self.assertIn("actions: read", RELEASE_GITHUB_TEXT)
        self.assertIn("contents: write", RELEASE_GITHUB_TEXT)
        self.assertNotIn("id-token: write", RELEASE_GITHUB_TEXT)
        self.assertEqual(
            RELEASE_GITHUB_TEXT.count(
                "          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}"
            ),
            3,
        )
        self.assertNotIn(
            "          GITHUB_TOKEN: ${{ secrets.GITHUB_TOKEN }}",
            RELEASE_GITHUB_TEXT,
        )
        self.assertIn("cancel-in-progress: false", RELEASE_GITHUB_TEXT)
        self.assertIn(f"actions/setup-node@{SETUP_NODE_SHA}", RELEASE_GITHUB_TEXT)
        self.assertIn("actions/download-artifact@3e5f45b2cfb9172054b4087a40e8e0b5a5461e7c", RELEASE_GITHUB_TEXT)
        self.assertIn("github.event.workflow_run.run_attempt || inputs.source_run_attempt", RELEASE_GITHUB_TEXT)
        self.assertIn('--source-run-attempt "$SOURCE_RUN_ATTEMPT"', RELEASE_GITHUB_TEXT)
        self.assertIn("verify-source-run-attempt", RELEASE_GITHUB_TEXT)
        self.assertIn('--expected-tag-object-sha "$TAG_OBJECT_SHA"', RELEASE_GITHUB_TEXT)
        self.assertIn('--expected-head-sha "$HEAD_SHA"', RELEASE_GITHUB_TEXT)
        self.assertNotIn("npm publish", RELEASE_GITHUB_TEXT)

    def test_release_includes_performance_baseline_dependency(self) -> None:
        self.assertIn('performance-baseline', RELEASE_TEXT)
        self.assertIn('needs: [build-platform, npm-dry-run, performance-baseline, verify-tag]', RELEASE_TEXT)

    def test_release_stress_workflow_is_bounded_and_uploads_failure_evidence(self) -> None:
        stress_text = RELEASE_STRESS_WORKFLOW.read_text(encoding='utf-8')
        self.assertIn(
            "name: Release stress\n\non:\n  workflow_dispatch:\n\npermissions:",
            stress_text,
        )
        self.assertNotIn("schedule:", stress_text)
        self.assertEqual(stress_text.count("timeout-minutes: 45"), 2)
        self.assertEqual(stress_text.count("timeout-minutes: 42"), 2)
        self.assertIn("--suite unix --iterations 10 --command-timeout-ms 180000", stress_text)
        self.assertIn("--suite windows --iterations 10 --command-timeout-ms 180000", stress_text)
        self.assertEqual(stress_text.count("if: ${{ always() }}"), 2)
        self.assertEqual(stress_text.count("actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a"), 2)

    def test_release_npm_workflow_uses_same_tag_specific_concurrency_without_cancellation(self) -> None:
        self.assertIn(
            "concurrency:\n  group: release-npm-${{ github.ref }}\n  cancel-in-progress: false",
            RELEASE_TEXT,
        )


if __name__ == '__main__':
    raise SystemExit(unittest.main())
