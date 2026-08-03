# Psyche W1 Coven Contract Audit

**Status:** W1 complete; G3 approved 2026-08-02

**Audit date:** 2026-08-02

**Audited Coven commit:** `16c170b0f7f1c735467cf419ad077bcd5381b8a5`

**Work item:** `coven-psy-w1`

**Scope:** Read-only audit of current Coven Rust code, public contracts,
persistence, and executable tests. No implementation issue or production code
is authorized by this document.

## Executive summary

- **High confidence:** three of the 21 required behaviors are current:
  versioned session create/input/inspect/events/terminate (`C-S2`), durable
  ordered event cursors (`C-S7`), and authoritative persisted terminal/orphan
  state (`C-S8`).
- **High confidence:** no current session contract binds a Psyche familiar
  snapshot, graph, node, attempt, request digest, adoption key, or fence. The
  current `familiar_id`, `conversation_id`, and `callerFamiliarId` fields are
  not substitutes for those bindings.
- **High confidence:** the existing scheduler, hub-job, executor-dispatch, and
  Coven Calls surfaces do not prove the Psyche multi-agent profile. They lack
  immutable attempt/session correlation, adoption lookup, ambiguity fencing,
  child-bound artifacts, and cross-restart coordinator recovery.
- **High confidence:** 17 rows are `planned` because they are required for G4
  or G6 but the complete public behavior does not exist. One row (`C-M4`) is
  `rejected` as a Coven aggregate responsibility: Psyche owns descendant
  traversal and parent completion; Coven must only supply the per-session
  cancellation primitive required by `C-S9`.
- **Medium confidence:** the current API is usable, but its public negotiation
  docs conflict with implementation at `GET /api/v1/api-version`, and the
  lifecycle docs omit live statuses implemented by Rust. These contradictions
  keep `C-S1` from `current` until a single exact contract is selected.
- **High confidence:** G3 approved this classification and dependency order on
  2026-08-02. The decision permits bounded post-G3 planning and issue creation;
  it does not declare G4 or G6 conformance or approve any implementation by
  implication.

## 1. Research questions

1. Which public, versioned Coven contracts currently expose the session and
   capability behavior required by Psyche?
2. Which adoption, lookup, fencing, cursor, terminal, cancellation, result, and
   restart guarantees exist in code, tests, and durable state?
3. Which familiar, project, graph, node, attempt, and digest bindings are
   immutable and fail closed?
4. Do current scheduler, hub, executor, or Coven Calls surfaces satisfy any
   production multi-agent behavior without inference?
5. Which apparent capabilities are internal, undocumented, contradictory, or
   fake-only and therefore cannot count as current production contracts?
6. What is the smallest ownership-preserving dependency order for accepted
   gaps after G3?

## 2. Method and decision rules

The audit uses repository code, tests, schemas, and contract documentation at
the pinned commit as primary sources. A behavior is classified as a whole; a
partial route or internal field does not promote a row.

| Classification | Audit rule |
|---|---|
| `current` | Public, versioned, documented, implemented, and covered by executable positive and negative tests; required persistence behavior is identifiable. |
| `current_but_undocumented` | Complete behavior is implemented and tested, but no stable public contract exists. |
| `planned` | Required G4/G6 behavior is incomplete; the missing behavior has an owner and dependency order, but no implementation is authorized before G3. |
| `optional` | Not required for the affected release capability. |
| `rejected` | Outside Coven ownership or intentionally unsupported. |

Absence searches covered the session request/record/store and daemon runtime
for `requestId`, `digest`, `graph`, `node`, `attempt`, `adopt`, `fence`,
`snapshot`, `delegation`, and parent/child correlation. Existing scheduler and
hub records were inspected separately rather than inferred to be session
contracts.

## 3. Classification overview

