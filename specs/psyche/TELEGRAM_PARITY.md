# Psyche Telegram Parity Ledger

**Status:** Approved Telegram adapter ledger - W0 reconciled and G1 verified 2026-08-01
**Work unit:** `coven-psy0`
**Reference date:** 2026-07-26
**Canonical decision:** [Familiar runtime design](./RUNTIME_DESIGN.md)
**Companions:** [Decision dossier](./DECISION_DOSSIER.md), [Product specification](./PRODUCT.md), [Technical architecture](./TECH.md), [Threat model](./THREAT_MODEL.md), [Coven prerequisites](./COVEN_PREREQUISITES.md), [Program plan](./PLAN.md)

## Scope and clean-room rule

This ledger is the evidence contract for Psyche's first production adapter. It
does not define Psyche's product, familiar identity, graph, verification, or
execution authority. Every required behavior maps an authenticated Telegram
observation or canonical Psyche surface effect to Bot API semantics.

The inventory was derived from public Telegram Bot API documentation,
public OpenClaw Telegram operator documentation, and OpenCoven's stated product
requirements. It is not a map of OpenClaw source code.

The ledger classifies every behavior in that reference scope:

- **Required** - must ship before Psyche claims Telegram v1 completion.
- **Deferred** - intentionally excluded from v1 with a stated follow-up
  boundary.
- **Rejected** - incompatible with Psyche's authority, identity, safety, or
  clean-room model.

No unlisted Bot API method or non-Telegram production adapter is implied. A newly discovered reference behavior
must be added and classified through design review before implementation.

## Evidence codes

| Code | Evidence |
|---|---|
| U | Unit, property, schema, or golden test |
| I | Fake Telegram/Coven integration test |
| C | Crash-window or restart-replay test |
| S | Security, fuzz, or boundary test |
| L | Live Telegram proof on a dedicated bot |
| O | Operator migration, rollback, or recovery drill |

Required rows must collect every listed evidence code. Deferred and rejected
rows require no implementation proof, but their boundary is tested where a
permissive fallback would create risk.

## Core-contract and gate mapping

| Adapter evidence | Core contract | Minimum gate |
|---|---|---|
| Authenticated update normalization and actor/locator preservation | `psyche.surface_event.v1` plus `psyche.telegram_event.v1` | G8 |
| Principal mapping, familiar routing, and durable acceptance | `psyche.identity_snapshot.v1`, `psyche.intent.v1`, `psyche.graph.v1`, `psyche.graph_node.v1` | G2; real execution additionally G4 |
| Coven-backed turn execution and artifact association | `psyche.execution_binding.v1` and W1-classified protected-resource contracts | G4 |
| Canonical reply/action authorization | `psyche.surface_effect.v1` plus `psyche.telegram_effect.v1` | G8 |
| Logical/physical send durability and ambiguity | `psyche.delivery.v1`, `psyche.recovery.v1` | G8 |
| Live Bot API behavior | Required rows marked `L` | G9 |
| Doctor, retention, export/restore, migration, token rotation, rollback, release security review | Psyche operations contracts | G10 |
| Production observation window and service objectives | Approved canary report | G11 |

G6 controls production child-session dispatch and is not implied by any
Telegram parity row. A single-node Telegram vertical slice may pass G8-G11
without enabling production multi-agent execution.

