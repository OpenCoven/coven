# Psyche Threat Model

**Status:** Approved security baseline - W0 reconciled and G1 verified 2026-08-01
**Work unit:** `coven-psy0`
**Canonical decision:** [Familiar runtime design](./RUNTIME_DESIGN.md)
**Companions:** [Decision dossier](./DECISION_DOSSIER.md), [Product specification](./PRODUCT.md), [Technical architecture](./TECH.md), [Telegram parity ledger](./TELEGRAM_PARITY.md), [Coven prerequisites](./COVEN_PREREQUISITES.md), [Program plan](./PLAN.md)

## Scope

This threat model covers `psyched`, the `psyche` CLI, familiar identity and
principal mapping, intent and graph state, delegation and budgets, execution
bindings, evidence and verification, add-on workers, surface adapters,
Psyche's local database and media cache, Telegram Bot API traffic, webhook
ingress, secret resolution, the same-user Coven socket, npm/native
distribution, and migration from another runtime.

Coven's daemon, harnesses, model providers, Telegram's service, the operating
system, and secret providers are dependencies with their own threat models.
Psyche must treat their inputs as untrusted at its boundary even when it relies
on their documented guarantees.

## Security objectives

1. No prompt, surface, model, harness, add-on, or Coven response may redefine
   familiar identity.
2. Every accepted intent must bind one mapped principal, familiar snapshot,
   project, constraints, and evidence policy before graph admission.
3. Delegation must not widen authority, budget, evidence access, or surface
   scope.
4. Psyche orchestration and surface policy must not grant Coven execution or
   protected-resource permission; Coven admission must not grant surface
   effects.
5. Accepted intents and surface updates must not be lost after acknowledgement.
6. Replayed or duplicated intents, updates, and callbacks must not repeat work
   silently.
7. Unknown adoption, cancellation, verification, or delivery must not be
   inferred safe or successful.
8. A generator must not certify its own result; verdicts must bind sealed
   evidence and declared reviewer provenance.
9. A Telegram reply or action must not escape its authorized account,
   chat/topic, and action class.
10. Bot tokens and sensitive local data must not appear in config, argv, logs,
   crash reports, databases, packages, or diagnostics.
11. Message and media content must remain untrusted data rather than becoming
   identity, policy, configuration, or shell input.
12. Security-relevant failures must be visible, attributable, and fail closed.
13. The implementation and distribution must remain clean-room and auditable.

## Trust assumptions

- The local operating-system account and kernel are trusted. A malicious
  process running as the same user can read process memory or impersonate a
  local client; Psyche reduces exposure but does not claim to defeat a fully
  compromised same-user account.
- Psyche is authoritative for identity resolution, principal mapping, graph,
  verification, add-on, and surface state. Coven is authoritative only for
  admitted execution and protected resources exposed by versioned contracts.
  Neither boundary trusts the other to replace its enforcement.
- The configured secret provider returns the intended token to the local
  process. Psyche verifies the token's Telegram bot identity before use.
- Telegram authenticates Bot API HTTPS endpoints. Webhook requests are not
  trusted until Psyche verifies the configured secret header.
- Familiar files and route config may be modified by local software and must be
  revalidated with safe filesystem operations.
- Harness output, model output, transcripts, media-derived text, filenames,
  usernames, callback values, and Telegram message content are untrusted.

## Assets

| Asset | Security property |
|---|---|
| Bot token and secret-provider output | Confidentiality, scoped use, rotation. |
| Familiar declaration, `IDENTITY.md`, `SOUL.md`, role/skill config | Integrity, provenance, coherent binding. |
| Principal mappings, intents, graphs, nodes, attempts, delegations, budgets | Integrity, provenance, non-widening transitions, durability. |
| Evidence sets, artifact references, verdicts | Immutability, correct producer/reviewer binding, retention. |
| Project roots, Coven sessions, protected resources, execution approvals | Coven-controlled admission and integrity. |
| Psyche orchestration and surface approvals | Psyche-controlled authorization, provenance, expiry, domain isolation. |
| Numeric ACLs, routes, account mapping | Integrity and fail-closed interpretation. |
| Telegram messages, media, observed context | Confidentiality, bounded retention, correct attribution. |
| Ingress queue, offsets, lane state, delivery ledger | Integrity, durability, ordering, replay resistance. |
| Approval IDs and callback nonces | Integrity, expiry, actor/surface binding, one-time use. |
| Logs, metrics, audit records, crash reports | Useful attribution without sensitive payload leakage. |
| Release binaries and npm wrappers | Authenticity, integrity, reproducibility, provenance. |

