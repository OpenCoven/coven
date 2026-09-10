#!/usr/bin/env python3
from __future__ import annotations

import pathlib
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
            'node --test scripts/package-automations-authority-profile.test.mjs',
            'node --test scripts/release-stress-test.mjs',
            'automations-authority-profile-bundle',
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
        pull_request_job = CI_TEXT.split("\n  npm-onboarding-pr:\n", 1)[1].split(
            "\n  npm-onboarding-main:\n", 1
        )[0]
        self.assertIn("npm-target: linux-x64", pull_request_job)
        self.assertIn("npm-target: windows", pull_request_job)


    def test_ci_sets_up_node_for_release_workflow_policy_tests(self) -> None:
        self.assertIn(f"actions/setup-node@{SETUP_NODE_SHA}", CI_TEXT)

    def test_all_platforms_exercise_feature_enabled_threads_daemon_journeys(self) -> None:
        command = "cargo test --locked -p coven-cli --test threads_e2e --features threads-test-clock"
        for job, next_job in [
            ("rust-test-linux", "rust-test-windows"),
            ("rust-test-windows", "rust-test-macos"),
            ("rust-test-macos", "afs-mount-linux"),
        ]:
            block = CI_TEXT.split(f"\n  {job}:\n", 1)[1].split(f"\n  {next_job}:\n", 1)[0]
            self.assertIn(command, block)
        harness = CI_WORKFLOW.parents[2] / "crates/coven-cli/tests/threads_e2e.rs"
        self.assertNotIn("#![cfg(unix)]", harness.read_text(encoding="utf-8"))

    def test_windows_threads_journeys_retain_failure_evidence(self) -> None:
        windows = CI_TEXT.split("\n  rust-test-windows:\n", 1)[1].split(
            "\n  rust-test-macos:\n", 1
        )[0]
        self.assertIn("name: Upload Threads daemon evidence", windows)
        self.assertIn("if: ${{ always() }}", windows)
        job_config, steps = windows.split("\n    steps:\n", 1)
        self.assertIn(
            "COVEN_THREADS_E2E_ARTIFACT_ROOT: ${{ github.workspace }}/../threads-e2e-${{ github.run_id }}-${{ github.run_attempt }}",
            job_config,
        )
        self.assertNotIn("COVEN_THREADS_E2E_ARTIFACT_ROOT:", steps)
        self.assertNotIn("runner.", job_config)
        self.assertIn("path: ${{ env.COVEN_THREADS_E2E_ARTIFACT_ROOT }}", steps)
        self.assertNotIn("path: target/e2e-artifacts/", steps)
        upload = steps.split("- name: Upload Threads daemon evidence\n", 1)[1]
        self.assertIn("if: ${{ always() }}", upload)
        self.assertIn("if-no-files-found: error", upload)
        self.assertIn("retention-days: 14", windows)

    def test_windows_has_budget_for_both_threads_build_profiles(self) -> None:
        windows = CI_TEXT.split("\n  rust-test-windows:\n", 1)[1].split(
            "\n  rust-test-macos:\n", 1
        )[0]
        self.assertIn("\n    timeout-minutes: 30\n", windows)
        for step in ["Exercise isolated Threads clock feature", "Exercise real-daemon Threads journeys"]:
            self.assertIn(f"- name: {step}\n        if: ${{{{ !cancelled() }}}}", windows)
        self.assertNotIn("continue-on-error: true", windows)

    def test_native_link_dependency_installs_use_scoped_apt_helper(self) -> None:
        release_stress_text = RELEASE_STRESS_WORKFLOW.read_text(encoding='utf-8')
        for workflow_text in [CI_TEXT, RELEASE_TEXT, release_stress_text]:
            self.assertNotIn("sudo apt-get update && sudo apt-get install", workflow_text)
            self.assertNotIn(
                "apt-get install -y --no-install-recommends libopenblas-dev",
                workflow_text,
            )

        expected_invocations = 7 + 3 + 1
        actual_invocations = (
            CI_TEXT.count("bash scripts/install-native-link-dependencies.sh")
            + RELEASE_TEXT.count("bash scripts/install-native-link-dependencies.sh")
            + release_stress_text.count("bash scripts/install-native-link-dependencies.sh")
        )
        self.assertEqual(actual_invocations, expected_invocations)
        self.assertIn("python3 scripts/install-native-link-dependencies-test.py", CI_TEXT)

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
