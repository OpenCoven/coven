# Restricted worker startup/control

This std-only library implements the isolated controller slice of
OpenCoven/coven#1004. It is executable sequencing and lifecycle logic, **not an
OS sandbox, a permission grant, or evidence of kernel enforcement**. There is
no production backend, harness, provider call, service, thread, process launch,
filesystem inspection, or restriction installation in this crate.

## Executable library behavior

`Controller<D: Driver, C: Clock>` takes unique ownership of one driver value and
one clock. It stores a separate, immutable copy of the caller's reviewed
`Binding`: opaque backend, attempt, worker, workspace, executable, sealed runtime
closure, and controller-stdio-pipe identities. `Binding` is ordinary trusted
backend metadata; constructing it or matching its fields proves no permissions
or physical identity. Its debug representation omits identifiers.

`start(duration_ms)` consumes the single attempt, including on failure. It
establishes a lease but does not run the target. A healthy caller then invokes
`advance()` four times:

| Call | Driver operations | Resulting state |
| --- | --- | --- |
| `start` | Check clock, binding, stop signal | `Preparing` |
| First `advance` | Prepare resources | `Installing` |
| Second `advance` | Install restrictions | `RetainingLifetime` |
| Third `advance` | Retain independent lifetime ownership | `Revalidating` |
| Fourth `advance` | Physically revalidate, immediately execute/release | `Running` |

The fourth call has no caller-controlled pause between revalidation and
execution. Clock, binding and cancellation checks bracket every startup driver
operation, including the two operations inside that call. A failed operation,
bad receipt, changed binding or fence stops all later startup operations.
Restriction installation precedes target execution, not just prompt delivery.

Every request has an increasing, non-wrapping sequence, operation and original
binding. The controller rejects stale, future, reordered and mismatched
receipts. Only its privately owned driver supplies reports; there is no public
acknowledgment/event setter or success/grant parser. Correlation prevents
accidental cross-attempt/phase reporting, **not a lying backend**.

Immediately before calling `Driver::execute`, the controller records
`Execution::PossiblyStarted`. An execute error, bad receipt, interrupted
callback, cancellation, or later cleanup cannot change that history back to
`NotStarted`. `Running` means the execute callback acknowledged the handoff;
it is not independent proof that the target is alive or contained. Duplicate
starts and attempts to advance after handoff or refusal return typed errors.
No automatic retry or relaunch exists.

### Finite lease and explicit control

The lease is non-renewable, monotonic, and inclusive of preparation time.
Durations must be `1..=300_000` milliseconds. Zero, excessive duration, deadline
addition overflow, clock error, clock-domain changes and regression relative
to the latest observation all fence the attempt. Equal readings are allowed.
`now >= deadline` is expired, including the exact deadline. This lease is
distinct from any wire request's wall-clock admission deadline; do not pass
Unix time through `Clock`.

The caller supplies a trusted monotonic `Clock` reporting `Reading { era,
millis }`. Its domain must be the same one used by the backend guardian; e.g.,
both can use a shared `std::time::Instant` origin, checked conversion of elapsed
milliseconds to `u64`, and an immutable domain ID. Report conversion failure
as `ClockError::Overflow`. Do not derive elapsed time from poll counts.

`stop_signal()` returns a clonable, sticky, first-writer-wins `StopSignal`.
`cancel()` and `report_owner_loss()` record typed notifications; repeated
signals return `StopAlreadySignalled`. Requests expose the same signal for the
backend guardian to retain. `report_owner_loss()` does not detect owner death
or kill anything. `poll()` checks fences without advancing startup.
`status()` and `lease()` are bounded snapshots, not live observations.

Neither polling nor a fake/frozen clock enforces kernel termination. This
library does not schedule polls, interrupt a blocked callback, or close the
race between its last check and a real OS release. The backend must do so.

### Cleanup and truthful terminal outcomes

A pre-handoff failure is `Refused` / `NotStarted`. A post-handoff failure or
stop is `Stopping` / `PossiblyStarted`. Neither means terminated.

After a stop notification, call `poll()` (or the pending startup `advance()`)
to apply the fence. On refusal or stopping, explicitly call `request_cleanup()`.
The controller makes **at most one cleanup callback**, including errors,
invalid receipts and unwinding. There is no automatic cleanup in error paths
or drop. Cleanup is also available after confirmed natural termination.
Invalid calls return codes rather than silently succeeding.

`CleanupState::Unknown` means cleanup failed, reported an invalid receipt, or
unwound. `Pending` means a request was acknowledged without a sufficient
release/termination outcome. `Released` can describe resources released before
handoff or after confirmed termination. Even a successful cleanup callback
reporting `Released` during a possibly live attempt only produces `Pending`;
it cannot mark the worker terminated.
The controller retains a matching `Released` report and combines it with later
confirmed termination to report `Released` without repeating cleanup. A merely
pending cleanup or failed cleanup remains pending/unknown; death alone does not
invent evidence of resource release.

