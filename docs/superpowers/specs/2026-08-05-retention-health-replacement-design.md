# Retention Health Replacement Design

## Problem

PR #608 implements bounded SQLite retention and storage-pressure health, but its
head no longer compiles, conflicts with current `main`, predates the live event
writer and session-handoff health contract, and contains commits that fail DCO.
The useful retention work must be ported without regressing features that
landed after the original branch diverged.

## Scope

Create a maintainer replacement for PR #608 from current `origin/main`.
Resolve only the current merge blockers:

- port the bounded retention, storage health, recovery-log rotation, indexes,
  tests, and documentation from PR #608;
- fix the compile errors in the maintenance convergence loop;
- integrate storage health with the current event-writer and session-handoff
  health contract;
- replace unsigned contributor history with signed maintainer commits while
  preserving contributor attribution.

The non-blocking review notes on PR #608 remain out of scope unless a directly
coupled correctness issue must be fixed for the port to work.

## Port Strategy

Start from current `origin/main` and squash-port PR #608's net changes. Resolve
the conflicts once against the current code rather than cherry-picking or
merging the unsigned seven-commit history.

The replacement implementation commit will include verified GitHub-linked
`Co-authored-by` trailers for Timothy Wayne Gregg (`CompleteDotTech`) and
Copilot, using numeric IDs resolved through the GitHub REST API.

The maintainer commit will also carry the repository-required DCO signoff.

## Health Contract Integration

The current `/health` response remains authoritative:

- preserve `capabilities.sessionHandoff`;
- preserve the existing optional `eventWriter` block;
- add the optional `storage` block from PR #608 alongside it.

`EventWriterHealth` will gain an exact `queued_events` field maintained with
the existing queued-byte count. It counts accepted but not yet committed
events, including an in-flight batch, and is decremented at the same completion
boundary as queued bytes. The health route will obtain one writer-health
snapshot from `SessionRuntime`, expose that snapshot as `eventWriter`, and pass
it to storage-health collection.

`StorageHealth.writerBacklogEvents` and
`StorageHealth.writerBacklogBytes` will be derived from that live snapshot
rather than hard-coded to zero. When no runtime writer snapshot is available,
the values are zero because there is no observed daemon queue.

## Retention and Maintenance Behavior

Port PR #608's reviewed behavior:

- bounded transactional event and sensitive-artifact pruning;
- indexes that serve the retention scans;
- a capped per-tick catch-up loop that converges without monopolizing SQLite;
- retention precedence matching `coven logs prune`;
- injectable configuration and free-disk seams for deterministic tests;
- no SQLite open or write below the 256 MiB safety watermark;
- passive WAL checkpointing only above the threshold;
- no automatic `VACUUM`;
- retention-lag participation in storage warning status;
- bounded recovery-log rotation that preserves valid older archives after a
  partial prior rotation.

The convergence loop will compare compatible integer types, and
`oldest_retained_event_at` will have an explicit optional string type so the
replacement compiles on current Rust targets.

## Error Handling

Maintenance remains fail-closed under disk pressure. Storage-health collection
returns the explicit unavailable representation when metrics cannot be read,
while the health response retains the rest of the current daemon contract.
Transactional pruning continues to rely on SQLite rollback and existing FTS
triggers for consistency.

No automatic compaction is introduced. Operators continue to use
`coven vacuum` when they intentionally accept the exclusive compaction cost.

## Tests

Preserve and reconcile PR #608's tests for:

- bounded event and artifact pruning;
- FTS consistency after interrupted work;
- configured retention precedence;
- deterministic free-disk watermark behavior;
- multi-batch convergence and synthetic steady-state size;
- retention-lag warning status;
- passive checkpoint reporting;
- recovery-log rotation and partial-rotation recovery;
- storage-health API serialization and documentation.

Add or update tests proving:

- `EventWriterHealth.queuedEvents` tracks the real in-memory queue;
- `queuedEvents` and `queuedBytes` remain present in `eventWriter`;
- storage backlog fields are populated from the same writer snapshot;
- `sessionHandoff` remains present after the health response merge.

Run focused retention, event-writer, health, and recovery-log tests before the
complete repository gates.

## Delivery

Open a replacement PR from `fix/597-retention-health-v2` that closes issue
#597 and credits the original contributor. After the replacement is open and
validated, comment on PR #608 with the replacement link and close #608 as
superseded.

## Non-Goals

- Addressing every non-blocking observation from PR #608.
- Redesigning the event writer or its queue limits.
- Changing retention defaults or public field names introduced by PR #608.
- Adding automatic `VACUUM`.
- Rewriting or force-pushing the contributor's fork branch.
