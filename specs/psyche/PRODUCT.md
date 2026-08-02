# Psyche Product Specification

**Status:** Approved product baseline - W0 reconciled and G1 verified 2026-08-01
**Work unit:** `coven-psy0`
**Product home:** standalone `OpenCoven/psyche` repository
**Canonical decision:** [Familiar runtime design](./RUNTIME_DESIGN.md)
**Companions:** [Decision dossier](./DECISION_DOSSIER.md), [Technical architecture](./TECH.md), [Threat model](./THREAT_MODEL.md), [Telegram parity ledger](./TELEGRAM_PARITY.md), [Coven prerequisites](./COVEN_PREREQUISITES.md), [Program plan](./PLAN.md)

## Product decision

Psyche is the clean-room, local-first, surface-neutral familiar runtime for a
Coven. It turns operator or surface intent into durable, reviewable
orchestration graphs while preserving familiar identity, execution authority,
evidence, and recovery across harnesses and human-facing surfaces.

Telegram is the first production and conformance adapter. It does not define
Psyche's permanent product boundary. Cave, CLI, mobile, webhooks, and later
surfaces must use the same canonical intent, graph, identity, verification,
and effect contracts.

Psyche is not an OpenClaw fork, a harness, a model provider, a generic agent
framework, or a second Coven authority layer.

Psyche owns:

- familiar identity resolution and immutable snapshots;
- durable intent, conversation, graph, node, attempt, and recovery state;
- graph authoring, dependencies, delegation, budgets, and coordination;
- verification policy, sealed evidence, verdicts, and escalation;
- surface-neutral ingress, effects, routing, and delivery state;
- trusted add-on discovery, lifecycle, and invocation records; and
- Telegram transport and Bot API adaptation as the first adapter.

Coven owns:

- project and working-directory validation;
- supported harness admission and supervised session lifecycle;
- session input, termination, ordered events, and authoritative terminal state;
- execution-layer approvals and protected resources exposed through versioned
  contracts;
- familiar snapshot validation and immutable execution binding where the
  negotiated contract supports it; and
- rejection of unsupported or out-of-bound execution requests.

Harnesses own provider authentication, model conversations, harness-native
tool discovery and invocation, and harness-native approvals unless a versioned
contract explicitly delegates one boundary. Surfaces own protocol
authentication and transport mechanics, but never familiar identity or graph
authority.

Psyche may ask Coven to execute bounded work. It may not substitute a local
decision when Coven rejects a request, lacks a capability, returns an unknown
contract version, or cannot resolve adoption, cancellation, or terminal state.

## Problem

OpenCoven can supervise harness sessions, but it does not provide a durable
surface-neutral intent ledger, orchestration graph, familiar identity kernel,
verification engine, or adapter delivery ledger. Individual harnesses and
channel connectors cannot safely own those cross-session responsibilities
without coupling familiar continuity to one provider or duplicating Coven's
execution boundary.

Psyche supplies that missing runtime layer. A familiar remains itself when the
harness, model, or surface changes; operators can inspect and recover durable
work; and Telegram users receive reliable responses without giving a channel
adapter direct authority over local projects or harness tools.

## Users

| User | Need |
|---|---|
| Principal | Reach a named familiar from Telegram without weakening its identity or approval rules. |
| Group member | Address an authorized familiar in a group or topic and receive correctly threaded replies. |
| Familiar maintainer | Define identity, roles, skills, routes, and channel behavior in reviewable files. |
| Operator | Declare intent, inspect graphs and evidence, resolve ambiguity, configure adapters, migrate safely, and roll back. |
| Verifier | Evaluate a sealed evidence set independently from the node that produced the candidate result. |
| Surface maintainer | Implement a protocol adapter against canonical Psyche ingress and effect contracts. |
| Coven maintainer | Classify and implement only the versioned execution contracts proven necessary by W1. |

## Product principles

1. **Identity precedes conversation.** No update reaches a harness until the
   route's familiar declaration, `IDENTITY.md`, `SOUL.md`, and role/skill
   configuration resolve to one coherent familiar.
