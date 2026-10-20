#!/usr/bin/env python3
"""Pin/preflight contracts; no downloads, package installs or provider access."""
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest


class ConfigTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "repo"
        self.root.mkdir()
        (self.root / "rust-toolchain.toml").write_text(
            '[toolchain]\nchannel = "1.2.3"\ncomponents = ["clippy", "rustfmt"]\nprofile = "minimal"\n'
        )
        (self.root / ".node-version").write_text("24.1.2\n")
        package = self.root / "packages/openclaw-coven"
        package.mkdir(parents=True)
        (package / "package.json").write_text(json.dumps({"packageManager": "pnpm@10.1.2"}))
        self.run_git("init", "-q")
        self.run_git("add", ".")
        self.run_git("-c", "user.name=Fixture", "-c", "user.email=fixture@users.noreply.github.com",
                     "-c", "commit.gpgsign=false", "commit", "-qm", "fixture")
        spec = importlib.util.spec_from_file_location("cloud_config", Path(__file__).with_name("config.py"))
        self.config = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.config)

    def run_git(self, *args):
        return subprocess.run(["git", "-C", str(self.root), *args], check=True, capture_output=True)

    def test_reads_pins_without_copying_version_constants(self):
        pins = self.config.read_pins(self.root)
        self.assertEqual(pins["rust"], {"channel": "1.2.3", "components": ["clippy", "rustfmt"],
                                       "profile": "minimal", "targets": []})
        self.assertEqual(pins["node"], "24.1.2")
        self.assertEqual(pins["package_managers"], {"packages/openclaw-coven": "pnpm@10.1.2"})
        (self.root / ".node-version").write_text("25.2.3\n")
        self.assertEqual(self.config.read_pins(self.root)["node"], "25.2.3")

    def test_package_specific_manager_pins_are_all_preserved(self):
        channels = self.root / "packages/channels"
        channels.mkdir()
        (channels / "package.json").write_text('{"packageManager":"npm@11.2.3"}')
        self.assertEqual(self.config.read_pins(self.root)["package_managers"], {
            "packages/channels": "npm@11.2.3", "packages/openclaw-coven": "pnpm@10.1.2"})

    def test_unpinned_missing_or_ambiguous_versions_fail(self):
        for filename, contents in (
            (".node-version", "24\n"), (".node-version", "24.1.2\n25.2.3\n"),
            ("rust-toolchain.toml", '[toolchain]\nchannel="stable"\n'),
            ("rust-toolchain.toml", '[toolchain]\nchannel="1.2.3"\nchannel="1.2.4"\n'),
            ("packages/openclaw-coven/package.json", '{"packageManager":"pnpm@latest"}'),
            ("packages/openclaw-coven/package.json", "[]"),
            ("packages/openclaw-coven/package.json", '{"packageManager":"pnpm@10.1.2","packageManager":"pnpm@10.2.3"}'),
        ):
            with self.subTest(filename=filename, contents=contents):
                path = self.root / filename
                previous = path.read_bytes()
                path.write_text(contents)
                with self.assertRaises(self.config.PreflightError):
                    self.config.read_pins(self.root)
                path.write_bytes(previous)
        (self.root / ".node-version").unlink()
        with self.assertRaises(self.config.PreflightError):
            self.config.read_pins(self.root)

    def test_preflight_is_read_only_and_records_source(self):
        cache = Path(self.temp.name) / "cache"
        before = self.run_git("status", "--porcelain").stdout
        report = self.config.preflight(self.root, cache)
        self.assertEqual(report["source_sha"], self.run_git("rev-parse", "HEAD").stdout.decode().strip())
        self.assertFalse(cache.exists())
        self.assertEqual(before, self.run_git("status", "--porcelain").stdout)
        self.assertNotIn("ready", report)

    def test_dirty_source_and_overlapping_cache_fail_without_repair(self):
        for cache in (self.root / "cache", self.root, self.root.parent):
            with self.subTest(cache=cache), self.assertRaises(self.config.PreflightError):
                self.config.preflight(self.root, cache)
        alias = Path(self.temp.name) / "alias"
        alias.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises(self.config.PreflightError):
            self.config.preflight(self.root, alias / "cache")
        (self.root / ".node-version").write_text("25.2.3\n")
        with self.assertRaises(self.config.PreflightError):
            self.config.preflight(self.root, Path(self.temp.name) / "cache")
        self.assertEqual((self.root / ".node-version").read_text(), "25.2.3\n")


if __name__ == "__main__":
    unittest.main()