| Profile | Current | Current but undocumented | Planned | Optional | Rejected |
|---|---:|---:|---:|---:|---:|
| Single-node (`C-S1`-`C-S12`) | 3 | 0 | 9 | 0 | 0 |
| Multi-agent (`C-M1`-`C-M9`) | 0 | 0 | 8 | 0 | 1 |
| **Total** | **3** | **0** | **17** | **0** | **1** |

Current behavior is not equivalent to a full profile. G4 remains blocked by
the nine planned single-node rows. G6 remains blocked by every multi-agent row
and by the single-node dependencies it inherits.

## 4. Single-node evidence matrix

### C-S1 - Exact API and capability negotiation

- **Classification:** `planned`
- **Public contract:** Partial: `GET /api/v1/health`,
  `GET /api/v1/api-version`, `GET /api/v1/capabilities`, and
  `GET /api/v1/capabilities/:harnessId` under `coven.daemon.v1`; no single
  internally consistent exact negotiation contract.
- **Code evidence:** `api::COVEN_API_VERSION`,
  `api::COVEN_API_NAMED_VERSION`, `api::HealthCapabilities`, and
  `api::handle_request_with_runtime` [S02]. Unknown `/api/v2/*` routes return a
  structured `404 invalid_request`; unknown harness capability targets return
  `404 harness_not_found`.
- **Test evidence:** Positive
  `api::tests::routes_versioned_health_request_to_named_api_contract` and
  `api::tests::routes_control_capabilities_discovery_to_json`; negative
  `api::tests::rejects_unknown_api_version_prefixes` and
  `api::tests::unknown_harness_capability_manifest_fails_closed_with_structured_error`
  [S05].
- **Persistence/restart:** Negotiation is compiled behavior and does not require
  mutable persistence. The daemon version and capability response are rebuilt
  on restart.
- **Gap:** `GET /api/v1/api-version` returns route tokens (`v1`) from code while
  the public reference documents named values (`coven.daemon.v1`). The socket
  guide also documents obsolete health capability fields. Required-capability
  failure is currently a client rule, not a request-level daemon negotiation.
- **Owner/order:** Coven contract owner; order O1 before any Psyche adapter is
  frozen.
- **Confidence:** High.

### C-S2 - Session create, input, inspect, events, and terminate

- **Classification:** `current`
- **Public contract:** `POST /api/v1/sessions`,
  `POST /api/v1/sessions/:id/input`, `GET /api/v1/sessions/:id`,
  `GET /api/v1/sessions/:id/events` or `GET /api/v1/events`, and
  `POST /api/v1/sessions/:id/kill` [S01].
- **Code evidence:** `api::session_launch_from_payload`,
  `api::launch_session`, `api::record_input`, `api::events_response`, and
  `api::kill_session`; `session_launch::resolve_launch_paths` and
  `session_launch::validate_harness` preserve Rust-side authority [S02, S03].
- **Test evidence:** Positive
  `api::tests::launch_request_invokes_runtime_and_persists_running_session`,
  `api::tests::input_request_invokes_live_runtime_hook`,
  `api::tests::routes_sessions_list_and_detail_requests_to_json`,
  `api::tests::events_response_has_paginated_envelope_with_next_cursor`, and
  `api::tests::kill_request_invokes_live_runtime_hook`. Negative
  `launch_request_rejects_cwd_outside_project_root`,
  `launch_request_with_unknown_harness_returns_400_upfront_no_session_row`,
  `input_and_kill_reject_completed_sessions_as_not_live`, and
  `input_and_kill_reject_unknown_sessions` [S05].
- **Persistence/restart:** Session rows and events are SQLite records. On
  restart, daemon-owned `running` rows become `orphaned`; they are inspectable
  but reject live input/kill [S03, S04].
- **Gap:** None for the narrow lifecycle surface. This does not imply adoption,
  binding, cancellation acknowledgement, artifact, or graph conformance.
- **Owner/order:** Current Coven contract; Psyche consumes only after O1.
- **Confidence:** High.

### C-S3 - Familiar snapshot and attempt binding