## Trust boundaries

```mermaid
flowchart TD
  Internet[Telegram / Internet] -->|HTTPS or webhook| Edge[Surface adapter boundary]
  Edge -->|authenticated observation| Runtime[Psyche identity, intent, graph, and surface policy]
  Files[Operator config and identity files] -->|no-follow validated reads| Runtime
  Store[(Psyche private state)] <--> Runtime
  Runtime <--> Verify[Evidence and verification boundary]
  Runtime <--> Addons[Trusted same-user add-on workers]
  Secrets[Secret provider] -->|bounded in-memory token| Edge
  Runtime -->|versioned execution request| Coven{{Coven execution boundary}}
  Coven -->|ordered events and terminal state| Runtime
  Coven --> Harness[Harness/provider/tool boundary]
  Runtime -->|surface-policy-gated effect| Edge
```

The adapter boundary authenticates protocol observations. Psyche maps actors to
principals and independently admits intent, graph changes, verification, and
surface effects. Coven independently admits execution and protected resources.
Passing one boundary never implies passing another.

## Adversaries

- an unauthenticated Internet client reaching a webhook listener;
- an unauthorized Telegram user, group member, anonymous admin, or bot;
- an authorized user sending malicious prompt, media, markup, or callback data;
- a group member attempting to use DM pairing in a group;
- a compromised or malicious Telegram group;
- a local process modifying config, identity files, the socket, database, or
  secret references;
- a malicious model or harness output attempting to redirect delivery or forge
  approvals;
- a network attacker causing resets, retries, stale DNS, or proxy redirection;
- a dependency or package supply-chain attacker; and
- an operator mistake during token rotation, migration, or rollback.

## Threats and required controls

