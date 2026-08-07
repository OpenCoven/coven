# Psyche G2 Atomic Termination Persistence Design

**Status:** Approved design for issue #652

**Scope:** Correct the G2 coordinator contract so Psyche never returns a
successful Coven termination disposition before the corresponding validated
outcome revision is durable.

## Problem

The G2 plan currently makes `TerminationRequested` durable before invoking
`CovenPort::terminate`, but the returned acknowledged or unresolved disposition
can escape without a coordinator-level proof that its outcome revision was
validated and committed. Store-level append tests alone do not close this gap:
the executable coordinator path must own the validate, derive, persist, and
attest sequence.

This design uses "atomic" to mean that callers cannot observe a successful
termination result without its durable Psyche outcome. It does not claim a
distributed transaction across Psyche and Coven.

## Required State Machine

For one execution attempt, termination advances the append-only binding ledger:

```text
session-bound -> termination-requested -> acknowledged | unresolved
```

The following invariants are mandatory:

1. Every successor increments the revision number by one and names the exact
   canonical digest of its predecessor.
2. Session, execution correlation, termination identity, termination reason,
   and termination authority window become frozen no later than the
   `termination-requested` revision.
3. An acknowledged or unresolved revision can only be derived from the exact
   durable `termination-requested` predecessor.
4. Exact replay is idempotent. A changed response for the same termination
   identity is a revision conflict.
5. A gap, fork, historical rewrite, rebinding, authority removal, or intervening
   revision fails without mutation.
6. `Ok(TerminationDisposition)` implies the matching terminal revision is
   durable and byte-exact after reopening storage.

## Components and Boundaries

### Validated request construction

`TerminationRequest` remains construction-closed. The coordinator accepts a
candidate `ExecutionBinding`, validates that it is a
`TerminationRequested` successor of an already session-bound revision, durably
appends it, and compares the store's committed canonical bytes with the
candidate bytes. The port is not called if any precondition, append, or
attestation fails.

### Termination coordinator

`persist_then_terminate` owns the complete ordering:

1. Validate the requested revision.
2. Durably append and attest the exact `TerminationRequested` bytes.
3. Construct the read-only `TerminationRequest`.
4. Invoke `CovenPort::terminate` with the persisted termination identity as its
   idempotency identity.
5. Validate the returned acknowledgement or unresolved evidence against the
   persisted termination authority.
6. Derive the one legal next digest-linked revision.
7. Durably append and attest the exact outcome bytes.
8. Return the disposition.

No adapter may append the outcome as a best-effort side effect, and no caller
may bypass the coordinator with an unchecked request or outcome constructor.

### Outcome derivation

`derive_termination_outcome_revision` accepts the durable requested revision and
the port disposition. It validates the termination request ID, Coven session,
immutable execution correlation, authority-evidence digest, and persisted
termination window. Acknowledgement timestamps must fall within the inclusive
window. Unresolved evidence is bound to the same authority and may not derive a
new deadline from response arrival time.

The function preserves all frozen fields, increments the revision once, and
sets `previous_revision_digest` to the canonical digest of the requested
revision.

### Persistence boundary

`TerminationPersistence` exposes separate requested and outcome append
operations. Both operations:

- validate the exact durable predecessor;
- append transactionally;
- treat an exact historical replay as idempotent;
- reject every changed same-revision replay, fork, gap, or intervening
  successor as a conflict; and
- return the committed RFC 8785 canonical bytes only after durable commit or
  proof of exact replay.

Persistence failures must distinguish a revision conflict from a storage/write
failure so the coordinator can preserve the approved error semantics.

### Restart reconciler

Startup reconciliation finds durable attempts whose tip is
`TerminationRequested`. It reconstructs the same construction-closed request,
replays `CovenPort::terminate` with the same termination identity, validates the
stable response, and appends the missing outcome.

The important crash windows are:

| Crash point | Durable state | Recovery |
|---|---|---|
| Before requested append | Session-bound only | No port call occurred; retry from the predecessor |
| After requested append, before port call | Termination requested | Replay the same port request |
| After port response, before outcome append | Termination requested | Replay the same idempotency identity and append the stable response |
| After outcome append, before caller observes success | Terminal outcome | Exact replay proves the same revision; no duplicate is appended |

A different replay response, a changed authority field, or an intervening
revision produces an explicit conflict. Recovery never invents a successful
outcome and never replaces ambiguity with a success-shaped fallback.

## Error Contract

The coordinator exposes phase-specific failures:

| Category | Durable effect | Port call | Returned success |
|---|---|---|---|
| Invalid request | None | No | No |
| Requested-revision persistence failure | No requested revision | No | No |
| Port failure | Requested revision remains | Yes | No |
| Invalid or mismatched response evidence | Requested revision remains | Yes | No |
| Outcome persistence failure | Requested revision may remain without an outcome | Yes | No; report indeterminate outcome persistence |
| Revision conflict | Existing chain is preserved | Depends on conflict point | No; report explicit ambiguity |
| Persisted-byte attestation mismatch | Store state is not trusted | Depends on phase | No |

Request persistence, outcome persistence, and revision conflict must not collapse
into one generic persistence variant. In particular, a failure after Coven has
responded is reported as indeterminate outcome persistence because the caller
must not infer that Coven did nothing.

## Concurrency and Replay

The requested revision's digest is the compare-and-append authority for the
outcome. If another writer commits any successor first, an outcome append based
on the old tip fails as a conflict. The coordinator does not reload and
silently rebase.

An exact repeated request and stable disposition return the already-durable
outcome. A different disposition under the same termination request ID cannot
create another revision with the same predecessor and is reported as a
conflicting replay. Historical exact replay remains valid even when a later
revision exists; historical changed bytes remain invalid.

## Verification

The G2 exact-test manifest and evidence matrix must name executable tests for:

- acknowledged success;
- unresolved success;
- exact replay;
- crash after port response but before outcome append;
- restart recovery from the durable requested revision;
- conflicting replay response;
- mismatched or invalid evidence;
- outcome-write failure;
- concurrent intervening revision; and
- requested- and outcome-byte attestation mismatches.

Every success test reopens the SQLite store and proves that the terminal
revision is durable, byte-exact, digest-linked to the requested revision, and
preserves all frozen correlation fields. The crash/restart test uses a
deterministic test-support fault point immediately after the validated port
response and before the outcome append, then creates a fresh coordinator over
the reopened store and stable fake port.

Negative tests prove no illegal terminal revision was appended and no
success-shaped result escaped. Manifest/listing validation must reject a
missing exact test, an unused manifest entry, or a zero-test filter.

## Non-goals

This correction does not implement Psyche crates, change Coven's O5 protocol,
add multi-agent cancellation policy, add Telegram behavior, or define
user-facing retry UX. It only closes the G2 coordinator persistence and recovery
contract needed before Psyche implementation begins.

## Acceptance Criteria

Issue #652 is design-complete when the G2 plan:

1. makes the validate-derive-append sequence part of the public coordinator
   contract;
2. makes successful outcome durability byte-attested and reopen-verifiable;
3. specifies same-identity replay and conflicting-response behavior;
4. specifies restart reconciliation for every crash window;
5. preserves phase-specific errors, including indeterminate outcome
   persistence and explicit revision conflict; and
6. names every required exact test in the G2 manifest and evidence matrix.
