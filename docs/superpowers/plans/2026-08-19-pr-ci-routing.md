# Pull Request CI Routing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route and parallelize pull-request checks so ordinary Rust PRs finish required CI near 10 minutes while deferred performance and full packaging coverage run on `main` and release tags.

**Architecture:** A repository-owned Python classifier converts changed paths into stable job-category outputs. `.github/workflows/ci.yml` uses those outputs to start independent conditional jobs, a final aggregate gate, bounded timeouts, cancellation, and pinned caches. The release workflow remains independent and adds the deferred performance baseline before publication.

**Tech Stack:** GitHub Actions YAML, Python 3 `unittest`, Bash, Cargo, Node.js 24, pnpm 10.11.1, actionlint 1.7.12, `actions/cache` 6.1.0.

---

## File Structure

- Create `scripts/classify-ci-changes.py`: pure changed-path classifier and GitHub-output CLI.
- Create `scripts/classify-ci-changes-test.py`: table-driven unit coverage for every routing category and fail-closed behavior.
- Create `scripts/check-workflows.sh`: checksum-verified actionlint 1.7.12 runner for Linux and macOS.
- Create `scripts/check-workflows-test.py`: verifies the pinned actionlint release/checksums and script interface without downloading tools.
- Create `scripts/check-ci-workflow-test.py`: textual policy tests for required job names, conditions, timeouts, cache pin, aggregate gate, and deferred release coverage.
- Rewrite `.github/workflows/ci.yml`: change router, parallel/conditional PR jobs, main-only extended jobs, caches, timeouts, cancellation, and `PR gate`.
- Modify `.github/workflows/release-npm.yml`: add the release performance baseline and require it before publish.
- Modify `CONTRIBUTING.md`: document routed PR checks and clarify that contributors still run the full relevant local gate.

### Task 1: Build the changed-path classifier

**Files:**
- Create: `scripts/classify-ci-changes-test.py`
- Create: `scripts/classify-ci-changes.py`

- [ ] **Step 1: Write the failing classifier tests**

Create `scripts/classify-ci-changes-test.py`:

```python
#!/usr/bin/env python3
from __future__ import annotations

import importlib.util
import io
import pathlib
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("classify-ci-changes.py")
spec = importlib.util.spec_from_file_location("classify_ci_changes", SCRIPT)
assert spec is not None
classify_ci_changes = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(classify_ci_changes)


class ClassifyCiChangesTests(unittest.TestCase):
    def assert_categories(
        self, paths: list[str], **expected: bool
    ) -> None:
        actual = classify_ci_changes.classify(paths)
        for name in classify_ci_changes.CATEGORY_NAMES:
            self.assertEqual(
                actual[name],
                expected.get(name, False),
                f"{name} for {paths}",
            )

    def test_docs_only_change_avoids_compiled_jobs(self) -> None:
        self.assert_categories(["docs/guide.md"], docs_only=True)

    def test_workspace_manifest_fans_out_shared_rust_surfaces(self) -> None:
        self.assert_categories(
            ["Cargo.lock"],
            rust=True,
            afs=True,
            npm_packaging=True,
            cargo_metadata=True,
        )

    def test_cli_rust_change_includes_linux_packaging_smoke(self) -> None:
        self.assert_categories(
            ["crates/coven-cli/src/daemon.rs"],
            rust=True,
            npm_packaging=True,
        )

    def test_afs_change_selects_rust_and_afs(self) -> None:
        self.assert_categories(
            ["crates/coven-afs/src/nfs.rs"],
            rust=True,
            afs=True,
        )

    def test_channels_change_is_package_local(self) -> None:
        self.assert_categories(
            ["packages/channels/src/index.ts"],
            channels=True,
        )

    def test_openclaw_change_is_package_local(self) -> None:
        self.assert_categories(
            ["packages/openclaw-coven/src/client.ts"],
            openclaw=True,
        )

    def test_npm_wrapper_change_selects_packaging(self) -> None:
        self.assert_categories(
            ["npm/coven/bin/coven.js"],
            npm_packaging=True,
        )

    def test_engine_change_selects_rust_engine_and_packaging(self) -> None:
        self.assert_categories(
            ["crates/coven-cli/src/engine_install.rs"],
            rust=True,
            npm_packaging=True,
            engine=True,
        )

    def test_workflow_change_fans_out_every_job_category(self) -> None:
        actual = classify_ci_changes.classify([".github/workflows/ci.yml"])
        self.assertFalse(actual["docs_only"])
        for name in classify_ci_changes.CATEGORY_NAMES:
            if name != "docs_only":
                self.assertTrue(actual[name], name)

    def test_unknown_non_doc_path_fails_closed_to_rust(self) -> None:
        self.assert_categories(["config/new-surface.toml"], rust=True)

    def test_mixed_docs_and_package_change_is_not_docs_only(self) -> None:
        self.assert_categories(
            ["docs/guide.md", "packages/channels/package.json"],
            channels=True,
        )

    def test_empty_input_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "at least one changed path"):
            classify_ci_changes.classify([])

    def test_github_output_uses_lowercase_booleans(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            output = pathlib.Path(directory) / "github-output"
            classify_ci_changes.write_github_output(
                classify_ci_changes.classify(["docs/guide.md"]), output
            )
            self.assertIn("docs_only=true\n", output.read_text())
            self.assertIn("rust=false\n", output.read_text())


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
python3 scripts/classify-ci-changes-test.py
```

