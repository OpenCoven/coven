# Psyche Product Specification

**Status:** Proposed v1 - design approval required
**Work unit:** `coven-psy0`
**Product home:** standalone `OpenCoven/psyche` repository
**Companions:** [Technical architecture](./TECH.md), [Threat model](./THREAT_MODEL.md), [Telegram parity ledger](./TELEGRAM_PARITY.md)

## Product decision

Psyche is a clean-room, local-first Rust service that gives one or more
OpenCoven familiars a production-grade Telegram presence. It is not an
OpenClaw fork, a harness, a model provider, or a second Coven authority layer.

Psyche owns:

- Telegram transport and Bot API adaptation;
- durable channel ingress and delivery state;
- conversation-to-familiar routing;
- channel-side identity consistency checks; and
- presentation of Coven output and approval requests in Telegram.

Coven owns:

- project and working-directory boundaries;
- session creation, input, termination, and event history;
- effective familiar validation and session identity binding;
- tool and capability policy;
- memory authority;
- approval decisions; and
- execution through supported harnesses.

Psyche may ask Coven to act. It may not substitute a local decision when Coven
rejects a request, lacks a capability, or returns an unknown contract version.

## Problem

OpenCoven has an outbound-only Telegram connector, while a mature Telegram
familiar requires inbound routing, identity continuity, authorization,
conversation state, interactive approvals, media, streaming, durable delivery,
and operational recovery. Keeping that behavior in a harness-specific runtime
would make the familiar depend on one provider and duplicate Coven's authority
boundary.

Psyche provides a focused channel runtime above Coven. A familiar remains
itself when the harness or model changes, and Telegram users receive reliable
responses without giving the channel process direct authority over local
projects or tools.

## Users

| User | Need |
|---|---|
| Principal | Reach a named familiar from Telegram without weakening its identity or approval rules. |
| Group member | Address an authorized familiar in a group or topic and receive correctly threaded replies. |
| Familiar maintainer | Define identity, roles, skills, routes, and channel behavior in reviewable files. |
| Operator | Configure accounts, inspect health, replay failures, rotate secrets, migrate safely, and roll back. |
| Coven maintainer | Integrate one untrusted client through the existing versioned daemon contract. |

## Product principles

1. **Identity precedes conversation.** No update reaches a harness until the
   route's familiar declaration, `IDENTITY.md`, `SOUL.md`, and role/skill
   configuration resolve to one coherent familiar.
2. **Capability intent is not permission.** Identity and route configuration
   describe intent; Coven decides whether the requested session or action is
   allowed.
3. **Durability precedes acknowledgement.** An accepted Telegram update is
   committed locally before a webhook returns success or a polling cursor
   advances.
4. **Wrong-surface success is failure.** Psyche never drops topic metadata,
   changes the account, or falls back to a different chat to make a send pass.
5. **One logical response is one visible response.** Streaming edits one
   persistent preview and finalizes it in place whenever Telegram permits.
6. **Authorization uses stable identifiers.** Telegram numeric user and chat
   IDs are authoritative; mutable usernames are presentation metadata.
7. **Unknown means denied.** Unknown API versions, capabilities, callback
   types, identity states, policies, and route states fail closed.
8. **Clean-room behavior, original implementation.** Publicly observable
   behavior may inform requirements. OpenClaw code, internal organization,
   names, text, tests, fixtures, assets, and private state are not copied.

## Goals

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

- Reimplementing Coven's daemon, policy engine, memory authority, session
  ledger, project boundary checks, or approval authority.
- Running model or harness provider SDKs directly from Psyche.
- Supporting Telegram user accounts through MTProto; Psyche uses the Bot API.
- Cloud multi-tenancy or a hosted OpenCoven control plane.
- Importing OpenClaw databases, source code, credentials, prompts, internal
  configuration, or runtime files.
- Automatic Telegram-triggered edits to protected familiar or operator
  configuration.
- Exposing private chain-of-thought or provider reasoning streams.
- Replacing Coven Cave's desktop or mobile control-room experience.
- Shipping channels other than Telegram in Psyche v1.

## Identity contract

Every enabled route names exactly one `familiar_id` and one Coven project
scope. Before activating the route, Psyche:

1. resolves the familiar declaration from an operator-configured familiar home;
2. opens `IDENTITY.md` and `SOUL.md` without following symlinks;
3. resolves the declaration's roles and skill configuration;
4. verifies that all sources name the same familiar and contain no conflicting
   role, principal, or governance declarations;
5. computes a content digest over the resolved identity inputs;
6. requires Coven to return a canonical effective-familiar binding whose
   per-input and aggregate digests equal Psyche's snapshot; and