Only `observe_termination()` receiving a fresh, matching
`TerminationReport { outcome: Termination::Confirmed, .. }` from the trusted
driver moves a handed-off attempt to `Terminated`. It can observe natural
termination while running or resolve stopping after a failed/ambiguous cleanup.
An observation error or invalid receipt fences a running attempt and remains
retryable **only as another observation**, never execution. Later valid
observations can resolve the original worker despite expiry, a broken clock,
or changed current driver binding. Their receipts must still match the
original attempt/resources, not the replacement.

Confirmed termination does not erase execution history, the first fence
reason, or a previous cleanup failure. Resource cleanup and whole-worker
termination are distinct facts. Duplicate terminal observations return
`InvalidState`; duplicate cleanup calls return `CleanupAlreadyRequested`.
The controller retains one status, lease, sequence and driver, not an event
history. Sequence exhaustion fails closed rather than reusing a receipt.

Errors are enums, not arbitrary driver error strings. Map private backend
diagnostics to `BackendError` without exposing paths, environment contents,
prompts, credentials or resource names. The first fence reason is retained
in `Status`; later operation errors are returned separately. No logging sink
or diagnostic payload is provided.

## Unimplemented trusted backend contract

A reviewed backend must own all physical handles and OS operations. It must
not expose aliased execution authority outside the controller or recycle
attempt/worker identities. It retains resource custody internally through
intermediate failures and cleanup uncertainty; callbacks do not consume
handles out of the driver. All callbacks, including cleanup and observations,
must be bounded. Caught startup, binding-observation and clock-observation
unwinds remain fenced. No panic is swallowed; this is not a general recovery
framework for faulty backend code.

The one worker is **offline**. Preparation must provide a sealed resource
closure under the bound workspace, including the target executable, loader,
libraries, runtime assets and configuration. There are no ambient executable,
runtime, home/configuration or credential reads and no cloud-inference
exception. Inputs do not accept a shell, URL, command line, configurable argv,
environment, mount list, or blanket inherited-descriptor permission.
`Request::initial_environment()` is explicitly empty.
`Request::stdio_pipe_identity()` names only the reviewed controller-owned
stdin/stdout/stderr pipes; all other target descriptors must be closed.

Installation must restrict the **whole target process and descendant lifetime**
before any target code runs. Preparation/install/revalidation/cleanup must not
execute the target as a side effect. Independent lifetime ownership must
survive controller/owner death, including a blocked controller or dropped
driver. Merely cloning `StopSignal`, holding a PID, or stopping a parent is not
sufficient. The backend must arrange autonomous lease expiry, owner-death
fencing and descendant termination, with a final atomic release gate against
its guardian, stop signal, lease and physically held resource identities.

Metadata equality cannot detect a substituted physical resource retaining the
same label. `revalidate` must inspect the retained physical identities and
sealed closure immediately before release, without a pathname reopen race.
Termination observations must cover that exact worker and its descendants,
be causally current for the request, and come from trusted OS/backend evidence,
not client assertions, a sent kill request, or a phase acknowledgment.

`Controller` has no driver-extraction method and no custom `Drop`. Dropping it
normally drops the driver; it neither performs observable cleanup nor claims
termination. A backend's own drop behavior must preserve its independent
guardian obligations. Callers must explicitly fence, request cleanup and
observe termination, or deliberately transfer responsibility to that guardian.

## Why session-policy v1 stays unavailable

Nothing here connects to `coven.session-policy.v1` discovery, capability
negotiation, `SessionRuntime`, automation, PTY runners, or ordinary launches.
Negotiated capability metadata is not kernel enforcement. Without a reviewed
backend and whole-process conformance evidence, v1 remains refusal-only.
There is no support flag or verified-permissions constructor in this crate.

Next integration obligations are to implement and review a real backend,
prove its sealed offline closure and release/guardian/termination behavior,
and coordinate OS conformance with the existing OpenCoven/coven#858 owner.
Only a separately reviewed integration can change production discovery or
launch behavior. The fake sequencing tests here are not that evidence.

## Pure local checks

From the workspace root, with existing Rust tooling:

```sh
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo test -p coven-restricted-runtime --locked
CARGO_BUILD_JOBS=2 CARGO_NET_OFFLINE=true cargo clippy -p coven-restricted-runtime --all-targets --locked -- -D warnings
cargo fmt --check
```

To format this crate without changing other workspace files, use
`cargo fmt -p coven-restricted-runtime`. The tests exercise the real controller
against a deterministic fake driver/clock at the resource/OS seam. They never
run a worker, harness, provider request or restriction installer, or inspect
ambient filesystem resources.
