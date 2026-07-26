---
summary: "Inspect and manage the Ward proposal lifecycle: pending reads, the audit ledger, and config migration."
read_when:
  - Looking up ward
  - Reviewing staged (held) Ward proposals
  - Auditing applied Ward writes
title: "coven ward"
description: "Reference for coven ward: list and inspect pending Ward proposals staged for the principal, read the append-only ward_audit ledger, and migrate v0.1 ward.toml files to the Phase-2 WardConfig dialect."
---

`coven ward` groups the Ward's principal-facing lifecycle verbs. Held writes
into a familiar home never dead-end: the daemon stages them at
`~/.coven/pending/` for the principal's decision, and `coven ward pending`
is the supported way to see what is waiting.

```sh
coven ward pending             # table of staged proposals, newest first
coven ward pending <id>        # one proposal in full
coven ward pending --json      # exact daemon body (GET /api/v1/threads/proposals)
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

Decisions are daemon-API verbs today
(`POST /api/v1/threads/proposals/<id>/approve|reject` — see
[api](api.md)); approving a `coherence` proposal stays fail-closed until
Gate 3's resolution stage lands. Nothing ever auto-approves — the principal
is the sole approver (design Non-goals).

## Audit ledger

`coven ward audit <familiar>` reads the append-only `ward_audit` ledger for
one familiar, newest first — the Gate 4 record of what the Ward actually did
(RFC-0001 §5.6). Every applied write through `POST /familiars/{id}/edits`
persists one `apply_audit` row per logged change: `diffSha256` is the
post-write content hash, and `detail` carries `prev_sha256` (null on file
creation) and `bytes_written`, so consecutive writes to the same surface form
a tamper-evident hash chain. Gate verdicts (`validation_verdict`), proposal
lifecycle events, and compaction ledger entries land in the same table.

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
