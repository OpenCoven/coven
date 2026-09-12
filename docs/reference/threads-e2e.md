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

The `output_auto` cases submit bounded `output-format.json` replacements
through the familiar-edits API, never by planting positive pending envelopes.
They cover both auto variants, explicit null veto, submission commitments,
minimum visibility/deadline and restart, exactly-once approval/veto/rejection,
malformed and mixed-batch refusal, explicit opt-in and protected aliases,
stronger human ceremonies, advisory isolation, failed/unscored regression
gates, valid identity/policy/probe/image drift, and final-commit races.
Interrupted-apply cases crash the real daemon after the persisted intent:
unchanged authority recovers, while unavailable Ward authority is quarantined
without a fabricated terminal.

The separate `output_auto_review` regressions cover outgoing symlinks and
hardlink aliases under auto and human policies, valid/malformed single and
mixed requests, both link-creation directions, and both forbidden Serde
representations as before and after images. Preservation cases cover lexical
and case-only routing, inward symlink staging, canonical hardlink replacement,
and unrelated hardlinked ordinary writes. Symlink-specific cases execute on
Unix; hardlink and representation cases are cross-platform. These are
additional scenarios, not replacements for the original auto/replay journeys.

The Unix `output_auto_chain` cases additionally cover composed symlink chains,
direct and hardlinked destination requests, broken-destination creation, and
directory-link/parent traversal semantics. Auto and human single/mixed requests
must leave files, pending state, and audit unchanged on refusal; unrelated
ordinary controls remain applicable with both live and broken canonical links.
The non-UTF-8 destination case is Linux-only because macOS rejects that fixture
filename before a daemon request can execute.

```sh
cargo test --locked -p coven-cli --test threads_e2e \
  --features threads-test-clock output_auto -- --nocapture
```

