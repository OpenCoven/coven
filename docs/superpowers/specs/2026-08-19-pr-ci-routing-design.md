# Pull Request CI Routing and Parallelization Design

Issue: [#771](https://github.com/OpenCoven/coven/issues/771)

## Goal

Reduce required pull-request feedback time toward 10 minutes while preserving
the Rust authority boundary, Windows coverage, security checks, and independent
release verification.

Recent PR runs show the main avoidable costs:

- CLI performance collection: about 8 minutes, despite being non-gating.
- Native npm onboarding: about 3-9 minutes per platform across four platforms.
- Engine contract: about 2 minutes on every change.
- Linux Rust checks: formatting, clippy, workspace tests, and AFS feature checks
  run serially in one job.
- Superseded or stuck Rust jobs can consume a runner for the six-hour default.

## Success Criteria

- Ordinary Rust PRs complete required checks in about 10 minutes when runners
  are available.
- Docs-only PRs run policy, secret, privacy, API-contract, and workflow checks
  without compiling Rust or packages.
- Relevant Rust PRs retain Linux and Windows workspace-test coverage.
- Package, AFS, engine, and packaging checks run on PRs only when their owned
  surfaces change.
- Performance collection and the full native packaging matrix run on `main`
  and release tags, not on every PR.
- A stable `PR gate` check summarizes conditional jobs and can be required by
  branch protection.
- Release tags continue to verify source independently instead of trusting
  artifacts or results from an earlier workflow.

## Non-Goals

- Do not weaken secret, privacy, API-contract, DCO, or dependency policies.
- Do not remove Windows workspace tests from relevant Rust PRs.
- Do not introduce a third-party path-filter action.
- Do not reuse PR-built binaries for releases.
- Do not redesign the Rust test suite or adopt a new test runner in this change.

## Workflow Architecture

Keep `.github/workflows/ci.yml` as the single workflow for pull requests and
pushes to `main`. Add a small `changes` job at the front of the graph. It checks
out enough history to compute the event's exact diff and passes changed paths
to a repository-owned classifier.

The classifier lives in a focused script with unit tests. It emits booleans for:

- `docs_only`
- `rust`
- `afs`
- `channels`
- `openclaw`
- `npm_packaging`
- `engine`
- `workflow`
- `cargo_metadata`

Downstream jobs use these outputs in job-level `if` expressions. Workflow and
classifier changes fan out to every PR job so changes to CI validate the whole
graph.

Add workflow concurrency:

```yaml
concurrency:
  group: ci-${{ github.event.pull_request.number || github.ref }}
  cancel-in-progress: ${{ github.event_name == 'pull_request' }}
```

This cancels obsolete runs after a PR receives a newer commit while allowing
`main` runs to finish.

Every compiled or integration job gets `timeout-minutes: 20`. A test that hangs
must fail promptly instead of occupying a runner for six hours.

## Change Classification

The classifier accepts newline-delimited repository-relative paths and emits
GitHub output values. Classification is conservative:

| Category | Representative paths |
| --- | --- |
| `docs_only` | Markdown and documentation assets, excluding workflow/config files |
| `rust` | `Cargo.toml`, `Cargo.lock`, `crates/**`, Rust build/config scripts |
| `afs` | `crates/coven-afs/**`, AFS mount code/tests/scripts, Cargo metadata |
| `channels` | `packages/channels/**` and its shared package/build inputs |
| `openclaw` | `packages/openclaw-coven/**` and its shared package/build inputs |
| `npm_packaging` | `npm/**`, publish/prepublish scripts, CLI packaging metadata, Cargo metadata |
| `engine` | `engine.lock`, engine install/pin scripts, engine client code and contract tests |
| `workflow` | `.github/workflows/**` and the classifier/tests themselves |
| `cargo_metadata` | workspace/crate manifests, lockfile, deny configuration |

Unknown non-documentation paths set `rust=true`. Because `cargo-deny` runs on
every non-doc PR, both workspace and dependency coverage fail closed without
guessing that an unknown path belongs to a package-specific surface.

For pull requests, the diff is
`${{ github.event.pull_request.base.sha }}...${{ github.event.pull_request.head.sha }}`.
For `main` pushes, it is `${{ github.event.before }}..${{ github.sha }}`, with
the existing empty-tree fallback for unavailable history.

## Pull Request Job Graph

All eligible jobs start after `changes`; they do not depend on one another.

### Always on Pull Requests

- `changes`
- `policy-guard`
  - privacy-script tests
  - secret-script tests
  - API-contract documentation tests
  - API-contract documentation check
  - current-tree secret scan
  - PR-range privacy scan
  - classifier unit tests
  - workflow syntax validation

`cargo-deny` runs for every non-doc-only PR. It is fast and catches newly
published advisories even when the lockfile did not change.

### Rust-Affecting Pull Requests

Split the current serial Rust matrix into independent jobs:

- `rust-lint-linux`
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
- `rust-test-linux`
  - `cargo test --workspace --locked`
- `rust-test-windows`
  - `cargo test --workspace --locked`
- `afs-mount-linux`, when `afs` is true
  - mount-feature clippy
  - mount-feature tests
- `afs-mount-macos`, when `afs` is true
  - mount-feature clippy
  - mount-feature tests

Formatting, linting, Linux tests, Windows tests, and AFS feature coverage
therefore run in parallel instead of accumulating in one Linux job.

### Package-Affecting Pull Requests

- `channels` only when `channels` is true.
- `openclaw-bridge` only when `openclaw` is true.
- `npm-onboarding-linux` only when `npm_packaging` is true.

The PR npm smoke builds and packages only `linux-x64`. The complete macOS ARM,
macOS Intel, Linux, and Windows packaging matrix moves out of PR CI.

### Engine-Affecting Pull Requests

Run `engine-contract` only when `engine` is true. Changes to the pin, installer,
engine-facing CLI code, or contract tests still receive pre-merge coverage.

### Docs-Only Pull Requests

Run only `changes`, `policy-guard`, and `PR gate`. No Rust toolchain, native
link dependency, package installation, platform runner, or release build starts.

## Main and Release Coverage

Pushes to `main` run the changed-surface jobs plus deferred extended coverage:

- CLI performance, chaos, and deterministic TUI baseline collection.
- Full native npm onboarding matrix for macOS ARM, macOS Intel, Linux x64, and
  Windows.
- Informational real macOS AFS mount probe.
- Engine contract regardless of changed paths.

`.github/workflows/release-npm.yml` retains:

- full formatting, clippy, workspace tests, and secret verification;
- signed-tag and ancestry verification;
- independent release builds for every native package;
- npm dry-run assembly;
- OIDC-authenticated publication.

It also runs the deferred performance baseline before publication. Release jobs
build from the tag and do not consume artifacts from PR or `main` workflows.

## Stable Aggregate Gate

Add a final `PR gate` job:

- `if: always()`
- `needs` every conditional PR job
- accepts only `success` or intentional `skipped` results
- fails on `failure`, `timed_out`, or unexpected `cancelled`
- requires `changes` and `policy-guard` to succeed

This gives branch protection one stable context even though the set of executed
jobs varies by diff. After one observed rollout PR confirms the graph, enable
branch protection on `main` and require `PR gate`.

## Caching

Use commit-pinned `actions/cache` for:

- Cargo registry and Git checkout caches keyed by OS and `Cargo.lock`.
- Separate Cargo target caches per OS and job kind so clippy/test artifacts do
  not collide.
- pnpm store caching for OpenClaw keyed by its lockfile.

Cache misses always run the real commands. No gate may return success because a
cache or artifact is unavailable.

## Failure Handling

- PR concurrency cancels only superseded PR runs.
- Each compiled/integration job has a 20-minute timeout.
- Deferred main failures remain visible and release verification reruns the
  relevant gates independently.
- The classifier fails closed: malformed input or unknown output generation
  fails `changes`; unknown non-doc paths receive conservative Rust coverage.
- Conditional skips are explicit and visible through `PR gate`.
- Informational performance chaos and real mount probes retain
  `continue-on-error`; deterministic tests and packaging remain gating.

## Validation

Implementation validation must include:

1. Classifier unit fixtures for docs-only, Rust, Cargo metadata, AFS, Channels,
   OpenClaw, npm packaging, engine, workflow, unknown, and mixed changes.
2. Workflow syntax validation with a pinned actionlint version.
3. Local execution of existing secret/privacy/API-contract checks.
4. Representative classifier runs against real repository diffs.
5. A rollout PR that confirms:
   - expected jobs run for workflow changes;
   - `PR gate` reports the graph correctly;
   - superseded runs cancel;
   - required-check completion is at or near 10 minutes.
6. A follow-up docs-only PR or test branch proving compiled jobs skip.
7. A `main` run proving deferred performance, engine, mount probe, and full npm
   packaging coverage execute.
8. Branch protection updated to require `PR gate` only after the rollout run is
   successful.

## Rollout

1. Add and test the classifier.
2. Refactor `ci.yml` while preserving existing job commands.
3. Add the stable aggregate gate and concurrency/timeouts.
4. Move deferred checks to `main`/release triggers.
5. Open a PR and inspect actual job selection and duration.
6. Correct routing gaps before removing any transitional duplicate job.
7. Merge after the rollout graph is green.
8. Enable `main` branch protection requiring `PR gate`.