## Account, configuration, and transport

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| A-01 | Bot token resolved from a secret reference | Required | I,S,L | Raw tokens are absent from config, argv, logs, and packages. |
| A-02 | Startup/reload `getMe` identity pin validation | Required | I,S,L | Invalid credentials or a valid token for another numeric bot ID block activation. |
| A-03 | Non-secret pinned bot identity cache with token-change invalidation | Required | U,I,S | Reduces probes without trusting stale identity; cross-bot rebind archives token-scoped state. |
| A-04 | Multiple bot accounts | Required | U,I,L | State, cursors, ACLs, limits, and routes are account-isolated. |
| A-05 | Explicit default account | Required | U,I | Multiple accounts without one default are invalid. |
| A-06 | Long polling | Required | I,C,L | Default transport. |
| A-07 | Authenticated webhook | Required | I,C,S,L | Secret validation and durable commit precede 2xx. |
| A-08 | Polling/webhook mutual exclusion | Required | U,I,O | One token has one active transport owner. |
| A-09 | Loopback webhook default and explicit public bind | Required | U,I,S,L | Public ingress is never implicit. |
| A-10 | Self-hosted/custom Bot API root | Required | U,I,S,L | HTTPS or explicit loopback only; token-bearing roots rejected. |
| A-11 | HTTP/SOCKS proxy and deterministic DNS controls | Required | U,I,L | Credentials are redacted; operator can recover from egress faults. |
| A-12 | Arbitrary private-network media bypass | Rejected | S | Weakens SSRF controls; trusted explicit origins require a later reviewed schema. |
| A-13 | Raw token in environment or plaintext file | Rejected | U,S | Psyche v1 accepts secret references only. |
| A-14 | Webhook self-signed certificate support | Required | I,L | Supports direct-IP/operator-managed webhook deployments. |
| A-15 | Independent webhook secret reference and atomic rotation | Required | U,I,S,L | New secret promotes only after `setWebhook`; old value has a five-minute in-flight grace. |

## Durability, ordering, and rate limits

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| T-01 | Webhook durable-before-ack | Required | I,C | Non-2xx on storage failure. |
| T-02 | Polling durable-before-offset | Required | I,C | Offset advances only after commit. |
| T-03 | `(account_id, update_id)` deduplication | Required | U,I,C | Duplicate updates create no duplicate turn. |
| T-04 | Per-account/chat/topic sequential lanes | Required | U,I,C | Preserves conversation order while allowing cross-lane concurrency. |
| T-05 | Lease heartbeat and stalled-worker recovery | Required | I,C | Restart does not create two lane owners. |
| T-06 | Complete adapter ingress at durable Psyche intent/graph adoption | Required | I,C | Surface event, principal mapping, intent, graph, and initial node commit before the adapter lane advances; Coven adoption is a later graph-execution boundary. |
| T-07 | Bounded retry by attempt count and age | Required | U,I,C | Transient failures are not silently completed. |
| T-08 | Telegram `retry_after` handling | Required | U,I,L | Token-scoped delay is honored. |
| T-09 | Shared token-scoped outbound limiter | Required | U,I,L | All account clients share one budget. |
| T-10 | 401/404 account-auth failure blocks startup | Required | I,L | Bad credentials are not mislabeled as cleanup failure. |
| T-11 | 409 polling conflict blocks account | Required | I,O | Parent runtime owns recovery; no duplicate poller. |
| T-12 | Safe transient retry classification | Required | U,I,C | Read-only and proven pre-write failures retry with jitter; post-write mutation 5xx/reset/timeout becomes unknown. |
| T-13 | Defensive non-JSON error parsing | Required | U,I | HTML/plain proxy errors cannot crash classification. |
| T-14 | Polling liveness watchdog | Required | I,C,O | Stale long polls rebuild transport after a configured threshold. |
| T-15 | Dead-letter with stable reason and operator action | Required | U,I,O | Work is never silently discarded. |
| T-16 | Explicit non-idempotent `delivery_unknown` and audited resolution | Required | U,I,C,S,O | Accepted-but-response-lost is not claimed sent or blindly retried; recovery binds duplicate risk to a Psyche surface decision and new effect. |
| T-17 | Per-entry hot-path persistence | Required | I,C | Processing does not rewrite whole caches on each update/send. |
| T-18 | Owned HTTP transports close on all exits | Required | U,I,S | Covers startup failure, shutdown, and account reload. |
| T-19 | Transport health and stale-activity probes | Required | I,L,O | Doctor distinguishes polling, webhook, DNS, auth, and storage faults. |
| T-20 | Per-effect Psyche surface-policy decision | Required | U,I,S,C | Adapter capability presence never authorizes replies, pairing, approvals, or Telegram mutations; surface permission does not grant Coven execution. |
| T-21 | Typed effect digest and persisted decision binding | Required | U,I,S,C | Psyche evaluates actual fields; restart/retry retains immutable effect, decision, policy revision, graph provenance, and expiry. |
| T-22 | One decision per familiar- or user-visible Bot API request | Required | U,I,S,C,L | Chunks, preview edits/deletes, formatting/quote fallbacks, and every retry after `sending` each use a new immutable effect decision; bounded account-activation protocol operations are the explicit exception. |

