---
title: "Daemon serving checkpoints implementation plan"
summary: "Measure native initialization after status publication without changing lifecycle policy."
read_when:
  - Extending native startup diagnostics
source_adjacent_reason: "Records the implementation and verification contract for coven issue 1436."
---

# Daemon serving checkpoints implementation plan

> Use superpowers:executing-plans for inline implementation and one independent
> final review. Checkboxes describe intended steps, not authoritative execution
> status. Code, verified results and the merged PR establish completion.

**Goal:** Attribute the unmeasured interval between daemon status publication
and entry to the API accept loop.

**Architecture:** Extend the existing `StartupCheckpointObserver` and fixed
phase vocabulary at the same operations in both native serve paths. Keep the
current operations and local resource handles in their existing scopes; no new
runtime abstraction, supervisor or background diagnostic worker is needed.

**Tech stack:** Rust 1.99.1, existing recovery log and native integration tests.

**Spec:** https://github.com/OpenCoven/coven/issues/1436, under
https://github.com/OpenCoven/coven-cave/issues/6119.

## Global constraints

- Preserve ownership, deadlines, SQL/durability, retries, recovery order and
  authenticated readiness. An accept-loop checkpoint is not readiness proof.
- Log only fixed phase names and the existing numeric elapsed/observer fields.
  Do not log configuration, paths, transcripts, SQL or credentials.
- Reuse bounded rotation and advisory log semantics. Timings include
  scheduling; recorded prior observer cost is not current append completion.
  End markers mean a call returned, including best-effort recovery; they do not
  assert background scheduler work completed. Existing log I/O can perturb
  startup and has no new wall-time guarantee in this measurement-only change.
- Preserve source evidence and failed samples. Use synthetic homes and owned
  child processes; never stop or inspect private contents of the real profile.
- The original startup failure remains unexplained until causal evidence exists.

## Review focus

- Failure after status publication must not emit a later completion checkpoint.
- Disabled mobile startup returning `None` is a completed initialization call,
  not evidence that a mobile listener exists.
- A missing or unreadable diagnostic log must not block a healthy daemon.
- Windows and Unix must retain the same phase vocabulary and resource lifetimes.
- Tests must kill/reap only their own foreground child and retain bounded,
  filtered diagnostics on failure, without inheriting provider credentials.

## Task 1: Observe and verify initialization through serving

**Files:** `crates/coven-cli/src/daemon.rs`,
`crates/coven-cli/tests/daemon_startup_diagnostics.rs`,
`crates/coven-cli/tests/fixtures/daemon_startup_diagnostics.rs`, and this plan.

**Interfaces:** Reuse `StartupCheckpointObserver::new`, `append`,
`StartupCheckpoint::label`, and `capture_startup_checkpoints(&Path) -> String`.
No public API, runtime handle or configuration interface changes.

- [ ] Add meaningful failing cases to the existing diagnostic integration target.
  A directly owned foreground daemon in a fresh isolated home must reach
  authenticated status with the same PID and produce the ordered phases below.
  Repeat with an enabled mobile configuration whose bind port is zero: it must
  exit unsuccessfully with the existing configuration error, retaining only
  phases through `automations-scheduler-end`. A directory at the log path must
  still allow healthy status. Always terminate and reap the directly owned
  child, including assertion failures. Bound harness waits generously without
  changing any production deadline.
- [ ] Extend the existing allowlist regression with all new fixed phases.
  Run `cargo test -p coven-cli --test daemon_startup_diagnostics --locked`;
  expect failure for missing production phases and rejected new allowlist values.
- [ ] Add these enum variants/labels and observe them in this order in both
  native serve paths, starting a new observer immediately before recovery:
  `runtime-initialize-begin`, `orphaned-sessions-end`, `stale-created-sessions-end`,
  `afs-recovery-end`, `session-runtime-end`, `threads-scheduler-end`,
  `maintenance-scheduler-end`, `liveness-sweeper-end`,
  `automations-scheduler-end`, `mobile-gateway-end`, `api-accept-loop-enter`.
  Each `-end` follows the existing successful return; emit accept-loop entry
  immediately before the loop, after transport-specific setup.
- [ ] Extend the test diagnostic reader's fixed allowlist without relaxing
  numeric validation or private-value rejection. Keep the existing deterministic
  observer-overhead and store/publication failure tests.
- [ ] Run the diagnostic target, relevant daemon startup unit selectors and
  existing Unix smoke/Windows lifecycle targets on their supported platforms.
  Inspect monotonic elapsed/observer fields and all child cleanup results.
- [ ] Run the repository-required fmt, clippy, workspace tests, secret/privacy
  and complete Policy guard checks. Resolve real failures without weakening
  assertions. Record exact installed compiler versions and platform gaps.
- [ ] Review the scoped diff, then commit with DCO and push the issue branch.
  Obtain one independent whole-branch review; address Important findings with
  RED→GREEN evidence. Validate the final committed source-built daemon using
  isolated homes, current-head hosted checks and protected merge guards.

The signed-off commit, PR, measured results, diagnostic cost and remaining
causal investigation belong on #1436. Preserve this worktree with an explicit
owner/reason when delivery finishes; release its claim without deleting it.