7. requires Coven to bind both the familiar ID and aggregate identity digest to
   every session, turn, policy decision, and external action.

Missing files, unreadable files, unsafe paths, unknown familiar IDs, digest
changes during a turn, or contradictions disable the route. Psyche reports a
structured operator error and does not replace the identity with a prompt,
account name, route label, or model-generated persona.

The channel account is presentation, not identity. One Telegram bot may serve
multiple familiars only when every chat/topic route is unambiguous. A familiar
may use multiple bot accounts while preserving one identity digest.

## Authority contract

Psyche is an untrusted Coven client for enforcement purposes.

| Decision | Owner | Psyche behavior |
|---|---|---|
| Which Telegram account received an update | Psyche | Verify token-scoped account and persist it. |
| Which chat/topic route matches | Psyche | Resolve one route or reject as ambiguous. |
| Whether a Telegram sender may trigger the route | Psyche and Coven | Apply channel ACL as a prefilter, then request a per-turn Coven decision bound to actor, surface, familiar, project, and content digest; either denial stops work. |
| Which familiar the route represents | Psyche and Coven | Resolve locally, require canonical Coven ID and effective-identity digest, reject any disagreement. |
| Whether a project path, harness, tool, memory operation, or external action is allowed | Coven | Forward intent and surface the daemon's decision. |
| Whether an approval is valid | Coven | Render an opaque approval reference; never decide locally. |
| How a permitted response appears in Telegram | Psyche | Format and deliver within route and Telegram policy. |

Capability discovery reports whether Coven can evaluate an action class; it is
never authorization. Every turn dispatch, routine reply, pairing approval,
approval decision, and Telegram mutation requires a versioned Coven decision
bound to the actor/requesting session, exact account/chat/topic surface,
familiar and identity digest, Ward revision, project, payload digest, policy
revision, and expiry. Sends to another chat, destructive message actions,
public group replies, and non-reply broadcasts use distinct action classes or
mandatory Coven-derived target relationships that policy can deny separately.
Bounded Telegram protocol administration needed to operate an enabled account
is covered only by a separate account-activation decision tied to the exact bot
ID, transport, API root, and config revision. Psyche rejects a missing, stale,
mismatched, or unknown decision.

## Core journeys

### First account setup

1. The operator stores a bot token in an approved secret provider.
2. Psyche config stores only the secret reference and expected numeric bot ID.
3. `psyche doctor` resolves the reference in memory, calls `getMe`, requires the
   returned bot ID to match, verifies Coven's API/capability profile, validates
   routes and identities, and prints no secret or token-bearing URL.
4. The operator starts `psyched` in polling or webhook mode.
5. Health reports the account, transport, route, identity, and Coven state
   separately.

### Pair an owner in a direct message

1. An unknown numeric sender messages a route whose DM policy is `pairing`.
2. Psyche persists the update and creates a short-lived, one-time pairing
   request without dispatching the message to a familiar.
3. The principal approves the request through a local Coven-authorized surface.
4. Psyche records the approved numeric sender ID for that account and DM scope.
5. A new message starts an identity-bound Coven conversation.

Pairing never grants group access, action approval, or owner privileges by
implication.

### Converse in a group or topic

1. Psyche verifies the account, group, sender, topic, and mention policy.
2. It derives an ordered lane for the account, chat, and topic.
3. It supplies bounded observed context and explicit reply metadata to Coven.
4. Coven launches or resumes the route's familiar-bound conversation.
5. Psyche sends typing or an acknowledgement if policy permits.
6. Output is streamed into one message and finalized in the same topic.

### Request a sensitive action

1. Coven emits an approval-required event with an opaque approval ID, action
   digest, expiry, and allowed approver principals.
2. Psyche renders the request only to configured Telegram approval surfaces.
3. A button press is checked against account, numeric sender, chat, expiry, and
   one-time callback state.
4. Psyche submits the decision to Coven.
5. Coven revalidates and decides. Psyche displays the result without executing
   the action itself.

### Recover from failure

1. On restart, Psyche leases committed but unfinished updates and delivery
   intents.
2. It preserves per-chat/topic ordering and resumes from the last durable
   state.
3. Known retryable failures follow bounded backoff and Telegram `retry_after`.
4. Ambiguous outbound outcomes are reported as such rather than silently
   declared sent.
5. Operators can inspect, retry, dead-letter, or abandon work through explicit
   commands whose actions are audited.

## Functional requirements

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

- A conversation key includes familiar, account, chat, and effective topic.
- Context identifies the current message separately from observed reply,
  forward, quote, and bounded history context.
- Context never claims arbitrary message hydration; only updates previously
  observed by Psyche may be recalled.
