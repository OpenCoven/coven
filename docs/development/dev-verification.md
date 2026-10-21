---
title: "Local verification contract"
summary: "Repository-owned verification tiers, failure semantics, and CI coverage."
source_adjacent_reason: "Defines scripts/dev-verify.sh behavior and its mapping to the checked-in CI workflow."
---

# Local verification contract

Run from any directory; the script resolves its own worktree:

```bash
bash scripts/dev-verify.sh --quick
bash scripts/dev-verify.sh --pr --range origin/main...HEAD
bash scripts/dev-verify.sh --full
bash scripts/dev-verify.sh --pr --plan
```

Select exactly one tier. `--plan` prints command arguments, working directories,
prerequisite names, and skips; it does not execute checks or report a pass.
The default committed range is `origin/main...HEAD`. Fetch the appropriate base
yourself before running; this tool never fetches or rewrites the checkout.
Both range endpoints are resolved to commit SHAs at startup and printed. The
same immutable range drives routing and later guards even if another worktree
fetches a new base. Do not edit the checkout during execution: working-file,
compiler and package checks inspect live contents, not an atomic snapshot.
An invalid or unavailable range fails before check execution. In shallow
checkouts, fetch the base/history rather than substituting a narrower range.

| Check | Quick | PR | Full |
| --- | --- | --- | --- |
| Rust compiler and Cargo match `rust-toolchain.toml` | Yes | Yes | Yes |
| Formatting and `cargo check --workspace --locked` | Yes | Yes | Yes |
| Secret scan of tracked working files and history | Yes | Yes | Yes |
| Privacy: committed range, staged index, changed working files | Yes | Yes | Yes |
| Documentation ownership of changed working files | Yes | Yes | Yes |
| Node matches `.node-version` | No | Yes | Yes |
| Documentation ownership of committed range | No | Yes | Yes |
| Clippy with `-D warnings`; locked workspace tests | No | Yes | Yes |
| Entire Policy guard command inventory and drift check | No | Yes | Yes |
| `cargo deny --all-features check advisories licenses bans sources` | No | Yes | Yes |
| Channels and OpenClaw install/build/test | No | CI path classification | Both |
| Optional browser readiness (no download) | No | Declared packages only | Declared packages only |
| Exact-checkout native Automations audit | No | CI-only | Before other checks |
| Threads clock and daemon feature suites | No | CI-only | Yes |
| AFS mount feature lint/tests | No | CI-only | Linux/macOS |
| Restricted-runtime lint/tests | No | CI-only | macOS |

The PR tier deliberately runs the core Rust and policy checks even for paths
whose CI routing is narrower. Package routing reuses
`scripts/classify-ci-changes.py` over the union of committed-range, staged,
unstaged, and untracked non-ignored paths. Package installs use `npm ci` for
Channels and `pnpm install --frozen-lockfile --ignore-scripts` for OpenClaw.
The latter requires the exact `packageManager` pin in its package manifest.
There is no root npm package. Build and test commands run inside their package.

The existing secret scanner inspects tracked working contents and committed
history; it does not independently scan index contents or untracked files.
Stage new files and commit the intended changes, then verify the exact committed
checkout before publication. Working-file privacy coverage does not substitute
for this secret-scan boundary. CI's cargo-deny action defaults to `--all-features`;
the local audit passes that flag explicitly.

## Prerequisites and effects

Provision tools and native link libraries before verification. The runner
does not install system packages, elevate privileges, activate a global package
manager, or change toolchain pins. A tool with the wrong version fails its pin
check even when it was installed outside rustup. Missing `cargo-deny`, pnpm,
compilers, libraries, or network access are real failures, not successful skips.
CI provisions these separately; use its reviewed setup and repository bootstrap
contract when available.

Package installs and tests can create normal build outputs. The actionlint
guard downloads its existing checksum-verified pinned artifact; dependency
audit and package installs may require network access. Verification makes no
claim of offline operation. Keep build caches outside the checkout when using
the exact-checkout native audit, for example:

```bash
CARGO_TARGET_DIR=/tmp/coven-task-target bash scripts/dev-verify.sh --full
```

The native audit requires a clean committed checkout, including ignored files,
and retains evidence in a newly allocated external temporary directory printed
before execution. It runs before package installs and other tests. Clean up
your task's artifacts after inspecting them; the runner retains audit evidence.

## Output and exit contract

Each executed check prints `RUN` with shell-quoted arguments, streams its child
stdout/stderr, then prints `PASS` or `FAIL` with the child status. Arguments are
passed directly without shell evaluation. Independent checks continue after a
failure. Dependent checks print `SKIP` naming the failed prerequisite; that skip
does not turn the run green. Planned commands print `PLAN`, never `PASS`.

Exit 0 means all selected local checks passed, or a requested plan was printed.
A real run returns its first failing child status; child signals use `128 +
signal`. Usage, invalid inventory, and pin validation errors return 2; a missing
executable returns 127 and another execution error returns 126. Interruption
returns 130. The summary explicitly retains unverified CI-only checks.

Missing applicable tools never become not-applicable. Non-selected packages,
out-of-tier checks, and unsupported host feature suites have explicit skip
reasons. PR/full runs `node scripts/browser-capability.mjs --check` after
package checks. With no declared browser tests this reports `not-applicable`;
otherwise a lock-matched installed package and a working browser are required.
The runner never downloads browsers. Use the optional `--install` command during
setup and keep `PLAYWRIGHT_BROWSERS_PATH` consistent; see
[optional browser verification](../../scripts/browser/README.md). The smoke
checks one selected browser, not the package's whole browser test suite.
Policy guard installs the locked parser tools and runs browser contract tests;
it does not download or launch browser artifacts.

The fresh Debian/Ubuntu bootstrap matrix is a routed required CI job; run its
explicit [safe container runner](../../scripts/bootstrap-contract/README.md)
locally when Docker is available. PR/full includes its host contract tests and
the A2 Cloud fixture suites, not actual container execution. These fixture
suites require maintained Python 3.11+ as documented by the Cloud setup scripts.

## CI remains final merge authority

The following are explicitly reported as CI-only; local tier success does not
prove their results:

- Other OS runners: Linux/Windows workspace and Threads jobs, macOS feature
  jobs, and the post-merge macOS workspace job. Full runs only its host suites.
- Required exact-commit Automations protocol and authority bundle generation,
  digest verification, and artifact upload. Their unit tests are in Policy guard;
  unit tests do not prove artifact delivery.
- Release-target npm onboarding matrix and the pinned engine contract job.
- Informational/post-merge performance collection and the real mount probe.
- Fresh Debian/Ubuntu bootstrap execution on routed setup, dependency and workflow changes.
- GitHub's required `PR gate`, merge-queue evaluation, and protected-main review.

Record exact failed commands and environment differences in the issue/PR.
Do not equate a macOS run with Linux or Windows CI, container results with a
managed Cloud VM, or a plan/fixture run with actual Rust/package execution.

## Keeping the inventory current

`scripts/dev-verify-policy.json` explicitly lists the Policy guard's single-line
commands. The runner separately supplies the committed-range docs/privacy
commands and working/index checks. Its source digest covers the entire policy
job, including multiline range handling. A changed command list or policy job
fails closed before PR/full execution. Review every CI change, update the mapped
commands and this table, then update the digest only after accounting for every
step. Never update the digest alone to dismiss a new required guard.

CI runs `python3 scripts/dev-verify-test.py` in Policy guard. Its subprocess and
disposable-repository tests cover orchestration, failure propagation, argument
handling, routing, range validation, and inventory drift. They do not substitute
for real tier runs. Preserve canonical standalone commands and CI jobs rather
than replacing CI with this wrapper.