## Access control, activation, and routing

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| R-01 | DM pairing policy | Required | U,I,S,L | Pairing requests are DM-only and expire. |
| R-02 | DM numeric allowlist policy | Required | U,I,L | Stable user IDs are authoritative. |
| R-03 | Explicit-open DM policy | Required | U,I,S,L | Requires an explicit wildcard. |
| R-04 | Disabled DM policy | Required | U,I,L | No DM dispatch. |
| R-05 | Pairing remains DM-scoped | Required | U,I,S,L | Does not grant group or approver authority. |
| R-06 | Group chat allowlist | Required | U,I,L | Group IDs and sender IDs are separate typed values. |
| R-07 | Group sender allowlist/open/disabled | Required | U,I,S,L | Fail-closed allowlist is default. |
| R-08 | Native bot-handle mention activation | Required | U,I,L | Bot username and familiar persona name remain distinct. |
| R-09 | Configured familiar mention patterns | Required | U,I,L | Patterns activate only after group/sender authorization. |
| R-10 | Session activation toggle | Required | U,I,L | Temporary `mention`/`always` state cannot weaken configured ACLs. |
| R-11 | Telegram privacy-mode diagnostics | Required | I,L,O | Doctor explains missing ambient group updates. |
| R-12 | Bounded rolling group history | Required | U,I,L | Authorized observed context remains available without destructive clears. |
| R-13 | Bounded DM history | Required | U,I,L | Per-route limits and zero-history mode. |
| R-14 | Forum-topic conversation isolation | Required | U,I,L | Topic ID participates in route, lane, and conversation keys. |
| R-15 | Topics-enabled DM isolation | Required | U,I,L | DM topic kind is distinct and enabled only from bot capability. |
| R-16 | General forum topic send/typing special case | Required | U,I,L | Message and typing methods use Telegram's required thread semantics. |
| R-17 | Topic wildcard defaults and exact overrides | Required | U,I,L | Exact topic route wins; ambiguity blocks. |
| R-18 | Per-topic familiar routing | Required | U,I,S,L | Each topic resolves one Psyche familiar snapshot and principal mapping; execution remains separately G4-gated. |
| R-19 | Deterministic reply-to-inbound channel | Required | U,I,L | The model does not choose the reply account or surface. |
| R-20 | Authorized bot senders | Deferred | - | v1 ignores other bots to prevent loops; typed bot-to-bot policy needs separate review. |
| R-21 | Anonymous admin/sender-chat authorization | Deferred | S | v1 fails closed when no authorized human numeric ID exists. |
| R-22 | Telegram channel-post conversations | Deferred | - | v1 product journeys cover DMs, groups, supergroups, and topics. |
| R-23 | OpenClaw ACP topic bindings and chat spawn commands | Rejected | U | Psyche uses its own surface-neutral conversations and W1-classified Coven session bindings, not OpenClaw ACP concepts. |

