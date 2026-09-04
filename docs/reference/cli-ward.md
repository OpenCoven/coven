---
summary: "Inspect and manage the Ward proposal lifecycle: pending reads, principal decisions, the audit ledger, and config migration."
read_when:
  - Looking up ward
  - Reviewing staged (held) Ward proposals
  - Approving or rejecting a Ward proposal
  - Auditing applied Ward writes
title: "coven ward"
description: "Reference for coven ward: inspect, approve, or reject pending Ward proposals, read the append-only ward_audit ledger, and migrate v0.1 ward.toml files to the Phase-2 WardConfig dialect."
source_adjacent_reason: "Tracks the Ward CLI and security contracts implemented in this repository."
---

`coven ward` groups the Ward's principal-facing lifecycle verbs. Held writes
into a familiar home never dead-end: the daemon stages them at
`~/.coven/pending/` for the principal's decision, and `coven ward pending`
is the supported way to see what is waiting.

```sh
coven ward pending             # bounded table of staged proposals
coven ward pending <id>        # one proposal in full
coven ward pending --json      # exact daemon body (GET /api/v1/threads/proposals)
coven ward approve <id>        # re-validate and atomically apply
coven ward reject <id> [--note "reason"] # reject without applying
coven ward audit <familiar>    # append-only ward_audit ledger, newest first
coven ward migrate --apply     # migrate v0.1 ward.toml files to Phase-2
```

## Ward apply resource limits

Every submitted Ward request accepts at most 32 edits. The cap includes
Tier-0/Tier-1 edits that will be held or staged as well as Tier-2/Tier-3 edits
eligible for direct apply. It runs on the borrowed request array before content
cloning, Ward/Gate-2 evaluation, probe execution, target preparation, proposal
staging, or mutation. Each existing edit can retain three file descriptors
through finalization: its before-image, installed staging inode, and displaced
backup. The 32-edit cap therefore bounds the worst case at 96 descriptors,
leaving substantial daemon headroom under the portable low 256-descriptor soft
limit common on macOS and Linux.

Proposed contents may total at most 16 MiB (16,777,216 bytes). During direct
apply, approval, and recovery, every retained existing-file before-image shares
that same aggregate with the proposed contents; one before-image may not exceed
16 MiB. Approved and recovered proposals are revalidated because pending files
and durable recovery state are untrusted and may predate the limit.

Phase-5 proposal copies are bounded independently before typed deserialization:
`pending.edits`, `materialized_diff.surfaces`, recovery `beforeImages`, and
derived replay bytes cannot exceed their collection/content ceilings.
Materialized `after` bytes are compared by count, surface identity, and exact
content with `pending.edits` only after both representations pass those bounds.
The generic encoded-envelope parser is capped at 406,847,488 bytes (388 MiB),
which includes conservative JSON expansion and 4 MiB of structural overhead.
The active pending store has a much smaller 64 MiB per-file and aggregate
quota, so no accepted proposal reaches that parser ceiling. Static oversize is
rejected from metadata before body allocation; a capped read catches
concurrent growth.

Direct preparation borrows proposed buffers rather than cloning them. Sparse
files count by logical metadata length. Reads remain bounded after metadata
inspection so concurrent growth cannot bypass the ceiling.

The 16 MiB value is a retained-content budget, not a hard ceiling for the
daemon's whole heap. Bounded reads and identity/content verification use one
fixed 64 KiB stack scratch buffer at a time, plus constant-size SHA-256 state
and, when needed, one 64-character digest. Verification does not allocate
another file-sized buffer.

An oversized apply request returns `413 ward_apply_too_large`; exhausted
pending capacity returns `413 proposal_quota_exceeded`. Both carry
`writeApplied: false`; quota failures also carry `retrySafe: true`. No earlier
edit commits and no rejected proposal staging file remains. For
`directBatchEdits`, split the request into batches of at most 32 edits. For
`existingBeforeImageBytes`, shrink or archive the reported target before
retrying. For `directBatchRetainedBytes`, split the edits into smaller batches.
For `proposalEnvelopeBytes`, quarantine or remove the oversized on-disk file.
The `directBatch*` detail labels remain for API compatibility but apply to
every Ward tier and to approved/recovered proposals. Repeating the unchanged
request or approval returns the same error.

## Pending proposals

Two lanes stage here, distinguished by `reviewKind`:

- `authority` — a Tier-0 (protected) write whose thread frayed
  (`DegradeToProposal`, coven-threads §5).