2. **Intent is durable before dispatch.** Psyche commits the operator or
   surface intent, constraints, provenance, and graph mutation before asking
   Coven to execute a node.
3. **Orchestration is not execution authority.** Psyche owns graph structure;
   Coven independently admits and supervises bounded execution.
4. **Capability is discovery, not permission.** Runtime, add-on, MCP, and
   marketplace metadata never grants authority.
5. **Unknown remains unknown.** Adoption, cancellation, delivery,
   verification, and recovery ambiguity is fenced rather than inferred.
6. **Evidence precedes success.** A generating node cannot certify its own
   output; declared evidence and independent review gate success.
7. **Surfaces are adapters.** Protocol identifiers and delivery behavior do
   not leak into core identity, graph, or verification contracts.
8. **Durability precedes acknowledgement.** An accepted surface event is
   committed locally before protocol acknowledgement or cursor advancement.
9. **Wrong-surface success is failure.** Psyche never drops topic metadata,
   changes the account, or falls back to a different chat to make a send pass.
10. **One logical response is one visible response.** Streaming edits one
   persistent preview and finalizes it in place whenever Telegram permits.
11. **Authorization uses stable identifiers.** Telegram numeric user and chat
   IDs are authoritative; mutable usernames are presentation metadata.
12. **Unknown means denied.** Unknown API versions, capabilities, callback
   types, identity states, policies, and route states fail closed.
13. **Clean-room behavior, original implementation.** Publicly observable
   behavior may inform requirements. OpenClaw code, internal organization,
   names, text, tests, fixtures, assets, and private state are not copied.

## Goals

- Preserve familiar identity independently from prompts, surfaces, models,
  harnesses, and add-ons.
- Persist immutable intent and durable graph/node/attempt state before
  execution.
- Author and simulate multi-node graphs while production child dispatch stays
  disabled until G6 passes.
- Execute one bounded node through a negotiated, conformant Coven session.
- Require deterministic evidence or human review for verified-success claims;
  enable independent verifier nodes only after G5.
- Expose inspectable recovery for adoption, cancellation, verification, and
  delivery ambiguity without local force-success paths.
- Deliver authorized direct-message conversations with pairing and allowlists.
- Deliver authorized group and forum-topic conversations with mention policy,
  bounded context, and deterministic familiar routing.
- Preserve DM topics when Telegram enables threaded direct messages.
- Support native and custom commands, callbacks, inline buttons, polls,
  reactions, replies, quotes, edits, deletes, typing, and acknowledgements.
- Support photos, albums, files, audio, voice notes, video, video notes,
  stickers, locations, and captions.
- Support persistent streaming previews without duplicate final messages.
- Support multiple bot accounts with explicit defaults and isolated state.
- Support long polling and authenticated webhooks with equivalent semantics.
- Recover accepted work after process crashes without losing authorization or
  ordering metadata.
- Expose enough health, metrics, and audit context to operate the service
  without logging message secrets or bot credentials.
- Migrate from an existing Telegram bot runtime with measurable canary and
  rollback gates.

## Non-goals

- Reimplementing Coven's daemon, session ledger, project boundary checks,
  harness supervision, or versioned execution/resource enforcement.
- Running model or harness provider SDKs directly from Psyche.
- Claiming that Coven mediates individual harness tool calls under the current
  session model.
- Supporting Telegram user accounts through MTProto; Psyche uses the Bot API.
- Cloud multi-tenancy or a hosted OpenCoven control plane.
- Importing OpenClaw databases, source code, credentials, conversations,
  caches, hidden memory, or runtime state. Compatibility is limited to
  reviewable prompts, declarations, hooks, commands, and configuration.
- Automatic Telegram-triggered edits to protected familiar or operator
  configuration.
- Exposing private chain-of-thought or provider reasoning streams.
- Replacing Coven Cave's desktop or mobile control-room experience.
- Shipping a production adapter other than Telegram in the first release.
- Enabling production child-session delegation before G6.
- Treating same-user Node add-ons as an untrusted security boundary.
- Recurring schedules, cross-host orchestration, or a hosted control plane
  without separate product decisions and enforceable contracts.

