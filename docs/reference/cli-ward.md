---
summary: "Inspect and manage the Ward proposal lifecycle: pending reads and config migration."
read_when:
  - Looking up ward
  - Reviewing staged (held) Ward proposals
title: "coven ward"
description: "Reference for coven ward: list and inspect pending Ward proposals staged for the principal, and migrate v0.1 ward.toml files to the Phase-2 WardConfig dialect."
---

`coven ward` groups the Ward's principal-facing lifecycle verbs. Held writes
into a familiar home never dead-end: the daemon stages them at
`~/.coven/pending/` for the principal's decision, and `coven ward pending`
is the supported way to see what is waiting.

```sh
coven ward pending             # table of staged proposals, newest first
coven ward pending <id>        # one proposal in full
coven ward pending --json      # exact daemon body (GET /api/v1/threads/proposals)
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

## Migration

`coven ward migrate` inspects (and with `--apply`, rewrites) v0.1
`ward.toml` files into the Phase-2 `WardConfig` dialect. Use `--familiar
<ID>` to scope to one familiar and `--fingerprint <FPR>` to set the
principal binding. Exits non-zero if any migration fails.
