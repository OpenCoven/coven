#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import pathlib
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).with_name("check-secrets.py")
spec = importlib.util.spec_from_file_location("check_secrets", SCRIPT)
assert spec is not None
check_secrets = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(check_secrets)


class SecretGuardLockfileTests(unittest.TestCase):
    def test_lockfile_package_keys_do_not_trigger_high_entropy(self) -> None:
        text = "\n".join(
            [
                "  '@smithy/util-defaults-mode-browser@4.3.49': {}",
                "  '@mariozechner/clipboard-win32-arm64-msvc':",
                "  '@mariozechner/clipboard-linux-riscv64-gnu': 0.3.2",
                '    "node_modules/@rolldown/binding-win32-arm64-msvc": {',
                '    "node_modules/lightningcss-win32-x64-msvc": {',
            ]
        )

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [])

    def test_lockfile_integrity_hashes_do_not_trigger_high_entropy(self) -> None:
        digest = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        text = "\n".join(
            [
                f"    resolution: {{integrity: sha512-{digest}}}",
                f'      "integrity": "sha512-{digest}",',
            ]
        )

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [])

    def test_lockfile_registry_tarball_urls_do_not_trigger_high_entropy(self) -> None:
        text = (
            '      "resolved": '
            '"https://registry.npmjs.org/@rolldown/binding-darwin-arm64/-/binding-darwin-arm64-1.0.0-rc.18.tgz"'
        )

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/package-lock.json")

        self.assertEqual(hits, [])

    def test_lockfile_still_reports_explicit_secret_patterns(self) -> None:
        key_name = "api" + "_key"
        secret_value = "S" * 24
        text = f"    {key_name}: {secret_value}\n"

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [("packages/openclaw-coven/pnpm-lock.yaml", 1, "generic_assignment")])

    def test_opencoven_github_urls_do_not_trigger_high_entropy(self) -> None:
        text = (
            "The canonical brand system lives in "
            "https://github.com/OpenCoven/coven/blob/main/DESIGN.md and "
            "https://github.com/OpenCoven/coven/tree/main/brand."
        )

        hits = check_secrets.scan_text(text, "docs/BRAND.md")

        self.assertEqual(hits, [])

    def test_opencoven_local_worktree_paths_do_not_trigger_high_entropy(self) -> None:
        text = "/Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module"

        hits = check_secrets.scan_text(text, "docs/superpowers/plans/example.md")

        self.assertEqual(hits, [])

    def test_lockfiles_are_not_excluded_from_scanning(self) -> None:
        self.assertFalse(check_secrets.is_excluded_path("packages/openclaw-coven/pnpm-lock.yaml"))

    def test_local_worktree_paths_do_not_trigger_high_entropy(self) -> None:
        text = (
            "cd /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module\n"
            "Expected: /Users/buns/Documents/GitHub/OpenCoven/coven/.worktrees/feat-tui-chat-module"
        )

        hits = check_secrets.scan_text(text, "docs/superpowers/plans/example.md")

        self.assertEqual(hits, [])

    def test_public_repo_links_do_not_trigger_high_entropy(self) -> None:
        text = (
            "[`DESIGN.md`](https://github.com/OpenCoven/coven/blob/main/DESIGN.md)\n"
            "[`brand/`](https://github.com/OpenCoven/coven/tree/main/brand)"
        )

        hits = check_secrets.scan_text(text, "docs/BRAND.md")

        self.assertEqual(hits, [])

    def test_github_advisory_links_do_not_trigger_high_entropy(self) -> None:
        text = (
            "Resolved advisory "
            "https://github.com/advisories/GHSA-rhfx-m35p-ff5j in the release notes."
        )

        hits = check_secrets.scan_text(text, "docs/reference/changelog.md")

        self.assertEqual(hits, [])

    def test_high_entropy_tokens_detected_on_lines_with_advisory_links(self) -> None:
        token = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        text = (
            "Resolved advisory https://github.com/advisories/GHSA-rhfx-m35p-ff5j "
            f"and observed token {token}."
        )

        hits = check_secrets.scan_text(text, "docs/reference/changelog.md")

        self.assertEqual(hits, [("docs/reference/changelog.md", 1, "high_entropy")])

    def test_history_scan_uses_head_for_rev_list_by_default(self) -> None:
        calls: list[tuple[str, ...]] = []

        def fake_sh(*args: str) -> str:
            calls.append(args)
            if args[:3] == ("git", "rev-list", "--objects"):
                return ""
            raise AssertionError(f"unexpected sh call: {args}")

        with mock.patch.object(check_secrets, "sh", side_effect=fake_sh):
            hits = check_secrets.history_blob_hits()

        self.assertEqual(hits, [])
        self.assertEqual(calls, [("git", "rev-list", "--objects", "HEAD")])

    def test_history_scan_uses_supplied_ref_for_rev_list(self) -> None:
        calls: list[tuple[str, ...]] = []

        def fake_sh(*args: str) -> str:
            calls.append(args)
            if args[:3] == ("git", "rev-list", "--objects"):
                return ""
            raise AssertionError(f"unexpected sh call: {args}")

        with mock.patch.object(check_secrets, "sh", side_effect=fake_sh):
            hits = check_secrets.history_blob_hits("origin/main")

        self.assertEqual(hits, [])
        self.assertEqual(calls, [("git", "rev-list", "--objects", "origin/main")])

    def test_base64_like_values_still_trigger_high_entropy(self) -> None:
        token = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        text = f"value: {token}\n"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "high_entropy")])

    def test_screaming_snake_constant_method_call_is_not_a_secret(self) -> None:
        text = (
            "            kind: CAST_QUEST_PHASE_COMPLETED_KIND.to_string(),\n"
            "            kind: CAST_QUEST_COMPLETED_KIND.to_string(),\n"
        )

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/tui/cast/attach.rs")

        self.assertEqual(hits, [])

    def test_long_snake_case_test_function_name_is_not_a_secret(self) -> None:
        text = "    fn non_zero_exit_codes_use_failure_handoff_reason() {\n"

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/tui/cast/quest.rs")

        self.assertEqual(hits, [])

    def test_high_entropy_token_without_identifier_shape_still_trips(self) -> None:
        # No underscores or dots, mixed case + slash + digits — clearly not a
        # programming identifier. Must still be reported even when the line
        # happens to also contain a real identifier-looking token.
        token = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        text = f"// CAST_QUEST_PHASE_COMPLETED_KIND.to_string() => {token}\n"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "high_entropy")])

    def test_identifier_heuristic_rejects_mixed_case_segments(self) -> None:
        # Segments mix upper and lower case within a single segment — not the
        # snake_case / SCREAMING_SNAKE_CASE shape we want to whitelist. The
        # heuristic should return False so the entropy rule still applies.
        self.assertFalse(
            check_secrets.is_programming_identifier_token(
                "MixedCaseToken_AnotherMixedCase_YetMoreMixed_AndAgain_FinalSegment"
            )
        )

    def test_identifier_heuristic_rejects_non_identifier_chars(self) -> None:
        # Tokens containing `/` or `+` are typical of base64/credential blobs.
        self.assertFalse(
            check_secrets.is_programming_identifier_token(
                "abc_def_ghi/jkl_mno_pqr+stu_vwx"
            )
        )

    def test_identifier_heuristic_requires_three_segments(self) -> None:
        # A token with only one underscore (two segments) is too generic to
        # safely whitelist; the entropy rule should still see it.
        self.assertFalse(
            check_secrets.is_programming_identifier_token("supersecret_payloadblob")
        )

    def test_workflow_relative_path_is_not_a_secret(self) -> None:
        # The pre-publish script prints `.github/workflows/release-npm.yml`
        # in its end-of-run hint; the tokenizer reads
        # `github/workflows/release-npm.yml` as one 33-char run.
        text = (
            "  console.log('Next: bump version + tag (vX.Y.Z) to trigger "
            ".github/workflows/release-npm.yml.');\n"
        )

        hits = check_secrets.scan_text(text, "scripts/test-cli-prepublish.mjs")

        self.assertEqual(hits, [])

    def test_identifier_heuristic_rejects_base64_with_single_slash(self) -> None:
        # A real base64 secret with a single `/` separator yields only two
        # segments and segments mix case — both checks must reject it.
        self.assertFalse(
            check_secrets.is_programming_identifier_token(
                "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/EuIqOwPz9RkTlVxCyNmS3HdG7fA"
            )
        )


if __name__ == "__main__":
    unittest.main()
