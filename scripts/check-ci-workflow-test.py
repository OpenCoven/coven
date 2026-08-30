#!/usr/bin/env python3
from __future__ import annotations

import json
import pathlib
import re
import unittest

CI_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'ci.yml'
RELEASE_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-npm.yml'
RELEASE_GITHUB_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-github.yml'
RELEASE_STRESS_WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / '.github' / 'workflows' / 'release-stress.yml'
REQUIRED_CHECKS_MANIFEST = pathlib.Path(__file__).resolve().parents[1] / 'scripts' / 'release-required-checks.json'
CACHE_SHA = "55cc8345863c7cc4c66a329aec7e433d2d1c52a9"
SETUP_NODE_SHA = "820762786026740c76f36085b0efc47a31fe5020"
CI_TEXT = CI_WORKFLOW.read_text(encoding='utf-8')
RELEASE_TEXT = RELEASE_WORKFLOW.read_text(encoding='utf-8')
RELEASE_GITHUB_TEXT = RELEASE_GITHUB_WORKFLOW.read_text(encoding='utf-8')
MANIFEST = json.loads(REQUIRED_CHECKS_MANIFEST.read_text(encoding='utf-8'))


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
            4,
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
        self.assertIn(
            'needs: [build-platform, npm-dry-run, performance-baseline, verify-tag, exact-source-gate]',
            RELEASE_TEXT,
        )

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

    # --- Exact source acceptance (issue #805) ---------------------------------

    def _job_block(self, text: str, job_id: str) -> str:
        start = text.find(f"\n  {job_id}:\n")
        self.assertGreaterEqual(start, 0, f"job {job_id} not found in workflow")
        rest = text[start + 1:]
        match = re.search(r"\n  [a-zA-Z0-9_-]+:\n", rest[len(f"{job_id}:\n"):])
        end = start + 1 + len(f"{job_id}:\n") + (match.start() if match else len(rest))
        return text[start + 1:end]

    def test_release_npm_gates_publication_on_exact_source_commit(self) -> None:
        gate_block = self._job_block(RELEASE_TEXT, "exact-source-gate")
        self.assertIn("name: Verify exact source commit checks", gate_block)
        self.assertIn("checks: read", gate_block)
        self.assertIn("actions: read", gate_block)
        self.assertIn("needs: [verify-tag]", gate_block)
        self.assertIn("scripts/verify-release-commit-gate.mjs verify", gate_block)
        self.assertIn("scripts/release-required-checks.json", gate_block)
        self.assertIn("needs.verify-tag.outputs.head_sha", gate_block)
        self.assertIn("release-commit-gate-receipt.json", gate_block)
        self.assertIn("if-no-files-found: error", gate_block)
        self.assertIn(
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            gate_block,
        )
        # The registry-mutating job must depend on the gate: a red, missing,
        # stale, or skipped required check on the exact tagged commit blocks
        # npm publication.
        publish_block = self._job_block(RELEASE_TEXT, "npm-publish")
        self.assertIn(
            "needs: [build-platform, npm-dry-run, performance-baseline, verify-tag, exact-source-gate]",
            publish_block,
        )
        # The gate consumes the tag's target commit produced by tag verification.
        verify_tag_block = self._job_block(RELEASE_TEXT, "verify-tag")
        self.assertIn("head_sha: ${{ steps.tag-target.outputs.tagged_commit_sha }}", verify_tag_block)
        self.assertIn('id: tag-target', verify_tag_block)
        self.assertIn('echo "tagged_commit_sha=$TAGGED_COMMIT_SHA" >> "$GITHUB_OUTPUT"', verify_tag_block)

    def test_release_github_revalidates_exact_source_commit(self) -> None:
        self.assertIn("name: Verify exact source commit required checks", RELEASE_GITHUB_TEXT)
        self.assertIn("--commit-sha \"${{ steps.source.outputs.head_sha }}\"", RELEASE_GITHUB_TEXT)
        self.assertIn("--tag-object-sha \"${{ steps.source.outputs.tag_object_sha }}\"", RELEASE_GITHUB_TEXT)
        self.assertIn("scripts/release-required-checks.json", RELEASE_GITHUB_TEXT)
        self.assertIn("release-commit-gate-receipt.json", RELEASE_GITHUB_TEXT)
        self.assertIn(
            "actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
            RELEASE_GITHUB_TEXT,
        )
        publish_block = self._job_block(RELEASE_GITHUB_TEXT, "publish-release")
        self.assertIn("checks: read", publish_block)

    def test_release_signer_controls_remain_wired(self) -> None:
        # Preserve the signed-tag / trusted-signer mechanisms: the gate binds
        # publication to the commit those controls attest.
        self.assertIn("git verify-tag", RELEASE_TEXT)
        self.assertIn("NPM_RELEASE_ALLOWED_SIGNERS", RELEASE_TEXT)
        self.assertIn(".verification.verified", RELEASE_TEXT)

    def test_ci_runs_the_exact_commit_gate_contract_tests(self) -> None:
        self.assertIn("node --test scripts/verify-release-commit-gate-test.mjs", CI_TEXT)

    def test_required_checks_manifest_matches_ci_job_names(self) -> None:
        # The manifest is the single source of truth for release required-check
        # names, so every entry must stay bound to a real CI job (and its
        # display name). Renames must update the manifest and this contract
        # together.
        self.assertEqual(MANIFEST["schema"], "coven.release-required-checks/v1")
        self.assertIn("strict_checks", MANIFEST)
        self.assertIn("routed_checks", MANIFEST)
        strict_names = [entry["name"] for entry in MANIFEST["strict_checks"]]
        routed_names = [entry["name"] for entry in MANIFEST["routed_checks"]]
        self.assertTrue(strict_names)
        self.assertFalse(set(strict_names) & set(routed_names))
        for entry in MANIFEST["strict_checks"] + MANIFEST["routed_checks"]:
            with self.subTest(check=entry["name"], job=entry["job_id"]):
                block = self._job_block(CI_TEXT, entry["job_id"])
                matrix_match = re.search(r"name: ([^\n]*\$\{\{ matrix\.npm-target \}\}[^\n]*)", block)
                if matrix_match:
                    prefix = matrix_match.group(1).split("${{")[0].strip()
                    if prefix.endswith("("):
                        prefix = prefix[:-1].strip()
                    targets = set(re.findall(r"npm-target: ([a-z0-9-]+)", block))
                    self.assertTrue(targets, f"job {entry['job_id']} matrix targets not found")
                    expanded = {f"{prefix} ({target})" for target in targets}
                    names_for_job = {
                        e["name"]
                        for e in MANIFEST["strict_checks"] + MANIFEST["routed_checks"]
                        if e["job_id"] == entry["job_id"]
                    }
                    self.assertEqual(names_for_job, expanded)
                else:
                    self.assertIn(f"name: {entry['name']}", block)

    def test_gate_script_consumes_manifest_source_workflow(self) -> None:
        gate_script = (pathlib.Path(__file__).resolve().parents[1] / 'scripts' / 'verify-release-commit-gate.mjs').read_text(encoding='utf-8')
        self.assertIn("manifest.source_workflow", gate_script)
        self.assertIn("MANIFEST_SCHEMA = 'coven.release-required-checks/v1'", gate_script)
        self.assertIn("RECEIPT_SCHEMA = 'coven.release-commit-gate-receipt/v1'", gate_script)


if __name__ == '__main__':
    raise SystemExit(unittest.main())
