#!/usr/bin/env python3
from __future__ import annotations

import json
import os
import pathlib
import shutil
import subprocess
import tempfile
import textwrap
import time
import unittest

SCRIPT = pathlib.Path(__file__).with_name("install-native-link-dependencies.sh")


class NativeLinkDependencyInstallerTests(unittest.TestCase):
    def run_installer(
        self,
        apt_etc_dir: pathlib.Path,
        *,
        distro: str = "ubuntu",
        uid: int = 1000,
        sudo_ok: bool = True,
        os_release_text: str | None = None,
        fail_command: str | None = None,
        packages: list[str] | None = None,
        mktemp_default_dir: pathlib.Path | None = None,
        slow_downloads: int = 0,
        print_uris: str = "",
        extra_env: dict[str, str] | None = None,
        mirror_log: list[list[str]] | None = None,
        temp_prefix: str = "coven installer ",
    ) -> tuple[subprocess.CompletedProcess[str], list[list[str]], list[str]]:
        with tempfile.TemporaryDirectory(prefix=temp_prefix) as directory:
            tempdir = pathlib.Path(directory)
            bin_dir = tempdir / "bin"
            bin_dir.mkdir()
            os_release = tempdir / "os-release"
            codename = "bookworm" if distro == "debian" else "noble"
            os_release.write_text(f'ID={distro}\nVERSION_CODENAME={codename}\n', encoding="utf-8")
            if os_release_text is not None:
                os_release.write_text(os_release_text, encoding="utf-8")
            fake_id = bin_dir / "id"
            fake_id.write_text(f"#!/bin/sh\necho {uid}\n", encoding="utf-8")
            fake_id.chmod(0o755)
            command_log = tempdir / "sudo-calls.jsonl"
            mirror_log_path = tempdir / "mirrors.jsonl"
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
                    import time

                    args = sys.argv[1:]
                    if pathlib.Path(sys.argv[0]).name == "apt-get":
                        args = ["apt-get", *args]
                    if args == ["-n", "true"]:
                        sys.exit(0 if os.environ["COVEN_FAKE_SUDO_OK"] == "1" else 1)
                    if args[:1] == ["-n"]:
                        args = args[1:]
                    if args[:1] == ["timeout"]:
                        os.execvp("timeout", args)
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
                    if "--print-uris" in args:
                        print(os.environ.get("COVEN_FAKE_PRINT_URIS", ""), end="")
                    if "--download-only" in args:
                        sourceparts = next(
                            pathlib.Path(arg.split("=", 1)[1]) for arg in args
                            if arg.startswith("Dir::Etc::sourceparts=")
                        )
                        mirrors = sourceparts.parent / "mirrors"
                        with open(os.environ["COVEN_FAKE_MIRROR_LOG"], "a", encoding="utf-8") as handle:
                            handle.write(json.dumps([
                                path.read_text(encoding="utf-8") for path in sorted(mirrors.iterdir())
                            ]) + "\\n")
                        counter = pathlib.Path(os.environ["COVEN_FAKE_MIRROR_LOG"] + ".downloads")
                        done = int(counter.read_text()) if counter.exists() else 0
                        counter.write_text(str(done + 1))
                        if done < int(os.environ.get("COVEN_FAKE_SLOW_DOWNLOADS", "0")):
                            # Model the ~19 kB/s crawl in #1578: apt keeps going
                            # far past any reasonable deadline and never fails.
                            time.sleep(600)
                    sys.exit(0)
                    """
                ),
                encoding="utf-8",
            )
            fake_sudo.chmod(0o755)
            (bin_dir / "apt-get").symlink_to(fake_sudo)
            # A coreutils-compatible timeout(1), so the watchdog also runs where
            # the host has none (macOS). It signals the whole process group.
            fake_timeout = bin_dir / "timeout"
            fake_timeout.write_text(
                textwrap.dedent(
                    """\
                    #!/usr/bin/env python3
                    import os
                    import signal
                    import subprocess
                    import sys

                    args = sys.argv[1:]
                    kill_after = None
                    if args[0].startswith("--kill-after="):
                        kill_after = float(args.pop(0).split("=", 1)[1])
                    duration = float(args.pop(0))
                    child = subprocess.Popen(args, start_new_session=True)
                    try:
                        sys.exit(child.wait(timeout=duration))
                    except subprocess.TimeoutExpired:
                        os.killpg(child.pid, signal.SIGTERM)
                    try:
                        child.wait(timeout=kill_after)
                    except subprocess.TimeoutExpired:
                        os.killpg(child.pid, signal.SIGKILL)
                        child.wait()
                        sys.exit(137)
                    sys.exit(124)
                    """
                ),
                encoding="utf-8",
            )
            fake_timeout.chmod(0o755)
            if mktemp_default_dir is not None:
                # Model platforms whose default temporary root takes precedence
                # over TMPDIR. Every generated directory still belongs to this test.
                real_mktemp = shutil.which("mktemp")
                self.assertIsNotNone(real_mktemp)
                fake_mktemp = bin_dir / "mktemp"
                fake_mktemp.write_text(
                    textwrap.dedent(
                        """\
                        #!/usr/bin/env python3
                        import os
                        import sys
                        import tempfile

                        if sys.argv[1:] == ["-d"]:
                            print(tempfile.mkdtemp(dir=os.environ["COVEN_FAKE_MKTEMP_DEFAULT"]))
                        else:
                            executable = os.environ["COVEN_REAL_MKTEMP"]
                            os.execv(executable, [executable, *sys.argv[1:]])
                        """
                    ),
                    encoding="utf-8",
                )
                fake_mktemp.chmod(0o755)
            env = os.environ.copy()
            env.update(
                {
                    "COVEN_APT_ETC_DIR": str(apt_etc_dir),
                    "COVEN_OS_RELEASE_FILE": str(os_release),
                    "COVEN_FAKE_SUDO_OK": "1" if sudo_ok else "0",
                    "COVEN_FAKE_SUDO_LOG": str(command_log),
                    "COVEN_FAKE_SOURCE_LOG": str(source_log),
                    "COVEN_FAKE_MIRROR_LOG": str(mirror_log_path),
                    "COVEN_FAKE_SLOW_DOWNLOADS": str(slow_downloads),
                    "COVEN_FAKE_PRINT_URIS": print_uris,
                    "PATH": f"{bin_dir}{os.pathsep}{env['PATH']}",
                    "TMPDIR": str(tempdir),
                }
            )
            if fail_command is not None:
                env["COVEN_FAKE_FAIL_COMMAND"] = fail_command
            env.update(extra_env or {})
            if mktemp_default_dir is not None:
                env["COVEN_FAKE_MKTEMP_DEFAULT"] = str(mktemp_default_dir)
                env["COVEN_REAL_MKTEMP"] = str(real_mktemp)
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
            if mirror_log is not None and mirror_log_path.exists():
                mirror_log.extend(
                    json.loads(line)
                    for line in mirror_log_path.read_text(encoding="utf-8").splitlines()
                )
            apt_calls = [call for call in calls if call[0] == "apt-get"]
            cleanup_calls = [call for call in calls if call[0] == "rm"]
            if apt_calls:
                source_arg = next(
                    arg for arg in apt_calls[0] if arg.startswith("Dir::Etc::sourceparts=")
                )
                workdir = pathlib.Path(source_arg.split("=", 1)[1]).parent
                self.assertEqual(cleanup_calls, [["rm", "-rf", "--", str(workdir)]] if uid else [])
                self.assertFalse(workdir.exists(), "privileged apt metadata was not cleaned up")
            else:
                self.assertEqual(cleanup_calls, [])
            return completed, apt_calls, sources

    def test_debian_sources_and_root_without_sudo(self) -> None:
        for uid in (0, 1000):
            with self.subTest(uid=uid), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(root)
                source = root / "sources.list.d/ubuntu.sources"
                text = source.read_text().replace("ubuntu.com/ubuntu", "debian.org/debian")
                text = text.replace("azure.archive.debian.org", "deb.debian.org")
                text = text.replace("security.debian.org/debian", "security.debian.org/debian-security")
                text = text.replace("noble", "bookworm").replace("ubuntu-archive", "debian-archive")
                source.unlink()
                (source.parent / "debian.sources").write_text(text)
                before = (source.parent / "debian.sources").read_bytes()
                for _ in range(2):
                    completed, calls, sources = self.run_installer(root, distro="debian", uid=uid, sudo_ok=bool(uid))
                    self.assertEqual(completed.returncode, 0, completed.stderr)
                    self.assertEqual(len(calls), 4)
                    self.assertIn("debian.sources", "\n".join(sources))
                self.assertEqual((source.parent / "debian.sources").read_bytes(), before)

    def test_ubuntu_ports_archive_is_supported_in_both_source_formats(self) -> None:
        for deb822 in (False, True):
            with self.subTest(deb822=deb822), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                uri = "https://ports.ubuntu.com/ubuntu-ports/"
                if deb822:
                    self.write_deb822_ubuntu_source(root)
                    source = root / "sources.list.d/ubuntu.sources"
                    source.write_text(
                        f"Types: deb\nURIs: {uri}\nSuites: noble noble-updates noble-security\n"
                        "Components: main universe\n"
                        "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n")
                else:
                    (root / "sources.list").write_text(f"deb {uri} noble main universe\n")
                completed, calls, sources = self.run_installer(root)
                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertEqual(len(calls), 4)
                self.assertIn(uri, "\n".join(sources))

    def test_unsupported_distro_and_cross_distro_fail_before_apt(self) -> None:
        for distro in ("fedora", "debian"):
            with self.subTest(distro=distro), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(root)
                completed, calls, _ = self.run_installer(root, distro=distro)
                self.assertNotEqual(completed.returncode, 0)
                self.assertEqual(calls, [])

    def test_missing_noninteractive_privilege_fails_before_apt(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(root)
            completed, calls, _ = self.run_installer(root, sudo_ok=False)
            self.assertNotEqual(completed.returncode, 0)
            self.assertIn("privilege", completed.stderr.lower())
            self.assertEqual(calls, [])

    def test_deb822_rejects_signature_bypasses_and_malformed_records(self) -> None:
        for suffix in ("Trusted: yes", "Allow-Insecure: yes", "Allow-Weak: yes",
                       "Allow-Downgrade-To-Insecure: yes", "Check-Valid-Until: no",
                       "Signed-By: /other.gpg", "malformed"):
            with self.subTest(suffix=suffix), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(root)
                source = root / "sources.list.d/ubuntu.sources"
                source.write_text(source.read_text().rstrip() + "\n" + suffix + "\n")
                completed, calls, _ = self.run_installer(root)
                self.assertNotEqual(completed.returncode, 0, suffix)
                self.assertEqual(calls, [])

    def test_legacy_rejects_signature_bypasses_wrong_suite_and_cross_distro(self) -> None:
        for entry in (
            'deb ["trusted"=yes] http://archive.ubuntu.com/ubuntu noble main',
            'deb [tr%75sted=yes] http://archive.ubuntu.com/ubuntu noble main',
            'deb ["allow-insecure"=yes] http://archive.ubuntu.com/ubuntu noble main',
            'deb ["check-valid-until"=no] http://archive.ubuntu.com/ubuntu noble main',
            "deb [trusted=yes] http://archive.ubuntu.com/ubuntu noble main",
            "deb [allow-insecure=yes] http://archive.ubuntu.com/ubuntu noble main",
            "deb [allow-weak=yes] http://archive.ubuntu.com/ubuntu noble main",
            "deb [allow-downgrade-to-insecure=yes] http://archive.ubuntu.com/ubuntu noble main",
            "deb [check-valid-until=no] http://archive.ubuntu.com/ubuntu noble main",
            "deb http://archive.ubuntu.com/ubuntu bookworm main",
            "deb http://deb.debian.org/debian bookworm main",
            "deb http://archive.ubuntu.com/ubuntu noble",
        ):
            with self.subTest(entry=entry), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                (root / "sources.list").write_text("deb http://archive.ubuntu.com/ubuntu noble main\n" + entry + "\n")
                completed, calls, _ = self.run_installer(root)
                self.assertNotEqual(completed.returncode, 0, entry)
                self.assertEqual(calls, [])

    def test_debian_legacy_sources(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            text = ("deb http://deb.debian.org/debian bookworm main\n"
                    "deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] "
                    "https://security.debian.org/debian-security bookworm-security main\n")
            (root / "sources.list").write_text(text)
            completed, calls, sources = self.run_installer(root, distro="debian")
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(len(calls), 4)
            self.assertIn(text, "\n".join(sources))
            self.assertEqual((root / "sources.list").read_text(), text)

    def test_os_release_is_required_unambiguous_data(self) -> None:
        for text in ("ID=ubuntu\n", "ID=ubuntu\nID=debian\nVERSION_CODENAME=noble\n",
                     'ID="ubuntu\nVERSION_CODENAME=noble\n',
                     'ID="$(echo ubuntu)"\nVERSION_CODENAME=noble\n',
                     'ID=linuxmint\nID_LIKE=ubuntu\nVERSION_CODENAME=noble\n'):
            with self.subTest(text=text), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(root)
                completed, calls, _ = self.run_installer(root, os_release_text=text)
                self.assertNotEqual(completed.returncode, 0)
                self.assertEqual(calls, [])

    def test_deb822_requires_host_suite_and_components(self) -> None:
        for old, new in (("Suites: noble", "Suites: bookworm"),
                         ("Components: main restricted universe multiverse", "Components:"),
                         ("Signed-By:", "Signed-By :")):
            with self.subTest(new=new), tempfile.TemporaryDirectory() as directory:
                root = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(root)
                source = root / "sources.list.d/ubuntu.sources"
                source.write_text(source.read_text().replace(old, new))
                completed, calls, _ = self.run_installer(root)
                self.assertNotEqual(completed.returncode, 0)
                self.assertEqual(calls, [])

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

    def test_temporary_root_is_explicit_when_platform_default_ignores_tmpdir(self) -> None:
        with tempfile.TemporaryDirectory(prefix="coven installer ") as directory:
            root = pathlib.Path(directory)
            apt_etc_dir = root / "apt"
            default_dir = root / "platform default"
            default_dir.mkdir()
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, _sources = self.run_installer(
                apt_etc_dir, mktemp_default_dir=default_dir
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(len(calls), 4)
            self.assertEqual(list(default_dir.iterdir()), [])

    def test_update_and_install_are_scoped_to_ubuntu_sources_and_temp_lists(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(len(calls), 4)
        self.assertEqual(calls[0][0], "apt-get")
        self.assertEqual(calls[1][0], "apt-get")
        self.assertIn("update", calls[0])
        for call, phase in zip(calls[1:], ("--print-uris", "--download-only", "--no-download")):
            self.assertIn("install", call)
            self.assertIn(phase, call)
            self.assertIn("-y", call)
            self.assertIn("--no-install-recommends", call)
            self.assertEqual(call[-2:], ["--", "libopenblas-dev"])
        for call in calls:
            joined = "\n".join(call)
            self.assertIn("Dir::Etc::sourcelist=/dev/null", call)
            self.assertIn("APT::Get::List-Cleanup=0", call)
            self.assertRegex(joined, r"Dir::Etc::sourceparts=.*sources\.list\.d")
            self.assertRegex(joined, r"Dir::State::lists=.*/lists")
            self.assertNotIn("/etc/apt/sources.list.d", joined)
            self.assertIn("APT::Get::AllowUnauthenticated=false", call)
            self.assertIn("Acquire::AllowInsecureRepositories=false", call)
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
        self.assertEqual(len(calls), 4)
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
                    deb [arch=amd64 signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] http://security.ubuntu.com/ubuntu noble-security main universe
                    deb [arch=amd64] https://dl.google.com/linux/chrome/deb/ stable main
                    deb https://evilubuntu.com/ubuntu noble main
                    deb https://third-party.example/packages stable main # ubuntu.com/ubuntu
                    """
                ),
                encoding="utf-8",
            )

            completed, calls, sources = self.run_installer(apt_etc_dir)

        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertEqual(len(calls), 4)
        source_text = "\n".join(sources)
        self.assertIn("archive.ubuntu.com/ubuntu", source_text)
        self.assertIn("security.ubuntu.com/ubuntu", source_text)
        self.assertNotIn("dl.google.com", source_text)
        self.assertNotIn("evilubuntu.com", source_text)
        self.assertNotIn("third-party.example", source_text)
        self.assertIn("signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg", source_text)

    def test_update_failure_propagates_without_running_install(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, _sources = self.run_installer(apt_etc_dir, fail_command="update")

        self.assertEqual(completed.returncode, 42)
        self.assertEqual(len(calls), 1)
        self.assertIn("update", calls[0])

    def test_lookalike_archive_hosts_are_never_installed(self) -> None:
        for uri in [
            "https://evilubuntu.com/ubuntu",
            "https://ports.ubuntu.com.attacker.example/ubuntu-ports",
            "https://archive.ubuntu.com/ubuntu-ports-extra",
            "https://ubuntu.com.attacker.example/ubuntu",
            "https://attacker.example/ubuntu.com/ubuntu",
        ]:
            for deb822 in [False, True]:
                with self.subTest(uri=uri, deb822=deb822), tempfile.TemporaryDirectory() as directory:
                    apt_etc_dir = pathlib.Path(directory)
                    if deb822:
                        self.write_deb822_ubuntu_source(apt_etc_dir)
                        source = apt_etc_dir / "sources.list.d/ubuntu.sources"
                        source.write_text(
                            f"Types: deb\nURIs: {uri}\nSuites: noble\n"
                            "Components: main\n"
                            "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
                            encoding="utf-8",
                        )
                    else:
                        (apt_etc_dir / "sources.list").write_text(
                            f"deb {uri} noble main\n", encoding="utf-8"
                        )
                    completed, calls, _sources = self.run_installer(apt_etc_dir)
                    self.assertEqual(completed.returncode, 1, completed.stderr)
                    self.assertEqual(calls, [])

    def test_every_deb822_stanza_requires_ubuntu_uris_and_signing_metadata(self) -> None:
        for extra in [
            "Types: deb\nURIs: https://third-party.example/packages\nSuites: stable\n"
            "Components: main\nSigned-By: /usr/share/keyrings/vendor.gpg\n",
            "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu/\nSuites: noble\n"
            "Components: main\n",
        ]:
            with self.subTest(extra=extra), tempfile.TemporaryDirectory() as directory:
                apt_etc_dir = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(apt_etc_dir)
                source = apt_etc_dir / "sources.list.d/ubuntu.sources"
                source.write_text(source.read_text(encoding="utf-8") + "\n" + extra, encoding="utf-8")
                completed, calls, _sources = self.run_installer(apt_etc_dir)
                self.assertEqual(completed.returncode, 1, completed.stderr)
                self.assertEqual(calls, [])

    def test_every_uri_in_a_deb822_field_or_mirror_file_must_be_ubuntu(self) -> None:
        for mirror in [False, True]:
            with self.subTest(mirror=mirror), tempfile.TemporaryDirectory() as directory:
                apt_etc_dir = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(apt_etc_dir)
                uris = "http://archive.ubuntu.com/ubuntu/\n https://third-party.example/packages"
                if mirror:
                    mirror_file = apt_etc_dir / "apt-mirrors.txt"
                    mirror_file.write_text(uris + "\n", encoding="utf-8")
                    uris = f"mirror+file:{mirror_file}"
                source = apt_etc_dir / "sources.list.d/ubuntu.sources"
                source.write_text(
                    f"Types: deb\nURIs: {uris}\nSuites: noble\nComponents: main\n"
                    "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
                    encoding="utf-8",
                )
                completed, calls, _sources = self.run_installer(apt_etc_dir)
                self.assertEqual(completed.returncode, 1, completed.stderr)
                self.assertEqual(calls, [])

    def test_deb822_ubuntu_uri_continuations_preserve_the_source(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)
            source = apt_etc_dir / "sources.list.d/ubuntu.sources"
            text = source.read_text(encoding="utf-8").replace(
                "URIs: http://azure.archive.ubuntu.com/ubuntu/",
                "URIs: http://azure.archive.ubuntu.com/ubuntu/\n"
                " https://archive.ubuntu.com/ubuntu/",
            )
            source.write_text(text, encoding="utf-8")
            completed, calls, sources = self.run_installer(apt_etc_dir)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertEqual(len(calls), 4)
            self.assertIn(text, "\n".join(sources))
            self.assertEqual(source.read_text(encoding="utf-8"), text)

    def test_install_failure_propagates(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_deb822_ubuntu_source(apt_etc_dir)

            completed, calls, _sources = self.run_installer(apt_etc_dir, fail_command="--no-download")

        self.assertEqual(completed.returncode, 43)
        self.assertEqual(len(calls), 4)
        self.assertIn("--no-download", calls[3])

    def test_download_failure_propagates_without_retry_or_install(self) -> None:
        # Only a stopped transfer is retried; apt's own errors stay fatal.
        for phase, count in (("--print-uris", 2), ("--download-only", 3)):
            with self.subTest(phase=phase), tempfile.TemporaryDirectory() as directory:
                apt_etc_dir = pathlib.Path(directory)
                self.write_deb822_ubuntu_source(apt_etc_dir)

                completed, calls, _sources = self.run_installer(apt_etc_dir, fail_command=phase)

                self.assertEqual(completed.returncode, 43)
                self.assertEqual(len(calls), count)
                self.assertIn(phase, calls[-1])
                self.assertNotIn("warning", completed.stderr)

    def write_mirror_file_source(self, apt_etc_dir: pathlib.Path) -> pathlib.Path:
        mirror_file = apt_etc_dir / "apt-mirrors.txt"
        mirror_file.write_text(
            "http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n"
            "https://archive.ubuntu.com/ubuntu/\tpriority:2 arch:amd64\n"
            "# comment\n"
            "https://security.ubuntu.com/ubuntu/\tpriority:3\n",
            encoding="utf-8",
        )
        source_dir = apt_etc_dir / "sources.list.d"
        source_dir.mkdir(parents=True)
        (source_dir / "ubuntu.sources").write_text(
            f"Types: deb\nURIs: mirror+file:{mirror_file}\n"
            "Suites: noble noble-updates noble-backports noble-security\n"
            "Components: main restricted universe multiverse\n"
            "Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n",
            encoding="utf-8",
        )
        return mirror_file

    def test_crawling_download_is_stopped_and_retried_with_the_next_mirror_first(self) -> None:
        # #1578: the archive download crawled at ~19 kB/s on the first mirror.
        for uid in (0, 1000):
            with self.subTest(uid=uid), tempfile.TemporaryDirectory() as directory:
                apt_etc_dir = pathlib.Path(directory)
                mirror_file = self.write_mirror_file_source(apt_etc_dir)
                before = mirror_file.read_bytes()
                mirror_lists: list[list[str]] = []
                started = time.monotonic()

                completed, calls, sources = self.run_installer(
                    apt_etc_dir,
                    uid=uid,
                    sudo_ok=bool(uid),
                    slow_downloads=1,
                    # 2 MiB at a 1 MiB/s floor plus 1 s of grace: a 3 s bound.
                    print_uris="'mirror+file:/x/pool/a.deb' a.deb 2097152 SHA512:00\n"
                               "0 upgraded, 4 newly installed, 0 to remove and 9 not upgraded.\n",
                    extra_env={
                        "COVEN_APT_DOWNLOAD_GRACE_SECONDS": "1",
                        "COVEN_APT_DOWNLOAD_MIN_BYTES_PER_SECOND": "1048576",
                    },
                    mirror_log=mirror_lists,
                    temp_prefix="coven-installer-",
                )

                self.assertEqual(completed.returncode, 0, completed.stderr)
                self.assertLess(time.monotonic() - started, 30)
                self.assertIn(
                    "::warning::Native dependency download exceeded 3s (attempt 1 of 3); "
                    "retrying with the next mirror first", completed.stderr)
                phases = [call[call.index("--") - 1] for call in calls[1:]]
                self.assertEqual(phases, ["--print-uris", "--download-only", "--download-only",
                                          "--no-download"])
                self.assertEqual(mirror_lists, [
                    ["http://azure.archive.ubuntu.com/ubuntu/\tpriority:1\n"
                     "https://archive.ubuntu.com/ubuntu/\tpriority:2 arch:amd64\n"
                     "https://security.ubuntu.com/ubuntu/\tpriority:3\n"],
                    ["https://archive.ubuntu.com/ubuntu/\tpriority:1 arch:amd64\n"
                     "https://security.ubuntu.com/ubuntu/\tpriority:2\n"
                     "http://azure.archive.ubuntu.com/ubuntu/\tpriority:3\n"],
                ])
                source_text = "\n".join(sources)
                self.assertNotIn(str(mirror_file), source_text)
                self.assertRegex(source_text, r"URIs: mirror\+file:\S+/mirrors/0\.list")
                self.assertIn("Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg", source_text)
                self.assertEqual(mirror_file.read_bytes(), before)

    def test_temporary_root_that_is_not_uri_safe_keeps_the_host_mirror_list(self) -> None:
        # A space would split the URIs field, so the retry runs without rotation.
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            mirror_file = self.write_mirror_file_source(apt_etc_dir)

            completed, calls, sources = self.run_installer(
                apt_etc_dir,
                slow_downloads=1,
                extra_env={"COVEN_APT_DOWNLOAD_GRACE_SECONDS": "1"},
                temp_prefix="coven installer ",
            )

            self.assertEqual(completed.returncode, 0, completed.stderr)
            self.assertIn("(attempt 1 of 3)", completed.stderr)
            self.assertIn(f"URIs: mirror+file:{mirror_file}\n", "\n".join(sources) + "\n")
            self.assertEqual(len(calls), 5)

    def test_download_that_crawls_on_every_attempt_fails_within_the_bound(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            apt_etc_dir = pathlib.Path(directory)
            self.write_mirror_file_source(apt_etc_dir)
            mirror_lists: list[list[str]] = []
            started = time.monotonic()

            completed, calls, _sources = self.run_installer(
                apt_etc_dir,
                slow_downloads=99,
                extra_env={"COVEN_APT_DOWNLOAD_GRACE_SECONDS": "1", "COVEN_APT_DOWNLOAD_ATTEMPTS": "2"},
                mirror_log=mirror_lists,
            )

            self.assertEqual(completed.returncode, 1)
            self.assertLess(time.monotonic() - started, 30)
            self.assertIn(
                "::error::Native dependency download did not finish within the bounded retry budget",
                completed.stderr)
            self.assertEqual(len(mirror_lists), 2)
            self.assertFalse(any("--no-download" in call for call in calls))

    def test_download_bounds_must_be_positive_integers(self) -> None:
        for name in ("COVEN_APT_DOWNLOAD_ATTEMPTS", "COVEN_APT_DOWNLOAD_GRACE_SECONDS",
                     "COVEN_APT_DOWNLOAD_MIN_BYTES_PER_SECOND"):
            for value in ("0", "-1", "1.5", "07", "1e3", "10000000000"):
                with self.subTest(name=name, value=value), tempfile.TemporaryDirectory() as directory:
                    apt_etc_dir = pathlib.Path(directory)
                    self.write_deb822_ubuntu_source(apt_etc_dir)
                    completed, calls, _sources = self.run_installer(apt_etc_dir, extra_env={name: value})
                    self.assertEqual(completed.returncode, 1)
                    self.assertEqual(calls, [])
                    self.assertIn("download bounds must be positive integers", completed.stderr)

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