- A route identity change closes the current conversation and requires a new
  explicit start after validation.
- Replies always return through the inbound account and conversation surface
  unless a separately authorized send intent names another target.

### Delivery and interaction

- Text uses safe Telegram HTML by default and falls back to plain text on
  entity errors.
- Long text splits at Telegram-safe boundaries and retains reply/topic
  metadata.
- Streaming supports off, partial, block, and progress modes.
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
  webhook/polling conflicts, Coven capabilities, familiar identity, routes,
  storage, and filesystem permissions.
- Health distinguishes `ready`, `degraded`, and `blocked`, with machine-readable
  reason codes.
- Metrics exclude message text, callback values, tokens, raw IDs, and local
  paths by default.
- Every sensitive operator action and approval bridge event carries a
  correlation ID shared with Coven.

## Data handling and retention

Default local retention:

| Data | Default | Notes |
|---|---:|---|
| Raw accepted Telegram update | 7 days | Encrypted at rest when an OS-backed key is available; otherwise startup requires explicit operator acceptance. |
| Normalized message content and observed context | 30 days | Per-route reduction is supported; zero disables historical context after processing. |
| Downloaded media | 24 hours | Deleted after successful turn adoption unless retained by explicit policy. |
| Delivery and deduplication metadata | 30 days | Content-free keys may outlive message content to prevent replay. |
| Security and operator audit metadata | 90 days | Contains hashes and reason codes, not secrets or full message bodies. |
| Bot token | 0 days | Resolved only in memory from a secret reference; never persisted by Psyche. |

Retention cleanup is transactional and auditable. Legal hold and backup are
operator responsibilities and require a separate explicit configuration.

## Service objectives

The v1 release target is:

- zero unauthorized familiar dispatches or approval decisions;
- 100% replay of updates acknowledged after durable commit in crash-injection
  tests;
- at least 99.9% successful webhook durable acknowledgements within 2 seconds,
  excluding Telegram or host outages;
- p95 committed-update-to-Coven-adoption latency below 2 seconds under the
  documented single-host load profile;
- p95 Coven-output-to-first-preview latency below 1 second when Telegram is
  healthy;
- fewer than 1 duplicate visible delivery per 10,000 logical deliveries, with
  every ambiguous outcome counted and exposed; and
- no plaintext secret findings in release artifacts, logs, crash reports, or
  command-line process listings.

Telegram does not provide an idempotency key for sends. Psyche therefore cannot
promise exactly-once visible delivery across every network ambiguity. The
technical contract defines explicit `delivery_unknown` handling instead of
hiding that limitation.

### Normative single-host load profile

`psyche.load.v1` runs on an otherwise idle AWS `c7gd.xlarge` reference runner
(Graviton3/arm64, 4 vCPUs, 8 GiB RAM, local NVMe) using the pinned OpenCoven
Ubuntu 24.04 image, no swap, and a 20 GiB ext4 data-volume quota. It uses
release-mode binaries, one `psyched` process, one Coven authority process, and
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
neither latency objective measures inference. The G4 live canary additionally
reports real Telegram network latency and may not substitute synthetic results
for the required 1,000 updates.

## Release and migration gates

| Gate | Entry evidence | Exit threshold |
|---|---|---|
| G0 - Design | These four documents are complete. | Val records explicit design approval on `coven-psy0`. |
| G1 - Contract | Original Rust schemas, storage migrations, and Coven profile exist. | All schema/contract tests pass; per-effect authorization, identity-digest binding, and idempotent turn adoption pass; unknown and missing capabilities fail closed. |
| G2 - Reliability | Fake Telegram and Coven services plus crash injection exist. | 100% durable-ack, ordering, retry, and restart cases pass in 20 consecutive CI runs. |
| G3 - Live test account | Dedicated non-production bot, DMs, groups, and topics are configured. | Every required parity row marked `L` passes twice on two Telegram client families; zero security-critical findings. |
| G4 - Canary | One operator-owned account runs Psyche without a shared production token. | Seven consecutive days at the service objectives and at least 1,000 updates with zero unauthorized dispatches. |
| G5 - Production cutover | OpenClaw transport is quiesced and a rollback owner is present. | One DM route for 24 hours, then one group/topic route for 72 hours, before wider enablement. |
| G6 - Release | Required parity evidence and migration drill are complete. | Signed binaries/npm wrappers published; rollback drill completes in under 15 minutes. |

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
3. versioned schemas and error semantics are implementation-ready;
4. threat mitigations map to testable controls;
5. release and rollback decisions use measurable thresholds;
6. no placeholder or unstated ownership remains; and
7. Val records explicit design approval before `coven-psy1` starts.
