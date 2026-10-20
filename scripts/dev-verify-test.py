#!/usr/bin/env python3
"""Execution contract tests; these do not prove the checks being orchestrated."""
import contextlib
import importlib.util
import io
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ExecutionTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name("dev_verify.py")
        self.assertTrue(source.exists(), "verification runner is not implemented")
        spec = importlib.util.spec_from_file_location("dev_verify", source)
        self.runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.runner)
        self.temp = tempfile.TemporaryDirectory(prefix="coven verify ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def run_commands(self, commands):
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = self.runner.execute(commands, self.root)
        return status, output.getvalue()

    def test_visible_command_and_pass(self):
        status, output = self.run_commands([
            ("example", [sys.executable, "-c", "pass"]),
        ])
        self.assertEqual(status, 0)
        self.assertIn("RUN example:", output)
        self.assertIn(sys.executable, output)
        self.assertIn("PASS example", output)

    def test_first_failure_preserved_and_later_commands_execute(self):
        marker = self.root / "later marker"
        status, output = self.run_commands([
            ("first", [sys.executable, "-c", "raise SystemExit(23)"]),
            ("second", [sys.executable, "-c", "raise SystemExit(7)"]),
            ("later", [sys.executable, "-c",
                       "from pathlib import Path; Path('later marker').touch()"]),
        ])
        self.assertEqual(status, 23)
        self.assertTrue(marker.is_file())
        self.assertIn("FAIL first (exit 23)", output)
        self.assertIn("FAIL second (exit 7)", output)
        self.assertIn("PASS later", output)
        self.assertNotIn("PASS first", output)

    def test_missing_tool_is_failure_not_skip(self):
        status, output = self.run_commands([
            ("missing", [str(self.root / "nonexistent")]),
        ])
        self.assertEqual(status, 127)
        self.assertIn("missing executable", output)
        self.assertIn("FAIL missing (exit 127)", output)
        self.assertNotIn("SKIP", output)

    def test_permission_denied_is_failure(self):
        command = self.root / "not executable"
        command.write_text("#!/bin/sh\nexit 0\n")
        command.chmod(0o600)
        status, output = self.run_commands([("denied", [str(command)])])
        self.assertEqual(status, 126)
        self.assertIn("FAIL denied (exit 126)", output)

    def test_arguments_are_not_shell_code(self):
        argument = "$(touch INJECTED); `touch INJECTED`"
        status, _ = self.run_commands([
            ("literal", [sys.executable, "-c",
                         "import sys; from pathlib import Path; "
                         "Path('argument').write_text(sys.argv[1])", argument]),
        ])
        self.assertEqual(status, 0)
        self.assertEqual((self.root / "argument").read_text(), argument)
        self.assertFalse((self.root / "INJECTED").exists())

    def test_signal_is_failure_with_shell_exit_status(self):
        status, output = self.run_commands([
            ("signal", [sys.executable, "-c",
                        "import os, signal; os.kill(os.getpid(), signal.SIGTERM)"]),
        ])
        self.assertEqual(status, 143)
        self.assertIn("FAIL signal (exit 143)", output)

    def test_child_output_is_streamed(self):
        # Capture OS-level stdout/stderr in an outer process, as real callers do.
        code = (
            "import sys; sys.path.insert(0, sys.argv[1]); import dev_verify; "
            "from pathlib import Path; "
            "raise SystemExit(dev_verify.execute([('child', [sys.executable, '-c', "
            "\"import sys; print('child stdout'); print('child stderr', file=sys.stderr)\"])]"
            ", Path(sys.argv[2])))"
        )
        result = subprocess.run(
            [sys.executable, "-c", code, str(Path(__file__).parent), str(self.root)],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("child stdout", result.stdout)
        self.assertIn("child stderr", result.stderr)
        self.assertLess(result.stdout.index("RUN child:"), result.stdout.index("child stdout"))
        self.assertLess(result.stdout.index("child stdout"), result.stdout.index("PASS child"))


if __name__ == "__main__":
    unittest.main()
