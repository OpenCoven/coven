# PTY SIGTERM Load-Resilience Design

**Status:** Approved for implementation
**Bead:** `coven-047`

## Problem

`native_stream_sigterm_cancels_and_reaps_process_tree` must exercise the
native-stream cancellation path without creating a signal-disposition race or
falling back to unsafe PID-file cleanup. A previous acknowledgement poll
observed the production cancellation atomic after `pthread_kill`; guard finish
legitimately clears that atomic, producing a false watchdog failure. Likewise,
PID-file readiness can precede guard activation, and numeric PIDs or their
process-group equality can be reused before failure cleanup runs.

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
- The durable cleanup coordinator is test-owned rather than a background
  process in the harness session. It opens the FIFO read-only and nonblocking
  before the fixture starts; the request side subsequently opens it write-only
  and nonblocking. It records the exact token in the acknowledgement file and
  exits. This keeps the acknowledgement available after the production runner
  correctly kills the harness session/process group—a lifecycle that otherwise
  kills an in-fixture FIFO reader on Linux. Cleanup always requests this
  acknowledgement before evaluating reaping; on an error the coordinator is
  stopped, so its test thread remains bounded. The coordinator never signals a
  fixture PID or process group. A separate fixture-local emergency FIFO retains
  the existing `kill -KILL 0` fallback: only a surviving member of the
  fixture's own process group can consume that command, so it cannot target a
  recycled PID or an unrelated group.

If PID readiness appears before activation, the signaler keeps bounded polling
outside lifecycle locks. If the runner finishes first, it records a non-signal
failure and lets the main thread perform cleanup. If the 30-second
fixture-start watchdog expires and the owner-scoped guard lifecycle is still
active, the fallback sends a thread-directed SIGTERM while holding the
lifecycle mutex, then records a fixture-start failure; if the lifecycle is
inactive, it records the failure without signalling. The stream then returns
and identity-safe cleanup plus aggregated assertions run. This is a
containment failure path, not normal cancellation or a performance assertion;
it never signals under the restored/default handler and does not alter async
handler semantics.

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
of side effects in the production handler. If platform identity inspection
cannot prove cleanup, the test fails without an unsafe fallback; this may leave
a fixture only in that explicitly reported broken-fixture case.
