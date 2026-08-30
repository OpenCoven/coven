# Authority module inventory

Status: living inventory for [OpenCoven/coven#806](https://github.com/OpenCoven/coven/issues/806)
("reduce Coven authority-module concentration behind stable contracts"). It records
where authority-bearing responsibility is concentrated, the prioritized extraction
order, and where new route, policy, persistence, transport, and mapping changes
belong. Update it when a slice lands or the concentration picture changes.

## Method

Rust modules ranked by lines (`git ls-files '*.rs' | xargs wc -l`), then classified
by responsibility: parsing, validation, authorization/policy, domain service,
persistence, process/PTY lifecycle, transport/API, mapping/serialization, telemetry.
Priority is review risk (fan-in of the dispatch surface, shared mutable state,
security sensitivity), not size alone. Counts include inline `#[cfg(test)]` tests.

## Inventory (as of this writing)

| Module (`crates/coven-cli/src` unless noted) | Lines | Responsibility classes | Review risk |
| --- | --- | --- | --- |
| `api.rs` | ~25.4k | transport/API route dispatch, request parsing, validation, launch policy, domain orchestration (sessions, travel, scheduler, threads, handoffs), persistence access, response/error mapping | Highest. The `handle_request_with_runtime_and_authority` match is the fan-in point of the whole crate (~80 route arms); handlers mix policy, store access, and mapping inline |
| `daemon.rs` | ~12.1k | transport (local socket), request forwarding, process/PTY supervision, runtime ownership, health handshake, telemetry | High. Owns the daemon lifecycle and the only production dispatch call into `api.rs` |
| `store.rs` | ~11.3k | persistence (SQLite schema, migrations, queries), storage health mapping | High. Every authority path funnels through it; queries and schema evolution share one module |
| `pty_runner.rs` | ~9.7k | executor/process lifecycle, PTY I/O, platform transport | High. Security-sensitive process supervision |
| `memory_import.rs` | ~8.9k | parsing of external memory formats, validation, persistence | Medium |
| `tui/chat/app.rs` | ~8.6k | UI state machine, transport client, rendering inputs | Medium (presentation; not authority) |
| `main.rs` | ~8.5k | CLI parsing, command dispatch, setup | Medium |
| `harness.rs` | ~6.3k | adapter policy for supported harnesses, launch construction | High (policy) |
| `ward.rs` | ~3.5k | authorization/policy gates, audit ledger | High (policy core) |
| `hub.rs` | ~2.6k | multi-node registry domain service, persistence, health | Medium |
| `threads_gate.rs` | ~1.6k | proposal adjudication policy | High (policy) |

## Prioritized extraction order

Each extraction must map to an existing contract/test surface and preserve it.
The stable outer contracts are: the public HTTP API (`docs/API-CONTRACT.md`,
`docs/API.md`), the daemon socket protocol (`docs/daemon/`), the SQLite store
schema, and the integration suites (`tests/smoke.rs`,
`tests/stream_json_integration.rs`, `tests/setup_cli.rs`) plus the inline
characterization tests of each module.

1. **Route/version authority gate — done (slice 1).** `ApiRoute`,
   `normalize_api_route`, `split_path_query`, and the route-version constants
   moved from `api.rs` to `api_routes.rs`. Pure parsing; rejection envelopes
   (`404 invalid_request` with `apiVersion`/`supportedApiVersions`,
   `404 not_found`) pinned by tests. No behavior change.
2. **Response/error envelope mapping** (`api_error`, `json_response`,
   `ApiResponse`, error-code precedence) — pure mapping, fan-in from every
   handler; extract behind the same public envelopes.
3. **Health/capability mapping and `RequestAuthority`** — the advertised
   capability surface is an authority decision (`sessionLaunchPolicy` depends
   on it); map it independently of session orchestration.
4. **Sessions route family** — launch/complete/input/kill/handoff/events/log
   handlers share process-lifecycle authority; extract as one bounded family
   only with crash/restart characterization in place.
5. **Threads proposal decision path** — decision locking, failpoints, and
   recovery authorization (`recovery_authorization`, proposal failpoints) are
   policy + persistence interleaved; extract behind the existing ward/threads
   gate contracts.
6. **Travel/scheduler handler groups** — large, mostly independent domains in
   `api.rs`; lower risk, do after the shared seams above exist.
7. **`store.rs` command/query vs. schema/migration split** — keep authority
   decisions out of the persistence layer (no ORM-shaped bypass).
8. **`daemon.rs` transport vs. supervision** — separate socket transport
   plumbing from session supervision state machines.

What will **not** change in any slice: route paths and versions, status codes,
error codes/messages/payload precedence, response payload schemas, the SQLite
schema, the socket protocol, CLI flags, and the fail-closed behavior of ward
gates. Behavioral changes require a separate, separately approved PR.

## Where new behavior belongs

- **Route/version policy** (new route arm, version bump, gate rejection):
  `api_routes.rs` for the gate, the `api.rs` dispatch match for the arm. Never
  in a helper that bypasses the gate.
- **Validation and domain policy**: the module that owns the domain
  (`ward.rs`, `threads_gate.rs`, `harness.rs`, `session_launch.rs`) — not
  inline in transport handlers where avoidable.
- **Persistence**: `store.rs` commands/queries; schema changes stay in its
  migration path. Policy decisions never move into the store layer.
- **Executor/process lifecycle**: `pty_runner.rs` (and daemon supervision);
  never coupled to API formatting.
- **Response/event mapping**: `api.rs` mapping helpers until slice 2 gives
  them their own module; mapping stays pure (no I/O, no policy).

New cross-cutting responsibilities must justify staying inside a large module
rather than joining the extracted contract.

## Metrics

Trends, not vanity LOC targets: largest authority module size; distinct
responsibility classes per selected module; review diff size for
security-sensitive changes; externally observable contracts covered by focused
tests; escaped defects caused by cross-responsibility interactions.
