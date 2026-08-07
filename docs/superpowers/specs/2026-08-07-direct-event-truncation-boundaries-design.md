# Direct-Event Truncation Boundary Design

**Goal:** Preserve one exact `output_truncated` boundary before every accepted
live-session input, kill, or targeted cast event that follows dropped raw PTY
output.

## Context

The bounded daemon `EventWriter` owns output-pressure episodes by session. Its
critical-event path claims a session, removes any pending episode marker, and
commits that marker immediately before the critical event. Raw output submitted
while a critical boundary is pending is rejected into a new episode, so it
cannot cross the claimed boundary.

The HTTP handlers for accepted input, kill, and targeted cast events currently
write directly to `Store::insert_event`. Those inserts bypass the writer, so a
pending marker can be delayed until later recovered output or exit. Drops on
both sides of the direct event can then be represented as one misleading
episode.

## Considered Approaches

### 1. Route accepted direct events through `EventWriter` (selected)

Add an explicit writer-backed persistence operation to `SessionRuntime`.
Daemon-backed runtimes return a handled result after calling
`EventWriter::record`; writerless runtimes return unhandled so the API uses the
existing direct store insertion.

This reuses the writer's single queue, per-session critical-boundary claim,
commit acknowledgement, and failure propagation. It gives all live events one
ordering authority and is the only approach that prevents a concurrently
arriving output callback from crossing a direct-event boundary.

### 2. Flush a pending marker, then keep direct insertion

The API could expose a marker-only flush before calling `Store::insert_event`.
This repairs the visible ordering but retains two independent persistence
authorities and requires new synchronization to prevent output from arriving
between marker flush and direct insertion.

### 3. Move all event ordering into the store

The store could coordinate output and direct inserts transactionally. This
would duplicate queue pressure accounting and make the non-blocking PTY path
depend on SQLite work. It is broader than the issue and weakens the existing
writer boundary.

## Design

### Runtime persistence contract

`SessionRuntime` gains a method for a non-output live-session event that
returns either:

- a handled `Result` after writer-backed persistence; or
- an explicit unhandled result when the runtime has no shared writer.

The daemon implementation calls `EventWriter::record(session_id, kind,
payload)`. The trait default is unhandled, preserving API tests and
non-daemon runtimes that do not own an event writer. The API helper uses the
direct store insert only for that explicit unhandled case; it must not fall
back after a writer error.

The `EventWriter::record` event remains critical and synchronous. It therefore
uses the existing per-session close/claim protocol and waits for the worker's
commit acknowledgement. Its existing marker construction remains the sole
implementation of `output_truncated`.

### Accepted-event ordering

After the existing action succeeds, each handler persists its accepted event
through the new runtime operation:

1. accepted `session.input` persists `input`;
2. accepted `session.kill` persists `kill`; and
3. accepted targeted cast dispatch persists `cast`.

The action still occurs before its event, matching current behavior. A failed
action produces no accepted event. A writer persistence failure remains an
explicit handler failure after the action, as direct store-write failures do
today; the API must not report success-shaped output or silently reinsert the
event through the store.

When an output-pressure episode is pending for that session, the writer queues
and commits:

```text
output_truncated -> input|kill|cast
```

The event is a critical boundary. Output arriving after the claim cannot join
the preceding episode; later pressure produces a second marker before the
next recovered output or boundary event. Sessions remain independent.

### Compatibility and scope

This is an internal persistence-routing change. Request and response schemas,
event payload shapes, output drop counters, and the append-only event stream
contract do not change. `output_truncated` remains the additive event kind
defined by the existing truncation design.

The change does not route historical import, offline maintenance, or
non-live-session events through the writer. It does not change output
coalescing, capacity limits, or lifecycle/exit routing, which already use the
writer.

## Failure Behavior

- A writer failure is returned to the API handler and is never replaced with a
  direct insert, avoiding duplicate or out-of-order events.
- A writerless runtime continues to use the existing direct store insert,
  preserving its current error behavior.
- Critical-event queue pressure waits for durable capacity and commit
  acknowledgement; it is not lossy.
- Raw PTY output remains the only lossy event class. The existing health
  counters continue to describe all rejected output.

## Verification

Add source-backed regressions that create one dropped-output episode, accept a
direct event, create a second episode, then recover output. Cover input, kill,
and targeted cast independently. Each test asserts:

1. the first `output_truncated` marker is immediately before the direct event;
2. the recovered output is preceded by a second marker;
3. the two markers carry only the drops from their respective episodes; and
4. no event crosses the direct-event boundary.

Retain focused writer tests for its critical record ordering and run the
relevant API/daemon tests plus the repository's required formatter, lints,
workspace tests, secret scan, and privacy guard before merge.
