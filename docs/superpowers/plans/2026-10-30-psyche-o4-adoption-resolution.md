---
source_adjacent_reason: "Defines the test-first implementation sequence for the approved producer-owned O4 adoption resolution design."
---

# Psyche O4 Adoption Resolution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement O4 for C-S5, C-S6, C-M6 and C-M7: read-only adoption lookup, durable return-or-fence, immutable-binding discovery, and retained recovery evidence without equating admission exclusion with execution containment.

**Architecture:** Extend O3's ledger with append-only exclusions and a unified discovery index. Reuse the exact key/attempt lock namespace and enforce cross-table exclusion at the final durable SQLite transaction. Dedicated owner-IPC execution routes require a store authority pinned before dispatch; read-only lookup and discovery never initialize storage or create lock files.

**Tech Stack:** Rust 2021, Serde/serde_json, Rusqlite 0.40 (existing bundled/functions/hooks/limits features), fs2, SHA-256, existing ChaCha20Poly1305/base64/rand crates, Node.js 24, TypeScript/Vitest, Markdown. Use repository-pinned Rust 1.99.1; no new crate is required.

**Spec:** `specs/psyche/O4_CONTRACT_DESIGN.md`, approved by the user on 2026-10-30 at [PR #1627](https://github.com/OpenCoven/coven/pull/1627) at commit `f1209c3e1f9606e55da90d7b329124be3dd14485`; approval recorded in PR #1627 comment `6518059993`. Owning issue: [#1629](https://github.com/OpenCoven/coven/issues/1629), under #1230. Source inspection baseline: `0d22aca7f935f6e632e112c51171c2cd4c4f3477`.

**Status:** Plan proposed for review. Design approval does not approve this plan or feature code. Execute only after the design is delivered on protected main and a maintainer explicitly approves this plan's exact revision. Record both merge and approval revisions on #1629 before the first implementation edit.

## Global Constraints

- The contract is `psyche.adoption_resolution.v1`; O2 remains `psyche.execution_binding.v1` and O3 remains `psyche.request_adoption.v1`. Preserve byte equality, closed nested objects, and existing O3 replay response/status behavior.
- Outcomes are exactly `adopted`, `proven-not-adopted`, and `unknown`. Empty lookup is unknown; only committed durable exclusion proves non-adoption. Store failures are errors, never healthy absence.
- All five O4 routes require transport-derived `OwnerLocalIpc` authority. A binding/key/receipt/health capability is not authentication or authorization. TCP receives static refusal before any store probe.
- Persist the negotiated authority id before original dispatch. Require `expectedAuthorityId` on dedicated O4 launch/input and recovery routes; recheck inside their final transaction. Never fall back to legacy O3 execution routes.
- Launch exclusion covers the global key and O3's exact five-field attempt scope. Input exclusion covers its global request key only. A changed input key cannot reconcile an earlier ambiguous input.
- Return existing adoption without runtime invocation. O3 releases its gate before runtime work; neither that lock nor a retained session status proves containment. O5/O7 and Psyche blocking remain separate requirements.
- Retain adoption, reservation, exclusion, authority metadata, and discovery evidence indefinitely. No release, expiry, pruning, rebind, automatic redispatch, or replacement execution is added.
- Restored/forked stores with copied ids cannot prove continuity. Mixed-version writers and unresolved migration-era old runtime activity must be isolated/quarantined before O4 is advertised. Never attest that a schema number alone stops older binaries.
- Familiar Contract owns identity/person binding; Threads owns protected-surface authorization; Psyche owns canonical request serialization, graph/coordinator meaning, and blocked recovery. No Telegram policy, principal mapping, surface delivery, or identityRebind mapping enters Coven.
- Ordinary prose paragraphs occupy one logical source line. Preserve required local/Policy/privacy gates and exact-head hosted evidence. Do not turn fake/G2 checks, a plan, or a docs merge into G1b/G4/G6 or #1237 acceptance.

## Review Focus

1. Duplicate JSON members at any nesting depth must fail before dispatch; add Task 1's raw-body visitor tests rather than testing only a pre-parsed `Value` that already lost duplicates.
2. Health and recovery reads on an empty/read-only home must create no files, migrations, locks, or timestamps; Task 3 and Task 6 assert filesystem snapshots and store write counters.
3. A durability barrier can fail after COMMIT has succeeded; Task 2 and Task 3 require exact retry to retain the same fence id and repeat the barrier before acknowledgement.
4. An old daemon can have passed adoption commit before migration; Task 2's old-writer test and Task 8's migration handoff must preserve quarantine for that runtime interval rather than infer containment from a new SQL guard.
5. Query pagination can outlive status changes and receive cross-selector or modified cursors; Task 5 binds the cursor to exact selector/authority/high-water and tests fresh-scan discovery of later records.

## File and responsibility map

| File | Change and responsibility |
|---|---|
| `crates/coven-cli/src/adoption_resolution.rs` | Create closed O4 wire types, strict raw-body parser, immutable request identity, typed redacted errors, response/page types. |
| `crates/coven-cli/src/adoption_resolution/store.rs` | Create durable authority/exclusion/discovery schema, strict retained reads, fence insertion, retention and writer-format guards. |
| `crates/coven-cli/src/adoption_resolution/cursor.rs` | Create authenticated bounded cursor codec using existing AEAD/base64 dependencies. |
| `crates/coven-cli/src/adoption_resolution/api.rs` | Create owner-gated lookup/return-or-fence/query adapters and exact denial rendering. |
| `crates/coven-cli/src/adoption_resolution/tests.rs` | Create module-level parser, store, API and race regression tests using existing CLI fixture patterns. |
| `crates/coven-cli/src/main.rs` | Register `mod adoption_resolution;`; no unrelated module movement. |
| `crates/coven-cli/src/store/schema.rs` | Register per-connection O4 writer marker, invoke O4 initialization after existing migration commit, retain existing O3 triggers. |
| `crates/coven-cli/src/store.rs` | Integrate exclusion resolution, raw insert guards, final input lease/adoption transaction and sacrifice checks. |
| `crates/coven-cli/src/adoption_gate.rs` | Add deadline-bounded acquisition over the existing sorted hashed lock paths. |
| `crates/coven-cli/src/api.rs` | Dispatch five routes, thread expected authority through existing adopted launch/input paths, retain O3 legacy semantics and runtime ordering. |
| `crates/coven-cli/src/api_health.rs` | Add optional fail-closed O4 negotiation fields; base/static health does not invent an authority. |
| `crates/coven-cli/src/request_authority.rs` | Add `allows_adoption_resolution()` for owner IPC only. |
| `crates/coven-cli/src/daemon.rs` | Load O4 readiness at startup, expose immutable runtime negotiation snapshot, preserve existing IPC/TCP/body/time limits and startup cleanup. |
| `crates/coven-cli/tests/adoption_resolution.rs` | Create isolated real-daemon IPC, kill/stall/reopen, replacement and old-writer integration tests. |
| `packages/openclaw-coven/src/adoption-resolution.ts` | Create closed public O4 wire types, validation and response parsing only. |
| `packages/openclaw-coven/src/client.ts` | Add explicit O4 negotiation and dedicated methods using existing owner-validated socket transport. |
| `packages/openclaw-coven/src/client.test.ts`, `compat.test.ts`, `runtime.test.ts` | Test exact wire routes, durable-pin callback ordering, denials, response parsing, and keep typed doubles complete. |
| `docs/API-CONTRACT.md`, `docs/SESSION-LIFECYCLE.md` | Publish delivered O4 wire/lifecycle limits and preserve O5–O8 exclusions after implementation acceptance. |
| `scripts/check-api-contract-docs.py`, `scripts/check-api-contract-docs-test.py` | Extend source-to-contract assertions without weakening existing O2/O3 checks. |
| `specs/psyche/PLAN.md`, `specs/psyche/O4_CONTRACT_DESIGN.md` | Record bounded implementation evidence only after delivery; design approval status is distinct from implementation. |

Paths/signatures below are proposed implementation decisions under the approved design. If source drift makes a named seam inaccurate, refresh the plan and review it before implementation rather than changing contract meaning. Do not initialize an alternate local work tracker.

### Task 0: Verify approval, protected design delivery, ownership, and baseline

**Files:** No product files; authoritative tracking is #1629 and #1230.

**Interfaces:** Consumes the design and plan approval revisions. Produces the pinned implementation baseline and exclusive issue claim.

- [ ] Read this plan and the full approved design; compare the protected-main copy with `f1209c3e` except separately approved status/tracker prose. Check #1629, #1230, psyche#12 and current PRs/claims. Require explicit approval for this exact plan; the earlier “i approve” approved the design and owning issue only.
- [ ] Fetch current main, enter a fresh implementation worktree, then acquire `issue-1629`. Record source SHA, toolchain, live owner and baseline gate results. Heartbeat during long tasks. Preserve another owner's worktrees and dirty state.
- [ ] Confirm no unresolved review requires an identityRebind action or compatibility mapping. Such a requirement returns to @BunsDev on psyche#12; no inferred equivalence is permitted. Do not wait on unrelated open parents when their explicit start gate is already satisfied.

### Task 1: Closed wire types, raw parsing, and denial vocabulary

**Files:** Create `adoption_resolution.rs` and `adoption_resolution/tests.rs`; modify `main.rs` to register the module.

**Interfaces:** Consume `execution_binding::ExecutionBinding`, `request_adoption::RequestAdoption`, and `store::RequestAdoptionOperation`. Produce `ResolutionRequest { expected_authority_id: String, operation: RequestAdoptionOperation, session_id: Option<String>, execution_binding: ExecutionBinding, request_adoption: RequestAdoption }`, `QueryRequest { expected_authority_id: String, selector: BindingSelector, limit: u32, cursor: Option<String> }`, and `BindingSelector { principal_ref: String, project_digest: String, graph_id: String, parent_session_id: Option<String> }`. Add `ResolutionData { contract, authority_id, disposition, adoption, fence }`, where typed `Disposition` serializes only the three approved literals; `AdoptionEvidence` and `FenceEvidence` have exactly design §3.3's member sets. Define `ResolutionError` variants `Invalid(&'static str)`, `Unsupported(&'static str)`, `Forbidden`, `Conflict(&'static str)`, `Fenced`, `Unknown`, `AuthorityLost`, `Busy`, `Store`; they carry no submitted or retained values.

- [ ] Write table-driven failing tests `o4_wire_rejects_every_missing_unknown_and_duplicate_member`, `o4_wire_preserves_o2_o3_bytes_and_expired_historical_proof`, and `o4_wire_rejects_bad_operation_target_and_query_limits`. Assertions include:

```rust
assert!(parse_resolution_body(raw_with_duplicate_nested_key).is_err());
assert_eq!(parse_resolution_body(valid_launch)?.session_id, None);
assert_eq!(parse_query_body(query_with_limit(0)), Err(ResolutionError::Invalid("limit")));
assert_eq!(parse_query_body(query_with_limit(101)), Err(ResolutionError::Invalid("limit")));
assert_eq!(parse_resolution_body(expired_exact_binding)?.execution_binding, expired_binding);
```

- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_wire -- --nocapture`; retain the expected missing-symbol or assertion failure.
- [ ] Implement reusable `parse_unique_value(body: &str) -> Result<serde_json::Value, ResolutionError>`, plus `parse_resolution_body(body: &str) -> Result<ResolutionRequest, ResolutionError>` and `parse_query_body(body: &str) -> Result<QueryRequest, ResolutionError>`. Use a recursive Serde map visitor that rejects duplicate keys before constructing `Value`; do not change global `api::parse_body`. Require canonical UUIDs, exact outer/nested keys including null members, launch digest equality, input UUID, and query limit 1–100. Reuse O2/O3 syntax parsers without expiry admission for historical lookup.
- [ ] Add static error/status/message/detail lookup matching design §8 verbatim; unsupported nested versions name only the nested static contract path. Test all variants, invalid JSON, non-object roots, key lengths 0/1/255/256, uppercase/wrong-prefix digests, parent/delegation presence, Unicode/whitespace and deterministic serialization. Assert debug/error chains exclude sentinel key, digest, binding, payload, cursor, path, and hidden session values.
- [ ] Re-run the focused command and O2/O3 parser suites; require pass. Commit with `git commit -s -m 'feat(psyche): define O4 resolution wire values'` after diff/privacy/secret checks.

