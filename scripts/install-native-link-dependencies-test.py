#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import pathlib
import subprocess
import tempfile
import textwrap
import unittest

SCRIPT = pathlib.Path(__file__).with_name("install-native-link-dependencies.sh")


class NativeLinkDependencyInstallerTests(unittest.TestCase):
    def run_installer(
        self,
        apt_etc_dir: pathlib.Path,
        *,
        fail_command: str | None = None,
        packages: list[str] | None = None,
    ) -> tuple[subprocess.CompletedProcess[str], list[list[str]], list[str]]:
        with tempfile.TemporaryDirectory() as directory:
            tempdir = pathlib.Path(directory)
            bin_dir = tempdir / "bin"
            bin_dir.mkdir()
            command_log = tempdir / "sudo-calls.jsonl"
            source_log = tempdir / "sources.txt"
            fake_sudo = bin_dir / "sudo"
            fake_sudo.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import json
                    import os
                    import pathlib
                    import shutil
                    import sys

                    args = sys.argv[1:]
                    with open(os.environ["COVEN_FAKE_SUDO_LOG"], "a", encoding="utf-8") as handle:
                        handle.write(json.dumps(args) + "\\n")

                    if args[:3] == ["rm", "-rf", "--"]:
                        cleanup_root = pathlib.Path(args[3])
                        assert cleanup_root.parent == pathlib.Path(os.environ["TMPDIR"])
                        shutil.rmtree(cleanup_root)
                        sys.exit(0)

                    for arg in args:
                        if arg.startswith("Dir::Etc::sourceparts="):
                            sourceparts = pathlib.Path(arg.split("=", 1)[1])
                            assert sourceparts.parent.stat().st_mode & 0o005 == 0o005, "apt sandbox cannot traverse metadata directory"
                            with open(os.environ["COVEN_FAKE_SOURCE_LOG"], "a", encoding="utf-8") as handle:
                                for path in sorted(sourceparts.iterdir()):
                                    handle.write(f"--- {path.name} ---\\n")
                                    handle.write(path.read_text(encoding="utf-8"))
                                    handle.write("\\n")

                    fail_command = os.environ.get("COVEN_FAKE_FAIL_COMMAND")
                    if fail_command and fail_command in args:
                        sys.exit(42 if fail_command == "update" else 43)
                    sys.exit(0)
                    """
                ),
                encoding="utf-8",
            )
            fake_sudo.chmod(0o755)
            env = os.environ.copy()
            env.update(
                {
                    "COVEN_APT_ETC_DIR": str(apt_etc_dir),
                    "COVEN_FAKE_SUDO_LOG": str(command_log),
                    "COVEN_FAKE_SOURCE_LOG": str(source_log),
                    "PATH": f"{bin_dir}{os.pathsep}{env['PATH']}",
                    "TMPDIR": str(tempdir),
                }
            )
            if fail_command is not None:
                env["COVEN_FAKE_FAIL_COMMAND"] = fail_command
            completed = subprocess.run(
                ["bash", str(SCRIPT), *(packages or [])],
                check=False,
                capture_output=True,
                text=True,
                env=env,
            )
            calls = (
                [
                    json.loads(line)
                    for line in command_log.read_text(encoding="utf-8").splitlines()
                ]
                if command_log.exists()
                else []
            )
            sources = source_log.read_text(encoding="utf-8").splitlines() if source_log.exists() else []
            apt_calls = [call for call in calls if call[0] == "apt-get"]
            cleanup_calls = [call for call in calls if call[0] == "rm"]
            if apt_calls:
                source_arg = next(
                    arg for arg in apt_calls[0] if arg.startswith("Dir::Etc::sourceparts=")
                )
                workdir = pathlib.Path(source_arg.split("=", 1)[1]).parent
                self.assertEqual(cleanup_calls, [["rm", "-rf", "--", str(workdir)]])
                self.assertFalse(workdir.exists(), "privileged apt metadata was not cleaned up")
            else:
                self.assertEqual(cleanup_calls, [])
            return completed, apt_calls, sources

    def write_deb822_ubuntu_source(self, apt_etc_dir: pathlib.Path) -> None:
        source_dir = apt_etc_dir / "sources.list.d"
        source_dir.mkdir(parents=True)
        (source_dir / "ubuntu.sources").write_text(
            textwrap.dedent(
                """\
                Types: deb
                URIs: http://azure.archive.ubuntu.com/ubuntu/
                Suites: noble noble-updates noble-backports
                Components: main restricted universe multiverse
                Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg

                Types: deb
                URIs: http://security.ubuntu.com/ubuntu/
                Suites: noble-security
                Components: main restricted universe multiverse
                Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
                """
            ),
            encoding="utf-8",
        )

    def test_update_and_install_are_scoped_to_ubuntu_sources_and_temp_lists(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(len(calls), 2)
        self.assertEqual(calls[0][0], "apt-get")
        self.assertEqual(calls[1][0], "apt-get")
        self.assertIn("update", calls[0])
        self.assertIn("install", calls[1])
        self.assertIn("-y", calls[1])
        self.assertIn("--no-install-recommends", calls[1])
        self.assertEqual(calls[1][-2:], ["--", "libopenblas-dev"])
        self.assertIn("libopenblas-dev", calls[1])
        for call in calls:
            joined = "\n".join(call)
            self.assertIn("Dir::Etc::sourcelist=/dev/null", call)
            self.assertIn("APT::Get::List-Cleanup=0", call)
            self.assertRegex(joined, r"Dir::Etc::sourceparts=.*sources\.list\.d")
            self.assertRegex(joined, r"Dir::State::lists=.*/lists")
            self.assertNotIn("/etc/apt/sources.list.d", joined)
            self.assertNotIn("AllowUnauthenticated", joined)
            self.assertNotIn("AllowInsecureRepositories", joined)
        self.assertTrue(any("ubuntu.sources" in line for line in sources))
        self.assertTrue(any("Signed-By:" in line for line in sources))

    def test_deb822_mirror_file_source_used_by_hosted_ubuntu_runners_is_supported(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            mirror_file = apt_etc_dir / "apt-mirrors.txt"
            mirror_file.write_text(
                "http://azure.archive.ubuntu.com/ubuntu/\n"
                "http://security.ubuntu.com/ubuntu/\n",
                encoding="utf-8",
            )
            source_dir = apt_etc_dir / "sources.list.d"
            source_dir.mkdir(parents=True)
            (source_dir / "ubuntu.sources").write_text(
                textwrap.dedent(
                    f"""\
                    Types: deb
                    URIs: mirror+file:{mirror_file}
                    Suites: noble noble-updates noble-backports noble-security
                    Components: main restricted universe multiverse
                    Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg
                    """
                ),
                encoding="utf-8",
            )

            completed, calls, sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(len(calls), 2)
        source_text = "\n".join(sources)
        self.assertIn("mirror+file:", source_text)
        self.assertIn("Signed-By:", source_text)

    def test_legacy_sources_list_filters_out_unrelated_repositories(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            (apt_etc_dir / "sources.list").write_text(
                textwrap.dedent(
                    """\
                    deb http://archive.ubuntu.com/ubuntu noble main universe
                    deb http://security.ubuntu.com/ubuntu noble-security main universe
                    deb [arch=amd64] https://dl.google.com/linux/chrome/deb/ stable main
                    """
                ),
                encoding="utf-8",
            )

            completed, calls, sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(len(calls), 2)
        source_text = "\n".join(sources)
        self.assertIn("archive.ubuntu.com/ubuntu", source_text)
        self.assertIn("security.ubuntu.com/ubuntu", source_text)
        self.assertNotIn("dl.google.com", source_text)

    def test_update_failure_propagates_without_running_install(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, _sources = self.run_installer(apt_etc_dir, fail_command="update")

        self.assertEqual(completed.returncode, 42)
        self.assertEqual(len(calls), 1)
        self.assertIn("update", calls[0])

    def test_install_failure_propagates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, _sources = self.run_installer(apt_etc_dir, fail_command="install")

        self.assertEqual(completed.returncode, 43)
        self.assertEqual(len(calls), 2)
        self.assertIn("install", calls[1])

    def test_missing_ubuntu_distribution_source_fails_before_sudo(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            completed, calls, _sources = self.run_installer(pathlib.Path(directory))

        self.assertEqual(completed.returncode, 1)
        self.assertEqual(calls, [])
        self.assertIn("Unable to find the Ubuntu apt distribution source", completed.stderr)

    def test_deb822_source_without_signed_by_fails_before_sudo(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            source_dir = apt_etc_dir / "sources.list.d"
            source_dir.mkdir(parents=True)
            (source_dir / "ubuntu.sources").write_text(
                textwrap.dedent(
                    """\
                    Types: deb
                    URIs: http://archive.ubuntu.com/ubuntu/
                    Suites: noble
                    Components: main universe
                    """
                ),
                encoding="utf-8",
            )

            completed, calls, _sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 1)
        self.assertEqual(calls, [])
        self.assertIn("does not declare Signed-By key material", completed.stderr)


if __name__ == "__main__":
    raise SystemExit(unittest.main())
