# Psyche Familiar Runtime Program Plan

> **For agentic workers:** This is a dependency-gated program plan, not an
> executable implementation checklist. Each workstream after W1 requires its
> own approved, test-first child plan before production code begins.

**Status:** W0 complete and G1 verified 2026-08-01 - W1 audit next

**Goal:** Build Psyche as the clean-room, local-first, surface-neutral familiar
runtime for a Coven, with durable intent and orchestration, evidence-first
verification, Telegram as the first production adapter, and Coven as the
independent bounded execution substrate.

**Canonical decision:** [Familiar runtime design](./RUNTIME_DESIGN.md)

**Companions:** [Decision dossier](./DECISION_DOSSIER.md), [Product specification](./PRODUCT.md), [Technical architecture](./TECH.md), [Threat model](./THREAT_MODEL.md), [Telegram parity ledger](./TELEGRAM_PARITY.md), [Coven prerequisites](./COVEN_PREREQUISITES.md)

## 1. Fixed program decisions

Psyche owns:

- familiar identity resolution and immutable snapshots;
- surface actor-to-principal mapping;
- immutable intent and durable graph/node/attempt state;
- graph authoring, dependencies, delegation, budgets, and coordination;
- verification policy, sealed evidence, verdicts, and escalation;
- trusted add-on discovery, supervision, and invocation records;
- surface-neutral ingress, effects, routing, delivery, and recovery; and
- operations, export/restore, migration, canary, and rollback.

Coven owns:

- project/cwd and supported harness admission;
- supervised session lifecycle, input, ordered events, termination, and
  authoritative terminal state;
- execution-layer approvals and protected resources exposed through explicit
  versioned contracts; and
- fail-closed rejection outside those boundaries.

Harnesses own provider authentication, provider conversations, harness-native
tool discovery/invocation, and native approvals unless a versioned contract
explicitly delegates one boundary. Surface adapters own protocol
authentication and transport mechanics, never familiar identity or graph
authority.

Telegram is the first production and conformance adapter, not Psyche's product
boundary. Production multi-agent child dispatch is architectural but remains
disabled until G6; it is not required to delay the first single-node Telegram
release. The standalone `OpenCoven/psyche` repository is created after G1 and
before W2 implementation.

OpenClaw compatibility is limited to separately reviewed operator-authored
prompts, declarations, hooks, commands, and configuration. Credentials,
databases, conversations, hidden memory, caches, runtime state, source,
internal names, and gateway internals are never imported.

## 2. First release slice

The first useful release includes:

- one local operator;
- one or more familiars with immutable identity snapshots;
- surface-neutral intent and durable single-node graphs;
- graph authoring and simulation for multi-node workflows;
- one real Coven session per executable node;
- deterministic evidence and human review;
- Telegram as the only production adapter;
- trusted pinned add-ons only if W9 does not delay W6;
- production child dispatch disabled unless G6 passes;
- explicit ambiguity recovery, doctor, export/restore, canary, and rollback.

This slice proves the common pipeline:

```text
authenticated Telegram observation
  -> mapped principal
  -> immutable familiar snapshot
  -> durable intent
  -> one graph node
  -> one Coven-supervised session
  -> candidate result and immutable references
  -> deterministic evidence or human review
  -> canonical surface effect
  -> Telegram delivery ledger
```

## 3. Workstream graph