### Task 2: Durable schema, writer compatibility, retention, and discovery index

**Files:** Create `adoption_resolution/store.rs`; modify `store/schema.rs`, `store.rs`, and `adoption_resolution/tests.rs`.

**Interfaces:** Produce `AuthorityState { id: String, cursor_key: [u8; 32] }`, `initialize(conn: &Connection) -> Result<AuthorityState, ResolutionError>`, `read_authority(conn: &Connection) -> Result<AuthorityState, ResolutionError>`, `check_authority(conn: &Connection, expected: &str) -> Result<(), ResolutionError>`, and `register_writer(conn: &Connection) -> anyhow::Result<()>`. Produce `read_exact(conn: &Connection, request: &ResolutionRequest) -> Result<ResolutionData, ResolutionError>` and `commit_absent_fence(conn: &Connection, request: &ResolutionRequest, now: &str) -> Result<ResolutionData, ResolutionError>` for the transaction owner in Task 3. No helper opens an implicit writer transaction inside an existing NORMAL transaction.

- [ ] Write failing tests `o4_schema_is_atomic_and_reopen_preserves_authority`, `o4_raw_writers_cannot_cross_adopted_and_excluded_identity`, `o4_old_writer_without_marker_cannot_adopt`, and `o4_retention_survives_every_existing_lifecycle_operation`. Assert one id/key across reopen, migration rollback on injected failure, and zero simultaneously adopted/excluded key or launch scope.
- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_schema -- --nocapture` and the named raw-writer/retention tests; retain red evidence.
- [ ] Add three append-only tables: `adoption_resolution_authority` (singleton format version 1, random canonical UUID, private cursor key); `adoption_exclusions` (stable id, full serialized O2/O3 identity, operation, nullable input target, launch five-field scope, committed timestamp); `adoption_resolution_entries` (`INTEGER PRIMARY KEY AUTOINCREMENT`, unique source-kind/source-id, immutable selector projections). Backfill launch adoptions and historical reservations in deterministic adoption-id order, without updating O3 rows. AFTER INSERT triggers append future launch adoptions/reservations/exclusions to the unified index; input rows are not discovery items.
- [ ] Install UPDATE/DELETE/REPLACE guards, per-key uniqueness, and bidirectional BEFORE INSERT guards across O3 adoptions and exclusions, including nullability and all five launch scope columns. Preserve O3's existing triggers and `recursive_triggers`. Enforce raw insert integrity with strict serialized identity readback; missing/dangling/corrupt evidence returns static Store, never absence. Extend sacrifice preflight and foreign-key retention to input exclusions and referenced sessions; no fence-release/prune helper exists.
- [ ] Register a connection-local zero-argument SQL function `coven_o4_writer_format()` returning integer 1, using existing Rusqlite functions support. O4 protected-write triggers call it; an older/unregistered connection fails closed rather than ignoring the new tables. Register before schema initialization and every subsequent writable open. Test real pre-O4 binary launch/input against a disposable migrated store in Task 8, plus raw connections and existing connections opened before migration. This is a format discriminator, not an authorization verifier; same-user malicious database access remains outside the trust claim.
- [ ] Run O4 initialization as a separate `store::durable_transaction` after the existing main schema transaction has committed. Never upgrade durability inside the old NORMAL transaction. Authority/exclusion/index initialization and backfill commit atomically; partial O4 metadata or unknown format errors rather than regenerating identity. Repeat `store::durable_barrier` before replaying an acknowledgement that may previously have lost its post-COMMIT directory barrier. Test WAL recovery and injected directory-sync failure.
- [ ] Re-run focused schema/raw-writer/retention tests plus existing `store::tests::adopted_` and schema initialization regressions. Commit `feat(psyche): retain O4 authority and exclusion evidence`. Do not advertise O4 until all later tasks pass.

### Task 3: Read-only lookup and serialized durable return-or-fence

**Files:** Modify `adoption_resolution/store.rs`, `adoption_gate.rs`, and `adoption_resolution/tests.rs`.

**Interfaces:** Produce `lookup(coven_home: &Path, request: &ResolutionRequest) -> Result<ResolutionData, ResolutionError>` and `return_or_fence(coven_home: &Path, request: &ResolutionRequest, deadline: Instant) -> Result<ResolutionData, ResolutionError>`. Add `AdoptionGate::acquire_until(coven_home: &Path, key: &str, scope: Option<&[&str]>, deadline: Instant) -> Result<AdoptionGate, GateAcquireError>` with `Busy`/static I/O classes. Existing O3 `acquire` stays source-compatible.

- [ ] Write failing tests `o4_lookup_absent_request_is_unknown_and_creates_no_files`, `o4_fence_races_admission_in_both_orders`, `o4_fence_retry_after_lost_commit_reply_is_identical`, and `o4_adopted_return_does_not_claim_runtime_containment`.

```rust
assert_eq!(lookup_unknown.disposition, Disposition::Unknown); // initialized authority, absent request
assert_eq!(lookup_on_missing_store, Err(ResolutionError::AuthorityLost));
assert_eq!(filesystem_after, filesystem_before);
assert_eq!(retry.fence, committed_fence.fence);
assert_eq!(runtime_launch_count, 0); // reconciliation itself invokes nothing
assert!(!(adopted_key_exists && excluded_key_exists));
assert_eq!(adopted_before_runtime.disposition, Disposition::Adopted);
```

- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_lookup -- --nocapture` and each named race/retry test; retain expected failures.
- [ ] Implement lookup using `store::open_existing_store_read_only`, never `open_store` or `AdoptionGate`. Missing healthy authority cannot settle an old pinned request: return AuthorityLost, while healthy known authority with absent request returns Unknown. In one read transaction, validate expected authority, global key and launch scope, exact O2/O3 identity, target relation if needed, and retained status/evidence. Historical incomplete reservation returns Unknown denial; a different keyed scope is Conflict. Existing adoption/fence readback precedes mutable familiar/filesystem/harness/liveness checks.
- [ ] Implement bounded lock acquisition with `try_lock_exclusive` over the exact existing sorted hash paths. Do not unlink lock files. Bound the whole lock/store operation by the supplied request deadline and temporary SQLite busy timeout from its remaining budget; restore connection settings afterward. Test readiness-signalled contention and an already-expired deadline; do not use scheduler-sensitive elapsed assertions.
- [ ] For return-or-fence acquire shared O3 key/scope gates before the writer transaction. Inside `durable_transaction`, recheck authority and exact retained state. Return adoption first, reject key/scope conflicts, replay exact fence (with retry barrier), otherwise validate input target and insert exact exclusion/index evidence. Commit before returning proven-not-adopted. Map interrupted lock/store/commit paths to Busy/Store without asserted absence. No runtime call or automatic mutation retry occurs.
- [ ] Exercise launch/input races in separate processes with channels/files for readiness: admission before fence; fence before delayed admission; commit before reply loss; same key/different operation/digest/session/binding; different key/same launch attempt; historical reservation; nonexistent/unbound/mismatched input session; damaged retained metadata. Join every producer before final negative assertions. Require tests pass, then commit `feat(psyche): resolve or fence exact adopted requests`.

