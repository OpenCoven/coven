#!/usr/bin/env python3
"""Offline Unix CLI journeys. Run with --binary /path/to/coven."""
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

BINARY = Path(sys.argv[sys.argv.index('--binary') + 1]).resolve()
del sys.argv[sys.argv.index('--binary'):sys.argv.index('--binary') + 2]


@unittest.skipIf(os.name == 'nt', 'Unix executable fixtures; Windows requires native qualification')
class EngineUpdate(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='coven-engine-update-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.engine_root = self.root / '.coven/engine'
        self.engine_root.mkdir(parents=True)
        self.install('0.8.0')
        (self.engine_root / 'current').write_text('0.8.0')
        self.original = (self.engine_root / '0.8.0/coven-code').read_bytes()
        # Match the host without assuming Apple Silicon.
        import platform
        self.artifact = f"coven-code-{'macos' if sys.platform == 'darwin' else 'linux'}-{'aarch64' if platform.machine() in ('arm64', 'aarch64') else 'x86_64'}.tar.gz"
        # Synthetic future release stays distinct from the real bundled pin.
        self.release('999.0.0')
        (self.root / 'latest.json').write_text(json.dumps({'tag_name': 'v999.0.0', 'draft': False, 'prerelease': False}))
        curl = self.bin / 'curl'
        curl.write_text(f'''#!{sys.executable}
import json, os, pathlib, shutil, sys
r = pathlib.Path(os.environ['COVEN_UPDATE_FIXTURES'])
a = sys.argv[1:]
with (r / 'requests').open('a') as f: f.write(json.dumps(a) + '\\n')
assert a[0] == '--disable', a
assert a[a.index('--proto') + 1] == '=https', a
assert a[a.index('--proto-redir') + 1] == '=https', a
assert '--max-time' in a and '--connect-timeout' in a, a
assert '--max-filesize' in a, a
assert a[a.index('--retry') + 1] == '3', a
assert '--speed-limit' in a and '--speed-time' in a, a
url = a[-1]
assert url.startswith('https://'), url
if (r / 'fail-download').exists(): sys.exit(22)
src = r / ('latest.json' if url.endswith('/releases/latest') else url.rsplit('/', 1)[-1])
shutil.copyfile(src, a[a.index('--output') + 1])
''')
        curl.chmod(0o755)
        self.env = dict(os.environ)
        self.env.pop('COVEN_ENGINE_BIN', None)
        self.env.update(HOME=str(self.root), USERPROFILE=str(self.root), COVEN_HOME=str(self.root / 'state'),
                        XDG_CONFIG_HOME=str(self.root / 'config'), COVEN_UPDATE_FIXTURES=str(self.root),
                        PATH=str(self.bin) + os.pathsep + os.environ['PATH'])

    def install(self, version, reported=None):
        p = self.engine_root / version / 'coven-code'
        p.parent.mkdir(exist_ok=True)
        p.write_text(f'#!/bin/sh\nif [ "$1" = "--version" ]; then echo "coven-code {reported or version}"; else echo "delegated:$*"; fi\n')
        p.chmod(0o755)
        return p

    def release(self, version, reported=None, corrupt=False):
        data = f'#!/bin/sh\necho "coven-code {reported or version}"\n'.encode()
        with tarfile.open(self.root / self.artifact, 'w:gz') as archive:
            entry = tarfile.TarInfo('coven-code')
            entry.size, entry.mode = len(data), 0o755
            archive.addfile(entry, io.BytesIO(data))
        digest = hashlib.sha256((self.root / self.artifact).read_bytes()).hexdigest()
        self.digest = digest
        (self.root / 'SHA256SUMS').write_text(f"{'0' * 64 if corrupt else digest}  {self.artifact}\n")

    def run_cli(self, *args, ok=True):
        result = subprocess.run([str(BINARY), *args], env=self.env, cwd=self.root,
                                capture_output=True, text=True, timeout=30)
        self.output = result.stdout + result.stderr
        self.assertEqual(result.returncode == 0, ok, self.output)
        return self.output

    def assert_old_active(self):
        self.assertEqual((self.engine_root / 'current').read_text().strip(), '0.8.0')
        self.assertEqual((self.engine_root / '0.8.0/coven-code').read_bytes(), self.original)

    def test_update_activates_latest_and_preserves_old_binary(self):
        output = self.run_cli('engine', 'update')
        self.assertIn('999.0.0', output)
        self.assertIn(str(self.engine_root / '999.0.0/coven-code'), output)
        self.assertEqual((self.engine_root / 'current').read_text().strip(), '999.0.0')
        self.assertEqual((self.engine_root / '0.8.0/coven-code').read_bytes(), self.original)
        self.assertIn('coven-code 999.0.0', self.run_cli('code', '--version'))
        status = json.loads(self.run_cli('engine', 'status', '--json'))
        self.assertEqual(status['version'], '999.0.0')
        self.assertEqual(status['path'], str(self.engine_root / '999.0.0/coven-code'))
        self.run_cli('engine', 'update')
        self.assertIn('already present', self.output.lower())

    def test_code_upgrade_routes_through_manager(self):
        self.run_cli('code', 'upgrade', '--version', 'v999.0.0')
        self.assertEqual((self.engine_root / 'current').read_text().strip(), '999.0.0')
        self.assertEqual((self.engine_root / '0.8.0/coven-code').read_bytes(), self.original)
        self.assertNotIn('delegated:', self.output)

    def test_code_update_alias_works_without_an_engine(self):
        (self.engine_root / 'current').unlink()
        self.run_cli('code', 'update', '--version', '999.0.0')
        self.assertIn('coven-code 999.0.0', self.run_cli('code', '--version'))

    def test_bad_checksum_preserves_previous_engine(self):
        self.release('999.0.0', corrupt=True)
        self.assertIn('checksum mismatch', self.run_cli('engine', 'update', ok=False))
        self.assert_old_active()
        self.assertFalse((self.engine_root / '999.0.0/coven-code').exists())

    def test_wrong_candidate_version_preserves_previous_engine(self):
        self.release('999.0.0', reported='0.7.0')
        self.assertIn('version', self.run_cli('engine', 'update', ok=False))
        self.assert_old_active()

    def test_duplicate_manifest_entry_is_rejected(self):
        p = self.root / 'SHA256SUMS'
        p.write_text(p.read_text() * 2)
        self.assertIn('duplicate', self.run_cli('engine', 'update', ok=False).lower())
        self.assert_old_active()

    def test_network_failure_preserves_previous_engine(self):
        (self.root / 'fail-download').touch()
        self.run_cli('engine', 'update', ok=False)
        self.assert_old_active()

    def test_missing_manifest_entry_is_rejected(self):
        (self.root / 'SHA256SUMS').write_text(f"{'a' * 64}  another-archive.tar.gz\n")
        self.assertIn('no checksum', self.run_cli('engine', 'update', ok=False))
        self.assert_old_active()

    def test_install_lock_prevents_competing_activation(self):
        import fcntl
        with (self.engine_root / 'install.lock').open('w') as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertIn('another engine install', self.run_cli('engine', 'update', ok=False))
        self.assert_old_active()
        self.run_cli('engine', 'update')
        self.assertEqual((self.engine_root / 'current').read_text(), '999.0.0')

    def test_force_rechecks_archive_and_preserves_active_candidate_on_failure(self):
        self.install('999.0.0')
        (self.engine_root / 'current').write_text('999.0.0')
        original = (self.engine_root / '999.0.0/coven-code').read_bytes()
        self.release('999.0.0', corrupt=True)
        self.assertIn('checksum mismatch', self.run_cli('engine', 'update', '--force', ok=False))
        self.assertEqual((self.engine_root / 'current').read_text(), '999.0.0')
        self.assertEqual((self.engine_root / '999.0.0/coven-code').read_bytes(), original)

    def test_override_is_explicit_and_no_download_occurs(self):
        self.env['COVEN_ENGINE_BIN'] = str(self.install('custom', '0.8.0'))
        self.assertIn('COVEN_ENGINE_BIN', self.run_cli('code', 'update', ok=False))
        self.assertFalse((self.root / 'requests').exists())
        self.assert_old_active()

    def test_invalid_version_cannot_escape_managed_root(self):
        for value in ['../escape', '999.0.0/../../escape', '0.9', '999.0.0?other', '0999.0.0']:
            with self.subTest(value=value):
                self.run_cli('engine', 'update', '--version', value, ok=False)
                self.assertFalse((self.root / 'requests').exists())
                self.assert_old_active()

    def test_corrupt_existing_candidate_is_repaired(self):
        self.install('999.0.0', '0.8.0')
        self.run_cli('engine', 'update', '--version', '999.0.0')
        self.assertIn('coven-code 999.0.0', self.run_cli('code', '--version'))

    def test_latest_does_not_silently_downgrade(self):
        self.install('999.1.0')
        (self.engine_root / 'current').write_text('999.1.0')
        self.assertIn('downgrade', self.run_cli('engine', 'update', ok=False).lower())
        self.assertEqual((self.engine_root / 'current').read_text(), '999.1.0')

    def test_latest_rechecks_selection_under_install_lock(self):
        self.install('999.1.0')
        # A concurrent updater activates 999.1.0 just after this run's
        # pre-check probed the selected 0.8.0 engine, before the lock.
        old = self.engine_root / '0.8.0/coven-code'
        old.write_text('#!/bin/sh\n'
                       f'printf 999.1.0 > "{self.engine_root}/current"\n'
                       'echo "coven-code 0.8.0"\n')
        self.assertIn('refusing to downgrade', self.run_cli('engine', 'update', ok=False).lower())
        self.assertEqual((self.engine_root / 'current').read_text(), '999.1.0')
        self.assertFalse((self.engine_root / '999.0.0').exists())

    def test_rollback_to_installed_version_needs_no_manifest(self):
        self.run_cli('engine', 'update')
        (self.root / 'requests').unlink()
        (self.root / 'SHA256SUMS').unlink()
        self.assertIn('already present', self.run_cli('code', 'update', '--version', '0.8.0').lower())
        self.assertFalse((self.root / 'requests').exists())
        self.assert_old_active()
        self.assertIn('coven-code 0.8.0', self.run_cli('code', '--version'))

    def test_missing_manifest_error_names_the_explicit_install(self):
        (self.root / 'SHA256SUMS').unlink()
        output = self.run_cli('engine', 'update', '--version', '999.0.0', ok=False)
        self.assertIn('SHA256SUMS manifest for Coven Code 999.0.0', output)
        self.assertIn('coven engine install --version 999.0.0 --sha256', output)
        self.assert_old_active()
        self.assertFalse((self.engine_root / '999.0.0').exists())

    def test_force_replaces_an_installed_candidate_after_verification(self):
        self.install('999.0.0')
        (self.engine_root / 'current').write_text('999.0.0')
        self.assertIn('Updated Coven Code to 999.0.0', self.run_cli('engine', 'update', '--force'))
        replaced = (self.engine_root / '999.0.0/coven-code').read_text()
        self.assertNotIn('delegated:', replaced)
        self.assertEqual((self.engine_root / 'current').read_text(), '999.0.0')
        self.assertIn('Downloading Coven Code 999.0.0', self.output)

    def test_unusable_lock_file_is_not_reported_as_contention(self):
        lock = self.engine_root / 'install.lock'
        lock.mkdir()
        output = self.run_cli('engine', 'update', ok=False)
        self.assertNotIn('another engine install', output)
        self.assertIn('install.lock', output)
        self.assert_old_active()

    def test_explicit_install_still_requires_a_checksum(self):
        self.assertIn('--sha256', self.run_cli('engine', 'install', '--version', '999.0.0', ok=False))
        self.assert_old_active()

    def test_explicit_install_uses_same_verified_activation_path(self):
        self.run_cli('engine', 'install', '--version', 'v999.0.0', '--sha256', self.digest)
        self.assertIn('coven-code 999.0.0', self.run_cli('code', '--version'))

    def test_bad_checksum_syntax_is_rejected_before_download(self):
        self.assertIn('invalid sha256', self.run_cli('engine', 'install', '--version', '999.0.0', '--sha256', 'bad', ok=False))
        self.assertFalse((self.root / 'requests').exists())
        self.assert_old_active()

    def test_help_and_unknown_options_do_not_launch_engine(self):
        self.assertIn('update', self.run_cli('code', 'update', '--help'))
        self.assertNotIn('delegated:', self.output)
        self.run_cli('code', 'upgrade', '--typo', ok=False)
        self.assertFalse((self.root / 'requests').exists())
        self.assert_old_active()


if __name__ == '__main__':
    unittest.main()
