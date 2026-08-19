#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import subprocess
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).with_name("check-workflows.sh")
TEXT = SCRIPT.read_text(encoding="utf-8")


class CheckWorkflowsScriptTests(unittest.TestCase):
    def test_pins_actionlint_version(self) -> None:
        self.assertIn("ACTIONLINT_VERSION=1.7.12", TEXT)

    def test_includes_expected_checksums(self) -> None:
        for checksum in [
            "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
            "aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f",
            "5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644",
        ]:
            self.assertIn(checksum, TEXT)

    def test_print_version_outputs_exact_version_without_downloading(self) -> None:
        completed = subprocess.run(
            [str(SCRIPT), "--print-version"],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.stdout, "1.7.12\n")
        self.assertEqual(completed.stderr, "")

    def test_script_uses_safe_shell_settings(self) -> None:
        self.assertIn("set -euo pipefail", TEXT)

    def test_script_supports_required_platforms(self) -> None:
        for platform in ["Linux-x86_64", "Darwin-arm64", "Darwin-x86_64"]:
            self.assertIn(platform, TEXT)
        self.assertIn("unsupported platform", TEXT)

    def test_script_downloads_exact_release_archive_and_verifies_checksum(self) -> None:
        self.assertIn("https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/", TEXT)
        self.assertIn("shasum -a 256 --check -", TEXT)
        self.assertIn("curl -fsSL", TEXT)

    def test_script_extracts_only_actionlint_and_cleans_up(self) -> None:
        self.assertIn("tar -xzf", TEXT)
        self.assertIn("actionlint", TEXT)
        self.assertIn("trap cleanup EXIT", TEXT)

    def test_script_runs_actionlint_color(self) -> None:
        self.assertIn('"$workdir/actionlint" -color', TEXT)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
