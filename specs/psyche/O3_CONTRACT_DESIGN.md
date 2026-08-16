# Psyche O3 Request Adoption Contract Design

**Status:** Approved 2026-08-15 for test-first implementation planning. Not
implemented.

**Depends on:** O2, merged in PR #732 at
`a45cbbfac8d8be6a02c52cf06982c78dc4854a53` (issue #728).

**Scope:** O3 only, per `specs/psyche/COVEN_W1_AUDIT.md` §8: stable,
durable adoption for bound session launch and input requests, plus launch-only
one-attempt/one-session uniqueness. This design resolves C-S4 and C-M2 at the
Coven contract boundary and supplies the durable idempotent-adoption substrate
for C-M3. Full C-M3 lost-response recovery still depends on O4 lookup/fencing
and O7 crash recovery. This design does not authorize production changes until
an exact-file, test-first child plan is approved.

## 1. Purpose

O2 gives Coven an immutable, opaque `psyche.execution_binding.v1` tuple and
exact mismatch detection, but it deliberately permits two sessions with
identical bindings and cannot identify a repeated side-effect request. A lost
launch or input response can therefore tempt a caller to submit the operation
again and execute it twice.

O3 adds one durable request-adoption primitive. Before a bound launch or input
can cross the runtime side-effect boundary, Coven records a caller-supplied
stable key and digest against the exact O2 binding. Reuse of the same complete
identity returns the existing adoption without invoking the runtime or writing
another event. Reuse with any conflicting identity is rejected. Launch
adoptions also reserve an exact attempt scope so changing the adoption key
cannot create a second session for the same Psyche attempt.

Adoption means that Coven durably accepted responsibility for a request before
attempting its side effect. It does not mean that the runtime action completed,
that an input was delivered, or that an ambiguous post-adoption crash has been
reconciled. O4 owns authoritative lookup and return-or-fence dispositions; O7
owns the cross-phase crash matrix.

## 2. Decision summary

### 2.1 Contract identity and field names

The contract is `psyche.request_adoption.v1`.

- Bound launch and input requests carry it under `requestAdoption`.
- Adopted launch uses `POST /api/v1/adopted-sessions`.
- Adopted input uses
  `POST /api/v1/sessions/:id/adopted-input`.
- The closed object carries its own `contract`, `key`, and `requestDigest`.
- Health advertises accepted versions under
  `capabilities.requestAdoptionContracts`.
- The object is request metadata only. It is stripped before
  `SessionRuntime::launch_session`, `SessionRuntime::send_input`, event
  capacity checks, and persisted session events.
- O3 adds no lookup endpoint and no public adoption-record response object.
  HTTP status distinguishes a first adoption from an exact replay.

The JSON shape is:

```json
{
  "contract": "psyche.request_adoption.v1",
  "key": "opaque-stable-request-key",
  "requestDigest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
```

The object is an exact closed set: all three members are required and unknown
members are rejected.

### 2.2 Syntax and byte semantics

| Field | Rule |
|---|---|
| `contract` | Must equal `psyche.request_adoption.v1` byte-for-byte. |
| `key` | 1 to 255 ASCII bytes matching `[A-Za-z0-9._:/-]`. |
| `requestDigest` | Exactly `sha256:` followed by 64 lowercase hexadecimal characters. |

Coven performs no trimming, case folding, Unicode normalization, or semantic
interpretation. Accepted values are stored and compared byte-for-byte.

Psyche owns canonical request serialization and digest computation. Coven
validates digest syntax and equality only; O3 is not an authentication or
content-attestation contract.

### 2.3 Where adoption is accepted

O3 adoption is required for every bound launch and bound input after the
daemon advertises `psyche.request_adoption.v1`:

- The O3 adopted-launch route requires valid `executionBinding` and
  `requestAdoption`.
- The O3 adopted-input route requires both the exact O2 binding proof and a
  valid `requestAdoption`.
- On an O3 daemon, the legacy launch/input routes reject bound requests with
  `request_adoption_required`; bound O3 operations cannot bypass attempt or
  request uniqueness through an older route.
- Existing unbound operations retain their current behavior and reject
  `requestAdoption`; they receive no O3 idempotency guarantee.
- A daemon migration reserves the attempt scope of every existing bound
  session before O3 is advertised. Existing duplicate scopes make startup fail
  closed with an integrity error and require operator reconciliation; the
  migrator never chooses a winner.
- Psyche conformance clients must negotiate
  `psyche.request_adoption.v1` and use it on every launch and input. They may
  not silently fall back to an unadopted mutation.
- Dedicated O3 routes are a downgrade discriminator: a pre-O3 daemon returns
  its normal unknown-route response instead of ignoring adoption metadata and
  executing an unadopted side effect.
- Kill is excluded. O5 owns cancellation acknowledgement and its durable
  unresolved disposition.
- External session registration rejects `requestAdoption` at all.

### 2.4 Launch digest relationship

For a launch, `requestAdoption.requestDigest` must exact-match
`executionBinding.requestDigest`. The duplicated location is intentional:

- O2 keeps the immutable attempt/session binding self-contained.
- O3 keeps every adopted request record self-describing.
- Equality prevents the launch adoption from naming a different request than
  the binding persisted on the resulting session.

For input, `requestAdoption.requestDigest` identifies that input request and is
independent of the immutable launch digest inside `executionBinding`.

## 3. Adoption identity and conflict rules

### 3.1 Global request key

`requestAdoption.key` is globally unique within one Coven store across
operation kinds, sessions, projects, and contract versions. A key cannot be
reused for a launch and an input, for two sessions, or for two different
requests.

An existing record is an exact replay only when all stored identity members
match:

- contract;
- operation kind (`launch` or `input`);
- request digest;
- session id for input;
- complete byte-exact execution binding; and
- for launch, the complete attempt scope in §3.2.

The submitted request payload is not stored or interpreted by the adoption
ledger. Psyche's digest is the opaque content identity. A caller that supplies
the same digest for different content violates the contract; O3 does not make
Coven a canonical request serializer.

Any difference is `request_adoption_conflict`. Coven never overwrites or
repurposes an existing key.

### 3.2 Launch attempt scope

Launch adoptions have a second uniqueness constraint independent of the
caller-supplied key. The exact attempt scope is:

```text
executionBinding.principalRef
executionBinding.projectDigest
executionBinding.graphId
executionBinding.nodeId
executionBinding.attemptId
```

Those five byte-exact fields are the complete O3 attempt identity. `attemptId`
is intentionally scoped by principal, project, graph, and node; it is not
assumed globally unique. The scope
deliberately excludes `requestDigest`, familiar fields, parent fields, and
delegation digest: changing any of those values must conflict for the same
attempt rather than create a second session. A different `attemptId` may adopt
a new session under a new key.

If the same attempt scope already exists:

- the same key and exact complete identity is an idempotent replay;
- a different key is `request_adoption_conflict`;
- the same key with a different binding or digest is
  `request_adoption_conflict`.

This closes the Coven side of C-M2. Psyche still owns graph and attempt meaning;
Coven only applies exact tuple equality and uniqueness.

### 3.3 Child adoption

Child launches use the same request object and attempt-scope rule. The complete
O2 child binding, including `parent` and `delegationDigest`, is stored on the
adoption record and exact-compared on replay. O3 adds no graph traversal,
delegation authorization, descendant enumeration, or production child
dispatch.

## 4. Persistence

### 4.1 Dedicated append-only ledger

Adoption records live in a dedicated `request_adoptions` table rather than on
the session row. A session-column-only design is rejected because Coven's
explicit `sacrifice` operation deletes session rows and would erase the only
deduplication proof.

The logical schema is:

```sql
CREATE TABLE request_adoptions (
    id TEXT PRIMARY KEY NOT NULL,
    adoption_key TEXT,
    contract TEXT,
    operation TEXT NOT NULL CHECK (operation IN ('launch', 'input')),
    request_digest TEXT NOT NULL,
    session_id TEXT NOT NULL,
    execution_binding_json TEXT NOT NULL,
    principal_ref TEXT,
    project_digest TEXT,
    graph_id TEXT,
    node_id TEXT,
    attempt_id TEXT,
    adopted_at TEXT NOT NULL,
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE RESTRICT,
    CHECK (
        (adoption_key IS NULL AND contract IS NULL AND operation = 'launch')
        OR
        (adoption_key IS NOT NULL AND contract IS NOT NULL)
    ),
    CHECK (
        (operation = 'launch'
            AND principal_ref IS NOT NULL
            AND project_digest IS NOT NULL
            AND graph_id IS NOT NULL
            AND node_id IS NOT NULL
            AND attempt_id IS NOT NULL)
        OR
        (operation = 'input'
            AND adoption_key IS NOT NULL
            AND principal_ref IS NULL
            AND project_digest IS NULL
            AND graph_id IS NULL
            AND node_id IS NULL
            AND attempt_id IS NULL)
    )
);

CREATE UNIQUE INDEX request_adoptions_key
ON request_adoptions(adoption_key)
WHERE adoption_key IS NOT NULL;

CREATE UNIQUE INDEX request_adoptions_launch_attempt
ON request_adoptions(
    principal_ref,
    project_digest,
    graph_id,
    node_id,
    attempt_id
)
WHERE operation = 'launch';

CREATE UNIQUE INDEX request_adoptions_launch_session
ON request_adoptions(session_id)
WHERE operation = 'launch';

-- Non-unique, covers *all* rows (launch and input) so retention preflight
-- (`session_has_request_adoption`) and the `sessions(id)` ON DELETE RESTRICT
-- foreign key check can seek instead of scanning the append-only ledger.
CREATE INDEX request_adoptions_session
ON request_adoptions(session_id);

ALTER TABLE events ADD COLUMN request_adoption_id TEXT
    REFERENCES request_adoptions(id) ON DELETE RESTRICT;

CREATE UNIQUE INDEX events_request_adoption
ON events(request_adoption_id)
WHERE request_adoption_id IS NOT NULL;

CREATE TRIGGER events_request_adoption_integrity
BEFORE INSERT ON events
WHEN NEW.request_adoption_id IS NOT NULL
BEGIN
    SELECT CASE WHEN
        NEW.kind != 'input'
        OR NOT EXISTS (
            SELECT 1
            FROM request_adoptions
            WHERE id = NEW.request_adoption_id
              AND operation = 'input'
              AND session_id = NEW.session_id
        )
    THEN RAISE(ABORT, 'invalid request adoption event correlation')
    END;
END;

CREATE TRIGGER events_request_adoption_update_integrity
BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
WHEN NEW.request_adoption_id IS NOT NULL
BEGIN
    SELECT CASE WHEN
        NEW.kind != 'input'
        OR NOT EXISTS (
            SELECT 1
            FROM request_adoptions
            WHERE id = NEW.request_adoption_id
              AND operation = 'input'
              AND session_id = NEW.session_id
        )
    THEN RAISE(ABORT, 'invalid request adoption event correlation')
    END;
END;

CREATE TRIGGER events_request_adoption_no_rebind
BEFORE UPDATE OF session_id, kind, request_adoption_id ON events
WHEN (OLD.request_adoption_id IS NOT NULL OR NEW.request_adoption_id IS NOT NULL)
  AND (
      NEW.request_adoption_id IS NOT OLD.request_adoption_id
      OR NEW.session_id IS NOT OLD.session_id
      OR NEW.kind IS NOT OLD.kind
  )
BEGIN
    SELECT RAISE(ABORT, 'request adoption event correlation is immutable');
END;

CREATE TRIGGER request_adoptions_no_update
BEFORE UPDATE ON request_adoptions
BEGIN
    SELECT RAISE(ABORT, 'request adoptions are immutable');
END;

CREATE TRIGGER request_adoptions_no_delete
BEFORE DELETE ON request_adoptions
BEGIN
    SELECT RAISE(ABORT, 'request adoptions are retained');
END;
```

The migration creates one launch reservation for each pre-O3 bound
session. A reservation has the same launch columns and exact binding but a
null external adoption key because no caller key existed historically. It
occupies the attempt-scope unique index, prevents a second session, and is not
replay-addressable. New bound launches can never omit the caller key.

For launch rows, every attempt-scope column is non-null. For input rows, every
attempt-scope column is null. Store helpers validate this cross-field rule
before insertion and strict readback rejects corrupt rows.

`execution_binding_json` is deterministic O2 serialization. It lets replay
comparison remain exact without depending on caller input and gives O4 a
durable adopted-request foundation. It does not create an O4 lookup API.

The event column is internal correlation metadata. It is never serialized in
the event payload or sent to the harness. The trigger rejects cross-session,
launch-adoption, and non-input-event correlation on insert or update even from
raw SQL. Separate triggers make adoption identity immutable and retained. The
migration may need to rebuild an existing SQLite table to add the foreign key
safely; the child plan must use the repository's transaction-safe migration
pattern and prove rollback.

### 4.2 Immutability and deletion

The ledger is append-only in O3:

- no production update helper exists;
- no production delete helper exists;
- normal session status, archive, summon, input, kill, and event retention
  operations do not alter adoption rows;
- `sacrifice` of a session with any adoption row is rejected explicitly before
  deletion;
- the foreign-key restriction is the final race-safe enforcement if a caller
  bypasses the preflight.

The rejection is a typed `AdoptionRetentionError`, rendered consistently by
CLI, TUI, and chat-client sacrifice surfaces as:
`session adoption evidence is retained; sacrifice is unavailable until an
approved retention/fence contract resolves it`. Documentation must remove the
old unconditional claim that every non-running session is sacrificable.

Every valid O2 child parent is bound. Migration reserves every historical bound
parent and every new bound parent is adopted, so O3 sacrifice protection makes
parent deletion impossible. Parent-retention release and orphan behavior after
an approved release remain O4 decisions; O3 defines no such state.

O4 may later add a separately approved retention/fence disposition. O3 does
not silently expire or prune adoption evidence.

### 4.3 Restart behavior

Opening a migrated store preserves every adoption row and uniqueness index.
After restart, exact replays still return the original adoption and conflicts
still fail. This is the bounded restart evidence required by C-S4/C-M3.

O3 does not claim recovery of the crash interval after adoption commit and
before runtime delivery. The durable record prevents automatic duplicate
execution; O4/O7 must later classify and reconcile that interval.

Existing stale-`created` recovery must exclude sessions with a launch-adoption
or historical attempt reservation. An adopted `created` row is durable
evidence of the ambiguous commit-to-runtime interval and remains `created`
until O4/O7 authoritatively reconcile it; generic age-based recovery must not
invent `failed`.

## 5. Transaction and side-effect ordering

### 5.1 Shared rule

For adopted operations, the durable adoption commit occurs before the runtime
side effect. A process crash after commit must leave enough evidence to prevent
automatic redispatch. Recording adoption after the runtime call is forbidden
because a lost response could duplicate execution.

Before mutable admission, Coven acquires an `AdoptionGate` keyed by cryptographic
digests of the request key and, for launch, the attempt scope. Locks are
acquired in sorted digest order and held through authoritative transaction
commit or pre-commit rejection. The gate uses process-independent advisory
file locks under the Coven runtime directory; filenames and diagnostics never
contain caller keys or binding values. A crashed process releases the OS lock.
Different keys/scopes do not block one another.

The gate serializes the window between read-only precheck and SQLite commit
without holding a SQLite writer lock across filesystem, roster, harness, or
maintenance work. A waiter always repeats adoption resolution after acquiring
the gate, before applying mutable checks.

### 5.2 Launch

An adopted launch follows this order:

1. Structurally parse the launch JSON: required member presence and JSON types,
   plus the closed O2/O3 object shapes, contract identities, syntax, and
   cross-object digest equality. This phase does not canonicalize or inspect
   filesystem paths, validate configured/available harnesses, apply expiry,
   consult the familiar roster, require parent existence, or acquire
   maintenance.
2. Perform a read-only adoption-key and attempt-scope precheck:
   - exact existing adoption -> return its session
     with HTTP 200;
   - any key or scope conflict -> return HTTP 409;
   - absent -> continue.
3. Acquire the request-key and attempt-scope `AdoptionGate` locks, then repeat
   adoption resolution. Exact replay/conflict returns immediately.
4. For a genuinely new adoption, run existing project/cwd canonicalization and
   containment, harness validation, O2 expiry, familiar resolution, and
   root/child correlation.
5. Acquire the existing maintenance writer.
6. Begin a SQLite `IMMEDIATE` transaction and repeat the authoritative key and
   scope resolution:
   - exact adoption -> release the newly acquired writer and return HTTP 200;
   - conflict -> release it and return HTTP 409;
   - absent -> continue.
7. Revalidate a child parent inside the transaction, as O2 already requires.
8. Insert the session with status `created` and insert its adoption row in the
   same transaction, then commit. `created` is the O1 state for a durable row
   without established runtime ownership.
9. Pass the acquired `WriterLease` and a terminal-safe ownership-publication
   callback into the adopted runtime launch. The runtime retains the writer for
   the session lifetime exactly as in the current launch path.
10. Immediately after cancellation ownership is registered, and before any
    initial stream or piped prompt delivery, the runtime invokes the callback
    exactly once to compare-and-set `created -> running`. Runtime exit
    persistence may already have moved `created` to `idle` or a terminal state;
    that winner is never overwritten. When establishment definitively fails
    before the callback, use a compare-and-set transition `created -> failed`.

If maintenance acquisition fails, Coven repeats adoption resolution while it
still owns the `AdoptionGate`, then returns the maintenance error only when no
winner exists. The implementation must not hold a SQLite writer transaction
while waiting on adoption or maintenance locks.

An exact replay does not reacquire maintenance, invoke the runtime, emit Coven
Calls side effects, or create another session. It returns the current persisted
session record even when that session is terminal, its original binding expiry
has elapsed, or its familiar is no longer in the current roster. A concurrent
replay during the commit-to-runtime window truthfully returns `created`, never
a false `running`. Those mutable checks apply only to a new side effect. Shape,
contract, exact stored identity, and conflict checks always apply.

If runtime ownership is established but its immediate `created -> running`
publication fails, Coven returns a post-adoption error marked
`{"adopted":true,"delivery":"not_asserted"}` and does not relaunch on replay.
The registered runtime remains post-adoption ambiguity unless its launch-path
cleanup proves quiescence. O3 does not invent a false terminal or running state
and leaves reconciliation to O7.

Every synchronous request error produced after the session/adoption
transaction commits carries the same redacted adoption marker, including
runtime-establishment failure, failure to persist its immediate `failed`
transition, and `created -> running` failure. No synchronous post-commit error
may resemble safe non-adoption. Asynchronous terminal-event persistence can
fail after the HTTP response and is currently logged rather than returned; O3
does not claim a retroactive response channel. Its adoption/event evidence is
retained for O7 reconciliation.

### 5.3 Input

An adopted input follows this order:

1. Look up the target session.
2. Parse the O2 proof and O3 object shapes/contracts/syntax, exact-match the
   proof to the stored binding, and validate `data`. Defer mutable O2 expiry.
3. Check the adoption ledger before expiry and session liveness:
   - exact existing adoption -> return HTTP 200 without runtime/event work;
   - conflict -> return HTTP 409;
   - absent -> continue.
4. Acquire the request-key `AdoptionGate`, then repeat adoption resolution.
   Exact replay/conflict returns immediately.
5. For a genuinely new adoption, apply O2 input expiry and existing liveness,
   capacity, and handoff-fence checks.
6. Begin an `IMMEDIATE` transaction, repeat adoption resolution, acquire the
   input lease, insert the input adoption, and commit.
7. Strip `executionBinding` and `requestAdoption`.
8. Invoke the runtime and persist at most one input event through the existing
   event boundary. The event row carries an internal nullable
   `request_adoption_id` column, not payload metadata, with a unique partial
   index so one adoption can correlate to at most one input event.
9. Release the input lease using existing semantics.

The first successful boundary returns HTTP 202:

```json
{
  "adopted": true,
  "replayed": false,
  "delivery": "not_asserted"
}
```

An exact replay returns HTTP 200:

```json
{
  "adopted": true,
  "replayed": true,
  "delivery": "not_asserted"
}
```

`delivery` has only the value `not_asserted` in O3. The internal event
correlation gives O4/O7 evidence to inspect later but does not let O3 claim
runtime delivery.

If the runtime or event boundary fails after adoption commit, the adoption
record remains and exact replay does not invoke the runtime again. The original
call retains its concrete error code but adds redacted details
`{"adopted":true,"delivery":"not_asserted"}` so a client cannot mistake the
failure for safe non-adoption. O4 lookup and O7 recovery are required before
Psyche may decide how to reconcile an ambiguous post-adoption failure.

An exact replay may return after the session becomes terminal, because it
performs no new side effect. Shape and exact proof comparison still precede
that result, but elapsed expiry does not: O2 expiry gates new input authority,
not a read-only report of an already adopted request.

## 6. Errors and precedence

O3 adds four bounded errors:

| Code | Status | Condition |
|---|---:|---|
| `request_adoption_required` | 400 | A bound launch or input omitted `requestAdoption`. |
| `request_adoption_invalid` | 400 | Non-object, missing/extra member, malformed key/digest, invalid cross-field use, unbound operation, launch digest mismatch, external registration, or unsupported operation location. |
| `request_adoption_unsupported` | 400 | Unknown `contract`. |
| `request_adoption_conflict` | 409 | A key or launch attempt scope was already adopted under a non-identical identity. |

Error details contain field paths only:

- malformed key or key collision: `requestAdoption.key`;
- missing adoption: `requestAdoption`;
- malformed or launch-mismatched digest:
  `requestAdoption.requestDigest`;
- unsupported contract: `requestAdoption.contract`;
- launch scope collision: `executionBinding.attemptId`;
- adoption on an unbound request: `executionBinding`;
- external registration: `requestAdoption`.

Messages and details never include adoption keys, digests, bindings, session
ids learned from the ledger, or submitted payload data.

O3 precedence extends O2 without changing unbound behavior:

- JSON decoding and structural member/type parsing precede O2/O3 object
  validation; mutable path and harness admission do not;
- closed shape, contract, syntax, binding equality, and adoption conflict
  resolution precede mutable expiry, roster, parent, liveness, maintenance,
  capacity, and handoff checks;
- an exact replay/conflict is authoritative even when mutable admission state
  has changed since the first adoption;
- for a new adoption, existing maintenance/liveness/capacity/handoff denials
  occur before the durable adoption commit.

Store corruption is an internal error, never an absent-adoption fallback.

## 7. Capability and client parity

Health adds:

```json
{
  "ok": true,
  "apiVersion": "coven.daemon.v1",
  "capabilities": {
    "requestAdoptionContracts": ["psyche.request_adoption.v1"]
  }
}
```

The OpenClaw client gains:

- exact `CovenRequestAdoption` and contract-literal types;
- closed-object syntax validation before HTTP;
- required `requestAdoption` on bound launch;
- an explicit adopted bound-input method accepting data, exact O2 proof, and
  request adoption;
- preservation of the additive health value as untrusted negotiation data.

Existing unbound launch/input and all kill methods remain source- and
wire-compatible. Existing bound launch/input client methods are retained only
as explicit legacy methods for pre-O3 daemons. Adopted methods use only the
dedicated O3 routes, and O3 daemons reject bound operations on legacy routes.

Every adopted client method completes health negotiation before POST in this
exact order: first require `apiVersion === "coven.daemon.v1"`, second require
`ok === true` without coercion, and only then require
`capabilities.requestAdoptionContracts` to be an array containing the exact
`psyche.request_adoption.v1` string. Missing, null, false, truthy non-boolean,
or otherwise non-true `ok` values stop locally. Any health, API-version,
health-ok, or O3-capability failure sends zero POST requests and never falls
back to a legacy mutation. Capability presence authorizes use of the wire shape
only; every adopted request still carries its complete, exact O2
`executionBinding` proof, and all per-operation admission checks still apply.

## 8. Verification matrix

### 8.1 Value object

- accepts the exact three-member object;
- rejects every missing and unknown member;
- rejects non-object roots;
- rejects unknown contract;
- tests key length 0, 1, 255, and 256;
- tests every allowed key character and representative whitespace, Unicode,
  and punctuation rejection;
- rejects malformed, uppercase, short, and wrong-prefix digests;
- proves mixed-case allowed keys round-trip byte-exact;
- proves deterministic serialization.

### 8.2 Store

- fresh schema and legacy migration create the table and launch index;
- schema initialization is idempotent across repeated opens;
- repeated migration over a mixture of existing reservations, already-adopted
  rows, and missing historical reservations creates exactly the missing rows
  without collision or omission;
- launch/input rows round-trip strictly across reopen;
- malformed persisted contract, operation, digest, binding, or nullability is
  a store error;
- raw update/delete of adoption identity is rejected;
- same key/same identity resolves the existing row;
- same key/different digest, operation, session, or binding conflicts;
- different key/same launch attempt scope conflicts;
- different `attemptId` succeeds;
- concurrent launch and input insert races produce one adoption;
- concurrent same-key/scope requests serialize through `AdoptionGate`, and a
  waiter cannot return a mutable-admission error after another request commits;
- migration reserves every existing bound launch attempt and fails on duplicate
  historical scopes rather than choosing a winner;
- session status/archive/summon and event retention leave rows unchanged;
- adopted-session sacrifice is rejected and the row remains;
- CLI, TUI, and chat-client sacrifice surfaces render the typed retention
  error, including a concurrent-delete regression;
- unadopted-session sacrifice remains compatible;
- no update/delete helper or adoption-pruning path exists.

### 8.3 Launch

- valid first adopted root and child launches return 201 and invoke runtime
  once;
- exact sequential and concurrent replay returns the same session with 200,
  one session row, one adoption row, and one runtime launch;
- replay/conflict wins after expiry, familiar removal, project/cwd
  disappearance, harness removal, and maintenance changes because it performs
  no new launch;
- malformed structural field types still fail before replay, while valid
  strings whose filesystem/harness targets later drift do not hide replay or
  conflict;
- concurrent replay during commit-to-runtime returns truthful `created`, and
  established runtime ownership publishes `running` immediately after
  cancellation registration and before initial prompt delivery;
- stale-created recovery leaves adopted/reserved `created` rows unchanged
  beyond the normal TTL;
- deterministic exit-before-registration-publication tests prove terminal
  status wins over the later `created -> running` compare-and-set;
- replay after restart returns the same session without runtime launch;
- replay of a failed or terminal adopted session returns that session without
  relaunch;
- key reuse with different digest/binding/operation conflicts;
- different key with the same attempt scope conflicts;
- same graph/node under a different `attemptId` succeeds;
- `requestAdoption.requestDigest` mismatch against the O2 binding is invalid;
- adoption without binding and external registration are invalid;
- adoption errors beat familiar, parent, maintenance, and runtime work in the
  documented order;
- failure before adoption commit writes neither session nor adoption;
- crash simulation with committed session/adoption and no runtime call proves
  reopen/replay does not launch automatically;
- runtime launch failure preserves one adoption and one failed session;
- every synchronous post-commit launch failure response carries
  `{"adopted":true,"delivery":"not_asserted"}`.

### 8.4 Input

- first adopted input returns the explicit O3 202 shape, calls runtime once,
  and writes one event correlated by internal adoption id;
- exact sequential/concurrent/restart replay returns 200 and writes/calls
  nothing again;
- replay after expiry or after the session becomes terminal returns the
  existing adoption without another side effect;
- key reuse across inputs, sessions, launch/input kinds, digests, or bindings
  conflicts;
- malformed adoption and adoption on an unbound session are invalid;
- liveness, writer capacity, and handoff denial before commit leave no
  adoption row;
- runtime and persistence failures after adoption leave the adoption row and
  never permit automatic replay, and their response marks adoption without
  claiming delivery;
- runtime payload, capacity payload, and persisted event contain neither
  `executionBinding` nor `requestAdoption`;
- event correlation rejects another session's adoption, a launch adoption, a
  non-input event, a second event for the same input adoption, and equivalent
  raw updates after insertion, including a null-to-non-null retroactive
  adoption attachment;
- response/error bodies never expose key, digest, binding values, or input
  data.

### 8.5 Compatibility and non-goals

- every existing unbound launch/input test remains unchanged;
- O3-capable bound launch/input requires adoption, while an explicitly legacy
  client fails closed instead of silently bypassing it;
- health and TypeScript fixtures are additive;
- adopted client methods check exact `apiVersion` first, exact `ok === true`
  second, and exact O3 capability third before POST; missing, null, false,
  truthy, coerced, malformed, unsupported, or failed health negotiation sends
  zero POST requests, retains the per-request exact O2 proof requirement, and
  never falls back to a legacy mutation;
- adopted methods use dedicated routes that pre-O3 daemons reject, proving a
  daemon replacement after health negotiation cannot downgrade to an
  unadopted side effect;
- no adoption lookup route exists;
- no proven-not-adopted, unknown, fence, generation, or redispatch endpoint or
  state exists;
- no cancellation acknowledgement, artifact binding, or production child
  dispatch is added.

## 9. Rejected alternatives

### 9.1 Derive adoption from O2 binding only

Rejected. O2 has one immutable launch digest and cannot identify multiple
input requests. Reinterpreting O2 fields would violate its frozen opacity and
still leave input replay unresolved.

### 9.2 Store a unique key only on `sessions`

Rejected. It cannot represent input adoptions, and `sacrifice` would delete the
only proof. It also gives O4 no durable adopted-request record to extend.

### 9.3 Implement lookup/fencing with adoption

Rejected for O3. Lookup dispositions, proof of non-adoption, ambiguity,
retention expiry, and return-or-fence are O4. O3 stores the immutable record
they require but exposes no query or recovery authority.

## 10. Ownership and security boundary

Psyche owns:

- stable key generation;
- canonical request serialization and digest computation;
- deciding that an operation requires adoption;
- graph, node, attempt, familiar, delegation, and policy meaning;
- blocking rather than falling back when adoption capability is absent or an
  outcome is ambiguous.

Coven owns:

- exact object/syntax validation;
- immutable append-only storage;
- byte-exact equality and conflict classification;
- launch attempt-scope uniqueness;
- atomic record-before-side-effect ordering;
- preventing automatic duplicate runtime invocation/event persistence;
- redacted errors and sacrifice protection.

Neither side may treat O3 as authentication, authorization, delivery
acknowledgement, terminal state, or ambiguity resolution.

## 11. O4-O8 handoff

O4 will define read-only adoption lookup, adopted/proven-not-adopted/unknown
dispositions, compatible retention, and authoritative return-or-fence. It may
extend the ledger only through a separately approved additive migration.

O5 cancellation acknowledgement, O6 artifact/result binding, O7 crash
recovery, and O8's complete denial taxonomy remain separate. O3 does not claim
G4, G6, real Psyche adapter conformance, or production child dispatch.

## 12. Approval and completion gates

The O3 design is approved when:

1. this document is reviewed and merged;
2. its issue records the approval commit and review evidence; and
3. no production implementation is included in the approval change.

O3 implementation is complete only when:

1. an implementation issue exists;
2. an exact-file, test-first child plan is approved;
3. all §8 tests and repository gates pass;
4. implementation and API documentation merge through a green PR;
5. the issue and Bead record merge and verification receipts; and
6. no O4-O8 behavior or production child dispatch is represented as complete.
