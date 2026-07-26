#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import pathlib
import unittest

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