## Identity contract

Every accepted intent names exactly one `familiar_id`, one identity snapshot,
one principal mapping, and one Coven project scope. Psyche resolves familiar
identity from operator-controlled declarations before graph admission:

1. resolves the familiar declaration from an operator-configured familiar home;
2. opens `IDENTITY.md` and `SOUL.md` without following symlinks;
3. resolves the declaration's roles and skill configuration;
4. verifies that all sources name the same familiar and contain no conflicting
   role, principal, or governance declarations;
5. computes a content digest and immutable revision over the resolved inputs;
6. persists the snapshot and provenance with the intent and graph; and
7. requires Coven to validate and immutably bind the exact snapshot, project,
   node, and attempt to any execution session when that negotiated contract is
   available.

Missing files, unreadable files, unsafe paths, unknown familiar IDs, digest
changes during an active graph, or contradictions block admission. Psyche
reports a structured operator error and does not replace identity with a
prompt, account name, route label, surface actor, or model-generated persona.

Ward is Coven's protected-familiar write and audit gate. It may validate a
snapshot or fence execution after a protected change, but it is never the
source of familiar identity. A principal authorizes intent; a familiar defines
who performs it. Neither substitutes for the other.

The surface account is presentation, not identity. One Telegram bot may serve
multiple familiars only when every chat/topic route is unambiguous. A familiar
may use multiple surfaces while preserving one identity snapshot.

## Authority contract

Psyche is an untrusted Coven client for execution enforcement purposes. Coven
is an untrusted Psyche input for graph, identity-source, verification, and
surface-policy purposes. Each boundary accepts only its versioned contracts.

| Decision | Owner | Psyche behavior |
|---|---|---|
| Which protocol account and location received input | Surface adapter | Authenticate protocol state and normalize it without assigning familiar identity. |
| Which principal a surface actor maps to | Psyche | Apply configured mapping; missing or conflicting mappings fail closed. |
| Which familiar and project an intent names | Psyche | Resolve one immutable familiar snapshot and explicit project scope or reject admission. |
| Graph structure, delegation, budgets, and verification policy | Psyche | Persist the decision before dispatch and never widen a delegated envelope. |
| Whether a project path, harness, session, or protected resource operation is allowed | Coven | Request the exact versioned execution/resource contract and honor denial or ambiguity. |
| Provider conversation, harness-native tools, and native approvals | Harness | Observe only normalized session events; do not claim daemon mediation that does not exist. |
| Whether a surface effect is allowed | Psyche | Apply configured surface policy to the exact principal, effect, and destination. |
| How an allowed effect is transported | Surface adapter | Execute the immutable effect without changing its target or semantic class. |
| Whether a candidate satisfies declared acceptance evidence | Psyche verification policy | Require deterministic evidence, human review, or a distinct verifier as configured; the generator cannot self-certify. |

Capability discovery reports whether a boundary can evaluate a request class;
it is never authorization. Psyche surface policy authorizes exact effects and
destinations. Coven independently authorizes only execution and protected
resources exposed through negotiated contracts. A surface effect cannot widen
or bypass a Coven execution denial, and a Coven capability cannot authorize a
Telegram mutation by implication.

Every binding carries version, request digest, principal, familiar snapshot,
project, graph, node, attempt, policy revision, and expiry as applicable.
Missing, stale, mismatched, or unknown bindings fail closed. Proposed contract
names in this specification remain hypotheses until W1 classifies them from
current Coven code and executable tests.

An intentional familiar identity change is not an ordinary hot reload. Psyche
blocks affected graph admission and routes, records an operator-authorized
identity revision, and requires Coven to validate or fence execution bindings
where its negotiated contract applies. Existing sessions remain bound to the
old snapshot and are never resumed under the new identity.

## Core journeys

### Declare and complete familiar work

1. An authenticated operator or surface submits intent, constraints, project,
   familiar, budget, and required evidence.
2. Psyche resolves and snapshots identity and principal provenance, then
   commits the immutable intent before acknowledgement.
