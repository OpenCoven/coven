#!/usr/bin/env python3
"""Bootstrap orchestration fixtures do not prove real Cloud provisioning."""
import importlib.util
import json
from pathlib import Path
import re
import unittest
from unittest.mock import patch

import bootstrap

spec = importlib.util.spec_from_file_location("config_tests", Path(__file__).with_name("config-test.py"))
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


class BootstrapTests(fixtures.ConfigTests):
    def provisioned_cache(self):
        cache = Path(self.temp.name) / "cache"
        for binary in ("cargo/bin/rustup", "node/24.1.2/bin/node", "managers/pnpm@10.1.2/bin/pnpm"):
            path = cache / binary
            path.parent.mkdir(parents=True, exist_ok=True)
            path.touch()
        return cache

    def runner(self, fail=None, mutate=False):
        self.commands = []
        def run(argv, **kwargs):
            self.commands.append((argv, kwargs))
            output = ""
            if argv[-1] == "--version":
                output = "v24.1.2" if Path(argv[0]).name == "node" else "10.1.2"
            if mutate and "ci" in argv:
                (self.root / ".node-version").write_text("25.2.3\n")
            return {"exit_code": 23 if fail and (argv[0] == "curl" if fail == "curl" else any(fail in arg for arg in argv)) else 0,
                    "stdout": output, "timed_out": False}
        return run

    def ready(self, *args, **kwargs):
        return {"development_ready": True, "operational_ready": False}

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_repeated_setup_reuses_tools_and_preserves_pinned_locked_commands(self, *_):
        cache = self.provisioned_cache()
        runner = self.runner()
        for _ in range(2):
            report = bootstrap.prepare(self.root, cache, run=runner, verify=self.ready)
            self.assertTrue(report["development_ready"], report)
        commands = [argv for argv, _ in self.commands]
        self.assertFalse(any("curl" == argv[0] for argv in commands))
        native = next(argv for argv in commands if any(arg.endswith("install-native-link-dependencies.sh") for arg in argv))
        self.assertIn("procps", native)
        self.assertIn("libssl-dev", native)
        self.assertIn("pkg-config", native)
        install = next(argv for argv in commands if "toolchain" in argv)
        self.assertEqual(install[1:6], ["toolchain", "install", "1.2.3", "--profile", "minimal"])
        self.assertIn("clippy", install)
        self.assertIn("rustfmt", install)
        self.assertTrue(any("--frozen-lockfile" in argv and "--ignore-scripts" in argv for argv in commands))
        self.assertTrue(any("ci" in argv for argv in commands))
        self.assertFalse(any("OPENAI_API_KEY" in kw["env"] for _, kw in self.commands))

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_native_failure_does_not_download_or_claim_ready(self, *_):
        runner = self.runner(fail="install-native-link-dependencies.sh")
        report = bootstrap.prepare(self.root, self.provisioned_cache(), run=runner, verify=self.ready)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "native_dependencies")
        self.assertEqual(report["stages"][-1]["exit_code"], 23)
        self.assertEqual(len(self.commands), 1)

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_tracked_mutation_blocks_readiness_without_repair(self, *_):
        report = bootstrap.prepare(self.root, self.provisioned_cache(),
                                   run=self.runner(mutate=True), verify=self.ready)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "source_integrity")
        self.assertEqual((self.root / ".node-version").read_text(), "25.2.3\n")

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_denied_download_has_no_fallback_or_ready_claim(self, *_):
        cache = Path(self.temp.name) / "cache"
        report = bootstrap.prepare(self.root, cache, run=self.runner(fail="curl"), verify=self.ready)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "rustup_download")
        downloads = [argv for argv, _ in self.commands if argv[0] == "curl"]
        self.assertEqual(len(downloads), 1)
        self.assertEqual(downloads[0][-1], "https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init")
        self.assertIn("--proto-redir", downloads[0])
        self.assertNotIn("--insecure", downloads[0])

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_owned_cache_lock_blocks_concurrent_setup(self, *_):
        import fcntl
        cache = self.provisioned_cache()
        with (cache / ".bootstrap.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            report = bootstrap.prepare(self.root, cache, run=self.runner(), verify=self.ready)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "cache")
        self.assertEqual(self.commands, [])

    @patch("bootstrap.platform.system", return_value="Linux")
    @patch("bootstrap.platform.machine", return_value="x86_64")
    def test_cold_cache_installs_verified_artifacts_then_reuses_them(self, *_):
        import hashlib
        import io
        import tarfile
        cache = Path(self.temp.name) / "cache"
        runner = self.runner()
        def cold_runner(argv, **kwargs):
            result = runner(argv, **kwargs)
            if argv[0] == "curl":
                destination = Path(argv[argv.index("--output") + 1])
                url = argv[-1]
                if url.endswith("rustup-init"):
                    destination.write_bytes(b"synthetic installer")
                elif url.endswith("rustup-init.sha256"):
                    digest = hashlib.sha256((destination.parent / "rustup-init").read_bytes()).hexdigest()
                    destination.write_text(f"{digest} *./rustup-init\n")
                elif url.endswith(".tar.gz"):
                    with tarfile.open(destination, "w:gz") as bundle:
                        for name in ("node", "npm"):
                            member = tarfile.TarInfo("node-v24.1.2-linux-x64/bin/" + name)
                            member.size = 1
                            bundle.addfile(member, io.BytesIO(b"x"))
                elif url.endswith("SHASUMS256.txt"):
                    filename = "node-v24.1.2-linux-x64.tar.gz"
                    digest = hashlib.sha256((destination.parent / filename).read_bytes()).hexdigest()
                    destination.write_text(f"{digest}  {filename}\n")
            elif Path(argv[0]).name == "rustup-init":
                binary = cache / "cargo/bin/rustup"
                binary.parent.mkdir(parents=True)
                binary.touch()
            elif "--global" in argv:
                binary = Path(argv[argv.index("--prefix") + 1]) / "bin/pnpm"
                binary.parent.mkdir(parents=True)
                binary.touch()
            return result
        for _ in range(2):
            report = bootstrap.prepare(self.root, cache, run=cold_runner, verify=self.ready)
            self.assertTrue(report["development_ready"], report)
        self.assertEqual(sum(argv[0] == "curl" for argv, _ in self.commands), 4)
        self.assertTrue((cache / "node/24.1.2/bin/node").is_file())

    def test_cache_directory_symlink_is_rejected_before_commands(self):
        cache = Path(self.temp.name) / "cache"
        cache.mkdir()
        (cache / "cargo").symlink_to(self.root, target_is_directory=True)
        report = bootstrap.prepare(self.root, cache, run=self.runner(), verify=self.ready)
        self.assertFalse(report["development_ready"])
        self.assertEqual(report["blocked_stage"], "cache")
        self.assertEqual(self.commands, [])


class NetworkInventoryTests(unittest.TestCase):
    """Every destination setup references is documented; enforcement needs a real Cloud run."""
    repo = Path(__file__).resolve().parents[2]

    def documented_hosts(self):
        readme = (self.repo / "scripts/cloud/README.md").read_text()
        return set(re.findall(r"^\| `([a-z0-9.-]+)` \|", readme, re.MULTILINE))

    def referenced_hosts(self):
        hosts = set()
        sources = [path for path in (self.repo / "scripts/cloud").glob("*.py") if not path.stem.endswith("-test")]
        for path in sources + [self.repo / "scripts/install-native-link-dependencies.sh"]:
            hosts |= set(re.findall(r"https://([A-Za-z0-9.-]+)", path.read_text()))
        for source in re.findall(r'^source = "([^"]+)"', (self.repo / "Cargo.lock").read_text(), re.MULTILINE):
            if source == "registry+https://github.com/rust-lang/crates.io-index":
                hosts |= {"index.crates.io", "static.crates.io"}  # sparse protocol, not the git index
            else:
                hosts.add(re.match(r"\w+\+https://([^/]+)/", source).group(1))
        lock = json.loads((self.repo / "packages/channels/package-lock.json").read_text())
        hosts |= {re.match(r"https://([^/]+)/", entry["resolved"]).group(1)
                  for entry in lock["packages"].values() if "resolved" in entry}
        for npmrc in self.repo.glob("packages/*/.npmrc"):
            self.assertNotRegex(npmrc.read_text(), r"(?m)^\s*(@[^:]+:)?registry\s*=", npmrc)
        return hosts | {"registry.npmjs.org"}  # npm and pnpm default registry

    def test_referenced_destinations_are_documented(self):
        missing = self.referenced_hosts() - self.documented_hosts()
        self.assertFalse(missing, f"document these hosts in scripts/cloud/README.md: {sorted(missing)}")

    def test_inventory_excludes_unscoped_destinations(self):
        documented = self.documented_hosts()
        self.assertTrue(documented)
        self.assertFalse(documented & {"api.github.com", "objects.githubusercontent.com"})


if __name__ == "__main__":
    unittest.main()
