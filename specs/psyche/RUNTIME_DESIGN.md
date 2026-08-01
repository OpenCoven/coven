# Psyche Familiar Runtime Design

**Status:** Approved architecture baseline - W0 reconciled and G1 verified 2026-08-01

**Design owner:** Psyche maintainers

**Approved:** 2026-07-31 by Val after final Nova and Sage review

**Reviewers:** Nova, Sage, and affected Coven maintainers

**Product home:** standalone `OpenCoven/psyche` repository

**Temporary design home:** `specs/psyche/` in `OpenCoven/coven`

**Companion documents:**
[Product specification](./PRODUCT.md),
[Technical architecture](./TECH.md),
[Threat model](./THREAT_MODEL.md),
[Telegram parity](./TELEGRAM_PARITY.md),
[Coven prerequisites](./COVEN_PREREQUISITES.md), and
[Program plan](./PLAN.md)

> This document defines the approved product and architecture direction. W0
> reconciles the companion documents to this surface-neutral boundary while
> preserving Telegram detail as adapter evidence. G1 is required before
> repository creation and W1; G3 is required before implementation planning,
> issues, or production code.

## 1. Product decision

Psyche is the local-first, surface-neutral familiar runtime for a Coven. It
preserves the operator's intent and each familiar's declared identity while
coordinating durable work across familiars, harnesses, skills, add-ons, memory,
verification, and human-facing surfaces.

Psyche is the operator-aligned mind of a Coven, not a generic agent framework.
It translates human intent into reviewable orchestration graphs while keeping
identity, authority, evidence, and recovery explicit.

Telegram is the first production and conformance adapter. It is not Psyche's
permanent product boundary. Cave, CLI, mobile, webhooks, and future channels
must use the same surface-neutral runtime contracts.

Multi-agent orchestration is core architecture. A release may expose only the
subset whose Coven contracts and evidence gates are satisfied. Unsupported
graph operations remain disabled through negotiated capability flags rather
than being approximated locally.

Psyche is not:

- a replacement for Coven's project-scoped execution substrate;
- a model provider or harness;
- a daemon-owned model or tool loop;
- a prompt-defined persona system;
- a thin Telegram bot;
- a generic cloud orchestration framework;
- an OpenClaw fork or compatibility clone; or
- evidence that an advertised architecture is safe to ship before its
  contracts exist.

## 2. Design principles

1. **Identity precedes work.** Every node resolves one familiar from its own
   `IDENTITY.md`, `SOUL.md`, roles, skills, principal, provenance, and revision.
   No prompt, surface, model, harness, or add-on may redefine it.
2. **Intent is durable before dispatch.** Psyche commits the operator or surface
   intent, constraints, provenance, and graph mutation before requesting work.
3. **Orchestration is not execution authority.** Psyche decides graph structure
   and coordination. Coven independently admits and supervises bounded sessions.
4. **Capability is discovery, not permission.** Runtime, add-on, MCP, and
   marketplace metadata never grants authority.
5. **Unknown remains unknown.** Adoption, cancellation, delivery, verification,
   and recovery ambiguity is fenced rather than inferred.
6. **Evidence precedes success.** Tests and artifacts are preferred over model
   opinion. A generating node cannot certify its own output.
7. **Surfaces are adapters.** Channel identifiers and delivery semantics do not
   leak into core graph, identity, or verification contracts.
8. **Durability precedes acknowledgement.** Accepted external input and graph
   transitions are committed before acknowledging them.
9. **Budgets are honest.** A limit is called hard only when the execution layer
   can enforce it and report trustworthy consumption.
10. **Local-first is a trust property.** Core state, identity, audit, and
    recovery do not require a hosted control plane.
11. **Stable contracts outrank framework convenience.** Psyche does not couple
    its core to LangGraph, CrewAI, AutoGen, or another fast-moving orchestration
    framework.
12. **Architecture and release scope are distinct.** Core schemas may exist
    before the corresponding production capability is enabled.

## 3. Ownership boundaries

### 3.1 Psyche owns