| ID | Workstream | Depends on | Exit result |
|---|---|---|---|
| W0 | Canonical specification reconciliation | G0 | Every companion describes the same surface-neutral product and ownership model. |
| W1 | Current Coven contract audit | W0, G1 | Every prerequisite is classified with code/test evidence and an owner where needed. |
| W2 | Rust foundation and canonical schemas | W0, W1, G3 | Buildable workspace, canonical schemas, store, fake services, and contract tests. |
| W3 | Identity and intent | W2 | Surface-neutral identity snapshots, principal mapping, immutable intent, and replay pass. |
| W4 | Graph store and simulation | W2, W3 | Graph/node/attempt state, dependencies, budgets, cancellation, and restart simulation pass without a real harness. |
| W5 | Single-node Coven execution | W1, W2, W3 | One node dispatches, adopts, follows, cancels, and recovers against real Coven. |
| W6 | Surface contract and Telegram vertical slice | W2, W3, W5 | One authorized Telegram text turn completes through the common pipeline. |
| W7 | Verification engine | W2, W4, W5 | Deterministic evidence and independent-verifier gates pass their approved scope. |
| W8 | Production multi-agent execution | W4, W5, W7, G6 prerequisites | Non-widening delegation, child binding, fencing, budgets, cancellation, results, and orphan recovery pass real conformance. |
| W9 | Trusted add-on host | W2, W3 | Pinned same-user packages invoke through Rust and fail safely. |
| W10 | Telegram parity | W6 | Every Required ledger row has fake, crash, security, live, and operator evidence as declared. |
| W11 | Operations, migration, and release | Applicable W6-W10 | Doctor, privacy, export/restore, migration, canary, rollback, and packages pass. |

### 3.1 Critical path

```text
G0 decision approval
  -> W0 / G1 specification coherence
  -> standalone repository creation
  -> W1 / G3 current Coven audit
  -> W2 / G2 foundation and schemas
  -> W3 identity and intent
  -> W5 / G4 single-node real Coven execution
  -> W6 Telegram vertical slice
  -> W10 / G8-G9 Telegram evidence
  -> W11 / G10-G12 operations, canary, and release
```

W1 begins after G1 and gates W2. W4 may proceed against fakes after W2/W3. W7
and W9 may progress when their contracts stabilize. W8 is never on the first
release critical path unless Val explicitly changes the launch requirement.

## 4. Workstream contracts

### W0 - Canonical specification reconciliation

**Scope:** Documentation only. No implementation issues or production code.

**Deliverables:**

- `PRODUCT.md` defines Psyche as a surface-neutral familiar runtime and
  separates core objectives from Telegram adapter objectives.
- `TECH.md` freezes core contracts and correct ownership boundaries while
  preserving Telegram implementation detail behind the adapter.
- `THREAT_MODEL.md` includes graph, delegation, verifier, evidence, add-on,
  principal-mapping, and multi-surface threats.
- `TELEGRAM_PARITY.md` remains an adapter evidence ledger mapped to G8-G11.
- `COVEN_PREREQUISITES.md` defines W1 behavior classifications rather than
  speculative implementation assignments.
- `PLAN.md` uses W0-W11 and G0-G12.

**Exit:** Every companion uses the same product definition, authority matrix,
first-release scope, repository timing, service-objective split, and OpenClaw
compatibility boundary. Contradiction, placeholder, link, privacy, secret, and
repository verification passes. G1 then permits repository creation and W1;
implementation child plans and issues wait for G3.

### W1 - Current Coven contract audit

**Scope:** Read current Coven Rust code, public schemas, and tests. Do not create
gap issues until the evidence matrix is reviewed and G3 passes.

**Deliverables:**

- classify every C-S* and C-M* behavior from `COVEN_PREREQUISITES.md` as
  `current`, `current_but_undocumented`, `planned`, `optional`, or `rejected`;
- cite exact code, tests, persistence, restart behavior, and negative cases;
- identify the smallest accepted gaps without assuming proposed names;
- preserve Psyche-owned identity, graph, verification, and surface authority.

**Exit:** G3 passes. Only then may accepted `planned` gaps receive bounded Coven
issues and child plans.

### W2 - Rust foundation and canonical schemas

**Scope:** Create the standalone repository and implement only the canonical
types, storage, fake boundaries, and conformance fixtures needed by later work.

**Required packages:** Rust core/runtime crates plus thin
`@opencoven/psyche` distribution. TypeScript cannot own daemon, storage,
identity, graph, policy, verification, or surface transport.

**Exit:** G2 passes with unknown-version denial, migration, state-machine,
property, and crash tests.

### W3 - Identity and intent

**Scope:** Safe identity-file resolution, provenance, principal mapping,
immutable intent, supersession, replay, and context-manifest binding.