## Commands, callbacks, and approvals

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| C-01 | Native command menu registration | Required | U,I,L | Startup registers allowed commands. |
| C-02 | Custom command menu validation | Required | U,I,L | Normalize names; reject duplicates, conflicts, invalid length/characters. |
| C-03 | Command-menu overflow handling | Required | I,L | Bounded trim/report; no startup crash loop. |
| C-04 | Fast-path commands before agent startup | Required | U,I,L | Status, identity, pairing, and safe diagnostics need no model turn. |
| C-05 | Typed callbacks | Required | U,I,S,L | Approval, command, selection, and plugin-like callbacks never become raw prompt text. |
| C-06 | Exact callback-value preservation | Required | U,I,L | Delimiters and UTF-8 values survive typed handling. |
| C-07 | Unknown callback acknowledgement and refusal | Required | U,I,S,L | No raw fallback to the familiar. |
| C-08 | Inline keyboards with surface scope | Required | U,I,S,L | Off, DM, group, all, and allowlist scopes. |
| C-09 | URL and Telegram Web App buttons | Required | U,I,S,L | Web Apps are limited to Telegram-supported private surfaces. |
| C-10 | Poll creation and results | Required | U,I,L | 1-12 options under Bot API 10.2, duration, anonymity, public results, topic targeting. |
| C-11 | Authority-domain approval prompts and decisions | Required | U,I,S,L | Opaque, expiring, one-time callbacks; Psyche or Coven decides only within the labeled owning domain. |
| C-12 | Approval delivery to DM/channel/both | Required | U,I,S,L | Group display requires explicit policy and preserves topic. |
| C-13 | Telegram-based mobile device pairing | Deferred | - | Cave/mobile pairing is a separate product contract. |
| C-14 | Dashboard Mini App | Deferred | - | Owned by Coven Cave; Psyche may later render an authorized deep link. |
| C-15 | Inline-query bot mode | Deferred | - | No v1 user journey or safe route/context contract. |

## Replies, formatting, streaming, and message actions

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| D-01 | Plain text send | Required | U,I,L | Basic delivery. |
| D-02 | Safe Telegram HTML entities | Required | U,I,S,L | Bold, italic, links, code, spoilers, and quotes. |
| D-03 | Bot API rich-only blocks | Deferred | - | Client support is inconsistent; original HTML renderer is the v1 contract. |
| D-04 | Link-preview policy | Required | U,I,L | Route/account configurable. |
| D-05 | Unicode-safe long-text chunking | Required | U,I,L | Defaults below 4096 and preserves entity/topic/reply metadata. |
| D-06 | Reply to current or explicit observed message | Required | U,I,L | Explicit typed metadata, not model-selected target strings. |
| D-07 | Native selected quote reply | Required | U,I,L | Enforce Telegram quote limit and plain-reply fallback. |
| D-08 | Observed reply/quote/forward context | Required | U,I,L | Only messages Psyche observed; current context outranks ancestry. |
| D-09 | Arbitrary historical `getMessage` hydration | Rejected | U | Telegram Bot API does not provide it; Psyche must not imply otherwise. |
| D-10 | Edit text, caption, and reply markup | Required | U,I,L | Individually policy-gated. |
| D-11 | Delete message | Required | U,I,S,L | Destructive action requires Psyche surface policy. |
| D-12 | Add/remove reaction | Required | U,I,L | Action-gated with Telegram semantics. |
| D-13 | Inbound reaction notifications | Required | U,I,L | Authorized and routed with documented no-topic fallback. |
| D-14 | Configurable acknowledgement reaction | Required | U,I,L | Identity emoji fallback; route scope controls use. |
| D-15 | Typing/chat actions | Required | U,I,L | Correct account/topic and bounded retry. |
| D-16 | Streaming off/partial/block/progress modes | Required | U,I,C,L | Original state machine, one logical answer. |
| D-17 | One persistent preview finalized in place | Required | U,I,C,L | No extra final bubble unless final content is unconfirmed. |
| D-18 | First-preview debounce and cumulative edits | Required | U,I,L | Handles token-sized deltas and rate limits. |
| D-19 | Safe tool-progress labels | Required | U,I,L | No unapproved command detail. |
| D-20 | Raw chain-of-thought/reasoning stream | Rejected | U,S | Psyche never exposes private model reasoning. |
| D-21 | Formatting/caption/quote fallback parity | Required | U,I,L | Durable and streaming funnels use the same predicates. |
| D-22 | Error reply policy `always`/`once`/`silent` | Required | U,I,L | Inherits account/group/topic policy. |
| D-23 | Policy-controlled visible group replies | Required | U,I,S,L | Ambient or public replies require explicit route and Psyche surface policy. |
| D-24 | Pin requested delivery | Required | U,I,S,L | Requires chat permission and Psyche surface policy. |
| D-25 | Bounded streaming preview lifetime | Required | U,I,C,L | Default maximum age is 10 minutes; expiry uses a newly authorized final edit, and ambiguous or denied finalization never sends an unauthorized duplicate. |

