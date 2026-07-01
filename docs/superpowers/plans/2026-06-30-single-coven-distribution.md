# Single Coven Distribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `coven` the single user-facing install and command surface for the daemon, CLI/TUI, and Coven Code workflows without erasing the daemon/client boundary.

**Architecture:** Keep the Rust Coven daemon as the authority layer for sessions, events, project-root validation, and socket API compatibility. Treat Coven Code as a managed harness adapter and workflow surface that can be launched, version-checked, and eventually bundled by `coven`; avoid merging repositories until the adapter contract is stable.

**Tech Stack:** Rust 2021, clap 4, serde/serde_json, existing `coven run` harness adapter surface, npm wrapper packages (`@opencoven/cli`, `coven-code`), Cave `/api/opencoven-tools/status` compatibility checks.

---

## Design Decisions

1. **User-facing command stays `coven`.** Users should not need to remember whether a workflow belongs to `@opencoven/cli` or `coven-code`.
2. **Coven Code becomes an adapter first.** `coven code` should launch the code workflow through an adapter boundary before any repo/package merge.
3. **The daemon remains the source of runtime truth.** Cave and other clients keep handshaking through `/api/v1/health` and capability discovery.
4. **Release compatibility is explicit.** Cave should warn when installed `coven` or `coven-code` versions are below the app's minimum compatible version, and show ordinary update notices separately.
5. **Repository merge is a later packaging decision.** Move code only after the command UX, version checks, and adapter contract are proven.

## Phase 1 — Compatibility and Release Visibility

**Outcome:** Cave can tell users when Cave itself, `coven`, or `coven-code` has an update, and it can distinguish ordinary updates from versions too old for the current Cave build.

- [ ] **Step 1: Add Cave compatibility floors**

  Implement minimum compatible versions for:

  ```text
  @opencoven/cli >= 0.0.49
  coven-code >= 0.0.22
  ```

  Cave status output should include `minimumVersion`, `compatible`, and `installCommand` per tool.

- [ ] **Step 2: Add a global Cave tools banner**

  The banner should:

  ```text
  warn when any required tool is below its compatibility floor;
  show info severity for newer optional releases;
  dismiss per released tool version;
  send the user to Settings -> About -> OpenCoven tools.
  ```

- [ ] **Step 3: Verify Cave**

  Run:

  ```sh
  pnpm typecheck
  pnpm check:tests-wired
  pnpm test:app
  git diff --check
  ```

## Phase 2 — `coven code` Command Alias

**Outcome:** Users can type `coven code` as the blessed entry point for the Coven Code workflow, even if the first implementation shells out to the installed `coven-code` binary.

- [ ] **Step 1: Add a failing CLI test**

  Add an integration test that runs:

  ```sh
  coven code --help
  ```

  Expected behavior:

  ```text
  exits 0;
  mentions Coven Code;
  shows install or launch guidance when the adapter binary is missing.
  ```

- [ ] **Step 2: Add the clap subcommand**

  Add `code` as a top-level subcommand in `crates/coven-cli/src/main.rs`.

- [ ] **Step 3: Implement adapter launch**

  For v1, resolve `coven-code` from `PATH` and execute it with forwarded args. If missing, fail with:

  ```text
  Coven Code is not installed. Install it with:
  npm i -g coven-code@latest
  ```

- [ ] **Step 4: Verify Coven**

  Run:

  ```sh
  cargo fmt --check
  cargo test -p coven-cli --locked
  ```

## Phase 3 — Adapter Capability Reporting

**Outcome:** `coven doctor` and the daemon capability surface report Coven Code readiness the same way they report Codex and Claude readiness.

- [ ] **Step 1: Add readiness fields**

  Extend capability reporting with:

  ```json
  {
    "id": "coven-code",
    "installed": true,
    "version": "0.0.22",
    "command": "coven code"
  }
  ```

- [ ] **Step 2: Teach `coven doctor` to print the managed command**

  Doctor should prefer:

  ```text
  coven code
  ```

  over direct `coven-code` instructions once the alias exists.

- [ ] **Step 3: Verify API compatibility**

  Add or update tests around `GET /api/v1/capabilities` so clients can detect Coven Code support without hardcoding package assumptions.

## Phase 4 — Packaging Decision

**Outcome:** Decide whether to keep `coven-code` as an independently published adapter package, bundle it as an optional native asset under `@opencoven/cli`, or move to a monorepo.

Evaluate after Phases 1-3:

- update friction in Cave demos;
- install success rate for `coven code`;
- adapter API churn;
- release cadence differences between daemon fixes and code-workflow UX;
- whether one package makes emergency hotfixes faster or slower.

Default recommendation: keep separate packages until `coven code` has shipped at least one release cycle and Cave compatibility checks have caught real version drift.