### Task 4: Final admission checks and dedicated owner-gated routes

**Files:** Create `adoption_resolution/api.rs`; modify `api.rs`, `request_authority.rs`, `store.rs`, `adoption_resolution/tests.rs`.

**Interfaces:** Add `RequestAuthority::allows_adoption_resolution(self) -> bool`. Produce `adoption_resolution::api::dispatch(method: &str, route: &str, home: &Path, raw_body: Option<&str>, runtime: &dyn SessionRuntime, authority: RequestAuthority, deadline: Instant) -> Option<anyhow::Result<ApiResponse>>`. Add an internal `expected_authority_id: Option<&str>` argument to `launch_adopted_session` and `record_adopted_input` and their final transaction helpers; existing O3 callers pass None, O4 wrappers pass only a validated Some. Preserve existing public request handling signatures by constructing an O4 deadline at request entry from the existing owner IPC budget, `daemon::SOCKET_IO_TIMEOUT` (60 seconds); do not increase TCP/socket/body/lifecycle budgets.

- [ ] Write failing tests `o4_routes_require_owner_before_shape_or_store`, `o4_launch_and_input_pin_authority_in_final_transaction`, `o4_legacy_o3_admission_honors_fences`, and `o4_predecessor_daemon_rejects_new_execution_routes`.

