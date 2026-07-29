# Idle Chat Scheduling Design

## Goal

Make `coven chat` quiet while idle. It must not redraw the terminal or poll
the daemon until terminal input, resize, or session-state changes require work.
Streaming and visible spinner states retain their current responsive cadence.

## Scope

This changes the chat event loop in `crates/coven-cli/src/tui/chat/events.rs`
and its scheduling boundary with `App::tick` in
`crates/coven-cli/src/tui/chat/app.rs`. It does not change the daemon
protocol, persist additional state, or redesign the chat UI.

## Current behavior

The event loop draws before every 100 ms terminal-event poll, then calls
`App::tick`. `tick` advances the spinner and polls session events at about
120 ms. An idle chat consequently redraws and wakes even when nothing visible
can change.

## Decision

Use explicit rendering demand and split the loop into active and idle phases.

### Rendering demand

The terminal draws once at startup, then only when a `needs_redraw` state is
set. Terminal input and resize, visible UI actions, session events (including
final PTY output), and observable poll results/errors/recovery/pause changes
all set it. A successful draw clears it.

### Active phase

While live session output or spinner/status animation is visible, use a bounded
terminal-event wait ending at the next existing approximately 120 ms tick
deadline. At that deadline, advance animation and poll session events as today.
Visible changes mark the UI dirty and are drawn promptly.

This preserves streaming and spinner responsiveness; it does not lower their
cadence.

### Idle phase

When neither live session output nor spinner animation is active, wait
indefinitely for a terminal event with `event::read()`. Do not use the 100 ms
poll timeout or invoke periodic session polling solely to keep the idle loop
awake.

Input and resize wake the loop immediately, mark the UI dirty when applicable,
and cause the next draw. Starting a session or spinner switches back to active
scheduling without waiting for an idle timeout.

### Existing session-event safety behavior

Keep the existing error backoff, failure-streak handling, and API-version
mismatch pause semantics intact. In the active phase these outcomes still
request a redraw when visible. Final session events render before their session
becomes inactive.

## Observable contracts

- A ten-second idle interval performs one initial draw and no periodic daemon
  poll or timer-driven redraw.
- Live streaming and visible spinner states continue ticking and polling at
  roughly the existing 120 ms cadence.
- Key handling, resize, overlays, status transitions, streamed output, and
  final PTY output render without an avoidable extra timer interval.
- Existing session-event error backoff and API-mismatch pause behavior is
  unchanged.

## Test strategy

Introduce a deterministic scheduling seam so tests assert next wait/render
decisions without real-time sleeps. Cover idle scheduling, stream and spinner
deadlines, input/resize/overlay/status/session-event redraws, final output,
and the existing terminal/session error, backoff, and API-mismatch paths. Run
the relevant chat tests plus repository-required format, lint, and locked
workspace test gates.

## Failure handling

Terminal event read or poll failures retain their current propagation. Session
polling retains its recoverable-error/backoff path; this must not create a busy
retry loop or suppress a visible recovery message.
