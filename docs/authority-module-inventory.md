# Authority module inventory

Status: living inventory for [OpenCoven/coven#806](https://github.com/OpenCoven/coven/issues/806)
("reduce Coven authority-module concentration behind stable contracts"). It records
where authority-bearing responsibility is concentrated, the prioritized extraction
order, and where new route, policy, persistence, transport, and mapping changes
belong. Update it when a slice lands or the concentration picture changes.

## Method

This snapshot was generated from commit
`7928c4e899fa76ee4c46ba8895275988889ddb15` on 2026-09-11. The `store.rs` and
`store/schema.rs` rows were refreshed from
`2bced9c7946d1e65d517c311d47c34ba702437a5` on 2026-09-13; other measurements
retain the original revision. Reproduce the original ranking without changing
your checkout:

```sh
python3 - <<'PY'
import re
import subprocess

ref = "7928c4e899fa76ee4c46ba8895275988889ddb15"
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

To reproduce the refreshed store rows, set `ref` to
`2bced9c7946d1e65d517c311d47c34ba702437a5` and remove the `[:20]` output limit.
The smaller private schema module is shown beside its facade below.

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
| [`api.rs`](../crates/coven-cli/src/api.rs) | 36,410 / 12,496 | 1,373,450 | transport dispatch, parsing, validation, transport authority, domain orchestration, persistence access, response mapping | **Highest concentration.** `handle_request_with_runtime_and_authority` remains the central route dispatcher. Session lifecycle, Ward/Threads proposals, and Automations handlers share the module. The proposal path also coordinates a process-wide Ward audit lock, filesystem artifacts, SQLite state, recovery, and response precedence. |
| [`daemon.rs`](../crates/coven-cli/src/daemon.rs) | 13,210 / 5,833 | 494,030 | local IPC and TCP transport, daemon lifecycle, live session ownership, process supervision, recovery, scheduler startup, telemetry | **High mutable-state risk.** `LiveSessionRuntime` owns the shared live-session registry and launch/shutdown coordination; `serve_forever` acquires lifecycle authority, initializes durable state, starts recovery and schedulers, and accepts concurrent transports. In `handle_http_stream_with_lifecycle`, ordinary API requests not consumed by owner-local lifecycle or mobile local-control handling derive transport authority and enter the central API dispatch. |
| [`store.rs`](../crates/coven-cli/src/store.rs) | 11,785 / 4,939 | 431,136 | initialization facade and cache, query/command persistence, retention, storage health mapping | **High fan-out.** The production portion exposes 107 crate/public free functions. `open_store_with_initialization_observer` delegates schema work to its private `schema` module and retains the initialized connection, while session, handoff, event, Ward, hub, Automations, maintenance, and health queries remain in the parent. The process-global `initialized_store_paths` cache and `STORAGE_HEALTH_SNAPSHOTS` add ownership coupling. |
| [`store/schema.rs`](../crates/coven-cli/src/store/schema.rs) | 1,301 / 1,301 | 52,229 | schema and migration, SQLite connection configuration, compatibility and FTS backfills | **Private initialization boundary.** The parent facade calls this module without changing SQL, migration order, transaction/rollback boundaries, or startup observation order. Initialization returns the live connection without an intervening close/reopen. Successful-initialization caching and domain commands/queries remain in `store.rs`; daemon startup still owns the final close before readiness. |
| [`automations/runner.rs`](../crates/coven-cli/src/automations/runner.rs) | 10,101 / 3,966 | 376,070 | occurrence dispatch, scheduler fencing, runtime adoption, retries, timeout/cancellation, crash recovery, terminal settlement | **High state-machine risk.** `dispatch_occurrence_with_clock` combines direct SQL, scheduler-generation checks, runtime launch, cancellation, and settlement; recovery and stop fencing share the production module. #856 has landed, but remaining #857 correctness work should settle before structural movement. |
| [`pty_runner.rs`](../crates/coven-cli/src/pty_runner.rs) | 9,954 / 5,551 | 367,600 | harness command construction, PTY/piped I/O, stream decoding, platform process containment, timeout/cancellation, supervisor protocol | **High platform/security risk.** Strict child containment, guardian/supervisor protocol, stream adapters, and detached/attached execution share one module. Windows and Unix ownership guarantees converge at `spawn_strict_child_process_tree`, while cancellation also uses process-global signal coordination in `SUPERVISED_STREAM_CANCELLATION_SIGNAL` and `SUPERVISED_STREAM_CANCELLATION_LOCK`. |
| [`main.rs`](../crates/coven-cli/src/main.rs) | 9,028 / 5,542 | 330,896 | CLI parsing and command dispatch, setup, user-facing mapping | **Medium.** High fan-in but mostly presentation/entry-point code. Do not prioritize it ahead of authority-bearing modules merely for size. |
| [`memory_import.rs`](../crates/coven-cli/src/memory_import.rs) | 8,914 / 4,598 | 322,653 | untrusted external-format parsing, normalization, validation, persistence | **Medium-high input risk.** Large because it combines several importer grammars with durable writes. It is not a central authority router, but malformed and cross-format ambiguity need characterization before splitting parsers from persistence. |
| [`ward.rs`](../crates/coven-cli/src/ward.rs) | 8,367 / 4,888 | 309,928 | authorization/policy, path materialization, edit budgets, verified filesystem mutation, rollback/cleanup, audit evidence | **Highest security sensitivity, bounded apply engine.** `Ward::evaluate` is the read-only policy entry point. Direct `Ward::apply` and the two policy-specific approved-apply entry points each re-run fail-closed evaluation before reaching private atomic-write helpers. The file has grown around hostile filesystem races and rollback evidence; decomposition must not expose those lower-level write helpers. |
| [`harness.rs`](../crates/coven-cli/src/harness.rs) | 6,264 / 2,506 | 231,228 | supported-harness policy, adapter selection, launch validation and construction | **High policy sensitivity, narrower scope.** It exposes 26 production functions. Preserve the supported Codex/Claude Code/GitHub Copilot CLI set and keep process ownership in `pty_runner.rs`/the daemon. |
| [`automations/command_adoption.rs`](../crates/coven-cli/src/automations/command_adoption.rs) | 3,711 / 1,580 | 135,827 | command adoption, attempt/executor binding, durable receipts, recovery | **High adoption sensitivity.** Keep the distinction between dispatch and durable adoption explicit; extraction must preserve command identity, binding checks, and non-replay after ambiguous execution. |
| [`automations/occurrences.rs`](../crates/coven-cli/src/automations/occurrences.rs) | 3,469 / 1,629 | 133,189 | occurrence persistence, claim transitions, deduplication, scheduler fencing | **High concurrency risk.** Claims and state transitions share SQL and fencing rules; characterize races and restart behavior before splitting storage from transitions. |
| [`automations/contract/types.rs`](../crates/coven-cli/src/automations/contract/types.rs) | 3,437 / 3,437 | 110,776 | canonical wire/domain types and validation | **Medium.** Large but cohesive contract code. Split only by stable protocol ownership, not line count. |

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

All three preserve the public HTTP API, daemon socket protocol, and caller
imports through narrow re-exports.

## Prioritized bounded extractions

Each movement needs positive and negative characterization before code moves.
The ranking accounts for current dependency blockers as well as inherent risk.

| Priority | Boundary | Characterization and stable contract | Must not change | Readiness |
| ---: | --- | --- | --- | --- |
| 1 | **Threads proposal coordinator out of `api.rs`** | Existing proposal/Ward tests in `api.rs`, `threads_gate.rs`, the public `/threads/proposals` API, Ward audit records, and the real-daemon work in #884 | transport-owner gate, decision/error precedence, lock ordering, audit reservation, filesystem recovery, fail-closed uncertainty | **Blocked.** #885-#888 must settle identity materialization, terminal close, protected-proposal rejection, and scheduler/recovery behavior first. Moving the code now would freeze disputed contracts. |
| 2 | **Session route family out of `api.rs`** | `SessionRuntime`, request-adoption and execution-binding tests, API contract docs, session lifecycle integration tests, Windows daemon lifecycle tests | launch/input/kill adoption semantics, event ordering, status/error payloads, lock release before runtime calls, crash/restart behavior | **Characterization in progress.** Use #884's real-daemon harness before moving the full family. |
| 3 | **Store initialization/schema from runtime commands and queries** | `open_store`, `initialize_store`, `open_initialized_store`, migration/compatibility tests, store health and smoke tests | schema and migration order, transaction boundaries, per-request no-DDL path, function signatures, and policy remaining outside persistence | **Implemented in #1033.** Private `store::schema` owns initialization behind the existing facade. Domain-query decomposition and the broader #806 acceptance criteria remain separate work. |
| 4 | **Daemon transport accept loops from live-session supervision** | daemon inline tests, Unix/TCP request tests, `windows_daemon_lifecycle.rs`, stop/restart budget and recovery tests | single-writer lifecycle locks, owner-derived request authority, bounded in-flight handling, shutdown cleanup, live-session registry semantics | Ready only as separate platform-complete slices; Unix and Windows must keep equivalent outer behavior. |
| 5 | **Process containment/supervisor from adapter and stream code in `pty_runner.rs`** | strict-containment tests, native Windows lifecycle tests, piped/PTY integration tests, timeout/cancellation tests | before-first-instruction ownership, kill-on-close/process-group guarantees, receipt protocol, terminal callback ordering | Characterize the supervisor protocol as one unit before moving it. Never split Unix and Windows into contracts that can drift. |
| 6 | **Ward pure classification/budget code from verified apply engine** | Ward unit tests, direct/proposal API tests, hostile replacement and rollback tests | the bounded set of direct, Threads-approved, and coherence-approved Ward entry points; all-or-nothing disposition; path confinement; audit evidence; cleanup uncertainty | Defer until #924's platform guarantee is resolved. Pure helpers may move; private atomic-write helpers must not become callable outside the Ward engine. |
| 7 | **Automations runner lifecycle sub-boundaries** | #856/#857 state-machine, crash, fencing, cancellation, authority, and receipt tests | adoption uncertainty, scheduler fences, retry/cancel semantics, terminal evidence, Rust authority ownership | #856 has landed; defer structural work until the remaining #857 correctness scope settles. |

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
