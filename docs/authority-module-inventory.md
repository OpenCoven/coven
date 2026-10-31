---
title: "Authority module inventory"
summary: "Source-anchored authority concentration, extraction priorities, and compatibility gates."
source_adjacent_reason: "Tracks Rust authority ownership and extraction evidence alongside the modules it governs."
---

# Authority module inventory

Status: living inventory for [OpenCoven/coven#806](https://github.com/OpenCoven/coven/issues/806)
("reduce Coven authority-module concentration behind stable contracts"). It records
where authority-bearing responsibility is concentrated, the prioritized extraction
order, and where new route, policy, persistence, transport, and mapping changes
belong. Update it when a slice lands or the concentration picture changes.

## Method

This snapshot was generated from commit
`10a7cecf6882243eb52233a8b1463781f329cdba` on 2026-10-20. Every measurement
below uses that same immutable revision. Reproduce the ranking without changing
your checkout:

```sh
python3 - <<'PY'
import re
import subprocess

ref = "10a7cecf6882243eb52233a8b1463781f329cdba"
paths = subprocess.check_output(
    ["git", "ls-tree", "-r", "--name-only", ref, "crates"], text=True
).splitlines()
rows = []
for path in paths:
    if not path.endswith(".rs"):
        continue
    data = subprocess.check_output(["git", "show", f"{ref}:{path}"])
    lines = data.decode().splitlines()
    production = next(
        (i for i, line in enumerate(lines)
         if re.fullmatch(r"(?:pub(?:\([^)]*\))? )?mod tests \{", line)),
        len(lines),
    )
    rows.append((len(lines), production, len(data), path))
for total, production, size, path in sorted(rows, reverse=True)[:20]:
    print(f"{total:>7} / {production:<7} {size:>9} {path}")
PY
```

Remove the `[:20]` output limit to include smaller modules such as
`store/schema.rs`, which is shown beside its facade below.

"Production lines" below means lines before the terminal module-scope
`mod tests` block. It is a comparison aid, not a source-code metric: some
`#[cfg(test)]` hooks intentionally remain beside the production boundary.

Modules were then classified by responsibility: parsing, validation,
authorization/policy, domain service, persistence, process/PTY lifecycle,
transport/API, mapping/serialization, and telemetry. Priority is review risk
(dispatch fan-in, mutable coordination, externally observable contracts, and
security sensitivity), not size alone.

## Current concentration snapshot

| Module | Total / production lines | Bytes | Responsibility classes | Review and coupling evidence |
| --- | ---: | ---: | --- | --- |
| [`api.rs`](../crates/coven-cli/src/api.rs) | 50,746 / 17,400 | 1,956,423 | transport dispatch, parsing, validation, transport authority, domain orchestration, persistence access, response mapping | **Highest concentration.** `handle_request_with_runtime_and_authority` remains the central route dispatcher. Session lifecycle, Ward/Threads proposals, and Automations handlers share the module. The proposal path also coordinates a process-wide Ward audit lock, filesystem artifacts, SQLite state, recovery, and response precedence. |
| [`daemon.rs`](../crates/coven-cli/src/daemon.rs) | 17,028 / 7,136 | 651,303 | local IPC and TCP transport, daemon lifecycle, live session ownership, process supervision, recovery, scheduler startup, telemetry | **High mutable-state risk.** `LiveSessionRuntime` owns the shared live-session registry and launch/shutdown coordination; `serve_forever` acquires lifecycle authority, initializes durable state, starts recovery and schedulers, and accepts concurrent transports. In `handle_http_stream_with_lifecycle`, ordinary API requests not consumed by owner-local lifecycle or mobile local-control handling derive transport authority and enter the central API dispatch. |
| [`store.rs`](../crates/coven-cli/src/store.rs) | 12,178 / 5,138 | 446,029 | initialization facade and cache, query/command persistence, retention, storage health mapping | **High fan-out.** The production portion exposes 110 crate/public free functions. `open_store_with_initialization_observer` delegates schema work to its private `schema` module and retains the initialized connection, while session, handoff, event, Ward, hub, Automations, maintenance, and health queries remain in the parent. The process-global `initialized_store_paths` cache and `STORAGE_HEALTH_SNAPSHOTS` add ownership coupling. |
| [`store/schema.rs`](../crates/coven-cli/src/store/schema.rs) | 1,399 / 1,399 | 56,647 | schema and migration, SQLite connection configuration, compatibility and FTS backfills | **Private initialization boundary.** The parent facade calls this module without changing SQL, migration order, transaction/rollback boundaries, or startup observation order. Initialization returns the live connection without an intervening close/reopen. Successful-initialization caching and domain commands/queries remain in `store.rs`; daemon startup still owns the final close, which now runs after readiness (#1227). |
| [`automations/runner.rs`](../crates/coven-cli/src/automations/runner.rs) | 12,001 / 4,722 | 452,981 | occurrence dispatch, scheduler fencing, runtime adoption, retries, timeout/cancellation, crash recovery, terminal settlement | **High state-machine risk.** `dispatch_occurrence_with_clock` combines direct SQL, scheduler-generation checks, runtime launch, cancellation, and settlement; recovery and stop fencing share the production module. #856 has landed, but remaining #857 correctness work should settle before structural movement. |
| [`ward.rs`](../crates/coven-cli/src/ward.rs) | 10,052 / 6,115 | 371,057 | authorization/policy, path materialization, edit budgets, verified filesystem mutation, rollback/cleanup, audit evidence | **Highest security sensitivity, bounded apply engine.** `Ward::evaluate` is the read-only policy entry point. Direct `Ward::apply` and the Threads-, coherence-, and scheduled-approval families retain fail-closed evaluation and conditional mutation. Their commit-check variants preserve final authority checks; decomposition must not expose private atomic-write helpers. |
| [`pty_runner.rs`](../crates/coven-cli/src/pty_runner.rs) | 11,026 / 6,130 | 408,676 | harness command construction, PTY/piped I/O, stream decoding, platform process containment, timeout/cancellation, supervisor protocol | **High platform/security risk.** Strict child containment, guardian/supervisor protocol, stream adapters, and detached/attached execution share one module. Windows and Unix ownership guarantees converge at `spawn_strict_child_process_tree`, while cancellation also uses process-global signal coordination in `SUPERVISED_STREAM_CANCELLATION_SIGNAL` and `SUPERVISED_STREAM_CANCELLATION_LOCK`. |
| [`main.rs`](../crates/coven-cli/src/main.rs) | 9,873 / 6,236 | 363,894 | CLI parsing and command dispatch, setup, user-facing mapping | **Medium.** High fan-in but mostly presentation/entry-point code. Do not prioritize it ahead of authority-bearing modules merely for size. |
| [`memory_import.rs`](../crates/coven-cli/src/memory_import.rs) | 9,051 / 4,598 | 328,142 | untrusted external-format parsing, normalization, validation, persistence | **Medium-high input risk.** Large because it combines several importer grammars with durable writes. It is not a central authority router, but malformed and cross-format ambiguity need characterization before splitting parsers from persistence. |
| [`harness.rs`](../crates/coven-cli/src/harness.rs) | 6,610 / 2,662 | 243,410 | supported-harness policy, adapter selection, launch validation and construction | **High policy sensitivity, narrower scope.** It exposes 27 production functions. Preserve the supported Codex/Claude Code/GitHub Copilot CLI set and keep process ownership in `pty_runner.rs`/the daemon. |
| [`automations/command_adoption.rs`](../crates/coven-cli/src/automations/command_adoption.rs) | 4,510 / 2,021 | 164,895 | command adoption, attempt/executor binding, durable receipts, recovery | **High adoption sensitivity.** Keep the distinction between dispatch and durable adoption explicit; extraction must preserve command identity, binding checks, and non-replay after ambiguous execution. |
| [`automations/occurrences.rs`](../crates/coven-cli/src/automations/occurrences.rs) | 3,798 / 1,858 | 146,975 | occurrence persistence, claim transitions, deduplication, scheduler fencing | **High concurrency risk.** Claims and state transitions share SQL and fencing rules; characterize races and restart behavior before splitting storage from transitions. |
| [`automations/contract/types.rs`](../crates/coven-cli/src/automations/contract/types.rs) | 3,585 / 3,585 | 115,912 | canonical wire/domain types and validation | **Medium.** Large but cohesive contract code. Split only by stable protocol ownership, not line count. |

Large TUI modules are intentionally omitted from the authority priority. They
matter for maintainability, but they do not outrank modules that decide
permission, durable state, or process ownership.

## Responsibility and dependency map

```text
daemon transport and lifecycle
        |
        v
api route/version gate -> central route dispatch -> domain authority
                                                   |-- sessions -> SessionRuntime
                                                   |-- Ward/Threads -> Ward + files + store
                                                   |-- Automations -> runner + store + runtime
                                                   |-- travel/hub -> store
        |
        v
response envelope mapping

SessionRuntime -> daemon live-session registry -> pty_runner containment/streams
domain modules -> store command/query surface -> schema/migrations/maintenance
```

The main review risk is not a single large file. It is a small number of entry
points coordinating several authority classes at once:

- `handle_request_with_runtime_and_authority` selects the central route
  operations and is the only production API dispatch call from the daemon.
- the API's Threads proposal path combines transport authority, Ward policy,
  filesystem evidence, durable decision state, recovery, and error mapping;
- `LiveSessionRuntime` and daemon startup combine process ownership with
  transport and scheduler lifecycle;
- the store facade offers one broad command/query surface over unrelated domains
  and retains process-global health/initialization caches; its private `schema`
  module owns schema migration and connection configuration;
- `pty_runner` combines adapter construction with the platform-specific process
  containment boundary that makes cancellation and crash recovery trustworthy.

## Landed stable seams

These extractions are complete and remain the model for bounded movement:

1. **Route/version authority gate.** `ApiRoute`, `normalize_api_route`,
   `split_path_query`, and route-version constants live in `api_routes.rs`.
   Rejection envelopes and version behavior remain pinned by focused tests.
2. **Response/error envelope mapping.** `ApiResponse`, `api_error`, and
   `json_response` live in `api_response.rs`. The seam performs serialization
   only; handlers still decide status, code, message, details, and precedence.
3. **Health/capability mapping and request authority.** Transport-derived
   permission decisions live in `request_authority.rs`; health wire types and
   base capability mapping live in `api_health.rs`. Live store, hub, and event
   writer collection remains with the API orchestrator.
4. **Store initialization/schema.** #1033 moved schema, migration, and connection
   configuration into private `store::schema`. The facade retains the initialized
   connection, successful-initialization cache, and domain command/query surface.
   Daemon startup still owns the final close, now after readiness (#1227).
5. **Proposal-envelope preflight.** Private `api::proposal_preflight` owns the
   streaming structural scan and decoded-content accounting behind the existing
   `api::validate_proposal_envelope_preflight` entry point. It returns an edit
   budget, never an authorization decision. Typed proposal decoding, revision
   serialization, audit reservation, file reads, claims, recovery, and final
   policy checks remain with the coordinator. No unchecked decoding API is added.
6. **Proposal revision digest.** Private `api::proposal_revision` owns the
   canonical JSON digest behind `ProposalEnvelopeDocument::revision`: objects
   hash with sorted, re-encoded keys, arrays keep their order, and scalars hash
   as written. It exposes only `hash_canonical_json` to the coordinator and does
   no I/O. Envelope decoding, authority bytes, and every revision comparison
   remain with the coordinator. `tests/proposal_revision_boundary.rs` rejects
   new dependencies, state, and exposed helpers.
7. **Persisted proposal envelope.** Private `api::proposal_document` owns the
   typed envelope: shape-based legacy/scheduled authority decoding, the stored
   forms, the envelope byte bound on serialization, and the revision over the
   authority bytes (decision progress excluded). It exposes the document, its
   authority enum, and six methods to the coordinator. Reading files, claims,
   recovery, policy, and every revision comparison remain with the coordinator.
   `tests/proposal_document_boundary.rs` rejects new dependencies, wider
   visibility, and any exposed wire or stored form.
8. **Proposal scheduler batch selection.** The durable scheduler cursor and the
   bounded, wrap-around candidate batch live in `proposal_store.rs` beside the
   other pending-directory persistence. They choose which proposals a pass
   visits and in what order; every decision about a candidate, quarantine
   logging, and cursor advancement policy remain with the coordinator.

These seams preserve the public HTTP API, daemon socket protocol, and existing
caller entry points. They do not complete #806's broader highest-risk module
decomposition or replace characterization for the next selected boundary.

## Prioritized bounded extractions

Each movement needs positive and negative characterization before code moves.
The ranking accounts for current dependency blockers as well as inherent risk.

| Priority | Boundary | Characterization and stable contract | Must not change | Readiness |
| ---: | --- | --- | --- | --- |
| 1 | **Threads proposal coordinator out of `api.rs`** | Existing proposal/Ward tests in `api.rs`, `threads_gate.rs`, the public `/threads/proposals` API, Ward audit records, and the real-daemon work in #884 | transport-owner gate, decision/error precedence, lock ordering, audit reservation, filesystem recovery, fail-closed uncertainty | **Engineering prerequisites accepted.** #884 and #885–#888 are closed with finite engineering dossiers; see reconciliation below. Start with envelope preflight characterization, then move coordinator responsibilities in separate slices. Live Cave, reliability, and human freeze acceptance remain separate. |
| 2 | **Session route family out of `api.rs`** | `SessionRuntime`, request-adoption and execution-binding tests, API contract docs, session lifecycle integration tests, Windows daemon lifecycle tests | launch/input/kill adoption semantics, event ordering, status/error payloads, lock release before runtime calls, crash/restart behavior | **Harness available.** #884 is closed. Retain its feature-enabled real-daemon targets and add session-specific adoption/error/restart characterization before moving the family. |
| 3 | **Store initialization/schema from runtime commands and queries** | `open_store`, `initialize_store`, `open_initialized_store`, migration/compatibility tests, store health and smoke tests | schema and migration order, transaction boundaries, per-request no-DDL path, function signatures, and policy remaining outside persistence | **Landed in #1033.** Private `store::schema` owns initialization behind the existing facade. Domain-query decomposition and the broader #806 acceptance criteria remain separate work. |
| 4 | **Daemon transport accept loops from live-session supervision** | daemon inline tests, Unix/TCP request tests, `windows_daemon_lifecycle.rs`, stop/restart budget and recovery tests | single-writer lifecycle locks, owner-derived request authority, bounded in-flight handling, shutdown cleanup, live-session registry semantics | Ready only as separate platform-complete slices; Unix and Windows must keep equivalent outer behavior. |
| 5 | **Process containment/supervisor from adapter and stream code in `pty_runner.rs`** | strict-containment tests, native Windows lifecycle tests, piped/PTY integration tests, timeout/cancellation tests | before-first-instruction ownership, kill-on-close/process-group guarantees, receipt protocol, terminal callback ordering | Characterize the supervisor protocol as one unit before moving it. Never split Unix and Windows into contracts that can drift. |
| 6 | **Ward pure classification/budget code from verified apply engine** | Ward unit tests, direct/proposal API tests, hostile replacement and rollback tests | the bounded set of direct, Threads-, coherence-, and scheduled-approval Ward entry points; conditional mutation and proven rollback; path confinement; audit evidence; cleanup uncertainty | Defer until #924's platform guarantee is resolved. Pure helpers may move; private atomic-write helpers must not become callable outside the Ward engine. |
| 7 | **Automations runner lifecycle sub-boundaries** | #856/#857 state-machine, crash, fencing, cancellation, authority, and receipt tests | adoption uncertainty, scheduler fences, retry/cancel semantics, terminal evidence, Rust authority ownership | #856 has landed; defer structural work until the remaining #857 correctness scope settles. |

Travel, scheduler, AFS, and cockpit route groups are easier to move but rank
below these boundaries because they do not currently dominate authority review
risk. They are suitable fallback slices only when a higher-risk boundary is
blocked and the move does not create a second policy path.

## Correctness dossier reconciliation (2026-10-20)

At source `10a7cecf6882243eb52233a8b1463781f329cdba`, the prior blanket
Threads dependency hold is stale. The accepted scope is finite:

- [#884 closing review](https://github.com/OpenCoven/coven/issues/884#issuecomment-6000558436)
  accepts the isolated real-daemon harness, deterministic clock, typed closes,
  restart coverage, and pinned compatibility mechanism. Run all four Threads
  integration targets with `--features threads-test-clock`; workspace tests
  alone intentionally omit these targets.
- [#885](https://github.com/OpenCoven/coven/issues/885) accepts identity binding
  for enumerated current intake/replay routes at `226bfcc8`, not future APIs.
- [#886 disposition](https://github.com/OpenCoven/coven/issues/886#issuecomment-5719691181)
  accepts the five terminal families and the inspected deployed profile's empty
  audit census. This is not evidence about arbitrary deployments or permission
  to repair ambiguous history.
- [#887 acceptance](https://github.com/OpenCoven/coven/issues/887#issuecomment-5656608527)
  accepts the nine-route protected-intake matrix; separate protected writes stay
  fail-closed and source forwarding evidence is not live Cave acceptance.
- [#888 acceptance](https://github.com/OpenCoven/coven/issues/888#issuecomment-5656608528)
  accepts seven retired-corpus scenarios, contention, and ordered typed closes.
- #924 remains open for final-delete identity guarantees; #857 remains open
  for automation dispatch/receipt authority. Do not move those unsettled engines.
  Current #1436 startup instrumentation and #857 PRs also require coordination.

These historical receipts establish which contracts were accepted. Every new
extraction still needs fresh exact-source tests and supported-platform CI.
They do not discharge Threads' long-run reliability, live Cave acceptance, or
human coherence/freeze gates. #806 remains open for the full decomposition.

## Proposal preflight extraction contract

The first post-refresh Threads slice removes the structural scanner, preflight
wire readers, and content accounting from the API coordinator without changing
any production expression. Against the snapshot above, `api.rs` drops from
50,746 to 49,284 lines; the new pure module contains 1,221 lines. This is one
parsing responsibility, not completion of the Threads coordinator decomposition.

Focused tests pin structural errors before content decoding, duplicate-field
rejection, validation of shadowed legacy edits, nested-before-legacy budget
errors independent of JSON field order, UTF-8/base64 accounting, overflow,
cardinality, nesting, and exact content limits. Existing API tests continue to
prove preflight precedes target access and Ward adjudication. Removing structural
scanning or shadowed-field validation was independently detected by the new
characterization before extraction.

`tests/proposal_preflight_boundary.rs` parses the Rust AST and rejects new
internal dependencies, I/O imports, exposed unchecked helpers, and production
submodules. Its negative fixtures exercise the guard itself. This is a drift
check, not a Rust sandbox. Production helpers stay private and the original
crate-visible entry point is a re-export of the single implementation.

No persistence, serialization, process, or protocol behavior is changed. The
four feature-enabled Threads integration targets remain required to verify the
outer authority/restart paths. Rollback restores the original module placement
and callers together; it requires no data migration and must retain all accepted
identity and fail-closed policy fixes. Platform execution evidence belongs in
the delivering PR; historical dossier acceptance does not substitute for it.

## Where new behavior belongs

- **Route/version policy** (new route arm, version bump, gate rejection):
  `api_routes.rs` for the gate and the `api.rs` dispatch for the arm. Never use
  a helper that bypasses the gate.
- **Proposal-envelope syntax and resource validation**: private
  `api::proposal_preflight`; keep authority decisions and I/O outside it.
- **Persisted proposal envelope fields and stored forms**: private
  `api::proposal_document`. A new field changes the persisted format and, if it
  joins the authority bytes, every revision.
- **Proposal revision canonicalization**: private `api::proposal_revision`.
  Changing the digest re-keys every stored proposal and recorded decision
  request, so it is a behavior change and never part of a structural move.
- **Validation and domain policy**: the module that owns the domain
  (`ward.rs`, `threads_gate.rs`, `harness.rs`, `session_launch.rs`) rather than
  inline transport code where a characterized seam exists.
- **Persistence**: store command/query modules behind the existing store API;
  schema changes stay in the initialization/migration boundary. Policy
  decisions never move into persistence.
- **Executor/process lifecycle**: `pty_runner.rs` and daemon supervision, never
  API formatting.
- **Request transport authority**: `request_authority.rs`; permission derives
  from the daemon-selected transport, never caller payloads.
- **Health capability contract**: `api_health.rs`; capability advertisement and
  wire types remain independent of live store, hub, and writer collection.
- **Response/error envelope mapping**: `api_response.rs`; mapping remains pure
  (no I/O or policy).

New cross-cutting responsibilities must justify staying inside a concentrated
module rather than joining the relevant stable contract.

## Extraction guardrails

Every decomposition PR must state and prove:

1. the externally observable contracts and invariants it preserves;
2. malformed, unauthorized, ambiguous-state, and recovery characterization;
3. that lower-level helpers cannot bypass validation or authority;
4. the smallest focused tests plus the full required repository/platform gates;
5. a rollback that restores the previous module boundary without data changes.

Behavior changes belong in separate, approved fixes. A structural PR must not
change route paths/versions, status or error payloads, SQLite schema, socket
protocol, CLI flags, process-containment guarantees, or Ward fail-closed
behavior.

## Metrics to track

Track trends rather than enforcing vanity line limits:

- total and production lines/bytes for selected authority modules;
- responsibility classes coordinated by each entry point;
- production functions exposed across module boundaries;
- review diff size for security-sensitive changes;
- externally observable contracts covered by focused tests;
- escaped defects caused by cross-responsibility interactions.