- operator intent and its provenance;
- familiar identity resolution and continuity;
- surface-neutral conversations and context selection;
- durable orchestration graphs, nodes, dependencies, and delegation;
- graph admission, routing, budgets, leases, and coordination;
- orchestration approvals and their provenance;
- node-to-session correlation and graph recovery;
- verification policy, evidence inventory, verdicts, and escalation;
- add-on discovery, trust policy, lifecycle, and invocation records;
- surface routing, presentation, ingress, and delivery state;
- Psyche storage, audit, export, restore, health, and diagnostics; and
- capability negotiation and fail-closed feature gating.

These responsibilities do not grant authority inside a Coven-managed execution
session.

### 3.2 Coven owns

- project and working-directory validation;
- supported harness admission and process launch;
- PTY allocation and supervised session lifecycle;
- session records, ordered events, input, and termination;
- authoritative session adoption and terminal state;
- execution-layer approvals and enforcement exposed by a versioned contract;
- artifact and memory operations that Coven explicitly exposes; and
- rejection of requests outside its supported project, session, or policy
  boundary.

Coven does not own Psyche's graph, delegation policy, surface routing, familiar
identity source, or verification verdicts.

### 3.3 Harnesses own

- provider authentication;
- the conversation with the model provider;
- harness-native context and continuation behavior;
- internal tool discovery and invocation;
- harness-native approvals unless a versioned contract delegates them; and
- provider-specific output and error semantics.

Session supervision is not daemon-mediated tool execution. Psyche and Coven do
not claim to inspect, authorize, or record individual harness tool calls under
the current session model.

### 3.4 Surfaces own

- protocol authentication and transport mechanics;
- channel-native actor and location identifiers;
- rendering and interaction affordances;
- protocol-specific acknowledgement and delivery semantics; and
- normalized translation into and out of Psyche's canonical contracts.

A surface never defines familiar identity or graph authority.

Surface authentication establishes a protocol actor and locator, not a Psyche
principal. Psyche maps that observation to a configured principal and
independently authorizes intent and approvals. Missing, stale, or conflicting
bindings fail closed.

### 3.5 Approval domains

Psyche owns orchestration approvals: delegation, budget expansion, accepting a
verification exception, changing a graph, and authorizing a surface effect
under configured surface policy. The adapter performs the authorized transport.

Coven independently enforces approvals required by its execution contracts.
Harness-internal approvals remain harness-owned unless an explicit versioned
contract says otherwise.

An approval in one domain never authorizes another domain by implication.

## 4. Runtime components

### 4.1 Identity kernel

The identity kernel resolves and snapshots:

- `familiar_id`;
- familiar name and lane within the current Coven;
- principal binding;
- `IDENTITY.md` and `SOUL.md` digests;
- role and skill configuration digests;
- aggregate identity digest;
- familiar revision;
- provenance for every identity input.

It rejects missing, contradictory, unsafe, or changing identity inputs. A
deliberate identity change creates a new revision and requires an audited
rebind. Existing graph nodes and sessions retain their original snapshot.

Project bindings are separate authorization constraints. They never alter the
familiar identity or its digest.

Coven may validate and bind the immutable snapshot to a session. It does not
synthesize the familiar's identity.

### 4.2 Intent ledger

The intent ledger durably records:

- the operator or surface actor;
- source surface and source locator;
- normalized request;
- requested outcome and acceptance evidence;
- constraints, deadlines, and budgets;
- approval requirements;
- identity and policy revisions;
- immutable payload digest;
- parent intent, when present; and
- provenance and redaction metadata.

An accepted intent is immutable. Corrections create a superseding intent rather
than rewriting history.

### 4.3 Orchestration graph

The graph is a durable directed acyclic graph. Admission rejects cycles. Every
node contains:

- stable `graph_id` and `node_id`;
- parent and dependency references;
- immutable task input;
- familiar identity snapshot;
- requested capability profile;
- project and working-directory intent;
- acceptance and verification requirements;
- budget reservation and accounting policy;
- cancellation policy;
- result and artifact references;
- durable execution attempts with stable request IDs and payload digests;
- Coven session correlation, when adopted;
- lease and recovery metadata; and
- terminal reason.

The graph engine owns readiness, delegation, dependency release, cancellation
propagation, verification scheduling, result adoption, and final aggregation.
It does not execute harness work itself.

### 4.4 Capability router

The router matches graph requirements against:

- reviewed familiar roles and skills;
- configured project bindings;
- accepted runtime capabilities;
- current Coven capability advertisement;
- add-on allowlists and integrity state;
- surface availability;
- budget and concurrency policy; and
- operator routing constraints.

Descriptions, annotations, package claims, and model-generated capability
statements are untrusted hints. They cannot expand a reviewed capability set.
Coven independently validates every requested session.

### 4.5 Context and memory coordinator

The coordinator builds bounded context from:

- immutable intent inputs;
- the selected familiar identity snapshot;
- dependency results and admitted artifacts;
- surface conversation observations;
- approved skill contributions; and
- authorized memory reads.

Every contribution retains source and trust metadata. Context selection is a
Psyche responsibility; memory authority remains with the memory contract that
serves the data. Psyche does not silently convert graph output into durable
memory.

### 4.6 Verification engine

Verification is part of the graph, not an afterthought. Each executable node
declares an evidence policy:

1. deterministic checks, tests, schemas, or external task oracles;
2. immutable artifact and available trajectory inspection exposed by versioned
   contracts;
3. independent verifier execution using a different node, familiar identity
   snapshot, and Coven session;
4. confidence-gated escalation; and
5. human review when required evidence is absent or conflicting.

A generating node cannot certify its own correctness. Intrinsic self-critique
may improve presentation, but it is not correctness evidence.

The evidence set is sealed and content-addressed before independent
verification. A verdict binds the exact evidence digests it evaluated.

Plain final-answer model judging must be treated as weak evidence. Higher
confidence requires trajectory and artifact access, judge-generator separation,
and benchmarked performance on Psyche's own task distribution. Published
evaluation numbers are illustrative, not guaranteed operating points.

### 4.7 Surface adapters

Each adapter implements:

- authenticated ingress;
- normalization to `psyche.surface_event.v1`;
- actor and locator preservation;
- acknowledgement and adoption semantics;
- presentation of graph state, approvals, and verification;
- delivery attempts and ambiguity;
- protocol-specific rate limits and recovery; and
- translation from canonical surface effects.

Telegram is the first adapter and provides the first live conformance suite.
Core schemas contain no Telegram account, chat, topic, message, callback, or
Bot API fields.

### 4.8 Add-on host

V1 add-ons are operator-approved, explicitly allowlisted, signed or
provenanced, pinned to an immutable reviewed digest, revocable, and audited per
invocation. MCP, package, and marketplace metadata is untrusted input and never
authority.

Process supervision, bounded protocol messages, minimal environment, temporary
working directories, and secret minimization reduce accidents. They do not
make same-user code untrusted or contained.

An untrusted marketplace tier requires a separate containment design. WASI or
another enforceable isolation mechanism may be evaluated in a future
experiment; Psyche makes no v1 sandbox or privilege-reduction claim.

### 4.9 Coven client

The client:

- negotiates exact API and capability versions;
- distinguishes current contracts from proposed prerequisites;
- persists stable intent IDs and request digests;
- requests session creation with an idempotency key and records Coven's
  authoritative adoption resolution;
- consumes ordered events through durable cursors;
- submits input and cancellation;
- reads authoritative session status;
- fences unknown adoption or termination outcomes; and
- exposes no local success fallback.

### 4.10 Operations core

The operations core provides:

- transactional storage and migrations;
- leases and single-owner processing;
- redacted audit and metrics;
- structured diagnostics;
- crash recovery and reconciliation;
- quarantine and operator repair;
- retention and privacy controls;
- checksummed export and restore; and
- release and schema compatibility checks.

## 5. Canonical contracts

The initial surface-neutral contract set is:

| Contract | Purpose |
|---|---|
| `psyche.identity_snapshot.v1` | Familiar, principal, source digests, provenance, and revision. |
| `psyche.intent.v1` | Immutable operator or surface request, outcome, constraints, provenance, and digest. |
| `psyche.surface_event.v1` | Normalized authenticated input with adapter-owned actor and locator data. |
| `psyche.graph.v1` | Graph identity, owner, policy, root intent, state, and aggregate result. |
| `psyche.graph_node.v1` | Immutable task, familiar snapshot, authorization constraints, dependencies, limits, state, and result. |
| `psyche.delegation.v1` | Parent-child relationship, delegated authority, acceptance criteria, and cancellation policy. |
| `psyche.budget.v1` | Reserved, consumed, and released accounting by resource class. |
| `psyche.approval.v1` | Psyche orchestration approval request, provenance, decision, and expiry. |
| `psyche.execution_binding.v1` | Stable attempt and request IDs, payload digest, Coven adoption resolution, event cursor, and terminal correlation. |
| `psyche.evidence.v1` | Immutable test, artifact, trajectory, verifier, or human evidence reference. |
| `psyche.verdict.v1` | Verification policy, evidence set, independent verifier, confidence class, and decision. |
| `psyche.recovery.v1` | Lease, ambiguity, fence, reconciliation, and operator resolution state. |
| `psyche.addon.v1` | Pinned package identity, provenance, contributions, allowlist, and revocation state. |
| `psyche.surface_effect.v1` | Canonical presentation or interaction intent before adapter translation. |
| `psyche.delivery.v1` | Logical effect, physical attempts, authorization, ambiguity, and resolution. |

Adapter contracts extend but do not replace this set:

- `psyche.telegram_event.v1`;
- `psyche.telegram_effect.v1`; and
- Telegram-specific delivery and parity fixtures.

Unknown major versions fail closed. Persisted unknown records are quarantined.

## 6. Graph and node lifecycle

### 6.1 Graph states

```text
draft -> admitted | rejected
admitted -> running
running -> waiting_approval | waiting_evidence | cancelling
running -> completed | failed
waiting_approval -> running | cancelling | failed
waiting_evidence -> running | cancelling | failed
cancelling -> cancelled | recovery_required
recovery_required -> running | completed | failed | cancelling
```

A graph completes only when its required nodes are terminal, its evidence
policy is satisfied, and its result aggregation commits.

Cancellation intent is valid from every nonterminal graph state. The graph
enters `cancelling` before propagation and cannot become `cancelled` while any
potentially adopted execution lacks authoritative terminal acknowledgement.

### 6.2 Node states

```text
proposed -> admitted | rejected
admitted -> blocked | ready
blocked -> ready | skipped
ready -> reserved -> dispatching
dispatching -> adopted | adoption_unknown | proven_not_adopted | failed
adoption_unknown -> adopted | proven_not_adopted | recovery_required
proven_not_adopted -> ready | failed
adopted -> running | candidate | failed
running -> waiting_approval | candidate | failed
waiting_approval -> running | candidate | failed
candidate -> awaiting_verification
awaiting_verification -> verified | rejected | escalation_required
escalation_required -> awaiting_verification | verified | rejected
cancelling -> cancelled | termination_unknown
termination_unknown -> cancelled | candidate | failed | recovery_required
recovery_required -> adopted | running | candidate | failed | cancelled
```

`candidate` means execution ended and produced a candidate result. `verified`
means the declared evidence policy accepted it. Cancellation intent is valid
from every nonterminal node state. A pre-adoption node may cancel locally and
release its unused reservation; a potentially adopted execution must enter
`cancelling` and await Coven's authoritative terminal state.

An authoritative terminal observation may move an adopted node directly to
`candidate` or `failed`, including during restart recovery. A retry creates a
new durable attempt with a new attempt ID under the same immutable node and task
digest; prior attempts and evidence remain recorded.

### 6.3 Lifecycle invariants

- A node has one immutable identity snapshot and task digest.
- Each attempt has a stable execution request ID and immutable payload digest,
  uniquely bound one-to-one to a Coven session. A digest mismatch fences the
  attempt.
- Lost responses use authoritative adoption lookup. An adoption-unknown attempt
  is never redispatched until Coven proves it was not adopted.
- Dependency release occurs in the same transaction as its committed success
  predicate. Failure, rejection, skip, and cancellation propagate according to
  immutable graph policy.
- Reservations are hierarchical and idempotent. They are retained during
  adoption or termination ambiguity, charged or released exactly once, and
  accounted separately for every retry. Exhaustion blocks dispatch.
- Parent cancellation durably marks descendants before requesting session
  termination.
- Psyche owns cancellation intent and graph propagation. Coven owns termination
  of adopted sessions.
- A lease carries a monotonically increasing fencing token. Expiry makes work
  recoverable, not failed, and never by itself permits redispatch.
