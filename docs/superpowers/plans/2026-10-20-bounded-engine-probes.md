---
title: "Bounded engine probes implementation plan"
summary: "Return useful Doctor diagnostics when local engine probes hang."
read_when:
  - Changing local engine diagnostics
source_adjacent_reason: "Defines the implementation and verification contract for coven issue 1458."
---

# Bounded engine probes implementation plan

> **For agentic workers:** Use superpowers:executing-plans inline with one independent final review. Checkbox state is not evidence of completion; code, verified results and merged PRs establish completion. Issue #1458 owns the work record.

**Goal:** Keep local engine diagnostics bounded and useful when version or authentication probes fail.

**Architecture:** Reuse the existing strict-child-tree authentication probe loop for both local commands. Doctor supplies one absolute three-second deadline, preserving headroom within Cave's six-second execution limit. Other version callers use the same bounded default. Cold native loading precedes the Rust clock; reserve that headroom rather than relaxing the six-second caller limit. Keep parsing and version admission unchanged.

**Tech Stack:** Rust 1.99.1, existing process containment and native fixtures.

**Spec:** https://github.com/OpenCoven/coven/issues/1458; parent https://github.com/OpenCoven/coven-cave/issues/6119.

## Global constraints

- No provider turn, daemon restart, private profile access, dependency addition or release.
- Preserve strict process containment, output limits, cleanup budget and local-auth exit/JSON agreement.
- Doctor's auth/version probes share an absolute three-second deadline; do not add independent delays past Cave's six-second budget.
- Version probe stdout is capped at 16KiB; existing auth output remains capped at 1MiB. Discard unused stderr. Never include arbitrary engine output in errors.
- Keep all existing version consumers and compatibility decisions; unavailable version is not a successful compatibility check.
- Retain the owned worktree and evidence; no deletion authority is inferred.

## Review focus

- A version child that exits while a descendant retains stdout must not hold the command open or survive cleanup.
- A second probe must not start after the shared deadline's observation budget is exhausted.
- Valid, malformed, nonzero and excessive version output must preserve honest unknown/version states without exposing arbitrary bytes.
- Windows containment must precede child execution; preserve existing strict-child helper unchanged.
- A slow auth probe plus a slow version probe must produce a diagnosis within six seconds, with all recorded owned processes stopped.

### Task 1: Bound and consolidate local engine probes

**Files:** Modify `crates/coven-cli/src/engine.rs`, `crates/coven-cli/src/main.rs`, `crates/coven-cli/tests/doctor_auth_boundary.rs`, `crates/coven-cli/tests/fixtures/doctor_auth_probe.rs`.

**Interfaces:**
- Add `engine::probe_stdout(binary: &Path, args: &[&str], deadline: Instant, output_limit: usize) -> Option<(ExitStatus, Vec<u8>)>` by moving the existing strict auth observation/cleanup loop, without a new supervisor or dependency.
- Add `engine::engine_version_until(binary: &Path, deadline: Instant) -> Result<(u64, u64, u64)>`; retain `engine_version(binary)` as the existing caller interface with a bounded default.
- Change `engine_auth_summary(binary: &Path, deadline: Instant) -> Option<bool>` to parse the shared helper's result. Doctor creates one deadline before gathering diagnostics and probes version before auth, preserving useful version evidence when auth stalls.

- [ ] Extend the existing native fixture with delayed version, stdout-holding version descendant, excessive valid-tail output, malformed/nonzero version, and combined auth/version delay. Existing auth behavior remains intact.
- [ ] Add native tests: `cli_version_remains_available_when_the_engine_probe_hangs`, `doctor_version_probe_is_bounded_and_cleans_up`, `doctor_version_descendants_do_not_hold_output_open`, `doctor_combined_engine_faults_share_one_deadline`, `doctor_version_failures_remain_private_and_unavailable`. Assert real command output, six-second contract, recorded-process cleanup and no provider-harness invocation. Use delayed fixtures long enough to distinguish old behavior; retain failure durations.
- [ ] Run `cargo test -p coven-cli --test doctor_auth_boundary --locked` before implementation. Expected: new deadline/containment/oversized-tail cases fail; existing local-auth contract passes.
- [ ] Move the bounded observation loop and wire both consumers to it. Refuse spawn after the observation deadline, cap stdout, preserve exit status, and keep cleanup within the shared absolute deadline. Use a fixed unavailable-version error instead of arbitrary output.
- [ ] Run the focused target and existing engine/auth unit cases. Expected: all pass; inspect actual command elapsed times and cleanup results.
- [ ] Run full fmt, workspace/all-target clippy, workspace tests, complete Policy guard and staged/ranged docs/privacy checks; obtain exact-head Windows/Linux CI and one independent review. Repeat matched isolated diagnostic measurements on the final binary. Commit/push and land through protected checks, preserving dependency and human-credit requirements.