- **Classification:** `planned`
- **Public contract:** `none`. `familiarId` is an optional roster identifier,
  not an immutable familiar snapshot or attempt binding.
- **Code evidence:** `api::SessionLaunch` accepts `familiar_id`; `store::SessionRecord`
  persists only `familiar_id`, `project_root`, and generic conversation/session
  metadata. No snapshot revision/digest, graph, node, attempt, or request digest
  columns exist [S02, S03].
- **Test evidence:** Positive partial evidence
  `api::tests::launch_request_persists_familiar_id_on_the_session_row`; negative
  partial evidence
  `api::tests::launch_request_rejects_unknown_familiar_id_without_inserting_session`.
  No match/mismatch test exists for the required immutable tuple [S05].
- **Persistence/restart:** `familiar_id` and `project_root` persist; the required
  tuple does not exist.
- **Gap:** Bind an opaque Psyche-defined snapshot/project/graph/node/attempt/
  request-digest tuple to the authoritative session record and reject mismatch
  without Coven interpreting familiar or graph policy.
- **Owner/order:** Psyche defines canonical opaque fields/digests in W2; Coven
  contract owner adds binding at O2; Psyche adapter follows in W5.
- **Confidence:** High.

### C-S4 - Stable request adoption

- **Classification:** `planned`
- **Public contract:** `none`. `POST /api/v1/sessions` always generates a new
  UUID; callers cannot supply a stable adoption id/digest pair.
- **Code evidence:** `api::session_launch_from_payload` assigns
  `Uuid::new_v4()` before insertion; the session schema has no request/adoption
  key or digest [S02, S03].
- **Test evidence:** Existing launch tests prove new session creation only. No
  same-id/same-digest, same-id/different-digest, concurrent replay, lost-response,
  or reopen test exists.
- **Persistence/restart:** No adoption record exists.
- **Gap:** Durable unique adoption key plus immutable digest conflict semantics.
- **Owner/order:** Coven contract owner, O3 after O2 and before W5 dispatch.
- **Confidence:** High.

### C-S5 - Adoption lookup and non-adoption proof

- **Classification:** `planned`
- **Public contract:** `none`. Session lookup by daemon-generated session id is
  not lookup by a pre-dispatch adoption id.
- **Code evidence:** `GET /sessions/:id` reads `store::get_session`; no adoption
  disposition type or lookup index exists [S02, S03].
- **Test evidence:** `api::tests::returns_not_found_for_unknown_session` proves
  ordinary session absence only. No lost-response or
  adopted/proven-not-adopted/unknown test exists.
- **Persistence/restart:** No adoption disposition exists.
- **Gap:** Read-only lookup with three authoritative outcomes and retention
  compatible with Psyche recovery/deduplication windows.
- **Owner/order:** Coven contract owner, O4 after O3.
- **Confidence:** High.

### C-S6 - Ambiguity fence

- **Classification:** `planned`
- **Public contract:** `none`.
- **Code evidence:** No session request fence token, generation, or terminal
  disposition exists. Scheduler redispatch and proposal recovery use unrelated
  job/proposal state and cannot be reused by inference [S02, S03, S09].
- **Test evidence:** No possible-adoption, fence, stale-fence, or redispatch
  exclusion test exists for sessions.
- **Persistence/restart:** No session fence state exists.
- **Gap:** Authoritative return-or-fence operation that prevents dispatch while
  disposition remains ambiguous.
- **Owner/order:** Coven contract owner, O4 after O3 and before Psyche recovery.
- **Confidence:** High.

### C-S7 - Ordered event cursor

- **Classification:** `current`
- **Public contract:** `GET /api/v1/events?sessionId=...&afterSeq=...&limit=...`
  and `GET /api/v1/sessions/:id/events`, returning
  `{events,nextCursor,hasMore}` [S01].
