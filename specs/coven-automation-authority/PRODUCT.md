# Coven Automation Authority — PRODUCT

**Status:** Draft v1 · 2026-08-30
**Owner:** Coven runtime
**Acceptance target:** "Every Coven automation occurrence is dispatched only as an authenticated, explicitly authorized embodiment of the intended familiar, with fresh runtime capability checks, approval handling, replay resistance, and a verifiable receipt describing exactly what authority was exercised."
**Source issue:** coven#857 · Parent program coven#854 · Protocol dependency coven#855 · Foundation coven#816

## Problem

Coven now owns recurring automation end to end: routine definitions persist in
the Coven store, the daemon fences occurrences with a lease, and every run
dispatches through the shared session-launch path into the Coven-owned run
ledger (`crates/coven-cli/src/automations/`). That foundation propagates a
familiar identifier into the launch path — but the identifier is the only thing
it propagates.

A routine carrying `familiarId = "charm"` is not sufficient proof that Charm
ran, that the current principal authorized the run, or that the runtime had
permission to perform its effects. Today:

1. **The familiar string is not an identity.** `familiar_identity.rs` resolves a
   display name from `familiars.toml`; nothing pins a familiar root, an
   identity revision, or a declaration digest to a run.
2. **There is no authenticated principal at dispatch.** The scheduler is trusted
   implicitly; no per-run authentication, authorization decision, or revocation
   check exists between occurrence claim and runtime launch.
3. **The run ledger records outcomes, not authority.** `automation_runs` stores
   status, exit code, and a bounded log — it cannot answer "what authority was
   exercised, under which approval, decided against which policy revision?"
4. **Approvals do not exist for automations.** Nothing binds an approval to an
   operation, consumes it, expires it, or prevents its replay against a
   changed definition.
5. **The Psyche execution binding (`psyche.execution_binding.v1`) covers
   Psyche-orchestrated sessions only.** Coven-owned automation dispatch — the
   direct scheduler path — has no equivalent binding of its own.

Until this contract is enforced, recurring familiar work must be treated as a
local execution convenience — not proof of authorized familiar continuity — and
unattended external side effects must remain disabled or approval-gated. This
spec makes the guarantees normative so implementation slices can land against a
single reviewable surface.

## Canonical boundaries

This contract consumes, and must not duplicate, canonical semantics owned
elsewhere:

- **Familiar Contract / continuity profile** (`specs/coven-familiar-spec/`)
  defines the familiar root, identity revisions, same-familiar rules, and
  principal binding. Coven resolves those inputs at dispatch time; it does not
  re-derive familiar identity from `familiars.toml` display data.
- **Coven Threads / authority profile** defines protected actions, capability
  decisions, approval semantics, degrade-to-proposal behavior, and authority
  evidence. The daemon's authority weave (`threads_gate.rs`) is the local
  enforcement point for protected surfaces.
- **Coven** resolves those inputs, authorizes dispatch, owns
  occurrence/run/attempt state, and commits the resulting receipt.
- **Psyche** may execute a multi-step authorized action but does not create
  identity or schedule authority. Its `psyche.execution_binding.v1` wire
  contract (`execution_binding.rs`) stays opaque to Coven and is unchanged by
  this spec.
- **Cave/SDK** propose, approve, inspect, and verify; they do not infer
  permission or author lifecycle truth.

## The AutomationExecutionBinding

Before any runtime launch, Coven atomically constructs or references an
immutable **`AutomationExecutionBinding`** (wire contract
`coven.automation_execution_binding.v1`). The binding is the only dispatch
credential: a run without one does not launch, and a runtime that receives an
unbound launch has hit an implementation bug, not a fast path.

The binding contains at least:

| # | Field | Shape | Producer | Notes |
|---|---|---|---|---|
| 1 | `automationId` | stable id | definition store | key of the routine |
| 2 | `definitionRevision` + `definitionDigest` | monotonic revision, `sha256:` digest of canonical `definition_json` | definition store | pins the exact definition; in-place edits mint a new revision |
| 3 | `occurrenceId`, `occurrenceKey`, `fenceGeneration` | ids + counter | occurrence ledger | `occurrenceId` + `UNIQUE(automation_id, scheduled_for)` fence; `attempt` count is the fence generation |
| 4 | `runId`, `attemptId`, `adoptedRequestKey` | ids + adoption key | run ledger / `psyche.request_adoption.v1` | the adopted request/idempotency key |
| 5 | `principalId` | stable opaque id | principal resolution | never a display name or email |
| 6 | `credentialRef` | credential/key id or authorization proof reference | principal resolution | an identifier, never secret material |
| 7 | `authorization` | operation, nonce, issued/valid times, replay state | AutomationAuthorizer | per-run, single-use |
| 8 | `familiarRootId` | stable root id | Familiar Binding Resolver | aliases must resolve to exactly one root |
| 9 | `familiarIdentityRevision` + `declarationDigest` | exact revision + `sha256:` digest | Familiar Binding Resolver | pins the identity/declaration the run embodies |
| 10 | `identityStatus` | valid / revoked / retired / paused at decision time | Familiar Binding Resolver | decision-time state, not creation-time state |
| 11 | `memoryProjectionIds` | allowed projection ids only | authorization decision | present only where authorized |
| 12 | `threadsDecision` + `protectedSurfaceManifestDigest` | permit / degrade / reject + manifest digest | Threads authority profile | decision digest pins what the gate saw |
| 13 | `capabilities` | requested, granted, denied, degraded sets | Runtime Capability Resolver | exact operation-specific grants |
| 14 | `approval` | requirement, approval id/evidence, scope, expiry, consumption state | approval ledger | `not_required` is an explicit value |
| 15 | `riskClass` | R0–R4 (below) | definition review | never self-declared by prompt text |
| 16 | `runtime` | exact descriptor/version/capabilities + selection rationale | Runtime Capability Resolver | pinned in the run, not discovered at launch |
| 17 | `policyVersions` | policy/profile revisions used for the decision | decision producer | every input version that affected the outcome |
| 18 | `decidedAt` + `producedBy` | timestamp + authoritative producer | Coven | who constructed the binding |

The binding is constructed atomically with the authorization decision — one
snapshot, one commit — and is immutable once committed.

### The bounded runtime projection

The runtime receives only the projection it needs to execute: the prompt, the
working directory, the familiar embodiment context, its exact capability set,
and the session correlation ids. It must **not** receive:

- principal credentials, key material, or authorization proofs;
- unrelated familiar identity history;
- private audit material or other familiars' data;
- broad ambient authority merely because the scheduler is trusted.

The existing `SessionLaunch` shape stays the transport; the binding adds a
reference (id + digest), not a copy of authority material, and the launch path
fails closed when the referenced binding cannot be resolved.

## Per-run authorization rules

1. Identity and authority are resolved **after** occurrence claim and
   immediately before dispatch. Creation-time revisions, credentials, approvals,
   runtime availability, and authorizations are never reused.
2. A paused, retired, or revoked familiar or principal **fails closed** with a
   typed, durable reason recorded on the occurrence.
3. A definition revision naming a familiar alias must resolve to exactly one
   stable root or the dispatch is refused as ambiguous.
4. Runtime capability descriptors must satisfy the action's declared
   requirements and be pinned in the run. Descriptor mismatch or downgrade
   refuses dispatch.
5. Unknown, stale, malformed, incompatible, or unavailable policy inputs refuse
   dispatch. There is no best-effort path.
6. Authorization and the binding commit operate on the same immutable
   snapshot/transaction boundary where the store allows it, and on a
   verifiable digest pair everywhere else (time-of-check/time-of-use is
   narrowed, never assumed away).
7. Approval evidence is operation-specific, bounded, expiring,
   non-transferable, and consumed or otherwise replay-safe. An approval for
   one occurrence/run cannot authorize another occurrence, a changed
   definition, or expanded capabilities.
8. Policy changes while a run is active do not rewrite history. Revocation and
   cancellation during queued, awaiting-approval, dispatching, and running
   states are explicit, auditable transitions.

## Side-effect classes and safe defaults

Ratified risk vocabulary, aligned with organization-wide agent risk classes:

| Class | Meaning | Unattended default |
|---|---|---|
| **R0** | read-only/local analysis, no protected mutation | allowed only under an explicit narrow grant |
| **R1** | bounded local artifact creation in an approved scope | allowed only under an explicit narrow grant |
| **R2** | mutable local state or migrations | requires a reviewed policy and a deterministic fixture/rollback contract |
| **R3** | network, credentials, user data, remote APIs, publication, external effects | per-run or tightly scoped recurring approval + protected-owner policy |
| **R4** | identity, authorization, persistence control, release, deletion, security-critical mutation | per-run or tightly scoped recurring approval + protected-owner policy |

Default policy:

- New and imported routines start **paused** (this matches the landed
  definition default and stays normative).
- Unsupported or ambiguous actions degrade to proposal where safe — staged
  like the authority gate's pending proposals, never written — and otherwise
  reject.
- Prompt text, tags, runtime names, and client payloads cannot self-declare a
  risk class or grant capabilities. The class comes from the reviewed
  definition and policy, not from content under the runtime's control.