Expected: FAIL because `scripts/classify-ci-changes.py` does not exist.

- [ ] **Step 3: Implement the classifier**

Create `scripts/classify-ci-changes.py`:

```python
#!/usr/bin/env python3
from __future__ import annotations

import argparse
import pathlib
import sys
from collections.abc import Iterable

CATEGORY_NAMES = (
    "docs_only",
    "rust",
    "afs",
    "channels",
    "openclaw",
    "npm_packaging",
    "engine",
    "workflow",
    "cargo_metadata",
)

WORKFLOW_PATHS = (
    ".github/workflows/",
    "scripts/classify-ci-changes.py",
    "scripts/classify-ci-changes-test.py",
    "scripts/check-workflows.sh",
    "scripts/check-workflows-test.py",
    "scripts/check-ci-workflow-test.py",
)
NPM_PACKAGING_SCRIPTS = {
    "scripts/publish-npm.mjs",
    "scripts/publish-npm-test.mjs",
    "scripts/release-npm-context.mjs",
    "scripts/release-npm-platform-matrix.mjs",
    "scripts/test-cli-prepublish.mjs",
}
ENGINE_PATHS = (
    "crates/coven-cli/engine.lock",
    "crates/coven-cli/src/engine.rs",
    "crates/coven-cli/src/engine_install.rs",
    "scripts/pin-engine.sh",
)


def starts_with_any(path: str, prefixes: Iterable[str]) -> bool:
    return any(path == prefix or path.startswith(prefix) for prefix in prefixes)


def is_documentation(path: str) -> bool:
    return (
        path.startswith("docs/")
        or path.endswith(".md")
        or path in {"LICENSE", "PATENTS"}
    )


def is_cargo_metadata(path: str) -> bool:
    return (
        path in {"Cargo.toml", "Cargo.lock", "deny.toml"}
        or path.endswith("/Cargo.toml")
    )


def classify(paths: Iterable[str]) -> dict[str, bool]:
    normalized = [path.strip().replace("\\", "/") for path in paths if path.strip()]
    if not normalized:
        raise ValueError("at least one changed path is required")

    result = {name: False for name in CATEGORY_NAMES}
    result["docs_only"] = all(is_documentation(path) for path in normalized)

    for path in normalized:
        workflow = starts_with_any(path, WORKFLOW_PATHS)
        cargo_metadata = is_cargo_metadata(path)
        rust = cargo_metadata or path.startswith("crates/") or path.endswith(".rs")
        channels = path.startswith("packages/channels/")
        openclaw = path.startswith("packages/openclaw-coven/")
        afs = cargo_metadata or starts_with_any(
            path,
            (
                "crates/coven-afs/",
                "crates/coven-cli/src/afs_mount.rs",
                "scripts/afs-mount-smoke.sh",
            ),
        )
        npm_packaging = (
            cargo_metadata
            or path.startswith("npm/")
            or path.startswith("crates/coven-cli/")
            or path in NPM_PACKAGING_SCRIPTS
        )
        engine = starts_with_any(path, ENGINE_PATHS)

        result["workflow"] |= workflow
        result["cargo_metadata"] |= cargo_metadata
        result["rust"] |= rust
        result["afs"] |= afs
        result["channels"] |= channels
        result["openclaw"] |= openclaw
        result["npm_packaging"] |= npm_packaging
        result["engine"] |= engine

        if not is_documentation(path) and not any(
            (workflow, rust, channels, openclaw, npm_packaging, engine)
        ):
            result["rust"] = True

    if result["workflow"]:
        for name in CATEGORY_NAMES:
            result[name] = name != "docs_only"
    elif any(value for name, value in result.items() if name != "docs_only"):
        result["docs_only"] = False

    return result


def write_github_output(result: dict[str, bool], output: pathlib.Path) -> None:
    with output.open("a", encoding="utf-8") as stream:
        for name in CATEGORY_NAMES:
            stream.write(f"{name}={str(result[name]).lower()}\n")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--github-output", type=pathlib.Path)
    args = parser.parse_args()
    result = classify(sys.stdin.read().splitlines())
    if args.github_output is not None:
        write_github_output(result, args.github_output)
    else:
        for name in CATEGORY_NAMES:
            print(f"{name}={str(result[name]).lower()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
```

