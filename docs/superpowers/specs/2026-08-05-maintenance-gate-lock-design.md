# Maintenance Gate Lock Design

## Problem

The maintenance gate serializes owner and writer metadata updates with an
exclusive-create lock file. A contender deletes that file after 30 seconds
without proving the holder exited, and the original holder later deletes
whatever file occupies the path. A live but stalled holder can therefore
overlap another critical section and remove the replacement lock.

## Scope

Fix issue #612 without changing the public maintenance commands, owner and
writer lease records, timeout behavior, or Cave integration. The change is
limited to the internal lock used by `MaintenanceGate::with_lock`.

## Design

Replace the exclusive-create and stale-file protocol with a cross-process
advisory lock provided by the existing `fs2` dependency.

`GateLock` will retain an open `std::fs::File` rather than a path. Acquisition
will:

1. Open the persistent gate lock file for reading and writing, creating it when
   absent, through the repository's existing no-follow, single-link validated
   lock-file helper.
2. Call `try_lock_exclusive`.
3. Retry recognized contention every 10 milliseconds.
4. Return `GateError::Contended` after the existing five-second wait.
5. Return other I/O errors with the lock path in the error context.

The lock file remains in place. No process removes it, and its modification time
has no protocol meaning. The operating system releases the lock when the file
handle is unlocked, closed, or the process exits, so a stalled live holder
cannot be mistaken for a dead holder.

`Drop` will best-effort unlock the held file handle, matching the repository's
existing `StateLock` RAII behavior. Closing the handle remains the final
release mechanism if explicit unlock fails.

## Compatibility

All callers continue through `MaintenanceGate::with_lock`; no caller or public
API changes are required. Cave uses the documented `coven maintenance`
commands, so it does not need to implement the advisory lock independently.
Owner generations, writer generations, heartbeat intervals, lease expiry, and
fail-closed malformed-record handling remain unchanged.

An already-running binary built before this fix does not participate in the
advisory lock. Long-running Coven daemons must therefore be restarted after
upgrading so every participant uses the same lock protocol.

## Error Handling

Only errors recognized as lock contention are retried. The existing bounded
wait and `GateError::Contended` result are preserved. File-open and lock errors
remain explicit and include maintenance-lock path context on top of the shared
helper's validation failures. Unlock errors are not surfaced from `Drop`,
consistent with existing lock cleanup behavior.

## Tests

Add focused unit tests that demonstrate:

- a second `GateLock` cannot acquire while the first holder is live;
- acquisition succeeds after the first holder drops; and
- an old lock-file modification time does not allow takeover of a live holder;
- a symlinked maintenance lock path is refused without touching the target; and
- a multiply linked maintenance lock file is refused.

Existing maintenance owner and writer tests continue to cover unchanged
protocol behavior.

## Non-Goals

- Redesigning owner or writer lease records.
- Adding PID, generation, or heartbeat data to the internal lock file.
- Changing maintenance CLI or HTTP response contracts.
- Broad hardening of owner or writer metadata files beyond the stale-lock race,
  while preserving lock-path safety through the existing validated helper.