- `coherence` — a Tier-1 (reviewed) write held for Gate-3 coherence review
  (`docs/design/ward-gate3-coherence.md`).

The active queue accepts at most **64 proposals** and **64 MiB
(67,108,864 bytes)** of exact serialized pending/decision-claim data. Coven
reconciles both limits from the actual directory under process and OS locks
before the atomic publish, so concurrent creators cannot over-admit and daemon
restart needs no trusted counter. Approval, rejection, veto, 30-day expiry,
terminal retry cleanup, deletion, and quarantine release active capacity.
Rejection/expiry keep a bounded terminal-claim escape hatch, so they remain
available to drain a queue that is already full without ever applying targets.

`proposal_quota_exceeded` identifies either `pendingProposalCount` or
`pendingProposalBytes` and reports current, attempted, and maximum values.
Resolve an old proposal, wait for expiry cleanup, or archive/remove an invalid
entry before retrying. A single proposal above 64 MiB cannot be published.

`--json` output carries exactly the daemon body's data (pretty-printed) per
the [observe contract](cli-observe.md). The list API returns at most 64 entries
in deterministic filename order with `hasMore` and an opaque `nextCursor`;
pass that cursor to `/api/v1/threads/proposals?cursor=...` for the next page.
The scheduler opens/parses at most 16 proposal or recovery-claim files per
30-second tick and persists a round-robin cursor, so later work is not starved.

Unparseable, non-regular, or globally oversized files appear once as
`degraded`, then move to `~/.coven/pending/quarantine/` so later list and
scheduler passes do not repeatedly parse them. Quarantine remains available
for operator inspection but still consumes disk outside the active quota;
consult the daemon recovery log, preserve any needed evidence, and delete old
quarantine artifacts under the normal retention policy. Unknown ids fail with
`proposal_not_found`.

Proposals older than 30 days are terminally rejected by the scheduler with
audit decision `expired`; target files are never applied. Interrupted expiry
uses the same durable decision-request recovery path as principal decisions.

Every newly staged proposal carries deterministic, offline probe evidence.
The list prints its aggregate `passed`, `failed`, or `unscored` status;
`coven ward pending <id>` prints each surface, its staging-time baseline and
proposed SHA-256, and every probe result. `--json` exposes the same data as
`probeSummary` (list and detail) plus `probes` (detail only).

Declare probes per surface in the familiar's `ward.toml`:

```toml
[[probe]]
surface = "reviewed/**"
id = "parse"
format = "markdown-front-matter" # also: toml, json

[[probe]]
surface = "reviewed/**"
id = "size-delta"

[[probe]]
surface = "reviewed/**"
id = "protected-region"

[[probe]]
surface = "reviewed/**"
id = "pattern-lint"
forbidden = ["(?i)ignore previous"]
required = ["(?m)^name:"]
```

- `parse` checks TOML, JSON, or a UTF-8 Markdown document whose opening and
  closing `---` fences contain valid YAML front matter.
- `size-delta` reports byte and logical-line deltas; v1 has no threshold.
- `protected-region` requires every block fenced by
  `<!-- ward:protected -->` and `<!-- /ward:protected -->` to stay
  byte-for-byte unchanged at the same logical line position.
- `pattern-lint` runs Rust regular expressions against the proposed UTF-8
  contents. Forbidden patterns must not match; every required pattern must.

No matching `[[probe]]` declaration is explicitly `unscored`, never a pass.
Invalid regexes, unreadable baselines, and other probe errors are also
`unscored`. Failed and unscored results are advisory evidence: neither result
applies, rejects, or auto-approves a proposal.

The daemon recomputes persisted evidence against the staged edits, current
baseline, Gate-2 path resolution, and declared probe set. Stale, malformed, or
inconsistent sidecars are demoted to `unscored` and carry
`probeEvidenceDegraded`; they are never summarized as a pass.

## Principal decisions

`coven ward approve <id>` and `coven ward reject <id>` are local CLI wrappers
over the existing daemon API decision routes. Both first read the pending
proposal and submit its exact `proposalRevision`, so a concurrent change fails
closed instead of deciding stale bytes. `--note <TEXT>` is available on both
verbs and is required when an approval's declared path calls for a principal
rationale. `--json` prints the API decision report.

Decision routes require the filesystem-permission-protected Unix socket or the
owner-only Windows pipe. Pending list/detail responses expose familiar
identity, target paths, writer fingerprints, hashes, and probe diagnostics, so
they are owner-local too. The optional loopback TCP listener returns
`403 transport_forbidden` for every `/api/v1/threads/proposals` read or
mutation before proposal lookup or audit mutation; UUID secrecy is never an
authorization control. Automatic expiry/apply and interrupted-decision
recovery are internal daemon work and have no TCP route.