## Media and Telegram-native content

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| M-01 | Inbound/outbound photos | Required | U,I,S,L | Safe download, description as untrusted derived text. |
| M-02 | Media albums/groups | Required | U,I,C,L | Bounded correlation and ordered follow-up for late items. |
| M-03 | Documents/files | Required | U,I,S,L | No filename trust or archive extraction. |
| M-04 | Audio files | Required | U,I,S,L | Preserve audio semantics. |
| M-05 | Voice notes and optional transcription | Required | U,I,S,L | Derived transcript is labeled untrusted. |
| M-06 | Video files | Required | U,I,S,L | Caption support and safe limits. |
| M-07 | Video notes | Required | U,I,S,L | Text is sent separately because Telegram has no video-note caption. |
| M-08 | Captions | Required | U,I,L | Telegram-safe length and plain fallback. |
| M-09 | Static stickers | Required | U,I,S,L | Metadata, safe file handling, and optional derived description. |
| M-10 | Animated/video stickers as preserved files | Required | U,I,S,L | Transport and metadata parity; semantic interpretation is not required. |
| M-11 | Sticker send by Telegram file ID | Required | U,I,L | Separately action-gated. |
| M-12 | Cached sticker search | Required | U,I,L | Search only Psyche-observed, policy-retained metadata. |
| M-13 | Native locations and venues | Required | U,I,L | Standalone typed payload; cannot combine with text/media. |
| M-14 | Configurable media byte limit | Required | U,I,S,L | Applies inbound and outbound. |
| M-15 | Force-document delivery | Required | U,I,L | Avoids Telegram image/GIF/video transformation when requested. |
| M-16 | Arbitrary URL auto-fetch from message text | Rejected | U,S | Prevents SSRF; media must be an explicit authorized artifact. |
| M-17 | Two-phase execution-artifact ingestion | Required | U,I,S,C | W1-classified/G4-proven protected-resource admission precedes upload; opaque IDs bind graph/node/attempt without crossing a project path. |
| M-18 | Execution output-artifact streaming | Required | U,I,S,C,L | Outbound bytes stream through a W1-classified/G4-proven opaque attempt artifact contract, then require separate Psyche media-send authorization. |

## Telegram actions and service events

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| X-01 | Create forum topic | Required | U,I,S,L | Psyche surface policy required. |
| X-02 | Edit/close/reopen forum topic | Required | U,I,S,L | Destructive mutations are separately gated. |
| X-03 | Group-to-supergroup migration event | Required | U,I,L,O | Update route health and emit an operator proposal; do not rewrite config or bridge old conversation history automatically. |
| X-04 | Telegram-triggered persistent config writes | Rejected | U,S | Config changes use local audited operator surfaces. |
| X-05 | Service/member events as normalized context | Required | U,I,L | Persisted as typed events; no turn by default. |
| X-06 | Read receipts | Rejected | U | Telegram Bot API has no bot read-receipt support. |
| X-07 | Human-account MTProto mode | Rejected | U,S | Bot API only. |

## Operations, migration, and release

