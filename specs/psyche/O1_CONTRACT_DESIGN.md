# Psyche O1 Coven Contract Design

**Status:** Implementation verified; delivery evidence is tracked in issue #567 and Bead `coven-psy-o1`

**Decision date:** 2026-08-02

**Work item:** `coven-psy-o1`

**GitHub issue:** [#567](https://github.com/OpenCoven/coven/issues/567)

**Depends on:** W1/G3, merged in PR #566 at
`3bcdd99be1dd77ee1377e69ac0acb02dd9c929e0`

**Scope:** O1 only: the C-S1 contract-version/capability handshake and the
C-S8 session-lifecycle vocabulary. This document does not authorize production
changes until its review gate passes and a test-first child plan is approved.

## 1. Purpose

Psyche needs one stable way to determine whether a Coven daemon is compatible
before it creates or adopts a session. The W1 audit found two blockers:

1. Coven uses the named contract `coven.daemon.v1` in `GET /api/v1/health`,
   documentation, and most clients, while the legacy
   `GET /api/v1/api-version` response uses the route token `v1` in fields named
   `apiVersion` and `supportedApiVersions`.
2. The public lifecycle guide omits the persisted `killed` state and does not
   cleanly separate harness-session status, synthetic store rows, conversation
   presentation state, and archive visibility.

O1 freezes the semantics needed by later Psyche work without inventing a new
endpoint, widening Coven authority, or making a breaking wire change inside
the current contract.

## 2. Approved decision

### 2.1 Contract identity and route identity are different concepts

The only canonical compatibility identity is the named contract:

```text
coven.daemon.v1
```

The wire field that carries it is `apiVersion` in
`GET /api/v1/health`. New and migrated clients must use that health response
for contract negotiation.

The token `v1` is only a route-family identifier:

```text
/api/v1/*
```

It selects an HTTP route namespace; it does not by itself prove compatibility.
Rust identifiers, documentation, and tests must call it a route version rather
than an API contract version wherever the distinction is under Coven's
control.

### 2.2 The legacy `/api-version` response is preserved, not promoted

The existing `GET /api/v1/api-version` response is already consumed by a
first-party compatibility path and may have external consumers. Changing its
existing `apiVersion: "v1"` value under `coven.daemon.v1` would violate the
additive-compatibility rule O1 is meant to clarify.

O1 therefore treats this endpoint as a compatibility-only route diagnostic:

- its current response remains wire-compatible;
- public guidance stops presenting it as the canonical client handshake;
- first-party clients migrate to `GET /api/v1/health` and the named contract;
- its historically misnamed fields are documented as route tokens, not named
  contract identifiers; and
- removing or retyping those fields requires a separately reviewed breaking
  contract version.

This is the only tolerated compatibility exception. No new client may use the
legacy response as evidence that the daemon implements `coven.daemon.v1`.

### 2.3 Capabilities are availability claims, not permission grants

After the named-contract check succeeds, a client checks the
`GET /api/v1/health` capability block for the exact primitive it needs. For the
current contract this includes the documented session/event booleans,
`eventCursor: "sequence"`, and `structuredErrors: true`.

Required-capability behavior is fail-closed:

1. If the contract identity is absent or not exactly supported, the client
   stops before a dependent request.
2. If a required health capability is absent, false, or has an unsupported
   value, the client stops before a dependent request and reports the missing
   capability plus an upgrade/remediation hint.
3. If a capability is advertised but a later action is denied by policy or
   authority, the daemon's action-specific structured denial is authoritative.
   Discovery never overrides authorization.
4. An unknown route prefix continues to fail with the current structured
   `404 invalid_request`; an unknown harness capability target continues to
   fail with `404 harness_not_found`.

O1 does not add a universal `requiredCapabilities` request field or invent a
universal server denial code. Request-level capability negotiation remains a
planned gap unless a later bounded design approves it.

## 3. Session lifecycle vocabulary

The complete persisted `SessionRecord.status` wire vocabulary for daemon-owned
or externally registered harness sessions is:

| Status | Terminal | Meaning |
|---|---:|---|
| `created` | No | The durable row exists, but no live runtime has been established. A stale unowned row is recovered as `failed`. |
| `running` | No | Coven owns a live runtime, or an external runtime is registered as live. |
| `idle` | No | A conversational session completed a turn successfully and remains available for later input. Psyche's first one-shot execution adapter does not emit or accept this value as attempt completion. |
| `completed` | Yes | The observed runtime exited successfully or an external owner reported successful completion. |
| `failed` | Yes | Launch failed, the runtime exited unsuccessfully, or a stale `created` row was recovered without an owner. |
| `killed` | Yes for the current ledger only | Coven accepted a kill request and preserved that decision against a later exit observer. This value does not prove acknowledged process termination and cannot satisfy Psyche cancellation until O5 defines terminal-or-unresolved acknowledgement. |
| `orphaned` | Yes | Daemon recovery found a row marked `running` for which the new daemon cannot prove a live owned runtime. |

Psyche's first one-shot execution adapter uses `created`, `running`,
`completed`, `failed`, `killed`, and `orphaned`. It treats `idle` as a valid
current-contract value for conversational sessions but not as terminal evidence
for a Psyche attempt.

The following concepts are explicitly outside that harness-session lifecycle:

- `active` is an internal synthetic Cast quest-anchor store value. It is not a
  daemon-owned harness-session state, is not terminal, and must not be emitted
  for a newly created Psyche execution session. Because synthetic rows share the
  store and may appear in an unfiltered raw session list, clients must classify
  the row kind before interpreting its status.
- archive is represented by `archived_at`. Archiving or summoning changes
  visibility without changing the stored lifecycle status.

Terminal status is authoritative for the current persisted ledger once written.
In particular, a late observer may add truthful exit evidence but may not
replace `killed` with `completed` or `failed`. That race rule does not promote
`killed` into process-termination acknowledgement. Psyche cancellation remains
unresolved until the separately designed O5 contract reports its authoritative
terminal-or-unresolved outcome. `orphaned` means ownership is unresolved, not
that the work completed.

## 4. Client data flow

A conforming Psyche-side adapter follows this sequence:

1. Connect to the configured local Coven socket.
2. Read `GET /api/v1/health`.
3. Require `apiVersion === "coven.daemon.v1"`.
4. Require every capability needed for the proposed operation.
5. Stop locally on a version or capability mismatch; do not probe a dependent
   mutating route to discover compatibility.
6. Send the bounded session/event request.
7. Treat daemon structured errors and persisted session status as
   authoritative; do not infer success from transport closure or missing
   events.

The legacy `/api-version` endpoint is not part of this sequence.

## 5. Error handling

O1 preserves the existing structured error envelope. It freezes these
boundaries rather than defining speculative new codes:

| Condition | Required outcome |
|---|---|
| Unsupported named contract from health | Client stops before dependent work and reports expected versus observed named contract. |
| Missing or unsupported required health capability | Client stops before dependent work and names the exact capability and remediation. |
| Unknown `/api/<route-version>/...` prefix | Daemon returns `404 invalid_request` with the requested and supported route-token evidence currently provided by the contract. |
| Unknown harness capability target | Daemon returns `404 harness_not_found`. |
| Advertised action later denied | Daemon returns the action-specific structured denial; the client must not treat discovery as authorization. |
| `idle` observed for a Psyche one-shot attempt | Client recognizes the current wire value but treats the attempt as nonterminal and incompatible with the one-shot completion path. |
| Unknown session status | Client treats the record as incompatible/unresolved and does not infer a terminal result. |

Diagnostics must remain redacted. Version, capability name, status, and public
error code are safe; socket paths, project paths, private identifiers, prompts,
and credentials are not required for these mismatch reports.

## 6. Delivery boundary

The O1 delivery may change only the smallest surfaces needed to make this
decision executable:

- canonical API/daemon/session-lifecycle documentation;
- Rust naming or guardrails that distinguish named contract from route token;
- the first-party compatibility client that still treats the legacy route
  token as the API contract;
- documentation/contract tests that prevent the contradiction from returning;
  and
- status-vocabulary tests that freeze terminal versus nonterminal behavior.

O1 must not add Psyche binding fields, adoption keys, lookup/fencing,
cancellation acknowledgement, result artifacts, multi-agent behavior, or new
authority policy. Those remain O2-O8. It must not introduce a new route,
change the current legacy `/api-version` wire values, or enable production
child dispatch.

## 7. Verification design

The implementation plan must start with failing tests and cover at least:

1. health returns the named `coven.daemon.v1` contract;
2. the legacy `/api-version` route remains schema- and value-compatible and is
   not used by the first-party compatibility handshake;
3. first-party compatibility accepts the named contract and rejects missing,
   wrong, or malformed health capability values before dependent work;
4. unknown route prefixes and unknown harness capability targets retain their
   exact structured failures;
5. daemon-owned and externally registered harness sessions use exactly
   `created`, `running`, `idle`, `completed`, `failed`, `killed`, or `orphaned`,
   while Psyche one-shot execution uses the subset that excludes `idle`;
6. `completed`, `failed`, `killed`, and `orphaned` are ledger-terminal, late
   exit observation cannot overwrite `killed`, and no O1 test represents
   `killed` as acknowledged process termination;
7. `idle` is nonterminal, a synthetic row carrying `active` cannot be mistaken
   for a harness session, and archive visibility cannot be mistaken for harness
   terminal state; and
8. canonical docs contain the named contract/route-token distinction and the
   complete lifecycle table without conflicting examples.

The final implementation PR must run:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
npm --prefix packages/openclaw-coven run typecheck
npm --prefix packages/openclaw-coven test
```

It must also run the exact documentation guardrails and focused Rust/TypeScript
tests named by the child plan.

## 8. Compatibility and rollout

- This design makes the named health contract normative without breaking the
  legacy route diagnostic.
- First-party client migration and docs/guardrails land atomically in one O1
  PR so the repository never recommends the old handshake after migrating it.
- External legacy consumers continue receiving the existing route-token
  response.
- A future removal or type/value change to the legacy fields requires a new
  named contract and is outside O1.
- Rollback is a normal PR revert because O1 introduces no storage migration,
  new persisted value, or release cutover.

## 9. Gates and completion evidence

This design consumes the approved W1/G3 result. It does not claim G4 or G6.
O1 is complete only when:

1. this written design is approved;
2. an exact-file, test-first child implementation plan is approved;
3. the scoped implementation and documentation land through a green PR;
4. issue #567 and Bead `coven-psy-o1` record the merge and verification
   evidence; and
5. no O2-O8 behavior or production child dispatch is represented as complete.