- [ ] **Step 4: Run the classifier tests**

Run:

```bash
python3 scripts/classify-ci-changes-test.py
```

Expected: 13 tests pass.

- [ ] **Step 5: Smoke-test representative path sets**

Run:

```bash
printf '%s\n' docs/guide.md | python3 scripts/classify-ci-changes.py
printf '%s\n' crates/coven-cli/src/daemon.rs | python3 scripts/classify-ci-changes.py
printf '%s\n' .github/workflows/ci.yml | python3 scripts/classify-ci-changes.py
```

Expected:

- docs output has only `docs_only=true`;
- CLI output has `rust=true` and `npm_packaging=true`;
- workflow output has every category except `docs_only` set to `true`.

- [ ] **Step 6: Commit**

```bash
git add scripts/classify-ci-changes.py scripts/classify-ci-changes-test.py
git commit -s -m "ci: classify pull request changes" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 2: Add pinned workflow validation

**Files:**
- Create: `scripts/check-workflows-test.py`
- Create: `scripts/check-workflows.sh`

- [ ] **Step 1: Write the failing pin/interface test**

Create `scripts/check-workflows-test.py`:

```python
#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import subprocess
import unittest

SCRIPT = pathlib.Path(__file__).with_name("check-workflows.sh")


class WorkflowCheckerTests(unittest.TestCase):
    def test_actionlint_release_is_pinned_with_platform_checksums(self) -> None:
        text = SCRIPT.read_text(encoding="utf-8")
        self.assertIn('ACTIONLINT_VERSION="1.7.12"', text)
        self.assertIn(
            "8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8",
            text,
        )
        self.assertIn(
            "aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f",
            text,
        )
        self.assertIn(
            "5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644",
            text,
        )

    def test_print_version_does_not_download(self) -> None:
        output = subprocess.run(
            ["bash", str(SCRIPT), "--print-version"],
            check=True,
            capture_output=True,
            text=True,
        )
        self.assertEqual(output.stdout, "1.7.12\n")


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
python3 scripts/check-workflows-test.py
```

Expected: FAIL because `scripts/check-workflows.sh` does not exist.

- [ ] **Step 3: Implement the checksum-verified actionlint runner**

Create `scripts/check-workflows.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ACTIONLINT_VERSION="1.7.12"

if [[ "${1:-}" == "--print-version" ]]; then
  printf '%s\n' "$ACTIONLINT_VERSION"
  exit 0
fi

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)
    platform="linux_amd64"
    checksum="8aca8db96f1b94770f1b0d72b6dddcb1ebb8123cb3712530b08cc387b349a3d8"
    ;;
  Darwin-arm64)
    platform="darwin_arm64"
    checksum="aba9ced2dee8d27fecca3dc7feb1a7f9a52caefa1eb46f3271ea66b6e0e6953f"
    ;;
  Darwin-x86_64)
    platform="darwin_amd64"
    checksum="5b44c3bc2255115c9b69e30efc0fecdf498fdb63c5d58e17084fd5f16324c644"
    ;;
  *)
    echo "unsupported actionlint host: $(uname -s)-$(uname -m)" >&2
    exit 1
    ;;
esac

temporary="$(mktemp -d)"
trap 'rm -rf "$temporary"' EXIT
archive="actionlint_${ACTIONLINT_VERSION}_${platform}.tar.gz"
curl --fail --location --silent --show-error \
  "https://github.com/rhysd/actionlint/releases/download/v${ACTIONLINT_VERSION}/${archive}" \
  --output "${temporary}/${archive}"
printf '%s  %s\n' "$checksum" "${temporary}/${archive}" | shasum -a 256 --check -
tar -xzf "${temporary}/${archive}" -C "$temporary" actionlint
"${temporary}/actionlint" -color
```

Make it executable:

```bash
chmod +x scripts/check-workflows.sh
```

- [ ] **Step 4: Run tests and actionlint**

Run:

```bash
python3 scripts/check-workflows-test.py
scripts/check-workflows.sh
```

Expected: unit tests pass and current workflows have no actionlint errors.

- [ ] **Step 5: Commit**

```bash
git add scripts/check-workflows.sh scripts/check-workflows-test.py
git commit -s -m "ci: add pinned workflow validation" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 3: Define workflow policy tests