- **Code evidence:** `api::EventCursor`, `api::EventsResponse`,
  `api::events_response`; `store::EventRecord.seq` is SQLite `rowid`, and
  `store::list_events_with_options` applies `seq > afterSeq`, ordered ascending,
  with a maximum page size of 1,000 [S02, S03].
- **Test evidence:** Positive
  `store::tests::events_have_monotonic_seq_fields`,
  `store::tests::list_events_with_after_seq_returns_tail`,
  `api::tests::events_endpoint_combines_after_seq_cursor_with_limit`, and
  `api::tests::events_endpoint_supports_after_event_id_cursor`; negative
  `events_endpoint_returns_structured_error_for_non_integer_after_seq`,
  `events_endpoint_returns_structured_error_for_non_integer_limit`, and
  `events_endpoint_returns_structured_error_for_unknown_session` [S05].
- **Persistence/restart:** Events and cursor positions persist in SQLite across
  daemon restart. Redacted events default to 30-day retention and are removed
  only by explicit retention pruning or session sacrifice; callers must handle
  a cursor older than retained history [S03, S08].
- **Gap:** No G4 gap. The W2 suite must still prove duplicate-safe client
  checkpointing against the unchanged real adapter.
- **Owner/order:** Current Coven contract; Psyche checkpoint ownership in W2/W5.
- **Confidence:** High for daemon ordering; medium for end-to-end duplicate
  handling, which remains a client responsibility.

### C-S8 - Authoritative terminal state

- **Classification:** `current`
- **Public contract:** `SessionRecord.status`, `SessionRecord.exit_code`, and
  persisted `exit` events through the v1 session/event endpoints [S01].
- **Code evidence:** `daemon::record_session_exit` conditionally moves a running
  row to `completed`, `failed`, or conversation `idle`; API kill records
  `killed`; `daemon::recover_orphaned_sessions` moves stale daemon-owned
  `running` rows to `orphaned`. Late exit events cannot overwrite `killed`
  [S02, S04].
- **Test evidence:** Positive
  `stream_json_integration::codex_json_turn_failure_with_zero_exit_marks_ledger_failed`,
  `store::tests::updates_session_status_and_exit_code`,
  `daemon::tests::recovers_persisted_running_sessions_as_orphaned`, and
  `daemon::tests::exit_event_does_not_overwrite_killed_session_status`.
  Negative `input_and_kill_reject_orphaned_sessions_as_not_live` proves that
  disconnect/orphan state is not treated as a live success [S05, S06].
- **Persistence/restart:** Status, exit code, and exit event are durable SQLite
  state; restart explicitly reconciles stale running rows.
- **Gap:** Lifecycle documentation must enumerate `idle` and `killed`, but the
  authoritative state mechanism needed by Psyche's one-shot daemon-managed
  sessions exists.
- **Owner/order:** Current Coven contract; documentation conflict included in O1.
- **Confidence:** High for one-shot daemon-managed sessions.

### C-S9 - Cancellation acknowledgement

- **Classification:** `planned`
- **Public contract:** Partial: `POST /api/v1/sessions/:id/kill` returns
  `202 {ok:true,accepted:true}` and persists `killed` after the runtime kill call.
  There is no explicit unresolved result.
- **Code evidence:** `api::kill_session` invokes `SessionRuntime::kill_session`,
  then records `killed`; `daemon::LiveSessionRuntime::kill_session` issues the
  platform kill but does not wait for an observed child exit before success
  [S02, S04].
- **Test evidence:** Positive partial evidence
  `api::tests::kill_request_marks_session_killed_and_records_event` and
  `daemon::tests::live_runtime_kills_and_removes_registered_session`; negative
  `api::tests::kill_request_runtime_failure_returns_500_not_daemon_crash`.
  No acknowledged-terminal-versus-unresolved contract test exists [S05].
- **Persistence/restart:** `killed` is durable; a failed/uncertain kill has only
  the immediate error response and no persisted unresolved cancellation state.
