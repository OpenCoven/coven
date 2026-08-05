# Chaos Benchmark Event Writer Recovery Design

**Goal:** Fix the intermittent Linux `sessions_32` timeout and retain bounded
diagnostics if a future output wait fails for another reason.

## Root cause

All 32 fixture children executed, but concurrent session setup held SQLite's
single-writer lock beyond the event writer connection's five-second
`busy_timeout`. The worker treated that transient `SQLITE_BUSY` as permanent,
latched a failed state, and stopped before committing any output events.

## Recovery approach

Retry only errors whose preserved `rusqlite::Error` reports
`DatabaseBusy` or `DatabaseLocked`. The worker makes at most four total commit
attempts with a 50 ms pause between attempts. Every attempt creates a fresh
transaction over the same in-memory batch.

The worker remains single-threaded and does not release queue capacity or
acknowledge critical events until the batch commits. This preserves batch
ordering and the existing critical-event acknowledgement guarantee. Exhausted
lock retries and every non-lock error still enter the existing permanent
failure path.

## Boundary diagnostics

Keep the 32-session scenario, launch behavior, and timeout unchanged. The
fixture harness appends one fixed marker before printing its ready line. If an
output wait expires, the benchmark reports the number of markers, the session
status and observed event kinds, and a bounded snapshot of
`/api/v1/health.eventWriter`.

This distinguishes child execution failure from PTY/output ingestion failure,
writer pressure/failure, and event-query mismatch without converting the
advisory benchmark itself into a retrying success path.

## Privacy and failure behavior

Markers contain only the fixed word `started`; diagnostics report only their
count. Event-writer output is restricted to documented scalar health fields,
control characters are collapsed, values are bounded, and fixture paths are
redacted. Diagnostic collection is best-effort and never replaces the original
timeout error.

## Testing

Rust tests create real competing SQLite connections to prove that a transient
lock is retried, a persistent lock stops at the attempt limit, and a non-lock
SQLite error is not retried. Node tests pin marker ordering, execution-count
summaries, event-writer field selection/redaction, and the combined timeout
diagnostic using injected request and file-read boundaries.
