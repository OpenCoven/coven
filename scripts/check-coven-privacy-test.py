#!/usr/bin/env python3
from __future__ import annotations

import contextlib
import importlib.util
import io
import pathlib
import subprocess
import unittest
import unittest.mock

SCRIPT = pathlib.Path(__file__).with_name("check-coven-privacy.py")
spec = importlib.util.spec_from_file_location("check_coven_privacy", SCRIPT)
assert spec is not None
check_coven_privacy = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(check_coven_privacy)


def phone_like_sha512_digest() -> str:
    return (
        "sha512-"
        + ("a" * 40)
        + "+"
        + "576"
        + ("b" * 42)
        + "=="
    )


class CovenPrivacyPatternTests(unittest.TestCase):
    def test_scanner_sources_do_not_match_their_own_rules(self) -> None:
        sources = [
            (path.name, path.read_bytes())
            for path in (SCRIPT, pathlib.Path(__file__))
        ]

        self.assertEqual(check_coven_privacy.scan_files(sources), [])

    def test_ci_scans_pull_request_changed_files(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            '--range "${{ github.event.pull_request.base.sha }}...'
            '${{ github.event.pull_request.head.sha }}"',
            workflow,
        )
        self.assertNotIn(
            '${{ github.event.pull_request.base.sha }}...HEAD',
            workflow,
        )

    def test_ci_runs_privacy_guard_unit_tests(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "python scripts/check-coven-privacy-test.py",
            workflow,
        )

    def test_ci_runs_secret_guard_unit_tests(self) -> None:
        workflow = (
            SCRIPT.parents[1] / ".github" / "workflows" / "ci.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "python scripts/check-secrets-test.py",
            workflow,
        )

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
        with unittest.mock.patch.object(
            check_coven_privacy, "git", return_value=b""
        ) as git:
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
        with unittest.mock.patch.object(
            check_coven_privacy, "git", return_value=b""
        ) as git:
            self.assertEqual(check_coven_privacy.changed_files("before..after"), [])

        git.assert_called_once_with(
            "diff",
            "--name-only",
            "--diff-filter=ACMR",
            "-z",
            "before..after",
            text=False,
        )

    def test_git_paths_accept_non_utf8_bytes(self) -> None:
        self.assertEqual(
            check_coven_privacy.nul_paths(b"docs/\xffexample.md\0"),
            ["docs/\udcffexample.md"],
        )

    def test_range_scan_reads_files_from_range_end_revision(self) -> None:
        calls: list[tuple[tuple[object, ...], dict[str, object]]] = []

        def fake_git(*args: str, text: bool = True) -> str | bytes:
            calls.append((args, {"text": text}))
            if args[:2] == ("diff", "--name-only"):
                return b"docs/example.md\0"
            if args == ("show", "after:docs/example.md"):
                return b"safe text"
            raise AssertionError(f"unexpected git call: {args!r}")

        with unittest.mock.patch.object(
            check_coven_privacy, "git", side_effect=fake_git
        ):
            self.assertEqual(
                check_coven_privacy.changed_files("before..after"),
                [("docs/example.md", b"safe text")],
            )
        self.assertEqual(
            calls,
            [
                (
                    ("diff", "--name-only", "--diff-filter=ACMR", "-z", "before..after"),
                    {"text": False},
                ),
                (("show", "after:docs/example.md"), {"text": False}),
            ],
        )

    def test_range_scan_rejects_single_revision(self) -> None:
        with unittest.mock.patch.object(check_coven_privacy, "git") as git:
            with self.assertRaisesRegex(ValueError, r"START\.\.END"):
                check_coven_privacy.changed_files("HEAD^")

        git.assert_not_called()

    def test_range_scan_fails_when_a_changed_file_cannot_be_read(self) -> None:
        def fake_git(*args: str, text: bool = True) -> str | bytes:
            if args[:2] == ("diff", "--name-only"):
                return b"docs/missing.md\0"
            if args == ("show", "after:docs/missing.md"):
                raise subprocess.CalledProcessError(128, ["git", *args])
            raise AssertionError(f"unexpected git call: {args!r}")

        with unittest.mock.patch.object(
            check_coven_privacy, "git", side_effect=fake_git
        ):
            with self.assertRaises(subprocess.CalledProcessError):
                check_coven_privacy.changed_files("before..after")

    def test_private_session_identifier_is_blocked(self) -> None:
        text = ":".join(["agent", "example", "telegram", "direct", "123456789"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "coven_session_key")])

    def test_session_key_suppresses_same_line_messenger_chat_id(self) -> None:
        text = " ".join(
            [
                ":".join(["agent", "example", "telegram", "direct", "123456789"]),
                ":".join(["telegram", "direct", "987654321"]),
            ]
        )

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "coven_session_key")])

    def test_standalone_messenger_chat_id_is_blocked(self) -> None:
        text = ":".join(["telegram", "direct", "123456789"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "messenger_chat_id")])

    def test_invite_or_handoff_url_with_token_is_blocked(self) -> None:
        text = "".join(["https://example.com/in", "vite?to", "ken=abc123"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "invite_or_handoff_url")])

    def test_absolute_home_path_is_blocked(self) -> None:
        text = "/" + "/".join(["Users", "privateuser", "workspace", "memory.md"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "absolute_home_path")])

    def test_absolute_home_path_with_dotted_username_is_blocked(self) -> None:
        text = "/" + "/".join(["Users", "private.user", "workspace", "memory.md"])

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "absolute_home_path")])

    def test_security_docs_match_tokenized_url_rule(self) -> None:
        security = (SCRIPT.parents[1] / "SECURITY.md").read_text(encoding="utf-8")

        self.assertIn("invite/handoff URLs containing tokens", security)

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

    def test_pnpm_integrity_digest_phone_like_substring_is_allowed(self) -> None:
        digest = phone_like_sha512_digest()
        text = f"resolution: {{integrity: {digest}}}"

        hits = check_coven_privacy.scan_text(
            text, "packages/example/pnpm-lock.yaml"
        )

        self.assertEqual(hits, [])

    def test_package_lock_integrity_digest_phone_like_substring_is_allowed(self) -> None:
        digest = phone_like_sha512_digest()
        text = f'"integrity": "{digest}",'

        hits = check_coven_privacy.scan_text(
            text, "packages/example/package-lock.json"
        )

        self.assertEqual(hits, [])

    def test_phone_number_outside_pnpm_integrity_digest_is_blocked(self) -> None:
        digest = phone_like_sha512_digest()
        phone = "+" + "1" + "312" + "555" + "0100"
        text = f"resolution: {{integrity: {digest}}} phone: {phone}"

        hits = check_coven_privacy.scan_text(
            text, "packages/example/pnpm-lock.yaml"
        )

        self.assertEqual(
            hits,
            [("packages/example/pnpm-lock.yaml", 1, "phone_number")],
        )

    def test_invalid_pnpm_integrity_digest_remains_scannable(self) -> None:
        phone = "+" + "1" + "312" + "555" + "0100"
        text = f"integrity: sha512-{phone}"

        hits = check_coven_privacy.scan_text(
            text, "packages/example/pnpm-lock.yaml"
        )

        self.assertEqual(
            hits,
            [("packages/example/pnpm-lock.yaml", 1, "phone_number")],
        )

    def test_pnpm_non_field_integrity_text_remains_scannable(self) -> None:
        digest = phone_like_sha512_digest()
        text = f"note: integrity={digest}"

        hits = check_coven_privacy.scan_text(
            text, "packages/example/pnpm-lock.yaml"
        )

        self.assertEqual(
            hits,
            [("packages/example/pnpm-lock.yaml", 1, "phone_number")],
        )

    def test_integrity_digest_in_regular_file_remains_scannable(self) -> None:
        digest = phone_like_sha512_digest()
        text = f"integrity: {digest}"

        hits = check_coven_privacy.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "phone_number")])

    def test_range_usage_documents_explicit_range_shapes(self) -> None:
        stderr = io.StringIO()

        with contextlib.redirect_stderr(stderr):
            result = check_coven_privacy.usage()

        self.assertEqual(result, 2)
        self.assertIn("--range START..END|START...END", stderr.getvalue())

    def test_explicit_file_scan_rejects_missing_path(self) -> None:
        stderr = io.StringIO()

        with contextlib.redirect_stderr(stderr):
            result = check_coven_privacy.main(
                ["--files", "docs/does-not-exist.md"]
            )

        self.assertEqual(result, 2)
        self.assertIn("missing or not a regular file", stderr.getvalue())

    def test_scan_files_scans_undecodable_bytes(self) -> None:
        private_path = b"/" + b"/".join(
            [b"Users", b"privateuser", b"workspace"]
        )
        phone = b"+" + b"1" + b"415" + b"555" + b"0100"
        hits = check_coven_privacy.scan_files(
            [("docs/example.md", private_path + b"/\xff" + phone)]
        )

        self.assertEqual(
            hits,
            [
                ("docs/example.md", 1, "absolute_home_path"),
                ("docs/example.md", 1, "phone_number"),
            ],
        )

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
