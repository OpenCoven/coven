# Psyche Coven Prerequisites

**Status:** W1 complete and G3 approved 2026-08-02 - bounded post-G3 planning permitted
**Workstream:** W1
**Gate:** G3
**Owner:** Coven and Psyche maintainers
**Canonical decision:** [Familiar runtime design](./RUNTIME_DESIGN.md)
**Companions:** [W1 audit](./COVEN_W1_AUDIT.md), [Decision dossier](./DECISION_DOSSIER.md), [Product specification](./PRODUCT.md), [Technical architecture](./TECH.md), [Threat model](./THREAT_MODEL.md), [Telegram parity ledger](./TELEGRAM_PARITY.md), [Program plan](./PLAN.md)

## Decision

This document defines the behavior Psyche requires from Coven. It does not
claim that a proposed endpoint, capability, action, field, or error exists in
the current daemon, and it does not authorize Coven implementation work.

W1 must inspect current Rust code, public schemas, and tests and classify each
requirement as:

- `current` - public, versioned, documented, and covered by executable tests;
- `current_but_undocumented` - implemented and tested but not yet a stable
  public contract;
- `planned` - accepted gap with an owner and dependency order;
- `optional` - not required for the affected release capability; or
- `rejected` - outside Coven's ownership or intentionally unsupported.

Only `current` behavior can satisfy production conformance. A
`current_but_undocumented` result must become an explicit contract before G4.
No implementation issue is created from this document alone. G1 must pass
first; Coven gap assignments wait for the completed W1 audit and G3.

## Ownership boundary

| Concern | Owner | Coven prerequisite |
|---|---|---|
| Familiar identity source and revision | Psyche | Coven may validate and bind the exact snapshot to execution; it never defines the familiar. |
| Surface actor-to-principal mapping | Psyche | None. Coven must not become a Telegram or generic surface policy engine. |
| Intent, graph, delegation, budgets, verification, and recovery policy | Psyche | Coven accepts only the bounded execution request exposed through a versioned session contract. |
| Project/cwd and supported harness admission | Coven | Validate independently and reject out-of-bound requests. |
| Session process lifecycle, ordered events, terminal state | Coven | Return authoritative versioned state with restart persistence. |
| Protected execution resources | Coven | Expose only explicit versioned contracts with fail-closed denial. |
| Provider conversation and harness-native tools | Harness | Coven supervises the session but does not claim per-tool mediation without a separate proven contract. |
| Surface effects and delivery | Psyche plus adapter | No Coven authorization by implication. A surface effect cannot widen a Coven execution decision. |

Ward remains Coven's protected-familiar write and audit gate. Ward may produce
an opaque protected-write generation used when validating execution, but it is
not the familiar identity source, principal mapper, graph authority, verifier,
or surface policy engine.

## W1 audit artifact

The completed evidence matrix is
[`COVEN_W1_AUDIT.md`](./COVEN_W1_AUDIT.md). Val approved its classifications
and dependency order at G3 on 2026-08-02. This prerequisite document remains
the behavior source; approval does not claim that planned gaps are implemented
or that G4/G6 conformance exists.

W1 produces one reviewable matrix with these columns:

| Field | Requirement |
|---|---|
| Behavior ID | Stable ID from the profiles below. |
| Classification | One of the five allowed states. |
| Public contract | Exact endpoint/schema/capability/error identifier, or `none`. |
| Code evidence | Repository-relative Rust path and owning type/function. |
| Test evidence | Exact test path/name and what negative case it proves. |
| Persistence | State retained across daemon restart and its cleanup rule. |
| Gaps | Missing behavior without invented fallback. |
| Owner/order | Required only for accepted `planned` work. |

Successful route probing, an unversioned internal method, a capability string
without enforcement, or fake-service behavior is not evidence that a current
production contract exists.

## Single-node execution profile

G4 requires the following behavior for one Psyche graph attempt:

| ID | Required behavior | Minimum evidence |
|---|---|---|
| C-S1 | Exact API and capability negotiation | Supported version succeeds; unknown version/capability fails closed. |
| C-S2 | Session create, input, inspect, events, and terminate | Public contract tests using canonical project/cwd and supported harnesses. |
| C-S3 | Familiar snapshot and attempt binding | Match/mismatch tests bind familiar snapshot, project, graph node, attempt, and request digest immutably. |
| C-S4 | Stable request adoption | Same ID/digest returns one adoption; same ID/different digest conflicts; state survives restart. |
| C-S5 | Adoption lookup and non-adoption proof | Lost-response tests return adopted, proven-not-adopted, or explicit unknown without redispatch. |
| C-S6 | Ambiguity fence | A possible adoption can be authoritatively returned or fenced; no Psyche-local unblock is accepted. |
| C-S7 | Ordered event cursor | Cursor replay is monotonic, persisted, bounded, and duplicate-safe. |
| C-S8 | Authoritative terminal state | Process output or disconnect cannot substitute for terminal state. |
| C-S9 | Cancellation acknowledgement | Cancellation ends in acknowledged terminal state or an explicit unresolved result that Psyche maps to `termination_unknown`; silence is not success. |
| C-S10 | Result and artifact association | Opaque content-addressed references bind session, graph/node/attempt, familiar snapshot, project, digest, type, size, and lifetime. |
| C-S11 | Restart persistence | Adoption, cursor, cancellation, terminal, and result bindings survive daemon restart. |
| C-S12 | Structured denial | Missing/unknown contract, mismatch, policy denial, and mid-flight authority loss have stable redacted errors and no local fallback. |