```rust
assert_eq!(tcp_denial.code, "forbidden");
assert_eq!((store_probes, sessions_created, inputs_sent), (0, 0, 0));
assert_eq!(replacement_denial.code, "adoption_resolution_authority_lost");
assert_eq!(mutation_against_exact_fence.code, "request_adoption_fenced");
assert_eq!(pre_o4_new_route_status, 404);
assert_eq!(pre_o4_runtime_calls, 0);
```

- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_routes -- --nocapture` plus the named final-admission/legacy tests. Confirm red before implementation.
- [ ] Dispatch only the five exact approved `/api/v1/adoption-resolution/` paths: `lookup`, `return-or-fence`, `query`, `adopted-sessions`, and `sessions/:id/adopted-input`. The first three return 200 with their closed evidence envelopes; execution routes reuse O3 201/200 launch and 202/200 input responses. Require owner IPC before parsing/probing. Task 1's `parse_unique_value` runs before any normal `Value` can erase duplicate fields; recovery routes then use their closed O4 parsers, while execution routes retain their existing O3 payload shape plus the required pin. Validate `expectedAuthorityId` on the dedicated execution payload, then remove only that outer field before passing ordinary launch/input content to shared O3 parsing. Carry the validated pin separately into every read and final transaction, never into runtime/event payloads.
- [ ] Extend existing O3 launch/input resolvers to detect exclusions on the optimistic read, under the shared gate, and at final write. Add explicit typed Fenced/AuthorityLost results rather than interpreting arbitrary SQLite error strings. Recheck authority inside the final launch transaction and input lease/adoption transaction; store triggers independently enforce exclusion even if a helper is bypassed. Preserve O3 exact replay/conflict precedence, parent correlation, capacity/handoff checks, lifecycle publication and input-event uniqueness. No O4 metadata reaches runtime arguments, input data, capacity accounting, or persisted events.
- [ ] Keep lookup/return-or-fence/query error precedence exactly as design §8. New execution routes retain normal O2 admission expiry and per-operation policy checks after retained replay resolution. Assert transport/auth loss sends no runtime call and no local substitute. Test static redaction of entire error chains with sentinel hidden values and malformed store rows; a submitted mismatch never returns another session's retained evidence.
- [ ] Re-run focused O4 API tests and existing `api::tests::adopted_`, execution binding, handoff and event-correlation tests. Commit `feat(psyche): enforce O4 authority and fences at admission`.

### Task 5: Bounded immutable-binding discovery and cursor integrity

**Files:** Create `adoption_resolution/cursor.rs`; modify `adoption_resolution/store.rs`, `adoption_resolution/api.rs`, `adoption_resolution/tests.rs`.

**Interfaces:** Produce `QueryPage { contract: String, authority_id: String, items: Vec<QueryItem>, next_cursor: Option<String> }`. `QueryItem` contains exactly `executionBinding`, nullable `requestAdoption`, `disposition`, nullable `adoption`, and nullable `fence`; evidence members reuse Task 1 types. Historical reservations have no requestAdoption, disposition unknown and no invented fence. Add `query(conn: &Connection, request: &QueryRequest) -> Result<QueryPage, ResolutionError>`, `encode_cursor(state: &AuthorityState, selector: &BindingSelector, high_water: i64, after: i64) -> Result<String, ResolutionError>`, and `decode_cursor(state: &AuthorityState, selector: &BindingSelector, encoded: &str) -> Result<(i64, i64), ResolutionError>`.

- [ ] Write failing tests `o4_query_partitions_principal_project_graph_and_parent`, `o4_query_pages_append_only_high_water`, `o4_query_rejects_modified_cross_selector_and_cross_authority_cursor`, and `o4_query_historical_reservations_remain_unknown`.
- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_query -- --nocapture`; retain expected red results.
- [ ] Select only launch entries with exact required principal/project/graph equality; null parent selects all in that scope and a UUID selects exact stored `parent.sessionId`. Use bound SQL parameters and the index sequence, limit 1–100 plus one look-ahead, first-page maximum sequence, and strictly increasing continuation. Read item evidence and current session status in the page's transaction. Input/external/unbound rows never become child discovery. An empty page cannot prove non-adoption. New entries above the high-water require a fresh scan.
- [ ] Encode a closed internal cursor payload `(version=1, authority UUID, SHA-256 of deterministic selector bytes, high-water, last sequence)` with a fresh random 96-bit nonce and existing ChaCha20Poly1305. Associated data is the fixed literal `coven:psyche:adoption-resolution:cursor:v1`; key is the private 32-byte authority key from Task 2. Base64 URL-safe/no-padding framing carries version, nonce and ciphertext. Reject wire cursors over 2048 bytes before decoding and plaintext over 1024 bytes, unknown version, failed authentication, unknown fields, negative/out-of-order bounds, selector mismatch or authority mismatch. No cursor content enters errors/logs; no new crypto dependency or hand-rolled MAC is needed.
- [ ] Assert no duplicate or omitted entries for a fixed high-water, deterministic migrated ordering across reopen, append during pagination, status change between pages, and a new first-page scan seeing appended rows. Test another principal/project with the same graph string, exact root versus child filters, malformed stored parent/delegation, and receipt retention after attempted sacrifice/prune. Query returns correlation only, never an authoritative coordinator-liveness judgement.
- [ ] Re-run focused query tests; commit `feat(psyche): query retained O4 bindings with bounded cursors`.