**Exit:** Identity-source confusion, unsafe paths, stale mappings, digest
mismatch, replay, and acknowledgement crash tests pass.

### W4 - Graph store and simulation

**Scope:** Graph/node/attempt state, acyclic dependencies, delegation envelopes,
budget accounting, leases/fences, cancellation propagation, and deterministic
simulation without real harness sessions.

**Exit:** Restart/property tests prove non-widening scope, once-only accounting,
no lease-only redispatch, and explicit unresolved cancellation.

### W5 - Single-node Coven execution

**Scope:** Implement the W1-classified adapter and run the unchanged fake
conformance suite against pinned real Coven.

**Exit:** G4 passes C-S1 through C-S12. No undocumented endpoint, test skip,
relaxed assertion, or adapter-only expected value is allowed.

### W6 - Surface contract and Telegram vertical slice

**Scope:** Common surface event/effect contracts plus one authenticated,
authorized Telegram text turn through W3/W5, deterministic evidence or human
review, and delivery recovery.

**Exit:** The common pipeline completes under fake/crash/security tests and a
dedicated live Telegram account without granting adapter authority to core or
Coven.

### W7 - Verification engine

**Scope:** Sealed evidence sets, deterministic checks, human verdicts,
independent-verifier identity/session separation, and calibration records.

**Exit:** G5. Independent model verification remains disabled until local
task-distribution evidence establishes approved thresholds; deterministic and
human review may ship first.

### W8 - Production multi-agent execution

**Scope:** Bounded child delegation and real execution only after graph
simulation and the C-M* Coven profile pass.

**Exit:** G6 proves non-widening delegation, one-attempt/one-session binding,
lease fencing, once-only budget accounting, descendant cancellation,
result/artifact correlation, ambiguity fencing, and orphan recovery.

### W9 - Trusted add-on host

**Scope:** `psyche.addon.v1`, pinned package provenance, operator allowlists,
revocation, framed Rust-owned worker protocol, minimal environment, typed
broker operations, bounded output/time, and per-invocation audit.

**Exit:** G7. Same-user Node workers are documented as trusted local code, not
an untrusted sandbox. W9 cannot delay W6; core may ship without add-ons.

### W10 - Telegram parity

**Scope:** Complete the Required rows in `TELEGRAM_PARITY.md` through common
surface contracts.

**Exit:** G8 fake/crash/security evidence and G9 live evidence pass. Deferred
and Rejected rows have no permissive fallback.

### W11 - Operations, migration, and release

**Scope:** Doctor, privacy, retention, encrypted export/restore, incident
response, reviewable OpenClaw concept migration, single-token cutover, rollback,
signed/checksummed distribution, SBOM, and provenance.

**Exit:** G10 operations, G11 operator-approved canary, and G12 distribution
pass. No calendar date overrides evidence.

## 5. Release and conformance gates

| Gate | Required evidence | Blocks |
|---|---|---|
| G0 - Decision approval | Passed 2026-07-31 after Val, Nova, and Sage review. | W0. |
| G1 - Specification coherence | All six companions share one product and ownership model. | Repository creation and the W1 contract audit. |
| G2 - Contract foundation | Schemas, migrations, fakes, state-machine/property tests, unknown-version denial. | Real execution integration. |
| G3 - Coven audit | Every prerequisite classified; accepted gaps have owner/order. | Implementation child plans, issues, code, and Coven assignments. |
| G4 - Single-node conformance | Unmodified suite against pinned real Coven, including denial/restart/cancellation/binding/ambiguity. | Real surface routes. |
| G5 - Verification | Deterministic gates; independent verification additionally proves reviewer separation, sealed evidence, policy, and local calibration. | Automated verified-success claims. |
| G6 - Multi-agent conformance | Delegation, child binding/adoption, fencing, budgets, cancellation, results, and orphan recovery. | Production child dispatch. |
| G7 - Trusted add-ons | Allowlist, digest/provenance, revocation, audit, protocol denial, crash/security evidence. | Add-on activation. |
| G8 - Adapter reliability | Fake surface, crash, security, ambiguity, and parity evidence. | Live Telegram. |
| G9 - Live Telegram | Required live rows pass twice on dedicated accounts and two client families. | Canary. |
| G10 - Operations | Doctor, retention, privacy, export/restore, incident response, migration, token rotation, rollback, and a release security review with no open critical or high-severity issue. | Production cutover. |
| G11 - Canary | Approved core and Telegram objectives hold for the operator-approved window/volume with zero unauthorized dispatch. | General release. |
| G12 - Distribution | Signed/checksummed artifacts, SBOM, provenance, clean-host install, rollback under threshold. | Publication. |

