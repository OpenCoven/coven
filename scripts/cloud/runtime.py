"""Credential-free child execution and faithful doctor capability classification."""
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile


def execution_environment(cache, home, *, node_bin=None):
    cache, home = Path(cache), Path(home)
    environment = {
        "PATH": os.pathsep.join([str(cache / "cargo/bin"), str(node_bin or cache / "node/bin"),
                                 "/usr/bin", "/bin"]),
        "HOME": str(home), "USERPROFILE": str(home),
        "XDG_CONFIG_HOME": str(home / "config"), "XDG_CACHE_HOME": str(cache / "xdg"),
        "CARGO_HOME": str(cache / "cargo"), "RUSTUP_HOME": str(cache / "rustup"),
        "CARGO_TARGET_DIR": str(cache / "target"),
        "COVEN_HOME": str(home / "coven"),
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
        "GIT_TERMINAL_PROMPT": "0", "CI": "true", "TERM": "dumb",
        "NO_COLOR": "1", "LANG": "C.UTF-8",
        "npm_config_cache": str(cache / "npm"),
        "npm_config_userconfig": os.devnull,
        "npm_config_globalconfig": os.devnull,
    }
    # Preserve managed routing and CA policy; do not add destinations, bypass
    # proxies or forward provider credentials / arbitrary process overrides.
    for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "NO_PROXY",
                 "http_proxy", "https_proxy", "all_proxy", "no_proxy",
                 "SSL_CERT_FILE", "SSL_CERT_DIR", "REQUESTS_CA_BUNDLE"):
        if name in os.environ:
            environment[name] = os.environ[name]
    return environment


def native_diagnostic(stderr):
    """Only exact fixed helper messages may leave the captured stderr stream."""
    messages = {
        "Native dependency privilege blocker: run in a root setup phase or grant noninteractive sudo.":
            ("native_privilege", "Run setup as root or have the environment owner grant noninteractive sudo."),
        "Unsupported OS: native dependencies require Debian or Ubuntu":
            ("native_distribution", "Select a supported Debian or Ubuntu managed base image."),
    }
    source_messages = [
        "Malformed os-release metadata", "Malformed os-release value",
        "Missing or invalid VERSION_CODENAME in os-release",
        "APT source requests an authentication bypass", "APT source disables release validity checks",
        "APT source suite does not match host VERSION_CODENAME", "APT source has no components",
        "Malformed legacy APT source", "Malformed legacy APT source option",
        "APT source contains a foreign distribution archive", "Unable to read APT or OS metadata",
    ]
    for distro in ("Debian", "Ubuntu"):
        source_messages += [f"{distro} apt source {suffix}" for suffix in (
            "has an orphan continuation line", "has malformed or duplicate fields", "has no package stanzas",
            "does not declare deb package sources", "contains a foreign distribution archive",
            "does not declare Signed-By key material", "has no deb entries for the host distribution archive")]
        source_messages.append(f"Unable to find the {distro} apt distribution source")
    for message in source_messages:
        messages[message] = ("native_source_validation", "Have the environment owner repair the base image's signed host-distribution APT sources; do not bypass validation.")
    for line in stderr.splitlines():
        if line.startswith("::error::") and line[9:] in messages:
            message = line[9:]
            code, recovery = messages[message]
            return {"code": code, "message": message, "recovery": recovery}
    return None


def run_command(argv, *, cwd, env, timeout):
    """Run an owned process group; never print potentially sensitive raw output."""
    with tempfile.TemporaryFile() as stdout, tempfile.TemporaryFile() as stderr:
        try:
            process = subprocess.Popen(argv, cwd=cwd, env=env, stdout=stdout, stderr=stderr,
                                       stdin=subprocess.DEVNULL, start_new_session=True)
        except OSError:
            return {"exit_code": None, "timed_out": False, "stdout": "", "launch_failed": True}
        timed_out = False
        try:
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            # Popen still owns the unreaped child; its process-group identity
            # cannot be reused before we terminate and reap it.
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        stdout.seek(0)
        data = stdout.read(1024 * 1024 + 1)
        oversized = len(data) > 1024 * 1024
        stderr.seek(0)
        safe_diagnostic = native_diagnostic(stderr.read(1024 * 1024).decode("utf-8", errors="replace"))
        return {"native_diagnostic": safe_diagnostic, "exit_code": process.returncode, "timed_out": timed_out,
                "stdout": "" if oversized else data.decode("utf-8", errors="replace"),
                "output_oversized": oversized, "launch_failed": False}


def classify_doctor(result):
    summary = {"exit_code": result.get("exit_code"), "contributor_diagnostic_complete": False,
               "operational_ready": False, "failed_checks": [], "provider_authentication": "unverified"}
    if result.get("timed_out") or result.get("launch_failed") or result.get("output_oversized"):
        return summary
    optional_missing = {
        "harnesses": "no harness is available",
        "engine": "the Coven engine is missing — `coven` and `coven chat` need it",
    }
    try:
        body = json.loads(result["stdout"])
        checks = body["checks"]
        if not isinstance(checks, list) or not checks:
            return summary
        identifiers = []
        failures = []
        for check in checks:
            identifier, status, message = check["id"], check["status"], check["message"]
            if (not isinstance(identifier, str) or identifier in identifiers
                    or status not in ("pass", "warn", "fail") or not isinstance(message, str)):
                return summary
            identifiers.append(identifier)
            if status == "fail":
                failures.append(check)
        if not {"daemon", "harnesses", "engine"} <= set(identifiers):
            return summary
        summary["failed_checks"] = [check["id"] for check in failures]
        # Reject contradictory status, JSON booleans, or unexpected process exits.
        if body["ok"] is not (not failures) or body["blocking"] is not bool(failures):
            return summary
        if result["exit_code"] != (1 if failures else 0):
            return summary
        summary["operational_ready"] = not failures
        summary["contributor_diagnostic_complete"] = all(
            optional_missing.get(check["id"]) == check["message"] for check in failures)
        return summary
    except (ValueError, KeyError, TypeError):
        return summary
