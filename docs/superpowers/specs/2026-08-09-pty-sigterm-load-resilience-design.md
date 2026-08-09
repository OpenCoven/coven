# PTY SIGTERM Load-Resilience Design

**Status:** Approved for implementation
**Bead:** `coven-047`

## Problem

`native_stream_sigterm_cancels_and_reaps_process_tree` must exercise the
native-stream cancellation path without creating a signal-disposition race or
using fixture PID files as unsafe cleanup authority. A previous acknowledgement
poll observed the production cancellation atomic after `pthread_kill`; guard
finish legitimately clears that atomic, producing a false watchdog failure.
Likewise, PID-file readiness can precede guard activation, and numeric PIDs or
their process-group equality can be reused before failure cleanup runs.

## Decision

The test will assert behavior, not host scheduling speed:

- The test observer holds its lifecycle lock across `pthread_kill`, and only
  sends after both fixture identities are ready and the observed runner owns an
  active guard. This makes dispatch while the test-owned cancellation
  disposition is active synchronous with restoration exclusion; it does not
  poll or extend the production cancellation atomic.
- The final expected native-stream cancellation error proves the runner
  consumed that dispatched SIGTERM.
- The previous SIGTERM handler is restored.
- The fixture records durable process-start identities before signalling. Linux
  uses `/proc/<pid>/stat` field 22; macOS uses `proc_pidinfo(PROC_PIDTBSDINFO)`
  and its seconds/microseconds start timestamp. A later missing PID is reaped
  only when it was first captured under that identity; a different start
  identity is reported as reuse, never as fixture reaping.
- Failure cleanup uses a per-test FIFO command and unpredictable token created
  before launch. The fixture-owned sentinel acknowledges that exact token and
  calls `kill -KILL 0` from inside its own `setsid` process group. The test
  therefore never sends a destructive signal to a PID or PGID read from a
  fixture file. The FIFO stays usable even when the harness or descendant PID
  file was never published.

If PID readiness appears before activation, the signaler keeps bounded polling
outside lifecycle locks. If the runner finishes first, it records a non-signal
failure and lets the main thread perform cleanup. At watchdog expiry, it asks
the fixture sentinel to end its own group rather than sending SIGTERM without
both prerequisites. Watchdogs remain diagnostic safeguards, not a normal
performance promise.

## Scope

Modify only the Linux/macOS Unix PTY runner test in
`crates/coven-cli/src/pty_runner.rs` and this test design/plan record. No
production cancellation or signal-handling code changes, timeout tuning in
unrelated tests, ignored tests, or CI test splitting are included. Linux and
macOS are the repository's supported Unix CI platforms; unsupported Unix
targets do not compile this platform-specific test rather than falling back to
unsafe PID cleanup.

## Validation

Run the named unit test normally and under Python-supervised load from exactly
24 locally started `yes` spinners. The test must continue to prove the
cancellation error, handler restoration, durable fixture reaping, and absence
of side effects in the production handler. If platform identity inspection or
the FIFO acknowledgement cannot prove cleanup, the test fails without a
destructive fallback; this may leave a fixture only in that explicitly reported
broken-fixture case.