**Files:**
- Create: `scripts/check-ci-workflow-test.py`

- [ ] **Step 1: Write policy tests before changing workflows**

Create `scripts/check-ci-workflow-test.py`:

```python
#!/usr/bin/env python3
from __future__ import annotations

import pathlib
import unittest

ROOT = pathlib.Path(__file__).parents[1]
CI = (ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
RELEASE = (ROOT / ".github/workflows/release-npm.yml").read_text(
    encoding="utf-8"
)
CACHE_SHA = "55cc8345863c7cc4c66a329aec7e433d2d1c52a9"


class CiWorkflowPolicyTests(unittest.TestCase):
    def test_pr_runs_cancel_when_superseded(self) -> None:
        self.assertIn(
            "cancel-in-progress: ${{ github.event_name == 'pull_request' }}",
            CI,
        )

    def test_router_and_stable_gate_exist(self) -> None:
        self.assertIn("\n  changes:\n", CI)
        self.assertIn("\n  pr-gate:\n", CI)
        self.assertIn("name: PR gate", CI)
        self.assertIn("if: ${{ always() && github.event_name == 'pull_request' }}", CI)

    def test_rust_work_is_split_into_parallel_jobs(self) -> None:
        for job in (
            "rust-lint-linux",
            "rust-test-linux",
            "rust-test-windows",
            "afs-mount-linux",
            "afs-mount-macos",
        ):
            self.assertIn(f"\n  {job}:\n", CI)
        self.assertNotIn("\n  rust:\n", CI)

    def test_compiled_jobs_have_bounded_timeouts(self) -> None:
        self.assertGreaterEqual(CI.count("timeout-minutes: 20"), 10)

    def test_cache_action_is_commit_pinned(self) -> None:
        self.assertIn(f"actions/cache@{CACHE_SHA}", CI)
        self.assertNotIn("actions/cache@v", CI)

    def test_docs_only_path_keeps_policy_checks(self) -> None:
        self.assertIn("python3 scripts/classify-ci-changes-test.py", CI)
        self.assertIn("python3 scripts/check-workflows-test.py", CI)
        self.assertIn("scripts/check-workflows.sh", CI)
        self.assertIn("needs.changes.outputs.docs_only != 'true'", CI)

    def test_pr_packaging_is_linux_only(self) -> None:
        self.assertIn("\n  npm-onboarding-linux:\n", CI)
        self.assertIn("\n  npm-onboarding-main:\n", CI)
        self.assertIn("github.event_name == 'push'", CI)

    def test_deferred_jobs_run_on_main(self) -> None:
        self.assertIn("\n  performance-baseline:\n", CI)
        self.assertIn("name: CLI performance baseline", CI)
        self.assertIn("if: ${{ github.event_name == 'push' }}", CI)

    def test_release_requires_performance_before_publish(self) -> None:
        self.assertIn("\n  performance-baseline:\n", RELEASE)
        self.assertIn(
            "needs: [build-platform, npm-dry-run, performance-baseline, verify-tag]",
            RELEASE,
        )


if __name__ == "__main__":
    unittest.main()
```

- [ ] **Step 2: Run the tests to verify they fail**

Run:

```bash
python3 scripts/check-ci-workflow-test.py
```

Expected: multiple failures because the current workflows still use the old
matrix and run extended jobs on every PR.

- [ ] **Step 3: Commit the failing policy test**

```bash
git add scripts/check-ci-workflow-test.py
git commit -s -m "test(ci): define routed workflow policy" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 4: Rewrite pull-request and main CI routing

**Files:**
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add workflow concurrency and the router**

At workflow level, add:

```yaml
concurrency:
  group: ci-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

Add `jobs.changes` with outputs for every classifier category:

