#!/usr/bin/env python3
"""Orchestration tests use mock commands, never claim installed-tool readiness."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

import readiness

spec = importlib.util.spec_from_file_location("config_tests", Path(__file__).with_name("config-test.py"))
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


class ReadinessTests(fixtures.ConfigTests):
    def runner(self, failing=None, mutate=False):
        self.commands = []
        def run(argv, **kwargs):
            self.commands.append(argv)
            code, output = 0, ""
            if "component" in argv:
                output = "clippy-x86_64-unknown-linux-gnu\nrustfmt-x86_64-unknown-linux-gnu"
            elif "target" in argv:
                output = "x86_64-unknown-linux-gnu"
            elif "rustc" in argv:
                output = "rustc 1.2.3 (fixture)"
            elif "cargo" in argv and "--version" in argv:
                output = "cargo 1.2.3 (fixture)"
            elif argv[-1] == "--version":
                output = "v24.1.2" if Path(argv[0]).name == "node" else "10.1.2"
            elif "doctor" in argv:
                code = 1
                output = json.dumps({"ok": False, "blocking": True, "checks": [
                    {"id": "daemon", "status": "warn", "message": "not running"},
                    {"id": "engine", "status": "fail", "message": "the Coven engine is missing — `coven` and `coven chat` need it"},
                    {"id": "harnesses", "status": "fail", "message": "no harness is available"}]})
            if failing and failing in argv:
                code = 19
            if mutate and "build" in argv:
                (self.root / ".node-version").write_text("25.2.3\n")
            return {"exit_code": code, "stdout": output, "timed_out": False}
        return run

    def test_readiness_runs_locked_build_real_doctor_command_and_smoke(self):
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=self.runner())
        self.assertTrue(report["development_ready"])
        self.assertFalse(report["operational_ready"])
        self.assertEqual(report["doctor"]["exit_code"], 1)
        build = next(argv for argv in self.commands if "build" in argv)
        smoke = next(argv for argv in self.commands if "smoke" in argv)
        self.assertIn("--locked", build)
        self.assertIn("--workspace", build)
        self.assertIn("--locked", smoke)
        self.assertIn("--nocapture", smoke)
        self.assertNotIn("ready", report)

    @patch("readiness.platform.system", return_value="Linux")
    def test_missing_openssl_blocks_build_with_explicit_stage(self, *_):
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=self.runner("openssl"))
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "native_openssl")
        self.assertEqual(report["stages"][-1]["exit_code"], 19)
        self.assertIn(["pkg-config", "--exists", "openssl"], self.commands)
        self.assertFalse(any("build" in argv or "doctor" in argv for argv in self.commands))

    @patch("readiness.platform.system", return_value="Linux")
    def test_missing_external_kill_blocks_build_with_explicit_stage(self, *_):
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=self.runner("kill"))
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "native_process_probe")
        self.assertEqual(report["stages"][-1]["exit_code"], 19)
        probe = next(argv for argv in self.commands if argv[0] == "kill")
        self.assertEqual(probe[1], "-0")
        self.assertTrue(probe[2].isdigit())
        self.assertFalse(any("build" in argv or "doctor" in argv for argv in self.commands))

    def test_failed_build_stops_before_smoke_and_does_not_report_ready(self):
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=self.runner("build"))
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "workspace_build")
        self.assertFalse(any("smoke" in argv for argv in self.commands))

    def test_smoke_failure_clears_top_level_operational_readiness(self):
        runner = self.runner("smoke")
        def healthy_doctor(argv, **kwargs):
            result = runner(argv, **kwargs)
            if "doctor" in argv:
                result.update(exit_code=0, stdout=json.dumps({"ok": True, "blocking": False,
                    "checks": [{"id": name, "status": "pass", "message": "fixture"}
                               for name in ("daemon", "engine", "harnesses")]}))
            return result
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=healthy_doctor)
        self.assertFalse(report["development_ready"])
        self.assertFalse(report["operational_ready"])
        self.assertTrue(report["doctor"]["operational_ready"])
        self.assertEqual(report["blocked_stage"], "cli_smoke")

    def test_smoke_failure_keeps_safe_diagnostic_without_raw_output(self):
        runner = self.runner("smoke")
        def diagnostic_runner(argv, **kwargs):
            result = runner(argv, **kwargs)
            if "smoke" in argv:
                result.update(stdout="private-fixture-marker",
                              smoke_diagnostic={"failed_tests": ["known_case"], "panic_lines": [12]})
            return result
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=diagnostic_runner)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "cli_smoke")
        self.assertEqual(report["smoke_diagnostic"]["failed_tests"], ["known_case"])
        self.assertNotIn("private-fixture-marker", json.dumps(report))

    def test_missing_pinned_components_block_build(self):
        runner = self.runner()
        def missing_component(argv, **kwargs):
            result = runner(argv, **kwargs)
            if "component" in argv:
                result["stdout"] = "rustfmt-x86_64-unknown-linux-gnu"
            return result
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=missing_component)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "rust_components")
        self.assertFalse(any("build" in argv for argv in self.commands))

    def test_source_mutation_is_not_repaired_or_accepted(self):
        report = readiness.verify(self.root, Path(self.temp.name) / "cache", run=self.runner(mutate=True))
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "source_integrity")
        self.assertEqual((self.root / ".node-version").read_text(), "25.2.3\n")


if __name__ == "__main__":
    unittest.main()