- Local fencing prevents further mutation; it is not proof that an ambiguous
  execution did not run.
- Results and artifacts bind immutably to graph, node, attempt, Coven session,
  and familiar identity revision before adoption.
- Descendants remain owned by the root graph and cannot outlive unresolved
  parent cancellation.
- A child cannot widen project, identity, budget, capability, or approval scope
  inherited from its delegation.
- Graph success requires evidence, not merely a successful process exit.

The parent-child, budget, and orphan invariants are engineering requirements
generalized from Coven's validated single-session adoption pattern. They are
not claimed as externally proven multi-agent standards.

## 7. Execution flow

1. A surface or operator submits a request.
2. The adapter authenticates the actor and preserves the exact source locator.
3. Psyche normalizes and durably commits the intent before acknowledgement.
4. The identity kernel resolves the target familiar and immutable revision.
5. Psyche admits the intent against local orchestration and surface policy.
6. The graph engine creates the root graph and explicit nodes.
7. The capability router selects reviewed familiars, add-ons, and a
   Coven-supported harness profile.
8. The context coordinator builds bounded, provenance-bearing node input.
9. Psyche reserves the node budget and commits the dispatch intent.
10. The Coven client requests a project-scoped session using a stable intent ID.
11. Coven independently validates the request and either rejects or supervises
    the harness session.
12. Psyche records Coven's authoritative adoption resolution and consumes
    ordered events for the bound session.
13. The harness conducts its provider conversation and internal tool loop.
14. Psyche records candidate output and admitted artifacts without inferring
    success from prose.
15. The verification engine runs deterministic evidence and, when required, an
    independent verifier node or human review.
16. Psyche accepts, rejects, retries, escalates, or delegates according to the
    committed graph policy.
17. Dependency release and result adoption commit transactionally.
18. Surface adapters present progress, approvals, evidence, and final results.
19. The graph reaches a terminal state after required evidence and result
    aggregation commit. Surface delivery remains separately durable and may
    remain ambiguous after graph terminality.

## 8. Cancellation, failure, and recovery

### 8.1 Failure classes

Psyche distinguishes:

- rejected identity or route;
- unsupported capability;
- denied session admission;
- dispatch adoption ambiguity;
- harness failure;
- session termination ambiguity;
- add-on invocation failure;
- budget exhaustion;
- verification rejection or inconclusive evidence;
- surface delivery ambiguity;
- storage or migration failure; and
- operator-required recovery.

These states never collapse into a generic success-shaped response.

### 8.2 Cancellation

Cancellation is a durable graph mutation:

1. record cancellation intent and actor;
2. stop admitting new descendants;
3. mark affected non-running nodes cancelled;
4. request Coven termination for adopted sessions;
5. await authoritative terminal states;
6. revoke outstanding Psyche approvals and unused reservations; and
7. aggregate the graph as cancelled only after every potentially adopted
   execution has authoritative terminal acknowledgement.

If an execution remains unknown, local fencing prevents further mutation but
does not prove cancellation. The graph remains `recovery_required`.

### 8.3 Restart recovery

On startup, Psyche:

1. validates schema and identity revisions;
2. acquires recovery leases;
3. resumes surface cursors without acknowledging uncommitted input;
4. queries Coven for dispatches in `adoption_unknown`;
5. resumes event cursors for adopted sessions;
6. reconciles terminal results and artifact references;
7. fences unknown or mismatched executions without redispatching them;
8. reclaims expired graph-node leases;
9. restores verification and approval waits; and
10. reports unresolved ambiguity to the operator.

### 8.4 Surface delivery ambiguity

One logical surface effect has one immutable ID and separately recorded
physical attempts. Delivery ambiguity remains durable until the adapter can
prove the outcome or an operator resolves it. Unrelated channel activity cannot
be used as proof.

## 9. Verification strategy

### 9.1 Evidence hierarchy

| Level | Evidence | Use |
|---|---|---|
| E0 | Schema, invariant, and deterministic policy checks | Required for all nodes. |
| E1 | Tests, interpreters, external services, or task-specific oracles | Preferred correctness evidence. |
| E2 | Immutable artifact and available trajectory inspection exposed by versioned contracts | Required for non-trivial agent work. |
| E3 | Independent verifier node with a distinct familiar identity snapshot and Coven session | Used when deterministic evidence is incomplete. |
| E4 | Human review | Required for policy, ambiguity, or configured high-risk work. |