```sh
coven ward approve <id> --note "reviewed identity change"
coven ward reject <id> --note "needs revision"
coven ward approve <id> --json
```

Approval re-runs Gates 1–2 and the probes, skips the threads validator for
coherence proposals, and atomically applies only while the write's before-image
matches the re-probed baseline. Rejection audits and removes the proposal
without applying it. Missing, malformed, stale, or inconsistent evidence
returns `409` and leaves the proposal pending. A valid `failed` or `unscored`
result remains advisory and may be explicitly approved. Nothing ever
auto-approves — the principal is the sole approver (design Non-goals).

Approval and scheduler recovery also preflight every duplicated proposal
collection, then reapply the 32-edit/16 MiB Ward budget before target access.
An oversized or legacy proposal fails with `413 ward_apply_too_large`,
`writeApplied: false`, and no partial write.

A first apply also requires the exact before-image even when concurrent bytes
equal the proposal; only a durable recovery intent may accept already-applied
bytes, and recovery revalidates the persisted Gate-2-resolved surface at the
Ward's final adjudication. Clean failures clear recovery state; only a write
that may have committed remains eligible for idempotent replay. Retrying the
same CLI decision after a terminal audit is idempotent; attempting the opposite
decision fails with `proposal-already-decided`.

## Audit ledger

`coven ward audit <familiar>` reads the append-only `ward_audit` ledger for
one familiar, newest first — the Gate 4 record of what the Ward actually did
(RFC-0001 §5.6). Successfully persisted direct writes and approved proposals
append one `apply_audit` row per logged change: `diffSha256` is the post-write
content hash, and
`detail` carries `prev_sha256` and `bytes_written`. `prev_sha256` hashes the
verified regular file displaced by the atomic commit; it is null only when a
no-replace file creation succeeds. Consecutive writes to the same surface
therefore form a tamper-evident hash chain.
Proposal apply rows commit atomically with `proposal_approved`. Gate verdicts
(`validation_verdict`), proposal lifecycle events, and compaction ledger
entries land in the same table.

The durable audit budget defaults to **256 MiB** of deterministic charged rows
(exact stored field lengths plus four SQLite pages per row). A persistent
reservation is acquired under SQLite `BEGIN IMMEDIATE` while the Ward
write/audit lock is held, before staging, claiming, or changing a target.
Connections checkpoint near **4 MiB**, retain at most **16 MiB** after reset,
and fail closed when the durable **128 MiB** WAL admission ceiling cannot be
recovered because a reader pins old frames.

Exhaustion returns `507 ward_audit_capacity_exceeded` with `resource:
"ledger"` or `"wal"`, byte accounting, and `writeApplied`. New operations are
refused with `writeApplied: false`; an interrupted recovery whose target may
already be committed reports `null`, never a false rollback claim.

Audit evidence is never pruned automatically. Operator recovery is:

1. stop new Ward writes;
2. stop the daemon and take a consistent SQLite backup of
   `~/.coven/coven.sqlite3`, including committed WAL content;
3. verify the backup, using `coven ward audit <familiar> --json` only as a
   bounded human-readable view rather than a complete export;
4. retain that backup as the archive; and
5. raise `coven_ward_audit_capacity.limit_bytes` in the stopped database (or
   `wal_limit_bytes` for a deliberately larger WAL budget) before restart.

Do not delete rows from `ward_audit`; append-only triggers reject updates and
deletes.

```sh
coven ward audit sage                      # full ledger, newest first
coven ward audit sage --event apply_audit  # only applied-write records
coven ward audit sage --limit 10           # at most 10 rows
coven ward audit sage --json               # exact daemon body
```

`--json` output carries exactly the daemon body
(`GET /api/v1/familiars/{id}/audit`) per the
[observe contract](cli-observe.md). The ledger is enforced append-only by
schema triggers — rows survive daemon restarts and cannot be updated or
deleted. Reading does not require a live `ward.toml`: history stays
observable even after a familiar's Ward config is removed. Unknown familiars
fail with `familiar_not_found`.

## Migration

`coven ward migrate` inspects (and with `--apply`, rewrites) v0.1
`ward.toml` files into the Phase-2 `WardConfig` dialect. Use `--familiar
<ID>` to scope to one familiar and `--fingerprint <FPR>` to set the
principal binding. Exits non-zero if any migration fails.