3. Psyche creates a durable graph. The first release may execute a single-node
   graph while preserving the same graph contract used for later workflows.
4. A node requests one bounded Coven-supervised session. Coven independently
   validates project, harness, snapshot binding, and supported policy.
5. Psyche correlates ordered session events and authoritative terminal state to
   exactly one graph attempt; ambiguity blocks redispatch.
6. Declared deterministic evidence or human review gates completion. A distinct
   verifier node is allowed only after G5; the generating node cannot certify
   itself.
7. An authorized canonical effect is rendered by the originating surface
   adapter, and both logical effect and physical attempt remain recoverable.

### First account setup

1. The operator stores a bot token in an approved secret provider.
2. Psyche config stores only the secret reference and expected numeric bot ID.
3. `psyche doctor` resolves the reference in memory, calls `getMe`, requires the
   returned bot ID to match, verifies available Coven contracts without
   inferring authority, validates routes and identities, and prints no secret
   or token-bearing URL.
4. The operator starts `psyched` in polling or webhook mode.
5. Health reports the account, transport, route, identity, and Coven state
   separately.

### Pair an owner in a direct message

1. An unknown numeric sender messages a route whose DM policy is `pairing`.
2. Psyche persists the update and creates a short-lived, one-time pairing
   request without dispatching the message to a familiar.
3. The principal approves the request through a local Psyche operator surface;
   any protected Coven resource remains subject to its independent decision.
4. Psyche records the approved numeric sender ID for that account and DM scope.
5. A new message starts an identity-bound Coven conversation.

Pairing never grants group access, action approval, or owner privileges by
implication.

### Converse in a group or topic

1. Psyche verifies the account, group, sender, topic, and mention policy.
2. It derives an ordered lane for the account, chat, and topic.
3. It supplies bounded observed context and explicit reply metadata to Coven.
4. Psyche requests a familiar-snapshot-bound Coven session for the graph node.
5. Psyche sends typing or an acknowledgement if policy permits.
6. Output is streamed into one message and finalized in the same topic.

### Request a sensitive action

1. Psyche or Coven emits an approval-required event in its own authority
   domain, with an opaque approval ID, action digest, expiry, and allowed
   approver principals.
2. Psyche renders the request only to configured Telegram approval surfaces
   and labels the owning authority domain.
3. A button press is checked against account, numeric sender, chat, expiry, and
   one-time callback state.
4. Psyche submits the decision to the owning authority.
5. That authority revalidates and decides. An approval in one domain never
   grants authority in another; the adapter performs only a separately allowed
   surface effect.

### Recover from failure

1. On restart, Psyche leases committed but unfinished updates and delivery
   intents.
2. It preserves per-chat/topic ordering and resumes from the last durable
   state.
3. Known retryable failures follow bounded backoff and Telegram `retry_after`.
4. Ambiguous outbound outcomes are reported as such rather than silently
   declared sent.
5. `psyche node inspect` and `psyche node reconcile` inspect an inconclusive
   Coven session adoption without resubmitting it. A possible adoption may be
   fenced only through an authoritative negotiated Coven recovery contract
   with explicit operator acknowledgement.
6. `psyche delivery inspect` shows the immutable effect, attempt history, and
   duplicate risk for an ambiguous Telegram mutation.
7. `psyche delivery resolve` can abandon, retry, or send a clarification only
   after a typed Psyche surface decision; retry and clarification require explicit
   duplicate-risk acknowledgement.
8. There is no generic lane-unblock or mark-sent command. Recovery completes
   only after the authoritative boundary records a durable disposition.

## Functional requirements

### Intents and graphs

- Every accepted request persists an immutable `psyche.intent.v1` with
  principal provenance, familiar snapshot, project, constraints, budgets,
  acceptance evidence, and originating surface reference.
- Every intent owns one durable graph with stable graph, node, and attempt IDs.
- Node dependencies are acyclic; graph mutation creates a versioned durable
  transition rather than rewriting history.
- Delegation is explicit, bounded, and non-widening. A child receives no
  authority, budget, evidence access, or surface right absent from its envelope.