### Task 6: Startup readiness and non-mutating capability negotiation

**Files:** Modify `api_health.rs`, `api.rs`, `daemon.rs`, `adoption_resolution/store.rs`, and `adoption_resolution/tests.rs`.

**Interfaces:** Produce `ResolutionHealth { authority_id: String, contract: &'static str, retention: &'static str }` with retention exactly `indefinite`. Add `SessionRuntime::adoption_resolution_health(&self) -> Option<ResolutionHealth>` defaulting to None; implement for `LiveSessionRuntime` from startup's successfully initialized/validated O4 state. Add health capability fields `adoption_resolution_contracts: Vec<String>`, `adoption_resolution_authority_id: Option<String>`, and `adoption_resolution_retention: Option<String>`, with additive defaults and the approved camelCase wire names. Never include the private cursor key.

- [ ] Write failing tests `o4_health_requires_ready_owner_runtime`, `o4_health_creates_no_store_or_lock_files`, and `o4_health_does_not_touch_read_only_initialized_store`. Preserve existing health mutation-counter and one-live-writer-snapshot assertions.
- [ ] Run `cargo test -p coven-cli --bin coven adoption_resolution::tests::o4_health -- --nocapture` and `cargo test -p coven-cli --bin coven health_ -- --nocapture`; retain red/green distinction.
- [ ] Load/validate O4 state once during normal daemon store startup before installing the live runtime readiness snapshot. Failure cannot leave O4 advertised. Plain/static health, empty home, TCP authority, missing readiness, unsupported schema or continuity quarantine yields no usable O4 tuple. The health route itself performs no initialization, migration, cache-writing or lock creation. It may expose only an already ready owner-IPC snapshot; actual request transactions always revalidate durable authority and never trust the snapshot as proof.
- [ ] Verify exact apiVersion, boolean ok and O2/O3/O4 membership plus canonical authority UUID and indefinite retention. Test advertised-but-denied access as a distinct case: capability presence grants no authority and produces no session/input/surface fallback. Missing readiness in fixtures must remain fail-closed, rather than changing every test double to advertise success.
- [ ] Re-run focused and existing health/startup tests, including missing directory, read-only home, initialization failure and post-health replacement; commit `feat(psyche): advertise only ready O4 authority state`.

