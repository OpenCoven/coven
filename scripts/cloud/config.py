"""Read repository-owned pins and validate source/storage before provisioning.

Requires Python 3.11+ for the standard-library TOML parser. No environment
mutation, network access, or installed-tool readiness claim is made here.
"""
import json
import os
from pathlib import Path
import re
import subprocess
import tomllib


class PreflightError(ValueError):
    """A fixed, non-secret diagnostic safe to include in the capability report."""


def _unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise PreflightError("duplicate package manifest field")
        result[key] = value
    return result


def _exact_version(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", value) is not None


def _names(value):
    return (isinstance(value, list) and len(value) == len(set(value))
            and all(isinstance(item, str) and re.fullmatch(r"[a-z0-9][a-z0-9_-]*", item)
                    for item in value))


def read_pins(repo):
    repo = Path(repo)
    try:
        rust_document = tomllib.loads((repo / "rust-toolchain.toml").read_text(encoding="utf-8"))
        rust = rust_document["toolchain"]
        node = (repo / ".node-version").read_text(encoding="utf-8").strip()
        # This package owns an explicit pin today. Its disappearance is a
        # contract change, not permission to silently use an ambient manager.
        if not (repo / "packages/openclaw-coven/package.json").is_file():
            raise PreflightError("missing package manager source")
        managers = {}
        for manifest in sorted((repo / "packages").glob("*/package.json")):
            package = json.loads(manifest.read_text(encoding="utf-8"), object_pairs_hook=_unique_object)
            if not isinstance(package, dict):
                raise PreflightError("package manifest must be an object")
            manager = package.get("packageManager")
            if manager is None and manifest.parent.name != "openclaw-coven":
                continue
            if not isinstance(manager, str) or not re.fullmatch(r"(?:pnpm|npm)@[0-9]+\.[0-9]+\.[0-9]+", manager):
                raise PreflightError("packageManager requires an exact supported version")
            managers[manifest.parent.relative_to(repo).as_posix()] = manager
        # Refuse future toolchain fields until provisioning explicitly supports
        # them; silently ignoring a newly pinned target/profile breaks the contract.
        if set(rust_document) != {"toolchain"} or not isinstance(rust, dict):
            raise PreflightError("unsupported Rust toolchain document")
        if not set(rust) <= {"channel", "components", "profile", "targets"}:
            raise PreflightError("unsupported Rust toolchain field")
        if not _exact_version(rust.get("channel")) or not _exact_version(node):
            raise PreflightError("Rust and Node require exact repository versions")
        components, targets = rust.get("components", []), rust.get("targets", [])
        profile = rust.get("profile", "default")
        if not _names(components) or not _names(targets) or profile not in ("minimal", "default", "complete"):
            raise PreflightError("invalid Rust toolchain components, targets or profile")
        return {"rust": {"channel": rust["channel"], "components": components,
                         "profile": profile, "targets": targets},
                "node": node, "package_managers": managers}
    except PreflightError:
        raise
    except (OSError, ValueError, KeyError, TypeError):
        # Parser errors can include source text; never expose it in diagnostics.
        raise PreflightError("missing or malformed repository pin metadata") from None


def preflight(repo, cache):
    repo, cache = Path(repo).resolve(), Path(cache).resolve()
    if repo == cache or repo in cache.parents or cache in repo.parents:
        raise PreflightError("cache and repository locations must not overlap")
    pins = read_pins(repo)
    # Read-only Git queries must not inherit alternate indexes, work trees or
    # injected Git configuration. Preserve PATH for the host's Git executable.
    env = {"PATH": os.environ.get("PATH", os.defpath), "GIT_OPTIONAL_LOCKS": "0",
           "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull}
    def git(*args):
        try:
            result = subprocess.run(["git", "-C", str(repo), *args], env=env,
                                    capture_output=True, check=True, timeout=30)
            return result.stdout.decode("utf-8").strip()
        except (OSError, ValueError, subprocess.SubprocessError):
            raise PreflightError("unable to inspect repository state") from None

    if Path(git("rev-parse", "--show-toplevel")).resolve() != repo:
        raise PreflightError("repository root is required")
    if git("status", "--porcelain", "--untracked-files=no"):
        raise PreflightError("tracked source is dirty; preserve changes before setup")
    return {"source_sha": git("rev-parse", "HEAD"), "pins": pins,
            "cache_root": str(cache), "repository_root": str(repo)}