- Lease expiry alone never proves an attempt safe to redispatch. Adoption,
  cancellation, and terminal ambiguity remain fenced until authoritative
  reconciliation.
- Graph authoring and simulation are in the first release. Production child
  dispatch remains disabled unless G6 passes without delaying the single-node
  Telegram vertical slice.

### Evidence and verification

- Each node declares required evidence before execution.
- Evidence is content-addressed and sealed before a verdict; changed artifacts
  create a new evidence set.
- Deterministic checks and human review are the initial trusted verdict paths.
- Independent model verification requires a distinct familiar identity and
  session, declared policy, sealed evidence, and G5 benchmark evidence.
- A generating node cannot certify its own output, and successful process exit
  alone cannot produce verified graph success.

### Surface contract

- Adapters normalize authenticated protocol input into versioned canonical
  observations without selecting familiar identity or graph authority.
- Psyche maps protocol actors to configured principals and rejects missing,
  stale, or conflicting mappings.
- Canonical effects are immutable and name the exact logical destination,
  reply relationship, semantic class, and payload digest.
- Adapters persist physical attempts separately from logical effects and never
  change destinations to convert a failed effect into success.
- Telegram-specific identifiers and delivery semantics remain inside the
  Telegram adapter and its parity ledger.

### Accounts and transports

- Each account has an immutable local ID, one secret reference, one transport,
  and isolated cursors, limits, and health.
- Multi-account configurations name an explicit default; there is no
  order-dependent fallback.
- Polling and webhook mode are mutually exclusive for one token.
- Webhooks validate Telegram's secret token before parsing or persistence.
- Long polling advances its offset only after durable adoption.
- A 409 conflict stops the account and requires operator-visible recovery.

### Authorization and routing

- DM policies are `pairing`, `allowlist`, `open`, and `disabled`; `open`
  requires an explicit wildcard.
- Group policies are `allowlist`, `open`, and `disabled`; fail-closed
  `allowlist` is the default.
- Group membership and group-sender authorization are separate checks.
- Pairing approvals apply to DMs only.
- Routes may match account, chat, and optional topic. Exact topic routes beat
  topic defaults, which beat group defaults. Equal-precedence matches are an
  error.
- Forum topics and enabled DM topics produce distinct conversations.
- Mention-gated groups do not dispatch ambient messages, but may retain a
  bounded authorized context window.

### Conversations

- A canonical conversation key includes familiar, principal, and surface-
  neutral location. The Telegram adapter location includes account, chat, and
  effective topic.
- Context identifies the current message separately from observed reply,
  forward, quote, and bounded history context.
- Context never claims arbitrary message hydration; only updates previously
  observed by Psyche may be recalled.
- A route identity change closes the current conversation and requires a new
  explicit start after validation.
- Replies always return through the inbound account and conversation surface
  unless a separately authorized send intent names another target.
- A Telegram group-to-supergroup migration changes `chat_id` and therefore
  starts a new conversation key. Psyche preserves the old context under
  retention policy but never bridges it automatically into the new chat.

### Delivery and interaction

- Text uses safe Telegram HTML by default and falls back to plain text on
  entity errors.
- Long text splits at Telegram-safe boundaries and retains reply/topic
  metadata.
- Streaming supports off, partial, block, and progress modes.
- A streaming preview has a configurable maximum age with a 10-minute default.
  Expiry attempts an authorized final edit from the latest cumulative content;
  it never leaves an active preview state indefinitely or sends an
  unauthorized fallback.
- Commands that do not need an agent turn are handled before session startup.
- Callback payloads remain typed and preserve their exact values.
- Polls, reactions, edits, deletes, topic actions, pins, and inline keyboards
  are individually capability- and policy-gated.
- Selected quote replies preserve Telegram's native quote limits and degrade to
  a normal reply, never an unrelated message.

### Media

- Inbound media is size-limited, content-inspected, stored outside project
  roots, and represented to Coven as untrusted channel input.
- Media downloads use Telegram-authenticated file metadata and approved
  origins; arbitrary URLs in messages are never fetched as media implicitly.
