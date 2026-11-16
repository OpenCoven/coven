#!/usr/bin/env python3
"""Run propose-engine-bump.sh against a local origin and a recording stand-in for gh."""
from __future__ import annotations

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / 'scripts' / 'propose-engine-bump.sh'
WORKFLOW = (ROOT / '.github' / 'workflows' / 'engine-bump.yml').read_text(encoding='utf-8')
LOCK = 'crates/coven-cli/engine.lock'
BRANCH = 'engine-bump/1.2.3'
PIN_STAND_IN = '#!/bin/sh\nprintf \'version = "%s"\\n%s\\n\' "$1" "${PIN_BODY:-digests}" > crates/coven-cli/engine.lock\n'
GH_STAND_IN = '''#!/bin/sh
printf '%s\\n' "$*" >> "$GH_RECORD"
case "$1 $2" in
  "pr list") printf '%s\\n' "${GH_PR_COUNT:-0}" ;;
  "pr create") exit "${GH_CREATE_EXIT:-0}" ;;
  *) echo "unexpected gh call: $*" >&2; exit 64 ;;
esac
'''


class ProposeEngineBumpTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.directory, ignore_errors=True)
        self.origin = self.directory / 'origin.git'
        self.work = self.directory / 'work'
        self.record = self.directory / 'gh-calls'
        tools = self.directory / 'bin'
        tools.mkdir()
        (tools / 'gh').write_text(GH_STAND_IN, encoding='utf-8')
        (tools / 'gh').chmod(0o700)
        self.env = {
            'PATH': f"{tools}:{os.environ['PATH']}", 'HOME': str(self.directory),
            'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_NOSYSTEM': '1', 'GH_RECORD': str(self.record),
            'GIT_AUTHOR_NAME': 'fixture', 'GIT_AUTHOR_EMAIL': 'fixture@example.invalid',
            'GIT_COMMITTER_NAME': 'fixture', 'GIT_COMMITTER_EMAIL': 'fixture@example.invalid',
        }
        self.git('init', '--quiet', '--bare', '--initial-branch=main', str(self.origin), cwd=self.directory)
        self.git('init', '--quiet', '--initial-branch=main', str(self.work), cwd=self.directory)
        (self.work / 'scripts').mkdir()
        (self.work / 'crates' / 'coven-cli').mkdir(parents=True)
        shutil.copy(SCRIPT, self.work / 'scripts' / 'propose-engine-bump.sh')
        (self.work / 'scripts' / 'pin-engine.sh').write_text(PIN_STAND_IN, encoding='utf-8')
        (self.work / LOCK).write_text('version = "1.2.2"\ndigests\n', encoding='utf-8')
        self.git('add', '.')
        self.git('commit', '--quiet', '-m', 'fixture')
        self.git('remote', 'add', 'origin', str(self.origin))
        self.git('push', '--quiet', 'origin', 'main')

    def git(self, *args: str, cwd: Path | None = None) -> str:
        done = subprocess.run(['git', *args], cwd=cwd or self.work, env=self.env,
                              capture_output=True, text=True, timeout=30)
        self.assertEqual(done.returncode, 0, done.stderr)
        return done.stdout.strip()

    def propose(self, version: str = '1.2.3', **env: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(['bash', 'scripts/propose-engine-bump.sh', version], cwd=self.work,
                              env={**self.env, **env}, capture_output=True, text=True, timeout=30)

    def gh_calls(self) -> list[str]:
        return self.record.read_text(encoding='utf-8').splitlines() if self.record.exists() else []

    def origin_head(self, branch: str = BRANCH) -> str:
        return self.git('ls-remote', str(self.origin), f'refs/heads/{branch}').split('\t')[0]

    def push_branch_without_pull_request(self) -> str:
        """Leave origin as a run that pushed the branch and could not open the pull request."""
        refused = self.propose(GH_CREATE_EXIT='1')
        self.assertEqual(refused.returncode, 1, refused.stdout + refused.stderr)
        self.record.unlink()
        self.git('checkout', '--quiet', 'main')
        return self.origin_head()

    def test_new_release_pushes_only_the_lock_and_opens_the_pull_request(self) -> None:
        (self.work / 'unrelated').write_text('left in the checkout by an earlier step\n', encoding='utf-8')
        done = self.propose()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.git('--git-dir', str(self.origin), 'show', f'{BRANCH}:{LOCK}', cwd=self.directory),
                         'version = "1.2.3"\ndigests')
        self.assertEqual(self.git('--git-dir', str(self.origin), 'diff', '--name-only', 'main', BRANCH,
                                  cwd=self.directory), LOCK)
        self.assertEqual(self.git('--git-dir', str(self.origin), 'log', '-1', '--format=%s', BRANCH,
                                  cwd=self.directory), 'chore(engine): bump pinned engine to 1.2.3')
        calls = self.gh_calls()
        self.assertEqual(len(calls), 1)
        self.assertTrue(calls[0].startswith('pr create --title chore(engine): bump pinned engine to 1.2.3 '))
        self.assertTrue(calls[0].endswith(f' --base main --head {BRANCH}'))

    def test_refused_pull_request_fails_and_names_the_secret(self) -> None:
        done = self.propose(GH_CREATE_EXIT='1')
        self.assertEqual(done.returncode, 1)
        self.assertIn('::error::Could not open the pull request', done.stdout)
        self.assertIn('ENGINE_BUMP_TOKEN', done.stdout)
        self.assertTrue(self.origin_head())

    def test_pushed_branch_without_a_pull_request_gets_one(self) -> None:
        pushed = self.push_branch_without_pull_request()
        done = self.propose()
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.origin_head(), pushed)
        calls = self.gh_calls()
        self.assertEqual(calls[0], f'pr list --head {BRANCH} --state all --json number --jq length')
        self.assertTrue(calls[1].startswith('pr create '))
        self.assertEqual(len(calls), 2)

    def test_pushed_branch_keeps_failing_until_the_pull_request_opens(self) -> None:
        self.push_branch_without_pull_request()
        done = self.propose(GH_CREATE_EXIT='1')
        self.assertEqual(done.returncode, 1)
        self.assertIn('ENGINE_BUMP_TOKEN', done.stdout)

    def test_branch_that_already_had_a_pull_request_is_left_alone(self) -> None:
        pushed = self.push_branch_without_pull_request()
        done = self.propose(GH_PR_COUNT='1')
        self.assertEqual(done.returncode, 0, done.stdout + done.stderr)
        self.assertEqual(self.origin_head(), pushed)
        self.assertEqual(len(self.gh_calls()), 1)
        self.assertIn('already exists', done.stdout)

    def test_pushed_branch_with_a_stale_lock_is_refused(self) -> None:
        pushed = self.push_branch_without_pull_request()
        done = self.propose(PIN_BODY='replaced digests')
        self.assertEqual(done.returncode, 1)
        self.assertIn('::error::', done.stdout)
        self.assertIn('Delete the branch', done.stdout)
        self.assertEqual(self.origin_head(), pushed)
        self.assertEqual(len(self.gh_calls()), 1)

    def test_pull_request_opened_with_the_workflow_token_fails_the_run(self) -> None:
        done = self.propose(WORKFLOW_TOKEN_ONLY='true')
        self.assertEqual(done.returncode, 1)
        self.assertEqual(len(self.gh_calls()), 1)
        self.assertIn('no CI started', done.stdout)
        self.assertIn('ENGINE_BUMP_TOKEN', done.stdout)

    def test_version_that_is_not_stable_is_refused_before_any_change(self) -> None:
        for version in ('v1.2.3', '1.2', '1.2.3-rc.1', '1.2.3;x', ''):
            with self.subTest(version=version):
                done = self.propose(version)
                self.assertNotEqual(done.returncode, 0)
                self.assertEqual(self.gh_calls(), [])
                self.assertEqual(self.origin_head(), '')
                self.assertEqual(self.git('status', '--porcelain'), '')

    def test_workflow_runs_the_script_with_the_secret_and_says_when_it_is_missing(self) -> None:
        self.assertIn('  schedule:\n', WORKFLOW)
        self.assertIn('  workflow_dispatch:\n', WORKFLOW)
        self.assertIn('concurrency:\n  group: engine-bump\n  cancel-in-progress: false\n', WORKFLOW)
        self.assertIn('          persist-credentials: true\n'
                      '          token: ${{ secrets.ENGINE_BUMP_TOKEN || github.token }}\n', WORKFLOW)
        step = WORKFLOW.split('      - name: Regenerate the pin and open a PR\n', 1)[1]
        self.assertIn("        if: steps.detect.outputs.newer == 'true'\n", step)
        self.assertIn('          GH_TOKEN: ${{ secrets.ENGINE_BUMP_TOKEN || github.token }}\n', step)
        self.assertIn("          WORKFLOW_TOKEN_ONLY: ${{ secrets.ENGINE_BUMP_TOKEN == '' }}\n", step)
        self.assertTrue(step.endswith('        run: bash scripts/propose-engine-bump.sh "$LATEST"\n'))


if __name__ == '__main__':
    unittest.main()
