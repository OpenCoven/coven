#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import pathlib
import unittest

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
            ]
        )

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [])

    def test_lockfile_integrity_hashes_do_not_trigger_high_entropy(self) -> None:
        digest = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        text = f"    resolution: {{integrity: sha512-{digest}}}\n"

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [])

    def test_lockfile_still_reports_explicit_secret_patterns(self) -> None:
        key_name = "api" + "_key"
        secret_value = "S" * 24
        text = f"    {key_name}: {secret_value}\n"

        hits = check_secrets.scan_text(text, "packages/openclaw-coven/pnpm-lock.yaml")

        self.assertEqual(hits, [("packages/openclaw-coven/pnpm-lock.yaml", 1, "generic_assignment")])

    def test_lockfiles_are_not_excluded_from_scanning(self) -> None:
        self.assertFalse(check_secrets.is_excluded_path("packages/openclaw-coven/pnpm-lock.yaml"))


if __name__ == "__main__":
    unittest.main()