- **Gap:** Persist a typed cancellation disposition that is terminal only after
  authoritative acknowledgement, otherwise explicit unresolved/
  `termination_unknown`-compatible state.
- **Owner/order:** Coven per-session contract owner, O5 after O4; Psyche maps the
  unresolved outcome and blocks dependants.
- **Confidence:** High.

### C-S10 - Result and artifact association

- **Classification:** `planned`
- **Public contract:** Partial, incompatible surface:
  `GET /api/v1/sessions/:id/artifacts/:artifactId?raw=1` retrieves optional
  encrypted raw event payloads. No content-addressed attempt-bound result or
  artifact contract exists.
- **Code evidence:** `store::sensitive_artifacts` binds artifact id to session,
  event, kind, ciphertext, and expiry; `encrypted_artifacts::artifact_aad`
  authenticates session/event/kind. It lacks content digest, graph/node/attempt,
  familiar snapshot, project, media type, and size [S03, S07].
- **Test evidence:** Positive partial evidence
  `store::tests::raw_artifacts_are_encrypted_when_explicitly_enabled`; negative
  `api::tests::raw_artifact_endpoint_is_disabled_by_default`,
  `api::tests::raw_artifact_endpoint_returns_404_for_expired_artifact`, and
  `store::tests::pruning_sensitive_artifacts_honors_expires_at_and_created_at_cutoff`.
  No cross-binding rejection tests exist [S05].
- **Persistence/restart:** Optional ciphertext persists in SQLite with a default
  seven-day raw-artifact retention; redacted events default to 30 days. Neither
  window is tied to Psyche recovery or adapter deduplication [S03, S08].
- **Gap:** Opaque content-addressed references with full immutable execution
  binding, validated type/size/digest, and explicit lifetime. Required only for
  Psyche paths that exchange bytes, but those paths cannot ship without it.
- **Owner/order:** Psyche defines canonical metadata in W2; Coven protected
  resource owner adds O6 after O2; W5 integrates only proven paths.
- **Confidence:** High.

### C-S11 - Restart persistence

- **Classification:** `planned`
- **Public contract:** Partial: session status, terminal data, redacted events,
  cursor sequence, and optional raw artifacts are durable. Adoption, fences,
  cancellation uncertainty, and Psyche result binding do not exist.
- **Code evidence:** SQLite schemas and startup orphan recovery cover only the
  existing partial state [S03, S04].
- **Test evidence:** Positive partial evidence
  `store::tests::creates_schema_idempotently_by_opening_same_db_twice`,
  `daemon::tests::recovers_persisted_running_sessions_as_orphaned`, and cursor/
  artifact persistence tests. No crash matrix covers before/after adoption,
  lookup, fence, cancellation acknowledgement, or bound result persistence.
- **Persistence/restart:** Partial as described; therefore the composite row is
  not current.
- **Gap:** Persist and recover every O2-O6 state transition before G4.
- **Owner/order:** Coven contract/storage owner, O7 after O2-O6; Psyche W2 suite
  supplies crash fixtures and W5 runs them unchanged.
- **Confidence:** High.

### C-S12 - Structured denial

- **Classification:** `planned`
- **Public contract:** Partial: v1 `{error:{code,message,details}}` with stable
  route, session, harness, live-state, launch, input, kill, and artifact codes
  [S01].
- **Code evidence:** `api::api_error`, version routing, launch validation, live
  control handlers, and raw artifact handler [S02].
- **Test evidence:** Positive partial evidence across
  `rejects_unknown_api_version_prefixes`,
  `launch_request_rejects_cwd_outside_project_root`,
  `unknown_harness_capability_manifest_fails_closed_with_structured_error`,
  `input_and_kill_reject_unknown_sessions`, and raw artifact denial tests.
  No required binding mismatch, adoption conflict/unknown, fence denial, or
  mid-flight authority-loss test exists [S05].
- **Persistence/restart:** Errors are responses, while the durable state behind
  many required error dispositions does not exist.
