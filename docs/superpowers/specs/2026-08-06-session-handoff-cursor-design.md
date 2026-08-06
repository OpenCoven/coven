# Session handoff cursor design

## Goal

Fix issue #613 without changing the handoff's workspace, claimant, or
generation safeguards. A handoff offered from a live source session remains
claimable and acknowledgeable when later source events extend its transcript.

## Design

Add a store helper that returns the latest event sequence for one session with
a scalar SQLite aggregate. It must return `0` when the session has no events
and must not materialize event payloads. The emit, claim, and acknowledgement
handlers use this helper instead of `list_events(...).last()`.

Treat the offered cursor as a required transcript prefix. Claim and
acknowledgement reject only when the current cursor is lower than the offered
cursor; a larger current cursor is a normal append-only extension. The store
acknowledgement guard uses the same lower-than predicate so the API and
transactional persistence boundary cannot disagree.

## Alternatives rejected

Limiting the event-list query still loads an event record and obscures that the
operation needs only a scalar. An in-memory cursor cache would add restart and
cross-process consistency risks to an answer SQLite already owns.

## Validation

Add focused store coverage for empty and latest cursors. Extend handoff API
coverage to prove output or input appended after an offer permits claim and
acknowledgement, while a lower cursor still returns `transcript_diverged`.
Retain existing workspace, claimant, and generation conflict tests.
