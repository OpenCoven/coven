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
This covers server authority and durable recovery, not the separate
[CLI lifecycle deadlines](../reference/threads-e2e.md#lifecycle-deadlines).
Dedicated lifecycle tests retain those contracts, including the two-second
standalone stop bound. Unix journeys continue to use the normal launcher.

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

Scheduled publication binds the exact pending-file bytes, including identity,
AUTO regression, and probe evidence, in a durable audit reservation before
exposing the file. After
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

Normal publication and receipt reconstruction share the same canonical detail:
the original `identity_evidence` and, for AUTO, `autoRegressionEvidence`. Recovery
copies these commitments from the bound body; it never recomputes original
authority from current configuration. An absent or null AUTO field is compatible
only for non-AUTO receipts, without bypassing historical identity-proof bounds.

Legacy-format intake also records a proposal-bound `proposal_submitted` row,
for both authority and coherence review. Before creating a decision request or
reservation, and again before execution, the daemon requires exactly one
matching legacy receipt. It checks the familiar, thread, targets, channel,
staging times, review lane, and the legacy tier/hash representation. A scheduled
receipt cannot authorize an envelope relabelled as legacy. The authority lane
remains unclassified at publication; its receipt never overrides protected
floors or grants approval.

Legacy receipts do not provide the scheduled protocol's exact-body recovery
binding. If legacy publication succeeds but its audit insert fails, the pending
file cannot be approved later or acquire an invented expiry terminal. The
daemon quarantines an unapplied orphan without first rewriting its request.
Existing applying state or any durable apply intent instead remains available
for explicit review, with its reservation and bytes preserved. Database errors
remain operational errors, not evidence for quarantine. Genuine matching legacy
receipts and their historical recovery continue through the existing decision
path; unknown review lanes retain the existing corrupt-envelope handling.

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

A missing or changed private AUTO commitment can likewise justify rejection,
never approval, when the complete original receipt still passes fresh regression
replay and any window has exactly one fully matching opening. A no-window AUTO
refusal does not invent a typed close. This proof does not rewrite the executable
envelope or receipt, and excludes applying state and orphaned durable apply
intent. Malformed, duplicate, misbound, or unreplayable original evidence stays
outside this exception. Origin validation still precedes cleanup and rejection.
Once the AUTO commitments contradict, confirmed missing authority or failed
original replay is invalid submission evidence, not ordinary expiry eligibility.
Quarantine preserves the original request and reservation; retention must not
replace them with a new server expiry request. Genuine I/O errors remain
retriable, and an already-bound expiry transition retains its separate recovery
path.

A failed retained Ward-file guard can consult the final-authority callback to
preserve the typed refusal cause. This is error classification only: it remains
a proven no-write refusal even if the callback succeeds, never a retry that
bypasses the retained-file guard.

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

## Deployed-history classification boundary

Startup and decision recovery classify retained pending items; they do not
perform a census of every historical `ward_audit` opening. The separate operator
command below inventories that history without changing recovery behavior.

| Evidence available | Existing recovery disposition |
| --- | --- |
| Matching submission, one opening, no apply intent, confirmed invalid authority | Typed `revalidation_failed` rejection; consume the pending item only after the terminal is durable. |
| Human-labelled envelope contradicts its opening, but original receipt and opening are fully provable and unapplied | Reconstruct rejection context only; never execute or rewrite it as approved history. |
| Transient authority read failure | Preserve request, claim, and reservation for retry; an I/O error is not proof of invalid authority. |
| Existing valid terminal plus leftover pending item | Consume leftover state idempotently without another write or terminal. |
| Interrupted apply, corrupt/misbound receipt, unprovable original opening, or invalid decision origin | Preserve/quarantine evidence for explicit recovery; no fabricated no-write assertion or close. |
| Audit opening without any retained pending/claim item | Outside the pending-item classifier; not automatically repaired or certified closed. |

### Read-only operator census

```sh
coven ward audit-census --json
coven ward audit-census --max-audit-rows 100000 --json
```

This offline-capable command reads the selected `COVEN_HOME/coven.sqlite3`
without starting a daemon, initializing a profile, installing/migrating a schema,
or consulting the current familiar registry. Deleted familiars and openings
without retained proposals therefore remain visible. Only the exact canonical
`current_v020` audit schema is accepted; missing, legacy, or unknown schemas
produce an explicit error, not a successful empty report.

The JSON format is `coven.ward-window-census.v1`. `complete: true` and exit status
zero mean the **bounded audit inventory completed**, not that every window is
closed or that engineering/release acceptance has been granted. Inspect
`unresolvedHistories`, each `classification`, and its `issues`. The report includes
audit row ids and proposal UUIDs, not private detail text, familiar declarations,
target contents, approval rationale, or proposal bodies.

| Classification | Meaning and permitted next action |
| --- | --- |
| `typed_terminal_recorded` | Exactly one opening and one core-valid typed terminal, with consistent familiar/target context, compatible available channels, and append order. This is not independent proof of original submission authority, replay inputs, or applied bytes. Existing recovery may consume retained leftovers idempotently. |
| `open_unverified` | No terminal; a pending/claim filename was observed. Its contents, eligibility, and authority were not validated. Use existing daemon recovery only under its own receipt and revalidation gates. |
| `quarantined_opening` / `untrusted_artifacts` | No terminal; only quarantined or untrusted matching names were observed. Preserve evidence for explicit recovery; do not promote these artifacts into the active queue. |
| `orphaned_opening` | No terminal or recognized matching artifact name was observed. This is outside automatic per-item recovery; preserve the audit snapshot and investigate missing or unattributed proposal evidence. |
| `unprovable_apply` | Apply intent exists without a terminal. Neither current bytes nor an absent claim proves that no write occurred. Preserve intent and before-image evidence for explicit recovery. |
| `inconsistent_history` | Invalid records, missing typed closes, duplicate openings/terminals, reverse append order, apply intent outside its opening/terminal interval, or contradictory scope. Preserve the original rows; do not manufacture an approval or balancing rejection. |

Audit reads and the canonical schema fingerprint share one SQLite read
transaction. `throughAuditId` identifies its high-water mark. Pending and
quarantine observations are a **separate, non-atomic filename census**; they
never establish submission or execution authority. They do not read proposal
bodies or intentionally follow symlink entries. For a stable operational packet,
stop the daemon using its supported command and preserve a consistent SQLite
backup together with pending, quarantine, and intent evidence before inspection.
Ordinary SQLite reads may use WAL/SHM sidecars; read-only here means no audit,
schema, proposal, or authority mutations, not zero filesystem metadata activity.

The default bound is 10,000 relevant audit rows, configurable up to 100,000.
Relevant rows are openings, normative terminals, historical expiry tags, and
durable apply intents, including human no-window terminals (which are excluded
from the resulting opened-window histories). Decoded data is capped at 32 MiB
and filesystem enumeration at 4,096 entries. Exceeding any bound, an inaccessible
artifact directory, or a database read failure aborts without publishing a
partial `complete` report. There is no paging or repair mode. A larger history
requires separately scoped inventory work; do not certify it from a failed scan.

A deployed-history migration must first inventory unresolved classes and
preserve the original database and pending/intent evidence. Do not update old
rows, infer approval from final file bytes, or append rejection merely to make
counts balance. Publication of this diagnostic is not evidence that a deployed
census or manual resolution has run. Unprovable interrupted apply and orphaned
history still require evidence-backed disposition; this remaining migration
obligation is separate from the supported nine-case terminal matrix and keeps
the universal historical closure claim open. Rollback is to stop using the
diagnostic; it installs no schema or persistent state to undo.

For deterministic scheduler deadlines, use the opt-in
[Threads test clock](threads-test-clock.md). Neither the clock nor seeded
historical recovery fixtures grant protected-write authority.