## Approval lifecycle

Authoritative states and transitions:

```text
not_required
required -> requested -> approved | rejected | expired | revoked
approved -> consumed | revoked | expired
```

Requirements:

- Approval requests record the exact definition revision, occurrence, action
  digest, capabilities, familiar revision, and intended runtime.
- Any relevant change after approval — definition revision, action digest,
  capability set, familiar revision, intended runtime — invalidates the
  approval.
- Cave/SDK approval calls run under authenticated principal context with
  stable adoption keys; no client can directly write `approved` into run
  state.
- Approval refusal or expiry leaves the occurrence explainable and recoverable
  without launching.
- The scheduler may wait within a bounded policy window or skip/fail according
  to a declared approval-misfire policy; both are recorded.
- Approvals are consumed at use, or carry a nonce that makes replay
  detectable. Replay fails closed.

## Receipt and audit evidence

On every terminal run, Coven commits a versioned **`AutomationReceipt`**
(`coven.automation_receipt.v1`) that an independent verifier can check without
trusting the producer. It includes:

- all definition/occurrence/run/attempt correlation;
- the exact familiar and principal binding (root, revision, digests);
- authorization, approval, and capability decision digests;
- runtime descriptor and session correlation;
- exercised capabilities and the declared side-effect class;
- start/finish timestamps and terminal disposition;
- normalized result, artifact, and delivery digests;
- retry/cancellation/recovery history;
- failure and partial-delivery details;
- producer identity, integrity/authentication, and the event cursor;
- privacy/retention class and redaction status.

A receipt proves what Coven observed and authorized. It must **not** overclaim
model intent, legal agency, personhood, ownership, correctness of generated
content, or completion of effects Coven could not verify. "Dispatched" and
"completed external effect" are different claims; receipts record the first
and, where verifiable, the second — never an inference between them.

## Privacy and erasure

- Public/operational receipt fields are stored separately from sensitive
  identity, prompt, memory, and authority payloads.
- Sensitive payloads are encrypted at rest where the deployment profile
  requires it, reusing the at-rest encryption primitives Coven already owns.
- Stable opaque ids and digests are preferred over copying declarations or
  personal data into every row or event.
- Retention, redaction, tombstone, and erasure semantics are defined without
  breaking minimum security/audit evidence; erasure applies to redactable
  payloads, never to the structural audit spine.
- A changefeed/subscriber receives only fields authorized for that principal
  and client profile.
- Logs and errors never print credentials, raw approval secrets, unrestricted
  prompts, private memories, or unrelated filesystem paths. Error paths carry
  static field paths, matching the existing validation-error style.

## Threat model and required refusals

The implementation must prove, with tests, refusal or safe handling of:

- forged `familiarId` or principal strings;
- stale/revoked identity revision;
- replayed approval or command nonce;
- approval reused after definition/action change;
- capability escalation through prompt text, tags, runtime name, or client payload;
- confused deputy across two familiars, projects, or principals;
- runtime capability descriptor mismatch/downgrade;
- stale policy snapshot and time-of-check/time-of-use races;
- duplicate scheduler/worker acting under an old fence;
- forged client-authored run/approval/receipt state;
- tampered receipt/event/history;
- unauthorized read of sensitive run evidence;
- revocation during queued, awaiting-approval, dispatching, and running states.

Every refusal is fail-closed: an unknown, stale, malformed, or ambiguous input
refuses dispatch with a typed reason. A missing adapter must fail closed; it
must not fall back to unbound launch.

## Acceptance criteria

- [ ] Every dispatched run pins an exact principal, familiar root/revision,
      authority decision, runtime descriptor, definition revision, occurrence
      fence, and adopted request.
- [ ] Revoked, stale, ambiguous, or unauthorized bindings cannot dispatch.
- [ ] Approval is operation-specific, replay-safe, and invalidated by
      relevant changes.
- [ ] External effects remain safely gated by explicit risk/capability policy.
- [ ] Terminal runs produce independently verifiable, privacy-classified
      receipts.
- [ ] No client or runtime can forge authoritative lifecycle, approval, or
      receipt state.
- [ ] Cross-repository vectors prove compatibility at immutable revisions.
- [ ] Direct and Psyche-orchestrated runs use the same binding and receipt
      semantics.

## Non-goals

- Reimplementing the Familiar Contract, SPAR continuity, Threads, or runtime
  registries in the scheduler.
- Treating familiarity or a human-readable name as authentication.
- Claiming exactly-once external effects Coven cannot verify.
- Granting broad unattended R3/R4 authority in v1.
