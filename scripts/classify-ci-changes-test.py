#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import io
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('classify_ci_changes', Path(__file__).with_name('classify-ci-changes.py'))
MOD = importlib.util.module_from_spec(SPEC)
assert SPEC and SPEC.loader
SPEC.loader.exec_module(MOD)


class ClassifyTest(unittest.TestCase):
    def classify(self, *paths):
        return MOD.classify(list(paths))

    def test_docs_only(self):
        self.assertEqual(self.classify('docs/a.md'), {'docs_only': True, 'rust': False, 'afs': False, 'channels': False, 'openclaw': False, 'npm_packaging': False, 'engine': False, 'workflow': False, 'cargo_metadata': False})

    def test_cargo_lock(self):
        self.assertEqual(self.classify('Cargo.lock')['rust'], True)
        self.assertEqual(self.classify('Cargo.lock')['afs'], True)
        self.assertEqual(self.classify('Cargo.lock')['npm_packaging'], True)
        self.assertEqual(self.classify('Cargo.lock')['cargo_metadata'], True)

    def test_crate_manifest_is_cargo_metadata(self):
        result = self.classify('crates/coven-client/Cargo.toml')
        self.assertTrue(result['rust'])
        self.assertTrue(result['afs'])
        self.assertTrue(result['npm_packaging'])
        self.assertTrue(result['cargo_metadata'])

    def test_cli_daemon(self):
        result = self.classify('crates/coven-cli/src/daemon.rs')
        self.assertTrue(result['rust'])
        self.assertTrue(result['npm_packaging'])

    def test_afs(self):
        result = self.classify('crates/coven-afs/src/nfs.rs')
        self.assertTrue(result['rust'])
        self.assertTrue(result['afs'])

    def test_channels_only(self):
        result = self.classify('packages/channels/src/index.ts')
        self.assertTrue(result['channels'])
        self.assertFalse(result['openclaw'])
        self.assertFalse(result['npm_packaging'])

    def test_openclaw_only(self):
        result = self.classify('packages/openclaw-coven/src/client.ts')
        self.assertTrue(result['openclaw'])
        self.assertFalse(result['channels'])
        self.assertFalse(result['engine'])

    def test_npm_wrapper_only(self):
        self.assertTrue(self.classify('crates/coven-cli/src/wrapper.rs')['npm_packaging'])

    def test_npm_bin_wrapper_only(self):
        result = self.classify('npm/coven/bin/coven.js')
        self.assertTrue(result['npm_packaging'])
        self.assertFalse(result['rust'])

    def test_help_surface_routes_to_npm_packaging(self):
        for path in (
            'docs/reference/cli-daemon.md',
            'docs/guides/core-access.md',
            'docs/development/cli-core-functionality.md',
            'scripts/cli-docs-test.mjs',
        ):
            with self.subTest(path=path):
                result = self.classify(path)
                self.assertTrue(result['npm_packaging'])
                self.assertFalse(result['workflow'])

    def test_packaged_user_journey_paths_route_to_npm_packaging(self):
        for path in (
            'scripts/test-cli-prepublish-test.mjs',
            'scripts/user-journey-e2e.mjs',
            'scripts/user-journey-e2e-test.mjs',
            'scripts/fixtures/fake-codex.mjs',
        ):
            with self.subTest(path=path):
                result = self.classify(path)
                self.assertTrue(result['npm_packaging'])
                self.assertFalse(result['workflow'])

    def test_release_gate_paths_route_to_npm_packaging(self):
        for path in (
            'scripts/release-required-checks.json',
            'scripts/verify-release-commit-gate.mjs',
            'scripts/verify-release-commit-gate-test.mjs',
        ):
            with self.subTest(path=path):
                result = self.classify(path)
                self.assertTrue(result['npm_packaging'])
                self.assertFalse(result['workflow'])

    def test_engine_install(self):
        result = self.classify('crates/coven-cli/src/engine_install.rs')
        self.assertTrue(result['rust'])
        self.assertTrue(result['npm_packaging'])
        self.assertTrue(result['engine'])

    def test_engine_related_paths(self):
        for path in (
            'crates/coven-cli/src/engine.rs',
            'crates/coven-cli/engine.lock',
            'scripts/pin-engine.sh',
        ):
            with self.subTest(path=path):
                result = self.classify(path)
                self.assertTrue(result['engine'])
                self.assertFalse(result['workflow'])

    def test_workflow_supporting_scripts_fan_out(self):
        for path in (
            'scripts/check-workflows.sh',
            'scripts/check-workflows-test.py',
            'scripts/check-ci-workflow-test.py',
        ):
            with self.subTest(path=path):
                result = self.classify(path)
                self.assertFalse(result['docs_only'])
                for key, value in result.items():
                    if key != 'docs_only':
                        self.assertTrue(value, key)

    def test_workflow_fans_all(self):
        result = self.classify('.github/workflows/ci.yml')
        self.assertFalse(result['docs_only'])
        for k, v in result.items():
            if k != 'docs_only':
                self.assertTrue(v, k)

    def test_unknown_non_docs(self):
        self.assertTrue(self.classify('foo/bar.txt')['rust'])

    def test_mixed_docs_channels(self):
        result = self.classify('docs/a.md', 'packages/channels/src/index.ts')
        self.assertFalse(result['docs_only'])
        self.assertTrue(result['channels'])

    def test_empty_input(self):
        with self.assertRaises(ValueError):
            self.classify()

    def test_github_output_lowercase(self):
        result = self.classify('docs/a.md')
        buf = io.StringIO()
        MOD.write_github_output(result, buf)
        self.assertIn('docs_only=true', buf.getvalue())


if __name__ == '__main__':
    unittest.main()
