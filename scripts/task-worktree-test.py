"""Exercise task allocation against real temporary Git repositories."""
import pathlib
import json
import os
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name('task-worktree.py')


class GitFixture(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.repo = self.root / 'repo'
        self.repo.mkdir()
        self.git('init', '-b', 'main')
        self.git('config', 'user.name', 'Fixture')
        self.git('config', 'user.email', 'fixture@example.invalid')
        self.git('commit', '--allow-empty', '-m', 'base')
        self.git('update-ref', 'refs/remotes/origin/main', 'HEAD')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True).strip()

    def run_tool(self, *args, env=None):
        return subprocess.run(['python3', str(SCRIPT), *args], cwd=self.repo, capture_output=True, text=True, env=env)


class AllocationTests(GitFixture):

    def test_create_starts_at_origin_main_and_does_not_track_it(self):
        destination = self.root / 'task'
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(destination))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(destination.is_dir())
        self.assertEqual(self.git('rev-parse', 'fix/42'), self.git('rev-parse', 'origin/main'))
        self.assertEqual(subprocess.run(['git', '-C', str(destination), 'rev-parse', '@{upstream}'], capture_output=True).returncode, 128)

    def test_local_branch_cap_refuses_without_creating_any_artifact(self):
        for n in range(32):
            self.git('branch', f'held/{n}')
        destination = self.root / 'task'
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(destination))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('local branches', result.stderr)
        self.assertFalse(destination.exists())
        self.assertNotIn('fix/42', self.git('branch', '--format=%(refname:short)'))

    def test_existing_allocator_lock_fails_closed(self):
        (self.repo / '.git' / 'task-worktree-allocation.lock').mkdir()
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(self.root / 'task'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('allocation lock', result.stderr)

    def test_existing_destination_is_preserved(self):
        destination = self.root / 'task'
        destination.mkdir()
        marker = destination / 'unique.txt'
        marker.write_text('preserve')
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(destination))
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(marker.read_text(), 'preserve')
        self.assertNotIn('fix/42', self.git('branch', '--format=%(refname:short)'))

    def test_origin_cap_refuses_even_with_local_capacity(self):
        for n in range(49):
            self.git('update-ref', f'refs/remotes/origin/held/{n}', 'HEAD')
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(self.root / 'task'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('origin branches', result.stderr)
        self.assertNotIn('fix/42', self.git('branch', '--format=%(refname:short)'))

    def test_detached_worktrees_count_toward_cap(self):
        for n in range(12):
            self.git('worktree', 'add', '--detach', str(self.root / f'held-{n}'), 'HEAD')
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(self.root / 'task'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('task worktrees', result.stderr)

    def test_check_allows_exact_limit_but_creation_refuses_next_branch(self):
        for n in range(32):
            self.git('branch', f'held/{n}')
        result = self.run_tool('check')
        self.assertEqual(result.returncode, 0, result.stderr)
        result = self.run_tool('create', '--issue', '42', '--branch', 'fix/42', '--path', str(self.root / 'task'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('local branches', result.stderr)

    def test_competing_allocations_cannot_consume_one_remaining_slot_twice(self):
        for n in range(31):
            self.git('branch', f'held/{n}')
        processes = [subprocess.Popen(
            ['python3', str(SCRIPT), 'create', '--issue', str(42 + n), '--branch', f'fix/{42 + n}', '--path', str(self.root / f'task-{n}')],
            cwd=self.repo, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        ) for n in range(2)]
        results = [(p.communicate(), p.returncode) for p in processes]
        self.assertEqual(sum(status == 0 for _, status in results), 1, results)
        self.assertEqual(sum((self.root / f'task-{n}').exists() for n in range(2)), 1)
        self.assertEqual(len(self.git('branch', '--format=%(refname:short)').splitlines()), 33)


class PublicationTests(GitFixture):
    def setUp(self):
        super().setUp()
        self.remote = self.root / 'remote.git'
        subprocess.run(['git', 'init', '--bare', '-b', 'main', str(self.remote)], check=True, capture_output=True)
        self.git('remote', 'add', 'origin', str(self.remote))
        self.git('push', 'origin', 'main')
        self.git('checkout', '-b', 'fix/42')
        self.git('commit', '--allow-empty', '-m', 'task')

    def fill_remote(self, count):
        for n in range(count):
            subprocess.run(['git', '--git-dir', str(self.remote), 'update-ref', f'refs/heads/held/{n}', self.git('rev-parse', 'main')], check=True)

    def remote_heads(self):
        return self.git('ls-remote', '--heads', 'origin')

    def exception_environment(self, state='open'):
        tools = self.root / 'tools'
        tools.mkdir()
        gh = tools / 'gh'
        gh.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys
if 'POST' in sys.argv:
    pathlib.Path(os.environ['EXCEPTION_LOG']).write_text(sys.stdin.read())
    print('{"id":1}')
else:
    print(json.dumps({'state':os.environ['EXCEPTION_STATE']}))
''')
        gh.chmod(0o755)
        return dict(os.environ, PATH=str(tools) + os.pathsep + os.environ['PATH'],
                    EXCEPTION_LOG=str(self.root / 'exception.json'), EXCEPTION_STATE=state)

    def test_publish_new_task_binds_the_exact_head(self):
        result = self.run_tool('publish', '--issue', '42')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.git('rev-parse', 'HEAD') + '\trefs/heads/fix/42', self.remote_heads())

    def test_live_remote_limit_refuses_even_with_stale_tracking_refs(self):
        self.fill_remote(48)
        result = self.run_tool('publish', '--issue', '42')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('origin branches', result.stderr)
        self.assertNotIn('refs/heads/fix/42', self.remote_heads())

    def test_existing_remote_task_can_continue_above_the_limit(self):
        self.fill_remote(48)
        self.git('push', 'origin', 'fix/42')
        self.git('commit', '--allow-empty', '-m', 'update')
        result = self.run_tool('publish', '--issue', '42')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.git('rev-parse', 'HEAD') + '\trefs/heads/fix/42', self.remote_heads())

    def test_main_is_never_published(self):
        self.git('checkout', 'main')
        result = self.run_tool('publish', '--issue', '42')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('main', result.stderr)

    def test_budget_exception_is_recorded_before_publication(self):
        self.fill_remote(48)
        env = self.exception_environment()
        result = self.run_tool('publish', '--issue', '42', '--exception-issue', '1626', '--reason', 'Finish the policy PR.\nRetire delivered refs after CI.', env=env)
        self.assertEqual(result.returncode, 0, result.stderr)
        payload = json.loads((self.root / 'exception.json').read_text())
        self.assertIn(self.git('rev-parse', 'HEAD'), payload['body'])
        self.assertIn('48', payload['body'])
        self.assertTrue(all('\n' not in paragraph for paragraph in payload['body'].strip().split('\n\n')))
        self.assertIn('refs/heads/fix/42', self.remote_heads())

    def test_closed_exception_issue_cannot_authorize_new_growth(self):
        self.fill_remote(48)
        result = self.run_tool('publish', '--issue', '42', '--exception-issue', '1626', '--reason', 'Deliver policy', env=self.exception_environment('closed'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('open exception issue', result.stderr)
        self.assertFalse((self.root / 'exception.json').exists())
        self.assertNotIn('refs/heads/fix/42', self.remote_heads())

    def test_non_fast_forward_update_is_not_forced(self):
        self.git('push', 'origin', 'fix/42')
        original = self.git('rev-parse', 'HEAD')
        self.git('reset', '--hard', 'main')
        self.git('commit', '--allow-empty', '-m', 'different history')
        result = self.run_tool('publish', '--issue', '42')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(original + '\trefs/heads/fix/42', self.remote_heads())

    def test_competing_publications_share_the_last_remote_slot(self):
        self.fill_remote(47)
        second = self.root / 'second-task'
        self.git('worktree', 'add', '-b', 'fix/43', str(second), 'main')
        processes = [subprocess.Popen(
            ['python3', str(SCRIPT), 'publish', '--issue', str(issue)], cwd=directory,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        ) for issue, directory in [(42, self.repo), (43, second)]]
        results = [(p.communicate(), p.returncode) for p in processes]
        self.assertEqual(sum(status == 0 for _, status in results), 1, results)
        self.assertEqual(len(self.remote_heads().splitlines()) - 1, 48)

    def alternate_push_remote(self):
        remote = self.root / 'push.git'
        subprocess.run(['git', 'init', '--bare', '-b', 'main', str(remote)], check=True, capture_output=True)
        self.git('push', str(remote), 'main')
        self.git('remote', 'set-url', '--push', 'origin', str(remote))
        return remote

    def test_counts_the_effective_push_destination(self):
        remote = self.alternate_push_remote()
        for n in range(48):
            subprocess.run(['git', '--git-dir', str(remote), 'update-ref', f'refs/heads/held/{n}', self.git('rev-parse', 'main')], check=True)
        result = self.run_tool('publish', '--issue', '42')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('origin branches', result.stderr)
        self.assertNotIn('refs/heads/fix/42', self.git('ls-remote', '--heads', str(remote)))

    def test_single_distinct_push_destination_can_publish_with_capacity(self):
        remote = self.alternate_push_remote()
        result = self.run_tool('publish', '--issue', '42')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(self.git('rev-parse', 'HEAD') + '\trefs/heads/fix/42', self.git('ls-remote', '--heads', str(remote)))
        self.assertNotIn('refs/heads/fix/42', self.remote_heads())

    def test_multiple_push_destinations_are_refused(self):
        remote = self.alternate_push_remote()
        self.git('config', '--add', 'remote.origin.pushurl', str(self.remote))
        result = self.run_tool('publish', '--issue', '42')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('one push destination', result.stderr)
        self.assertNotIn('refs/heads/fix/42', self.remote_heads())
        self.assertNotIn('refs/heads/fix/42', self.git('ls-remote', '--heads', str(remote)))


if __name__ == '__main__':
    unittest.main()