The generating node's reflection is metadata, not an evidence level.

### 9.2 Research basis

Sage's reviewed evidence supports these conclusions:

- unaided intrinsic self-correction can degrade reasoning accuracy
  (**confidence: high**);
- same-model judges exhibit self-preference and position bias
  (**confidence: high**);
- deterministic or tool-grounded feedback is stronger than intrinsic critique
  (**confidence: high**);
- trajectory and artifact access improves agent evaluation
  (**confidence: medium-high; local validation required**).

Plain LLM judges have shown substantially below adequate precision on certain
published agent-trajectory benchmarks. Those external results are illustrative,
not a guaranteed operating point for Psyche's task distribution. Psyche
therefore treats an unbenchmarked plain judge as advisory, not authoritative.

These findings inform the verification architecture. They do not establish
Psyche-specific precision, cost, or safety guarantees.

### 9.3 Required verification tests

- A node cannot verify itself.
- The evidence set is sealed and content-addressed before verification.
- A verifier cannot read mutable evidence after the evidence set is sealed.
- A verdict binds the exact evidence digests it evaluated.
- Reordering pairwise candidates cannot silently change a deterministic gate.
- Missing required evidence blocks dependent nodes.
- Conflicting evidence escalates rather than selecting the favorable result.
- Retry policy cannot erase prior failed evidence.
- Human overrides record actor, reason, evidence, and scope.
- Graph success cannot bypass a required verification node.

## 10. Add-on and marketplace trust

The add-on host enforces:

- operator approval before enablement;
- explicit package and capability allowlists;
- signed or otherwise reviewed provenance;
- immutable digest pinning;
- no capability inference from descriptions or annotations;
- revocation effective before the next invocation;
- per-invocation package, graph, node, identity, and request digests;
- bounded protocol messages and resource accounting;
- secret minimization and redaction; and
- stable failure diagnostics.

Signed code can still be malicious or semantically misleading. Integrity proves
which bytes ran, not that they were safe. Marketplace ingestion therefore
requires review, revocation, and audit even for signed packages.

## 11. Capability and release gates

### 11.1 Architecture capability flags

| Capability | Meaning |
|---|---|
| `psyche.graphs.v1` | Durable graph and node schemas are supported. |
| `psyche.singleNodeExecution.v1` | One graph node may bind to one Coven session. |
| `psyche.multiAgentExecution.v1` | Parent-child execution and recovery contracts pass conformance. |
| `psyche.hardBudgets.<resource>.v1` | Enforceable limits and trustworthy usage reporting exist for one named resource class. |
| `psyche.independentVerification.v1` | Distinct familiar and session execution plus sealed evidence access pass conformance. |
| `psyche.surface.telegram.v1` | Telegram adapter passes parity and reliability gates. |
| `psyche.addons.trusted.v1` | Trusted pinned add-on host passes security and lifecycle gates. |

Flags are computed fail-closed from exact negotiated Coven capabilities and
current conformance evidence. Configuration, adapters, or package metadata
cannot force-enable them.

### 11.2 Single-node production gate

Requires versioned Coven contracts for:

- session creation, input, status, events, and termination;
- familiar identity snapshot binding;
- idempotent session adoption lookup;
- authoritative adoption ambiguity fencing and quarantine recovery;
- authoritative terminal state; and
- required artifact or memory operations.

### 11.3 Multi-agent production gate

Requires:

- child execution correlation;
- idempotent child adoption;
- cancellation acknowledgement and propagation evidence;
- result and artifact association;
- authoritative terminal state; and
- orphan fencing and recovery.

Until these contracts exist and pass fake and real Coven conformance,
`psyche.multiAgentExecution.v1` is false. Graph authoring, inspection, and
simulation may exist without production child dispatch.

### 11.4 Hard-budget gate

Hard-budget flags are resource-class scoped. Without an enforceable limit and
trustworthy usage reporting for a particular resource class, Psyche budgets for
that class are admission, concurrency, and accounting controls, not hard
resource containment.

### 11.5 Automated-verifier gate