- Voice transcription, image description, and other derived text are labeled
  untrusted machine output.
- Outbound media preserves the distinction between audio and voice notes, and
  between video and video notes.
- Media groups are correlated without blocking unrelated conversation lanes.

### Operations

- `psyche doctor` validates config, secret references, Telegram reachability,
  webhook/polling conflicts, negotiated Coven contracts, familiar identity,
  principal mappings, graph/store invariants, routes, storage, and filesystem
  permissions.
- `psyche doctor --json` emits `psyche.doctor_report.v1`; human output is a
  rendering of the same checks, reason codes, and remediations.
- Health distinguishes `ready`, `degraded`, and `blocked`, with machine-readable
  reason codes.
- adoption, cancellation, verification, and delivery unknown states have
  explicit inspect and authoritative resolve paths; none can be cleared by
  editing local state.
- Metrics exclude message text, callback values, tokens, raw IDs, and local
  paths by default.
- Every sensitive operator action and approval bridge event carries a
  correlation ID shared with Coven.

## Data handling and retention

Default local retention:

| Data | Default | Notes |
|---|---:|---|
| Immutable intents, graphs, nodes, attempts, and decisions | 90 days after terminal disposition | Unresolved or ambiguous work is retained until an authoritative disposition plus the configured recovery window. |
| Familiar identity and principal-mapping snapshots | Lifetime of referencing graph plus 90 days | Stores digests and provenance; secrets are excluded. |
| Coven request/adoption keys and ordered cursors | At least the greater of the configured graph-recovery window and every enabled adapter deduplication window | G4 is blocked unless Coven proves compatible authoritative retention; Psyche never assumes a fixed daemon duration. |
| Sealed evidence sets and verdicts | 90 days after graph disposition | Artifact bytes may use a shorter explicit policy only when immutable references remain diagnosable. |
| Raw accepted Telegram update | 7 days | Encrypted at rest when an OS-backed key is available; otherwise startup requires explicit operator acceptance. |
| Normalized message content and observed context | 30 days | Per-route reduction is supported; zero disables historical context after processing. |
| Downloaded media | 24 hours | Deleted after successful turn adoption unless retained by explicit policy. |
| Delivery and deduplication metadata | 30 days | Content-free keys may outlive message content to prevent replay. |
| Security and operator audit metadata | 90 days | Contains hashes and reason codes, not secrets or full message bodies. |
| Bot token | 0 days | Resolved only in memory from a secret reference; never persisted by Psyche. |

Retention cleanup is transactional and auditable. Legal hold and backup are
operator responsibilities and require a separate explicit configuration.
Before canary or production cutover, `psyche export` must produce a
checksummed, encrypted, restore-tested artifact containing the retained state
needed to recover unresolved ingress, intents, graphs, identity snapshots,
Coven adoption, evidence, delivery, routing, and audit records. Bot tokens and
secret-provider values are never exported.

## Service objectives

W0 separates surface-neutral objectives from adapter objectives. Core
durability and security objectives apply to every release:

- zero unauthorized familiar dispatches or approval decisions;
- zero graph-success claims without the declared evidence or human disposition;
- zero redispatches based only on lease expiry or ambiguous adoption;
- 100% replay of accepted intents acknowledged after durable commit in
  crash-injection tests;
- 100% correlation of execution sessions, terminal results, and retained
  artifact references to one graph node and attempt;
- 100% explicit unresolved state for unknown adoption, cancellation,
  verification, or delivery; and
- no plaintext secret findings in release artifacts, logs, crash reports, or
  command-line process listings.

The first Telegram adapter release additionally targets:

- 100% replay of Telegram updates acknowledged after durable commit in
  crash-injection tests;
- at least 99.9% successful webhook durable acknowledgements within 2 seconds,
  excluding Telegram or host outages;
- p95 committed-update-to-Coven-adoption latency below 2 seconds under the
  documented single-host load profile;
- p95 Coven-output-to-first-preview latency below 1 second when Telegram is
  healthy;
- fewer than 1 duplicate visible delivery per 10,000 logical deliveries, with
  every ambiguous outcome counted and exposed.