| ID | Threat | Required control | Verification |
|---|---|---|---|
| P-01 | Forged webhook update | Require exactly one secret header, constant-time compare before JSON parsing, bounded body/read time, non-2xx on failure. | Integration tests for missing, duplicate, malformed, and timing-varied headers; live webhook probe. |
| P-02 | Webhook exposed unintentionally | Bind loopback by default; public bind requires explicit config and startup warning; never trust proxy identity headers as Telegram auth. | Config tests and socket bind inspection. |
| P-03 | Polling and webhook both consume one token | One transport mode per account plus process/database ownership lease; Telegram 409 blocks the account. | Integration conflict tests and operator drill. |
| P-04 | Update acknowledged before durable storage | Commit raw update, normalized event, dedupe key, and cursor state before webhook 2xx or next polling offset. | Crash injection at every acknowledgement boundary. |
| P-05 | Replay repeats a turn | Unique `(account_id, update_id)` key and idempotent durable acceptance; one event record per update. | Duplicate and restart integration tests. |
| P-06 | Out-of-order turns corrupt context | Account/chat/topic lane leases, monotonic per-lane sequence, no concurrent lane owner. | Concurrency property tests and crash replay. |
| P-07 | Unauthorized DM reaches a familiar | Fail-closed DM policy, numeric IDs, one-time DM pairing, explicit wildcard for open mode. | Authorization matrix tests and live unapproved sender probe. |
| P-08 | DM pairing grants group or owner authority | Store pairing scope as account + numeric sender + DM only; group ACL and approver policy remain independent. | Cross-surface authorization tests. |
| P-09 | Mutable username bypasses an ACL | Usernames are display metadata only; ACL keys are decimal numeric IDs. | Config rejection and rename tests. |
| P-10 | Group ID confused with sender ID | Separate typed fields and sign semantics; negative chat IDs cannot parse as allowed user IDs. | Parser/property tests. |
| P-11 | Ambient group traffic triggers work | Require allowed group, allowed sender, and mention/activation policy before dispatch; unauthorized content never enters history. | Group matrix and privacy-mode live tests. |
| P-12 | Anonymous admin or sender-chat attribution bypass | Fail closed when a policy requires a human numeric ID and none is present; add explicit typed policy only in a later reviewed schema. | Anonymous sender fixtures. |
| P-13 | Route ambiguity selects a weaker familiar or project | Deterministic precedence; equal-precedence matches block the event and route set. | Route ambiguity property tests. |
| P-14 | Prompt, route, surface, model, add-on, or Coven response overrides familiar identity | Psyche resolves one immutable declaration/`IDENTITY.md`/`SOUL.md`/role/skill snapshot with provenance; prompts and external responses cannot supply identity; Coven may validate only the exact execution snapshot. | Contradiction matrix, provenance, identity-source confusion, and execution-binding mismatch tests. |
| P-15 | Identity file swapped through symlink or race | Canonical approved root, directory-relative no-follow opens, regular-file validation, digest recheck before each turn. | Symlink, replacement, ownership, and race tests. |
| P-16 | One authority domain bypasses another | Treat capabilities as discovery only; Psyche separately enforces intent/graph/surface policy while Coven independently enforces execution/protected-resource contracts; neither decision is accepted in another domain. | Cross-domain approval/effect substitution, missing/unknown/deny, and binding-mismatch tests. |
| P-17 | Malicious message becomes system instruction | Use typed context sections; label channel and derived text untrusted; never concatenate message content into identity or permission fields. | Prompt-construction snapshots and injection tests. |
| P-18 | Model output redirects a reply | Reply surface is pinned from the admitted intent; model text cannot set account/chat/topic. Cross-chat effects require a separate Psyche surface decision. | Delivery binding tests. |
| P-19 | Model output forges a callback or approval | Callbacks use opaque stored nonces; approval IDs/action digests come only from the recorded owning authority; rendered text has no authority. | Forged callback, authority-domain confusion, and fake-output tests. |
| P-20 | Approval replay or approval by wrong actor/domain | Bind nonce to authority domain, account, mapped principal, numeric actor, chat/topic, message, approval ID, action digest, decision set, and expiry; consume once; the owning authority revalidates. | Replay, cross-domain, cross-user, cross-chat, expiry, and mutation tests. |
| P-21 | Sensitive command text leaks in a group approval | Default approval target is authorized DM; group/topic display requires explicit Psyche surface policy and warns that command text is visible. | Policy tests and redaction snapshots. |
| P-22 | Bot token leaks through config or process metadata | Accept secret references only; argv-based provider invocation; bounded pipe; redact token patterns and `/bot<TOKEN>` URLs; zeroize buffers where practical. | Process-list, log, crash, package, and secret-scan tests. |
| P-23 | Secret reference points to attacker-controlled helper | Secret provider executables are operator-configured absolute paths or trusted built-ins; no shell lookup or interpolation. | Config and execution tests. |
| P-24 | Custom Bot API root exfiltrates a token | HTTPS required except explicit loopback self-hosted mode; host allowlist; construct token path internally; never accept token-bearing `api_root`. | SSRF and config tests. |
| P-25 | Telegram media path becomes SSRF or crosses graph/execution identity | Fetch only paths returned by the configured Telegram API for the same account; pin origin and reject origin-changing redirects; deny private destinations except the exact configured loopback API origin; digest bytes/source/project/familiar-snapshot/graph/node/attempt metadata and require a W1-classified protected-resource binding before execution. | DNS rebinding, redirect, private-address/loopback exception, two-phase admission, and cross-binding artifact tests. |
| P-26 | Filename traversal or unsafe local file | Ignore inbound path components; generate private names; no-follow temp directory; never extract archives. | Traversal, symlink, archive, and platform path tests. |
| P-27 | Media exhausts disk, memory, CPU, or decompressor | Stream with byte/time limits; content sniff; decompression and dimension budgets; per-account quotas; cleanup on every exit path. | Oversize, slow stream, decompression bomb, and quota tests. |
| P-28 | HTML/markup injection changes message meaning | Original allowlisted renderer; escape all untrusted text; Telegram entity validation; plain-text fallback. | Fuzzing and golden rendering tests. |
| P-29 | Callback value is parsed as command text | Typed callback registry; exact opaque values; unknown callbacks are rejected or acknowledged as unsupported, never injected as a user command. | Callback parser and delimiter tests. |
| P-30 | Non-idempotent send repeats after network or server ambiguity | Persist effect before send; only pre-write failures and read-only operations retry automatically; post-write 5xx/reset/timeout becomes `delivery_unknown`; resolution requires a typed Psyche surface decision and separately authorized recovery effect with duplicate-risk acknowledgement. | TCP reset, proxy/server 5xx, timeout, and unknown-resolution fault injection. |
| P-31 | Flood limit causes reordering or outage | One limiter per token; honor `retry_after`; hold lane order; bounded queue/admission control. | Rate-limit and overload tests. |
| P-32 | Poison update blocks polling forever | Durably classify unsupported/non-dispatchable updates and advance only after that classification commits. | Malformed/unknown update sequence tests. |
| P-33 | Local database tampering creates authorized work | Private permissions, schema constraints, hashes, typed states, callback entropy, startup integrity checks; treat same-user compromise as residual risk. | Permission and tamper tests. |
| P-34 | Logs or metrics leak content/IDs | Structured allowlist logging, hashed identifiers, no payloads by default, privileged diagnostic mode with explicit warning and expiry. | Snapshot tests and automated secret/PII scanning. |
| P-35 | Retained content exceeds policy | Transactional expiry, startup cleanup, bounded backups, auditable hold configuration. | Time-travel retention tests. |
| P-36 | Operator accidentally runs two runtimes during migration | No shared-token shadow mode; explicit quiesce and ownership checks; 409 blocks; named rollback operator. | Migration rehearsal under dedicated tokens. |
| P-37 | Migration imports compromised private state | Import only human-reviewed prompts, declarations, hooks, commands, config, aliases, ACLs, and mappings; never read OpenClaw code, credentials, databases, conversations, hidden memory, caches, or runtime state. | Import schema rejection tests and review checklist. |
| P-38 | Malicious npm/native package replaces binary | Signed release provenance, checksummed platform artifact, wrapper verifies expected version/hash, reproducible build evidence. | Release verification in CI and clean-host install. |
| P-39 | Dependency compromise adds telemetry or exfiltration | Minimal dependencies, lockfile review, license/security audit, no analytics, egress limited to Telegram, Coven socket, and configured secret provider. | Dependency audit and egress integration tests. |
| P-40 | Unknown schema/API is interpreted permissively | Exact major-version match, unknown enum rejection, quarantine persisted unknown events, missing capabilities denied. | Forward-version contract tests. |
| P-41 | Lost Coven response repeats execution | Require W1-classified stable request adoption and lookup; reuse with another digest conflicts; inconclusive lookup blocks the graph node as `coven_adoption_unknown`; recovery requires authoritative reconciliation or fence, never a local unblock. | Launch/input lost-response, operator-recovery, fence, dependency-block, and restart crash tests. |
| P-42 | Secret rotation silently switches Telegram bots | Pin expected numeric bot ID; compare every `getMe`; different bot requires a new account identity or audited destructive rebind. | Valid-wrong-token startup/reload and rebind tests. |
| P-43 | Client supplies a harmless digest but performs another surface effect | Persist strict `psyche.surface_effect.v1` plus adapter effect; Psyche recomputes canonical digests and rejects class/effect or digest disagreement before adapter invocation. | Effect mutation, unknown-field, and digest-confusion tests. |
| P-44 | Restart or local transformation escapes the surface decision | Persist immutable effect JSON, digests, decision ID, policy revision, and expiry; append renewals; require a distinct decision for every physical chunk, preview edit, cleanup, or fallback. | Restart, chunk/fallback mutation, and expired-decision retry tests. |
| P-45 | Local CLI self-asserts principal authority | Local operator actions carry only a Psyche-minted short-lived context from configured local authentication; Psyche derives/revalidates the principal; the context grants no Coven authority. | Forged, expired, cross-user, and cross-domain context tests. |
| P-46 | Psyche reads arbitrary Coven output paths or swaps evidence/media bytes | Retrieve execution artifacts only as opaque attempt-bound streams through a W1-classified contract; verify graph/node/attempt/familiar-snapshot/project, expiry, hash, size, and media type. | Cross-attempt ID, expired reference, stream substitution, and hash/size/type tests. |
| P-47 | An intentional identity change silently inherits old sessions or authority | Psyche blocks changed snapshots and requires an audited identity rebind; every old execution must be terminal or authoritatively fenced through a W1-classified Coven contract before reactivation; old sessions never resume under the new snapshot. | Unauthorized/mismatched/lost-response/active-work/adoption-unknown fence and old-session-resume tests. |
| P-48 | Psyche treats capability presence or a previously healthy Coven connection as continuing execution permission | Test capability-present-but-denied separately from missing capability; terminate/stall Coven after discovery and each execution boundary; block affected nodes without local fallback; require G4 real-daemon conformance. | Fake- and real-Coven negative admission plus mid-flight termination/stall tests. |
| P-49 | A stalled execution leaves a streaming preview active indefinitely | Persist a bounded preview maximum age; on expiry freeze current content and attempt only a newly surface-authorized final edit; ambiguous edits create `delivery_unknown`, and surface-policy denial/unavailability leaves `preview_finalize_blocked`. | Time-travel, policy-loss, definitive-edit-failure, and ambiguous-edit crash tests. |
| P-50 | Child delegation widens scope | Persist an immutable non-widening envelope; reject any child project, authority, budget, evidence, or surface right absent from the parent. | Delegation lattice and mutation property tests. |
| P-51 | Parent cancellation leaves descendants running | Persist propagation intent; require terminal acknowledgement or explicit unknown for every adopted descendant before parent cancellation completes. | Fan-out cancellation, daemon-loss, and restart tests. |
| P-52 | Lease expiry causes duplicate execution | Use fencing tokens and authoritative adoption lookup; expiry alone never permits redispatch. | Clock-skew, stalled worker, takeover, and lost-response tests. |
| P-53 | Result attaches to the wrong node or familiar | Bind graph/node/attempt/session/project/familiar snapshot and result/artifact digests immutably. | Cross-binding substitution tests. |
| P-54 | Budget is double-released, undercharged, or falsely called hard | Idempotent reserve/consume/release; label limits hard only with enforceable W1 contract and trustworthy usage evidence. | Concurrent accounting, restart, and unenforceable-class tests. |
| P-55 | Generator self-certifies | Reject verifier identity/session equal to the generating attempt when independent verification is required. | Same-session, same-familiar, and forged-reviewer tests. |
| P-56 | Verifier reads changed evidence | Seal a content-addressed evidence set before verdict; any changed artifact creates a new set and invalidates the pending verdict. | TOCTOU and digest-substitution tests. |
| P-57 | Add-on or marketplace metadata poisons routing | Treat metadata as untrusted; only operator-authored allowlists and pinned reviewed digests grant contributions; Node workers remain same-user trusted code. | Malicious manifest, contribution spoof, revocation, and crash tests. |
| P-58 | Surface actor is confused with principal | Persist explicit versioned mapping; missing, stale, duplicate, or conflicting mappings fail closed. | Actor reuse, account change, collision, and stale-mapping tests. |
| P-59 | Graph state is inferred from stale session output | Require ordered cursors and authoritative terminal state; output text or process exit alone cannot settle graph state. | Cursor replay, stale-output, restart, and terminal mismatch tests. |
| P-60 | Another adapter leaks protocol IDs into core authority | Keep actor/locator data behind `psyche.surface_event.v1`; core identity, graph, verification, and execution contracts accept only canonical IDs/digests. | Cross-adapter schema and identifier-injection tests. |