```yaml
  changes:
    name: Classify changes
    runs-on: ubuntu-latest
    timeout-minutes: 5
    outputs:
      docs_only: ${{ steps.classify.outputs.docs_only }}
      rust: ${{ steps.classify.outputs.rust }}
      afs: ${{ steps.classify.outputs.afs }}
      channels: ${{ steps.classify.outputs.channels }}
      openclaw: ${{ steps.classify.outputs.openclaw }}
      npm_packaging: ${{ steps.classify.outputs.npm_packaging }}
      engine: ${{ steps.classify.outputs.engine }}
      workflow: ${{ steps.classify.outputs.workflow }}
      cargo_metadata: ${{ steps.classify.outputs.cargo_metadata }}
    steps:
      - uses: actions/checkout@v7.0.1
        with:
          fetch-depth: 0
      - name: Collect changed paths
        shell: bash
        env:
          EVENT_NAME: ${{ github.event_name }}
          PR_BASE_SHA: ${{ github.event.pull_request.base.sha }}
          PR_HEAD_SHA: ${{ github.event.pull_request.head.sha }}
          BEFORE_SHA: ${{ github.event.before }}
          AFTER_SHA: ${{ github.sha }}
        run: |
          set -euo pipefail
          if [[ "$EVENT_NAME" == "pull_request" ]]; then
            range="${PR_BASE_SHA}...${PR_HEAD_SHA}"
          elif git cat-file -e "${BEFORE_SHA}^{commit}" 2>/dev/null; then
            range="${BEFORE_SHA}..${AFTER_SHA}"
          else
            empty_tree="$(git hash-object -t tree -w --stdin </dev/null)"
            range="${empty_tree}..${AFTER_SHA}"
          fi
          git diff --name-only --diff-filter=ACMRD "$range" > changed-files.txt
      - name: Classify changed paths
        id: classify
        run: python3 scripts/classify-ci-changes.py --github-output "$GITHUB_OUTPUT" < changed-files.txt
```

- [ ] **Step 2: Rename and extend the policy guard**

Replace `secret-guard` with `policy-guard`, add `needs: changes` and
`timeout-minutes: 10`, preserve every existing secret/privacy/API-contract
command, and append:

```yaml
      - run: python3 scripts/classify-ci-changes-test.py
      - run: python3 scripts/check-workflows-test.py
      - run: python3 scripts/check-ci-workflow-test.py
      - run: scripts/check-workflows.sh
```

- [ ] **Step 3: Add the pinned Cargo cache block to every Rust build job**

Use this exact action pin. The example below is the lint cache:

```yaml
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-rust-lint-${{ hashFiles('Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-rust-lint-
```

Use these exact key prefixes in the corresponding jobs:

| Job | Cache key prefix |
| --- | --- |
| `rust-lint-linux` | `${{ runner.os }}-rust-lint-` |
| `rust-test-linux`, `rust-test-windows` | `${{ runner.os }}-rust-test-` |
| `afs-mount-linux`, `afs-mount-macos` | `${{ runner.os }}-rust-afs-` |
| `npm-onboarding-linux`, `npm-onboarding-main` | `${{ runner.os }}-rust-npm-` |
| `engine-contract` | `${{ runner.os }}-rust-engine-` |
| `performance-baseline` | `${{ runner.os }}-rust-perf-` |

- [ ] **Step 4: Replace the Rust matrix with parallel jobs**

Create these jobs, each with `needs: changes` and `timeout-minutes: 20`:

```yaml
  rust-lint-linux:
    name: Rust lint (Linux)
    if: ${{ needs.changes.outputs.rust == 'true' }}
    runs-on: ubuntu-latest
```

Steps: checkout, stable toolchain with `rustfmt, clippy`, pinned lint cache,
OpenBLAS install, `cargo fmt --check`, and
`cargo clippy --workspace --all-targets -- -D warnings`.

```yaml
  rust-test-linux:
    name: Rust tests (Linux)
    if: ${{ needs.changes.outputs.rust == 'true' }}
    runs-on: ubuntu-latest
```

Steps: checkout, stable toolchain, pinned test cache, OpenBLAS install, and
`cargo test --workspace --locked`.

```yaml
  rust-test-windows:
    name: Rust tests (Windows)
    if: ${{ needs.changes.outputs.rust == 'true' }}
    runs-on: windows-latest
```

Steps: checkout, stable toolchain, pinned test cache, and
`cargo test --workspace --locked`.

```yaml
  afs-mount-linux:
    name: AFS mount backend (Linux)
    if: ${{ needs.changes.outputs.afs == 'true' }}
    runs-on: ubuntu-latest
```

Steps: checkout, stable toolchain with clippy, pinned AFS cache, OpenBLAS
install, mount-feature clippy, and mount-feature tests.

```yaml
  afs-mount-macos:
    name: AFS mount backend (macOS)
    if: ${{ github.event_name == 'push' || needs.changes.outputs.afs == 'true' }}
    runs-on: macos-26
```

