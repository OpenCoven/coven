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

The HTTP handlers for accepted input, kill, and targeted cast events previously
wrote directly to `Store::insert_event`. Routing their accepted events through
the writer closes that bypass, but claiming the writer boundary only after
`send_input` or `kill_session` returns is still too late. Output can arrive
during input transport and join the preceding episode, while a fast process
exit can persist `exit` before the handler persists `kill`.

## Considered Approaches

### 1. Reserve input boundaries and coordinate kill/exit ordering (selected)

Add an `EventWriter` reservation that detaches the current truncation episode
before input transport begins. Successful input commits the detached marker and
event together. Failed input cancels the reservation and merges the detached
episode back into any drops observed during the failed action.

Kill keeps the existing non-blocking runtime guarantee: it is issued without
waiting for a blocked input transport. A per-session exit-order guard spans the
kill action, killed-status update, and durable kill event. The process exit
callback takes the same guard before recording `exit`, so exit cannot overtake
an accepted kill. Targeted cast has no separate runtime side effect and keeps
the existing synchronous `EventWriter::record` path.

This design preserves one ordering authority without making a hung input prevent
the daemon from issuing kill.

### 2. Serialize every action under the writer claim

The handler could claim the existing critical boundary before input or kill and
hold it until the action finishes. This is simpler, but a blocked input would
prevent the kill action itself from running, violating the live-session
recovery contract.

### 3. Coordinate only kill and exit

A per-session kill/exit mutex alone prevents exit from overtaking kill, but it
does not stop output observed during `send_input` from joining the preceding
truncation episode. It fixes only half of the concurrency gap.

## Design

### Runtime persistence contract

`SessionRuntime` adds an object-safe
`with_session_event_boundary(session_id, kind, payload, action)` operation that
returns either:

- a handled `Result` after writer-backed persistence; or
- an explicit unhandled result when the runtime has no shared writer.

The `action` argument is an API-owned `&mut dyn FnMut() -> Result<()>`. This
keeps transport calls and store status updates in the API while allowing the
daemon runtime to place synchronization around them. The trait default returns
unhandled; the API then executes the action followed by its existing direct
insert. A handled runtime owns the action and persistence attempt completely,
and the API must not retry either operation or fall back after an error.

For input, the daemon implementation begins a writer reservation before calling
the action. The reservation owns the detached `OutputTruncation`, keeps the
session in boundary mode, and exposes consuming `commit` and `cancel`
operations.

Cancellation restores the detached episode without manufacturing a marker. If
output arrived while the action was pending, cancellation merges counts and
bytes into that later episode and preserves the earlier `created_at`. Commit
constructs the marker through the existing marker builder, queues
`output_truncated -> input`, waits for durable acknowledgement, and then
releases the boundary. A consumed or dropped reservation cannot commit twice.

The existing `EventWriter::record` path remains critical and synchronous for
targeted cast and other action-free boundaries. Its marker construction remains
the sole implementation of `output_truncated`.

### Kill and exit coordination

Each live session registration owns an exit-order guard shared by
`with_session_event_boundary` for `kill` and the detached process observer. The
API action closure contains both `kill_session` and the killed-status update.
The daemon runtime:

1. acquires the guard;
2. invokes the API action, which issues the runtime kill without waiting for an
   input reservation and updates the durable status to `killed`;
3. records the critical `kill` event through the writer; and
4. releases the guard.

The exit callback acquires the same guard before cleanup and
`EventWriter::record_exit`. A fast child exit therefore waits until the kill
event is durable. A blocked input may delay kill-event persistence, but it does
not delay issuing the kill that unblocks or terminates the child.

### Accepted-event ordering

Each accepted event uses the ordering appropriate to its side effect:

1. `session.input` reserves the boundary, performs transport I/O, then commits
   `input`; a transport failure cancels the reservation;
2. `session.kill` uses the exit-order guard around kill, status, and durable
   `kill` persistence; and
3. targeted cast dispatch synchronously persists `cast`.

The action still occurs before its accepted event, matching current behavior.
A failed action produces no accepted event. A writer persistence failure
remains an explicit handler failure after the action, as direct store-write
failures do today; the API must not report success-shaped output or silently
reinsert the event through the store.

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
coalescing, capacity limits, or event payload schemas. It strengthens lifecycle
ordering by sharing one per-session guard between accepted kill and exit.

## Failure Behavior

- A writer failure is returned to the API handler and is never replaced with a
  direct insert, avoiding duplicate or out-of-order events.
- An input transport failure cancels its reservation and restores the complete
  pending truncation episode; it persists neither `input` nor a boundary marker.
- A writerless runtime continues to use the existing direct store insert,
  preserving its current error behavior.
- Kill is issued even while input transport is blocked. Exit persistence waits
  for the accepted kill event rather than overtaking it.
- Critical-event queue pressure waits for durable capacity and commit
  acknowledgement; it is not lossy.
- Raw PTY output remains the only lossy event class. The existing health
  counters continue to describe all rejected output.

## Verification

Retain source-backed regressions that create one dropped-output episode, accept
a direct event, create a second episode, then recover output. Cover input, kill,
and targeted cast independently. Each test asserts:

1. the first `output_truncated` marker is immediately before the direct event;
2. the recovered output is preceded by a second marker;
3. the two markers carry only the drops from their respective episodes; and
4. no event crosses the direct-event boundary.

Add deterministic concurrency regressions:

1. block input transport after reservation, inject output, release input, and
   prove that output belongs to the later episode;
2. fail input after output arrives and prove cancellation merges both drop
   totals with no marker or input event;
3. block input, issue kill concurrently, and prove the runtime kill executes
   before input is released; and
4. trigger process exit during kill and prove durable order is `kill -> exit`.

Retain focused writer tests for critical record ordering and run the relevant
API/daemon tests plus the repository's required formatter, lints, workspace
tests, secret scan, and privacy guard before merge.