### Task 7: Thin socket client methods and durable-pin ordering

**Files:** Create `packages/openclaw-coven/src/adoption-resolution.ts`; modify `client.ts`, `client.test.ts`, `compat.test.ts`, and `runtime.test.ts`.

**Interfaces:** Export closed O4 request/result/query types mirroring Tasks 1/5. Add client methods `negotiateAdoptionResolution(persistPin: (authorityId: string) => Promise<void>, signal?: AbortSignal): Promise<CovenResolutionPin>`, `launchResolvedSession(input: LaunchAdoptedCovenSessionInput, pin: CovenResolutionPin, signal?: AbortSignal): Promise<CovenSessionRecord>`, `sendResolvedInput(sessionId: string, data: string, binding: CovenExecutionBinding, adoption: CovenRequestAdoption, pin: CovenResolutionPin, signal?: AbortSignal): Promise<CovenAdoptionResult>`, `lookupAdoption(request: CovenResolutionRequest, signal?: AbortSignal): Promise<CovenResolutionData>`, `returnOrFence(request: CovenResolutionRequest, signal?: AbortSignal): Promise<CovenResolutionData>`, and `queryAdoptions(request: CovenResolutionQuery, signal?: AbortSignal): Promise<CovenResolutionPage>`. `CovenResolutionPin` carries the exact id and contract/retention tuple; callers must recover it from their durable intent record for outstanding requests.

- [ ] Write failing tests `resolution_pin_persistence_completes_before_any_mutation`, `resolution_pin_failure_sends_zero_posts`, `resolution_uses_only_dedicated_routes_after_health_replacement`, and `resolution_responses_reject_extra_wrong_type_or_inconsistent_members`. Use a deferred Promise for pin persistence and exact socket request counters.