The daemon pins the matching published core with `OutputFormatRegion`. To
observe a different current Threads checkout, use the ephemeral local override
described below and require `COVEN_THREADS_E2E_REQUIRE_LOCAL_OVERRIDE=1`.
The fixture does not reinterpret legacy migration as tier-2
authorization. Native platform execution is required separately; compilation
and macOS results do not establish Windows acceptance.

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
Passing scheduled-intake journeys does not close the separately coordinated
production scheduled-publication work in
[OpenCoven/coven#888](https://github.com/OpenCoven/coven/issues/888).

The clock feature is disabled in production builds. It uses a capability-gated
fixture in each disposable home and explicit real-scheduler ticks, not sleeps
or a mock scheduler. See [Threads test clock](../design/threads-test-clock.md).
Without the feature, Cargo runs only the smaller smoke/lifecycle subset. CI
executes the feature-enabled target on Linux, Windows, and the existing macOS
push lane. All platforms run the same feature-enabled journeys through `coven-client`
discovery and authenticated transport; Windows uses the owner-only named pipe.
Additional regressions cover startup-evidence retention, artifact-root
isolation, launch selection, readiness, owned replacement/crash/reaping,
failed-child output, failed-request response pairing, and failed-restart CLI
evidence. A real-process regression injects fatal pipe error 109 before observing
an invalid-store child's natural exit, checking the original transport error,
exit code 1, persisted stderr, and absence of a surviving child together.
These are not extra authority journeys or native Windows execution evidence.
HTTP framing regressions belong to the shared client's tests, not a separate
harness parser. A cross-target compilation or Windows workspace run alone is
not evidence that these journeys executed on Windows.

## Lifecycle deadlines

Production `start` and the complete `restart` operation each use one
five-second deadline for cold readiness. The deadline starts before profile
resolution and lifecycle locking; restart does not renew it after stopping the
old daemon. Standalone `stop` and `status` retain their two-second deadlines.
The startup allowance does not relax authentication, profile matching, or
failure cleanup.

The dedicated Windows CLI fixture captures allowlisted startup phases and
numeric timing fields before temporary-home cleanup on failure. This diagnostic
read uses at most 16 KiB and a one-second caller guard, preserves the original
command result, and reports missing or delayed data explicitly. Its five helper
tests are diagnostic coverage, not additional CLI or authority journeys.

Startup retains the initialized, runtime-guarded SQLite connection through
hub/cache setup, avoiding an intermediate last-connection checkpoint and
reopen. Its one explicit final close still completes before readiness. There
is no lifetime-long keeper connection or deferred shutdown checkpoint.
`store-close-begin` marks that final close. `prior_observer_ms` counts completed
prior checkpoint append attempts, including scheduling inside them; it is not
pure disk time and excludes the current append. A visible line does not prove
its own append returned. The new `store-initialize-end` marks the retained
connection, so its timing is not directly comparable with the older
post-close boundary.

Windows authority journeys serialize admission and retain an owned
`coven daemon serve` child, using the same native helper as the smaller Threads
fixtures. Their fixed 15-second readiness budget starts before process spawn,
so spawn time and deferred probing cannot renew it. It is a fixture hang guard
for cold-store initialization, independent of production CLI lifecycle deadlines.
Only pending transport observations are polled; invalid health, changed
identity, and child-exit/inspection errors fail admission immediately. Readiness
requires the authenticated pipe server PID, health PID, and owner-local pipe to
match the owned child. The child is reaped on shutdown, crash, or setup failure,
even if it never published status.

Owned fixture restart means checked CLI stop followed by a new owned
`daemon serve`; provenance records those operations, not `daemon restart`.
Unix authority journeys default to the CLI launch path. The dedicated same-home
CLI lifecycle journey and failed-restart artifact regression explicitly select
CLI mode on every platform; the native `windows_daemon_lifecycle` deadline
tests also remain separate. Explicit foreground regressions exercise owned
replacement, crash/reaping, and bad-store child-exit/output evidence on both
Unix and Windows.

Both fixtures use `fixtures/threads_admission.rs` for owned child lifecycle and
the fixed readiness budget. Unix owned admission uses the client's bounded,
authenticated lifecycle health probe, then checks the returned PID and socket
against the owned child and expected endpoint. Windows retains its authenticated
pipe server PID plus exact health PID/socket checks.
Unix lifecycle health compares the canonical profile directory and socket
basename, then checks non-symlink socket metadata against the authenticated
endpoint's device/inode. This supports retained private staging hard links
without accepting another reported socket name for the same inode.
Owned crash injection terminates the retained child handle. The shared
admission regressions inject time, pending probes, and child/health identity;
they do not use wall-clock sleeps as authority evidence. Requests are never
retried automatically; a response timeout does not prove a mutation did not
commit.

The shared `admission_rejects_unowned_or_invalid_health_without_retry` regression
subsumes the foreground process/endpoint/health matching cases and additionally
rejects an incorrect authenticated Windows server PID and malformed JSON.
`startup_wait_retries_only_pending_transport_not_identity_or_protocol_errors`
subsumes the foreground pending-transport cases, including Unix-only
not-found/connection-refused observations. Those errors are not retryable on
Windows; identity, permission, partial-response, and raw error-109 failures
remain terminal. Shared regressions also retain natural code-0 exits across
repeated observation, keep failed admission failed after a later successful
process exit, and exercise zero/10-ms remaining settlement and observer errors.

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
Owned-serve stdout/stderr and observed child exit status are included in the
sanitized evidence, including admission failures before status publication.
After fatal readiness failure, the fixture observes only its owned child through
the remainder of the original pre-spawn admission deadline; it never retries
health. If the child remains alive, exact-handle termination and reaping use a
separate absolute 15-second teardown budget shared with fallback cleanup.
Neither settlement nor fallback renews either deadline. Exit evidence is
finalized before an owned start/restart returns failure or startup failure
artifacts are written, so child output and status do not depend on a later
destructor. The original readiness error remains the primary error-chain cause,
with any finalization failure attached as additional context.

Each owned lifecycle event's `owned_child_exit` records the observed status,
optional numeric code, and `origin`: `observed_exit` means the fixture had not
requested termination; `fixture_termination_requested` records an attempt,
not a claim that the request caused the exit. This distinguishes startup exit
from fixture cleanup even when termination races with natural exit.
For owned launches, `command_status` records only a natural exit; it remains
null when termination was requested, even if Windows reports the same code as
a natural startup failure. The actual status and code remain in `owned_child_exit`.
Failed restart commands retain their exit status and CLI output even before a
health probe can run. A request that fails before a JSON response is captured
records a null response, never a response from an earlier request.

Successful valid-identity-drift scenarios also retain
`identity-replay-proof.json`: original and fresh intake documents, queried
submission/opening/terminal audit projections, and the stale proposal's
zero-apply observation before the positive control. These packets supplement
the scenario manifest; they do not replace its source/override provenance.

For a constructed fixture, `manifest.json` includes exact Coven and Threads
revisions and `local_threads_override_active`, even if daemon startup fails;
earlier failures can record those fields as null. Its `daemon_launch` field
distinguishes `owned_foreground` from `detached_cli`; foreground success is not
evidence of meeting the CLI startup deadline. Windows CLI readiness errors
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
