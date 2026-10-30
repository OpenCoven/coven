"""Remote triage must preserve local-only work even with a gone upstream."""
import os
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

TRIAGE = Path(__file__).resolve().parents[1] / '.github/actions/branch-triage/triage.sh'


class LocalPreservationTests(unittest.TestCase):
    def check_mode(self, dry, task_checkout=False):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            remote = root / 'remote.git'
            repo = root / 'repo'
            subprocess.run(['git', 'init', '--bare', '-b', 'main', str(remote)], check=True, capture_output=True)
            subprocess.run(['git', 'clone', str(remote), str(repo)], check=True, capture_output=True)

            def git(*args):
                return subprocess.check_output(['git', '-C', str(repo), *args], text=True, stderr=subprocess.DEVNULL).strip()

            git('config', 'user.name', 'Fixture')
            git('config', 'user.email', 'fixture@example.invalid')
            git('commit', '--allow-empty', '-m', 'base')
            git('push', '-u', 'origin', 'main')
            git('checkout', '-b', 'unique-local')
            git('commit', '--allow-empty', '-m', 'unique work')
            head = git('rev-parse', 'HEAD')
            git('update-ref', 'refs/remotes/origin/deleted-upstream', head)
            git('config', 'branch.unique-local.remote', 'origin')
            git('config', 'branch.unique-local.merge', 'refs/heads/deleted-upstream')
            git('checkout', 'main')
            if task_checkout:
                git('checkout', 'unique-local')
                (repo / 'retained.txt').write_text('untracked task data')
            tools = root / 'tools'
            tools.mkdir()
            gh = tools / 'gh'
            gh.write_text('#!/bin/sh\nprintf "[]\\n"\n')
            gh.chmod(0o755)
            env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'], DRY_RUN=str(dry).lower())
            result = subprocess.run(['bash', str(TRIAGE)], cwd=repo, env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            refs = git('for-each-ref', '--format=%(refname:short) %(objectname)', 'refs/heads')
            self.assertIn('unique-local ' + head, refs, 'remote triage deleted unique local work')
            if task_checkout:
                self.assertEqual(git('branch', '--show-current'), 'unique-local')
                self.assertEqual((repo / 'retained.txt').read_text(), 'untracked task data')

    def test_dry_run_preserves_local_branch_with_gone_upstream(self):
        self.check_mode(True)

    def test_live_remote_triage_preserves_local_branch_with_gone_upstream(self):
        self.check_mode(False)

    def test_dry_run_keeps_the_current_checkout(self):
        self.check_mode(True, task_checkout=True)


class RemotePreservationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.remote = self.root / 'remote.git'
        self.repo = self.root / 'repo'
        subprocess.run(['git', 'init', '--bare', '-b', 'main', str(self.remote)], check=True, capture_output=True)
        subprocess.run(['git', 'clone', str(self.remote), str(self.repo)], check=True, capture_output=True)
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('commit', '--allow-empty', '-m', 'base')
        self.base = self.git('rev-parse', 'HEAD')
        self.git('checkout', '-b', 'candidate')
        self.git('commit', '--allow-empty', '-m', 'PR work')
        self.head = self.git('rev-parse', 'HEAD')
        self.git('checkout', 'main')
        self.git('commit', '--allow-empty', '-m', 'squash delivery')
        self.merge = self.git('rev-parse', 'HEAD')
        self.git('push', '-u', 'origin', 'main', 'candidate')
        # Keep all transport local while testing the origin-owner predicate.
        self.git('config', f'url.{self.remote}.insteadOf', 'https://github.com/fixture/repo.git')
        self.git('remote', 'set-url', 'origin', 'https://github.com/fixture/repo.git')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True, stderr=subprocess.DEVNULL).strip()

    def run_triage(self, move=False, merged_oid=None, late_open=False, api_error=False):
        tools = self.root / 'tools'
        tools.mkdir()
        gh = tools / 'gh'
        gh.write_text('''#!/usr/bin/env python3
import json, os, subprocess, sys
if sys.argv[1] == 'api':
    if os.environ.get('API_ERROR'):
        sys.exit(1)
    print('candidate' if os.environ.get('LATE_OPEN') else '')
elif '--state' in sys.argv and sys.argv[sys.argv.index('--state')+1] == 'merged':
    if os.environ.get('MOVE_REMOTE'):
        subprocess.run(['git', '--git-dir', os.environ['TEST_REMOTE'], 'update-ref', 'refs/heads/candidate', os.environ['MOVE_REMOTE']], check=True)
    print(os.environ['MERGED_JSON'])
else:
    print('[]')
''')
        gh.chmod(0o755)
        merged = [{'headRefName': 'candidate', 'headRefOid': self.head,
                   'headRepositoryOwner': {'login': 'fixture'},
                   'mergeCommit': {'oid': merged_oid or self.merge}}]
        env = dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                   DRY_RUN='false', MERGED_JSON=json.dumps(merged), TEST_REMOTE=str(self.remote))
        if move:
            env['MOVE_REMOTE'] = self.merge
        if late_open:
            env['LATE_OPEN'] = 'true'
        if api_error:
            env['API_ERROR'] = 'true'
        result = subprocess.run(['bash', str(TRIAGE)], cwd=self.repo, env=env, capture_output=True, text=True)
        # A total REST outage holds deletion and also fails final-state
        # reporting; it must not be presented as a successful inventory.
        self.assertEqual(result.returncode, 1 if api_error else 0, result.stdout + result.stderr)
        return result

    def remote_refs(self):
        return self.git('ls-remote', '--heads', 'origin')

    def test_reused_merged_branch_with_new_commits_is_preserved(self):
        self.git('checkout', 'candidate')
        self.git('commit', '--allow-empty', '-m', 'new work after merge')
        advanced = self.git('rev-parse', 'HEAD')
        self.git('push', 'origin', 'candidate')
        self.git('checkout', 'main')
        self.run_triage()
        self.assertIn(advanced + '\trefs/heads/candidate', self.remote_refs())

    def test_remote_head_advanced_after_fetch_is_preserved(self):
        self.run_triage(move=True)
        self.assertIn(self.merge + '\trefs/heads/candidate', self.remote_refs())

    def test_merge_into_a_different_base_is_not_delivery_proof(self):
        self.run_triage(merged_oid=self.head)
        self.assertIn(self.head + '\trefs/heads/candidate', self.remote_refs())

    def test_exact_merged_head_with_delivered_merge_can_be_deleted(self):
        self.run_triage()
        self.assertNotIn('refs/heads/candidate', self.remote_refs())

    def test_open_pr_discovered_after_classification_holds_the_ref(self):
        result = self.run_triage(late_open=True)
        self.assertIn(self.head + '\trefs/heads/candidate', self.remote_refs())
        self.assertIn('deleted=0', result.stdout)

    def test_open_pr_refresh_failure_holds_the_ref(self):
        result = self.run_triage(api_error=True)
        self.assertIn(self.head + '\trefs/heads/candidate', self.remote_refs())
        self.assertIn('could not refresh open PR heads', result.stdout)


if __name__ == '__main__':
    unittest.main()
