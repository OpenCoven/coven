"""Host runner contracts; no container execution is implied by these tests."""
import importlib.util
import json
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
        self.assertEqual(set(inventory), {'debian12', 'ubuntu24'})
        for entry in inventory.values():
            self.assertRegex(entry['image'], r'^docker.io/library/(debian|ubuntu)@sha256:[0-9a-f]{64}$')
            self.assertEqual(entry['platform'], 'linux/amd64')

    def test_invalid_inventory_fails_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / 'images.json'
            target.write_text(json.dumps({'debian12': {'image': 'debian:latest', 'platform': 'linux/amd64'}}))
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

    def test_report_path_cannot_overlap_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp) / 'repo'
            repo.mkdir()
            for output in (repo, repo / 'output', repo.parent):
                with self.assertRaisesRegex(ValueError, 'overlap'):
                    self.runner.validate_output(repo, output)


if __name__ == '__main__':
    unittest.main()
