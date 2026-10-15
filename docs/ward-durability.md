---
title: "Ward power-loss durability"
summary: "Protected proposal commit barriers, recovery guarantees, and platform limits."
read_when:
  - Changing protected proposal persistence or recovery
  - Investigating missing Ward audit evidence
source_adjacent_reason: "Documents the cross-file durability protocol implemented in coven-cli's Rust authority layer."
---

# Ward power-loss durability

Protected proposal approvals use this order:

1. Commit the scheduled submission recovery binding durably before publishing
   its pending file. Commit its submission receipt before acknowledging staging.
2. Sync the claim contents and its directory after publishing or renaming it.
3. Commit the apply intent with SQLite `synchronous=FULL` before swapping targets.
4. Sync each installed target and its retained parent directory. New directory
   components are published with checked parent barriers. Rollback also syncs
   restored contents and namespace before reporting a completed rollback.
5. Commit apply audit rows, protected baselines, and the approval terminal in one
   FULL transaction. Rejections and veto terminals also use durable transactions.
6. Check the database directory barrier, then remove the claim with a checked
   directory barrier, before returning success.

Recovery repeats the evidence and claim barriers before applying. An idempotent
terminal retry forces a new FULL commit and checks the database directory again:
COMMIT may have succeeded even when its following directory barrier failed.
A reported barrier failure never means that a preceding rename or commit did
not happen. Retained intent/claims and terminal evidence drive reconciliation.

New Coven-owned home/workspace hierarchies use per-parent
`.coven-directory-publications` registries. A marker precedes each mkdir and is
removed only after child and parent sync succeed. Process-crash retries finish
marked publications before creating descendants. Existing unmarked ancestors
are the trusted initialization boundary; they need no write access to their own
parents. Marker basenames use filesystem comparison rules, and Windows verifies
that the registry has the same case-sensitivity setting as its parent.
The census treats a real registry directory in `pending` as bookkeeping;
files or symlinks under that name remain unattributed artifacts.

## SQLite policy

Ordinary session/event connections retain `synchronous=NORMAL`. The scoped
Rust helper temporarily selects FULL for authority transactions, rejects a
nested NORMAL transaction, and restores the original mode after commit, error,
or unwind. An already durable outer transaction owns the final barrier.
`fullfsync=ON` enables SQLite's stronger macOS sync calls, including checkpoints.
The locked bundled SQLite build does not disable sync or directory sync.

[SQLite's WAL documentation](https://sqlite.org/wal.html#performance_considerations)
and [synchronous pragma](https://sqlite.org/pragma.html#pragma_synchronous)
distinguish NORMAL's consistent but potentially lost commits from FULL's
per-commit WAL barrier. A daemon exit alone does not model power loss: the OS
page cache survives it, and graceful connection close may checkpoint the WAL.

## Filesystem contract

On Unix, checked Rust file/directory `sync_all` calls use `fsync`, or
`F_FULLFSYNC` on macOS. Ward keeps the target directory and file identities
open across replacement and sync. Directory barriers reopen `.` relative to the
retained handle with read access because Linux `O_PATH` handles cannot be synced.
The private proposal store retains its
existing assumption that its trusted home/parent path is not concurrently
replaced by another process; target containment still uses anchored handles.

On Windows, regular files are reopened through their retained handles for
flushing. Directories use an empty-name `NtCreateFile` relative to the retained
handle, followed by checked `FlushFileBuffers`. This avoids reopening a replaced
pathname and does not require administrator volume access. This durability
path requires NTFS: [MS-FSA 2.1.5.7](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/0de7dc40-9627-437e-a4df-c4696cdc3d02)
and [product behavior note 80](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-fsa/4e3695bd-7574-4f24-a223-b4679c065b63)
specify directory persistence and identify other filesystem implementations
that may return success without flushing directory structure.

These guarantees depend on storage honoring successful sync requests. Unsupported
or failed barriers return errors; hardware that lies about completed flushes,
filesystem corruption, and external deletion are outside this protocol.

## Deterministic coverage

The test VFS delegates to native SQLite and captures database/WAL bytes only at
successful `xSync` calls. Tests freeze those images before closing connections,
then restore them while keeping the target and claim files. A deliberately
unsynced NORMAL write must disappear, proving the lost-suffix model is active.

The approval matrix covers legacy, human, rationale, veto-window, automatic,
and newly created targets at intent-before-write, apply-before-finalization,
finalization-before-cleanup, and acknowledged-success boundaries. It verifies
intent/terminal survival, exactly-once recovery, and the veto-window census's
unresolved versus typed-terminal classifications. Additional tests cover
submission recovery, baseline/terminal atomicity, SQLite sync errors, directory
sync errors, mode restoration, and retained directory identity after a rename.
These are deterministic protocol tests, not physical power-cut certification.