- **Gap:** Stable redacted codes for every O2-O7 mismatch and unresolved state;
  no local fallback.
- **Owner/order:** Coven contract owner, O8 implemented alongside each O2-O7
  primitive and frozen before W5.
- **Confidence:** High.

## 5. Multi-agent evidence matrix

The multi-agent rows inherit all C-S requirements. Current scheduler and hub
records are explicitly excluded as proof: their `job_id`, node routing, and
redispatch semantics are multi-host scheduling contracts, not Psyche graph,
familiar, attempt, delegation, or session-adoption contracts [S09].

| ID | Classification | Public contract and evidence | Persistence/test evidence | Gap and owner/order | Confidence |
|---|---|---|---|---|---|
| C-M1 | `planned` | `none`. `callerFamiliarId` creates a display-oriented Coven Calls sidecar entry with caller/callee/session, but there is no immutable parent graph/child node/attempt/session tuple [S10]. | Atomic JSON persistence; tests `coven_calls::tests::emit_running_creates_file_and_returns_id` and `emit_terminal_patches_record`. No mismatch or immutability tests. | Extend O2 with opaque graph/node/attempt/parent correlation. Psyche owns graph meaning; Coven owns exact session binding. | High |
| C-M2 | `planned` | `none`. Daemon session ids are generated per request; hub `job_id` uniqueness is unrelated. | No concurrent/replay test preventing a second session for one Psyche attempt. | O3 unique adoption tuple, after O2. Coven owner. | High |
| C-M3 | `planned` | `none`. No child adoption id/digest lookup. | No lost-response/restart child adoption test. | O3/O4 applied to child attempts. Coven owner; Psyche persists coordinator state. | High |
| C-M4 | `rejected` | `none` as a Coven aggregate. Coven does not own the Psyche descendant set or parent graph completion. | Coven Calls cancellation is display state; no authoritative descendant traversal or aggregate cancellation test. | Psyche W4/W8 owns descendant enumeration, propagation, and parent completion. Each child uses the Coven O5 per-session acknowledgement from C-S9. | High |
| C-M5 | `planned` | `none`. Current raw event artifacts bind only session/event/kind, not graph/node/attempt/familiar/project [S07]. | Encryption, expiry, and pruning tests exist; cross-child/cross-attempt rejection tests do not. | O6 full binding. Coven resource owner plus Psyche canonical metadata owner. | High |
| C-M6 | `planned` | Partial incompatible surface: session listing plus restart `orphaned` state has no graph/attempt adoption key or coordinator liveness/fence disposition. | `daemon::tests::recovers_persisted_running_sessions_as_orphaned` proves daemon orphan marking only. | O4 query by immutable binding and authoritative disposition; Psyche decides whether its coordinator is live. | High |
| C-M7 | `planned` | `none`. Scheduler redispatch is node-failure routing, not ambiguous child adoption fencing [S09]. | Scheduler recovery tests do not inject a lost child-session adoption response. | O4 return-or-fence semantics reused per child. Coven owner. | High |
| C-M8 | `planned` | `none` for the cross-daemon guarantee. Coven can persist/orphan its own sessions; Psyche restart state does not yet exist. | Partial Coven reopen/orphan tests only; no either-daemon crash matrix. | O7 Coven recovery plus Psyche W4/W8 recovery; unchanged W2 suite proves no duplicate or invented terminal state. | High |
| C-M9 | `planned` | `none`. Unknown familiar and project/cwd denial exist, but graph/node/attempt/delegation/digest mismatch fields do not [S02, S05]. | Partial negative tests only; no exact tuple mismatch matrix. | O2/O8 exact opaque binding rejection. Psyche owns delegation authorization; Coven compares the bound delegation digest without interpreting graph policy. | High |

## 6. Evidence that does not count as conformance

