---
source_adjacent_reason: "Proposes producer-owned adoption lookup and admission fencing against Coven's O2/O3 storage and runtime boundaries."
---

# Psyche O4 Adoption Lookup and Fence Contract Design

**Status:** Proposed. No maintainer approval, implementation, conformance result, or downstream unblock is asserted. Endpoint names, capability spelling, response shapes, and the choices below are proposals for review.

**Tracker:** [Coven #1230](https://github.com/OpenCoven/coven/issues/1230). A dedicated O4 owning issue has been requested there; creation awaits maintainer agreement. Accountable decision owner: @BunsDev. A claim coordinates writes and does not assign durable implementation ownership.

**Baseline:** Coven `6b5caaec`, with O2 implemented through #732 and O3 through #753/#754. Read this alongside [O3](./O3_CONTRACT_DESIGN.md), [W1 audit §8](./COVEN_W1_AUDIT.md#8-accepted-gap-dependency-order), [Coven prerequisites](./COVEN_PREREQUISITES.md), and the current [API contract](../../docs/API-CONTRACT.md). The API contract's O4-O8 exclusions remain accurate until a separately approved implementation lands.

**Scope:** O4 only: lookup, explicit unknown/non-adoption proof, authoritative return-or-fence, immutable-binding query, and compatible retention for C-S5, C-S6, C-M6, and C-M7. O8 denial behavior is specified alongside this primitive. This proposal neither defines nor replaces `coven.familiar.identityRebind.v1`.

## 1. Purpose and existing boundary

O3 durably records launch/input adoption before its runtime side effect. Its global key and launch-attempt uniqueness prevent duplicate replay, including after restart. Adoption proves accepted responsibility, not runtime delivery or completion. The existing `AdoptionGate` is released after the adoption transaction and before runtime launch/input; acquiring that gate cannot prove an already adopted operation has stopped. Input events, runtime state, session lists, elapsed time, and transport failures cannot independently settle adoption uncertainty.

O4 lets an authorized local caller retrieve retained adoption evidence without replaying a mutation, discover bound executions after coordinator loss, or durably prevent an absent request from being adopted later. It must distinguish evidence of absence from inability to establish authority, and admission exclusion from execution terminality. Psyche keeps routes and lanes blocked whenever the disposition or required old-execution containment remains unresolved.

## 2. Approaches and proposed decisions

| Approach | Assessment |
|---|---|
| Return non-adoption from an empty ledger read | Rejected as redispatch authority: an in-flight request can commit after the read. A read can only describe its snapshot. |
| Read-only lookup plus durable exclusion serialized with O3 admission | Recommended: preserves O3 records and gives a concrete non-adoption proof only after competing admission is excluded. Existing adopted work is returned, never silently classified as fenced or stopped. |
| Make O4 perform cancellation, identity migration, and replacement dispatch | Rejected for this scope: requires O5 terminal acknowledgement, O7 recovery, and unresolved cross-repository decisions. |

The recommended approach and all concrete names below are the author's proposal, not a recorded maintainer choice. Use dedicated routes, a separately negotiated O4 contract, append-only fence evidence, permanent retention in this version, and no fence release or automatic redispatch. Reuse O2 byte equality and O3 global-key/attempt identity without interpreting graph, familiar, principal, or delegation meaning.

## 3. Proposed wire contract

### 3.1 Negotiation and transport

The proposed contract is `psyche.adoption_resolution.v1`, advertised only after its schema, lookup, exclusion, and admission checks are active, under `capabilities.adoptionResolutionContracts`. O2 `psyche.execution_binding.v1` and O3 `psyche.request_adoption.v1` remain required. Clients check exact `apiVersion === "coven.daemon.v1"`, exact `ok === true`, then exact O2/O3/O4 capability membership. Health also returns the durable store UUID as `capabilities.adoptionResolutionAuthorityId`. O4 clients persist that exact id before sending any execution mutation and carry it as `expectedAuthorityId` on every adopted launch/input and O4 request. The server compares it before replay, evidence reads, or admission, and revalidates it in the same authoritative transaction as the final launch/input write or exclusion commit; a mismatch returns `adoption_resolution_authority_lost` with zero effects. This closes replacement between health and the original mutation: a request pinned to authority A cannot execute on authority B. Failure sends zero mutations; there is no legacy-route fallback. Health is negotiation, never authentication or authorization.

| Route | Semantics |
|---|---|
| `POST /api/v1/adoption-resolution/lookup` | Read-only exact request lookup; POST keeps opaque correlation out of URL/access logs. |
| `POST /api/v1/adoption-resolution/return-or-fence` | Return an existing adoption or atomically commit an exclusion against future adoption. Never launches, sends input, kills, rebinds, or redispatches. |
| `POST /api/v1/adoption-resolution/adopted-sessions` | O3 adopted launch with required outer `expectedAuthorityId`; same atomic adoption and side-effect rules. |
| `POST /api/v1/adoption-resolution/sessions/:id/adopted-input` | O3 adopted input with required outer `expectedAuthorityId`; same exact proof, input lease, and adoption rules. |
| `POST /api/v1/adoption-resolution/query` | Read-only bounded immutable-binding discovery. No inference about coordinator liveness or absence. |

The two O4 execution routes accept the existing O3 launch/input payload plus required `expectedAuthorityId`. O4 clients use only these dedicated routes for original execution mutations. A pre-O4 daemon must return unknown-route rather than ignore the expected-authority field on an old route. These routes reuse O3 atomic adoption and runtime behavior; they add no new execution kind.

Use the existing authenticated same-user owner-gated local IPC authority for all five routes. HTTP reachability, an opaque binding, a request key, or a fence receipt grants no authority. A future remote transport needs separately approved access control before exposing these operations. Apply authentication and transport authorization before any store probe. Bodies, raw response data, query selectors, and cursors must not enter access logs or diagnostic errors.

### 3.2 Exact request identity

Lookup and return-or-fence take exactly these members; all are required, including nullable `sessionId`:

```json
{
  "contract": "psyche.adoption_resolution.v1",
  "expectedAuthorityId": "11111111-1111-4111-8111-111111111111",
  "operation": "launch",
  "sessionId": null,
  "executionBinding": "<complete O2 object>",
  "requestAdoption": "<complete O3 object>"
}
```

The two placeholder strings above denote required nested objects, not accepted wire strings. Both objects use the unchanged closed O2/O3 shapes and syntax. The outer object is closed. `expectedAuthorityId` must be a canonical UUID and match the negotiated durable authority. `operation` is exactly `launch` or `input`. For launch, `sessionId` must be null and the O3 request digest must equal the immutable O2 launch digest. For input, `sessionId` is a canonical daemon session UUID and the O3 digest describes that input independently of the launch digest. Reject missing, unknown, duplicate JSON members, wrong types, invalid nesting, unsupported versions, and oversized bodies before store work. Use existing API body bounds.

Identity is the O3 global adoption key plus contract, operation, request digest, full byte-exact O2 tuple, and input session id. Launch also reserves O3's five-field attempt scope: `principalRef`, `projectDigest`, `graphId`, `nodeId`, `attemptId`. Changing a key cannot evade a launch fence. Input exclusion is for its exact global request key; it neither fences the entire session nor prevents differently keyed future inputs. Psyche must retain the original key/digest and may not rename a possibly delivered input to evade uncertainty.

For evidence recovery, structurally valid expired O2 bindings are accepted as historical proof, subject to current transport authority. This does not renew the binding or admit any execution. Lookup/return of retained evidence precedes mutable familiar roster, filesystem, harness, liveness, and policy admission checks. Input requests still require exact equality with the stored session binding; an unknown session, malformed session evidence, or a historical reservation that cannot recover exact identity cannot yield non-adoption proof.

### 3.3 Success envelope and dispositions

Successful lookup, return-or-fence, and query requests return HTTP 200 with `ok: true`. The dedicated execution routes retain O3 launch/input success statuses and response shapes. Lookup and return-or-fence return exactly `contract`, `authorityId`, `disposition`, `adoption`, and `fence` in `data`. `authorityId` is a durable random UUID identifying this store authority lineage, not a daemon PID, caller identity, or authorization token. It is created once by the approved migration, survives ordinary reopen, and cannot turn a new/empty or restored store into evidence for an earlier authority. A client with an outstanding legacy O3 request and no pre-dispatch authority pin must keep it unknown until operator reconciliation; the first O4 response cannot retroactively establish its original lineage.

| `disposition` | `adoption` | `fence` | Meaning |
|---|---|---|---|
| `adopted` | Object | null | Exact O3 identity is durably retained. No delivery, terminality, or containment claim. |
| `proven-not-adopted` | null | Object | Exact request has never been adopted in this authority lineage and durable exclusion now prevents its future adoption. |
| `unknown` | null | null | No authoritative adoption or durable non-adoption proof is available from a healthy read. Remain blocked. |

An adoption object contains exactly `operation`, `sessionId`, `adoptedAt`, `sessionStatus`, and `delivery: "not_asserted"`. Return only the session matching the submitted full proof, never the session behind a conflicting key. `sessionStatus` uses the existing O1 vocabulary and is read in the same snapshot as the adoption. It is observational lifecycle evidence; in particular, `created` and restart `orphaned` do not prove process containment. O4 does not strengthen existing terminal-state guarantees or add cancellation acknowledgement.

A fence object contains exactly `id`, `committedAt`, and `scope`, with `scope` equal to `request-and-attempt` for launch or `request` for input. `id` is a stable opaque random UUID for retained exclusion evidence; exact retries return the same receipt. Timestamps use canonical UTC syntax and are descriptive, never expiry or freshness authority. Responses do not echo keys, digests, bindings, prompts, or input. Receipt references are meaningful only through a current authorized query of the matching authority; they are not transferable signed attestations.

Lookup returns `unknown` when no matching adoption or fence exists. An empty successful read is never `proven-not-adopted`. Lookup is read-only and creates no session, event, adoption, fence, runtime call, or admission lock file. A matching durable fence returns `proven-not-adopted`; a matching adoption returns `adopted`. Conflicting key/scope identity produces a redacted denial, not a success containing hidden identity.

A store read failure, corruption, busy timeout, missing authority metadata, or authority continuity failure is an error under §8, never a healthy `unknown` or non-adoption proof. Connection loss leaves the client uncertain. Clients retain the request identity and repeat lookup or the exact return-or-fence; they never interpret a transport failure as non-adoption.

## 4. Authoritative return-or-fence

### 4.1 Linearization and conflict ordering

After transport authorization and complete shape validation, use O3's shared key lock and, for launch, shared five-field attempt lock, in the existing deterministic lock order. Never make a fence-specific parallel lock domain. Re-read the adoption/fence state inside the same SQLite immediate writer transaction that would insert an exclusion. A pre-lock read is only an optimization and cannot authorize absence.

1. An exact retained adoption wins and returns `adopted` without runtime work or a new fence. An adoption already in `created` may still cross its runtime boundary after this return; the caller must remain blocked on whatever containment it requires.
2. A retained key with any identity difference returns `adoption_resolution_conflict`, naming only `requestAdoption.key`. A launch attempt occupied under another key returns the same code naming only `executionBinding.attemptId`. A legacy reservation without complete O3 request identity returns `adoption_resolution_unknown`; never fabricate adoption or non-adoption.
3. An exact retained fence returns the identical `proven-not-adopted` receipt. Different identity against an existing fence is a conflict, with the same redaction rules as adopted conflicts.
4. For an absent launch, insert both the exact key exclusion and five-field attempt exclusion atomically. For an absent input, verify the existing exact session binding and insert the exact key exclusion. No mutation of the O3 ledger is allowed.
5. Return `proven-not-adopted` only after durable commit succeeds. Losing the response does not release the fence. A commit with uncertain outcome returns an error if possible; an exact retry resolves the retained state.

If the admission wins the race, reconciliation returns its adoption. If exclusion wins, the original delayed admission and any new-key launch in that attempt are rejected before runtime. If the lock cannot be acquired within the existing request deadline, return unresolved/busy without creating a different execution. Cross-process transactions enforce the same exclusion, including when an optimistic replay path skipped a lock. No SQLite writer transaction is held across runtime calls.

### 4.2 Enforcement on every admission path

A fence is authoritative only if all applicable writers honor it. O4 implementation must update O3 launch and input resolution both before mutable admission and at final commit; uniqueness/exclusion must also be enforced transactionally by storage. New fence tables alone do not satisfy C-S6. Test the actual final launch transaction and `acquire_session_input_lease_and_adopt`, not just a resolver helper.

Legacy bound launch/input routes remain rejected under O3. Alternate registration, migration, summon, and any session reactivation path cannot erase or bypass adopted/fenced attempt reservations. A fenced key reused across operation kinds, bindings, or sessions remains globally conflicting. Replay of an existing adoption remains side-effect-free and cannot be rewritten into non-adoption. Fences cannot be released, overwritten, pruned, or used to reopen an attempt in this contract.

No `fenced` success is offered for already adopted work: a request admission fence cannot revoke a runtime call that may already have happened. To replace an adopted execution, Psyche must first obtain the separately required authoritative terminal/containment result. O5 owns per-session cancellation acknowledgement or durable unresolved status; O7 owns combined crash recovery. If those guarantees are unavailable, retain blocked state. This restriction applies equally to root and child attempts and does not discharge the historical rebind obligation.

## 5. Binding query and orphan discovery

Query takes exactly `contract`, `expectedAuthorityId`, `selector`, `limit`, and `cursor`. The expected authority follows §3.1 and is checked before scanning. `contract` is the O4 literal. `selector` is a closed object containing required `principalRef`, `projectDigest`, `graphId`, and `parentSessionId`; the first three use O2 syntax and exact equality, and the last is a canonical UUID or null. Null selects all executions in that exact principal/project/graph scope; a UUID selects only children whose immutable O2 `parent.sessionId` equals it. No wildcard, substring, path interpretation, principal mapping, or graph traversal is performed.

`limit` is an integer from 1 through 100; `cursor` is null on the first page or an opaque continuation from the same exact selector. Results contain only retained launch adoptions and launch fences, never prompts, raw events, input payloads, or an unbound session roster. A launch item contains the complete immutable O2 binding, the O3 request object if known, and its §3.3 disposition/evidence; a historical reservation has `requestAdoption: null`, `disposition: "unknown"`, and no invented fence. This deliberate authorized discovery response may reveal stored child bindings; errors and diagnostics may not. Transport authority remains the same-user store authority, not proof of separate principal-level authorization.

The response has `contract`, `authorityId`, `items`, and `nextCursor`. Paginate by an internal append-only sequence for the merged adoption/reservation/fence index. Capture a high-water mark on page one; bind cursor to authority id, exact selector, high-water mark, and last sequence. Reject tampered, malformed, or cross-selector/authority cursors using static errors; do not trust client-provided cursor contents as SQL. The exact encoding and integrity protection are implementation-plan details requiring review; their behavioral contract is fixed here. Status is read at each page's snapshot, not frozen across pages. Newly appended records require a fresh scan. An exhausted or empty query is never proof of non-adoption.

Coven returns correlation, not an `orphan` judgement based on coordinator liveness. Psyche compares durable discoveries with its own coordinator state after either process restarts, retains all unresolved children, and uses exact request lookup/return-or-fence individually with the authority id retained before original dispatch. No descendant cancellation or production child dispatch is added. C-M6/C-M7 end-to-end acceptance still requires the real Psyche consumer and O7 evidence.

## 6. Persistence, retention, and authority continuity

Use an additive migration: durable authority metadata, append-only exact-identity exclusions, and an indexed discovery sequence including historical launch reservations. Preserve O3's immutable rows/triggers and launch uniqueness. Fence key uniqueness spans both O3 adopted keys and exclusions; launch attempt uniqueness spans adoptions, historical reservations, and exclusions. Cross-table checks and insertion must be one transaction; a prior application read is insufficient. Malformed persisted identity, dangling session references, duplicated scopes, or simultaneous adopted/excluded identity fail closed before advertisement. Never choose a winning record or regenerate a missing proof.

This version retains adoption, reservation, fence, authority, and discovery evidence indefinitely, including restart, archive, summon, event pruning, and sacrifice attempts. Extend existing sacrifice protection to referenced fence/session evidence. There is no TTL, expiry, release, or retention-pruning API. Indefinite retention is compatible with any finite effective window, defined by the prerequisites as the maximum of graph recovery and all enabled adapter deduplication windows. Health advertises this proposal's retention mode as `adoptionResolutionRetention: "indefinite"`; consumers reject any other or absent mode when relying on this version. Disk exhaustion or retention failure is an availability failure, never permission to delete evidence or assert absence. A later bounded-retention version needs its own approved compatibility and stale-proof design.

Ordinary daemon restart over the same durable authoritative store preserves receipts and identity. A daemon replacement with a new/empty store, backup rollback, forked clone, lost WAL, or unverifiable storage history cannot settle old uncertainty. Psyche persists the expected authority id with every outstanding request and refuses an unexpected id. Matching copied ids alone cannot detect a rolled-back or forked store: this proposal requires one authoritative writer/store lineage, no writable clones, and no rollback of retained evidence. Restore or downgrade requires quarantining outstanding routes and explicit operator reconciliation; do not resume them on copied ids alone. This limitation must be visible in the eventual operational contract and tested, not described as rollback-safe recovery.

Never run an older binary that lacks O4 fence enforcement against an O4 authority store. The implementation plan must prove an effective schema-version/startup refusal or another reviewed writer exclusion before O4 advertisement. If existing old binaries cannot be prevented from writing, mixed-version/downgrade use is unsupported and the deployment must isolate the O4 store from them. No conformance or rollback guarantee may assume old writers honor a new table. This proposed constraint does not authorize any deployment.

## 7. Compatibility and ownership

O2 wire identity, nested O3 adoption objects/replay responses, and unbound operations remain unchanged. The new O4 execution routes require outer `expectedAuthorityId`, checked before replay and final admission. O4 clients never use legacy O3 execution routes as a fallback: an older daemon could ignore additive metadata there. Legacy O3 routes retain O3 behavior and cannot claim O4 authority-continuity protection. Every writer in an O4 daemon still honors durable fences, including the unchanged O3 routes. O4 requires additive final-admission checks; fenced O3 mutation attempts receive the new §8 denial. No key is silently rewritten, no adoption is deleted, and no new O2 binding is installed on an old session. A daemon replacement between negotiation and request either supports the dedicated routes with all checks or fails closed. Unknown-route/health errors never trigger fallback.

Coven owns strict parsing, retained execution/adoption evidence, admission exclusion, exact binding queries, and redacted denials. Psyche owns request canonicalization/key retention, graph/coordinator meaning, recovery windows, route/lane blocking, discovery reconciliation, and any separately approved replacement attempt. Familiar Contract owns identity/person binding; Threads owns protected-surface authorization. O4 adds no Telegram policy, principal resolution, surface delivery, graph scheduling, or independent identity authority. Thin client types may mirror the approved wire contract; core decisions stay in Rust.

## 8. O8 stable redacted denials for O4

Use the existing `ok: false` / structured `error` envelope. The messages below are static. `details` is null unless a closed static `fields` list is explicitly allowed. Never include supplied values, hidden ledger identities, cursors, store paths, raw SQL/OS errors, binding snapshots, roster entries, prompts, or request payloads in errors or diagnostic chains. An authorized success may contain the narrowly specified evidence; errors never reveal a conflicting record. Lookup `unknown` is a healthy disposition; failed reconciliation is a denial.

| HTTP | Code | Static message | Details / caller action |
|---:|---|---|---|
| 400 | `adoption_resolution_invalid` | Adoption resolution request is invalid. | Only static field paths; zero mutation. |
| 400 | `adoption_resolution_unsupported` | Adoption resolution contract is unsupported. | `fields: ["contract"]`; no fallback. |
| 403 | `forbidden` | Adoption resolution is not permitted. | null; no store probe, no fallback. Transport authentication retains existing unauthenticated behavior. |
| 409 | `adoption_resolution_conflict` | Adoption resolution identity conflicts with retained evidence. | Only `requestAdoption.key` or `executionBinding.attemptId`; do not reveal which hidden member differs. |
| 409 | `request_adoption_fenced` | Request admission is durably fenced. | null; applies to a mutation matching an exclusion; never retry under another key. |
| 409 | `adoption_resolution_unknown` | Adoption resolution remains unresolved. | null; historical identity is incomplete; retain blocked state. |
| 409 | `adoption_resolution_authority_lost` | Adoption resolution authority continuity is unavailable. | null; quarantine outstanding work and reconcile with the owner. |
| 503 | `adoption_resolution_busy` | Adoption resolution is temporarily unavailable. | null; bounded lock/store contention; repeat exact recovery only. |
| 500 | `adoption_resolution_store_error` | Adoption resolution evidence could not be read or committed. | null; preserve uncertainty, no local substitute. |

For nested O2/O3 syntax errors, map to `adoption_resolution_invalid` with only their static field paths. Unsupported nested O2/O3 contracts use `adoption_resolution_unsupported` with the corresponding static contract path. Existing session-binding mismatch remains `execution_binding_mismatch` with existing static paths; unavailable input target uses a static `adoption_resolution_unknown`, without echoing its id. Transport authority is evaluated first, then outer/nested shape, then retained key/scope conflict, then target relationship needed for an absent input fence. Precedence must not leak hidden identity through differing detail shapes. Read-only evidence recovery ignores binding expiry as specified in §3.2; new execution admission retains all existing O2 expiry/policy denials. Authority failure cannot be converted into either `proven-not-adopted` or `adopted` using stale cache.

## 9. Required test-first verification matrix

These are acceptance requirements for a later approved implementation plan, not executed tests. Preserve unchanged real/fake consumer assertions. Deterministic barriers and subprocess readiness signals should select race/crash boundaries; timing-only sleeps and local state guesses are insufficient.

| Area / row | Required evidence |
|---|---|
| Parsing and denial / C-S12, C-M9 | Every missing/unknown/duplicate field, wrong type/version, O2 parent/delegation mismatch, key/digest boundary, expired historical proof, invalid input target, malformed/tampered/cross-selector cursor; static redacted errors, zero runtime/admission effects. |
| Lookup / C-S5 | Lost launch and input responses return exact adopted evidence after reopen with zero replay; healthy empty read is unknown; exact committed fence is proven-not-adopted; corrupt/unavailable evidence is an error, never absence. |
| Return-or-fence / C-S6 | Launch and input races in both orders, including a waiting precommit writer: adoption wins or exclusion wins, never both. A new-key launch cannot bypass its fenced attempt. Input key changes do not masquerade as reconciliation. |
| Commit boundaries / C-S5, C-S6 | Kill before exclusion commit, after commit before reply, before adoption commit, after adoption commit before runtime, and during input delivery. Reopen in another process and resolve exact retained evidence; no automatic runtime retry or invented terminal state. |
| Already adopted / C-S6, C-M7 | Hold runtime work after O3 gate release; return-or-fence returns adopted while work remains possible. Assert no false contained/fenced/non-adopted result, no replacement execution, and continued Psyche blocking until required O5 evidence exists. |
| Conflicts / C-M9 | Key collision across operations/sessions/bindings/digests and launch attempt collision across keys; exact receipt replay; one transaction enforces cross-table uniqueness even for concurrent writers that skip an optimistic read. |
| Discovery / C-M6 | Root/child/historical/fenced entries, equal graph ids in other principal/project scopes, exact parent selector, bounded multi-page scans, insertion during pagination, restart of each daemon, and a new scan for later records. Psyche identifies missing coordinators without Coven interpreting liveness. |
| Retention / C-S5, C-M6 | Receipt/id/sequence survives reopen, event pruning, archive, summon, attempted sacrifice, and elapsed recovery/deduplication windows; no prune/release path; disk/storage failure retains blocked state. |
| Authority continuity / C-S6, C-M7 | New/empty store and mismatched authority id cannot settle old requests; restored/forked store remains quarantined despite copied id; old writer/downgrade exclusion proven before capability advertisement. |
| Capability and transport / C-S12 | Exact negotiation, malformed health and advertised-but-denied capability, daemon replacement after health and before original launch/input (O4 authority mismatch or pre-O4 unknown-route creates zero adoption/runtime effects), the explicit A-health → replacement B mutation with lost reply → recovery in A race cannot yield false safe replacement, legacy request with no pre-dispatch pin stays unresolved, owner-gated IPC versus unauthorized transports; no session, input, surface request, or local fallback on denial. |
| Existing behavior | Unchanged unbound API tests, O2 exact tuple tests, O3 replay/conflict tests, input lease/event correlation, migration reservations, and lifecycle tests remain green. Runtime/event payloads remain free of O2/O3/O4 metadata. |

The final evidence packet must retain source SHA, real daemon artifact digest, negotiated contract/retention/authority tuple, fixture/report digests, exact assertions, first failures and retries/skips, and durable outcomes per row. A helper test, fake G2 pass, source search, merged docs, or capability advertisement cannot certify G1b/G4/G6. O4's own restart boundaries are necessary here; they do not replace O7's cross-O2–O6 crash matrix.

## 10. Mapping to #1230 and remaining design ownership

| 2026-10-20 #1230 matrix obligation | O4 contribution / remaining owner |
|---|---|
| Client-intent idempotency/adoption lookup | C-S5 read-only exact lookup and retained non-adoption proof; C-S6 durable admission exclusion. Coven contract/storage owns this proposal; O3 remains delivered replay/conflict substrate. |
| Pre-existing adoption unknown before rebind | C-S6 returns an existing adoption or excludes an absent one; C-M7 applies the same rules to children. Already adopted execution still needs authoritative containment. No rebind mapping or equivalence is asserted. |
| Termination/stall during adoption and blocked routes | O4 race/crash denials plus C-M6 discovery provide evidence; O5 and O7 must complete cancellation and recovery. Psyche preserves route/lane blocking. |
| Capability present but denied | O8's O4-specific taxonomy plus the real consumer side-effect/fallback assertions; does not satisfy every authorization/profile denial. |

| Remaining artifact or decision | Accountable boundary / next approval |
|---|---|
| Dedicated O4 issue and approval of this design | Coven maintainer @BunsDev on #1230; no issue creation agreement or design approval is inferred. |
| O5 per-session cancellation acknowledgement / unresolved disposition design | Coven runtime/storage contract owner; Psyche coordinator owns descendant aggregation. @BunsDev coordinates assignment through #1230. |
| O6 input/output content-addressed artifact/result binding design | Psyche metadata owner and Coven protected-resource owner, coordinated by @BunsDev through #1230 and psyche#12. |
| O7 crash/restart design across O2–O6 | Coven storage/runtime and Psyche recovery owners after O5/O6 contracts; assignment coordinated by @BunsDev. |
| O8 complete denial taxonomy | Coven contract owner alongside every primitive; this document proposes only O4 states. Psyche verifies consumer behavior; @BunsDev approves ownership. |
| Historical `coven.familiar.identityRebind.v1` old-to-current mapping and executable equivalence criteria | @BunsDev on [psyche#12](https://github.com/OpenCoven/psyche/issues/12), with Familiar/Threads/Psyche/Coven owners. Explicitly unresolved; O2–O4 composition is not an approved equivalent. |
| Recover or explicitly correct historical spec pin `9b07c500ad10a8d509d451d64b9739a1bdc4dc0f` | @BunsDev on psyche#12; original artifact or approved provenance correction required. Current source is a proposal baseline, not a replacement historical pin. |

The W1 §8 row is authoritative for the O5 cancellation / O6 artifacts numbering; older summaries that label O5/O6 together as artifacts or attach artifacts to O8 do not change that row. No individual production implementer is assigned by this proposal. #1237 remains gated on the complete #1230 real-daemon packet, and production child dispatch remains gated on G6.

## 11. Approval, plan, and stop conditions

This design is approved only after a maintainer explicitly accepts its scope, wire and denial choices, admission-only fence limitation, retention/authority continuity constraints, and discovery exposure; the review and approval commit must be recorded on the O4 issue and #1230. A self-review, open PR, or green checks is not maintainer approval. The design PR contains no production implementation.

After that approval, write a separate exact-file, test-first plan under `docs/superpowers/plans/`, using #746's O3 plan pattern and the approved design revision. Merge the plan through protected review and required checks. Stop before feature code unless the plan is approved. No plan is pre-approved by this proposal.

If approval is absent, leave this design PR open as proposed and stop. If review requires a rebind action or an O2–O4 rebind compatibility mapping, stop and request the decision on psyche#12; do not invent one. Reject any attempt to label fake/G2 or docs evidence as G1b. Full #1230 acceptance, unresolved compatibility/provenance decisions, and downstream gates remain open.
