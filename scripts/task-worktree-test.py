"""Exercise task allocation against real temporary Git repositories."""
import pathlib
import subprocess
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name('task-worktree.py')


class AllocationTests(unittest.TestCase):
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
        return subprocess.check_output(['git', '-C', str(self.repo), *args], text=True)

    def run_tool(self, *args):
        return subprocess.run(['python3', str(SCRIPT), *args], cwd=self.repo, capture_output=True, text=True)

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


if __name__ == '__main__':
    unittest.main()