Capability flags remain false until their gates pass:

| Capability | Gate |
|---|---|
| `psyche.graphs.v1` | G2 |
| `psyche.singleNodeExecution.v1` | G4 |
| `psyche.independentVerification.v1` | G5 |
| `psyche.multiAgentExecution.v1` | G6 |
| `psyche.addons.trusted.v1` | G7 |
| `psyche.surface.telegram.v1` | G8 and G9; production use additionally requires G10-G11. |
| `psyche.hardBudgets.<resource>.v1` | Separate resource-specific enforcement/reporting evidence. |

## 6. Child-plan standard

Every post-W1 workstream requires a child plan that:

1. names exact files, schemas, state transitions, and public boundaries;
2. starts with failing unit/contract/property/crash tests;
3. uses one bounded worktree, issue/Bead, and shared claim;
4. preserves the Rust authority boundary and thin TypeScript packages;
5. defines fake and real conformance without adapter-only relaxation;
6. lists security/privacy/secret failure cases;
7. records verification commands and expected terminal evidence;
8. maps the change to one or more gates; and
9. stops at approval, publish, migration, or production cutover gates.

Plans may not silently reopen fixed W0 decisions, invent current Coven
capabilities, enable unsupported adapters, call unenforceable budgets hard,
enable child dispatch before G6, or treat add-ons as sandboxed.

## 7. Delivery discipline

- Fresh branch/worktree and shared issue-keyed claim per delivery unit.
- One concern per PR; no drive-by refactors.
- Human attribution preserved with GitHub-linked trailers.
- Local required gates pass before PR; remote matrix and review threads reach
  terminal state before merge.
- Beads, claims, PRs, gates, and live branch state are reconciled before work.
- No implementation issue exists merely because this program names a future
  workstream; issue creation follows G3 and an approved child plan.

## 8. Program risks

| Risk | Control |
|---|---|
| Telegram detail redefines Psyche | Keep adapter schemas behind common surface contracts and G8-G9. |
| Graph ambition delays useful release | Ship single-node vertical slice; keep W8 off critical path unless explicitly required. |
| Proposed Coven contracts become assumed scope | W1 classification and G3 before any Coven issue. |
| Generator self-certifies | Sealed evidence and distinct-verifier requirements at G5. |
| Lease/restart duplicates work | Stable adoption, fencing, and explicit unknown states. |
| Add-on marketplace weakens trust | Trusted pinned same-user tier only; untrusted tier needs separate containment design. |
| Service objectives mix core and adapter | Core durability/security objectives remain separate from Telegram latency/delivery targets. |
| OpenClaw migration imports hidden state | Allow only reviewed prompts/declarations/hooks/commands/config; reject credentials/databases/conversations/runtime state. |

## 9. Program completion

The program is complete only when:

1. G1-G12 have terminal evidence for every shipped capability;
2. surface-neutral identity, intent, graph, verification, and effect contracts
   remain free of Telegram identifiers;
3. Coven execution and protected-resource boundaries pass unchanged real
   conformance with no local fallback;
4. every Required Telegram row has its declared evidence;
5. production child dispatch remains off unless G6 passed;
6. add-ons remain within their approved trust tier;
7. export/restore, migration, canary, rollback, and distribution evidence is
   retained; and
8. no credential, database, conversation, hidden memory, cache, runtime state,
   private path, or secret enters migration or release artifacts.