## Security invariants

These invariants are release-blocking:

- No code path launches a harness, edits project files, writes familiar memory,
  or accesses Coven-protected resources without a conformant Coven execution
  contract.
- No prompt, surface, harness, model, add-on, or Coven response defines familiar
  identity; Psyche resolves one immutable snapshot.
- No intent or graph node is admitted without one mapped principal, familiar
  snapshot, project, constraints, and required-evidence policy.
- No child widens its delegation envelope.
- No graph reports verified success without sealed required evidence and an
  allowed verdict.
- No generator acts as its own independent verifier.
- No Coven request or surface effect proceeds from capability presence alone;
  each passes its own authority domain.
- No Coven launch or input is retried after an inconclusive adoption lookup.
- No cancellation-unknown node or descendant is redispatched as terminal.
- No Telegram update is acknowledged before its durable disposition exists.
- No username authorizes a sender.
- No group authorization is inherited from DM pairing.
- No delivery fallback changes account, chat, topic, or action class.
- No raw bot token is accepted in config or emitted in diagnostics.
- No unknown callback becomes prompt text.
- No outbound ambiguity is recorded as confirmed success.
- No public webhook listener starts implicitly.

## Secret lifecycle

1. Config stores a provider-specific reference.
2. The secret adapter resolves it through an argv-only child process or
   OS-native API.
