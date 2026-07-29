---
summary: "Inspect and manage the Ward proposal lifecycle: pending reads, principal decisions, the audit ledger, and config migration."
read_when:
  - Looking up ward
  - Reviewing staged (held) Ward proposals
  - Approving or rejecting a Ward proposal
  - Auditing applied Ward writes
title: "coven ward"
description: "Reference for coven ward: inspect, approve, or reject pending Ward proposals, read the append-only ward_audit ledger, and migrate v0.1 ward.toml files to the Phase-2 WardConfig dialect."
---

`coven ward` groups the Ward's principal-facing lifecycle verbs. Held writes
into a familiar home never dead-end: the daemon stages them at
`~/.coven/pending/` for the principal's decision, and `coven ward pending`
is the supported way to see what is waiting.

```sh
coven ward pending             # table of staged proposals, newest first
coven ward pending <id>        # one proposal in full
coven ward pending --json      # exact daemon body (GET /api/v1/threads/proposals)
coven ward approve <id>        # re-validate and atomically apply
coven ward reject <id> [--note "reason"] # reject without applying
coven ward audit <familiar>    # append-only ward_audit ledger, newest first
coven ward migrate --apply     # migrate v0.1 ward.toml files to Phase-2
```

## Pending proposals

Two lanes stage here, distinguished by `reviewKind`:

- `authority` — a Tier-0 (protected) write whose thread frayed
  (`DegradeToProposal`, coven-threads §5).
- `coherence` — a Tier-1 (reviewed) write held for Gate-3 coherence review
  (`docs/design/ward-gate3-coherence.md`).

`--json` output carries exactly the daemon body's data (pretty-printed) per
the [observe contract](cli-observe.md). Unparseable pending files appear as
`degraded` entries instead of aborting the read. Unknown ids fail with
`proposal_not_found`.

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
(RFC-0001 §5.6). Direct writes and approved proposals persist one `apply_audit`
row per logged change: `diffSha256` is the post-write content hash, and
`detail` carries `prev_sha256` (null on file creation) and `bytes_written`, so
consecutive writes to the same surface form a tamper-evident hash chain.
Proposal apply rows commit atomically with `proposal_approved`. Gate verdicts
(`validation_verdict`), proposal lifecycle events, and compaction ledger
entries land in the same table.

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