Telegram does not provide an idempotency key for sends. Psyche therefore cannot
promise exactly-once visible delivery across every network ambiguity. The
technical contract defines explicit `delivery_unknown` handling instead of
hiding that limitation.

### Normative single-host load profile

`psyche.load.v1` runs on an otherwise idle AWS `c7gd.xlarge` reference runner
(Graviton3/arm64, 4 vCPUs, 8 GiB RAM, local NVMe) using the pinned OpenCoven
Ubuntu 24.04 image, no swap, and a 20 GiB ext4 data-volume quota. It uses
release-mode binaries, one `psyched` process, one Coven daemon process, and
no unrelated workload. The benchmark report records instance type, CPU model,
microcode, kernel, Rust version, image ID, and Psyche/Coven commit hashes;
results from another fingerprint cannot satisfy the gate.

The versioned workload manifest is:

```json
{
  "schema_version": "psyche.load.v1",
  "warmup_seconds": 900,
  "measurement_seconds": 3600,
  "accounts": [
    { "id": "poll-1", "transport": "polling", "lanes": 25 },
    { "id": "poll-2", "transport": "polling", "lanes": 25 },
    { "id": "hook-1", "transport": "webhook", "lanes": 25 },
    { "id": "hook-2", "transport": "webhook", "lanes": 25 }
  ],
  "baseline_updates_per_second": 20,
  "burst": {
    "total_updates_per_second": 50,
    "start_seconds": [600, 1200, 1800, 2400, 3000],
    "duration_seconds": 60
  },
  "payload_cycle": [
    "text", "text", "text", "text", "text", "text", "text",
    "text", "text", "text", "text", "text", "text",
    "command", "callback", "reaction_change", "poll",
    "media", "media", "service_event"
  ],
  "coven_output": {
    "dispatch_types": ["text", "command", "callback", "poll", "media"],
    "first_answer_delta_ms": 100,
    "answer_delta_interval_ms": 50,
    "answer_delta_count": 4,
    "final_event_ms": 300,
    "final_text_bytes_cycle": [512, 4000, 8000],
    "progress_every_nth_turn": 10,
    "progress_event_ms": [50, 75]
  }
}
```

Bursts replace, rather than add to, the baseline rate. Events use fixed
intervals (50 ms baseline, 20 ms burst), rotate accounts and then lanes in
lexical order, and repeat the 20-entry payload cycle exactly. Text alternates
512 and 4,000 UTF-8 bytes. Command and callback payloads are 128 bytes.
Reaction/poll payloads contain four options or reactions. Media is 256 KiB,
except every twentieth media event is 10 MiB. Service events alternate member
and topic events. The generated JSONL event sequence is committed with the
implementation and its SHA-256 is recorded in every benchmark report.

The fake policy starts turns only for the five listed dispatch types.
For every accepted turn, fake Coven emits four cumulative answer deltas at
100, 150, 200, and 250 ms, then a final event at 300 ms. Final UTF-8 text sizes
repeat 512, 4,000, and 8,000 bytes by turn index. Every tenth turn also emits
two fixed 64-byte progress events at 50 and 75 ms. Output bytes are generated
from the turn index and a fixed ASCII/Unicode template, included in the
committed JSONL trace, and covered by the same reported SHA-256.

The fake Telegram service returns every scheduled inbound update at its target
time and delays every successful outbound response by exactly 50 ms. The fake
Coven authority performs real JSON/schema validation, canonical digesting,
idempotency persistence, SQLite writes, identity/Ward checks, and policy
decisions, then delays each accepted decision by 10 ms; it performs no
harness/model inference and injects no faults. Separate reliability profiles
cover errors and retries.

Committed-update-to-Coven-adoption is measured from the successful Psyche
ingress commit to Coven's idempotent adoption timestamp.
Coven-output-to-first-preview is measured from the committed Coven output event
to Telegram's successful response. P95 is the nearest-rank percentile across
all scheduled samples; failed or missing samples remain failures and are not
dropped from the report.

