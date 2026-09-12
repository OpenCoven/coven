---
summary: "Keep opened Threads veto windows tied to one typed terminal outcome."
read_when:
  - Changing Threads proposal decisions or startup recovery
title: "Threads terminal recovery"
description: "Typed close evidence, replay failures, and the boundary between rejection and quarantine."
source_adjacent_reason: "Documents the terminal audit guard and proposal recovery policy."
---

# Threads terminal recovery

An opened Threads veto window requires one typed terminal outcome, so you can
distinguish an applied change, a veto, and a failed revalidation in the audit.

```sh
cargo test --locked -p coven-cli --test threads_terminal_recovery
```

This target runs the actual daemon. Its historical-window fixtures seed audit
state only while the daemon is stopped. They exercise recovery, not a supported
scheduled-proposal publication route.

## Terminal evidence

The central decision audit writer rejects duplicate terminal rows. It also
requires an opened-window row if and only if the decision carries typed
`window_close` evidence.

| Terminal event | Typed close reason |
| --- | --- |
| `proposal_approved` | `applied` |
| `proposal_vetoed` | `vetoed` |
| `proposal_rejected` | `evidence_diverged`, `revalidation_failed`, or `superseded` |

The `superseded` reason is part of the contract. This checkpoint does not add a
production supersession or scheduled-publication path.

Deadline expiry starts revalidation. It does not authorize a write or replace
an opened window with `proposal_expired`. Human approval without a window
remains valid and does not fabricate close evidence.

## Recover without granting authority

An unapplied proposal with an existing window fails closed when its familiar,
Ward configuration, or authoritative replay becomes unavailable. Its rejection
uses `revalidation_failed` and `replay_hash_matched = false`. If live weave
construction fails, recovery retains the committed window hash as audit
context. That hash is not evidence that current bytes matched.

Live promotion to a protected target still rejects before mutation. An
unavailable replay cannot turn that rejection into an indefinitely pending
window. Recovery preserves a previously recorded decision request rather than
switching verbs and conflicting with an interrupted approval.

Inconsistent human-labelled pending history with an existing window must be
rejected, never normalized into approval. Once a terminal row is durable,
repeated recovery consumes leftover pending state and reservations without
appending another terminal row or applying edits again.

## Quarantine is not a close

An interrupted apply with unverifiable or inconsistent committed intent is
different from an unapplied proposal. Recovery quarantines that evidence rather
than asserting that no write occurred. Quarantine does **not** satisfy the typed
terminal invariant and requires separate resolution.

Do not delete quarantined intent or classify it as a completed rejection to
make the terminal count balance. This checkpoint also does not repair arbitrary
historical database corruption or infer missing close evidence for past writes.

For deterministic scheduler deadlines, use the opt-in
[Threads test clock](threads-test-clock.md). Neither the clock nor seeded
historical recovery fixtures grant protected-write authority.
