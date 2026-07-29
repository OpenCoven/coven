#!/usr/bin/env python3
from __future__ import annotations

import io
import importlib.util
import pathlib
import subprocess
import tempfile
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

    def test_environment_secret_reads_do_not_trigger_generic_assignment(self) -> None:
        text = '    api_key = os.environ.get("ELEVENLABS_API_KEY")\n'

        hits = check_secrets.scan_text(text, "skills/higgsfield/scripts/elevenlabs_narrate.py")

        self.assertEqual(hits, [])

    def test_markdown_environment_secret_references_do_not_trigger(self) -> None:
        text = "and the header `X-API-Key: $TINYFISH_API_KEY`."

        hits = check_secrets.scan_text(text, "skills/tinyfish-agent-run/SKILL.md")

        self.assertEqual(hits, [])

    def test_literal_secret_assignments_still_trigger_generic_assignment(self) -> None:
        key_name = "api" + "_key"
        secret_value = "S" * 24
        text = f'    {key_name} = "{secret_value}"\n'

        hits = check_secrets.scan_text(text, "docs/example.py")

        self.assertEqual(hits, [("docs/example.py", 1, "generic_assignment")])

    def test_known_fake_private_key_fixture_does_not_trigger(self) -> None:
        text = (
            '    "-----BEGIN PRIVATE KEY-----\\n'
            'fakefakefakefakefakefakefake\\n'
            '-----END PRIVATE KEY-----";'
        )

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/privacy.rs")

        self.assertEqual(hits, [])

    def test_fake_private_key_fixture_does_not_hide_another_header(self) -> None:
        begin = "-----BEGIN " + "PRIVATE KEY-----"
        end = "-----END " + "PRIVATE KEY-----"
        other_begin = "-----BEGIN OPENSSH " + "PRIVATE KEY-----"
        fixture = f'"{begin}\\nfakefakefakefakefakefakefake\\n{end}"'
        text = f"{fixture} {other_begin}"

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/privacy.rs")

        self.assertEqual(
            hits,
            [("crates/coven-cli/src/privacy.rs", 1, "private_key")],
        )

    def test_real_private_key_header_still_triggers(self) -> None:
        text = "-----BEGIN PRIVATE KEY-----\nnot-a-placeholder\n-----END PRIVATE KEY-----"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "private_key")])

    def test_opencoven_github_urls_do_not_trigger_high_entropy(self) -> None:
        text = (
            "The canonical brand system lives in "
            "https://github.com/OpenCoven/coven/blob/main/DESIGN.md and "
            "https://github.com/OpenCoven/coven/tree/main/brand."
        )

        hits = check_secrets.scan_text(text, "docs/BRAND.md")

        self.assertEqual(hits, [])

    def test_opencoven_local_worktree_paths_do_not_trigger_high_entropy(self) -> None:
        text = "/tmp/OpenCoven/coven/.worktrees/feat-tui-chat-module"

        hits = check_secrets.scan_text(text, "docs/superpowers/plans/example.md")

        self.assertEqual(hits, [])

    def test_lockfiles_are_not_excluded_from_scanning(self) -> None:
        self.assertFalse(check_secrets.is_excluded_path("packages/openclaw-coven/pnpm-lock.yaml"))

    def test_local_worktree_paths_do_not_trigger_high_entropy(self) -> None:
        text = (
            "cd /tmp/OpenCoven/coven/.worktrees/feat-tui-chat-module\n"
            "Expected: /tmp/OpenCoven/coven/.worktrees/feat-tui-chat-module"
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

    def test_opencoven_repo_relative_paths_do_not_trigger_high_entropy(self) -> None:
        text = (
            "Consume `OpenCoven/coven/skills/familiar-board-stewardship/` by symlink.\n"
            "Canonical source: OpenCoven/coven/docs/familiars/board-stewardship.md"
        )

        hits = check_secrets.scan_text(text, "docs/familiars/board-stewardship.md")

        self.assertEqual(hits, [])

    def test_repo_relative_path_heuristic_still_rejects_other_mixed_case_tokens(self) -> None:
        token = "OpenCoven/covenLikeFakePayloadMixedCase1234567890"

        self.assertFalse(check_secrets.is_opencoven_repo_relative_path_token(token))

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

    def test_batch_reader_rejects_malformed_headers(self) -> None:
        with self.assertRaisesRegex(RuntimeError, "malformed object header"):
            check_secrets.read_batch_object(io.BytesIO(b"malformed\n"), "a" * 40)

    def test_batch_reader_rejects_out_of_order_objects(self) -> None:
        stream = io.BytesIO(f"{'b' * 40} blob 0\n\n".encode())

        with self.assertRaisesRegex(RuntimeError, "objects out of order"):
            check_secrets.read_batch_object(stream, "a" * 40)

    def test_batch_reader_rejects_unknown_object_types(self) -> None:
        stream = io.BytesIO(f"{'a' * 40} future-object 0\n\n".encode())

        with self.assertRaisesRegex(RuntimeError, "unknown object type"):
            check_secrets.read_batch_object(stream, "a" * 40)

    def test_batch_reader_rejects_invalid_object_sizes(self) -> None:
        sha = "a" * 40
        invalid_sizes = (
            b"not-a-number",
            b"-1",
            str(check_secrets.MAX_BATCH_OBJECT_BYTES + 1).encode(),
        )

        for size in invalid_sizes:
            with self.subTest(size=size):
                stream = io.BytesIO(sha.encode() + b" blob " + size + b"\n")
                with self.assertRaisesRegex(RuntimeError, "malformed object size"):
                    check_secrets.read_batch_object(stream, sha)

    def test_batch_reader_rejects_truncated_objects_and_missing_trailers(self) -> None:
        sha = "a" * 40
        streams = (
            io.BytesIO(f"{sha} blob 4\n".encode() + b"abc"),
            io.BytesIO(f"{sha} blob 3\n".encode() + b"abcX"),
        )

        for stream in streams:
            with self.subTest(data=stream.getvalue()):
                with self.assertRaisesRegex(RuntimeError, "truncated object"):
                    check_secrets.read_batch_object(stream, sha)

    def test_history_scan_rejects_nonzero_batch_exit(self) -> None:
        sha = "a" * 40

        class FailedBatch:
            stdout = io.BytesIO(f"{sha} blob 0\n\n".encode())

            def __enter__(self) -> FailedBatch:
                return self

            def __exit__(self, *_: object) -> None:
                return None

            def wait(self) -> int:
                return 1

        with (
            mock.patch.object(
                check_secrets,
                "sh",
                return_value=f"{sha} docs/example.md\n",
            ),
            mock.patch.object(subprocess, "Popen", return_value=FailedBatch()),
        ):
            with self.assertRaisesRegex(RuntimeError, "cat-file --batch failed"):
                check_secrets.history_blob_hits()

    def test_history_scan_rejects_unexpected_batch_output(self) -> None:
        sha = "a" * 40

        class ExtraOutputBatch:
            stdout = io.BytesIO(f"{sha} blob 0\n\nunexpected".encode())

            def __enter__(self) -> ExtraOutputBatch:
                return self

            def __exit__(self, *_: object) -> None:
                return None

            def wait(self) -> int:
                return 0

        with (
            mock.patch.object(
                check_secrets,
                "sh",
                return_value=f"{sha} docs/example.md\n",
            ),
            mock.patch.object(
                subprocess,
                "Popen",
                return_value=ExtraOutputBatch(),
            ),
        ):
            with self.assertRaisesRegex(RuntimeError, "unexpected trailing output"):
                check_secrets.history_blob_hits()

    def test_history_scan_uses_bounded_git_processes_without_skipping_deleted_secrets(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            repo = pathlib.Path(temp_dir)

            def git(*args: str) -> None:
                subprocess.run(
                    ["git", *args],
                    cwd=repo,
                    check=True,
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                )

            git("init", "-q")
            git("config", "user.name", "Secret Guard Test")
            git("config", "user.email", "secret-guard@example.invalid")
            secret_file = repo / "secret.txt"
            secret_file.write_text(
                "token=" + "ghp_" + ("A1" * 12) + "\n",
                encoding="utf-8",
            )
            git("add", "secret.txt")
            git("commit", "-q", "-m", "add historical fixture")
            secret_file.unlink()
            git("add", "-u")
            git("commit", "-q", "-m", "remove historical fixture")

            original_popen = subprocess.Popen
            spawned: list[tuple[object, dict[str, object]]] = []

            def counting_popen(*args: object, **kwargs: object) -> subprocess.Popen[bytes]:
                spawned.append((args[0], kwargs))
                return original_popen(*args, **kwargs)

            with (
                mock.patch.object(check_secrets, "ROOT", repo),
                mock.patch.object(
                    subprocess,
                    "Popen",
                    side_effect=counting_popen,
                ),
            ):
                hits = check_secrets.history_blob_hits()

        self.assertIn(
            ("secret.txt", 1, "github_token"),
            [(path, line, rule) for _, path, line, rule in hits],
        )
        self.assertLessEqual(
            len(spawned),
            3,
            f"history scan spawned one or more Git processes per object: {spawned}",
        )
        batch_calls = [
            kwargs
            for command, kwargs in spawned
            if command == ["git", "cat-file", "--batch"]
        ]
        self.assertEqual(len(batch_calls), 1)
        self.assertIsNot(
            batch_calls[0].get("stdin"),
            subprocess.PIPE,
            "batch stdin must be finite so malformed output cannot wait for another request",
        )
        self.assertEqual(
            batch_calls[0].get("stderr"),
            subprocess.DEVNULL,
            "batch stderr must not be an undrained pipe",
        )

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

    def test_sha_pinned_github_action_ref_is_not_a_secret(self) -> None:
        # SHA-pinning third-party actions is an OpenSSF best practice; the
        # resulting `<owner>/<repo>@<40-hex>` token should not trip the entropy
        # rule even though the trailing SHA is high-entropy.
        text = "\n".join(
            [
                "      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd  # v6.0.0",
                "      - uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8  # stable",
            ]
        )

        hits = check_secrets.scan_text(text, ".github/workflows/release-npm.yml")

        self.assertEqual(hits, [])

    def test_rust_target_triple_path_is_not_a_secret(self) -> None:
        # `target/x86_64-pc-windows-msvc/release` is read as one 37-char token
        # by the entropy regex; the heuristic must accept it because the `64`
        # segment is all-digits.
        text = "          path: target/x86_64-pc-windows-msvc/release\n"

        hits = check_secrets.scan_text(text, ".github/workflows/release-npm.yml")

        self.assertEqual(hits, [])

    def test_macos_launchagent_paths_do_not_trigger_high_entropy(self) -> None:
        # launchd/service docs reference `~/Library/LaunchAgents/<label>.plist`
        # paths; the reverse-DNS plist filename pushes the token over the
        # entropy threshold but it is never a credential.
        text = "\n".join(
            [
                "`~/Library/LaunchAgents/dev.opencoven.hub.plist`:",
                'launchctl bootstrap "gui/$(id -u)" ~/Library/LaunchAgents/dev.opencoven.hub.plist',
                "sudo cp hub.plist Library/LaunchAgents/dev.opencoven.hub.plist",
            ]
        )

        hits = check_secrets.scan_text(text, "docs/HUB-OPERATIONS.md")

        self.assertEqual(hits, [])

    def test_apple_plist_dtd_url_does_not_trigger_high_entropy(self) -> None:
        # Every plist XML doctype embeds the public Apple DTD system
        # identifier; it is boilerplate, not a secret.
        text = '  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'

        hits = check_secrets.scan_text(text, "docs/HUB-OPERATIONS.md")

        self.assertEqual(hits, [])

    def test_macos_library_path_heuristic_stays_narrow(self) -> None:
        # Only the closed set of well-known Library subdirectories is
        # accepted, and token-like blobs inside a path segment must still
        # trip the entropy rule via the segment-length cap.
        self.assertTrue(
            check_secrets.is_macos_library_path_token(
                "Library/LaunchDaemons/dev.opencoven.hub.plist"
            )
        )
        self.assertFalse(
            check_secrets.is_macos_library_path_token(
                "Library/SecretsVault/dev.opencoven.hub.plist"
            )
        )
        self.assertFalse(
            check_secrets.is_macos_library_path_token(
                "Library/LaunchAgents/" + "Qz7" * 30 + ".plist"
            )
        )
        self.assertFalse(
            check_secrets.is_apple_dtd_url_token(
                "www.apple.com/DTDs/" + "Qz7" * 30 + ".dtd"
            )
        )
        self.assertFalse(
            check_secrets.is_apple_dtd_url_token("www.evil.example/DTDs/PropertyList-1.0.dtd")
        )

    def test_identifier_heuristic_rejects_pure_digit_segment_tokens(self) -> None:
        # A token whose every segment is pure digits has no name shape; the
        # entropy rule must still see it, even though it splits cleanly.
        self.assertFalse(
            check_secrets.is_programming_identifier_token(
                "12345678.12345678.12345678.12345678"
            )
        )

    def test_sha_ref_heuristic_requires_exactly_40_hex_chars(self) -> None:
        # 40-char hex SHA is the only ref shape we whitelist; tags, branches,
        # short hex prefixes, and non-hex refs must still trip the entropy rule.
        self.assertTrue(
            check_secrets.is_github_action_sha_ref_token(
                "actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd"
            )
        )
        self.assertFalse(
            check_secrets.is_github_action_sha_ref_token("actions/checkout@de0fac2")
        )
        self.assertFalse(
            check_secrets.is_github_action_sha_ref_token("actions/checkout@v6")
        )
        self.assertFalse(
            check_secrets.is_github_action_sha_ref_token(
                "actions/checkout@m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aBEuIqOwPz9RkT"
            )
        )


class SecretGuardExceptionScopeTests(unittest.TestCase):
    def test_angle_brackets_do_not_suppress_structural_secret_rules(self) -> None:
        cases = {
            "aws_access_key": "AKIA" + "A1" * 8,
            "github_token": "ghp_" + "A1" * 12,
            "openai_key": "sk-" + "A1" * 16,
            "anthropic_key": "sk-ant-" + "A1" * 10,
            "slack_token": "xoxb-" + "A1-" * 7,
        }

        for expected_rule, value in cases.items():
            with self.subTest(rule=expected_rule):
                hits = check_secrets.scan_text(
                    f"<span>{value}</span>", "docs/example.html"
                )

                self.assertEqual(
                    hits, [("docs/example.html", 1, expected_rule)]
                )

    def test_safe_line_context_does_not_suppress_high_entropy_tokens(self) -> None:
        token = (
            "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/"
            "EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        )
        contexts = [
            "<span>rendered value</span>",
            "example value",
            "api_key=<secret-value>",
            "https://github.com/OpenCoven/coven/blob/main/DESIGN.md",
            "/tmp/OpenCoven/coven/.worktrees/demo",
        ]

        for context in contexts:
            with self.subTest(context=context):
                hits = check_secrets.scan_text(
                    f"{context} {token}", "docs/example.md"
                )

                self.assertEqual(
                    hits, [("docs/example.md", 1, "high_entropy")]
                )

    def test_angle_placeholder_is_scoped_to_the_assignment_value(self) -> None:
        placeholder_hits = check_secrets.scan_text(
            "api_key=<secret-value>", "docs/example.env"
        )
        literal_hits = check_secrets.scan_text(
            "api_key=hunter2hunter2hunter2 <span>example</span>",
            "docs/example.env",
        )

        self.assertEqual(placeholder_hits, [])
        self.assertEqual(
            literal_hits,
            [("docs/example.env", 1, "generic_assignment")],
        )

    def test_angle_placeholder_rejects_literal_payload(self) -> None:
        hits = check_secrets.scan_text(
            "api_key=<hunter2hunter2hunter2>", "docs/example.env"
        )

        self.assertEqual(
            hits, [("docs/example.env", 1, "generic_assignment")]
        )

    def test_common_angle_placeholder_is_safe(self) -> None:
        cases = [
            "api_key=<YOUR_API_KEY>",
            "api_key=<YOUR_API_KEY_HERE>",
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "docs/example.env")

                self.assertEqual(hits, [])

    def test_safe_value_rejects_unquoted_passphrase_continuation(self) -> None:
        cases = [
            "api_key=<secret-value> correct horse battery staple",
            "api_key=<secret-value> correct horse battery",
            "api_key=<secret-value> hunter2 hunter2 hunter2",
            "api_key=<secret-value> correct horse battery.",
            "api_key=<secret-value> correct horse battery;",
            "api_key=<secret-value> correct horse battery!",
            "api_key=<secret-value> correct horse battery # note",
            "api_key=<YOUR_API_KEY> correct horse battery.",
            "api_key=<secret-value> correct, horse, battery",
            "api_key=<secret-value> correct: horse: battery",
            "api_key=<secret-value> correct / horse / battery",
            "api_key=<secret-value> (correct horse battery)",
            "api_key=<secret-value> correct horse (battery)",
            "api_key=<secret-value> [correct horse battery]",
            "api_key=<secret-value> correct horse battery <b>",
            "api_key=<secret-value> correct horse and battery",
            (
                "api_key=<secret-value> "
                "# note AbCdEfGhIjKlMnOpQrStUvWx"
            ),
            (
                "api_key=<secret-value> "
                "[note AbCdEfGhIjKlMnOpQrStUvWx](README.md)"
            ),
            (
                "api_key=<secret-value> "
                "# replace correct horse battery with actual value"
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "docs/example.env")

                self.assertEqual(
                    hits, [("docs/example.env", 1, "generic_assignment")]
                )

    def test_lockfile_exception_is_scoped_to_the_integrity_token(self) -> None:
        digest = (
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
        )
        token = (
            "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB/"
            "EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        )
        text = f"resolution: {{integrity: sha512-{digest}}} observed: {token}"

        hits = check_secrets.scan_text(
            text, "packages/openclaw-coven/pnpm-lock.yaml"
        )

        self.assertEqual(
            hits,
            [
                (
                    "packages/openclaw-coven/pnpm-lock.yaml",
                    1,
                    "high_entropy",
                )
            ],
        )

    def test_known_public_discord_article_url_is_not_high_entropy(self) -> None:
        text = (
            "https://support-dev.discord.com/hc/en-us/articles/"
            "6207308062871-What-are-Privileged-Intents"
        )

        hits = check_secrets.scan_text(text, "docs/channels/discord-setup.md")

        self.assertEqual(hits, [])

    def test_other_discord_article_slugs_still_trigger_high_entropy(self) -> None:
        token = (
            "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB"
            "EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        )
        text = (
            "https://support-dev.discord.com/hc/en-us/articles/1-"
            f"{token}"
        )

        hits = check_secrets.scan_text(text, "docs/channels/discord-setup.md")

        self.assertEqual(
            hits,
            [("docs/channels/discord-setup.md", 1, "high_entropy")],
        )

    def test_ordered_alphabet_fixtures_are_not_high_entropy(self) -> None:
        fixtures = [
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUV",
        ]

        for fixture in fixtures:
            with self.subTest(fixture_length=len(fixture)):
                hits = check_secrets.scan_text(fixture, "src/lib.rs")

                self.assertEqual(hits, [])

    def test_reserved_example_url_assignment_is_not_a_secret(self) -> None:
        cases = [
            (
                'let url = "https://private-gateway.example.test/'
                'session?token=fakegatewaytoken123";'
            ),
            (
                '"https://private-gateway.example.test/'
                'session?token=fakegatewaytoken123".to_string()'
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(
                    text, "crates/coven-cli/src/privacy.rs"
                )

                self.assertEqual(hits, [])

    def test_reserved_example_url_rejects_literal_payload(self) -> None:
        value = "hunter2" * 3
        cases = [
            (
                '"https://private-gateway.example.test/'
                f'session?token=fakegatewaytoken123" + "{value}"'
            ),
            (
                '"https://private-gateway.example.test/'
                f'session?token=fakegatewaytoken123" {value}'
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(
                    text, "crates/coven-cli/src/privacy.rs"
                )

                self.assertEqual(
                    hits,
                    [
                        (
                            "crates/coven-cli/src/privacy.rs",
                            1,
                            "generic_assignment",
                        )
                    ],
                )

    def test_safe_generic_values_allow_only_syntax_suffixes(self) -> None:
        cases = [
            "api_key=<secret-value>;",
            "<code>api_key=<secret-value></code>",
            "const token = process.env.REALLY_LONG_SECRET_NAME;",
            "const token = process.env.REALLY_LONG_SECRET_NAME.trim();",
            "const token = process.env.REALLY_LONG_SECRET_NAME?.trim();",
            "const token = process.env.REALLY_LONG_SECRET_NAME!;",
            'const token = process.env.REALLY_LONG_SECRET_NAME ?? "";',
            'const token = process.env.REALLY_LONG_SECRET_NAME || "";',
            'api_key=os.environ.get("API_KEY", "")',
            'api_key=os.environ.get("API_KEY", None)',
            'api_key=os.environ.get("API_KEY", "").strip()',
            'api_key=os.environ.get("API_KEY") or None',
            'let token = std::env::var("TOKEN").unwrap_or_default();',
            "api_key=your_api_key_here",
            "api_key=placeholder_value",
            "api_key=op://Development/Service/api_key",
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "docs/example.md")

                self.assertEqual(hits, [])

    def test_environment_reference_with_appended_value_is_not_safe(self) -> None:
        text = "api_key=${API_KEY}hunter2hunter2hunter2"

        hits = check_secrets.scan_text(text, "src/config.env")

        self.assertEqual(
            hits,
            [("src/config.env", 1, "generic_assignment")],
        )

    def test_environment_read_with_literal_fallback_is_not_safe(self) -> None:
        text = 'api_key=os.environ.get("API_KEY","hunter2hunter2hunter2")'

        hits = check_secrets.scan_text(text, "src/config.py")

        self.assertEqual(
            hits,
            [("src/config.py", 1, "generic_assignment")],
        )

    def test_shell_environment_reference_with_literal_default_is_not_safe(
        self,
    ) -> None:
        text = "api_key=${API_KEY:-hunter2hunter2hunter2}"

        hits = check_secrets.scan_text(text, "src/config.env")

        self.assertEqual(
            hits,
            [("src/config.env", 1, "generic_assignment")],
        )

    def test_quoted_and_call_safe_values_reject_appended_payloads(self) -> None:
        short_mixed = "AbCdEfGh" + "IjKlMnOp" + "QrStUvWx"
        hex_value = "deadbeef" * 4
        punctuated = ":".join(["hunter2"] * 3)
        password = "P@ssw0rd!" * 2
        segmented = [
            separator.join(["hunter2"] * 3)
            for separator in ("_", ".", "/", "-")
        ]
        cases = [
            'api_key="<secret-value>"hunter2hunter2hunter2',
            'api_key="<secret-value>" hunter2hunter2hunter2',
            f'api_key="<secret-value>" {short_mixed}',
            f'api_key="<secret-value>" {hex_value}',
            f'api_key="<secret-value>" {punctuated}',
            f'api_key="<secret-value>" {password}',
            'api_key="<secret-value>" "hunter2 hunter2 hunter2"',
            (
                'api_key="<secret-value>" '
                '"correct horse battery staple"'
            ),
            (
                'api_key="<secret-value>" '
                '"hunter2" "hunter2" "hunter2"'
            ),
            'api_key="<secret-value>" "hunter2 hunter2 hunter2',
            (
                "api_key=\"<secret-value>\" "
                "'correct horse battery staple"
            ),
            'api_key="<secret-value>" a"hunter2 hunter2"b',
            (
                'api_key="<secret-value>" '
                "[hunter2hunter2hunter2]"
                "(https://github.com/OpenCoven/coven)"
            ),
            (
                'api_key="<secret-value>" '
                "[P@ssw0rd!P@ssw0rd!]"
                "(https://github.com/OpenCoven/coven)"
            ),
            (
                'api_key="<secret-value>" '
                "[correcthorsebatterystaple]"
                "(https://github.com/OpenCoven/coven)"
            ),
            (
                'api_key="<secret-value>" '
                "[correct-horse-battery-staple]"
                "(https://github.com/OpenCoven/coven)"
            ),
            (
                'api_key="<secret-value>" '
                "# correct horse battery staple"
            ),
            f'api_key="<secret-value>" # {short_mixed}',
            (
                f'api_key="<secret-value>" [{short_mixed}]'
                "(https://github.com/OpenCoven/coven)"
            ),
            'api_key="<secret-value>" hunter2\\ hunter2\\ hunter2',
            'api_key="<secret-value>" hunter2/hunter2',
            (
                'api_key="<secret-value>" '
                "https://github.com/OpenCoven/coven/"
                "hunter2hunter2hunter2"
            ),
            *[
                f'api_key="<secret-value>" {value}'
                for value in segmented
            ],
            'api_key="${API_KEY}"hunter2hunter2hunter2',
            'api_key="${API_KEY}" hunter2hunter2hunter2',
            'api_key=os.environ.get("API_KEY")+hunter2hunter2hunter2',
            'api_key=os.environ.get("API_KEY") hunter2hunter2hunter2',
            'api_key=std::env::var("API_KEY")+hunter2hunter2hunter2',
            'api_key="<secret-value>" + hunter2hunter2hunter2',
            'api_key="${API_KEY}" + hunter2hunter2hunter2',
            'api_key=os.environ.get("API_KEY") + hunter2hunter2hunter2',
            "api_key=process.env.API_KEY + hunter2hunter2hunter2",
            'api_key=process.env.API_KEY || "hunter2hunter2hunter2"',
            'api_key=process.env.API_KEY ?? "hunter2hunter2hunter2"',
            'api_key=os.environ.get("API_KEY") or "hunter2hunter2hunter2"',
            'api_key=os.environ.get("API_KEY", "").strip()hunter2hunter2hunter2',
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "src/config.example")

                self.assertEqual(
                    hits,
                    [("src/config.example", 1, "generic_assignment")],
                )

    def test_safe_values_allow_known_nonsecret_trailing_context(self) -> None:
        cases = [
            "api_key=<secret-value> docs/reference/api.md",
            (
                "api_key=<secret-value> "
                "https://docs.example.com/configuration"
            ),
            "api_key=<secret-value> CONFIG_VALUE_REFERENCE",
            "api_key=<secret-value> CONFIGURATION_VALUE",
            "api_key=<secret-value> CONFIGURATION_VALUE:",
            (
                "api_key=<secret-value> "
                "https://github.com/OpenCoven/coven"
            ),
            (
                "api_key=<secret-value> "
                "(https://github.com/OpenCoven/coven)."
            ),
            "api_key=<secret-value> /etc/configuration",
            "api_key=<secret-value> docs/reference/api.md:",
            (
                "api_key=<secret-value> "
                "https://github.com/advisories/GHSA-rhfx-m35p-ff5j"
            ),
            (
                "api_key=<secret-value> "
                "https://github.com/OpenCoven/coven/commit/"
                + ("deadbeef" * 5)
            ),
            (
                "api_key=<secret-value> "
                "https://www.apple.com/DTDs/PropertyList-1.0.dtd"
            ),
            (
                "api_key=<secret-value> "
                "https://support-dev.discord.com/hc/en-us/articles/"
                "6207308062871-What-are-Privileged-Intents"
            ),
            'api_key=<secret-value> "$CONFIGURATION_FILE"',
            'api_key=<secret-value> "${CONFIGURATION_FILE}"',
            (
                "api_key=<secret-value> "
                "[reference](https://github.com/OpenCoven/coven)"
            ),
            (
                "api_key=<secret-value> "
                "https://github.com/OpenCoven/coven#readme"
            ),
            (
                "api_key=<secret-value> "
                "[configuration](docs/reference/api.md)"
            ),
            (
                'api_key=<secret-value> '
                '"$HOME/.config/opencoven/config.toml"'
            ),
            (
                "api_key=<secret-value> "
                "https://github.com/OpenCoven/coven/"
                "blob/main/README.md#L10"
            ),
            "api_key=<secret-value> # configuration placeholder",
            "api_key=<secret-value> # configuration placeholder.",
            "api_key=<secret-value> # replace with your actual value",
            (
                "api_key=<secret-value> "
                "# don't commit the placeholder"
            ),
            "api_key=<secret-value> [Read more](README.md)",
            (
                "api_key=<secret-value> "
                "[configuration](#configuration)"
            ),
            (
                "api_key=<secret-value> "
                "[configuration]"
                "(docs/reference/api.md#configuration)"
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "docs/example.md")

                self.assertEqual(hits, [])

    def test_broad_safe_context_shapes_reject_payloads(self) -> None:
        password = "P@ssw0rd!" * 2
        cases = [
            (
                f"github.com/advisories/GHSA-{password}",
                "high_entropy",
            ),
            (f"/tmp/a/b/{password}", "generic_assignment"),
        ]

        for value, expected_rule in cases:
            with self.subTest(value=value):
                text = f'api_key="<secret-value>" {value}'
                hits = check_secrets.scan_text(text, "docs/example.md")

                self.assertEqual(
                    hits, [("docs/example.md", 1, expected_rule)]
                )

    def test_safe_path_context_rejects_credential_like_segments(self) -> None:
        token = "".join(
            ("m9R3tQv7WzK2pL5", "nX8cF1gJ4sD6hY0", "aBEuIqOwPz")
        )
        cases = [
            (f"/tmp/a/b/{token}", "high_entropy"),
            (
                "https://github.com/OpenCoven/coven/blob/main/"
                f"{token}",
                "high_entropy",
            ),
            (
                "https://github.com/OpenCoven/coven/"
                f"releases/download/v1/{token}",
                "high_entropy",
            ),
            (f"Library/LaunchAgents/{token}", "high_entropy"),
            ("/tmp/a/b/hunter2hunter2hunter2", "generic_assignment"),
            ("/tmp/a/b/abcdefghijklmnopqrst", "generic_assignment"),
            ("/tmp/a/b/correcthorsebattery", "generic_assignment"),
            ("/tmp/a/b/hunterhunterhunter", "generic_assignment"),
            ("/tmp/a/b/CorrectHorseBattery", "generic_assignment"),
            ("/tmp/a/b/correct/horse/battery", "generic_assignment"),
            (
                "https://github.com/OpenCoven/coven/blob/main/"
                "correcthorsebattery",
                "generic_assignment",
            ),
        ]

        for value, expected_rule in cases:
            with self.subTest(value=value):
                hits = check_secrets.scan_text(
                    f"api_key=<secret-value> {value}",
                    "docs/example.md",
                )

                self.assertEqual(
                    hits, [("docs/example.md", 1, expected_rule)]
                )

    def test_structured_reference_rejects_credential_like_segments(self) -> None:
        token = "".join(
            ("m9R3tQv7WzK2pL5", "nX8cF1gJ4sD6hY0", "aBEuIqOwPz")
        )
        sha = "deadbeef" * 5
        cases = [
            (
                f"https://github.com/{token}/repo/commit/{sha}",
                "high_entropy",
            ),
            (
                f"https://github.com/OpenCoven/{token}/commit/{sha}",
                "high_entropy",
            ),
            (f"actions/{token}@{sha}", "high_entropy"),
            (
                f"https://www.apple.com/DTDs/{token}.dtd",
                "high_entropy",
            ),
            (
                "https://github.com/OpenCoven/coven"
                "#correcthorsebattery",
                "generic_assignment",
            ),
        ]

        for value, expected_rule in cases:
            with self.subTest(value=value):
                hits = check_secrets.scan_text(
                    f"api_key=<secret-value> {value}",
                    "docs/example.md",
                )

                self.assertEqual(
                    hits, [("docs/example.md", 1, expected_rule)]
                )

    def test_grep_extended_regex_pattern_assignment_terms_are_safe(self) -> None:
        cases = [
            "grep -cE '(token=|key=|secret=|password=)' history",
            "grep -c -E '(token=|key=|secret=|password=)' history",
            (
                "COUNT=$(grep -cE "
                "'(token=|key=|secret=|password=|Bearer [A-Za-z0-9])' "
                '"$HIST_FILE" 2>/dev/null || true)'
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "scripts/audit.sh")

                self.assertEqual(hits, [])

    def test_grep_exception_is_scoped_to_its_pattern_argument(self) -> None:
        key_name = "api" + "_key"
        value = "hunter2" * 3
        cases = [
            f'echo "{key_name}={value}" # unrelated grep -E "safe"',
            f'grep "{key_name}={value}" file # use -E later',
            f'grep -E "safe" "{key_name}={value}"',
            f'grep safe; echo -E "{key_name}={value}"',
            f'grep -E safe "{key_name}={value}"',
            f'grep -E -f "{key_name}={value}" data',
            f'echo grep -E "{key_name}={value}"',
            f'x=$(grep -E safe) echo "{key_name}={value}"',
            (
                "grep -E '(token=|key=|secret=|password=''"
                + value
                + ")' history"
            ),
            f'pattern = "(token=|key=|secret=|password=" + "{value})"',
            f"grep -E 'token=|bearer [api_key={value}]' history",
            f"grep -E '(token=|bearer [password={value}])' history",
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(text, "scripts/audit.sh")

                self.assertEqual(
                    hits,
                    [("scripts/audit.sh", 1, "generic_assignment")],
                )


class SecretGuardRustLetBindingTests(unittest.TestCase):
    def test_known_rust_runtime_bindings_do_not_trigger_generic_assignment(
        self,
    ) -> None:
        text = "\n".join(
            [
                "    let token = text.split_whitespace().last()?;",
                "    let token = token.strip_prefix('v').unwrap_or(token);",
                "    let mut secret = std::env::args().nth(1).unwrap_or_default();",
                (
                    "    let token = text.split_whitespace().last()?; "
                    "// parse runtime token"
                ),
                "    let password: String = prompt_hidden(\"Password: \")?;",
                "    let api_key = format!(\"{prefix}-{suffix}\");",
            ]
        )

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/engine.rs")

        self.assertEqual(hits, [])

    def test_rust_let_with_literal_value_still_triggers_generic_assignment(self) -> None:
        hits = check_secrets.scan_text(
            'let token = "hunter2hunter2hunter2";', "crates/coven-cli/src/engine.rs"
        )

        self.assertEqual(
            hits, [("crates/coven-cli/src/engine.rs", 1, "generic_assignment")]
        )

    def test_rust_let_call_with_appended_value_still_triggers_generic_assignment(
        self,
    ) -> None:
        cases = [
            'let token = compute_token() + "hunter2hunter2hunter2";',
            'let token = compute_token().trim() + "hunter2hunter2hunter2";',
            "let token = compute_token() || hunter2hunter2hunter2;",
            'let token = compute_token().unwrap_or("hunter2hunter2hunter2");',
            "let token = compute_token(hunter2hunter2hunter2);",
            'let token = compute_token(r#"hunter2hunter2hunter2"#);',
            'let token = compute_token("hunterhunter");',
            'let password = prompt_hidden("correct horse battery staple");',
            'let token = compute_token("abcdefghijklmnop-qrst");',
            'let token = compute_token("correct horse battery");',
            'let token = compute_token("Correct Horse Battery Staple");',
            (
                "let token = compute_token("
                '/* ); // */ "hunter2hunter2hunter2");'
            ),
            (
                'let token = compute_token(concat!("hunter2", '
                '"hunter2", "hunter2"));'
            ),
            (
                'let token = compute_token(format!("{}{}{}", '
                '"hunter2", "hunter2", "hunter2"));'
            ),
            (
                'let token = compute_token(["hunter2", "hunter2", '
                '"hunter2"].concat());'
            ),
            "let token = compute_token(12345678901234567890.into());",
            "let token = compute_token(&12345678901234567890);",
            "let token = compute_token::<12345678901234567890>();",
            (
                "let token = text.split_whitespace().last()?; "
                "// correct horse battery staple"
            ),
            (
                "let token = text.split_whitespace().last()?; "
                "// note AbCdEfGhIjKlMnOpQrStUvWx"
            ),
        ]

        for text in cases:
            with self.subTest(text=text):
                hits = check_secrets.scan_text(
                    text, "crates/coven-cli/src/engine.rs"
                )

                self.assertEqual(
                    hits,
                    [
                        (
                            "crates/coven-cli/src/engine.rs",
                            1,
                            "generic_assignment",
                        )
                    ],
                )

    def test_rust_let_call_exception_is_scoped_to_rust_files(self) -> None:
        hits = check_secrets.scan_text(
            "let token = text.split_whitespace().last()?;", "docs/example.txt"
        )

        self.assertEqual(
            hits, [("docs/example.txt", 1, "generic_assignment")]
        )

    def test_rust_let_with_bare_blob_still_triggers_generic_assignment(self) -> None:
        # Assembled at runtime so the fixture itself doesn't look like a secret
        # assignment to source-level scanners; the scanned LINE is what matters.
        blob = "SGVsbG9" + "Xb3JsZFRoaXNJc05vdEFDYWxs"
        hits = check_secrets.scan_text(f"let token = {blob};", "src/lib.rs")

        self.assertEqual(
            hits,
            [
                ("src/lib.rs", 1, "generic_assignment"),
                ("src/lib.rs", 1, "high_entropy"),
            ],
        )

    def test_other_rules_still_apply_to_let_call_binding_lines(self) -> None:
        hits = check_secrets.scan_text(
            'let token = mint("ghp_0123456789abcdefghij123456");', "src/lib.rs"
        )

        self.assertEqual(hits, [("src/lib.rs", 1, "github_token")])


class SecretGuardReleaseUrlTests(unittest.TestCase):
    def test_opencoven_release_download_urls_do_not_trigger_high_entropy(self) -> None:
        text = (
            '            "https://github.com/OpenCoven/coven-code/releases/download'
            '/v0.6.1/coven-code-macos-aarch64.tar.gz"'
        )

        hits = check_secrets.scan_text(text, "crates/coven-cli/src/engine_install.rs")

        self.assertEqual(hits, [])

    def test_non_opencoven_release_urls_still_trigger_high_entropy(self) -> None:
        blob = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB" + "EuIqOwPz9RkTlVxCyNmS3HdG7fA"
        text = f"https://github.com/NotOpenCoven/repo/releases/download/v1/{blob}"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "high_entropy")])

    def test_overlong_release_artifact_segment_still_triggers_high_entropy(self) -> None:
        blob = "m9R3tQv7WzK2pL5nX8cF1gJ4sD6hY0aB" * 3
        text = f"https://github.com/OpenCoven/coven-code/releases/download/v1/{blob}"

        hits = check_secrets.scan_text(text, "docs/example.md")

        self.assertEqual(hits, [("docs/example.md", 1, "high_entropy")])


if __name__ == "__main__":
    unittest.main()
