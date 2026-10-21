#!/usr/bin/env python3
"""Execution and diagnostic contracts; no provider calls or package installs."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

import runtime


class RuntimeTests(unittest.TestCase):
    def test_real_npm_reads_isolated_configuration_without_double_loading(self):
        node, npm = shutil.which('node'), shutil.which('npm')
        if not node or not npm:
            self.skipTest('real npm check requires the repository Node toolchain')
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            home = root / 'home'
            home.mkdir()
            (home / '.npmrc').write_text('registry=https://ambient.invalid/\n')
            env = runtime.execution_environment(root / 'cache', home, node_bin=Path(node).parent)
            result = runtime.run_command([npm, 'config', 'get', 'registry'], cwd=folder, env=env, timeout=30)
            self.assertEqual(result['exit_code'], 0)
            self.assertEqual(result['stdout'].strip(), 'https://registry.npmjs.org/')

    def test_child_environment_does_not_inherit_credentials_or_overrides(self):
        with tempfile.TemporaryDirectory() as folder:
            prior = os.environ.get("COVEN_TEST_PRIVATE_VALUE")
            os.environ["COVEN_TEST_PRIVATE_VALUE"] = "synthetic-do-not-forward"
            try:
                env = runtime.execution_environment(Path(folder), Path(folder) / "home")
                result = runtime.run_command([sys.executable, "-c", "import os,json;print(json.dumps(dict(os.environ)))"],
                                             cwd=folder, env=env, timeout=10)
            finally:
                if prior is None:
                    os.environ.pop("COVEN_TEST_PRIVATE_VALUE", None)
                else:
                    os.environ["COVEN_TEST_PRIVATE_VALUE"] = prior
            child = json.loads(result["stdout"])
            self.assertNotIn("COVEN_TEST_PRIVATE_VALUE", child)
            self.assertEqual(child["CARGO_HOME"], str(Path(folder) / "cargo"))
            self.assertEqual(child["HOME"], str(Path(folder) / "home"))
            self.assertNotIn("GIT_CONFIG_COUNT", child)

    def test_command_failure_is_retained(self):
        with tempfile.TemporaryDirectory() as folder:
            result = runtime.run_command([sys.executable, "-c", "raise SystemExit(17)"],
                                         cwd=folder, env={}, timeout=10)
            self.assertEqual(result["exit_code"], 17)
            self.assertFalse(result["timed_out"])

    def test_safe_native_failures_remain_distinct_without_raw_output(self):
        diagnostics = (
            ("Native dependency privilege blocker: run in a root setup phase or grant noninteractive sudo.", "native_privilege"),
            ("Unsupported OS: native dependencies require Debian or Ubuntu", "native_distribution"),
        )
        with tempfile.TemporaryDirectory() as folder:
            for message, expected in diagnostics:
                with self.subTest(expected=expected):
                    program = "import sys;sys.stderr.write(" + repr("::error::" + message + "\nprivate-fixture-marker\n") + ");sys.exit(1)"
                    result = runtime.run_command([sys.executable, "-c", program], cwd=folder, env={}, timeout=10)
                    self.assertEqual(result["native_diagnostic"]["code"], expected)
                    self.assertNotIn("private-fixture-marker", json.dumps(result))
                    self.assertTrue(result["native_diagnostic"]["recovery"])

    def test_smoke_diagnostic_only_reports_source_names_and_panic_locations(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "crates/coven-cli/tests/smoke.rs"
            source.parent.mkdir(parents=True)
            source.write_text("#[test]\nfn known_case() {}\n")
            stdout = "test known_case ... FAILED\ntest private_marker ... FAILED\nprivate-marker\n"
            stderr = "thread 'known_case' panicked at crates/coven-cli/tests/smoke.rs:2:3:\nprivate-marker\n"
            diagnostic = runtime.smoke_diagnostic(root, stdout, stderr)
            self.assertEqual(diagnostic, {"failed_tests": ["known_case"], "panic_lines": [2]})
            self.assertNotIn("private", json.dumps(diagnostic))
            self.assertEqual(runtime.smoke_diagnostic(root, "", ""),
                             {"failed_tests": [], "panic_lines": []})

    def test_real_rust_smoke_failure_reports_safe_source_evidence(self):
        rustc = shutil.which("rustc")
        if not rustc:
            self.skipTest("real diagnostic check requires the repository Rust toolchain")
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            source = root / "crates/coven-cli/tests/smoke.rs"
            source.parent.mkdir(parents=True)
            source.write_text('#[test]\nfn smoke_case() { panic!("private-fixture-marker"); }\n')
            binary = root / "smoke-test"
            compiled = subprocess.run([rustc, "--test", "crates/coven-cli/tests/smoke.rs", "-o", str(binary)],
                                      cwd=root, capture_output=True)
            self.assertEqual(compiled.returncode, 0)
            result = runtime.run_command([str(binary), "--test", "smoke", "--nocapture"],
                                         cwd=root, env={}, timeout=30)
            self.assertEqual(result["exit_code"], 101)
            self.assertEqual(result["smoke_diagnostic"], {"failed_tests": ["smoke_case"], "panic_lines": [2]})
            self.assertNotIn("private-fixture-marker", json.dumps(result["smoke_diagnostic"]))

    def test_timeout_terminates_owned_command(self):
        with tempfile.TemporaryDirectory() as folder:
            result = runtime.run_command([sys.executable, "-c", "import time;time.sleep(60)"],
                                         cwd=folder, env={}, timeout=0.1)
            self.assertTrue(result["timed_out"])
            self.assertNotEqual(result["exit_code"], 0)

    def report(self, failures):
        checks = [{"id": "daemon", "status": "warn", "message": "not running"},
                  {"id": "harnesses", "status": "pass", "message": "available"},
                  {"id": "engine", "status": "pass", "message": "available"}]
        for identifier, message in failures.items():
            checks = [c for c in checks if c["id"] != identifier]
            checks.append({"id": identifier, "status": "fail", "message": message})
        return {"exit_code": 1 if failures else 0, "timed_out": False,
                "stdout": json.dumps({"ok": not failures, "blocking": bool(failures), "checks": checks})}

    def test_missing_optional_tools_are_visible_and_not_a_doctor_pass(self):
        result = runtime.classify_doctor(self.report({
            "harnesses": "no harness is available",
            "engine": "the Coven engine is missing — `coven` and `coven chat` need it"}))
        self.assertTrue(result["contributor_diagnostic_complete"])
        self.assertFalse(result["operational_ready"])
        self.assertEqual(result["exit_code"], 1)
        self.assertEqual(result["failed_checks"], ["harnesses", "engine"])

    def test_other_failures_and_malformed_reports_cannot_pass(self):
        for result in (self.report({"engine": "version is too old"}),
                       self.report({"daemon": "stale daemon record"}),
                       {"exit_code": 1, "timed_out": False, "stdout": "not json"},
                       {"exit_code": 0, "timed_out": False, "stdout": '{"ok":true,"blocking":false,"checks":[]}'},
                       {"exit_code": -9, "timed_out": True, "stdout": ""}):
            with self.subTest(result=result):
                self.assertFalse(runtime.classify_doctor(result)["contributor_diagnostic_complete"])

    def test_successful_real_diagnostic_is_recognized(self):
        result = runtime.classify_doctor(self.report({}))
        self.assertTrue(result["contributor_diagnostic_complete"])
        self.assertTrue(result["operational_ready"])


if __name__ == "__main__":
    unittest.main()
