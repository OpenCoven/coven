# Coven Automations Protocol v1 (`coven.automations.v1`)

Status: Proposed implementation contract

Tracks: OpenCoven/coven#855 (this specification), OpenCoven/coven#854 (parent program), OpenCoven/coven#816 (landed foundation)

Machine-readable artifacts: [`spec/coven-automations/v1/`](../../spec/coven-automations/v1/) — JSON Schemas, state machines, capability negotiation, compatibility matrix, golden vectors, and a pinned TypeScript projection.

## Normative language

The key words MUST, MUST NOT, REQUIRED, SHOULD, SHOULD NOT, and MAY are to be interpreted as normative requirements.

## Purpose

Turn the internal automation structs and control actions into a stable, independently testable protocol so Cave, the SDK, Psyche adapters, runtimes, and future implementations can consume automations without importing Coven internals or reproducing lifecycle semantics by hand. The protocol is one canonical truth for definitions, occurrences, runs, attempts, authority bindings, receipts, commands, events, and failures.

This document specifies the contract. It does not change schedule authority, familiar identity, or authority semantics: those stay in their canonical layers (see [Implementation boundaries](#implementation-boundaries)).

## Current gap, with code paths

The #816 foundation is a valid v1 Rust implementation, but the public contract is still inferred from implementation. Each gap below cites the code as of this writing:

1. **Definitions are schedule + prompt records.** `RoutineDefinition` (`crates/coven-cli/src/automations/definition.rs`) carries `schemaVersion`, `id`, `name`, `status` (`ACTIVE | PAUSED`), `rrule`, `timeoutMinutes`, `runtime`, `familiarId`, `prompt`, and little else. The legacy `outputTarget` field is explicitly refused until the stable contract has a pinned definition revision and crash-recoverable delivery state machine. There is no revision counter, no integrity digest, no trigger/action unions, no lifecycle beyond two states, no provenance, and no retention policy.
2. **JSON payloads are assembled inside the control action router.** Every wire shape is hand-built in `control_plane.rs` (`automation_list_payload`, `automation_create_payload`, `automation_runs_payload`, ... at `crates/coven-cli/src/control_plane.rs`), so the router is the only specification of the payloads.
3. **Domain failures are not machine-typed.** The control router now distinguishes payload success from failure before constructing an accepted event, but `rejected_action` still exposes only an unversioned free-form `reason` string and HTTP 400. Clients cannot branch on stable automation error codes or a versioned error envelope.
4. **No revision/adoption model.** `update_definition` (`crates/coven-cli/src/automations/store.rs`) mutates the row in place; nothing records which revision a caller expected, and `intentId` on control actions (`control_plane.rs`) is echoed, never adopted, stored, or replay-checked.
5. **No versioned event envelope or replay reducer.** `ControlEvent` (`control_plane.rs`) has `kind/action/origin/intentId/payload` but no schema version, no event id, no per-stream sequence, and no timestamps; there is no changefeed at all — Cave polls list/get endpoints.
6. **Lifecycle semantics are stringly typed and partial.** Occurrences distinguish `scheduled` from `manual` sources, but states remain free strings (`'planned'/'claimed'/'running'/'succeeded'/'failed'/'skipped'` in `crates/coven-cli/src/automations/occurrences.rs`). Settlement terminal states are exactly `[succeeded, failed]` (`OCCURRENCE_TERMINAL_STATES`), stale planned fences collapse to `skipped`, and lease recovery maps straight to `failed` with reason `lease expired` (`recover_expired_leases`). There are no eligible/dispatching/recovering/cancelled/timed_out states, no run state machine beyond `status` strings, and no attempt object at all — only an `attempt` counter incremented by claim (`claim_due_occurrence`).
7. **No receipts, no digests, no integrity anywhere** in the automations module; run outcomes are ledger rows (`automation_runs.log_json`, `exit_code`) with no tamper-evident summary.
8. **No capability negotiation.** `capabilities()` (`control_plane.rs`) lists action ids, but nothing lets a client ask which trigger/action/policy variants an implementation executes, and nothing forces a definition with an unsupported variant to fail explicitly.
9. **Adoption-key gaps.** Run and occurrence ids are wall-clock derived (`fresh_id`, `crates/coven-cli/src/automations/runner.rs` — `format!("{prefix}-{millis}")`), so a retried `coven.automations.run` command can create a second occurrence/run rather than replaying the first outcome.

## Contract profile and versioning

- The wire contract profile is the string `coven.automations.v1`, carried as `schemaVersion` on every object (`spec/coven-automations/v1/protocol-version.json`).
- Contract version is separate from implementation/release version: envelopes carry the producer's `implementationVersion` alongside the contract profile, and clients MUST NOT infer semantics from release versions.
- Unknown profiles fail closed with `SCHEMA_VERSION_UNSUPPORTED` (`coven.automations.v0` and future `coven.automations.v2` are both refusals — golden vectors pin both).
- Additive evolution rules and per-field change classes are machine-readable in `compatibility-matrix.json`; see [Compatibility and evolution](#compatibility-and-evolution).

## Data model

All objects are JSON per draft 2020-12 schemas under `spec/coven-automations/v1/`, with `additionalProperties: false`: unknown fields fail closed, and optional, non-semantic data travels only in the explicit `extensions` bag (keys `x-*` or reverse-DNS; preserved on round-trip, never interpreted until promoted by a new profile).

### `AutomationDefinition`

Specified by `automation-definition.schema.json`. Field groups (all required unless noted):

- `automationId` — stable identity, charset `[A-Za-z0-9._-]`, 1..=96 chars, matching the #816 validator in `definition.rs` (`AUTOMATION_ID_MAX_CHARS`).
- `revision` — monotonic integer, starts at 1, incremented by exactly one per accepted mutating command.
- `integrity` — SHA-256 over the RFC 8785 (JCS) canonical serialization of the definition body with the `integrity` member removed (`common.schema.json#/$defs/digest`). The digest pins what a revision means; receipts and occurrences pin it so history stays verifiable even if the definition store is later revised or lost.
- `schemaVersion` — the constant `coven.automations.v1`. The #816 numeric `schemaVersion: 1` is the pre-contract encoding (see [Migration](#migration-from-816)).
- `lifecycleState` — `draft | paused | active | disabled | invalid` (tombstoning is a deletion marker, not a state; see below).
- `display` — `name` (required, 1..=160), optional `description`, `tags`.
- `trigger` — exactly one versioned union. v1 ratifies `schedule` only (scoped RRULE per `crates/coven-cli/src/automations/rrule.rs`: `FREQ=DAILY|WEEKLY`, optional `BYHOUR`, optional `BYDAY` for weekly; anything else is refused at validation). The union admits future variants as new branches in future profiles without redefining v1 fields, and consumers reject unknown variants via capability negotiation.
- `conditions` — zero or more; v1 defines zero variants (the slot is a boolean-false schema), so any value fails validation until a future profile adds branches.
- `action` — exactly one versioned union. v1 ratifies `familiarInvocation` (non-empty `prompt`, optional `cwd`), mirroring the #816 prompt requirement and the runner's no-cwd failure rule.
- `binding` — `familiarBindingPolicy: "exact"` plus `familiarId` and the authority/approval policy reference. Exact binding is the only v1 policy: the familiar recorded at activation is the familiar every run binds; rebinding requires a new revision.
- `runtimeRequirements` — `runtimeId` + capability keys + optional model (required for active/paused/disabled definitions; the #816 `runtime` field maps to `runtimeId`, default `coven-code`).
- `policies` — `timeout.perRunMinutes` (1..=44640, as `definition.rs`), `retry` (max attempts 1..=10, backoff policy, retryable failure classes), `concurrency.overlap: forbid` (the #816 `RoutineOverlap::Forbid`), `misfire.disposition: latest` (the #816 `RoutineMisfire::Latest`), `delivery` (optional atomic `outputTarget` — commit only after a completed, verified run, per `definition.rs`), and `retention` classes for occurrence history, run logs, and receipts.
- `provenance` — creator principal, creation/update timestamps, optional `importedFrom` marker (set to `codex.automation.toml` by the #816 importer in `import_legacy.rs`).
- `activation` — optional `effectiveFrom`/`effectiveUntil` window; outside the window the definition behaves as paused without a lifecycle change.

### `AutomationOccurrence`

Specified by `automation-occurrence.schema.json`.

- `occurrenceId`, `automationId`, `automationRevision` (exact revision pinned forever).
- `triggerIdentity` (`schedule.slot` with `rruleRef`, or `manual.request` with requester) and the canonical `occurrenceKey`: `automationId@scheduledFor` for slots — this is the wire projection of the idempotent planning fence `UNIQUE(automation_id, scheduled_for)` (`crates/coven-cli/src/automations/occurrences.rs`), and `automationId@manual-<adoptionKey>` for manual runs.
- `scheduledFor` / `observedAt` / `eligibleAt` timestamps.
- `fence` — monotonic `generation` per occurrence plus claimant and lease expiry; the contract-level form of the lease columns and `attempt` counter in `occurrences.rs`.
- `state` + `stateReason` — the occurrence state machine (below).
- `misfireDisposition` — `collapsed_to_latest` records the #816 misfire-latest collapse (`plan_latest_due_occurrence` walks forward from the later of creation time and the latest fenced slot and fences exactly one slot; `occurrences.rs`).
- `claimMetadata` (bounded lease 1..=1440 minutes, per `claim_due_occurrence`).
- `activeRunRef` — present while exactly one accepted run owns the fence generation.
- `cancellation` — request vs acknowledgment vs reconciliation timestamps (cancellation is a request until acknowledged or reconciled).
- `recovery` — evidence class and resolved disposition for the recovering/recovery_required path.
- `eventWindow` — first/last sequence of the occurrence's authoritative stream, so readers can resume without gaps.

### `AutomationRun`

Specified by `automation-run.schema.json`.

- `runId`, occurrence correlation (`occurrenceId`, `automationId`, `automationRevision`).
- `binding` — the exact familiar, the exact principal/authority/approval (references only; authority semantics live in the canonical authority layer), and the exact runtime descriptor/capabilities observed at dispatch. These are frozen at acceptance; retries change attempts, never the run's bindings.
- `state` — `accepted | running | succeeded | failed | cancelled | timed_out | ambiguous`.
- `attemptCount` (monotonic) and `currentAttemptId` while unsettled.
- `terminalDisposition` (required when terminal) with outcome and failure class.
- `delivery` (none/pending/committed/refused/rolled_back; `committed` only after a completed, verified run — the #816 atomic output rule), artifact references, `resultDigest`.
- `receiptRef` — the receipt recording this run's disposition.
- `startedAt` (required) and `finishedAt` (required when terminal — schema-enforced).

### `AutomationAttempt`

Specified by `automation-attempt.schema.json`.

- `attemptId`, `runId`, `occurrenceId`, monotonic `attemptNumber` (never reused within a run).
- `adoptionKey` — workers adopt by key; replays return the same attempt instead of creating a second one.
- `priorDisposition` — required when `attemptNumber > 1`: every retry names the attempt and outcome it retries (`failed | timed_out | ambiguous | cancelled`).
- `dispatchFence` — the occurrence fence generation plus a dispatch generation guarding double dispatch.
- `workerCorrelation` — worker id and the single bound `sessionId`; a second session bind on one attempt is `ILLEGAL_TRANSITION`, never a silent overwrite.
- `retryClassification` — initial / automatic_retry / operator_retry / operator_recovery, with the eligible-classes snapshot.
- `leaseObservations` — heartbeat evidence; expired evidence moves work to recovery and can never be reinterpreted as success.
- `outputCursors` — event/log cursors a resuming worker continues from without re-emitting.
- `state` — `adopted → dispatching → started → observing → succeeded | failed | cancelled | timed_out | ambiguous`. `ambiguous` is terminal for the attempt (see state machines).

### `AutomationReceipt`

Specified by `automation-receipt.schema.json`. Immutable and versioned; written once, never revised.

- Pins `definitionDigest` (the revision's digest), the occurrence fence generation, run, attempt, exact identity, authority/approval, and runtime.
- `exercisedCapabilities` and `sideEffectClass` (`none → local_read → local_write → external_read → external_mutation → irreversible_external_mutation`), which drive whether ambiguous work can be resolved as `failed_deterministic`.
- `outcome` with partial failures and recovery disposition; timestamps; `producer` identity.
- `integrity` — digest over the canonical receipt body plus an `authentication` marker (`none | producer-hmac | cosign`); unauthenticated receipts are integrity-checked but not provenance-proof, and consumers MUST surface the distinction.
- `privacy` — classification and retention.

## Lifecycle semantics (state machines)

Machine-readable source of truth: `spec/coven-automations/v1/state-machines.json`. Clients do not author state; transitions are committed by command handlers or the scheduler, never by arbitrary client writes.

### Occurrence

```text
planned -> eligible -> claimed -> dispatching -> running
    -> succeeded | failed | cancelled | timed_out | recovery_required

planned/eligible -> skipped | superseded | cancelled
claimed/dispatching with expired evidence -> recovering -> failed | recovery_required
recovery_required -> failed | dispatching (explicit operator recovery only)
```

Terminal: `succeeded, failed, cancelled, timed_out, skipped, superseded`. Non-obvious ratifications:

- `dispatching -> failed` exists for deterministic pre-side-effect launch refusals (`launch_refused`); without it, every refused launch would be ambiguous, which is wrong — the #816 runner already treats a launch error as a recorded failure with a reason (`runner.rs`).
- `recovering` is automatic reconciliation in progress; `recovery_required` needs an explicit operator command. `recovery_required` is deliberately **not terminal**: its only exits are `failed` (operator determines no side effects were possible) or `dispatching` (operator-approved recovery attempt, which creates a new attempt carrying `priorDisposition: ambiguous`).
- `skipped` vs `superseded`: skipped means policy disposed of the slot (overlap forbid, paused, invalid); superseded means a newer definition revision replaced this one before it was claimed.

### Attempt

```text
adopted -> dispatching -> started -> observing
    -> succeeded | failed | cancelled | timed_out | ambiguous
```

`ambiguous` is terminal for the attempt (dispatch sent but no deterministic ack, or evidence lost). The occurrence carries the recovery; the attempt never re-opens. `dispatching -> failed` covers deterministic launch refusal; `dispatching -> ambiguous` covers unconfirmed dispatch.

### Run

```text
accepted -> running -> succeeded | failed | cancelled | timed_out | ambiguous
```

`accepted` commits before any consequential side effect. A run spans its attempts: retry/recover increments `attemptCount` and swaps `currentAttemptId`; terminal `terminalDisposition` is written exactly once.

### Definition

```text
draft -> paused -> active -> paused | disabled | invalid | tombstoned
disabled -> paused (explicit re-enable only)
invalid -> paused (after a repairing revise) | tombstoned
tombstoned (terminal)
```

`draft` replaces the #816 implicit "created PAUSED" default (every import lands in `draft` until validated and explicitly paused/activated). Tombstoning is a deletion marker on the definition plus retention of all history.

### Invariants

`state-machines.json` carries the machine-readable form. The ten normative invariants, each traceable to a current-code motivation:

1. **Command adoption commits before consequential side effects** — closes the gap where `coven.automations.run` launches before any adoption record exists (`runner.rs` fences and launches in one call with no command record).
2. **One occurrence fence cannot own two accepted runs** — generalizes the compare-and-set claim in `claim_due_occurrence` (`occurrences.rs`) from claiming to acceptance.
3. **One attempt cannot bind two runtime sessions** — a second bind is `ILLEGAL_TRANSITION`, never an overwrite.
4. **Terminal states do not regress** — #816 already enforces a weak form (`settle_occurrence` only settles claimed/running; `record_run_finish` only finishes `running` rows); the contract ratifies it for every state.
5. **Absence of runtime evidence cannot become success** — #816's `run_routine_now` marks `succeeded` on a bare `launch_session` ack with no completion evidence (`runner.rs`); under the contract that launch ack is `dispatching -> started`, and settling `succeeded` requires verified evidence.
6. **Cancellation is a request until acknowledged/reconciled** — typed `CANCEL_PENDING` while a running attempt has not acknowledged.
7. **Retry creates a new attempt and never rewrites the prior attempt** — attempts are immutable after settling.
8. **Ambiguous mutating work is not automatically retried** — only `occurrence.recover.v1` with an explicit operator determination opens work after `ambiguous`.
9. **Definition revision changes never rewrite historical occurrences/runs** — every record pins `automationRevision` + definition digest.
10. **Deleting a definition tombstones it without erasing required history** — replaces `delete_definition`'s hard `DELETE FROM automation_definitions` (`store.rs`), which erases identity while leaving orphan occurrences.

## Commands, idempotency, and revision semantics

Specified by `command-envelope.schema.json`. Every command is an envelope:

```json
{
  "schemaVersion": "coven.automations.v1",
  "command": "definition.revise.v1",
  "adoptionKey": "adopt:revise-daily-notes-0002",
  "expectedRevision": 2,
  "origin": { "principal": { "principalId": "principal:tim" }, "channel": "sdk" },
  "intent": { "statement": "Move the daily notes slot to 10:00." },
  "payload": { "definition": { } }
}
```

- **`adoptionKey` (required):** the stable request/adoption key. First commit wins; the key is stored with the committed outcome. A repeat with the same key returns the first committed outcome unchanged (`outcome: "replayed"` with `replay.firstCommittedAt`) — identical bytes, no second event, no second revision. A repeat with the same key but different command/payload is `ADOPTION_REPLAY_MISMATCH` (409) carrying what the key actually committed, so callers reconcile instead of guessing. Recommendation: keys are caller-chosen ULIDs; handlers may derive per-attempt keys deterministically (`adopt:<runId>:<attemptNumber>`).
- **`expectedRevision`:** required for `definition.revise/activate/pause/disable/tombstone`, forbidden otherwise (schema-enforced). Commit happens only when the stored revision equals `expectedRevision`; mismatch returns `REVISION_CONFLICT` (409) with `currentRevision`, committing nothing.
- **`origin`:** authenticated principal, channel, authentication class, requested-at, correlation id. Transports that cannot authenticate must refuse upstream; the envelope records, never decides.
- **`intent.statement`:** explicit human-authored intent, recorded on events and receipts.

Command catalog (all names versioned in the envelope): create, revise, activate, pause, disable, tombstone; run now; cancel occurrence/run/attempt; retry with explicit prior disposition; recover with explicit evidence determination; list/get/history/health; events read/subscribe; legacy import. The response is one of `committed`, `replayed`, `rejected` — with `result` only on committed/replayed and `error` only on rejected.

**Idempotency storage note for implementers:** the adoption key must be persisted in the same transaction as the state change it drives (a `command_adoption` table keyed by adoption key storing the serialized committed response), so replays are answerable without recomputation.

## Errors and status mapping

Specified by `error-envelope.schema.json`. Domain failures are errors; a rejected or failed domain operation MUST NOT be wrapped as `accepted: true` merely because routing succeeded. This is the direct fix for the current `automation_event` behavior (`control_plane.rs`) that emits `ok/accepted/completed` around `{"error": ...}` payloads.

Twenty typed codes with a frozen HTTP mapping (also machine-readable in the schema): validation and schema-version refusals → 400; not found → 404; tombstoned → 410; adoption/revision conflicts, cancel-pending, overlap, out-of-order stream → 409; capability, transition, retry-disposition, ambiguous-retry refusals → 422; authority/approval → 403; payload too large → 413; concurrency → 429; deadline → 504; internal → 500.

Control-action transport mapping: `POST /api/v1/actions` (`crates/coven-cli/src/api.rs`, `route_action`) keeps its `ControlActionResponse` shape but the automations actions MUST surface domain failures as `ok: false, accepted: false, status: rejected` with the typed error envelope embedded, and the transport status MUST be the mapped one — exactly what `rejected_action` already does for routing failures. The same command handlers back both transports and return the same typed outcomes; the router stops assembling domain payloads (its current payload builders move into the shared handlers).

## Event/changefeed

Specified by `event-envelope.schema.json`.

- **Envelope:** `schemaVersion`, `eventId` (globally unique), `stream {kind, id}`, gapless `sequence` per stream, `recordedAt`/`observedAt`, `producer`, optional `causation` (adoption key, cause event id, correlation id), object ids as applicable, `kind`, user-safe `summary` (no secrets, no prompts), typed `payload`, `privacy`, optional `integrity`. The event kind discriminates the payload: definition lifecycle kinds use definition payloads, each transition kind requires its matching entity, and misfire, receipt, and snapshot kinds use only their corresponding payload.
- **Streams:** `automation/{id}`, `occurrence/{id}`, `run/{id}`, plus a global `feed`. Stream-local sequences are gapless and append via compare-and-set; out-of-order appends are refused (`STREAM_OUT_OF_ORDER`), never reordered.
- **Delivery:** at-least-once. Consumers deduplicate on `eventId` and refuse regressions against their cursor (the golden vectors pin both).
- **Read:** `events.read.v1` with `after` (exclusive sequence) or `from` (timestamp, resolved to a concrete cursor in the response); `events.subscribe.v1` with an opaque `checkpoint`. Expired checkpoints return `CURSOR_EXPIRED` (410) with the expiry instant — never a silent rewind.
- **Rehydration:** the read model is a fold: dedupe by eventId → apply strictly-increasing sequences → final state. Reconnection and duplicates converge to the same state (vector `event-replay-rehydrates-deterministically`). Occurrence records carry `eventWindow` so a reader knows the stream bounds it read.
- **Compaction:** `feed.snapshot` events carry `throughSequence` plus compacted state; consumers fold the snapshot and apply strictly-later events. Retention may compact streams only behind a snapshot.

## Capability negotiation

`capabilities.json` lists supported v1 variants (trigger `schedule`, action `familiarInvocation`, policies `misfire.latest`, `overlap.forbid`, `timeout.required`, retention `standard`), an empty `experimental` list, and explicit `refused` entries (`trigger.webhook`, `action.pipeline`, `misfire.backfill`, `outputTarget.atomic`) with reasons. The delivery shape remains reserved in the schema and type projection, but schema validity does not override capability refusal. Rules:

- A definition referencing a variant absent from the producer's supported list MUST be refused with `CAPABILITY_UNSUPPORTED`, naming the variant. Nothing is guessed, defaulted, or silently downgraded — the same fail-closed stance as the #816 RRULE vocabulary gate (`rrule.rs` refuses unsupported frequencies instead of approximating).
- Refusal is per-variant and additive: refusing one variant says nothing about others.
- Unknown values inside a supported variant are still unknown variants.
- The negative path is also a schema property: v1 unions are closed, so an unknown variant fails schema validation before negotiation is even needed; producers that relax schema validation in future profiles still refuse at the capability layer.

## Canonicalization and digests

Digests (definition integrity, receipts, event integrity where required) are SHA-256 over RFC 8785 (JCS) canonical JSON: UTF-8, recursively key-sorted, no whitespace, minimal escaping, ES6 number formatting. The golden vectors pin actual digest values computed this way over integer/ASCII-only fixtures, so any conformant JCS implementation reproduces them byte-for-byte. Producers MUST NOT digest ad-hoc serializations.

## Migration from #816

Non-destructive, no data loss, no rewritten history:

1. **Definitions:** on first contract adoption, each stored `automation_definitions` row gains sidecar columns (`revision` = 1, `integrity` = digest over its existing `definition_json` bytes, lifecycle mapping `ACTIVE → active`, `PAUSED → paused`, default `draft` for import). `definition_json` bytes stay byte-identical — the digest is computed over them, not written into them — so pre-migration rows remain verifiable.
2. **Occurrences:** every existing row pins `automationRevision: 1` plus the definition digest; `attempt` counter maps to fence `generation` (claim already increments it in `claim_due_occurrence`); state strings map 1:1 (`planned/claimed/running/succeeded/failed`) with `succeeded/failed` becoming the v1 terminals of the same names and implementation-only `skipped` migrating to v1 `superseded`.
3. **Runs:** `automation_runs` rows map to v1 runs with `state` from `status`; the ledger's `exit_code/log_json/output_commit` columns carry into `terminalDisposition`/`delivery` without backfilling receipts — receipts exist only for runs that produce them after adoption (receipts are never fabricated for history).
4. **Wire compatibility:** the legacy control actions (`coven.automations.*`, `control_plane.rs`) continue to respond during migration, each response additionally carrying the contract profile; new commands are additive. `coven.automations.import` maps to `legacy.import.v1` (`source: codex-automation-toml`), keeping the non-destructive, created-PAUSED/draft semantics of `import_legacy.rs`.
5. **Nothing is deleted:** no definitions, occurrences, or run history are erased at any step (acceptance criterion), and the migration is idempotent (re-running adopts nothing twice — the adoption table marks it).

## Implementation boundaries

- `crates/coven-cli/src/automations/**` may implement the contract but is not its specification; this directory plus `spec/coven-automations/v1/` is the specification.
- Control actions and any HTTP routes delegate to the same command handlers and return the same typed outcomes; the router stops being a payload assembler.
- Cave, SDK, and Psyche consume the pinned artifacts (`schemas`, `test-vectors.json`, `coven.automations.v1.d.ts`) as packed/released artifacts — never source-relative imports, never hand-maintained parallel types.
- This protocol does not move schedule authority into Psyche or authority semantics into Cave: it binds references (`principalId`, `approvalPolicyRef`, `familiarId`) and defers semantics to their canonical layers.

## Corresponding Rust types (pinned mapping, implementation deferred)

The Rust projection is mechanical and lands in a follow-up implementation PR (this issue specifies; it does not implement): a `contract` module with serde types renamed to camelCase (`#[serde(rename_all = "camelCase")]`, the existing wire style in `definition.rs`), where each struct maps 1:1 to a schema (`AutomationDefinition`, `AutomationOccurrence`, `AutomationRun`, `AutomationAttempt`, `AutomationReceipt`, `CommandEnvelope`, `CommandResponse`, `ErrorEnvelope`, `EventEnvelope`), `#[serde(deny_unknown_fields)]` on every v1 struct to mirror `additionalProperties: false`, `serde_json::Value` for the extension bag, and round-trip tests generated from the golden vectors. Status enums map to the schemas' enums exactly; the schema files remain the source of truth.

## Verification matrix

| Issue requirement | Contract artifact | Required test suite |
| --- | --- | --- |
| Schema validation + Rust round-trip | all schemas | `schema-validation`, `rust-round-trip` |
| State-machine invariant preservation | `state-machines.json` | `state-machine-property-tests` |
| Adoption replay/conflict | command envelope semantics | `request-adoption-replay-and-conflict` |
| Expected-revision conflict | command envelope | `expected-revision-conflict` |
| Duplicate/out-of-order replay | event envelope + vectors | `duplicate-and-out-of-order-event-replay` |
| Typed transport/domain error mapping | error envelope + status mapping | `typed-transport-domain-error-mapping` |
| Golden vectors runnable outside the Coven crate | `test-vectors.json` (self-contained, digest recipe inline) | `golden-vectors-external-runners` |
| Packed/released artifact tests + cross-repo canaries | pinned `.d.ts` + schemas + vectors | `packed-artifact-canaries` (Coven, SDK, Cave pin exact artifacts) |
| #816 migration | migration section above | migration proof: pre/post digest equality, row counts, no deletes |

All suites are enumerated with `releaseState: proposed` in `conformance-manifest.json`.

## Alternatives considered (recommendations for the maintainer)

1. **Tolerant reader vs fail-closed unknown fields.** Chosen: fail-closed (`additionalProperties: false`) with an explicit namespaced extension bag, matching `spec/device-pairing/v1` ("unknown required restrictions fail closed") and the security-first posture of the repo. Alternative considered: ignore unknown fields for additive ease — rejected because silent reinterpretation is exactly the drift this issue exists to end; additive fields still land via a minor profile with dual-emit.
2. **`recovery_required` terminal vs non-terminal.** Chosen non-terminal with two explicit exits (`failed` or a new operator-approved attempt), because the issue requires retry/recover commands with explicit prior disposition while also requiring that ambiguous work is never auto-retried; a terminal `recovery_required` would make explicit recovery impossible without violating "retry creates a new attempt". Alternative considered: model recovery as a new occurrence — rejected because it forks the audit trail for one logical slot.
3. **Revision integer vs content-addressed definition ids.** Chosen monotonic integer + digest (the digest already gives content addressing); integer revisions give callers a trivial compare-and-set and match the store's row model. Alternative considered: content-hash identity — rejected as the primary key (history joins become opaque), kept as the integrity layer.
4. **RFC 8785 vs a Coven-private canonical form.** Chosen JCS: portable, implemented everywhere, and sufficient for v1's integer/ASCII digest fixtures. Alternative considered: a private canonicalization — rejected; it would force every canary to import Coven code, violating the independence requirement.
5. **Attempt-level retry within a run vs run-per-attempt.** Chosen retries-within-a-run (`attemptCount`, `priorDisposition`), matching the issue's wording ("current attempt", "retry creates a new attempt"). Alternative considered: a new run per retry — rejected; it would fragment authority bindings the run is supposed to pin.
6. **`cancelled` as request vs state.** Chosen: the request lives in `cancellation` metadata with `requestedAt`; `cancelled` is only committed on acknowledgment/reconciliation, keeping `CANCEL_PENDING` representable. Alternative considered: immediate cancelled state — rejected; it would let a client-authored transition lie about runtime state.

## Non-goals

- Implementing every future trigger or action variant (v1 ships `schedule` + `familiarInvocation` only).
- A general-purpose workflow language (no pipelines, no step graphs).
- Client-authored run state (clients issue commands; the authority commits transitions).
- Defining familiar identity or authority semantics independently of their canonical layers (this protocol binds references only).