3. Psyche bounds output size, trims only documented transport whitespace, and
   rejects an empty or structurally invalid token.
4. Psyche calls `getMe` and requires the numeric bot ID to equal the account's
   configured pin before activating or swapping the client.
5. The token is installed in a token-scoped HTTP client and excluded from debug
   formatting.
6. Bot API URLs are built by a redacting URL type whose display form replaces
   the token.
7. Reload creates a new client, verifies the same bot ID, atomically swaps it,
   closes
   the previous client, and clears old buffers where the platform permits.
8. A different bot ID is not an in-place rotation. The operator creates a new
   account ID or performs an audited rebind that archives token-scoped cursors,
   callbacks, pairings, and pending deliveries before route revalidation.
9. Shutdown closes clients and drops token buffers.

Raw environment-token fallback is rejected in v1 because process environments
are commonly captured by diagnostics and service managers. Secret providers
may themselves use environment-backed authentication without exposing the bot
token to Psyche config.

### Webhook secret lifecycle

Webhook mode requires a second secret-provider reference whose resolved value
is structurally valid and distinct from the bot token. The value is excluded
from debug output, process arguments, database state, metrics, and crash
reports. Rotation first resolves the candidate, obtains an account-activation
decision for the new config digest, and calls `setWebhook`. Only a successful
Telegram response promotes it. The prior secret remains accepted for exactly
five minutes for in-flight requests, after which its buffer is dropped. A
failed rotation keeps the prior secret and listener state unchanged. Health
reports only secret version IDs and digest prefixes.