```typescript
expect(posts).toEqual([]); // persistence callback still pending
persisted.resolve();
await pinPromise;
expect(pin.authorityId).toBe(negotiatedAuthorityId);
expect(sent.path).toBe('/api/v1/adoption-resolution/adopted-sessions');
expect(sent.body.expectedAuthorityId).toBe(negotiatedAuthorityId);
expect(legacyFallbackCalls).toBe(0);
```

- [ ] Run `pnpm --dir packages/openclaw-coven exec vitest run src/client.test.ts src/compat.test.ts`; retain the first failure. Confirm actual package scripts before execution; the implementation must not silently skip a file because a runner flag differs.
- [ ] Implement types/strict validators and socket methods only. Preserve existing owner/permission/socket-fingerprint checks, request size/time limits, abort behavior, O2/O3 methods, and error normalization. Negotiation checks exact API version first, exact boolean ok next, then exact O2/O3/O4 arrays, canonical id and indefinite retention; await the caller's durable persistence callback before returning a pin. A pending/rejected callback sends no POST. A caller that lost the original pre-dispatch pin cannot adopt a newly observed id as old-request recovery authority.
- [ ] Require an explicit pin on O4 original mutations; never overwrite it from later health. Lookup, return-or-fence and query carry their saved expected id and validate returned authority equality. Reject inconsistent adopted/fence/unknown union combinations, duplicate members if raw response parsing supports it, wrong status types, echoed hidden values in errors, or unsupported retention. Malformed health/response, 404, authority loss, disconnect and abort never call an O3 fallback.
- [ ] Update only typed mocks that need the additive interface. Keep O4-less fixtures O4-less to prove compatibility. Add sentinel error-redaction and cross-selector cursor tests without reimplementing Coven storage policy in TypeScript. Run package build/typecheck and complete package tests; commit `feat(psyche): expose explicit O4 socket client methods`.

### Task 8: Real isolated daemon fault matrix, documentation, and bounded delivery

**Files:** Create `crates/coven-cli/tests/adoption_resolution.rs`; modify `docs/API-CONTRACT.md`, `docs/SESSION-LIFECYCLE.md`, `scripts/check-api-contract-docs.py`, `scripts/check-api-contract-docs-test.py`, `specs/psyche/O4_CONTRACT_DESIGN.md`, and `specs/psyche/PLAN.md` only for the delivered O4 contract/evidence.

**Interfaces:** Consume the public five routes, health tuple, exact O2/O3 values and real built daemon over owner IPC. Test-only fixture harness observes runtime invocation without accepting fake authority. Use helper `run_o4_fault_case(case: FaultBoundary) -> anyhow::Result<FaultReceipt>` in the new integration file; receipts retain source/artifact/report digests, negotiated authority tuple, observed durable rows, runtime/event counts, and blocked/unknown disposition. Fault hooks must be test-only and must not mint accepting production decisions.

- [ ] Add failing subprocess cases for `BeforeAdoptionCommit`, `AfterAdoptionCommitBeforeRuntime`, `DuringInputDelivery`, `BeforeFenceCommit`, and `AfterFenceCommitBeforeReply`, each with both kill and stall. Children signal the exact boundary through a readiness channel/file and announce early error before the parent waits. Reopen persisted state in a separate process; do not replace this with one in-memory connection or sleep-selected timing.
- [ ] Run `cargo test -p coven-cli --test adoption_resolution -- --nocapture`; retain the red baseline and each first fault result. Genuine timeout tests use explicit injected blocked operations and assert failure class; tests must not loosen production deadlines.
- [ ] Add the exact authority replacement sequence: health A; durably save A's id; original launch/input sent to B with lost response; recovery in A. B must reject before adoption if O4, or return unknown-route if pre-O4. Assert no B runtime/event/adoption, no false safe replacement, and no fallback. Also test no original pin, unexpected id, copied-id restore/fork quarantine, authority metadata corruption, retention pressure, stale cursor and fence conflicts after reopen.
- [ ] Build or use the exact pre-O4 baseline binary in a separate external target directory against a disposable migrated store. Prove new writes fail the format guard and cannot bypass exclusions. Separately hold an old request after its pre-migration adoption commit: discovery/lookup must return retained adoption, and neither migration nor a new gate permits a containment or replacement claim. No old-runtime overlap or restored writable clone is accepted as a supported deployment; document the operator quarantine/reconciliation requirement and preserve blocked routes. Raw trigger tests alone are not an old-binary compatibility receipt.
- [ ] Cover root and child discovery after each daemon restart; page all retained child adoptions and unknown historical reservations while Psyche coordinator state is unavailable. Compare the real and fake boundary's unchanged assertions for O4 rows without claiming the complete Psyche G4/G6 suite. Record any absent Psyche adapter integration as a remaining gate, not a skipped conformance pass. No rebind action or production child dispatch is introduced.
- [ ] Publish the exact routes, closed request/response examples, capability/retention tuple, all static §8 errors, admission-only fence limitation, legacy pin limitation, durability retry behavior and unsupported restore/downgrade operations in `docs/API-CONTRACT.md`. Remove only delivered O4 exclusions; retain O5 cancellation, O6 artifacts, O7 complete recovery, full O8 profile and production-dispatch limits. Extend the source/document checker with positive and missing-field/incorrect-name negative cases. Avoid editing migrated public-pointer pages solely to duplicate the normative contract.
- [ ] Run the complete local gates below, then exact-head protected CI. Retain first failures/retries/ignores. After reviewed implementation merge, fetch main, verify the delivered tree and immutable contract/artifact tuple, and only then change O4 status to implemented in design/PLAN and check the actual #1629 criteria. Design/plan acceptance remains distinct from runtime acceptance and #1230 completion. Commit `docs(psyche): record bounded O4 contract and verification` only when that evidence exists.

