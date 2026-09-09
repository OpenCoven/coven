---
summary: "Run the real-daemon Threads E2E harness and inspect its sanitized evidence."
read_when:
  - Testing the Coven daemon against coven-threads-core
  - Diagnosing a Threads E2E journey failure
title: "Threads real-daemon E2E"
description: "Source-adjacent reference for the real-daemon Threads integration harness, local dependency override proof, and sanitized failure artifacts."
source_adjacent_reason: "Tracks the real daemon test fixture and evidence contract implemented in this repository."
---

# Threads real-daemon E2E

The `threads_e2e` integration target exercises the Ward/Threads boundary
through a real `coven daemon` process and its local IPC transport. Each test
uses a unique temporary `COVEN_HOME`, familiar workspace, SQLite store, pending
directory, and daemon socket.

Run the complete available Unix daemon journeys from a Coven checkout:

```sh
cargo test --locked -p coven-cli --test threads_e2e --features threads-test-clock -- --nocapture
```

The feature-enabled target covers bounded human approval with validation and
write receipts, protected-route refusal, reviewed drift, canonical retired-Ward
migration and intake, exact visibility/deadline boundaries, all five typed
window terminals, and no-window human approval. It also covers restart with
changed or unavailable identity, principal binding, materialized surfaces,
regional approval policy, and inconsistent human-path/opened-window history.
Unsupported corpus input and invalid identity are exercised at intake.

These tests use owner-local IPC, the strongest current supported authorization
path. A supplied fingerprint does not grant protected-write authority. The
suite does not certify a signed principal-authorization profile, changed runtime
bindings, multi-file atomic visibility, or Cave acceptance. It is bounded
process-boundary evidence, not complete closure of every assertion in
[OpenCoven/coven#884](https://github.com/OpenCoven/coven/issues/884).

The clock feature is disabled in production builds. It uses a capability-gated
fixture in each disposable home and explicit real-scheduler ticks, not sleeps
or a mock scheduler. See [Threads test clock](../design/threads-test-clock.md).
Without the feature, Cargo runs only the smaller smoke/lifecycle subset. CI
executes the feature-enabled target on Linux and on the existing macOS push
lane; this Unix-socket harness does not provide Windows daemon-journey proof.

## Testing a local Threads checkout

A downstream Threads job overlays its checkout of `coven-threads-core` into the
pinned Coven revision. Set the following variable to make the test fail before
daemon startup unless `cargo metadata` proves that a local path package is
active:

```sh
COVEN_THREADS_E2E_REQUIRE_LOCAL_OVERRIDE=1 \
  cargo test --locked -p coven-cli --test threads_e2e --features threads-test-clock -- --nocapture
```

The downstream job remains responsible for applying its ephemeral Cargo patch
and updating the ephemeral lock state before invoking the locked test command.
Normal Coven runs exercise the repository's reviewed Git-pinned Threads
revision.

## Evidence

Every journey writes JUnit to:

```text
target/e2e-artifacts/<run-id>/junit.xml
```

On failure, the same directory also contains the synthetic request and
response, daemon recovery log, Ward audit rows, hashed pending/workspace
inventories, and SQLite schema. Setup failures emit the same paths with explicit
unavailable markers. Once dependency preflight completes, `manifest.json`
includes exact Coven and Threads revisions and
`local_threads_override_active`; earlier failures record those fields as null.
The fixtures use only synthetic identities and content; evidence never reads a
developer's real Coven home.
