# Spoke Result Ledger Design

## Context

Issue #267 already provides the shared `coven.executor.v1` protocol, outbound
SSH/local dispatch, hub-initiated health polls, normalized result envelopes,
and durable latest-state dispatch records. The migrated Phase 1B acceptance
criteria additionally require result envelopes to be append-only and safe to
replay. The current `executor_dispatches` row is keyed by `job_id` and updates
`envelope_json` in place, so it is a useful projection but not an immutable
history.

## Decision

Keep `executor_dispatches` as the backward-compatible latest-state projection
and add `executor_result_envelopes` as the authoritative append-only ledger.
Each ledger entry contains:

- a deterministic envelope ID derived from the node ID and canonical serialized
  envelope;
- the job and node IDs;
- the exact normalized envelope JSON; and
- the hub recording timestamp.

The deterministic ID makes replay idempotent: inserting the same envelope for
the same node is a complete no-op, including its derived projections, while a
different attempt or result remains a distinct entry. Rows are inserted with
conflict-ignore semantics and are never updated or deleted by the spoke
protocol.

## Persistence Flow

The hub writes the pre-dispatch projection before starting transport, preserving
evidence if the hub exits during SSH execution. Once a normalized envelope is
available, one SQLite transaction:

1. appends the envelope to the immutable ledger;
2. updates the latest-state dispatch projection;
3. advances a matching hub job when the result is terminal;
4. updates node availability and its last error; and
5. holds or resumes assigned jobs and refreshes the persistent node subqueue.

Replaying an envelope does not append a duplicate ledger entry or reapply older
state over newer projections. Existing databases backfill each non-null legacy
`executor_dispatches.envelope_json` as one `legacy:<job_id>` ledger row during
store initialization.

## API Compatibility

`GET /api/v1/hub/dispatches/:jobId` retains its existing `envelope` field and
adds `resultEnvelopes`, ordered by append sequence. Existing clients continue to
read the latest projection; audit and recovery clients can replay the immutable
history.

## Failure Semantics

Transport failures remain normalized as `transport_error` envelopes and are
appended like successful results. A failed poll or dispatch marks the node
unavailable without deleting or reassigning hub work. SQLite transaction
failure returns an error and leaves the pre-dispatch evidence intact; it cannot
partially append a result while omitting its hub-state projection.

## Verification

Store tests cover persistence across reopen, duplicate replay, distinct
envelopes for one job, and legacy backfill. Hub tests cover result history in
the dispatch API, replay idempotency, and unchanged node/job failure behavior.
The existing executor protocol integration tests continue to cover stateless
probe and run-job behavior.