| Apparent match | Why it is insufficient |
|---|---|
| `conversation_id` | Groups/resumes chat sessions. It is neither a stable request adoption id nor a graph attempt id, and it has no request digest conflict rule. |
| `familiar_id` | Identifies a configured roster entry. It does not bind an immutable familiar snapshot revision/digest. |
| `callerFamiliarId` and `cave-coven-calls.json` | Produces a UI delegation ledger. The caller field is not part of `SessionRecord`, failures to write the sidecar do not fail launch, and no graph/node/attempt identity is enforced. |
| Scheduler `job_id`, redispatch, and loop state | Implements multi-host routing and failure simulation. The public contract explicitly does not define Psyche graph, delegation, adoption, fencing, or verifier semantics. |
| Hub job/executor dispatch persistence | Persists node routing/envelopes, not one Psyche attempt to one authoritative harness session. Upsert behavior is not digest-based adoption conflict behavior. |
| Sensitive raw artifacts | Encrypts optional event payloads. IDs are not content addressed and bindings omit project, familiar snapshot, graph, node, attempt, digest, type, and size. |
| Fake service or future W2 schemas | No fake Psyche service exists in this audited repository, and future fake behavior cannot prove a current real-daemon contract. |

## 7. Public-contract conflicts

| Conflict | Evidence | Consequence |
|---|---|---|
| API version vocabulary | Rust returns `apiVersion: "v1"` and `supportedApiVersions: ["v1"]` from `/api/v1/api-version`, while `docs/reference/api-contract.md` documents named `coven.daemon.v1` values. `/health` returns the named value [S01, S02]. | `C-S1` is planned until one exact negotiation rule is authoritative. |
| Health capability shape | `docs/daemon/socket-api.md` shows `actions` and `harnesses`; Rust `HealthCapabilities` exposes `travel`, `scheduler`, `hub`, `executorDispatch`, `eventCursor`, and `structuredErrors` instead [S01, S02]. | A client following the socket guide can negotiate the wrong fields. |
| Terminal status vocabulary | `docs/SESSION-LIFECYCLE.md` lists created/running/completed/failed/orphaned, while Rust also persists `killed` and conversation `idle` [S01, S04]. | Does not block Psyche one-shot terminal reads, but O1 must freeze the enum before W5. |
| Cancellation wording | Public API promises only accepted kill, while the profile needs acknowledged terminal or explicit unresolved state [S01, S02]. | `C-S9` remains planned even though process-tree kill tests pass. |

## 8. Accepted gap dependency order

This is an ownership and sequencing recommendation, not authorization to create
implementation issues.

| Order | Owner | Accepted outcome | Rows |
|---:|---|---|---|
| O1 | Coven contract/docs | Choose one API-version vocabulary, correct capability/lifecycle docs, freeze exact terminal values and required-capability failure behavior. | C-S1, C-S8 documentation |
| O2 | Psyche schema owner, then Coven contract owner | Psyche defines opaque canonical binding fields/digests; Coven persists and exact-compares them without taking identity, graph, or delegation authority. | C-S3, C-M1, C-M9 |
| O3 | Coven contract/storage | Stable adoption key plus digest, uniqueness, conflict, and one-attempt/one-session semantics. | C-S4, C-M2, C-M3 |
| O4 | Coven contract/storage | Adoption lookup, explicit unknown/non-adoption proof, return-or-fence, binding query, and compatible retention. | C-S5, C-S6, C-M6, C-M7 |
| O5 | Coven runtime/storage; Psyche coordinator | Per-session terminal cancellation acknowledgement or durable unresolved disposition. Psyche aggregates descendants. | C-S9; C-M4 remains rejected as Coven aggregate |
| O6 | Psyche metadata owner, Coven protected-resource owner | Opaque content-addressed result/artifact references with full execution binding and lifecycle. | C-S10, C-M5 |
| O7 | Coven storage/runtime plus Psyche recovery | Crash-safe persistence and recovery across O2-O6; either daemon restart cannot duplicate work or invent terminal state. | C-S11, C-M8 |
| O8 | Coven contract | Stable redacted denials for every mismatch, conflict, unknown, fence, cancellation, and authority-loss state. | C-S12, C-M9 |

