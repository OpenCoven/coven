---
title: "Background daemon startup failure diagnostics"
summary: "Carry safe, correlated mobile startup failures to start and restart callers."
read_when:
  - Changing background daemon startup diagnostics
source_adjacent_reason: "Defines implementation and verification of native issue 1474."
---

# Background daemon startup failure diagnostics implementation plan

> **For agentic workers:** Use superpowers:executing-plans inline, followed by
> one independent whole-branch review. Checkboxes are intended steps; commits
> and issue evidence establish completion.

**Goal:** Explain a failed configured mobile gateway to daemon start/restart
callers without changing readiness or configuration policy.

**Architecture:** Reuse the existing recovery log for fixed failure codes,
correlated by launched PID and a digest of the existing started-at identity.
One short-lived diagnostic reader per foreground launch observes a bounded log
tail within the original lifecycle deadline. It only enriches an already
failed health result; it never grants readiness or stops a process.

**Tech stack:** Existing Rust standard library, anyhow, blake3 and recovery log.

**Spec:** https://github.com/OpenCoven/coven/issues/1474 under
https://github.com/OpenCoven/coven-cave/issues/6119.

## Global constraints

- Preserve authenticated intended-instance health, existing deadlines, startup
  order, service ownership, private-address checks, configuration and data.
- No foreground config/DNS validation, new dependency, supervisor, retries,
  persistent status model, installer, silent disabling or timeout increase.
- Emit only fixed codes, numeric PID and a fixed-size identity digest. Never
  copy arbitrary errors, configuration values, credentials or private paths.
- Read at most 16 KiB from the current regular log, reject links/devices, and
  do not collect archives. Missing/rotated/unreadable evidence is advisory.
- The reader has one worker per launch, polls at most every 50 ms, and stops
  on cancellation/deadline. A blocked filesystem call cannot delay its caller;
  it may finish in the background until the short-lived CLI process exits.
- Healthy readiness never waits for diagnostic completion. Failure collection
  waits at most 50 ms and only within the remaining original lifecycle budget.
- Use isolated homes and owned processes. No release or real-user fault injection.

## Review focus

- Stale PID/attempt, malformed or arbitrary log records must not alter errors.
- A hung diagnostic read must not extend failure or healthy-start deadlines.
- A healthy intended instance wins even when a matching diagnostic is present.
- Windows and Unix preserve existing health, ownership and process containment.
- Invalid configured gateways retain their failure semantics and exact bytes;
  absent/disabled configuration and explicit correction can still start.

## Task 1: Preserve actionable startup failure evidence

**Files:** `crates/coven-cli/src/daemon.rs`, new
`crates/coven-cli/src/daemon/startup_failure.rs`,
`crates/coven-cli/src/mobile_memory/config.rs`,
`crates/coven-cli/src/mobile_memory/gateway.rs`,
`crates/coven-cli/tests/daemon_startup_diagnostics.rs`, and this plan.

**Interfaces:** A typed `MobileConfigValidationFailure` context marks validation
errors without changing their original chain. `startup_failure::record(home,
status, error)` appends one fixed failure classification. `Observer::start(home,
status, deadline)` returns a cancellable advisory observer;
`Observer::enrich(result, deadline)` returns healthy results immediately, preserves failed results, and adds only
the matching safe corrective context. No public CLI/status schema changes.

- [ ] Add a native regression: both background start and restart with enabled
  zero-port configuration must fail with `mobile gateway configuration`,
  `mobile/gateway.json`, `non-zero` and `coven daemon restart` in stderr. Keep
  configuration bytes, and verify start/status/stop after explicit correction.
  Retain existing foreground, missing-configuration and unavailable-log cases.
- [ ] Run `cargo test -p coven-cli --test daemon_startup_diagnostics --locked`;
  expect the background corrective-message assertion to fail before edits.
- [ ] Add reader tests for stale PID/attempt, unknown/malformed codes, injected
  private text, a matching record beyond the tail limit, symlink/FIFO/directory,
  deadline exhaustion, blocked reads, cancellation, and success precedence.
  Use explicit channel barriers for races, with generous harness hang guards.
- [ ] Implement the interfaces above. Wrap the existing system-controller
  health result, and record mobile errors in both serve paths before returning
  the original failure. Keep existing malformed/disabled config semantics.
- [ ] Run focused tests, format, workspace Clippy/tests and every native policy
  guard, including changed-range docs ownership and privacy. Expect exit zero.
- [ ] Commit with human numeric no-reply sign-off. Repeat the production Cave
  API fault scenario with the exact real candidate binary and preserve all
  failures, source/binary identities, correction, health and cleanup evidence.
- [ ] Obtain one independent exact-diff review; fix substantive findings in one
  failing-first pass, preserve rulings, then push/PR/protected merge with current
  exact-head checks. Retain worktree and evidence and release the claim on merge.

## Acceptance limits

This is diagnostic propagation, not automatic configuration repair. A missing
record leaves the original error unchanged. Log rotation can omit evidence.
Local Unix verification does not establish Windows runtime behavior; its CI
and native tests remain required. No original-user root-cause claim is made.