## Required verification and delivery commands

Run from the final staged implementation worktree with external `CARGO_TARGET_DIR`, correct toolchains and no other session sharing its store. These commands are requirements for execution of the plan; their presence is not an assertion they have run. Use existing repository `AGENTS.md` for the full Policy guard loop without omissions.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python3 scripts/check-secrets.py
python3 scripts/check-coven-privacy.py --staged
python3 scripts/check-docs-ownership.py --range 'origin/main...HEAD'
python3 scripts/check-coven-privacy.py --range 'origin/main...HEAD'
python3 scripts/check-api-contract-docs.py
python3 scripts/check-api-contract-docs-test.py
npm run build
npm test
```

The npm commands above run in `packages/openclaw-coven` when implementing Task 7; inspect its actual scripts and also run package typecheck. The existing package uses pnpm for its normal local loop, so `pnpm --dir packages/openclaw-coven build` and `pnpm --dir packages/openclaw-coven test` are the equivalent repository-local package invocations after dependency setup. Do not run missing root scripts and describe them as coverage.

Reproduce every command in the AGENTS.md Policy guard block using Bash and Python 3.11+; require its actual trailing `Policy guard (local): 0`, not a clean tail from an earlier success. Run revision-range guards after commit because staged checks do not inspect committed changes. Review final serialized Markdown payloads for one logical source line per prose paragraph and inspect rendered preview where available before posting.

Use protected PRs and the required merge queue. Do not use direct pushes, administrator bypass, simulated maintainer approval, or weakened workflow checks. If the queue cannot be entered with an available authorized tool, leave the reviewed PR ready and report the specific queue action required; do not claim merge from an enqueue attempt or timeout. After merge, verify fetched-main content, release only the task claim, delete only the task's remote branch, and remove the clean merged worktree/branch under current lifecycle rules.

## Requirement coverage and acceptance audit

| Approved design requirement | Owning plan task / proof |
|---|---|
| §1–2 O3 boundary and selected approach | Tasks 3/4/8: no runtime call during recovery and no false containment after gate release. |
| §3 exact negotiation, five routes and closed values | Tasks 1/4/6/7: raw duplicate parsing, owner-only denial, exact dedicated paths, saved pin before mutation. |
| §3.3 adopted/proven-not-adopted/unknown; C-S5 | Tasks 2/3/8: retained snapshot, durable exclusion only, absent healthy read unknown, errors distinct. |
| §4 linearization and every admission path; C-S6/C-M7 | Tasks 2/3/4: same O3 locks, transaction recheck, triggers, legacy routes and new-key launch attempts. |
| §5 binding query and child discovery; C-M6 | Task 5 and Task 8: exact selectors, stable bounded cursor, page/restart cases; Psyche owns liveness. |
| §6 permanent retention, durability and continuity | Tasks 2/3/6/8: WAL/directory barrier, immutable records, old binary guard, postcommit runtime and restored clone quarantine. |
| §7 compatibility and ownership | Tasks 4/7/8: existing O2/O3/unbound regressions, no fallback, Rust authority and thin clients. |
| §8 O4-specific O8 errors | Tasks 1/4/7: exact static codes/messages/fields, hidden-value redaction, no cached substitute. |
| §9 complete primitive fault matrix | Tasks 1–8, especially actual kill/stall/reopen and exact artifact receipts. No fake-only G1b/G4/G6 assertion. |
| §10 #1230 mapping and residual owners | #1629/#1230 plus Task 8 evidence handoff; O5/O6/O7/full O8 and psyche#12 decisions stay open. |
| §11 approval sequence | Task 0: explicit plan approval and protected design delivery before code; no inferred rebind mapping. |

Self-review: every approved design section is mapped above; named types and functions are defined in the producing task before consumers reference them. The five Review Focus cases have explicit tests in their owning tasks. Test bodies are assertion sketches where fixtures already exist, not a second implementation. If any listed task cannot be implemented without changing an approved wire or authority choice, stop for a separately reviewed design update.

## Execution handoff

The plan must be reviewed as a concrete artifact before execution. No execution method has been selected. Native execution is the recommended starting point because Tasks 2–6 share storage, transaction and routing seams; independently editing those in parallel would make correctness harder to review. If the user selects subagent-driven execution instead, serialize dependent implementation tasks and use fresh review gates rather than concurrent edits to shared files. Either choice remains blocked until explicit approval of this plan revision, and neither choice grants merge/deployment/conformance authority beyond the owning issue's gates.
