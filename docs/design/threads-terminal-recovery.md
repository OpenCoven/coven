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

On Windows, the fixture owns a foreground `coven daemon serve` child and waits
for authenticated health under a bounded startup guard. It uses the real
`daemon stop` command and replaces the process before checking restart recovery.
This covers server authority and durable recovery, not the separate two-second
CLI launcher or atomic `daemon restart` contract. Dedicated lifecycle tests
retain those contracts. Unix journeys continue to use the normal launcher.

## Terminal evidence

The central decision audit writer rejects duplicate terminal rows. It also
requires an opened-window row if and only if the decision carries typed
`window_close` evidence.

| Terminal event | Typed close reason |
| --- | --- |
| `proposal_approved` | `applied` |
| `proposal_vetoed` | `vetoed` |
| `proposal_rejected` | `evidence_diverged`, `revalidation_failed`, or `superseded` |

The `superseded` reason is part of the contract. The integrated decision layer
supports explicit supersession only after validating the replacement proposal.
See [Threads real-daemon E2E](../reference/threads-e2e.md) for supported scheduled
intake coverage; seeded historical recovery alone does not prove that path.

Deadline expiry starts revalidation. It does not authorize a write or replace
an opened window with `proposal_expired`. Human approval without a window
remains valid and does not fabricate close evidence.

## Recover without granting authority

Scheduled publication binds the exact pending-file bytes, including identity and
probe evidence, in a durable audit reservation before exposing the file. After
an interruption, scheduler and decision entry can reconstruct a missing
submission receipt only from that binding. Changed bytes cannot acquire a new
receipt through recovery, and existing duplicate or inconsistent receipts still
fail the ordinary receipt gate. Recoverable publication failures retain and log
the binding for retry. The scheduler explicitly defers those proposal IDs and
paths, so their missing receipts do not trigger quarantine and unrelated due
work, terminal cleanup, and cursor progression continue. A direct decision for
the affected proposal reports the reconstruction failure instead of claiming it.
Global database failures still fail the pass rather than masquerading as partial
recovery.

An unapplied proposal with an existing window fails closed when its familiar,
Ward configuration, or authoritative replay is confirmed absent or invalid.
Its rejection uses `revalidation_failed` and `replay_hash_matched = false`. If
live weave construction fails, recovery retains the committed window hash as
audit context. That hash is not evidence that current bytes matched.

An I/O failure reading the familiar registry or Ward configuration is not proof
of missing or invalid authority. Recovery retains the durable request, applying
state, and reservation for retry rather than closing or quarantining solely
because of that read failure. Byte reads are separate from UTF-8 decoding:
invalid UTF-8, like invalid TOML, is invalid authority content rather than a
transient I/O failure. This also applies when the legacy Ward backup is consulted.

Live promotion to a protected target still rejects before mutation. An
unavailable replay cannot turn that rejection into an indefinitely pending
window. Recovery preserves a previously recorded decision request rather than
switching verbs and conflicting with an interrupted approval.

Inconsistent human-labelled pending history with an existing window must be
rejected, never normalized into approval. Once a terminal row is durable,
repeated recovery consumes leftover pending state and reservations without
appending another terminal row or applying edits again.

For a current scheduled proposal relabelled as human-only, receipt divergence
can admit **rejection only** when the canonical original authority, complete
submission receipt, and exactly one matching opened-window record remain
provable. The reconstructed classification is never executed or persisted.
Decision-origin checks precede revision/replacement cleanup as well as
terminalization, even past retention. Invalid-origin claims are quarantined with
their bound reservations retained, not reset for a fresh automatic decision.
Any applying sidecar or durable apply intent excludes this proof, including
an intent whose sidecar was removed. Malformed or unbound receipts, openings,
or legacy lane sidecars remain corruption rather than a trusted close context.

## Quarantine is not a close

An interrupted apply with unverifiable or inconsistent committed intent is
different from an unapplied proposal. Recovery quarantines that evidence rather
than asserting that no write occurred. Quarantine does **not** satisfy the typed
terminal invariant and requires separate resolution.

If the familiar or Ward authority disappears, becomes unreadable, or cannot be
constructed, an interrupted apply leaves the automatic retry queue. Its claim
bytes and recorded apply intent remain available for manual recovery. The
response reports `terminal = false` and `manualRecoveryRequired = true`; simply
restoring the configuration does not automatically resume a quarantined apply.

Do not delete quarantined intent or classify it as a completed rejection to
make the terminal count balance. This checkpoint also does not repair arbitrary
historical database corruption or infer missing close evidence for past writes.

For deterministic scheduler deadlines, use the opt-in
[Threads test clock](threads-test-clock.md). Neither the clock nor seeded
historical recovery fixtures grant protected-write authority.