Requires a verifier node with a distinct familiar identity snapshot and Coven
session, sealed content-addressed evidence, a declared policy, and benchmark
evidence on Psyche's task distribution.

### 11.6 Surface release gates

Each production adapter requires its own authentication, parity, ambiguity,
privacy, crash recovery, canary, and rollback evidence. Telegram is intended to
satisfy the first adapter gate, but its capability remains false until that
evidence exists. It does not redefine the runtime core.

## 12. Subsystem decomposition

Comprehensive orchestration is delivered through separate plans:

| Subsystem | Independently testable result |
|---|---|
| Identity kernel | Surface-neutral snapshots, revisions, contradiction checks, and session bindings. |
| Intent ledger | Immutable normalized intents with provenance, supersession, and replay protection. |
| Graph store and state machine | Durable nodes, dependencies, leases, budgets, cancellation, and recovery without real harnesses. |
| Coven execution binding | One node dispatches, adopts, follows, cancels, and recovers one real session. |
| Multi-agent execution | Parent-child correlation, bounded delegation, propagation, result adoption, and orphan fencing. |
| Capability router | Reviewed capability matching with negative tests for metadata poisoning and unsupported contracts. |
| Context and memory coordinator | Bounded provenance-aware context and explicit memory operations. |
| Verification engine | Evidence policies, independent verifier nodes, escalation, and anti-self-certification invariants. |
| Trusted add-on host | Pinned packages, broker protocol, revocation, audit, and failure isolation. |
| Surface contract | Adapter-neutral ingress/effect schemas and conformance fixtures. |
| Telegram adapter | Full Telegram parity, delivery ambiguity, live canary, and rollback. |
| Operations and release | Diagnostics, privacy, migration, export/restore, packaging, and upgrade safety. |

No implementation plan should combine these into one unreviewable project.

## 13. Research confidence and open questions

### 13.1 High-confidence evidence

- Intrinsic self-critique is insufficient correctness evidence.
- Same-model judging is biased.
- External tests and tool-grounded feedback strengthen verification.
- Trained process verifiers require substantial domain-specific labeled data.
- Add-on descriptions and annotations are attack surfaces.

### 13.2 Evidence requiring local validation

- Trajectory and artifact access can improve agent evaluation
  (**confidence: medium-high**), but published results do not establish
  Psyche-specific operating performance.

### 13.3 Engineering inferences

- The exact parent-child adoption, cancellation, budget, and orphan invariants
  generalize Coven's single-session durability pattern to graphs.
- All marketplace metadata, not only MCP tool descriptions, should be treated as
  untrusted input.
- A capability router should join familiar, runtime, project, surface, and
  add-on constraints at graph scale.
- Framework churn argues for stable internal contracts.
- The task volume that justifies building a trained verifier is an open design
  threshold.

These are explicit design choices requiring conformance evidence, not claims of
external consensus.

### 13.4 Open research and design questions

- The best verifier integration point for different graph node classes.
- Psyche-specific precision, latency, and cost for trajectory-aware review.
- Whether recurring triggers belong in Psyche or a future Coven contract.
- The minimum enforceable budget and usage contract across supported harnesses.
- The containment boundary, if any, for a future untrusted marketplace tier.
- Cross-host graph execution and portability beyond the local-first core.

Sage's broader orchestration and portability research remains checkpointed.
Cloud transports, A2A, NATS, gRPC, WebRTC, and multi-cloud designs are not
adopted by this document.

## 14. Acceptance criteria for this design

The W0 package is ready for G1 specification-coherence verification only when:

1. Val approves the product boundary, ownership matrix, components, lifecycle,
   verification model, and release gates;
2. Nova confirms identity, authority, approval, adoption, and recovery
   coherence;
3. Sage confirms the research claims and confidence labels are faithful;
4. the six companion Psyche documents share this product and ownership model;
5. Telegram remains an adapter rather than a core product boundary;
6. Coven prerequisites are behavior-level W1 hypotheses, not assumed current
   contracts or implementation assignments; and
7. repository creation and W1 remain blocked until G1, while implementation
   plans, issues, and production code remain blocked until G3.

W1 classifies current Coven contracts before G3; each implementation
workstream then receives its own dependency-gated child plan. No production
implementation begins from this design document alone.
