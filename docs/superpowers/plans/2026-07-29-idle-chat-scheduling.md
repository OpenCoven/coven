# Idle Chat Scheduling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate timer-driven redraws and daemon polls from an idle `coven chat` while preserving the current responsive cadence for active sessions and visible spinners.

**Architecture:** Keep terminal waiting and rendering decisions in `events.rs`; keep session state, poll backoff, and spinner cadence in `App`. `App` exposes whether periodic work is needed, how long until the next 120 ms deadline, and whether a due tick produced a visible change. The event loop draws only when dirty, uses a bounded poll only when periodic work exists, and otherwise blocks in `event::read()`.

**Tech Stack:** Rust, Crossterm, Ratatui, existing `ChatClient` test double, Cargo test/fmt/clippy.

---

## File structure

- Modify: `crates/coven-cli/src/tui/chat/app.rs:218-226, 1676-1743, 2499-2505` — report periodic-work deadlines and visible effects without altering event-poll error/backoff behavior.
- Modify: `crates/coven-cli/src/tui/chat/events.rs:14-278` — use a dirty render flag, select timed versus blocking terminal reads, and update deterministic scheduling metrics.
- Modify: `docs/superpowers/plans/2026-07-29-idle-chat-scheduling.md` — mark completed implementation and verification steps as work lands.

### Task 1: Make App ticks report scheduling and rendering demand

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/app.rs:1676-1743`
- Modify: `crates/coven-cli/src/tui/chat/app.rs:2499-2505`
- Test: `crates/coven-cli/src/tui/chat/app.rs` test module

- [x] **Step 1: Write failing tick-scheduling tests**

Add these tests beside the existing `poll_session_events_backs_off_and_coalesces_repeated_failures` coverage:

```rust
#[test]
fn idle_tick_has_no_deadline_or_daemon_poll() {
    let client = RecordingChatClient::default();
    let (mut app, mirror) = app_with_client(client);
    app.last_tick = Instant::now() - Duration::from_millis(120);

    assert_eq!(app.tick_timeout(), None);
    assert!(!app.tick());
    assert!(!mirror.calls.borrow().iter().any(|call| call.starts_with("events:")));
}

#[test]
fn active_session_tick_polls_without_redrawing_when_nothing_visible_changed() {
    let client = RecordingChatClient::default();
    let (mut app, mirror) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.last_tick = Instant::now() - Duration::from_millis(120);

    assert_eq!(app.tick_timeout(), Some(Duration::ZERO));
    assert!(!app.tick());
    assert!(mirror.calls.borrow().contains(&"events:session-1:0".to_string()));
}

#[test]
fn responding_tick_advances_the_spinner_and_requests_a_redraw() {
    let client = RecordingChatClient::default();
    let (mut app, _) = app_with_client(client);
    app.is_responding = true;
    app.last_tick = Instant::now() - Duration::from_millis(120);
    let frame = app.spinner_frame;

    assert!(app.tick());
    assert_ne!(app.spinner_frame, frame);
}

#[test]
fn active_session_event_requests_a_redraw() {
    let client = RecordingChatClient::default();
    client
        .events
        .borrow_mut()
        .push(output_event(1, "session-1", "final text"));
    let (mut app, _) = app_with_client(client);
    app.active_session_id = Some("session-1".to_string());
    app.last_tick = Instant::now() - Duration::from_millis(120);

    assert!(app.tick());
    assert!(app.messages.iter().any(|message| message.content.contains("final text")));
}
```

- [x] **Step 2: Run the new tests to verify the current boundary fails**

Run:

```bash
cargo test -p coven-cli tui::chat::app::tests --locked -- --nocapture
```

Expected: FAIL to compile because `tick_timeout` does not exist and `tick` returns `()`.

- [x] **Step 3: Implement the explicit App scheduling boundary**

Near the existing `tick` method, add one shared interval constant and these methods:

```rust
const CHAT_TICK_INTERVAL: Duration = Duration::from_millis(120);

pub(super) fn tick_timeout(&self) -> Option<Duration> {
    (self.is_responding || self.active_session_id.is_some())
        .then(|| CHAT_TICK_INTERVAL.saturating_sub(self.last_tick.elapsed()))
}

pub(super) fn tick(&mut self) -> bool {
    if self.tick_timeout().is_none()
        || self.last_tick.elapsed() < CHAT_TICK_INTERVAL
    {
        return false;
    }

    self.last_tick = Instant::now();
    let spinner_changed = self.is_responding;
    if spinner_changed {
        self.spinner_frame = (self.spinner_frame + 1) % SPINNER_FRAMES.len();
    }
    let session_changed = self.poll_session_events();
    spinner_changed || session_changed
}
```

Change `poll_session_events` and `record_event_poll_failure` to return `bool`. Return `false` for no active session, backoff, and API-mismatch pause; return `true` when at least one daemon event is processed or a non-duplicate failure adds its system message. Preserve the existing query, cursor, early break when the active session changes, reset, exponential-backoff, repeated-error coalescing, and API-mismatch message text exactly. Existing callers that poll immediately after a user action may discard the boolean result.

Use `active_session_id.is_some()` rather than only `is_responding` for the deadline: a live `/attach` or `/summon` session must continue receiving output even when it does not show the chat-owned spinner. A no-event active poll must return `false`, so it does not cause a redraw.

- [x] **Step 4: Run App regressions**

Run:

```bash
cargo test -p coven-cli tui::chat::app::tests --locked -- --nocapture
```

Expected: PASS. The focused cases prove idle silence, active-session polling without a frame, visible spinner redraw demand, output-driven redraw demand, and unchanged error/pause behavior.

- [x] **Step 5: Commit the App scheduling boundary**

```bash
git add crates/coven-cli/src/tui/chat/app.rs
git commit -m "perf: expose chat tick scheduling"
```

### Task 2: Replace the unconditional event-loop cadence with dirty rendering

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/events.rs:1-230`
- Test: `crates/coven-cli/src/tui/chat/events.rs:234-278`

