# PTY SIGTERM Load-Resilience Design

**Status:** Approved for implementation
**Bead:** `coven-047`

## Problem

`native_stream_sigterm_cancels_and_reaps_process_tree` currently
asserts that cancellation completes in under two seconds. Under heavy machine
load, SIGTERM delivery and process scheduling can legitimately exceed that
wall-clock budget even when the stream runner returns the required cancellation
error and reaps its child process tree.

## Decision

The test will assert behavior, not host scheduling speed:

- SIGTERM causes the expected native-stream cancellation error.
- The previous SIGTERM handler is restored.
- The harness and its descendant no longer exist after cancellation.

The test will retain generously bounded watchdogs only to fail a genuinely
broken fixture or unreaped process. Those watchdogs are diagnostic safeguards,
not a promise that normal cancellation completes within a fixed duration.

## Scope

Modify only the Unix PTY runner test in
`crates/coven-cli/src/pty_runner.rs`. No production cancellation or signal
handling code changes, timeout tuning in unrelated tests, ignored tests, or CI
test splitting are included.

## Validation

Run the named unit test repeatedly both normally and under deliberate CPU load.
The test must continue to prove the cancellation error, handler restoration,
and process-tree reaping. Run the package and workspace test gates after the
targeted stress result is clean.