The Psyche-side adoption/recovery retention requirement is the greater of the
configured graph-recovery window and every enabled adapter deduplication
window. W1 must prove Coven retains authoritative adoption/fence evidence for
at least that effective duration. No fixed 30-day daemon promise is assumed.

## Production multi-agent execution profile

G6 requires every single-node behavior plus:

| ID | Required behavior | Minimum evidence |
|---|---|---|
| C-M1 | Parent graph and child node correlation | Exact immutable graph/node/attempt/session relationship. |
| C-M2 | One attempt to one session | Concurrent or replayed adoption cannot bind a second live session. |
| C-M3 | Idempotent child adoption | Lost responses and restart preserve one child execution. |
| C-M4 | Descendant cancellation acknowledgement | Every adopted descendant is terminal or explicit unknown before parent cancellation completes. |
| C-M5 | Child result/artifact association | Results cannot cross graph, node, attempt, familiar, or project boundaries. |
| C-M6 | Orphan discovery | Psyche can discover adopted child sessions lacking a live coordinator after either daemon restarts. |
| C-M7 | Ambiguous child fencing | Possible adoption is returned or fenced before redispatch. |
| C-M8 | Safe restart recovery | Either daemon may restart without duplicate child execution or false terminal state. |
| C-M9 | Exact rejection | Mismatched identity, project, graph, node, attempt, delegation, or digest fails closed. |

Production child dispatch remains disabled until G6. Graph authoring,
simulation, and single-node execution do not imply this profile exists.

## Optional and rejected boundaries

- Memory integration is optional unless a familiar declaration requires it.
  Missing support degrades or blocks only that declared path; Psyche never
  writes directly into Coven-owned memory.
- Artifact input/output is required only for nodes, evidence, or Telegram rows
  that need bytes. Missing safe support blocks those paths.
- Runtime registry data may be consumed only through an accepted Coven public
  capability; Psyche does not depend on registry internals.
- Coven-side Telegram authorization, principal mapping, graph policy,
  verification verdicts, add-on trust, recurring timers, and surface delivery
  are rejected prerequisites because they belong outside Coven.
- Existing multi-host scheduler endpoints do not prove Psyche graph,
  delegation, recurring schedule, or verification semantics.

## Shared conformance suite

W2 creates one behavior-level conformance suite with adapters for a fake Coven
service and the W1-classified real daemon surface. The exact same assertions,
fixtures, schemas, and negative cases run against both.

The suite records Psyche and Coven commits, negotiated versions, classified
profile, fixture digest, and report digest. Any real-adapter skip, xfail,
relaxed assertion, alternate expected value, or undocumented endpoint fails
G4 or G6 as applicable.

Required fault injection includes daemon termination or stall:

- before and after session request adoption;
- before and after input adoption;
- before and after adoption lookup;
- during event-cursor consumption;
- before and after cancellation acknowledgement;
- before terminal/result/artifact persistence; and
- during ambiguity fencing.

Every case proves Psyche preserves an explicit unresolved state and does not
substitute local authority.

## Gate sequence

| Order | Workstream/gate | Exit evidence |
|---:|---|---|
| 1 | W0 / G1 | All six companion documents share one product and ownership model. |
| 2 | W1 / G3 | Every prerequisite is classified from code/test evidence; accepted gaps have owners and order. |
| 3 | W2 / G2 | Canonical schemas, fake services, persistence, and the reusable suite pass. |
| 4 | W5 / G4 | The unmodified suite passes against a pinned real Coven build for C-S1 through C-S12. |
| 5 | W8 / G6 | The unmodified multi-agent suite passes C-M1 through C-M9 before production child dispatch. |

No calendar promise, merged code, issue state, or capability advertisement
substitutes for terminal gate evidence.

## W1 acceptance

W1 is complete only when:

1. every C-S* and C-M* row has one allowed classification;
2. every `current` result cites public contract, code, positive tests, negative
   tests, persistence, and restart behavior;
3. every `current_but_undocumented` result remains non-production until its
   contract is made explicit;
4. every accepted `planned` gap has one owner and dependency order;
5. rejected surface/identity/graph authority is not assigned to Coven;
6. no fake-only behavior is described as a current daemon capability; and
7. the reviewed matrix, not this W0 hypothesis document, drives any later Coven
   implementation issues.