| ID | Capability | Class | Evidence | Decision |
|---|---|---|---|---|
| O-01 | CLI send target by numeric chat/topic | Required | U,I,L | Usernames may be resolved for outbound convenience but never authorize. |
| O-02 | CLI polls, buttons, pins, and force-document | Required | U,I,S,L | Same policy path as agent-originated actions. |
| O-03 | Account/transport/route/familiar health | Required | I,L,O | Ready, degraded, and blocked states with stable reasons. |
| O-04 | Polling, webhook, DNS, proxy, auth, and privacy diagnostics | Required | U,I,L,O | `psyche doctor` provides actionable redacted output from the versioned `psyche.doctor_report.v1` schema. |
| O-05 | Secret-free logs, metrics, and crash reports | Required | U,I,S,O | Automated scans gate release. |
| O-06 | Retention and transactional cleanup | Required | U,I,C,S | Raw, normalized, media, dedupe, and audit classes have explicit lifetimes. |
| O-07 | Live E2E proof for sensitive Telegram changes | Required | L | Transport, streaming, topics, callbacks, authorization, media, and reply context. |
| O-08 | Crash/restart proof for reliability changes | Required | C | Durable ack, offset, lane, retry, and delivery state. |
| O-09 | Dedicated-token canary | Required | L,O | G11 starts only after the minimum export passes a clean restore drill; observation window and volume are operator-approved before the gate. |
| O-10 | Single-owner migration cutover | Required | O | Previous runtime is quiesced; no dual poller/webhook. |
| O-11 | Rollback in under 15 minutes | Required | O | Preserves uncertain delivery and audit state. |
| O-12 | Import reviewable operator-authored configuration | Required | U,S,O | Human-readable, secret-free original schema may include prompts, declarations, hooks, commands, numeric ACLs, routes, and mappings. |
| O-13 | Import OpenClaw database/runtime state | Rejected | U,S | Violates clean-room and creates unsafe hidden coupling. |
| O-14 | Signed/checksummed npm native distribution | Required | I,S,O | Canonical package is `@opencoven/psyche`. |
| O-15 | Bot API upgrade compatibility gate | Required | U,I,L | Unknown update kinds are classified; new behavior needs review and proof. |
| O-16 | Psyche-minted local operator context | Required | U,I,S,O | CLI sends and local pairing decisions cannot self-assert principal authority; the context grants no Coven permission. |
| O-17 | Audited familiar identity rebind | Required | I,C,S,O | Changed identity inputs block routes; Psyche owns the rebind, while any old execution is terminal or fenced through a W1-classified Coven contract before reactivation. |
| O-18 | Real-Coven execution-loss conformance gate | Required | I,C,S | G4 proves capability-present-but-denied and mid-flight termination/stall has no Psyche-local execution fallback. |
| O-19 | Inconclusive Coven adoption recovery | Required | U,I,C,S,O | Inspect/reconcile is read-only unless Coven returns an authoritative disposition; possible adoption requires a W1-classified fence and explicit acknowledgement. |
| O-20 | Ambiguous Telegram delivery recovery | Required | U,I,C,S,O | Inspect plus abandon/retry/clarify uses a typed Psyche surface decision; retry and clarification require duplicate-risk acknowledgement. |
| O-21 | Minimum export and clean restore | Required | U,I,C,S,O | Checksummed encrypted `psyche.export.v1` preserves retained unresolved operational state and excludes secrets. |

## Completion rule

Psyche may claim comprehensive Telegram v1 support only when:

1. every **Required** row has all listed evidence attached to its implementation
   work unit;
2. every **Deferred** and **Rejected** boundary has no permissive fallback;
3. G1 specification coherence and all applicable G2-G10 gates pass;
4. no adapter row grants familiar identity, graph authority, Coven execution,
   or protected-resource authority by implication;
5. threat-model security acceptance passes;
6. migration and rollback drills pass with one token owner;
7. G11 canary meets the operator-approved window and volume; and
8. Val records explicit parity approval before production cutover.
