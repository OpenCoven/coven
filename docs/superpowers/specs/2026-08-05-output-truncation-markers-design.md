# Per-Session Output Truncation Marker Design

**Goal:** Make every raw PTY output gap visible in the affected session's
append-only event stream without weakening the event writer's byte bound or
turning pressure reporting into another event flood.

## Context

The event writer intentionally rejects raw output when its output budget is
full. Daemon health records exact global dropped-event and dropped-byte totals,
but the affected session currently proceeds to later events and exit with no
durable indication that its transcript is incomplete.

The queue reserves 128 KiB for lifecycle and other critical events. Truncation
markers can use that reservation while raw output remains the only lossy event
class.

## Considered approaches

### 1. Close-boundary marker with exact totals

Track one active pressure episode per session in queue state. Each rejected
chunk increments that episode. Before the next accepted event for the same
session, enqueue one `output_truncated` marker carrying the exact episode
totals, then enqueue the accepted event.

This is the recommended approach. It produces one immutable event at the
precise transcript gap, preserves append-only cursor semantics, and cannot
flood the queue during sustained pressure. The marker becomes visible when the
episode closes rather than on the first rejected chunk.

### 2. Immediate marker per queued pressure window

Enqueue a marker on the first rejection and coalesce only while that marker
remains in the queue. This reports pressure sooner, but a sustained episode can
emit repeated markers whenever the worker drains the previous one. Totals are
also split across implementation-dependent queue windows.

### 3. Immediate marker updated in place

Insert one marker at episode entry and update its payload as more chunks are
dropped. This provides early and exact eventual totals, but it makes event rows
mutable after clients may have consumed their sequence cursor. That violates
the event log's append-only contract and creates cache/replay ambiguity.

## Event contract

The marker is an ordinary redacted session event:

```json
{
  "kind": "output_truncated",
  "payload_json": "{\"droppedEvents\":3,\"droppedBytes\":8192}",
  "created_at": "<timestamp of the first rejected chunk>"
}
```

`droppedEvents` counts rejected output callbacks in the episode.
`droppedBytes` counts the rejected UTF-8 payload bytes and excludes queue
accounting overhead.

An episode begins with the first rejected output chunk for a session. It ends
at the next accepted event for that session, including accepted output, a
recorded lifecycle/tool/error event, or exit. Sessions are tracked
independently, so one noisy session does not merge another session's gap.

Adjacent accepted output chunks may continue to coalesce into one `output`
event. The coalesced event retains the first accepted chunk's `created_at`.

## Queue and ordering behavior

The queue owns a map of active truncation episodes keyed by session id.
Rejected output updates only that map and the existing daemon-global health
counters; the PTY drain remains non-blocking.

When an event for that session can be accepted:

1. Remove the active episode from the map.
2. Build a critical `output_truncated` event.
3. Reserve capacity for the marker and accepted event together.
4. Push the marker immediately before the accepted event.

For accepted output, the existing output-budget check leaves the full critical
reservation available, so the small marker cannot make the queue exceed its
total capacity. For critical events and exit, the existing condition-variable
wait accounts for both items before either is queued.

Claiming a critical boundary linearizes that event for its session before any
capacity wait. While the boundary is pending, same-session raw output remains
non-blocking but is rejected into the next pressure episode; it cannot overtake
the saved marker or boundary event. Other critical producers for that session
wait until the owner commits or fails the boundary.

If a maximum-sized critical event and its marker cannot fit together, the
writer queues and synchronously acknowledges the marker first, then queues the
critical event while retaining ownership of the session boundary. This
preserves the prior maximum critical-event size instead of creating an
impossible capacity wait.

The marker has no independent completion channel. A following critical event's
acknowledgement proves both records committed in order because they share the
same single-worker queue and transaction boundary. The oversized sequential
case gives the marker its own internal acknowledgement before the caller's
event is queued.

## Failure behavior

- Writer failure clears queued events and in-memory episodes and remains
  visible through `eventWriter.state = "failed"` and `lastError`.
- Marker serialization uses fixed scalar fields and cannot include raw output.
- Unknown event consumers already ignore non-`output` kinds or render the kind
  generically, so the additive event kind does not require a protocol version
  change.
- Raw output remains lossy; markers, lifecycle events, and exit remain
  non-lossy under queue pressure.

## Testing

Rust tests will prove:

- multiple rejected chunks for one session produce one marker with exact totals;
- the marker is ordered before recovered output;
- exit closes an episode and commits the marker before the terminal event;
- sessions maintain independent episode totals;
- accepted output with no prior drop produces no marker;
- health counters and byte-bounded queue accounting remain correct.

API documentation will define the additive event kind and the existing output
coalescing timestamp behavior.