Steps: preserve current macOS mount clippy/tests. Run
`./scripts/afs-mount-smoke.sh` only when
`${{ github.event_name == 'push' }}`, with `continue-on-error: true`.

- [ ] **Step 5: Route package, packaging, engine, and audit jobs**

Keep the current job command bodies, add `needs: changes`,
`timeout-minutes: 20`, and these conditions:

```yaml
channels:
  if: ${{ needs.changes.outputs.channels == 'true' }}

openclaw-bridge:
  if: ${{ needs.changes.outputs.openclaw == 'true' }}

npm-onboarding-linux:
  if: ${{ github.event_name == 'pull_request' && needs.changes.outputs.npm_packaging == 'true' }}

npm-onboarding-main:
  if: ${{ github.event_name == 'push' }}

engine-contract:
  if: ${{ github.event_name == 'push' || needs.changes.outputs.engine == 'true' }}

cargo-deny:
  if: ${{ github.event_name == 'push' || needs.changes.outputs.docs_only != 'true' }}
```

`npm-onboarding-linux` uses only:

```yaml
    runs-on: ubuntu-latest
    env:
      NPM_TARGET: linux-x64
      RUST_TARGET: x86_64-unknown-linux-gnu
```

Build `coven-cli` in release mode and run:

```bash
node scripts/test-cli-prepublish.mjs --target="$NPM_TARGET" --skip-build --skip-secrets-scan
```

`npm-onboarding-main` retains the current four-platform matrix and macOS AFS
helper build unchanged.

For OpenClaw, after activating pnpm, add:

```yaml
      - id: pnpm-store
        run: echo "path=$(pnpm store path --silent)" >> "$GITHUB_OUTPUT"
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: ${{ steps.pnpm-store.outputs.path }}
          key: ${{ runner.os }}-pnpm-openclaw-${{ hashFiles('packages/openclaw-coven/pnpm-lock.yaml') }}
          restore-keys: |
            ${{ runner.os }}-pnpm-openclaw-
```

- [ ] **Step 6: Move performance collection to `main`**

Keep the current performance job body, add `needs: changes`,
`timeout-minutes: 20`, a pinned `perf` Cargo cache, and:

```yaml
    if: ${{ github.event_name == 'push' }}
```

- [ ] **Step 7: Add the stable PR gate**

Add:

```yaml
  pr-gate:
    name: PR gate
    if: ${{ always() && github.event_name == 'pull_request' }}
    runs-on: ubuntu-latest
    timeout-minutes: 5
    needs:
      - changes
      - policy-guard
      - rust-lint-linux
      - rust-test-linux
      - rust-test-windows
      - afs-mount-linux
      - afs-mount-macos
      - performance-baseline
      - channels
      - openclaw-bridge
      - npm-onboarding-linux
      - npm-onboarding-main
      - engine-contract
      - cargo-deny
    env:
      JOB_RESULTS: ${{ toJSON(needs) }}
    steps:
      - name: Require successful or intentional skipped jobs
        run: |
          python3 - <<'PY'
          import json
          import os

          jobs = json.loads(os.environ["JOB_RESULTS"])
          bad = {
              name: data["result"]
              for name, data in jobs.items()
              if data["result"] not in {"success", "skipped"}
          }
          for required in ("changes", "policy-guard"):
              if jobs[required]["result"] != "success":
                  bad[required] = jobs[required]["result"]
          if bad:
              raise SystemExit(
                  "PR gate rejected job results: "
                  + ", ".join(f"{name}={result}" for name, result in sorted(bad.items()))
              )
          print("PR gate accepted all required and conditional jobs")
          PY
```

- [ ] **Step 8: Run workflow policy and syntax tests**

Run:

```bash
python3 scripts/check-ci-workflow-test.py
python3 scripts/check-coven-privacy-test.py
python3 scripts/check-secrets-test.py
scripts/check-workflows.sh
```

Expected: all tests pass and actionlint reports no errors.

- [ ] **Step 9: Commit the routed CI graph**

