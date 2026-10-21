"""Host runner contracts; no container execution is implied by these tests."""
import importlib.util
import json
import os
import subprocess
import sys
from unittest import mock
from pathlib import Path
import tempfile
import unittest


class RunnerTests(unittest.TestCase):
    def setUp(self):
        source = Path(__file__).with_name('runner.py')
        self.assertTrue(source.exists(), 'container contract runner is not implemented')
        spec = importlib.util.spec_from_file_location('contract_runner', source)
        self.runner = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.runner)

    def test_only_digest_pinned_official_images_are_accepted(self):
        inventory = self.runner.load_images()
        self.assertEqual(set(inventory), {'debian12', 'debian13', 'ubuntu24'})
        self.assertEqual(inventory['debian12']['contract'], 'native-and-python-rejection')
        self.assertEqual(inventory['debian13']['contract'], 'full-bootstrap')
        self.assertEqual(inventory['ubuntu24']['contract'], 'full-bootstrap')
        for entry in inventory.values():
            self.assertRegex(entry['image'], r'^docker.io/library/(debian|ubuntu)@sha256:[0-9a-f]{64}$')
            self.assertEqual(entry['platform'], 'linux/amd64')

    def test_invalid_inventory_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / 'images.json'
            target.write_text(json.dumps({'debian12': {'image': 'debian:latest', 'platform': 'linux/amd64'}}))
            with self.assertRaisesRegex(ValueError, 'inventory'):
                self.runner.load_images(target)
            inventory = self.runner.load_images()
            inventory['debian13']['contract'] = 'native-and-python-rejection'
            target.write_text(json.dumps(inventory))
            with self.assertRaisesRegex(ValueError, 'inventory'):
                self.runner.load_images(target)

    def test_container_command_has_no_privileged_or_host_network_access(self):
        image = self.runner.load_images()['debian12']
        command = self.runner.container_command(image, Path('/tmp/contract-source'), Path('/tmp/contract-output'), 'a' * 40, 'contract-test')
        self.assertEqual(command[:3], ['docker', 'run', '--rm'])
        self.assertNotIn('--privileged', command)
        self.assertNotIn('--network=host', command)
        self.assertIn('no-new-privileges', command)
        self.assertIn('type=bind,source=/tmp/contract-source,target=/source,readonly', command)
        self.assertIn(image['image'], command)
        self.assertFalse(any('docker.sock' in part for part in command))

    def test_container_uses_init_to_reap_orphaned_smoke_descendants(self):
        for image in self.runner.load_images().values():
            command = self.runner.container_command(image, Path('/tmp/source'), Path('/tmp/output'), 'a' * 40, 'contract-test')
            # Python's contract runner is not a PID 1 orphan reaper. The daemon
            # smoke tests require killed orphan descendants to disappear.
            self.assertIn('--init', command[:command.index(image['image'])])

    def test_report_path_cannot_overlap_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / 'repo'
            repo.mkdir()
            for output in (repo, repo / 'output', repo.parent):
                with self.assertRaisesRegex(ValueError, 'overlap'):
                    self.runner.validate_output(repo, output)


    def test_git_queries_ignore_ambient_worktree_and_gitdir(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source, foreign = root / 'source', root / 'foreign'
            source.mkdir()
            foreign.mkdir()
            env = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
            subprocess.run(['git', 'init', '-q', str(source)], env=env, check=True)
            subprocess.run(['git', 'init', '-q', str(foreign)], env=env, check=True)
            with mock.patch.dict(os.environ, {'GIT_DIR': str(foreign / '.git'), 'GIT_WORK_TREE': str(foreign)}):
                self.assertEqual(Path(self.runner.git(source, 'rev-parse', '--show-toplevel')).resolve(), source.resolve())

    def test_cleanup_requires_positive_absence_after_failed_removal(self):
        complete = subprocess.CompletedProcess
        with mock.patch.object(self.runner.subprocess, 'run', side_effect=[complete([], 1), complete([], 1, stdout='')]):
            self.assertFalse(self.runner.remove_container('owned-test'))
        with mock.patch.object(self.runner.subprocess, 'run', side_effect=[complete([], 1), complete([], 0, stdout='still-present')]):
            self.assertFalse(self.runner.remove_container('owned-test'))
        with mock.patch.object(self.runner.subprocess, 'run', side_effect=[complete([], 1), complete([], 0, stdout='')]):
            self.assertTrue(self.runner.remove_container('owned-test'))
        with mock.patch.object(self.runner.subprocess, 'run', return_value=complete([], 0)):
            self.assertTrue(self.runner.remove_container('owned-test'))


    def test_execute_verifies_provenance_timeout_and_cleanup(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / 'repo'
            source.mkdir()
            self.runner.git(source, 'init', '-q')
            self.runner.git(source, 'config', 'user.name', 'Contract Fixture')
            self.runner.git(source, 'config', 'user.email', '0+fixture@users.noreply.github.com')
            for file in ('scripts/cloud/bootstrap.sh', 'scripts/cloud/readiness.sh',
                         'scripts/bootstrap-contract/container.sh', 'scripts/bootstrap-contract/inside.py'):
                target = source / file
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text('fixture only\n')
            self.runner.git(source, 'add', '.')
            self.runner.git(source, 'commit', '-qm', 'fixture')
            sha = self.runner.git(source, 'rev-parse', 'HEAD')
            original_run = subprocess.run
            for case, expected in [('pass', 'contract-passed'), ('provenance', 'container-or-source-error'),
                                   ('profile', 'container-or-source-error'),
                                   ('timeout', 'container-timeout'), ('cleanup', 'container-cleanup-failed')]:
                output = root / case
                def run(command, **kwargs):
                    if command[0] != 'docker':
                        return original_run(command, **kwargs)
                    if command[1] == 'run':
                        if case == 'timeout':
                            raise subprocess.TimeoutExpired(command, 5400)
                        (output / 'result.json').write_text(json.dumps({'status': 'passed', 'distribution': 'debian',
                                                                    'contract': 'wrong' if case == 'profile' else 'native-and-python-rejection',
                                                                    'source_sha': 'wrong' if case == 'provenance' else sha}))
                        return subprocess.CompletedProcess(command, 0)
                    return subprocess.CompletedProcess(command, 1 if case == 'cleanup' else 0, stdout='')
                foreign = root / 'outside'
                foreign.mkdir(exist_ok=True)
                with mock.patch.object(self.runner.shutil, 'which', return_value='/mock/docker'), \
                        mock.patch.object(self.runner.subprocess, 'run', side_effect=run), \
                        mock.patch.dict(os.environ, {'GIT_WORK_TREE': str(foreign), 'GIT_DIR': str(source / '.git')}):
                    result = self.runner.execute(source, self.runner.load_images()['debian12'], output)
                self.assertEqual(result['code'], expected)
                self.assertEqual(result['status'], 'passed' if case == 'pass' else 'failed')
                self.assertTrue((output / 'runner.json').is_file())
                self.assertEqual(list(foreign.iterdir()), [])


    def test_failed_bootstrap_json_is_retained_before_failure(self):
        spec = importlib.util.spec_from_file_location('inside_contract', Path(__file__).with_name('inside.py'))
        inside = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(inside)
        with tempfile.TemporaryDirectory() as tmp:
            output = Path(tmp)
            body = {'development_ready': False, 'blocked_stage': 'node_download'}
            result = subprocess.CompletedProcess([], 1, stdout=json.dumps(body), stderr='private diagnostic')
            report = {'cases': []}
            with mock.patch.object(inside.subprocess, 'run', return_value=result):
                with self.assertRaisesRegex(RuntimeError, 'case-failed'):
                    inside.run_case('bootstrap-first', ['mock-bootstrap'], report=report, output=output,
                                    diagnostic=lambda _: None, report_file='bootstrap-first.json')
            self.assertEqual(json.loads((output / 'bootstrap-first.json').read_text()), body)
            self.assertEqual(report['cases'][0]['status'], 'failed')
            self.assertNotIn('private diagnostic', json.dumps(report))

    def test_python_capability_cannot_turn_missing_security_into_full_success(self):
        spec = importlib.util.spec_from_file_location('inside_contract', Path(__file__).with_name('inside.py'))
        inside = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(inside)
        class RejectedArchive(ValueError):
            pass
        extractor = mock.Mock(side_effect=RejectedArchive('Python needs the maintained tarfile extraction security filters'))
        provision = mock.Mock(ProvisionError=RejectedArchive, extract_node=extractor)
        result = inside.python_contract('native-and-python-rejection', False, provision)
        self.assertFalse(result['bootstrap_supported'])
        self.assertEqual(result['code'], 'python-security-filters-rejected')
        extractor.assert_called_once()
        for profile, available in [('full-bootstrap', False), ('native-and-python-rejection', True), ('unknown', True)]:
            with self.subTest(profile=profile, available=available), self.assertRaises(RuntimeError):
                inside.python_contract(profile, available, provision)
        self.assertTrue(inside.python_contract('full-bootstrap', True, provision)['bootstrap_supported'])
        extractor.side_effect = None
        with self.assertRaises(RuntimeError):
            inside.python_contract('native-and-python-rejection', False, provision)

    def test_native_rejection_report_never_runs_bootstrap_or_claims_readiness(self):
        spec = importlib.util.spec_from_file_location('inside_contract', Path(__file__).with_name('inside.py'))
        inside = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(inside)
        class RejectedArchive(ValueError):
            pass
        provision = mock.Mock(ProvisionError=RejectedArchive,
                              extract_node=mock.Mock(side_effect=RejectedArchive(
                                  'Python needs the maintained tarfile extraction security filters')))
        environment = {'COVEN_CONTRACT_PROFILE': 'native-and-python-rejection',
                       'COVEN_CONTRACT_SOURCE_SHA': 'a' * 40, 'COVEN_CONTRACT_DISTRO': 'debian',
                       'COVEN_CONTRACT_CODENAME': 'bookworm'}
        os_release = mock.Mock()
        os_release.read_text.return_value = 'ID=debian\nVERSION_CODENAME=bookworm\n'
        def path(value):
            return os_release if value == '/etc/os-release' else Path(value)
        def git(command, **kwargs):
            return 'a' * 40 if command[1] == 'rev-parse' else b''
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.dict(os.environ, environment), \
                mock.patch.dict(sys.modules, {'provision': provision, 'runtime': mock.Mock()}), \
                mock.patch.object(inside, 'Path', side_effect=path), \
                mock.patch.object(inside, 'tarfile', mock.Mock(spec=[])), \
                mock.patch.object(inside, 'source_hashes', return_value={}), \
                mock.patch.object(inside.subprocess, 'check_output', side_effect=git), \
                mock.patch.object(inside, 'run_case', return_value='') as run:
            report = inside.execute(Path(directory))
        self.assertEqual(report['status'], 'passed')
        self.assertFalse(report['development_ready'])
        self.assertFalse(report['python']['bootstrap_supported'])
        self.assertEqual({item['name'] for item in report['skipped']},
                         {'mock-suites', 'bootstrap-first', 'bootstrap-second', 'fresh-readiness'})
        self.assertEqual([call.args[0] for call in run.call_args_list],
                         ['native-root-first', 'native-root-second', 'native-nonroot',
                          'unsupported-os-fixture', 'foreign-source-fixture', 'malformed-source-fixture'])


if __name__ == '__main__':
    unittest.main()