## Webhook posture

- Default listener: `127.0.0.1`, operator-selected port.
- HTTPS termination: trusted reverse proxy or direct Rust TLS when explicitly
  configured.
- Secret header: exactly one value, fixed maximum length, constant-time match.
- Failed-auth rate limiting: applies only to failed requests so authenticated
  Telegram delivery cannot be starved by the same bucket.
- Request controls: maximum headers, body bytes, nesting depth, read timeout,
  connection limit, and JSON duplicate-key rejection.
- Success: empty 2xx only after durable commit.
- Failure: generic status and request correlation ID, with detail confined to
  redacted local logs.

The webhook secret is distinct from the bot token and rotates independently.

## Approval bridge posture

Telegram is a presentation and input adapter for authority-domain approvals,
not an approval store. Psyche orchestration approvals and Coven execution or
protected-resource approvals remain distinct.

- Approval buttons carry random callback nonces, not shell commands, project
  paths, policy text, or self-contained authorization claims.
- Psyche stores the nonce binding, including authority domain, and sends an
  acknowledgement to Telegram promptly.
- Approval prompts show the familiar, action class, bounded redacted summary,
  expiry, and origin surface.
- The default destination is a configured approver DM.
- A decision event contains the numeric Telegram actor, mapped Psyche
  principal, authority domain, and nonce binding. The owning authority
  revalidates final authorization; an approval cannot cross domains.
- Edited messages do not alter the action digest.
- Expired, superseded, already decided, or mismatched approvals render a stable
  refusal and cannot restart the action.

## Data protection

The data directory is created mode `0700`; regular state files are mode `0600`.
Psyche rejects group/world-writable parent directories unless an explicit
development-only override is active.

Content encryption at rest uses a random data key protected by the OS keychain.
On platforms where that profile is unavailable, production startup requires an
explicit acknowledgement that filesystem permissions are the only at-rest
control. The acknowledgement is stored as non-secret policy metadata and
reported by `psyche doctor`.

