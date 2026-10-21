"""Prove contributor capabilities; provisioning and publication are separate."""
import argparse
import json
import os
from pathlib import Path
import platform
import tempfile

from config import PreflightError, preflight
from runtime import classify_doctor, execution_environment, run_command


class StageFailure(Exception):
    pass


def verify(repo, cache, *, run=run_command):
    report = {"schema_version": 1, "mode": "readiness", "environment": platform.system(),
              "development_ready": False, "operational_ready": False,
              "provider_authentication": "unverified", "stages": []}
    try:
        inventory = preflight(repo, cache)
    except PreflightError as error:
        return {**report, "blocked_stage": "preflight", "diagnostic": str(error)}
    repo, cache = Path(inventory["repository_root"]), Path(inventory["cache_root"])
    report.update(source_sha=inventory["source_sha"], pins=inventory["pins"])
    pins = inventory["pins"]
    rustup = str(cache / "cargo/bin/rustup")
    cargo = [rustup, "run", pins["rust"]["channel"], "cargo"]
    node_bin = cache / "node" / pins["node"] / "bin"
    current_stage = "runtime_home"
    try:
        # Short, disposable homes avoid Unix socket path limits and any existing
        # provider configuration. Smoke independently creates its own fake harnesses.
        with tempfile.TemporaryDirectory(prefix="cv-", dir="/tmp") as home:
            env = execution_environment(cache, Path(home), node_bin=node_bin)
            def command(stage, argv, *, timeout=30, expected=None, permit_doctor=False):
                nonlocal current_stage
                current_stage = stage
                result = run(argv, cwd=str(repo), env=env, timeout=timeout)
                passed = result.get("exit_code") == 0 and not result.get("timed_out")
                if expected is not None:
                    passed = passed and expected(result.get("stdout", "").strip())
                report["stages"].append({"name": stage, "exit_code": result.get("exit_code"),
                                         "timed_out": bool(result.get("timed_out")), "passed": passed})
                if not passed and not permit_doctor:
                    if stage == "cli_smoke" and result.get("smoke_diagnostic") is not None:
                        report["smoke_diagnostic"] = result["smoke_diagnostic"]
                    raise StageFailure
                return result

            channel = pins["rust"]["channel"]
            command("rust_version", [rustup, "run", channel, "rustc", "--version"],
                    expected=lambda text: text.startswith(f"rustc {channel} "))
            command("cargo_version", [*cargo, "--version"],
                    expected=lambda text: text.startswith(f"cargo {channel} "))
            command("rust_components", [rustup, "component", "list", "--installed", "--toolchain", channel],
                    expected=lambda text: all(any(entry == component or entry.startswith(component + "-")
                        for entry in text.splitlines()) for component in pins["rust"]["components"]))
            command("rust_targets", [rustup, "target", "list", "--installed", "--toolchain", channel],
                    expected=lambda text: set(pins["rust"]["targets"]) <= set(text.splitlines()))
            command("node_version", [str(node_bin / "node"), "--version"],
                    expected=lambda text: text == "v" + pins["node"])
            for package, pin in pins["package_managers"].items():
                name, version = pin.split("@")
                executable = cache / "managers" / pin / "bin" / name
                command("package_manager:" + package, [str(executable), "--version"],
                        expected=lambda text, version=version: text == version)
            # The locked Linux graph uses openssl-sys through coven-memory.
            # Fail at the missing native prerequisite before an opaque Cargo error.
            if platform.system() == "Linux":
                command("native_openssl", ["pkg-config", "--exists", "openssl"])
                # Smoke calls the external executable, not a shell builtin.
                # Signal zero checks our live parent without sending a signal.
                command("native_process_probe", ["kill", "-0", str(os.getpid())])
            command("workspace_build", [*cargo, "build", "--workspace", "--locked"], timeout=1800)
            doctor = command("doctor", [str(cache / "target/debug/coven"), "doctor", "--json"],
                             timeout=60, permit_doctor=True)
            report["doctor"] = classify_doctor(doctor)
            report["operational_ready"] = report["doctor"]["operational_ready"]
            if not report["doctor"]["contributor_diagnostic_complete"]:
                raise StageFailure
            command("cli_smoke", [*cargo, "test", "-p", "coven-cli", "--test", "smoke",
                                  "--locked", "--", "--nocapture"], timeout=900)
            report["development_ready"] = True
    except (StageFailure, OSError):
        report["operational_ready"] = False
        report["blocked_stage"] = current_stage
        report["diagnostic"] = "required command failed, timed out, or did not match repository pins"
    finally:
        try:
            after = preflight(repo, cache)
            if after["source_sha"] != inventory["source_sha"] or after["pins"] != pins:
                raise PreflightError("source changed during verification")
        except PreflightError as error:
            report.update(development_ready=False, operational_ready=False,
                          blocked_stage="source_integrity", diagnostic=str(error))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--cache", type=Path, required=True,
                        help="external storage populated by bootstrap; no credentials")
    args = parser.parse_args()
    report = verify(args.repo, args.cache)
    print(json.dumps(report, sort_keys=True))
    return 0 if report["development_ready"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
