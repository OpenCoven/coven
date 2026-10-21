"""Provision repository-pinned contributor tools, then prove local capabilities."""
import argparse
import fcntl
import json
import os
from pathlib import Path
import platform
import tempfile

from config import PreflightError, preflight
from provision import ProvisionError, check_checksum, extract_node
from readiness import verify as verify_readiness
from runtime import execution_environment, run_command


class StageFailure(Exception):
    pass


def prepare(repo, cache, *, run=run_command, verify=verify_readiness):
    report = {"schema_version": 1, "mode": "bootstrap", "environment": platform.system(),
              "development_ready": False, "operational_ready": False,
              "provider_authentication": "unverified", "stages": []}
    try:
        inventory = preflight(repo, cache)
    except PreflightError as error:
        return {**report, "blocked_stage": "preflight", "diagnostic": str(error)}
    repo, cache = Path(inventory["repository_root"]), Path(inventory["cache_root"])
    pins = inventory["pins"]
    report.update(source_sha=inventory["source_sha"], pins=pins)
    stage = "cache"
    lock = None
    try:
        # Cache roots are owned by setup. Refuse redirected roots before writing;
        # ordinary tool symlinks within an installed distribution remain supported.
        owned = ["cargo", "cargo/bin", "rustup", "target", "node", "managers", "npm", "xdg",
                 "node/" + pins["node"]]
        owned += ["managers/" + pin for pin in pins["package_managers"].values()]
        for relative in owned:
            path = cache / relative
            if path.is_symlink() or not path.resolve().is_relative_to(cache):
                raise ProvisionError("cache storage is redirected; use a new dedicated external cache")
        cache.mkdir(parents=True, exist_ok=True, mode=0o700)
        lock = os.open(cache / ".bootstrap.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ProvisionError("another bootstrap owns this cache; wait for it to finish") from None
        stage = "platform"
        architectures = {"x86_64": ("x86_64", "x64"), "aarch64": ("aarch64", "arm64")}
        if platform.system() != "Linux" or platform.machine() not in architectures:
            raise ProvisionError("managed bootstrap requires Linux x86_64 or aarch64")
        rust_arch, node_arch = architectures[platform.machine()]
        with tempfile.TemporaryDirectory(prefix="cv-", dir="/tmp") as home, \
                tempfile.TemporaryDirectory(prefix="setup-", dir=cache) as staging:
            staging = Path(staging)
            node_bin = cache / "node" / pins["node"] / "bin"
            env = execution_environment(cache, Path(home), node_bin=node_bin)

            def command(name, argv, *, cwd=repo, timeout=900, expected=None):
                nonlocal stage
                stage = name
                result = run([str(arg) for arg in argv], cwd=str(cwd), env=env, timeout=timeout)
                passed = result.get("exit_code") == 0 and not result.get("timed_out")
                if expected is not None:
                    passed = passed and expected(result.get("stdout", "").strip())
                report["stages"].append({"name": name, "exit_code": result.get("exit_code"),
                                         "timed_out": bool(result.get("timed_out")), "passed": passed})
                if not passed:
                    if name == "native_dependencies" and result.get("native_diagnostic"):
                        report["native_diagnostic"] = result["native_diagnostic"]
                    raise StageFailure
                return result

            def download(name, url, destination):
                # Fixed official HTTPS destinations only. Managed policy stays in
                # force; a denied transfer fails this stage with no alternate host.
                command(name, ["curl", "--fail", "--silent", "--show-error", "--location",
                               "--proto", "=https", "--proto-redir", "=https",
                               "--connect-timeout", "30", "--max-time", "600",
                               "--output", destination, url], timeout=660)

            command("native_dependencies", ["bash", repo / "scripts/install-native-link-dependencies.sh",
                    "libopenblas-dev", "build-essential", "pkg-config", "curl", "ca-certificates"], timeout=1200)
            rustup = cache / "cargo/bin/rustup"
            if not rustup.is_file():
                base = f"https://static.rust-lang.org/rustup/dist/{rust_arch}-unknown-linux-gnu"
                artifact, checksum = staging / "rustup-init", staging / "rustup-init.sha256"
                download("rustup_download", base + "/rustup-init", artifact)
                download("rustup_checksum_download", base + "/rustup-init.sha256", checksum)
                stage = "rustup_integrity"
                check_checksum(artifact, checksum.read_text(), "rustup-init")
                artifact.chmod(0o700)
                command("rustup_install", [artifact, "-y", "--no-modify-path", "--default-toolchain", "none"])
            rust = pins["rust"]
            install = [rustup, "toolchain", "install", rust["channel"], "--profile", rust["profile"]]
            for component in rust["components"]:
                install += ["--component", component]
            for target in rust["targets"]:
                install += ["--target", target]
            command("rust_toolchain", install, timeout=1200)
            if not (node_bin / "node").is_file():
                prefix = f"node-v{pins['node']}-linux-{node_arch}"
                filename = prefix + ".tar.gz"
                base = f"https://nodejs.org/dist/v{pins['node']}"
                artifact, checksum = staging / filename, staging / "SHASUMS256.txt"
                download("node_download", base + "/" + filename, artifact)
                download("node_checksum_download", base + "/SHASUMS256.txt", checksum)
                stage = "node_integrity"
                check_checksum(artifact, checksum.read_text(), filename)
                extracted = extract_node(artifact, staging / "unpacked", prefix)
                node_bin.parent.parent.mkdir(parents=True, exist_ok=True)
                # Interrupted installation is not silently deleted or reused.
                if node_bin.parent.exists():
                    raise ProvisionError("incomplete Node cache; use a new dedicated external cache")
                extracted.rename(node_bin.parent)
            command("node_version", [node_bin / "node", "--version"],
                    expected=lambda value: value == "v" + pins["node"])
            for pin in sorted(set(pins["package_managers"].values())):
                name, version = pin.split("@")
                prefix = cache / "managers" / pin
                executable = prefix / "bin" / name
                if not executable.is_file():
                    command("manager_install:" + name, [node_bin / "npm", "install", "--global",
                            "--prefix", prefix, pin, "--ignore-scripts", "--no-audit", "--no-fund"])
                command("manager_version:" + name, [executable, "--version"],
                        expected=lambda value, version=version: value == version)
            for package, default in (("packages/channels", "npm"), ("packages/openclaw-coven", "pnpm")):
                pin = pins["package_managers"].get(package)
                name = pin.split("@")[0] if pin else default
                executable = cache / "managers" / pin / "bin" / name if pin else node_bin / name
                args = ["ci", "--no-audit", "--no-fund"] if name == "npm" else ["install", "--frozen-lockfile", "--ignore-scripts"]
                command("dependencies:" + package, [executable, *args], cwd=repo / package)
            stage = "source_integrity"
            after = preflight(repo, cache)
            if after["source_sha"] != inventory["source_sha"] or after["pins"] != pins:
                raise ProvisionError("source changed during setup")
            stage = "readiness"
            report["readiness"] = verify(repo, cache, run=run)
            report["development_ready"] = report["readiness"]["development_ready"]
            report["operational_ready"] = report["readiness"]["operational_ready"]
            if not report["development_ready"]:
                raise StageFailure
    except (StageFailure, OSError, ProvisionError, PreflightError) as error:
        diagnostic = str(error) if isinstance(error, (ProvisionError, PreflightError)) else (
            "required setup command failed or storage is unavailable; inspect the stage and exit code")
        report.update(development_ready=False, operational_ready=False, blocked_stage=stage, diagnostic=diagnostic)
    finally:
        if lock is not None:
            os.close(lock)
        try:
            after = preflight(repo, cache)
            if after["source_sha"] != inventory["source_sha"] or after["pins"] != pins:
                raise PreflightError("source changed during setup")
        except PreflightError as error:
            report.update(development_ready=False, operational_ready=False,
                          blocked_stage="source_integrity", diagnostic=str(error))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--cache", type=Path, required=True, help="dedicated external tool cache")
    args = parser.parse_args()
    report = prepare(args.repo, args.cache)
    print(json.dumps(report, sort_keys=True))
    return 0 if report["development_ready"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