Database backups are not automatic in v1. `psyche export` applies retention and
redaction before writing a mode-`0600`, checksummed, encrypted archive. The
minimum artifact contains a versioned manifest and the transactionally
consistent retained state needed to recover unresolved ingress, intents,
graphs, attempts, Coven adoptions/cancellations, evidence, verdicts,
deliveries, routing, conversations, callbacks, and audit history. It
excludes tokens, resolved secret values, and secret-provider references.
Canary and production cutover require a clean restore drill.

## Abuse and overload handling

- Per-account, per-sender, per-chat, and global admission limits protect the
  durable queue.
- Authenticated updates are persisted when capacity exists; overload returns a
  retryable webhook failure or stops polling before advancing the offset.
- Pairing attempts have stricter limits and do not reveal whether a numeric
  user is otherwise authorized.
- Unsupported media is rejected before full buffering.
- Repeated permanent failures are dead-lettered under attempt and age gates.
- Operators can pause an account or route without deleting durable state.

## Clean-room controls

- Requirements cite only public behavior and protocol documentation.
- Implementers do not consult OpenClaw source while writing Psyche code.
- No OpenClaw source file, symbol map, internal module name, test fixture,
  source-derived prompt, error prose, configuration parser, or asset enters the
  Psyche repository. Separately reviewed operator-authored prompts,
  declarations, hooks, commands, and configuration may migrate as data.
- Test fixtures are independently authored from Telegram's public Bot API
  schema and Psyche's own contracts.
- A provenance note accompanies each parity implementation PR.
- Review checks both functional independence and copyright/license hygiene.

## Residual risks

1. A same-user local compromise can access process memory, the Coven socket,
   identity files, or the database.
2. Telegram controls update delivery and can observe bot messages and metadata.
3. Bot API sends have an unavoidable accepted-but-response-lost ambiguity
   because Telegram provides no client idempotency key.
4. Authorized users can still prompt-inject a model; containment relies on
   typed context, non-widening graph policy, and independent execution/surface
   boundaries, not on perfect model obedience.
5. Secret providers, proxies, harnesses, models, and dependencies may be
   compromised outside Psyche's boundary.
6. Message deletion in Telegram does not guarantee deletion from clients,
   notifications, Telegram infrastructure, or previously created Coven events.

These risks must appear in operator documentation and may not be described as
solved.

## Incident response

Security events that immediately block an account or route:

- bot-token authentication failure or suspected disclosure;
- identity or Coven binding mismatch;
- unauthorized dispatch or approval attempt;
- webhook secret failure spike;
- database integrity failure;
- repeated delivery to a wrong surface;
- secret detection in logs or artifacts; or
- unexpected token ownership conflict during migration.

Graph-wide blocking additionally applies to identity-source confusion,
delegation widening, cross-attempt result/evidence binding, generator self-
certification, budget-accounting corruption, or unacknowledged descendant
cancellation.

Response:

1. stop intake for the affected account;
2. preserve the private database and redacted correlation bundle;
3. rotate the bot token or webhook secret when implicated;
4. revoke active callback nonces and pairings when actor scope is implicated;
5. confirm graphs, attempts, evidence, authority-domain approvals, and Coven
   sessions associated with the incident;
6. reconcile or authoritatively fence every `coven_adoption_unknown` attempt,
   preserve every `coven_cancellation_unknown`, and explicitly resolve every
   `delivery_unknown` without local state edits;
7. restore from a known release or roll back the transport;
8. document accepted, rejected, and ambiguous events; and
9. require a new live security probe before reactivation.

## Security acceptance

G1 cannot pass, and Psyche cannot release, until:

- all six companion documents share the surface-neutral product and authority
  model;
- all invariants have automated negative tests;
- crash tests prove durable acknowledgement;
- webhook, media, callback, identity, and Coven-boundary fuzz targets run in
  CI;
- a security review finds no open critical or high-severity issue;
- release artifacts pass secret scanning and provenance verification;
- minimum export/restore and ambiguous-recovery drills pass on a clean host
  profile;
- migration and token-rotation drills pass on a dedicated account;
- W1 classifies Coven requirements before any implementation assignment; and
- repository creation and W1 remain blocked until G1; implementation planning,
  issues, and production code remain blocked until G3.