Only declared host maintenance and independently confirmed Telegram outages are
excluded. Queueing, storage, authorization, retries, and Psyche/Coven local
transport time remain included. Harness/model settlement is excluded because
neither latency objective measures inference. The G11 live canary additionally
reports real Telegram network latency and may not substitute synthetic results.
The former seven-day and 1,000-update canary values remain provisional planning
thresholds until operators approve the observation window and volume before
G11.

## Release and migration gates

| Gate | Required evidence | Blocks |
|---|---|---|
| G0 - Decision approval | Passed 2026-07-31: Val approved `RUNTIME_DESIGN.md` and `DECISION_DOSSIER.md` after Nova and Sage review. | W0 reconciliation. |
| G1 - Specification coherence | Product, technical, threat, prerequisites, parity, and program documents share one surface-neutral product and ownership model. | Standalone repository creation and the W1 contract audit. |
| G2 - Contract foundation | Canonical schemas, migrations, fake services, state-machine/property tests, and unknown-version denial pass. | Real execution integration. |
| G3 - Coven audit | W1 classifies every required contract from current code/test evidence and assigns owners only to accepted gaps. | Implementation child plans, issues, code, and Coven assignments. |
| G4 - Single-node conformance | The unmodified fake contract suite passes against pinned real Coven, including denial, restart, cancellation, one-attempt/one-session binding, digest mismatch, and ambiguity. | Real surface routes. |
| G5 - Verification | Deterministic gates pass; independent verification additionally proves distinct familiar/session identity, sealed evidence, declared policy, and locally approved benchmark thresholds. | Automated verified-success claims. |
| G6 - Multi-agent conformance | Non-widening delegation, child correlation/adoption, lease fencing, once-only budget accounting, descendant cancellation, result/artifact association, and orphan recovery pass. | Production child dispatch. |
| G7 - Trusted add-ons | Approval, allowlisting, digest pinning, provenance, revocation, invocation audit, protocol denial, crash, and security evidence pass. | Trusted add-on activation. |
| G8 - Adapter reliability | Fake-surface, crash, security, ambiguity, and parity evidence pass repeatedly. | Live Telegram. |
| G9 - Live Telegram | Every required live parity row passes twice on dedicated non-production accounts and two client families. | Canary. |
| G10 - Operations | Doctor, retention, privacy, export/restore, incident response, migration, token rotation, and rollback drills pass; a release security review finds no open critical or high-severity issue. | Production cutover. |
| G11 - Canary | Approved core and Telegram service objectives hold for the approved window and volume with zero unauthorized dispatch. | General release. |
| G12 - Distribution | Signed/checksummed artifacts, SBOM, provenance, clean-host install, and rollback under threshold pass. | Publication. |

No date, issue state, capability advertisement, or merged implementation can
override a gate. The standalone `OpenCoven/psyche` repository is created after
G1 and before W2 implementation.

Rollback is mandatory if any unauthorized dispatch occurs, accepted updates are
lost, identity binding disagrees, secrets appear in output, or duplicate
delivery exceeds 1 per 1,000 deliveries in a rolling hour. Only one runtime may
own a bot token at a time. Rollback stops Psyche, records uncertain deliveries,
removes its webhook or poller, and starts the previous runtime under the named
operator.

## Acceptance

This product specification is accepted when:

1. all companion documents use the same ownership and identity boundaries;
2. every in-scope Telegram capability has one classification in the parity
   ledger;
3. Telegram is consistently an adapter rather than the product boundary;
4. graph authoring and simulation are distinct from G6-gated production child
   dispatch;
5. versioned schemas and error semantics are implementation-ready;
6. threat mitigations map to testable controls;
7. W1, rather than W0 inference, owns current Coven contract classification;
8. operator recovery, diagnostics, and minimum backup behavior are
   implementation-ready;
9. core and Telegram service objectives remain separate;
10. OpenClaw compatibility excludes credentials, databases, conversations,
    hidden memory, caches, and runtime state;
11. no placeholder or unstated ownership remains; and
12. G1 passes before repository creation or W1, and G3 passes before
    implementation planning, issues, or production code begin.
