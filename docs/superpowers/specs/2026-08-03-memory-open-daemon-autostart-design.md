# Memory Open Daemon Auto-Start Design

## Context

The Coven Memory dashboard is already distributed as a packaged npm
dependency. Users do not need the `coven-memory` source repository to run it.
However, `coven memory open` currently launches only the dashboard process.
When the local Coven daemon is stopped, the dashboard starts successfully but
its `/api/memory` proxy cannot reach the daemon and returns
`503 memory_unavailable`.

The command should establish its complete runtime dependency before opening the
browser.

## Considered Approaches

### 1. Let the Rust CLI own daemon readiness

Resolve the installed dashboard, reuse or start the daemon through the existing
daemon lifecycle API, and launch the dashboard only after its socket is ready.

This is the recommended approach. The CLI already owns the daemon executable,
home directory, lifecycle lock, stale-state reporting, and platform-specific
background startup. It keeps the Node package focused on serving the dashboard.

### 2. Let the dashboard launcher spawn `coven daemon start`

The Node executable could search `PATH` for `coven` and start it before the
server.

This creates bidirectional package coupling, duplicates platform process
handling, and makes the standalone dashboard package responsible for locating
and supervising a separate executable.

### 3. Read canonical memory directly when the daemon is absent

The dashboard could fall back to filesystem reads.

This is rejected because it bypasses the daemon API, duplicates path and
privacy validation, and breaks the established authority boundary.

## Design

### Launch sequence

`memory_dashboard::run_open` performs these steps:

1. Resolve the wrapper-provided dashboard entrypoint, explicit override, or
   `coven-memory-dashboard` executable on `PATH`.
2. Resolve `COVEN_HOME` and the current Coven executable.
3. Call `daemon::ensure_background_server` with the normal lifecycle lock and
   startup timestamp.
4. Launch the packaged dashboard only after the daemon reports its socket
   ready.

The dashboard is resolved first so a missing optional package does not start an
otherwise unnecessary daemon.

### Failure behavior

- Missing dashboard: keep the existing installation guidance and do not start
  the daemon.
- Daemon start or readiness failure: return a contextual terminal error and do
  not launch the dashboard or browser.
- Stale daemon status: preserve the existing explicit restart guidance from
  `ensure_background_server`; do not silently terminate a recorded process.
- Dashboard process failure: preserve the existing launch and exit-status
  errors.
- A daemon that becomes unavailable after launch remains a retryable dashboard
  error; this change guarantees initial readiness, not perpetual availability.

### Testability

Extract a small preparation helper that accepts the resolved launch command and
an injected daemon-readiness closure. Unit tests verify:

- the daemon is ensured before a valid dashboard command is returned;
- a missing dashboard fails without invoking daemon startup; and
- daemon startup failure prevents dashboard launch preparation.

Existing resolution tests continue covering wrapper, override, and `PATH`
precedence. Existing daemon lifecycle tests remain the authority for
start/reuse/readiness behavior.

## Documentation

Update the CLI-facing Memory documentation to state that
`coven memory open` starts or reuses the local daemon automatically. Direct
execution of `coven-memory-dashboard` remains an advanced entrypoint that
requires a current daemon.

## Validation

1. Run focused `memory_dashboard` unit tests.
2. Run daemon lifecycle tests used by `ensure_background_server`.
3. Run the CLI test suite and workspace Clippy with warnings denied.
4. Run workspace tests, preserving only documented unrelated skips if needed.
5. Verify a temporary Coven home can execute `coven memory open` with a fake
   dashboard launcher and observe daemon readiness before launcher execution.

## Non-Goals

- Bundling the dashboard UI into the Rust binary.
- Starting the daemon from direct `coven-memory-dashboard` execution.
- Automatically restarting a stale or incompatible running daemon.
- Direct filesystem fallback in the dashboard.
