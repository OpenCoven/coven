# Psyche O2 Coven Contract Design

**Status:** Approved and implemented; merge/verification evidence is recorded by the implementation issue and PR.

**Depends on:** O1, merged (issue #567, Bead `coven-psy-o1`).

**Scope:** O2 only, per `specs/psyche/COVEN_W1_AUDIT.md` §8: the opaque
canonical execution-binding tuple that resolves C-S3, C-M1, and the O2 exact-
binding portion of C-M9. This document does not authorize production changes
until review passes and a test-first child plan is approved.

## 1. Purpose

Psyche needs the daemon to bind every session to an immutable, opaque
identity tuple that Psyche defines and Coven never interprets. Today
`familiar_id` and `project_root` are mutable-adjacent, roster-level hints with
no snapshot, graph, attempt, parent, or delegation identity. Without this, a
mismatched request — one whose bound fields do not match the session it is
sent against — can attach to the wrong Psyche attempt, and multi-agent
delegation (`callerFamiliarId`) has no immutable, opaque correlation to the
session it displays.

O2 freezes exactly one thing: Coven persists a Psyche-defined opaque
`psyche.execution_binding.v1` tuple at session creation, validates its syntax
and expiry, and exact-compares it on every subsequent bound mutating request
(input, kill) that must prove the binding, while plain reads return the
stored tuple without requiring proof. This exact-match comparison is a
mismatch-correlation guarantee only: it detects cross-binding substitution,
i.e. a proof drawn from, or matching, a different attempt's tuple. O2
deliberately permits two sessions to be launched with byte-identical
`executionBinding` objects (§9), so a valid proof presented more than once
against its own matching session — a replay, or a duplicate adoption of the
same binding — is indistinguishable from a legitimate request under this
design. O2 defines no adoption key, request nonce, or single-use/uniqueness
semantics, so replay prevention and duplicate-adoption prevention are **not**
resolved by O2 and remain an O3 guarantee. Coven owns storage, syntax
validation, and comparison; it does not own graph meaning, familiar
authority, delegation authorization, replay/duplicate prevention, or
uniqueness (that is O3).

## 2. Proposed decision

### 2.1 Contract identity and field naming

The named contract is `psyche.execution_binding.v1`. Requests carry it under
the camelCase field `executionBinding`, matching current request
conventions. The persisted `SessionRecord` response exposes it under the
snake_case field `execution_binding`, matching current response conventions.
The binding object itself always carries its own `contract` field so a stored
or returned object is self-describing without depending on the wrapper field
name. The health capability array (§6) is named `executionBindingContracts`,
distinct from the request/response field name, so it cannot collide by name
or type with the `executionBinding` object itself.

### 2.2 JSON shape

Launch request:

```json
{
  "familiarId": "opaque-string",
  "callerFamiliarId": "opaque-string-or-absent",
  "executionBinding": {
    "contract": "psyche.execution_binding.v1",
    "principalRef": "opaque-string",
    "familiarId": "opaque-string",
    "familiarSnapshotDigest": "sha256:<64 lowercase hex>",
    "projectDigest": "sha256:<64 lowercase hex>",
    "graphId": "opaque-string",
    "nodeId": "opaque-string",
    "attemptId": "opaque-string",
    "requestDigest": "sha256:<64 lowercase hex>",
    "policyRevision": "opaque-string",
    "expiresAt": "YYYY-MM-DDTHH:MM:SSZ",
    "parent": null,
    "delegationDigest": null
  }
}
```

For a child (delegated) binding, `parent` is a complete object and
`delegationDigest` is a digest, never null:

```json
"parent": {
  "sessionId": "opaque-string",
  "graphId": "opaque-string",
  "nodeId": "opaque-string",
  "attemptId": "opaque-string"
},
"delegationDigest": "sha256:<64 lowercase hex>"
```

`GET /api/v1/sessions/:id` and any session-listing route return the same typed
field values under `execution_binding`. Matching current `SessionRecord`
convention, an unbound session serializes `execution_binding: null`; it is
never omitted from the payload. A bound session serializes the full typed
object. O2 introduces no adoption key, request ID, or uniqueness field; that
is O3's responsibility.

### 2.3 Field semantics (Coven's view only)

Every field is opaque to Coven except for syntax, contract identity, and
expiry, which Coven validates. Coven never interprets principal, familiar,
graph, node, attempt, policy, or delegation meaning — it only stores,
syntax-checks, and exact-compares.

| Field | Nullable | Coven's obligation |
|---|---:|---|
| `contract` | No | Must equal `psyche.execution_binding.v1`; reject unknown values before insertion. |
| `principalRef` | No | Opaque ref syntax; store and exact-compare. |
| `familiarId` | No | Opaque ref syntax; must exact-match the canonical id resolved from top-level `familiarId` at launch (§2.4). |
| `familiarSnapshotDigest` | No | Digest syntax; store and exact-compare. |
| `projectDigest` | No | Digest syntax; store and exact-compare; independent of Coven-canonical `project_root` (§2.4). |
| `graphId` | No | Opaque ID syntax; store and exact-compare. |
| `nodeId` | No | Opaque ID syntax; store and exact-compare. |
| `attemptId` | No | Opaque ID syntax; store and exact-compare. |
| `requestDigest` | No | Digest syntax; store and exact-compare on bound input/kill. O2 performs no uniqueness or conflict detection over this field (O3). |
| `policyRevision` | No | Opaque revision syntax; store and exact-compare; Coven never evaluates policy. |
| `expiresAt` | No | Canonical UTC RFC3339 seconds; Coven checks only that it is syntactically valid and, at launch and for bound input (§5), that it has not already elapsed. Launch normatively rejects an `executionBinding` whose `expiresAt` is already in the past when the launch is processed. |
| `parent` | Yes | Null for a root binding; a complete four-field object for a child binding. Coven checks existence and exact-match against the referenced parent session's stored fields (§2.4); it never infers graph topology. |
| `delegationDigest` | Yes | Null for a root binding; a digest for a child binding. Store and exact-compare; Coven never authorizes delegation. |

### 2.4 Launch correlation rules

- The top-level, Coven-resolved canonical `projectRoot` remains Coven
  authority and is persisted unchanged as `project_root`, exactly as today.
- `executionBinding.projectDigest` is Psyche-owned, independently persisted,
  and is never derived from or checked against `project_root`.
- A bound launch requires top-level `familiarId`. Unlike an unbound launch —
  which trims `familiarId` and collapses an empty or whitespace-only value to
  "no familiar" — a bound launch applies no such trimming to the raw
  top-level `familiarId` it received (§3.1): the raw value must already be
  byte-exact, or the request is rejected as `400 execution_binding_invalid`
  (`details.fields: ["familiarId"]`) before resolution, the runtime, or the
  store are touched. Coven then runs its existing `resolve_familiar`
  resolution on that exact value exactly as it does today, and
  `executionBinding.familiarId` must exact-match the resolved
  `FamiliarContext.id`, not merely the raw alias supplied. That same
  canonical id is what Coven persists in `SessionRecord.familiar_id`. This
  equality check correlates Psyche's opaque snapshot reference to the
  familiar Coven will actually inject into the session; it does not make
  Coven the source of familiar identity or snapshot content, which remain
  Psyche's. A mismatch is rejected before session creation as
  `409 execution_binding_mismatch`, `details.fields: ["executionBinding.familiarId"]`
  (see §7).
- **Root binding:** `parent` must be `null`, `delegationDigest` must be
  `null`, and `callerFamiliarId` must be absent. Any of these being present
  is rejected. Unbound launches are unaffected: their existing
  `callerFamiliarId` display behavior is unchanged by O2.
- **Child binding:** `parent` must be a complete object and `delegationDigest`
  must be present. `callerFamiliarId` is required at the top level and is
  correlation metadata only — it identifies which stored parent session's
  fields Coven must exact-match, and Coven performs no delegation
  authorization decision from it. The session named by `parent.sessionId`
  must exist and itself carry a stored, non-null `execution_binding`. That
  parent's stored `familiar_id` must exact-match the request's
  `callerFamiliarId`, and the parent's stored `graphId`, `nodeId`, and
  `attemptId` must exact-match the request's `parent.graphId`,
  `parent.nodeId`, and `parent.attemptId` respectively.
- Coven performs only existence and equality checks for parent correlation
  and delegation. It never authorizes delegation policy or infers graph
  topology beyond the single parent reference given.

## 3. Shape validation

| Value class | Applies to | Rule |
|---|---|---|
| Opaque ref/ID/policy-revision | `principalRef`, `familiarId` (both locations), `graphId`, `nodeId`, `attemptId`, `policyRevision`, `parent.sessionId`, `parent.graphId`, `parent.nodeId`, `parent.attemptId` | 1 to 255 ASCII bytes, matching `[A-Za-z0-9._:/-]` only. |
| Digest | `familiarSnapshotDigest`, `projectDigest`, `requestDigest`, `delegationDigest` (when present) | Exactly `sha256:` followed by 64 lowercase hexadecimal characters. |
| Timestamp | `expiresAt` | Canonical UTC RFC3339 seconds: `YYYY-MM-DDTHH:MM:SSZ`. No fractional seconds, no non-`Z` offsets. Coven validates by round-tripping the parsed instant back through this same canonical formatting; `SS` is not restricted to `00`-`59` only — the RFC 3339 leap-second value `60` also round-trips and is accepted. |
| Contract | `contract` | Must equal `psyche.execution_binding.v1` exactly. |

Coven validates only syntax, contract identity, and expiry as defined here.
It never validates or interprets the semantic meaning of any field's content.

### 3.1 Exact-object membership and no normalization

The `executionBinding` object and its nested `parent` object are each parsed
as an exact, closed set of members — there is no open/extensible schema at
either level:

- `executionBinding` must contain exactly the members listed in §2.2/§2.3
  (`contract`, `principalRef`, `familiarId`, `familiarSnapshotDigest`,
  `projectDigest`, `graphId`, `nodeId`, `attemptId`, `requestDigest`,
  `policyRevision`, `expiresAt`, `parent`, `delegationDigest`) — no more, no
  fewer required members, and no additional ones.
- The nested `parent` object, when non-null, must contain exactly the four
  members listed in §2.2 (`sessionId`, `graphId`, `nodeId`, `attemptId`) — no
  more, no fewer, and no additional ones.
- Any member key present in either object that is not in that object's exact
  field set — at any nesting depth, including inside `parent` — is rejected
  with `execution_binding_invalid` before any other validation in §2.4 or
  this section runs. This applies identically at launch and to the binding
  proof on bound input/kill.
- Coven performs **no normalization** of accepted field values beyond the
  syntax rules in this section: no trimming of leading/trailing bytes, no
  case folding, no Unicode normalization (e.g. NFC/NFKC), and no other
  reformatting. A value is validated against the applicable rule in the
  table above and then stored and compared exactly as received, byte for
  byte. A value that only matches after such normalization is not a match —
  it is a syntax failure (`execution_binding_invalid`) if it fails the raw
  syntax check, or a mismatch (`execution_binding_mismatch`) on bound
  input/kill if it is syntactically valid but byte-differs from the stored
  value.
- This byte-exact rule also governs the top-level `familiarId` field of a
  *bound* launch, even though `familiarId` is not itself a member of
  `executionBinding`: its correlation against `executionBinding.familiarId`
  (§2.4) depends on comparing it exactly as received, with no trimming. This
  is a deliberate departure from an *unbound* launch, which keeps its
  existing `familiarId` trim/collapse-to-"no familiar" behavior unchanged
  (§2.4).

## 4. Persistence and API behavior

- The immutable tuple is the complete `executionBinding` object plus the
  session row's own Coven-canonical `project_root` and assigned session id.
  These are the only Coven-authoritative values combined with the Psyche
  tuple; nothing else is added to it.
- Values are serialized in a deterministic field order matching §2.2 after
  passing the exact-membership and syntax rules in §3/§3.1 unchanged (no
  normalization, per §3.1). Coven makes no promise to preserve the caller's
  original JSON member order or whitespace.
- The binding is persisted atomically with session-row creation in a
  nullable `execution_binding_json TEXT` column on the session row. O2 does
  not introduce a separate binding table.
- A `NULL` stored `execution_binding_json` value is the only representation
  of an unbound session and serializes as `execution_binding: null`. If a
  non-null stored value fails to parse as valid JSON, or its `contract` field
  does not equal `psyche.execution_binding.v1`, reading that row is a store
  error. It is never silently treated as an unbound session.
- No route may update any field of an existing session's `execution_binding`
  after creation.
- `GET /api/v1/sessions/:id` and any listing route return the same typed
  `execution_binding` field values (or `null` for an unbound session)
  unchanged by archive state, cursor position, or lifecycle status.

## 5. Bound operation requirements

`POST /api/v1/sessions/:id/input` on a bound session requires the complete,
exact `executionBinding` object alongside the existing `data` payload:

```json
{
  "data": "existing input payload, unchanged shape",
  "executionBinding": { "...": "complete object, matching §2.2" }
}
```

`POST /api/v1/sessions/:id/kill` on a bound session, which today carries no
body, gains a JSON body carrying only the binding:

```json
{
  "executionBinding": { "...": "complete object, matching §2.2" }
}
```

For both routes, missing or incomplete proof fails closed
(`execution_binding_required`); a present but non-matching proof fails closed
(`execution_binding_mismatch`). Input additionally rejects an expired binding
(`execution_binding_expired`, see §7); input never proceeds against an
expired binding. **Explicit exception:** kill may proceed after the binding's
`expiresAt` has elapsed, because kill only narrows authority (stops a running
attempt) and preserves operator safety. Kill still requires an exact match on
every other field.

Read/list/events endpoints (`GET /api/v1/sessions/:id`,
`GET /api/v1/sessions`, event/cursor reads) remain plain session-id reads and
require no binding proof. `GET /api/v1/sessions/:id` and any session-listing
route return the immutable `execution_binding` field already stored, per
§2.2/§4. Event/cursor reads do not: the persisted `EventRecord` shape carries
no `execution_binding` field at all, bound or unbound, so there is nothing to
return there. O2 defines correlation, not authentication — read access is
unchanged from today.

O2 adds no artifact-lookup route and no lookup-by-binding route. Lookup
remains by daemon session id only.

### 5.1 Operation precedence

**Launch** (`POST /api/v1/sessions`):

1. Existing JSON body parsing, `projectRoot`/`cwd` resolution, and harness
   validation, unchanged from today.
2. Execution-binding contract identity, shape (§3), expiry, and root/child
   cross-field validation (§2.4), if `executionBinding` is present.
3. Existing familiar resolution (`resolve_familiar`), unchanged from today.
4. Canonical familiar-equality check (§2.4) and, for a child binding, parent
   lookup and exact correlation (§2.4).
5. Existing maintenance-gate check, unchanged from today.
6. Atomic session-row insert, including `execution_binding_json` if present.

No session row is created, and no existing session state is mutated, unless
every step through 5 succeeds.

**Input and kill** (`POST /api/v1/sessions/:id/input`,
`POST /api/v1/sessions/:id/kill`):

1. Existing session lookup by id (`session_not_found` if absent), unchanged
   from today.
2. If the session is bound, require and parse the request's
   `executionBinding` (`execution_binding_required` if missing/incomplete,
   `execution_binding_invalid` if malformed, or
   `execution_binding_unsupported` if its contract is unknown).
3. Exact comparison of the parsed binding against the stored binding
   (`execution_binding_mismatch` on any field difference).
4. For input only, expiry check (`execution_binding_expired`); kill has no
   expiry check per its explicit exception above.
5. Existing external-runtime/liveness checks, unchanged from today.
6. The runtime action itself (deliver input, or send kill). Per §5.2,
   `executionBinding` is stripped from the request body before this step: it
   is never passed to `SessionRuntime::send_input`/`SessionRuntime::kill_session`
   and never written into the recorded input/kill event.

For an unbound session, steps 2-4 are skipped entirely and existing
precedence (steps 1 and 5) is unchanged. Step 6 is unchanged for kill, whose
body is never parsed for an unbound session; for input, step 6 is unchanged
for every field except that a now-reserved `executionBinding` key, if
present in the body, is always stripped before the runtime call and the
recorded event, even though it is never validated (§5.2). No runtime action
occurs unless every required prior step succeeds.

### 5.2 Metadata isolation

`executionBinding` is proof metadata consumed entirely by the API layer; it
must never leak into the harness/runtime input or into an ordinary input
event, on a bound or unbound session alike. Concretely:

- **Input, bound session:** once step 4 of §5.1 succeeds, the request
  handler discards the parsed body in favor of only the existing `data`
  field — the pre-O2 request/runtime shape — which is passed to
  `SessionRuntime::send_input`. `executionBinding` is never forwarded to the
  harness/runtime. The event recorded for the request (the existing input
  event) is likewise built from `data` only; it is the pre-O2 event shape
  and never contains `executionBinding` or any of its fields.
- **Input, unbound session:** every other field of the parsed body is passed
  to `SessionRuntime::send_input` and recorded in the input event exactly as
  before O2 — legacy precedence and shape for those fields is unaffected.
  `executionBinding` is the one exception: it is now a reserved key, so the
  request handler always strips it from the body before the runtime call and
  the recorded event, even though an unbound session never runs §5.1 step
  2's proof validation against it. A malformed `executionBinding` value is
  stripped the same as a well-formed one; it is never rejected on this path.
- **Kill:** once step 3 of §5.1 succeeds (bound) or immediately after step 1
  (unbound, since kill's body is never parsed at all), the binding proof —
  if any — exists solely to satisfy §5.1's exact-match and (non-)expiry
  checks. It is never passed to `SessionRuntime::kill_session`, which
  continues to take only the session id, and it is never written into the
  kill event, which remains the pre-O2 shape (a bare status marker, no
  binding fields), for both bound and unbound sessions.
- This isolation holds regardless of whether the request succeeds or is
  rejected earlier in §5.1 — `executionBinding` is never given to the
  runtime or recorded in an event on any code path, including error paths.

## 6. Health negotiation

Health capability discovery is additive:

```json
{
  "capabilities": {
    "executionBindingContracts": ["psyche.execution_binding.v1"]
  }
}
```

A client requiring execution binding must confirm
`"psyche.execution_binding.v1"` is present in `capabilities.executionBindingContracts`
before sending a bound launch. An unknown or missing required contract value
fails before any dependent request, per the existing O1 fail-closed rule —
O2 adds no new negotiation mechanism beyond this array entry.

Legacy and unbound sessions remain fully compatible: a launch that omits
`executionBinding` behaves exactly as it does today. Externally registered
(non-Coven-owned) sessions must reject any `executionBinding` supplied at
registration time, because Coven does not supervise that runtime and cannot
honor bound-operation guarantees for it.

## 7. Error matrix

| Code | Status | Condition |
|---|---:|---|
| `execution_binding_invalid` | 400 | Malformed, missing a required field, contains an unknown/extra member in `executionBinding` or its nested `parent` object (§3.1), or fails the root/child cross-field rule or the top-level-`familiarId`-presence rule (§2.4) at launch; malformed binding proof (including an unknown/extra member) on bound input/kill; or an externally registered session's registration request supplies `executionBinding` at all (§6). |
| `execution_binding_unsupported` | 400 | `contract` is not `psyche.execution_binding.v1`. |
| `execution_binding_required` | 400 | Bound input or kill omits or supplies incomplete binding proof. |
| `execution_binding_expired` | 409 | Launch or input references a binding whose `expiresAt` has elapsed. |
| `execution_binding_mismatch` | 409 | Any exact-match check fails, including canonical-familiar correlation (§2.4), parent correlation mismatch, or a request attempts to mutate an existing stored binding. This includes a child launch whose `parent.sessionId` exists but carries a `null` stored `execution_binding` — details name only `parent.sessionId` in that case, since no stored binding fields exist to compare. |
| `session_not_found` | 404 | The current session, or a child launch's referenced `parent.sessionId`, does not exist at all. Unchanged from existing behavior. |

Error `details` may name only the mismatched field path (e.g.
`executionBinding.graphId`, `parent.attemptId`); they must never include
field values or digests. No broader taxonomy is introduced — O2 does not
attempt the full O8 denial catalogue.

## 8. Future file/test map

| File | Change |
|---|---|
| `crates/coven-cli/src/store.rs` | Add `execution_binding_json` nullable column and (de)serialization; parent-lookup helper for stored binding fields. |
| `crates/coven-cli/src/api.rs` | Parse `executionBinding`/`callerFamiliarId` on launch with exact-object (closed-set) member checking at both the `executionBinding` and nested `parent` levels (§3.1); validate syntax, contract, root/child cross-field rules, and parent correlation with no value normalization; enforce bound-operation checks on input/kill, stripping `executionBinding` before it reaches `SessionRuntime::send_input`/`SessionRuntime::kill_session` or any recorded event (§5.2); add `capabilities.executionBindingContracts` to health. |
| `docs/API-CONTRACT.md` | Document `executionBinding`/`execution_binding`, the `psyche.execution_binding.v1` shape, the exact-object/no-normalization rule (§3.1), validation rules, the kill-after-expiry exception, and the input/kill metadata-isolation rule (§5.2). |
| `packages/openclaw-coven/src/client.ts` | Add typed `executionBinding` request/response support and capability check parity with the Rust validation, including rejecting unknown members client-side before send. |

Tests to add in the existing test modules of `store.rs` and `api.rs` (and
`client.ts` tests where applicable). Launch cannot test arbitrary semantic
mismatch of opaque fields such as `principalRef` or `projectDigest` against
some pre-existing expected value, because Coven has no external expectation
to compare against at creation time — launch tests therefore cover only
syntax, canonical familiar equality, parent correlation, and the root/child
cross-field rules. Per-field mismatch and substitution tests, which require
an already-stored binding to compare against, belong to bound input and kill:

Launch:

- accepts a complete, valid root binding and persists it, and a subsequent
  read returns the same typed field values;
- accepts a complete, valid child binding referencing an existing bound
  parent;
- rejects an unknown `contract` value (`execution_binding_unsupported`);
- rejects invalid characters, invalid length (0 or >255), a malformed
  digest, and a malformed `expiresAt` for each applicable field
  (`execution_binding_invalid`);
- rejects an `executionBinding` object containing any unknown/extra
  top-level member (e.g. an unexpected sibling of `contract`)
  (`execution_binding_invalid`);
- rejects a `parent` object containing any unknown/extra member (e.g. an
  unexpected sibling of `sessionId`/`graphId`/`nodeId`/`attemptId`)
  (`execution_binding_invalid`);
- accepts an `executionBinding` object (and, for a child binding, a `parent`
  object) containing exactly the defined members at each level, as a
  positive control paired with the two unknown-member rejection tests above;
- persists and returns opaque ref/ID field values (e.g. `graphId`,
  `attemptId`) containing mixed-case ASCII letters unchanged, and a
  subsequent bound-input/kill request differing only in letter case from the
  stored value is rejected as `execution_binding_mismatch`, proving no case
  folding is applied (§3.1);
- rejects an already-expired `expiresAt` (`execution_binding_expired`);
- rejects a top-level `familiarId` whose `resolve_familiar`-resolved
  `FamiliarContext.id` does not exact-match `executionBinding.familiarId`
  (`execution_binding_mismatch`);
- rejects a top-level `familiarId` that carries leading/trailing whitespace
  even when the trimmed value would otherwise resolve to, and byte-match,
  the canonical familiar (`execution_binding_invalid`, `details.fields:
  ["familiarId"]`), proving the bound path applies no trim before
  correlation (§2.4/§3.1); a paired positive control confirms an *unbound*
  launch still trims `familiarId` as before;
- rejects a root binding carrying non-null `parent`, non-null
  `delegationDigest`, or a present `callerFamiliarId`;
- rejects a child binding carrying null `parent`, null `delegationDigest`,
  or a missing `callerFamiliarId`;
- rejects a child binding whose `parent.sessionId` does not exist at all
  (`session_not_found`);
- rejects a child binding whose `parent.sessionId` exists but carries a
  `null` stored `execution_binding`, with details naming only
  `parent.sessionId` (`execution_binding_mismatch`);
- rejects a child binding whose parent's stored `familiar_id`, `graphId`,
  `nodeId`, or `attemptId` does not match the request's
  `callerFamiliarId`/`parent.*` fields (one test per field,
  `execution_binding_mismatch`);
- an invalid or mismatched launch performs no row insertion and mutates no
  existing session (atomic rollback/no partial row);
- a stored binding round-trips deterministically across a daemon restart
  (same typed field values, byte-exact and unnormalized per §3.1, on read,
  independent of original caller whitespace/member order);
- a launch omitting `executionBinding` behaves identically to current legacy
  behavior, and its `execution_binding` reads back as `null` (no regression);
- external session registration supplying `executionBinding` is rejected
  with `execution_binding_invalid`;
- `GET /api/v1/health` advertises `capabilities.executionBindingContracts`
  and clients (Rust first-party and `client.ts`) negotiate it consistently
  with docs;
- two sessions may be launched with byte-identical `executionBinding`
  objects and both succeed, proving O2 introduces no uniqueness/adoption
  behavior.

Bound input and kill:

- input rejects an expired binding (`execution_binding_expired`); kill
  succeeds with an exact binding whose `expiresAt` has elapsed (the explicit
  exception), while still requiring an exact match on all other fields;
- input and kill each reject a missing or incomplete `executionBinding`
  (`execution_binding_required`);
- input and kill each reject an unknown `contract` with
  `execution_binding_unsupported`;
- input and kill each reject a proof `executionBinding` object containing
  any unknown/extra top-level member, and each reject a proof whose `parent`
  object (child bindings only) contains any unknown/extra member — both
  cases produce `execution_binding_invalid` (§3.1);
- input and kill each reject a mismatch on every other individual field —
  cross-project, cross-familiar, cross-graph, cross-node, cross-attempt,
  cross-delegation, cross-request-digest, cross-policy-revision,
  cross-expiry, and cross-parent (`parent.sessionId`/`graphId`/`nodeId`/
  `attemptId`) substitution — supplying a syntactically valid binding that
  does not match the stored one, one test per substituted field
  (`execution_binding_mismatch`), with details naming only the mismatched
  field path;
- input and kill each reject an attempt to submit a binding that would
  mutate the stored binding rather than merely prove it;
- a successful bound input call records an input event and calls
  `SessionRuntime::send_input` with a payload equal to (or a strict subset
  of, containing only) `data`, asserting the recorded event and the
  `send_input` payload each contain no `executionBinding` key (§5.2);
- a successful bound kill call records a kill event and calls
  `SessionRuntime::kill_session` with only the session id, asserting the
  recorded kill event contains no `executionBinding` key and the
  `kill_session` call signature carries no binding argument (§5.2);
- an *unbound* session's input call strips a reserved `executionBinding` key
  from the payload — even a syntactically malformed one, since an unbound
  session never runs proof validation — before writer-capacity checks,
  `SessionRuntime::send_input`, and the persisted input event, while every
  other field and existing unbound precedence is unaffected; an unbound
  kill call never parses its body at all, so no equivalent stripping step
  applies there.

### 8.1 Metadata isolation acceptance evidence

The tests in the last two bullets above are the acceptance evidence for
§5.2: they must inspect the actual argument passed to
`SessionRuntime::send_input`/`SessionRuntime::kill_session` and the actual
persisted event row, not merely the HTTP response, since a leak would be
invisible in the response body.

## 9. O3 handoff

O2 defines the exact stored `psyche.execution_binding.v1` identity and its
`requestDigest` field but creates no uniqueness or adoption semantics over
them. O2 must accept two sessions launched with identical, otherwise-valid
`executionBinding` objects, including identical `requestDigest` values,
because no conflict or adoption rule exists yet. Consequently, O2 has no way
to distinguish a legitimately repeated proof from a replay or a duplicate
adoption of the same binding — mismatch detection is the only guarantee O2
makes, and replay/duplicate-adoption prevention is explicitly O3's
guarantee, not O2's, per §1. O3 is responsible for defining the adoption-key
schema (which fields, if any, become a uniqueness scope), the
collision/conflict outcome, replay/duplicate-adoption rejection, and any
lookup-by-adoption-key behavior. O2 does not anticipate or partially
implement that decision.

## 10. Relationship to `RUNTIME_DESIGN`

This O2 design freezes only the immutable launch/correlation core of
`psyche.execution_binding.v1` as specified in §§2-7: the request/response
field names, the exact v1 object, shape validation, launch correlation, the
bound-operation and precedence rules, and the six O2 error codes. The current
`RUNTIME_DESIGN.md` table's references to adoption resolution, event-cursor
correlation, and terminal-state correlation under the same contract name
describe later companion state and contracts owned by O3 (adoption), O4/O5
(lookup, fencing, cancellation acknowledgement), and O7 (crash-safe
recovery) — they are not fields of the immutable O2 object defined here and
must not be implemented as part of O2. `RUNTIME_DESIGN.md` wording that
implies those later fields already belong to the O2 object must be corrected
to reflect this boundary once this design is approved. This document does
not itself edit `RUNTIME_DESIGN.md` or add any of those fields.

## 11. Acceptance checklist

- [x] Contract name, request field, response field, and inner `contract`
      value exactly match §2.1.
- [x] The v1 object contains exactly the fields listed in §2.2/§2.3, with no
      added adoption key, request ID, or extra field.
- [x] Shape validation matches §3 exactly (byte length, character class,
      digest format, timestamp format).
- [x] `executionBinding` and its nested `parent` object each reject any
      unknown/extra member with `execution_binding_invalid` at both launch
      and bound input/kill (§3.1); accepted values are stored/compared byte
      for byte with no trimming, case folding, or Unicode normalization
      (§3.1); positive (exact-fields-only) and negative (unknown-member)
      tests exist at both object levels, per §8.
- [x] `project_root`/`projectDigest` separation and canonical familiar
      equality (`resolve_familiar`/`FamiliarContext.id` versus
      `executionBinding.familiarId`) are enforced per §2.4.
- [x] Root/child cross-field rules (`parent`, `delegationDigest`,
      `callerFamiliarId`) are enforced exactly as specified.
- [x] Parent correlation performs existence + exact-match only, never
      topology inference or delegation authorization.
- [x] Binding persists atomically in `execution_binding_json`, is immutable
      after creation, and survives restart with deterministic serialization.
- [x] Unbound sessions serialize `execution_binding: null`, never omitted;
      bound sessions serialize the full typed object.
- [x] Invalid stored JSON/contract is a store error, never silently unbound.
- [x] Bound input/kill require complete exact-match proof, following the
      precedence in §5.1; input rejects expiry; kill's expiry exception is
      implemented and tested.
- [x] `executionBinding` is stripped before the runtime call and before event
      recording on both input and kill (§5.2): a bound session's
      `SessionRuntime::send_input` receives only `data` and its recorded
      input event contains only `data`; an unbound session's
      `SessionRuntime::send_input` and recorded input event retain every
      other field unchanged but never carry an `executionBinding` key;
      `SessionRuntime::kill_session` receives no binding argument on either
      path, and the kill event carries no binding fields — verified by
      inspecting the actual runtime call and stored event, not just the HTTP
      response (§8.1).
- [x] Read/list/events endpoints remain unauthenticated by binding and
      unchanged in access behavior.
- [x] Health advertises `capabilities.executionBindingContracts` additively.
- [x] External session registration rejects `executionBinding` with
      `execution_binding_invalid`.
- [x] All six error codes in §7 are implemented with field-path-only details,
      including the parent-exists-but-unbound and external-registration
      mappings.
- [x] All tests in §8 (including §8.1) are present and passing.
- [x] `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
      `cargo test --workspace --locked`, `python scripts/check-secrets.py`,
      `python3 scripts/check-coven-privacy.py --staged`,
      `npm --prefix packages/openclaw-coven run typecheck`, and
      `npm --prefix packages/openclaw-coven test` all pass.
- [x] `docs/API-CONTRACT.md` updated in the same PR as the implementation.
- [x] Two identical bindings on separate sessions succeed, proving no O3
      behavior was introduced; the design's Purpose/self-review make clear
      this also means replay and duplicate-adoption prevention are not
      claimed by O2 (§1, §9, §13).
- [x] `RUNTIME_DESIGN.md` wording is corrected per §10 once this design is
      approved, without adding O3-O7 fields to the O2 object.

## 12. Non-goals

- O2 does not implement adoption-key uniqueness, conflict detection, or
  one-attempt/one-session enforcement (O3; see §9).
- O2 does not detect or prevent replay of a valid proof, or duplicate
  adoption of an identical binding across sessions — O2's exact-match
  comparison is mismatch-correlation only, not a replay/uniqueness guarantee
  (O3; see §1, §9).
- O2 does not add lookup-by-binding or return-or-fence semantics (O4).
- O2 does not define cancellation acknowledgement (O5).
- O2 does not define content-addressed result/artifact binding (O6).
- O2 does not add crash-matrix recovery proofs beyond deterministic
  persistence and restart round-trip (O7 covers the full recovery
  guarantee).
- O2 does not interpret `graphId`/`nodeId`/`attemptId` topology, enforce
  descendant completion, or authorize delegation policy — those remain
  Psyche authority (C-M4 stays rejected as a Coven aggregate).
- O2 does not change `docs/SESSION-LIFECYCLE.md` behavior; lifecycle status
  is unaffected by binding.
- O2 does not introduce a broad structured-denial taxonomy; only the six
  codes in §7 are in scope (full O8 catalogue is out of scope).
- O2 does not implement, and does not require editing now, the adoption
  resolution, event-cursor, or terminal-correlation companion state
  described in §10.

## 13. Self-review against audit scope

- **C-S3 (familiar snapshot and attempt binding):** Covered by this design.
  It specifies binding an opaque, Psyche-defined
  `familiarSnapshotDigest`/`graphId`/`nodeId`/`attemptId`/`requestDigest`
  tuple to the authoritative session record, with canonical familiar
  correlation via `resolve_familiar`/`FamiliarContext.id`, and rejecting
  mismatch, without Coven interpreting familiar or graph policy (§2.3, §2.4,
  §7).
- **C-M1 (parent graph/child node/attempt correlation):** Covered by this
  design for exact-binding correlation. The `parent` object plus
  `delegationDigest` give an opaque graph/node/attempt/parent correlation
  tuple that Coven is specified to store and exact-compare (§2.2, §2.4),
  without Coven owning graph meaning or descendant enumeration (§12,
  consistent with C-M4 remaining rejected as a Coven aggregate).
- **C-M9, O2 portion only (exact opaque binding rejection):** Covered by
  this design, and *only* for the cross-binding-substitution/mismatch
  portion of C-M9. Every specified field mismatch, including
  `delegationDigest`, is defined to produce `execution_binding_mismatch`,
  with Coven comparing the bound delegation digest without interpreting
  delegation authorization (§2.3, §2.4, §7). This design does **not** cover,
  and does not claim to cover, replay of a valid proof or duplicate adoption
  of an identical binding — those require the O3 uniqueness/adoption
  semantics (§1, §9) and are explicitly out of scope here. The broader C-M9
  O8 denial-taxonomy portion is also intentionally left out of scope (§12).

No O3-O8 behavior, uniqueness semantics, replay/duplicate-adoption
prevention, or production child dispatch is represented as complete by this
document.

## 14. Gates and completion evidence

This design consumes the approved O1 result. O2 is complete only when:

1. this written design is approved;
2. an exact-file, test-first child implementation plan is approved;
3. the scoped implementation and documentation land through a green PR;
4. an issue and Bead recording this work are opened and record merge and
   verification evidence; and
5. no O3-O8 behavior or production child dispatch is represented as
   complete.