O2 must precede O3 because adoption uniqueness is scoped by the immutable
binding. O3 precedes O4 because lookup/fencing requires an adopted request
record. O5 and O6 bind to O2/O3 state. O7 proves their restart behavior. O8 is
specified with each primitive and frozen before real-adapter conformance.

## 9. G3 decision

**Decision: approved by Val on 2026-08-02.** The W1 classification matrix and
O1-O8 dependency order are the accepted basis for bounded post-G3 planning.
Endpoint names, implementation designs, and G4/G6 conformance remain subject
to their own test-first plans and gates.

The approval means:

1. `C-S2`, `C-S7`, and `C-S8` are the only current Psyche-relevant behaviors at
   the audited commit.
2. The 17 planned rows are accepted gaps in the order above, without approving
   endpoint names or implementation designs.
3. `C-M4` is assigned to Psyche coordination, with Coven limited to C-S9's
   per-session acknowledgement.
4. Existing scheduler/hub/Coven Calls surfaces cannot be cited as G4/G6 proof.
5. Maintainers may create the smallest bounded issues and test-first child plans
   for O1-O8 after this decision is merged. Production child dispatch remains
   blocked until G6.

Any later revision must identify the exact row, classification, or
ownership/order change and pass through a separately reviewed contract update.

## 10. Verification record

Baseline at the pinned commit:

```text
cargo build --workspace
  passed

cargo test --workspace --locked
  1,629 passed; 4 ignored; 0 failed
```

Audit-focused tests were rerun for named version health, unknown-version and
unknown-capability denial, launch validation, event cursor positive/negative
cases, event ordering, orphan recovery, and late-exit terminal preservation.
All passed. The final documentation branch must still pass the complete local
CI gate set before the G3 review PR is opened.

## 11. Source ledger

All sources are repository-primary and pinned to
`16c170b0f7f1c735467cf419ad077bcd5381b8a5`.

| ID | Source | Use |
|---|---|---|
| S01 | `docs/API-CONTRACT.md`; `docs/reference/api.md`; `docs/reference/api-contract.md`; `docs/daemon/socket-api.md`; `docs/SESSION-LIFECYCLE.md` | Public v1 shapes, errors, cursor, lifecycle, and contradictions. |
| S02 | `crates/coven-cli/src/api.rs` | Version routing, capabilities, session request parsing, live controls, events, artifacts, structured errors, and API tests. |
| S03 | `crates/coven-cli/src/store.rs` | Session/event/artifact schemas, persistence, cursor queries, retention, and store tests. |
| S04 | `crates/coven-cli/src/daemon.rs` | Live runtime, process kill, terminal persistence, orphan recovery, and daemon tests. |
| S05 | `crates/coven-cli/src/api.rs` test module | Public handler positive and negative contract evidence. |
| S06 | `crates/coven-cli/tests/stream_json_integration.rs` | Real subprocess terminal, failure, cancellation, descendant reap, and ledger agreement evidence. |
| S07 | `crates/coven-cli/src/encrypted_artifacts.rs` | Current encrypted event-artifact AAD and key behavior. |
| S08 | `crates/coven-cli/src/privacy.rs`; `docs/reference/cli-logs.md` | Default and configurable event/raw-artifact retention. |
| S09 | `crates/coven-cli/src/hub.rs`; `crates/coven-cli/src/proposal_scheduler.rs`; scheduler/hub sections of `crates/coven-cli/src/api.rs` and `store.rs` | Challenge to scheduler/hub equivalence claims. |
| S10 | `crates/coven-cli/src/coven_calls.rs` | Challenge to delegation-ledger equivalence claims. |
| S11 | `specs/psyche/COVEN_PREREQUISITES.md` | Normative C-S/C-M behaviors and W1 classification rules. |
