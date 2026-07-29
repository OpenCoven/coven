# Coven CLI performance baselines

## Purpose

Establish a reproducible, end-to-end measurement harness before changing the
Coven CLI's session, store, capability, or TUI implementations. The harness
must distinguish a stable regression signal from host-specific timing noise.

## Chosen approach

Use a Node 24 runner at `scripts/benchmark-cli.mjs`, following the repository's
existing npm smoke conventions. It receives a prebuilt native binary, builds
isolated `COVEN_HOME` fixtures, invokes the real CLI and daemon socket API, and
writes one JSON report. It does not include build time in any measurement.

This is preferred over Criterion-only benchmarks because the priority paths
include process startup, daemon socket work, and shell shims. It is preferred
over a hard dependency on an external timing program because the runner must
work in the existing GitHub Actions matrix with no global installation.

## Measured scenarios

1. `coven --help` and `coven --version` cold command execution.
2. `coven doctor` against an isolated, intentionally unconfigured home.
3. Daemon-backed session listing and event-tail reads with 100, 1,000, and
   10,000 fixture records.
4. Daemon-launched non-interactive fake-harness session to the first meaningful
   output event.
5. Chat scheduling-model metrics: idle draw/poll wakeups and active-stream
   update cadence, expressed as deterministic counters rather than host CPU
   utilization.

## Fixture and report contract

The runner creates all state below a temporary directory, starts and stops its
own daemon, and never reads a user's real Coven or harness configuration. A
small API fixture client registers synthetic external sessions for session-list
measurements. Event-tail fixtures launch a controlled live fake harness, seed
safe input events through the documented session-input API, advance to the final
bounded event page through cursor reads, then measure that page. The runner must
not write SQLite files directly or add a fixture-only daemon route.

The JSON report includes schema version, commit identifier when available,
platform, runner options, per-scenario sample durations in milliseconds, and
summary values (`min`, `median`, `p95`, `max`). The report may be retained as a
CI artifact or compared locally. It must not contain environment paths, prompt
text, or harness output that could contain private user data.

## CI policy

The runner first validates fixture construction and report shape in CI. CI also
captures the ignored deterministic TUI metric alongside the JSON report. These
publish trend data but do not fail a pull request on wall-clock duration until
repeated per-platform runs establish a stable threshold. Functional, privacy,
secret, and packaging gates remain unchanged.

## Boundaries

The baseline slice adds no production optimization and does not change the
public daemon API. It may add an ignored, test-only chat scheduling metric that
prints deterministic frame/poll counts; it does not alter the event loop.
Pagination, store initialization, capability caching, and idle rendering remain
in their separate dependent issues (#526 through #529).

## Verification

- `node --test scripts/benchmark-cli.test.mjs`
- a focused Rust test for the chat scheduling counters
- `node scripts/benchmark-cli.mjs --binary <prebuilt-coven> --iterations 3`
- existing Rust and JavaScript repository gates before a PR