- [x] **Step 1: Write failing wait-selection and metric tests**

Replace the current one-case metric assertion with these deterministic contracts:

```rust
#[test]
fn next_terminal_wait_blocks_when_app_has_no_tick_deadline() {
    assert_eq!(next_terminal_wait(None), TerminalWait::Blocking);
}

#[test]
fn next_terminal_wait_uses_the_remaining_active_tick_budget() {
    assert_eq!(
        next_terminal_wait(Some(Duration::from_millis(37))),
        TerminalWait::Timed(Duration::from_millis(37))
    );
}

#[test]
fn schedule_metrics_model_idle_blocking_and_active_tick_cadence() {
    let idle = schedule_metrics(10_000, false);
    assert_eq!((idle.draws, idle.polls), (1, 0));

    let streaming = schedule_metrics(10_000, true);
    assert_eq!((streaming.draws, streaming.polls), (84, 83));
}
```

Keep the ignored `benchmark_schedule_metrics_emit_json` test and update its asserted model rather than changing its JSON marker or field names; the performance workflow consumes that marker as an artifact.

- [x] **Step 2: Run the focused event-loop tests and verify failure**

Run:

```bash
cargo test -p coven-cli tui::chat::events::tests --locked -- --nocapture
```

Expected: FAIL because `TerminalWait` and `next_terminal_wait` do not exist, and the current metrics still report 101 draws and 100 polls for idle.

- [x] **Step 3: Implement timed-or-blocking reads and a dirty render flag**

Introduce this production scheduling seam above `run_event_loop`:

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalWait {
    Blocking,
    Timed(Duration),
}

fn next_terminal_wait(tick_timeout: Option<Duration>) -> TerminalWait {
    match tick_timeout {
        Some(timeout) => TerminalWait::Timed(timeout),
        None => TerminalWait::Blocking,
    }
}

fn read_terminal_event(wait: TerminalWait) -> Result<Option<Event>> {
    match wait {
        TerminalWait::Blocking => Ok(Some(event::read()?)),
        TerminalWait::Timed(timeout) if event::poll(timeout)? => Ok(Some(event::read()?)),
        TerminalWait::Timed(_) => Ok(None),
    }
}
```

Initialize `let mut needs_redraw = true;` before the loop. On each iteration,
draw only when it is true and clear it immediately after `terminal.draw`.
Replace the current `if event::poll(...)` wrapper with
`read_terminal_event(next_terminal_wait(app.tick_timeout()))?`; when it returns
`Some(input_event)`, set `needs_redraw = true` and run the current complete
`Event::Key`/`Mouse`/`Resize`/`Paste` dispatch match against
`input_event`. Preserve that dispatch body byte-for-byte, including
key-release `continue` behavior, quit returns, key bindings, mouse scrolling,
resize handling, paste insertion, and terminal error propagation. Finally, set
`needs_redraw = true` only when `app.tick()` returns true. This gives every
input or resize an immediate next frame, while a timed active poll with no event
and an idle blocking wait never creates a frame.

Revise the test-only `schedule_metrics` helper to model the implementation: inactive mode returns one initial draw and zero polls; streaming mode returns one initial draw plus `duration_ms / 120` tick-driven draws and polls. Keep it test-only and preserve the existing `COVEN_BENCHMARK_TUI=` JSON schema.

- [x] **Step 4: Run focused chat and performance-metric tests**

Run:

```bash
cargo test -p coven-cli tui::chat --locked -- --nocapture
cargo test -p coven-cli --bin coven tui::chat::events::tests::benchmark_schedule_metrics_emit_json --locked -- --ignored --nocapture
```

Expected: PASS. The ignored run emits `COVEN_BENCHMARK_TUI=` with idle `draws: 1`, `polls: 0`, and streaming `draws: 84`, `polls: 83` for 10,000 ms.

- [x] **Step 5: Commit the event-loop change**

```bash
git add crates/coven-cli/src/tui/chat/events.rs
git commit -m "perf: block idle chat event loop"
```

### Task 3: Validate the issue as a single scoped delivery

**Files:**
- Modify: `crates/coven-cli/src/tui/chat/app.rs`
- Modify: `crates/coven-cli/src/tui/chat/events.rs`
- Modify: `docs/superpowers/plans/2026-07-29-idle-chat-scheduling.md`

- [x] **Step 1: Run required local gates**

Run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git diff --check
```

Expected: every command exits 0.

- [x] **Step 2: Stage, run the privacy gate, and record the completed plan**

Run:

```bash
git add crates/coven-cli/src/tui/chat/app.rs crates/coven-cli/src/tui/chat/events.rs docs/superpowers/plans/2026-07-29-idle-chat-scheduling.md
python3 scripts/check-coven-privacy.py --staged
git diff --cached --check
```

Expected: `Coven privacy guard passed` and no whitespace errors. Update only the completed task checkboxes in this plan before staging it.

- [x] **Step 3: Commit and deliver the issue-linked branch**

Run:

```bash
git commit -m "docs: record idle chat scheduling verification"
git push -u origin perf/529-idle-tui
gh pr create --repo OpenCoven/coven --base main --title "perf: block idle chat event loop" --body "Closes #529"
```

Expected: one scoped PR exists for #529. Do not merge until its required checks are green and review feedback is verified.
