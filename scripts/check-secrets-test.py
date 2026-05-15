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

    def test_base64_like_values_still_trigger_high_entropy(self) -> None:
        token = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        text = f"value: {token}\n"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "high_entropy")])


if __name__ == "__main__":
    unittest.main()