```bash
git add .github/workflows/ci.yml scripts/check-ci-workflow-test.py
git commit -s -m "ci: route and parallelize pull request checks" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 5: Add deferred release performance coverage

**Files:**
- Modify: `.github/workflows/release-npm.yml`
- Test: `scripts/check-ci-workflow-test.py`

- [ ] **Step 1: Add the release performance job**

Add a `performance-baseline` job after `verify-tag`:

```yaml
  performance-baseline:
    name: Release performance baseline
    runs-on: ubuntu-latest
    timeout-minutes: 20
    needs: [verify, verify-tag]
    permissions:
      contents: read
    steps:
      - uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v6.0.0
      - uses: actions/setup-node@820762786026740c76f36085b0efc47a31fe5020 # v6.0.0
        with:
          node-version: 24
      - uses: dtolnay/rust-toolchain@29eef336d9b2848a0b548edc03f92a220660cdb8 # stable @ 2026-05-20
      - uses: actions/cache@55cc8345863c7cc4c66a329aec7e433d2d1c52a9 # v6.1.0
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            target
          key: ${{ runner.os }}-rust-release-perf-${{ hashFiles('Cargo.lock') }}
          restore-keys: |
            ${{ runner.os }}-rust-release-perf-
      - name: Install native link dependencies
        run: sudo apt-get update && sudo apt-get install -y --no-install-recommends libopenblas-dev
      - name: Build coven
        run: cargo build -p coven-cli --locked
      - name: Validate benchmark runner
        run: node --test scripts/benchmark-cli.test.mjs scripts/benchmark-chaos.test.mjs
      - name: Collect release baseline artifact
        run: node scripts/benchmark-cli.mjs --binary target/debug/coven --iterations 3 --output coven-perf.json
      - name: Collect concurrent runtime baseline artifact
        continue-on-error: true
        run: node scripts/benchmark-chaos.mjs --binary target/debug/coven --output coven-chaos.json
      - name: Collect deterministic TUI scheduling metric
        run: |
          set -o pipefail
          cargo test -p coven-cli --bin coven tui::chat::events::tests::benchmark_schedule_metrics_emit_json --locked -- --ignored --nocapture | tee coven-tui-metrics.txt
      - uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a # v7.0.0
        with:
          name: coven-release-performance-${{ needs.verify-tag.outputs.release_tag }}
          path: |
            coven-perf.json
            coven-chaos.json
            coven-tui-metrics.txt
          if-no-files-found: error
```

- [ ] **Step 2: Gate publication on the performance job**

Change `npm-publish.needs` to:

```yaml
    needs: [build-platform, npm-dry-run, performance-baseline, verify-tag]
```

Do not add PR/main artifacts as release inputs.

- [ ] **Step 3: Run release workflow policy and syntax tests**

Run:

```bash
python3 scripts/check-ci-workflow-test.py
scripts/check-workflows.sh
```

Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/release-npm.yml scripts/check-ci-workflow-test.py
git commit -s -m "ci: defer performance validation to main and release" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 6: Document contributor expectations

**Files:**
- Modify: `CONTRIBUTING.md`

- [ ] **Step 1: Add the routed-CI note**

After the Pull Request Workflow local-check block, add:

```markdown
CI routes platform, package, AFS, engine, and packaging jobs from the changed
paths so unrelated pull requests receive faster feedback. This routing does not
reduce the contributor-side bar: run every local command relevant to the files
you changed. Workflow/classifier changes intentionally exercise the complete CI
graph, while performance baselines and the full native packaging matrix run on
`main` and release tags.
```

- [ ] **Step 2: Check the documentation diff**

Run:

```bash
git diff --check
```

Expected: no whitespace errors.

- [ ] **Step 3: Commit**

```bash
git add CONTRIBUTING.md
git commit -s -m "docs: explain routed CI coverage" \
  -m "Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>"
```

### Task 7: Run repository validation

**Files:**
- No new files.

- [ ] **Step 1: Run all Python policy tests**

```bash
python3 scripts/classify-ci-changes-test.py
python3 scripts/check-workflows-test.py
python3 scripts/check-ci-workflow-test.py
python3 scripts/check-coven-privacy-test.py
python3 scripts/check-secrets-test.py
python3 scripts/check-api-contract-docs-test.py
```

Expected: all tests pass.

- [ ] **Step 2: Validate workflows**

```bash
scripts/check-workflows.sh
```

Expected: actionlint 1.7.12 exits successfully.

- [ ] **Step 3: Run Rust gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked -- --test-threads=1
cargo clippy -p coven-afs --features mount --all-targets -- -D warnings
cargo test -p coven-afs --features mount --locked
```

Expected: all deterministic gates pass. If an unchanged PTY timing test fails,
rerun that exact test serially and record the pre-existing flake rather than
changing unrelated runtime code.

- [ ] **Step 4: Run security/privacy guards**

```bash
python3 scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
git diff --check
```

Expected: all pass.

- [ ] **Step 5: Verify real diff classification**

```bash
git diff --name-only origin/main...HEAD | python3 scripts/classify-ci-changes.py
```

Expected for this CI/workflow branch: `workflow=true` and every category except
`docs_only` is `true`.

