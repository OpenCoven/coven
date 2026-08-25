# Maintenance Participant Design

Issue: [OpenCoven/coven#795](https://github.com/OpenCoven/coven/issues/795)  
Downstream tracker: `cave-cgk9v`

## Problem

A Coven-launched harness holds a repository writer intent for its full lifetime.
The supervisor renews that intent every 20 seconds. When a command inside the
harness invokes `coven maintenance acquire`, the new owner enters `draining`
and waits for every existing writer, including the invoking session's own
supervisor-owned writer. That writer keeps renewing, so the wait cannot
converge.

Increasing the drain timeout does not fix the cycle. Stopping all renewals while
an owner drains would be unsafe because unrelated live sessions would disappear
from the blocker set while they can still write.

## Decision

Add generation-bound maintenance participation. A writer lease may mint a
participant capability containing its writer id and generation. Coven injects
that capability only into the harness process owned by the lease. A maintenance
command launched by that harness may present the capability automatically.

The owner records the exact participant identity. Owner phase calculation
excludes only the matching writer id and generation. Every other live writer
continues to block acquisition, and new writers remain fenced as soon as the
owner record is published.

## Protocol

### Participant capability

`WriterLease` exposes an opaque serialized capability containing:

- writer id
- writer generation

The generation is the authority-bearing value. A writer id without the matching
generation cannot participate. The capability is placed in the harness
environment as `COVEN_MAINTENANCE_PARTICIPANT`.

The value must not be printed, persisted in the session database, included in
events, or copied into user-visible diagnostics.

### Owner state

`Owner` gains an optional participant field. Serialization omits the field when
absent, and deserialization defaults it to `None`, preserving compatibility
with existing owner records.

`MaintenanceGate::acquire_owner` accepts an optional participant capability.
While holding the metadata lock it verifies that:

1. the referenced writer file exists;
2. the writer id and generation exactly match the capability;
3. the writer lease is not expired.

Malformed, missing, expired, or mismatched capabilities fail closed before an
owner record is published.

### Effective writer set

Status and owner refresh continue to read and clean all writer files. When the
current owner has a participant, the exact matching writer is separated from
the blocking `writers` array. The owner becomes `held` only when no
non-participant writers remain.

The participant remains observable through the owner record, while existing
clients that decide readiness from `owner.phase` and `writers.length` continue
to work without changes.

If the participant writer later disappears, the owner remains valid. Its
authority was proven at acquisition, and disappearance only reduces concurrent
activity. A new writer using the same id but a different generation is never
excluded.

### CLI behavior

`coven maintenance acquire` reads `COVEN_MAINTENANCE_PARTICIPANT` when present,
parses it strictly, and passes it to the gate. No new user-facing flag is
needed. An invalid inherited value is an error rather than silently falling
back to ordinary acquisition.

`heartbeat`, `status`, and `release` use the participant recorded in the owner
state. They do not depend on the environment after acquisition.

### Harness propagation

Every supervisor-owned session writer must propagate its capability to the
harness command:

- attached and detached `coven run` launch paths;
- patch sessions;
- daemon/API-launched sessions, including adopted sessions.

`HarnessCommand` receives a narrow environment-override helper. Runtime code
adds the capability immediately before spawn. The capability is never added to
stored launch metadata.

## Error handling

- Invalid capability encoding: reject maintenance acquisition.
- Missing writer file: reject as a stale participant.
- Generation mismatch: reject as a stale or forged participant.
- Expired writer: remove it through normal cleanup and reject participation.
- Other active writers: remain in `writers`; acquisition stays `draining`.
- Owner or writer state corruption: retain existing fail-closed behavior.

## Compatibility

- Existing sessions without the environment capability retain current behavior.
- Existing clients can deserialize the extended owner object because fields are
  additive.
- Existing owner records deserialize with no participant.
- The JSON `writers` array remains the blocking writer set, matching the
  semantics already consumed by Cave.

## Tests

Rust unit tests will prove:

1. a live writer can participate and let its owner reach `held`;
2. a second live writer still blocks that owner;
3. wrong, expired, and missing generations fail closed;
4. a replacement writer with the same id is not excluded;
5. owner heartbeat preserves participant exclusion;
6. old owner JSON without a participant still deserializes;
7. harness command construction propagates the capability without logging or
   persistence.

CLI integration tests will run a fake harness inside `coven run`, invoke
maintenance acquisition from that process, and verify the owner reaches `held`
while the supervisor writer remains live.

## Delivery

The implementation lands in `OpenCoven/coven` through issue #795 and a protected
pull request. Cave adoption must require the first released Coven version that
contains this protocol before removing its last-resort guidance for
`cave-cgk9v`.
