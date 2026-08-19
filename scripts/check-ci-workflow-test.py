#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import unittest

CI_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'ci.yml'
RELEASE_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-npm.yml'
CI_TEXT = CI_WORKFLOW.read_text(encoding='utf-8')
RELEASE_TEXT = RELEASE_WORKFLOW.read_text(encoding='utf-8')


class CheckCiWorkflowTests(unittest.TestCase):
    def test_ci_routes_pull_requests_through_gate(self) -> None:
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", CI_TEXT)
        self.assertIn("\n  changes:\n", CI_TEXT)
        self.assertIn("\n  pr-gate:\n", CI_TEXT)
        self.assertIn("name: PR gate", CI_TEXT)
        self.assertIn("if: ${{ always() && github.event_name == 'pull_request' }}", CI_TEXT)

    def test_ci_contains_expected_jobs(self) -> None:
        for job_name in [
            'rust-lint-linux',
            'rust-test-linux',
            'rust-test-windows',
            'afs-mount-linux',
            'afs-mount-macos',
        ]:
            self.assertIn(f"\n  {job_name}:\n", CI_TEXT)
        self.assertNotIn("\n  rust:\n", CI_TEXT)

    def test_ci_uses_expected_timeouts_and_cache_policy(self) -> None:
        self.assertGreaterEqual(CI_TEXT.count('timeout-minutes: 20'), 10)
        self.assertIn('55cc', CI_TEXT)
        self.assertNotIn('actions/cache@v', CI_TEXT)

    def test_ci_runs_expected_workflow_checks(self) -> None:
        for needle in [
            'python3 scripts/classify-ci-changes-test.py',
            'python3 scripts/check-workflows-test.py',
            'python3 scripts/check-ci-workflow-test.py',
            'scripts/check-workflows.sh',
            "needs.changes.outputs.docs_only != 'true'",
            'npm-onboarding-linux',
            "github.event_name == 'push'",
            'performance-baseline',
            'name: CLI performance baseline',
            "if: ${{ github.event_name == 'push' }}",
        ]:
            self.assertIn(needle, CI_TEXT)

    def test_release_includes_performance_baseline_dependency(self) -> None:
        self.assertIn('performance-baseline', RELEASE_TEXT)
        self.assertIn('needs: [build-platform, npm-dry-run, performance-baseline, verify-tag]', RELEASE_TEXT)


if __name__ == '__main__':
    raise SystemExit(unittest.main())
