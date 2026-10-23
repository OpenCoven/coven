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


class TierTests(unittest.TestCase):
    setUp = ExecutionTests.setUp

    def test_quick_excludes_expensive_checks_and_reports_scope(self):
        checks, skips = self.runner.plan('quick', 'origin/main...HEAD', [], Path(__file__).resolve().parents[1], 'Darwin')
        commands = [check['argv'] for check in checks]
        self.assertIn(['cargo', 'check', '--workspace', '--locked'], commands)
        self.assertIn(['python3', 'scripts/check-secrets.py'], commands)
        self.assertIn(['python3', 'scripts/check-coven-privacy.py', '--range', 'origin/main...HEAD'], commands)
        self.assertNotIn(['cargo', 'test', '--workspace', '--locked'], commands)
        self.assertTrue(any('quick' in skip for skip in skips))

    def test_pr_retains_security_and_routes_package_changes(self):
        checks, skips = self.runner.plan('pr', 'base...HEAD', ['packages/channels/src/index.ts'], Path(__file__).resolve().parents[1], 'Darwin')
        commands = [check['argv'] for check in checks]
        self.assertIn(['cargo', 'test', '--workspace', '--locked'], commands)
        self.assertIn(['cargo', 'clippy', '--workspace', '--all-targets', '--', '-D', 'warnings'], commands)
        self.assertIn(['python3', 'scripts/check-docs-ownership.py', '--range', 'base...HEAD'], commands)
        self.assertIn(['cargo', 'deny', '--all-features', 'check', 'advisories', 'licenses', 'bans', 'sources'], commands)
        self.assertIn(['npm', 'ci'], commands)
        self.assertIn(['npm', 'test'], commands)
        self.assertNotIn(['pnpm', 'test'], commands)
        self.assertTrue(any('openclaw' in skip for skip in skips))

    def test_browser_contract_setup_precedes_tests_and_readiness_is_conditional(self):
        checks, skips = self.runner.plan('pr', 'base...HEAD', [], Path(__file__).resolve().parents[1], 'Darwin')
        setup = next(check for check in checks if check['argv'][:4] == ['npm', 'ci', '--prefix', 'scripts/browser'])
        contract = next(check for check in checks if check['argv'] == ['node', '--test', 'scripts/browser-capability.test.mjs'])
        self.assertIn(setup['name'], contract['needs'])
        self.assertIn('node-pin', setup['needs'])
        readiness = next(check for check in checks if check['name'] == 'browser-readiness')
        self.assertEqual(readiness['argv'], ['node', 'scripts/browser-capability.mjs', '--check'])
        self.assertIn('node-pin', readiness['needs'])
        self.assertFalse(any('--install' in check['argv'] for check in checks))

    def test_failed_package_install_skips_browser_readiness(self):
        checks, _ = self.runner.plan('pr', 'base...HEAD', ['packages/channels/package.json'], Path(__file__).resolve().parents[1], 'Darwin')
        readiness = next(check for check in checks if check['name'] == 'browser-readiness')
        self.assertIn('channels-install', readiness['needs'])
        self.assertNotIn('openclaw-coven-install', readiness['needs'])
        marker = self.root / 'browser incorrectly ran'
        simulated = [dict(name=name, argv=[sys.executable, '-c', 'raise SystemExit(9)' if name == 'channels-install' else 'pass'], cwd='.', needs=[]) for name in readiness['needs']]
        simulated.append({**readiness, 'argv': [sys.executable, '-c', "from pathlib import Path; Path('browser incorrectly ran').touch()"]})
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = self.runner.run_checks(simulated, self.root)
        self.assertEqual(status, 9)
        self.assertFalse(marker.exists())
        self.assertIn('SKIP browser-readiness', output.getvalue())

    def test_full_includes_features_and_all_packages(self):
        checks, skips = self.runner.plan('full', 'base...HEAD', [], Path(__file__).resolve().parents[1], 'Darwin')
        commands = [check['argv'] for check in checks]
        self.assertIn(['pnpm', 'install', '--frozen-lockfile', '--ignore-scripts'], commands)
        self.assertIn(['npm', 'ci'], commands)
        self.assertIn(['cargo', 'test', '-p', 'coven-afs', '--features', 'mount', '--locked'], commands)
        self.assertTrue(any('threads-test-clock' in argv for argv in commands))
        self.assertTrue(any('Windows' in skip for skip in skips))

    def test_failed_prerequisite_never_runs_dependent_check(self):
        marker = self.root / 'incorrectly ran'
        checks = [
            dict(name='setup', argv=[sys.executable, '-c', 'raise SystemExit(9)'], cwd='.', needs=[]),
            dict(name='dependent', argv=[sys.executable, '-c', "from pathlib import Path; Path('incorrectly ran').touch()"], cwd='.', needs=['setup']),
            dict(name='independent', argv=[sys.executable, '-c', 'pass'], cwd='.', needs=[]),
        ]
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = self.runner.run_checks(checks, self.root)
        self.assertEqual(status, 9)
        self.assertFalse(marker.exists())
        self.assertIn('SKIP dependent', output.getvalue())
        self.assertIn('PASS independent', output.getvalue())

    def test_policy_drift_fails_closed(self):
        repo = Path(__file__).resolve().parents[1]
        self.runner.validate_inventory(repo)
        import shutil
        (self.root / '.github/workflows').mkdir(parents=True)
        (self.root / 'scripts').mkdir()
        shutil.copy(repo / 'scripts/dev-verify-policy.json', self.root / 'scripts')
        workflow = (repo / '.github/workflows/ci.yml').read_text()
        workflow = workflow.replace('      - run: python scripts/check-secrets.py', '      - run: python scripts/check-secrets.py\n      - run: python scripts/new-required-guard.py')
        (self.root / '.github/workflows/ci.yml').write_text(workflow)
        with self.assertRaisesRegex(ValueError, 'inventory.*drift'):
            self.runner.validate_inventory(self.root)

    def test_cli_help_and_usage_do_not_run_checks(self):
        script = Path(__file__).with_name('dev-verify.sh')
        self.assertTrue(script.exists(), 'shell entry point is not implemented')
        result = subprocess.run(['bash', str(script), '--help'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0)
        self.assertIn('--quick', result.stdout)
        self.assertNotIn('RUN ', result.stdout)
        result = subprocess.run(['bash', str(script), '--quick', '--full'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn('RUN ', result.stdout)


class PreflightTests(unittest.TestCase):
    setUp = ExecutionTests.setUp

    def init_repo(self):
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=1+test@users.noreply.github.com', 'commit', '--allow-empty', '-qm', 'baseline'], cwd=self.root, check=True)

    def test_changed_paths_include_committed_staged_unstaged_untracked(self):
        self.init_repo()
        base = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=self.root, text=True).strip()
        (self.root / 'committed').touch()
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=1+test@users.noreply.github.com', 'commit', '-qm', 'change'], cwd=self.root, check=True)
        (self.root / 'staged').touch()
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        (self.root / 'committed').write_text('unstaged')
        (self.root / 'untracked space').touch()
        self.assertEqual(self.runner.changed_paths(self.root, base + '...HEAD'), ['committed', 'staged', 'untracked space'])

    def test_staged_change_restored_in_worktree_still_routes_package(self):
        self.init_repo()
        path = self.root / 'packages/channels/source.ts'
        path.parent.mkdir(parents=True)
        path.write_text('original')
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=1+test@users.noreply.github.com', 'commit', '-qm', 'original'], cwd=self.root, check=True)
        path.write_text('staged change')
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        path.write_text('original')
        self.assertEqual(self.runner.changed_paths(self.root, 'HEAD...HEAD'), ['packages/channels/source.ts'])

    def test_range_resolves_refs_before_other_worktrees_can_move_them(self):
        self.init_repo()
        original = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=self.root, text=True).strip()
        subprocess.run(['git', 'branch', 'base'], cwd=self.root, check=True)
        resolved = self.runner.resolve_range(self.root, 'base...HEAD')
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=1+test@users.noreply.github.com', 'commit', '--allow-empty', '-qm', 'next'], cwd=self.root, check=True)
        subprocess.run(['git', 'branch', '-f', 'base', 'HEAD'], cwd=self.root, check=True)
        self.assertEqual(resolved, original + '...' + original)
        self.assertNotEqual(resolved, self.runner.resolve_range(self.root, 'base...HEAD'))

    def test_bad_revision_fails_before_any_execution(self):
        script = Path(__file__).with_name('dev-verify.sh')
        result = subprocess.run(['bash', str(script), '--quick', '--range', 'nonexistent-base...HEAD'], capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('RUN ', result.stdout)
        self.assertIn('FAIL verification preflight', result.stderr)

    def test_plan_from_other_directory_never_claims_execution(self):
        script = Path(__file__).resolve().with_name('dev-verify.sh')
        result = subprocess.run(['bash', str(script), '--quick', '--plan'], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('PLAN check', result.stdout)
        self.assertNotIn('PASS ', result.stdout)
        self.assertNotIn('RUN ', result.stdout)

    def test_docs_failure_does_not_suppress_working_privacy(self):
        import shutil
        self.init_repo()
        (self.root / 'scripts').mkdir()
        repo = Path(__file__).resolve().parents[1]
        for filename in ('check-docs-ownership.py', 'check-coven-privacy.py'):
            shutil.copy(repo / 'scripts' / filename, self.root / 'scripts')
        (self.root / 'docs/guides').mkdir(parents=True)
        (self.root / 'docs/guides/invalid.md').write_text('# Unowned public doc')
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            status = self.runner.internal_check('working-policy', self.root)
        self.assertNotEqual(status, 0)
        self.assertIn('RUN privacy-working:', output.getvalue())

    def test_loading_classifier_does_not_dirty_audited_checkout(self):
        import shutil
        (self.root / 'scripts').mkdir()
        shutil.copy(Path(__file__).with_name('classify-ci-changes.py'), self.root / 'scripts')
        from unittest.mock import patch
        with patch.object(sys, 'dont_write_bytecode', False), patch.object(sys, 'pycache_prefix', None):
            self.runner.load_script(self.root, 'classify-ci-changes')
        self.assertFalse((self.root / 'scripts/__pycache__').exists())

    def test_wrong_node_pin_is_rejected(self):
        (self.root / '.node-version').write_text('0.0.0')
        with contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaisesRegex(ValueError, 'node pin mismatch'):
                self.runner.internal_check('node-pin', self.root)

if __name__ == "__main__":
    unittest.main()
