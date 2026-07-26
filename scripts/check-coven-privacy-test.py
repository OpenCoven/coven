#!/usr/bin/env python3
from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).with_name("check-coven-privacy.py")
spec = importlib.util.spec_from_file_location("check_coven_privacy", SCRIPT)
assert spec is not None
check_coven_privacy = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(check_coven_privacy)


class CovenPrivacyPatternTests(unittest.TestCase):
    def test_ci_scans_pull_request_changed_files(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("scripts/check-coven-privacy.py --range", workflow)

    def test_ci_scans_the_entire_push_range(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn("BEFORE_SHA: ${{ github.event.before }}", workflow)
        self.assertIn("AFTER_SHA: ${{ github.sha }}", workflow)
        self.assertIn('--range "${BEFORE_SHA}..${AFTER_SHA}"', workflow)
        self.assertNotIn("HEAD^...HEAD", workflow)

    def test_ci_scans_the_full_tree_when_push_before_is_unavailable(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn('git cat-file -e "${BEFORE_SHA}^{commit}"', workflow)
        self.assertIn("git hash-object -t tree -w --stdin", workflow)

    def test_staged_scan_includes_renames_and_copies(self) -> None:
        with mock.patch.object(check_coven_privacy, "git", return_value=b"") as git:
            self.assertEqual(check_coven_privacy.staged_files(), [])

        git.assert_called_once_with(
            "diff",
            "--cached",
            "--name-only",
            "--diff-filter=ACMR",
            "-z",
            text=False,
        )

    def test_range_scan_includes_renames_and_copies(self) -> None:
        with mock.patch.object(check_coven_privacy, "git", return_value=b"") as git:
            self.assertEqual(check_coven_privacy.changed_files("before..after"), [])

        git.assert_called_once_with(
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            "-z",
            "before..after",
            text=False,
        )

    def test_private_session_identifier_is_blocked(self) -> None:
        text = ":".join(["agent", "example", "telegram", "direct", "123456789"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "coven_session_key")])

    def test_absolute_home_path_is_blocked(self) -> None:
        text = "/" + "/".join(["Users", "privateuser", "workspace", "memory.md"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "absolute_home_path")])

    def test_runtime_internal_path_is_blocked(self) -> None:
        text = "~/." + "/".join(["coven", "workspaces", "example", "memory.md"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "runtime_internal_path")])

    def test_phone_number_is_blocked(self) -> None:
        text = "+" + "1" + "312" + "555" + "0100"

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "phone_number")])

    def test_international_e164_phone_number_is_blocked(self) -> None:
        text = "+" + "44" + "20" + "7183" + "8750"

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "phone_number")])

    def test_short_e164_phone_number_is_blocked(self) -> None:
        text = "+" + "683" + "1234"

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "phone_number")])

    def test_range_usage_accepts_any_git_revision_range(self) -> None:
        stderr = io.StringIO()

        with contextlib.redirect_stderr(stderr):
            result = check_coven_privacy.usage()

        self.assertEqual(result, 2)
        self.assertIn("--range REVISION_RANGE", stderr.getvalue())
        self.assertNotIn("BASE...HEAD", stderr.getvalue())

    def test_coven_contract_placeholders_are_allowed(self) -> None:
        text = "\n".join(
            [
                "FAMILIAR_ROOT/<familiar-id>/memory/example.md",
                "<familiar-id>:memory/example.md#L1-L2",
                "01JEXAMPLE0000000000000000",
                "~/.coven/memory/",
            ]
        )

        self.assertEqual(check_coven_privacy.scan_text(text, "docs/example.md"), [])


if __name__ == "__main__":
    unittest.main()