### Task 8: Publish and observe the rollout PR

**Files:**
- No new files.

- [ ] **Step 1: Confirm claim and clean state**

```bash
coven claim heartbeat issue-771
git status --short --branch
git log --oneline --decorate origin/main..HEAD
```

Expected: only intended commits and a clean worktree.

- [ ] **Step 2: Push the workflow implementation before the final docs commit**

```bash
rollout_head="$(git rev-parse HEAD^)"
git push -u origin "${rollout_head}:refs/heads/chore/771-ci-pr-speed"
```

Expected: the remote branch contains the complete workflow implementation and
tests but not the final `CONTRIBUTING.md` commit.

- [ ] **Step 3: Open the rollout PR**

Create `/tmp/coven-771-pr-body.md`:

```markdown
Closes #771

## Summary

- classify changed paths with repository-owned tested logic
- run Rust, AFS, package, packaging, and engine checks only when relevant
- split Linux lint/tests, Windows tests, and AFS coverage into parallel jobs
- defer performance and the full native packaging matrix to main/release
- add bounded timeouts, stale-run cancellation, caches, and a stable PR gate

## Baseline

Recent PRs spent about 8 minutes on non-gating performance collection, 3-9
minutes per native packaging platform, and up to six hours on stuck Rust jobs.

## Validation

- Python classifier/workflow/policy tests
- actionlint 1.7.12
- Rust fmt, clippy, workspace tests, and AFS feature tests
- secret and privacy guards

## Rollout

This workflow-changing PR intentionally fans out every conditional job. After
merge, confirm deferred jobs on main, then require `PR gate` in branch
protection.
```

Open the PR:

```bash
gh pr create \
  --base main \
  --head chore/771-ci-pr-speed \
  --title "ci: route and parallelize pull request checks" \
  --body-file /tmp/coven-771-pr-body.md
```

- [ ] **Step 4: Trigger and verify stale-run cancellation with the final docs commit**

Wait until the first PR workflow reports at least one in-progress job:

```bash
pr_number="$(gh pr view --json number --jq .number)"
gh pr checks "$pr_number"
```

Then push the already-created final documentation commit as a normal
fast-forward:

```bash
git push origin HEAD:refs/heads/chore/771-ci-pr-speed
```

Expected: the first PR workflow is cancelled and a replacement workflow starts.
No empty or temporary commit is introduced.

- [ ] **Step 5: Inspect job selection**

Run:

```bash
pr_number="$(gh pr view --json number --jq .number)"
gh pr checks "$pr_number" --watch --interval 20
```

Expected for a workflow-changing PR:

- every conditional job executes because `workflow=true`;
- `PR gate` succeeds only after every child succeeds or intentionally skips;
- performance and full native packaging do not run on the PR;
- required PR checks finish near 10 minutes when runners are available.

- [ ] **Step 6: Merge only after the routed graph is green**

Use the repository's normal squash/merge policy. Confirm the `main` push runs
performance, full native onboarding, the engine contract, and the macOS mount
probe.

### Task 9: Enable branch protection after rollout

**Files:**
- Repository setting only.

- [ ] **Step 1: Confirm the merged `PR gate` context exists**

```bash
gh api repos/OpenCoven/coven/commits/main/check-runs \
  --jq '.check_runs[] | select(.name == "PR gate") | {name,conclusion,html_url}'
```

Expected: at least one successful `PR gate`.

- [ ] **Step 2: Enable minimal branch protection requiring the gate**

After explicit maintainer confirmation, run:

```bash
gh api --method PUT repos/OpenCoven/coven/branches/main/protection \
  --input - <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": ["PR gate"]
  },
  "enforce_admins": false,
  "required_pull_request_reviews": null,
  "restrictions": null,
  "required_conversation_resolution": true,
  "allow_force_pushes": false,
  "allow_deletions": false
}
JSON
```

Expected: HTTP 200 with `PR gate` listed under required status checks.

- [ ] **Step 3: Verify branch protection**

```bash
gh api repos/OpenCoven/coven/branches/main/protection \
  --jq '{strict:.required_status_checks.strict,contexts:.required_status_checks.contexts,conversation_resolution:.required_conversation_resolution.enabled}'
```

Expected:

```json
{"strict":true,"contexts":["PR gate"],"conversation_resolution":true}
```

- [ ] **Step 4: Release the claim and clean the worktree**

```bash
coven claim release issue-771
```

After merge, delete the remote branch and remove
`/tmp/coven-771-ci-pr-speed` from the primary checkout.
