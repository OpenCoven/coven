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
directory, and owner-local daemon endpoint.

Run the complete available daemon journeys from a Coven checkout:

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
The final-commit journey pauses after authority validation, changes the
authoritative identity bytes, and requires a typed rejection without applying
the candidate writes.

Four additional identity-replay cases each exercise a live deadline and a
restart: changed `IDENTITY.md` bytes, changed `SOUL.md` bytes, changed roster
metadata, and a strengthened but still-satisfied active purpose predicate.
They use supported scheduled intake, not injected proposal envelopes. The
original proposal must have one `EvidenceDiverged` close when deadline replay
is exercised, without applying; its submission/opening/terminal chain must survive
another restart unchanged. A fresh proposal under the changed authority must
then apply successfully, distinguishing valid evidence drift from a failed
identity predicate.

These tests use owner-local IPC, the strongest current supported authorization
path. A supplied fingerprint does not grant protected-write authority. The
suite does not certify a signed principal-authorization profile, changed runtime
bindings, every commit/recovery interleaving, or Cave acceptance. It is bounded
process-boundary evidence, not complete closure of every assertion in
[OpenCoven/coven#884](https://github.com/OpenCoven/coven/issues/884).

The clock feature is disabled in production builds. It uses a capability-gated
fixture in each disposable home and explicit real-scheduler ticks, not sleeps
or a mock scheduler. See [Threads test clock](../design/threads-test-clock.md).
Without the feature, Cargo runs only the smaller smoke/lifecycle subset. CI
executes the feature-enabled target on Linux, Windows, and the existing macOS
push lane. All platforms run the same feature-enabled journeys through `coven-client`
discovery and authenticated transport; Windows uses the owner-only named pipe.
Additional regressions cover startup-evidence retention, artifact-root
isolation, launch selection, readiness, and owned-child cleanup; these are not
extra authority journeys or native Windows execution evidence.
HTTP framing regressions belong to the shared client's tests, not a separate
harness parser. A cross-target compilation or Windows workspace run alone is
not evidence that these journeys executed on Windows.

Windows authority journeys serialize admission and retain an owned
`coven daemon serve` child, using the same native helper as the smaller Threads
fixtures. Their fixed 15-second readiness budget is a fixture hang guard for
cold-store initialization, independent of production CLI lifecycle deadlines.
Only pending transport observations are polled; invalid health, changed
identity, and child-exit/inspection errors fail admission immediately. Readiness
requires the authenticated pipe server PID, health PID, and owner-local pipe to
match the owned child. The child is reaped on shutdown, crash, or setup failure,
even if it never published status.

Windows fixture restart means checked CLI stop followed by a new owned
`daemon serve`; provenance records those operations, not `daemon restart`.
Unix fixture lifecycle commands and the dedicated native
`windows_daemon_lifecycle` CLI start/stop/restart deadline tests are unchanged.
Crash injection terminates the retained owned handle on Windows. The shared
admission regressions inject time, pending probes, and child/health identity;
they do not use wall-clock sleeps as authority evidence. Requests are never
retried automatically; a response timeout does not prove a mutation did not
commit.

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

By default, every journey writes JUnit to:

```text
target/e2e-artifacts/<run-id>/junit.xml
```

The test-only `COVEN_THREADS_E2E_ARTIFACT_ROOT` environment variable replaces
`target/e2e-artifacts` with a caller-selected directory; unique per-scenario
run directories remain underneath it. It does not change the daemon's home,
transport, or authority configuration.

The Windows CI job initializes this variable through `GITHUB_ENV` before cache
restore and all workspace and feature invocations. Its value is
`${{ runner.temp }}/threads-e2e-${{ github.run_id }}-${{ github.run_attempt }}`,
outside the checkout and build cache. The runner context is used only at step
level, where it is supported; the path has no parent-directory traversal
segments, which artifact upload rejects.
The always-run upload independently selects that same run/attempt directory
and fails if no files were produced. A canceled job cannot substitute
historical artifacts restored from the build cache. Separate run attempts use
different roots; multiple test invocations within an attempt share the root
but retain unique scenario directories.
Uploaded artifact names also include the commit, workflow run, and attempt,
so name-based selection distinguishes repeated attempts at the same commit.
For older artifacts with commit-only names, select the exact artifact ID.

Local default directories still accumulate evidence. Inspect each manifest's
revision and result rather than treating aggregate file counts as acceptance
proof. Root isolation does not authenticate provenance or replace the native
test log, and early setup failures can still lack revision metadata.

On failure, the same directory also contains the synthetic request and
response, daemon recovery log, Ward audit rows, hashed pending/workspace
inventories, SQLite schema, and a bounded, sanitized status snapshot. Startup
failures retain the failed CLI or owned-serve event and available fixture evidence before
cleanup; fallback markers do not overwrite captured files. A capture error in
one state source is reported without discarding the others.
Owned-serve stdout/stderr are included in the sanitized daemon log, including
admission failures before status publication.

Successful valid-identity-drift scenarios also retain
`identity-replay-proof.json`: original and fresh intake documents, queried
submission/opening/terminal audit projections, and the stale proposal's
zero-apply observation before the positive control. These packets supplement
the scenario manifest; they do not replace its source/override provenance.

For a constructed fixture, `manifest.json` includes exact Coven and Threads
revisions and `local_threads_override_active`, even if daemon startup fails;
earlier failures can record those fields as null. Production CLI Windows readiness errors
include a bounded last-probe category and observation of the retained child
handle, without changing the startup deadline or identity checks.
The existing recovery log also records fixed-category remaining-budget samples
immediately before and after the launch call, plus elapsed store-initialization
checkpoints (connection configured, Ward complete, runtime schema complete,
main lock acquired, main schema complete, commit complete). Remaining budgets
saturate at zero; they do not renew the deadline. Both launch samples are
written after launch, so their log timestamps are write times, not sample times.
Store checkpoints share the daemon-store elapsed-time origin.

These diagnostics are advisory and add I/O, including inside the main schema
transaction. They can perturb timing; missing markers do not alone distinguish
blocked work, failure, process exit, or a failed diagnostic write. They neither
measure post-cleanup survival nor prove the cause of a startup timeout.
The fixtures use only synthetic identities and content; evidence never reads a
developer's real Coven home.

Both constructed-fixture and early-setup manifests record a generic
reproduction `command` matching the compiled test profile, including
`--features threads-test-clock` when enabled. This is not a capture of the
producer's exact invocation or Cargo overlay; retain the upstream runner's
command and overlay evidence when reproducing a downstream observation.
