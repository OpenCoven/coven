# Authority module inventory

Status: living inventory for [OpenCoven/coven#806](https://github.com/OpenCoven/coven/issues/806)
("reduce Coven authority-module concentration behind stable contracts"). It records
where authority-bearing responsibility is concentrated, the prioritized extraction
order, and where new route, policy, persistence, transport, and mapping changes
belong. Update it when a slice lands or the concentration picture changes.

## Method

This snapshot was generated from commit `380e765` on 2026-09-09. Rust modules
were ranked by tracked-file lines and bytes:

```sh
for file in $(git ls-files 'crates/**/*.rs'); do
  wc -lc "$file"
done
```

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
| [`api.rs`](../crates/coven-cli/src/api.rs) | 35,038 / 12,463 | 1,319,664 | transport dispatch, parsing, validation, transport authority, domain orchestration, persistence access, response mapping | **Highest concentration.** The 523-line central dispatch has 74 direct method arms plus guarded path arms ([lines 695-1217](../crates/coven-cli/src/api.rs#L695-L1217)). Session lifecycle occupies roughly lines 2173-5240; Ward and Threads proposal lifecycle occupies roughly lines 5240-12198. The latter also coordinates a process-wide Ward audit lock, filesystem artifacts, SQLite state, recovery, and response precedence. |
| [`store.rs`](../crates/coven-cli/src/store.rs) | 12,890 / 6,146 | 476,081 | schema and migration, query/command persistence, retention, storage health mapping | **High fan-out.** The production portion exposes 106 crate/public functions. Initialization and the full schema start at [`initialize_store`](../crates/coven-cli/src/store.rs#L577) while session, handoff, event, Ward, hub, Automations, maintenance, and health queries remain in the same module. Process-global initialized-path and health-snapshot caches add ownership coupling ([lines 249-252](../crates/coven-cli/src/store.rs#L249-L252), [521-522](../crates/coven-cli/src/store.rs#L521-L522)). |
| [`daemon.rs`](../crates/coven-cli/src/daemon.rs) | 12,366 / 5,423 | 462,079 | local IPC and TCP transport, daemon lifecycle, live session ownership, process supervision, recovery, scheduler startup, telemetry | **High mutable-state risk.** `LiveSessionRuntime` owns the shared live-session registry and launch/shutdown coordination ([lines 210-217](../crates/coven-cli/src/daemon.rs#L210-L217)); `serve_forever` acquires lifecycle authority, initializes durable state, starts recovery and schedulers, and accepts concurrent transports ([lines 4307-4540](../crates/coven-cli/src/daemon.rs#L4307-L4540)). In `handle_http_stream_with_lifecycle`, ordinary API requests not consumed by owner-local lifecycle or mobile local-control handling derive transport authority and enter the central API dispatch ([lines 4694-4800](../crates/coven-cli/src/daemon.rs#L4694-L4800)). |
| [`pty_runner.rs`](../crates/coven-cli/src/pty_runner.rs) | 9,925 / 5,522 | 366,689 | harness command construction, PTY/piped I/O, stream decoding, platform process containment, timeout/cancellation, supervisor protocol | **High platform/security risk.** Strict child containment, guardian/supervisor protocol, stream adapters, and detached/attached execution share one module. Windows and Unix ownership guarantees converge at [`spawn_strict_child_process_tree`](../crates/coven-cli/src/pty_runner.rs#L1865), while cancellation also uses process-global signal coordination ([lines 838-840](../crates/coven-cli/src/pty_runner.rs#L838-L840)). |
| [`memory_import.rs`](../crates/coven-cli/src/memory_import.rs) | 8,914 / 4,598 | 322,653 | untrusted external-format parsing, normalization, validation, persistence | **Medium-high input risk.** Large because it combines several importer grammars with durable writes. It is not a central authority router, but malformed and cross-format ambiguity need characterization before splitting parsers from persistence. |
| [`main.rs`](../crates/coven-cli/src/main.rs) | 8,551 / 5,270 | 314,483 | CLI parsing and command dispatch, setup, user-facing mapping | **Medium.** High fan-in but mostly presentation/entry-point code. Do not prioritize it ahead of authority-bearing modules merely for size. |
| [`ward.rs`](../crates/coven-cli/src/ward.rs) | 8,367 / 4,888 | 309,928 | authorization/policy, path materialization, edit budgets, verified filesystem mutation, rollback/cleanup, audit evidence | **Highest security sensitivity, bounded apply engine.** `Ward::evaluate` is the read-only policy entry point. Direct `Ward::apply` and the two policy-specific approved-apply entry points each re-run fail-closed evaluation before reaching private atomic-write helpers ([lines 1090-1425](../crates/coven-cli/src/ward.rs#L1090-L1425)). The file has grown around hostile filesystem races and rollback evidence; decomposition must not expose those lower-level write helpers. |
| [`automations/runner.rs`](../crates/coven-cli/src/automations/runner.rs) | 7,756 / 3,457 | 285,163 | occurrence dispatch, scheduler fencing, runtime adoption, retries, timeout/cancellation, crash recovery, terminal settlement | **High state-machine risk.** Dispatch combines direct SQL, scheduler-generation checks, runtime launch, cancellation, and settlement ([lines 1724-2145](../crates/coven-cli/src/automations/runner.rs#L1724-L2145)); recovery and stop fencing continue through the rest of the production module. Active hardening under #856 and #857 must settle before structural movement. |
| [`harness.rs`](../crates/coven-cli/src/harness.rs) | 6,264 / 2,506 | 231,228 | supported-harness policy, adapter selection, launch validation and construction | **High policy sensitivity, narrower scope.** It exposes 26 production functions. Preserve the supported Codex/Claude Code/GitHub Copilot CLI set and keep process ownership in `pty_runner.rs`/the daemon. |
| [`automations/contract/types.rs`](../crates/coven-cli/src/automations/contract/types.rs) | 3,432 / 3,432 | 110,686 | canonical wire/domain types and validation | **Medium.** Large but cohesive contract code. Split only by stable protocol ownership, not line count. |

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

- `handle_request_with_runtime_and_authority` selects more than 70 route
  operations and is the only production dispatch call from the daemon.
- the API's Threads proposal path combines transport authority, Ward policy,
  filesystem evidence, durable decision state, recovery, and error mapping;
- `LiveSessionRuntime` and daemon startup combine process ownership with
  transport and scheduler lifecycle;
- the store offers one broad command/query surface over unrelated domains while
  also owning schema migration and process-global health/initialization caches;
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

All three preserve the public HTTP API, daemon socket protocol, and caller
imports through narrow re-exports.

## Prioritized bounded extractions

Each movement needs positive and negative characterization before code moves.
The ranking accounts for current dependency blockers as well as inherent risk.

| Priority | Boundary | Characterization and stable contract | Must not change | Readiness |
| ---: | --- | --- | --- | --- |
| 1 | **Threads proposal coordinator out of `api.rs`** | Existing proposal/Ward tests in `api.rs`, `threads_gate.rs`, the public `/threads/proposals` API, Ward audit records, and the real-daemon work in #884 | transport-owner gate, decision/error precedence, lock ordering, audit reservation, filesystem recovery, fail-closed uncertainty | **Blocked.** #885-#888 must settle identity materialization, terminal close, protected-proposal rejection, and scheduler/recovery behavior first. Moving the code now would freeze disputed contracts. |
| 2 | **Session route family out of `api.rs`** | `SessionRuntime`, request-adoption and execution-binding tests, API contract docs, session lifecycle integration tests, Windows daemon lifecycle tests | launch/input/kill adoption semantics, event ordering, status/error payloads, lock release before runtime calls, crash/restart behavior | **Characterization in progress.** Use #884's real-daemon harness before moving the full family. |
| 3 | **Store initialization/schema from runtime commands and queries** | `open_store`, `initialize_store`, `open_initialized_store`, migration/compatibility tests, store health and smoke tests | schema and migration order, transaction boundaries, per-request no-DDL path, function signatures, and policy remaining outside persistence | **Best independent next slice.** First move schema/migration ownership behind the existing three-function facade; do not reorganize domain queries in the same PR. |
| 4 | **Daemon transport accept loops from live-session supervision** | daemon inline tests, Unix/TCP request tests, `windows_daemon_lifecycle.rs`, stop/restart budget and recovery tests | single-writer lifecycle locks, owner-derived request authority, bounded in-flight handling, shutdown cleanup, live-session registry semantics | Ready only as separate platform-complete slices; Unix and Windows must keep equivalent outer behavior. |
| 5 | **Process containment/supervisor from adapter and stream code in `pty_runner.rs`** | strict-containment tests, native Windows lifecycle tests, piped/PTY integration tests, timeout/cancellation tests | before-first-instruction ownership, kill-on-close/process-group guarantees, receipt protocol, terminal callback ordering | Characterize the supervisor protocol as one unit before moving it. Never split Unix and Windows into contracts that can drift. |
| 6 | **Ward pure classification/budget code from verified apply engine** | Ward unit tests, direct/proposal API tests, hostile replacement and rollback tests | the bounded set of direct, Threads-approved, and coherence-approved Ward entry points; all-or-nothing disposition; path confinement; audit evidence; cleanup uncertainty | Defer until #924's platform guarantee is resolved. Pure helpers may move; private atomic-write helpers must not become callable outside the Ward engine. |
| 7 | **Automations runner lifecycle sub-boundaries** | #856/#857 state-machine, crash, fencing, cancellation, authority, and receipt tests | adoption uncertainty, scheduler fences, retry/cancel semantics, terminal evidence, Rust authority ownership | Defer structural work until active correctness hardening lands. |

Travel, scheduler, AFS, and cockpit route groups are easier to move but rank
below these boundaries because they do not currently dominate authority review
risk. They are suitable fallback slices only when a higher-risk boundary is
blocked and the move does not create a second policy path.

## Where new behavior belongs

- **Route/version policy** (new route arm, version bump, gate rejection):
  `api_routes.rs` for the gate and the `api.rs` dispatch for the arm. Never use
  a helper that bypasses the gate.
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
