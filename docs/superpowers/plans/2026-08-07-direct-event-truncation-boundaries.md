# Direct-Event Truncation Boundaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure accepted live-session input, kill, and targeted cast events
close pending raw-output truncation episodes at their exact event boundary.

**Architecture:** Extend `EventWriter` with a cancellable per-session boundary
reservation and extend `SessionRuntime` with an object-safe action wrapper.
Input reserves before transport I/O and commits or restores its truncation
episode afterward. Kill remains immediately issuable while a per-session guard
orders its durable event before process exit. Writerless runtimes preserve the
direct store path, and no writer failure falls back.

**Tech Stack:** Rust, SQLite via rusqlite, serde_json, existing `EventWriter`,
Rust unit/integration tests.

---

## File Structure

- `crates/coven-cli/src/event_writer.rs` owns byte-bounded queueing,
  per-session truncation episodes, critical-event ordering, and the new
  reservation token that commits or restores one detached episode.
- `crates/coven-cli/src/api.rs` owns `SessionRuntime`, HTTP action handlers,
  phase-specific action/persistence errors, direct-store fallback, and
  endpoint-level regressions.
- `crates/coven-cli/src/daemon.rs` owns the live runtime implementation that
  has access to the shared daemon `EventWriter` and the per-registration
  kill/exit ordering guard.

### Task 1: Lock generic critical-record boundary behavior

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs:25-31`
- Modify: `crates/coven-cli/src/event_writer.rs:109-193`
- Test: `crates/coven-cli/src/event_writer.rs:1229-1410`

- [ ] **Step 1: Write the failing generic-record regression**

Add this test beside `exit_closes_pressure_episode_before_terminal_event`:

```rust
#[test]
fn direct_session_records_split_truncation_episodes() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    writer.record("s-1", "input", json!({ "data": "ls\n" }))?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    assert!(!writer.record_output("s-1", "x".repeat(3072))?);
    writer.record("s-1", "kill", json!({ "status": "killed" }))?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    writer.record("s-1", "cast", json!({ "code": "/handoff" }))?;

    assert!(!writer.record_output("s-1", "x".repeat(2048))?);
    assert!(writer.record_output("s-1", "recovered".to_string())?);
    writer.record_exit(
        "s-1",
        PtyRunResult {
            status: "completed",
            exit_code: Some(0),
        },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(
        events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
        [
            "output_truncated", "input",
            "output_truncated", "kill",
            "output_truncated", "cast",
            "output_truncated", "output", "exit",
        ]
    );
    for (index, expected) in [(0, (1, 2048)), (2, (2, 5120)), (4, (1, 2048)), (6, (1, 2048))] {
        let marker: serde_json::Value = serde_json::from_str(&events[index].payload_json)?;
        assert_eq!(marker["droppedEvents"], expected.0);
        assert_eq!(marker["droppedBytes"], expected.1);
    }
    Ok(())
}
```

- [ ] **Step 2: Run the regression before changing production visibility**

Run:

```bash
cargo test -p coven-cli event_writer::tests::direct_session_records_split_truncation_episodes
```

Expected: PASS; `EventWriter::record` already owns critical marker ordering.
This characterization test protects that invariant before API routing changes.

- [ ] **Step 3: Make the existing small-capacity constructor test-visible**

Change only the visibility of these two existing items; their bodies and
values stay unchanged:

```rust
pub(crate) const RESERVED_CRITICAL_BYTES: usize = 128 * 1024;

pub(crate) fn start_with_capacity(
    coven_home: PathBuf,
    capacity_bytes: usize,
) -> Result<Self>
```

Keep `EventWriter::start` calling `start_with_capacity`. Remove
`#[allow(dead_code)]` from `record` and document that accepted direct
live-session events use this critical path.

- [ ] **Step 4: Run all event-writer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
```

Expected: PASS, including the new generic-record episode-splitting regression.

- [ ] **Step 5: Commit the writer-boundary characterization**

```bash
git add crates/coven-cli/src/event_writer.rs
git commit -s -m "test: cover direct event truncation boundaries"
```

### Task 2: Route accepted live API events through the shared writer

**Files:**
- Modify: `crates/coven-cli/src/api.rs:225-241`
- Modify: `crates/coven-cli/src/api.rs:370-405`
- Modify: `crates/coven-cli/src/api.rs:2451-2529`
- Modify: `crates/coven-cli/src/api.rs:2531-2588`
- Modify: `crates/coven-cli/src/api.rs:2767-2821`
- Modify: `crates/coven-cli/src/api.rs:3033-3048`
- Test: `crates/coven-cli/src/api.rs:8590-10020`
- Modify: `crates/coven-cli/src/daemon.rs:384-414`

- [ ] **Step 1: Add failing endpoint regressions**

Add a `WriterBackedRuntime` test fixture in `api.rs` test module:

```rust
struct WriterBackedRuntime {
    writer: crate::event_writer::EventWriter,
}

impl SessionRuntime for WriterBackedRuntime {
    fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
        Ok(())
    }

    fn send_input(&self, _: &str, _: &Value) -> Result<()> {
        Ok(())
    }

    fn kill_session(&self, _: &str) -> Result<()> {
        Ok(())
    }

    fn record_session_event(
        &self,
        session_id: &str,
        kind: &str,
        payload: &Value,
    ) -> Option<Result<()>> {
        Some(self.writer.record(session_id, kind, payload.clone()))
    }
}
```

Add a test helper that checks the durable sequence and exact marker totals:

```rust
fn assert_split_boundary(
    home: &Path,
    session_id: &str,
    expected_boundary: &str,
) -> Result<()> {
    let conn = store::open_store(&store_path(home))?;
    let events = store::list_events(&conn, session_id)?;
    assert_eq!(
        events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
        ["output_truncated", expected_boundary, "output_truncated", "output", "exit"]
    );
    for (index, expected) in [(0, (1, 2048)), (2, (1, 3072))] {
        let marker: Value = serde_json::from_str(&events[index].payload_json)?;
        assert_eq!(marker["droppedEvents"], expected.0);
        assert_eq!(marker["droppedBytes"], expected.1);
    }
    Ok(())
}
```

Add tests named `accepted_input_splits_truncation_episodes`,
`accepted_kill_splits_truncation_episodes`, and
`targeted_cast_splits_truncation_episodes`. Each test uses this body pattern:

```rust
let home = tempfile::tempdir()?;
insert_test_session(home.path(), "sess-input")?;
let runtime = WriterBackedRuntime {
    writer: crate::event_writer::EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        crate::event_writer::RESERVED_CRITICAL_BYTES + 1024,
    )?,
};
assert!(!runtime.writer.record_output("sess-input", "x".repeat(2048))?);
let response = handle_request_with_runtime(
    "POST",
    "/sessions/sess-input/input",
    home.path(),
    None,
    Some(r#"{"data":"ls\n"}"#),
    &runtime,
)?;
assert_eq!(response.status, 202);
assert!(!runtime.writer.record_output("sess-input", "y".repeat(3072))?);
assert!(runtime.writer.record_output("sess-input", "recovered".to_string())?);
runtime.writer.record_exit(
    "sess-input",
    crate::pty_runner::PtyRunResult {
        status: "completed",
        exit_code: Some(0),
    },
)?;
assert_split_boundary(home.path(), "sess-input", "input")?;
```

Repeat the pattern with these endpoint-specific substitutions:

- `sess-kill`, `POST /sessions/sess-kill/kill`, `None`, and `"kill"`;
- `sess-cast`, `POST /cast`, `Some(r#"{"code":"/handoff","target":"sess-cast"}"#)`,
  and `"cast"`.

For every test, retain the writer capacity, first `2048`-byte rejected output,
second `3072`-byte rejected output, recovered output, terminal exit, and
`assert_split_boundary` call shown above. Each test therefore proves the
boundary marker is immediate and later pressure is a distinct episode.

- [ ] **Step 2: Run the endpoint regressions to confirm the present bug**

Run:

```bash
cargo test -p coven-cli truncation_episodes
```

Expected: FAIL. Current handlers call `insert_event` directly, so the first
`output_truncated` marker is not immediately before the direct event.

- [ ] **Step 3: Add the optional runtime persistence operation**

In `SessionRuntime`, add the default writerless operation:

```rust
fn record_session_event(
    &self,
    _session_id: &str,
    _kind: &str,
    _payload: &Value,
) -> Option<Result<()>> {
    None
}
```

In `LiveSessionRuntime`'s trait implementation, add:

```rust
fn record_session_event(
    &self,
    session_id: &str,
    kind: &str,
    payload: &Value,
) -> Option<Result<()>> {
    self.event_writer
        .as_ref()
        .map(|writer| writer.record(session_id, kind, payload.clone()))
}
```

Do not change `NoopSessionRuntime` or existing test runtimes; the trait
default retains their direct-store behavior.

- [ ] **Step 4: Centralize the writer-first API helper**

Immediately below `insert_event`, add:

```rust
fn record_direct_session_event(
    runtime: &dyn SessionRuntime,
    conn: &rusqlite::Connection,
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    match runtime.record_session_event(session_id, kind, &payload) {
        Some(result) => result,
        None => insert_event(conn, coven_home, session_id, kind, payload),
    }
}
```

This `Some(result) => result` branch is required: a failed writer must remain
an error and may not silently reinsert the event through the store.

- [ ] **Step 5: Replace all three accepted live-event direct inserts**

Apply the helper only after the existing action succeeds:

```rust
// record_input success branch
record_direct_session_event(
    runtime, &conn, coven_home, session_id, "input", payload,
)?;

// kill_session after update_session_status
record_direct_session_event(
    runtime, &conn, coven_home, session_id, "kill",
    json!({ "status": "killed" }),
)?;
```

Change the route to pass the runtime and update `submit_cast`:

```rust
("POST", "/cast") => submit_cast(coven_home, body, runtime),

fn submit_cast(
    coven_home: &Path,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse>
```

Within the existing `submit_cast` body, replace its sole
`insert_event(&conn, coven_home, session_id, "cast", ...)` call with:

```rust
record_direct_session_event(
    runtime,
    &conn,
    coven_home,
    session_id,
    "cast",
    json!({ "cast_id": cast_id, "code": code, "target": target }),
)?;
```

Do not route offline reconciliation, handoff bookkeeping, historical import,
or unrelated direct store writes through this helper.

- [ ] **Step 6: Run focused API and writer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
cargo test -p coven-cli truncation_episodes
cargo test -p coven-cli 'api::tests::(input_request|kill_request|post_cast)'
```

Expected: PASS. Existing writerless-runtime tests continue using the default
direct-store path; all three writer-backed endpoint tests show distinct
markers on either side of the accepted boundary.

- [ ] **Step 7: Commit the routing change and regressions**

```bash
git add crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs crates/coven-cli/src/event_writer.rs
git commit -s -m "fix: preserve direct event truncation boundaries"
```

### Task 3: Validate the complete issue contract

**Files:**
- Verify: `crates/coven-cli/src/api.rs`
- Verify: `crates/coven-cli/src/daemon.rs`
- Verify: `crates/coven-cli/src/event_writer.rs`

- [ ] **Step 1: Format the changed Rust sources**

Run:

```bash
cargo fmt --check
```

Expected: PASS. If formatting is needed, run `cargo fmt`, then rerun
`cargo fmt --check`.

- [ ] **Step 2: Run the required Rust quality gates**

Run:

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Expected: PASS with no warnings and no failing workspace tests.

- [ ] **Step 3: Run repository safety gates**

Run:

```bash
python scripts/check-secrets.py
git add crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs crates/coven-cli/src/event_writer.rs
python3 scripts/check-coven-privacy.py --staged
```

Expected: both commands exit successfully. The privacy guard covers only the
implementation changes, not unrelated worktree state.

- [ ] **Step 4: Review the final diff before opening the replacement PR**

Run:

```bash
git diff origin/main...HEAD --check
git diff origin/main...HEAD -- crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs crates/coven-cli/src/event_writer.rs
```

Expected: only #642’s writer-routing, test-support visibility, and regression
coverage are present; no handoff cursor changes from superseded PR #661 are
reintroduced.

### Task 4: Reserve and restore input boundaries

**Files:**
- Modify: `crates/coven-cli/src/event_writer.rs:63-80`
- Modify: `crates/coven-cli/src/event_writer.rs:184-217`
- Modify: `crates/coven-cli/src/event_writer.rs:284-376`
- Modify: `crates/coven-cli/src/event_writer.rs:630-665`
- Test: `crates/coven-cli/src/event_writer.rs:970-1110`

- [ ] **Step 1: Add failing reservation concurrency tests**

Add these tests beside
`critical_boundary_prevents_same_session_output_from_overtaking_marker`:

```rust
#[test]
fn reserved_record_splits_output_arriving_before_action_completion() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "a".repeat(2048))?);
    let reservation = writer.reserve_record("s-1", "input", json!({ "data": "ls\n" }))?;
    assert!(!writer.record_output("s-1", "b".repeat(3072))?);
    reservation.commit()?;
    assert!(writer.record_output("s-1", "recovered".to_string())?);
    writer.record_exit(
        "s-1",
        PtyRunResult { status: "completed", exit_code: Some(0) },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(
        events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
        ["output_truncated", "input", "output_truncated", "output", "exit"],
    );
    assert_truncation(&events[0], 1, 2048)?;
    assert_truncation(&events[2], 1, 3072)?;
    Ok(())
}

#[test]
fn cancelled_record_restores_one_contiguous_output_episode() -> Result<()> {
    let home = tempfile::tempdir()?;
    let conn = store::open_store(&home.path().join(STORE_FILE_NAME))?;
    store::insert_session(&conn, &session("s-1"))?;
    let writer = EventWriter::start_with_capacity(
        home.path().to_path_buf(),
        RESERVED_CRITICAL_BYTES + 1024,
    )?;

    assert!(!writer.record_output("s-1", "a".repeat(2048))?);
    let reservation = writer.reserve_record("s-1", "input", json!({ "data": "ls\n" }))?;
    assert!(!writer.record_output("s-1", "b".repeat(3072))?);
    reservation.cancel();
    assert!(writer.record_output("s-1", "recovered".to_string())?);
    writer.record_exit(
        "s-1",
        PtyRunResult { status: "completed", exit_code: Some(0) },
    )?;

    let events = store::list_events(&conn, "s-1")?;
    assert_eq!(
        events.iter().map(|event| event.kind.as_str()).collect::<Vec<_>>(),
        ["output_truncated", "output", "exit"],
    );
    assert_truncation(&events[0], 2, 5120)?;
    Ok(())
}
```

Add the focused helper in the test module:

```rust
fn assert_truncation(
    event: &store::EventRecord,
    dropped_events: u64,
    dropped_bytes: u64,
) -> Result<()> {
    let payload: serde_json::Value = serde_json::from_str(&event.payload_json)?;
    assert_eq!(payload["droppedEvents"], dropped_events);
    assert_eq!(payload["droppedBytes"], dropped_bytes);
    Ok(())
}
```

- [ ] **Step 2: Run the tests to verify the reservation API is absent**

Run:

```bash
cargo test -p coven-cli event_writer::tests::reserved_record_
cargo test -p coven-cli event_writer::tests::cancelled_record_
```

Expected: compile failure because `reserve_record` and its token do not exist.

- [ ] **Step 3: Add the consuming reservation token**

Add this type after `QueuedEvent`:

```rust
pub(crate) struct EventBoundaryReservation {
    writer: EventWriter,
    session_id: String,
    event: Option<PendingEvent>,
    bytes: usize,
    detached: Option<OutputTruncation>,
}

impl EventBoundaryReservation {
    pub(crate) fn commit(mut self) -> Result<()> {
        let event = self.event.take().expect("reservation event is present");
        let marker = self
            .detached
            .take()
            .map(|truncation| truncation_marker(&self.session_id, truncation));
        let result = self.writer.enqueue_closed_critical(event, self.bytes, marker);
        self.writer.release_boundary(&self.session_id);
        result
    }

    pub(crate) fn cancel(mut self) {
        self.restore();
    }

    fn restore(&mut self) {
        let mut queue = self.writer.lock_queue();
        if let Some(detached) = self.detached.take() {
            restore_truncation(&mut queue, &self.session_id, detached);
        }
        queue.closing_sessions.remove(&self.session_id);
        self.writer.shared.available.notify_all();
        self.event.take();
    }
}

impl Drop for EventBoundaryReservation {
    fn drop(&mut self) {
        self.restore();
    }
}
```

Add the release helper inside `impl EventWriter`:

```rust
fn release_boundary(&self, session_id: &str) {
    let mut queue = self.lock_queue();
    queue.closing_sessions.remove(session_id);
    self.shared.available.notify_all();
}
```

- [ ] **Step 4: Add reservation construction and reuse it from `record`**

Replace `EventWriter::record` with:

```rust
pub fn record(&self, session_id: &str, kind: &str, payload: serde_json::Value) -> Result<()> {
    self.reserve_record(session_id, kind, payload)?.commit()
}

pub(crate) fn reserve_record(
    &self,
    session_id: &str,
    kind: &str,
    payload: serde_json::Value,
) -> Result<EventBoundaryReservation> {
    let record = store::EventRecord {
        seq: 0,
        id: Uuid::new_v4().to_string(),
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        payload_json: serde_json::to_string(&payload)
            .context("failed to serialize event writer payload")?,
        created_at: crate::api::current_timestamp(),
    };
    let bytes = record.payload_json.len().saturating_add(EVENT_OVERHEAD_BYTES);
    anyhow::ensure!(
        bytes <= self.shared.capacity_bytes,
        "critical event exceeds event writer capacity"
    );
    let event = PendingEvent::Record(record);
    let mut queue = self.lock_queue();
    loop {
        if let Some(error) = &queue.failed {
            return Err(anyhow!(error.clone()));
        }
        if queue.closing_sessions.insert(session_id.to_string()) {
            let detached = queue.truncations.remove(session_id);
            return Ok(EventBoundaryReservation {
                writer: self.clone(),
                session_id: session_id.to_string(),
                event: Some(event),
                bytes,
                detached,
            });
        }
        queue = self
            .shared
            .available
            .wait(queue)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}
```

Keep `record_exit` on `enqueue_critical`; it must wait behind an outstanding
input reservation.

- [ ] **Step 5: Refactor marker construction and cancellation merge**

Replace `take_truncation_marker` with:

```rust
fn truncation_marker(session_id: &str, truncation: OutputTruncation) -> QueuedEvent {
    let payload_json = serde_json::to_string(&json!({
        "droppedEvents": truncation.dropped_events,
        "droppedBytes": truncation.dropped_bytes,
    }))
    .expect("truncation marker payload is always serializable");
    let bytes = payload_json.len().saturating_add(EVENT_OVERHEAD_BYTES);
    QueuedEvent {
        event: PendingEvent::Record(store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: "output_truncated".to_string(),
            payload_json,
            created_at: truncation.created_at,
        }),
        bytes,
        completion: None,
    }
}

fn take_truncation_marker(queue: &mut Queue, session_id: &str) -> Option<QueuedEvent> {
    queue
        .truncations
        .remove(session_id)
        .map(|truncation| truncation_marker(session_id, truncation))
}

fn restore_truncation(
    queue: &mut Queue,
    session_id: &str,
    detached: OutputTruncation,
) {
    match queue.truncations.entry(session_id.to_string()) {
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert(detached);
        }
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let current = entry.get_mut();
            current.dropped_events = detached
                .dropped_events
                .saturating_add(current.dropped_events);
            current.dropped_bytes = detached
                .dropped_bytes
                .saturating_add(current.dropped_bytes);
            current.created_at = detached.created_at;
        }
    }
}
```

- [ ] **Step 6: Run all event-writer tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::
```

Expected: PASS, including reservation commit, cancellation merge, existing
critical ordering, capacity, failure, and exit tests.

- [ ] **Step 7: Commit the reservation primitive**

```bash
git add crates/coven-cli/src/event_writer.rs
git commit -s -m "fix: reserve direct event truncation boundaries"
```

### Task 5: Coordinate input actions and kill/exit ordering

**Files:**
- Modify: `crates/coven-cli/src/api.rs:225-260`
- Modify: `crates/coven-cli/src/api.rs:2490-2630`
- Modify: `crates/coven-cli/src/api.rs:3100-3130`
- Modify: `crates/coven-cli/src/daemon.rs:85-175`
- Modify: `crates/coven-cli/src/daemon.rs:384-425`
- Modify: `crates/coven-cli/src/daemon.rs:800-850`
- Test: `crates/coven-cli/src/api.rs:9100-9500`
- Test: `crates/coven-cli/src/daemon.rs:3980-4160`

- [ ] **Step 1: Add the phase-specific boundary result**

Add beside `SessionRuntime`:

```rust
pub enum SessionEventBoundaryError {
    Runtime(anyhow::Error),
    Coordination(anyhow::Error),
    Persistence(anyhow::Error),
}

pub type SessionEventBoundaryResult =
    std::result::Result<(), SessionEventBoundaryError>;
```

Add this object-safe default method to `SessionRuntime`:

```rust
fn with_session_event_boundary(
    &self,
    _session_id: &str,
    _kind: &str,
    _payload: &Value,
    _action: &mut dyn FnMut() -> SessionEventBoundaryResult,
) -> Option<SessionEventBoundaryResult> {
    None
}
```

- [ ] **Step 2: Centralize writer-backed and writerless action execution**

Add below `record_direct_session_event`:

```rust
fn perform_direct_session_event(
    runtime: &dyn SessionRuntime,
    conn: &rusqlite::Connection,
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
    action: &mut dyn FnMut() -> SessionEventBoundaryResult,
) -> SessionEventBoundaryResult {
    match runtime.with_session_event_boundary(
        session_id,
        kind,
        &payload,
        action,
    ) {
        Some(result) => result,
        None => {
            action()?;
            insert_event(conn, coven_home, session_id, kind, payload)
                .map_err(SessionEventBoundaryError::Persistence)
        }
    }
}
```

The `Some(result)` branch must never run the action or persistence again.

- [ ] **Step 3: Move input and kill through the action boundary**

For input, replace the separate `send_input` and event calls with:

```rust
let mut action = || {
    runtime
        .send_input(session_id, &payload)
        .map_err(SessionEventBoundaryError::Runtime)
};
let result = perform_direct_session_event(
    runtime,
    &conn,
    coven_home,
    session_id,
    "input",
    payload,
    &mut action,
);
```

Map `SessionEventBoundaryError::Runtime(error)` through the existing
`NotLiveError`/`send_input_failed` response logic. Return
`Coordination(error)` and `Persistence(error)` as internal errors without a
success-shaped response.

For kill, make the action own both the runtime effect and status update:

```rust
let kill_payload = json!({ "status": "killed" });
let mut action = || {
    runtime
        .kill_session(session_id)
        .map_err(SessionEventBoundaryError::Runtime)?;
    let now = current_timestamp();
    store::update_session_status(&conn, session_id, "killed", None, &now)
        .map_err(SessionEventBoundaryError::Coordination)
};
let result = perform_direct_session_event(
    runtime,
    &conn,
    coven_home,
    session_id,
    "kill",
    kill_payload,
    &mut action,
);
```

Map runtime failures through the existing `NotLiveError`/`kill_failed`
responses. Propagate coordination and persistence errors.

- [ ] **Step 4: Add the daemon input reservation and kill guard**

Add `event_order` to `LiveSessionRegistration`:

```rust
struct LiveSessionRegistration {
    exited: AtomicBool,
    writer: Mutex<Option<crate::maintenance_gate::WriterLease>>,
    event_order: Mutex<()>,
}
```

Initialize it with `Mutex::new(())`.

Implement the new trait method:

```rust
fn with_session_event_boundary(
    &self,
    session_id: &str,
    kind: &str,
    payload: &Value,
    action: &mut dyn FnMut() -> crate::api::SessionEventBoundaryResult,
) -> Option<crate::api::SessionEventBoundaryResult> {
    let writer = self.event_writer.as_ref()?;
    Some((|| -> crate::api::SessionEventBoundaryResult {
        match kind {
            "input" => {
                let reservation = writer
                    .reserve_record(session_id, kind, payload.clone())
                    .map_err(crate::api::SessionEventBoundaryError::Persistence)?;
                match action() {
                    Ok(()) => reservation
                        .commit()
                        .map_err(crate::api::SessionEventBoundaryError::Persistence),
                    Err(error) => {
                        reservation.cancel();
                        Err(error)
                    }
                }
            }
            "kill" => {
                let registration = {
                    let sessions = self.sessions.lock().map_err(|_| {
                        crate::api::SessionEventBoundaryError::Coordination(anyhow::anyhow!(
                            "live session registry lock poisoned"
                        ))
                    })?;
                    sessions
                        .get(session_id)
                        .map(|handle| Arc::clone(&handle.registration))
                        .ok_or_else(|| {
                            crate::api::SessionEventBoundaryError::Runtime(anyhow::Error::new(
                                NotLiveError { session_id: session_id.to_string() },
                            ))
                        })?
                };
                let _event_order = registration.event_order.lock().map_err(|_| {
                    crate::api::SessionEventBoundaryError::Coordination(anyhow::anyhow!(
                        "live session event-order lock poisoned"
                    ))
                })?;
                action()?;
                writer
                    .record(session_id, kind, payload.clone())
                    .map_err(crate::api::SessionEventBoundaryError::Persistence)
            }
            _ => {
                action()?;
                writer
                    .record(session_id, kind, payload.clone())
                    .map_err(crate::api::SessionEventBoundaryError::Persistence)
            }
        }
    })())
}
```

Keep `record_session_event` for targeted cast.

- [ ] **Step 5: Make exit take the same registration guard**

At the start of `on_exit`, before `cleanup.mark_exited()` and
`writer.record_exit`, acquire the registration guard:

```rust
let registration = cleanup
    .as_ref()
    .map(|cleanup| Arc::clone(&cleanup.registration));
let _event_order = registration.as_ref().map(|registration| {
    registration
        .event_order
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
});
```

Keep the guard alive until after `record_exit` returns. This ordering must not
reuse the output `event_closed` mutex because the input/output drain path has
different blocking requirements.

- [ ] **Step 6: Add deterministic blocked-input and exit-race regressions**

Extend the existing `BlockingWriter` test support with an `entered` flag so the
test waits for the write to block instead of sleeping:

```rust
struct BlockingWriter {
    entered: Arc<(StdMutex<bool>, Condvar)>,
    unblock: Arc<(StdMutex<bool>, Condvar)>,
}
```

In `write`, set `entered` to true and notify before waiting on `unblock`.

Replace the sleep in
`kill_session_succeeds_even_while_send_input_is_blocked_on_a_hung_child` with
a wait on `entered`. Exercise input through
`SessionRuntime::with_session_event_boundary`, then exercise kill through the
same boundary. Assert the killer flag becomes true before `unblock` is set.

Add `kill_event_commits_before_concurrent_exit` using
`observer_for_session`, a killer that launches `on_exit` on a separate thread,
and a writer-backed `LiveSessionRuntime`. After the kill boundary returns, join
the exit thread and assert the durable kinds end in:

```rust
["kill", "exit"]
```

Add an API test whose writer-backed runtime fails its input action after a
reservation. Inject dropped output before and during the action, recover output,
and assert one combined marker followed by `output`, with no `input` event.

- [ ] **Step 7: Run focused concurrency tests**

Run:

```bash
cargo test -p coven-cli event_writer::tests::reserved_record_
cargo test -p coven-cli event_writer::tests::cancelled_record_
cargo test -p coven-cli daemon::tests::kill_session_succeeds_even_while_send_input_is_blocked_on_a_hung_child
cargo test -p coven-cli daemon::tests::kill_event_commits_before_concurrent_exit
cargo test -p coven-cli api::tests::failed_input_boundary_restores_truncation_episode
cargo test -p coven-cli truncation_episodes
```

Expected: PASS. The kill flag is set before blocked input is released, durable
kill precedes exit, and failed input restores one episode.

- [ ] **Step 8: Commit runtime coordination**

```bash
git add crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs
git commit -s -m "fix: order direct actions at writer boundaries"
```

### Task 6: Validate the corrected pull request

**Files:**
- Verify: `crates/coven-cli/src/api.rs`
- Verify: `crates/coven-cli/src/daemon.rs`
- Verify: `crates/coven-cli/src/event_writer.rs`
- Verify: `docs/superpowers/specs/2026-08-07-direct-event-truncation-boundaries-design.md`
- Verify: `docs/superpowers/plans/2026-08-07-direct-event-truncation-boundaries.md`

- [ ] **Step 1: Run focused direct-event tests**

```bash
cargo test -p coven-cli event_writer::tests::
cargo test -p coven-cli truncation_episodes
cargo test -p coven-cli oversized_writer_backed
cargo test -p coven-cli writer_failure
```

Expected: PASS with no zero-test filter.

- [ ] **Step 2: Run repository gates**

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
python scripts/check-secrets.py
git add crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs crates/coven-cli/src/event_writer.rs docs/superpowers/specs/2026-08-07-direct-event-truncation-boundaries-design.md docs/superpowers/plans/2026-08-07-direct-event-truncation-boundaries.md
python3 scripts/check-coven-privacy.py --staged
```

Expected: every command succeeds.

- [ ] **Step 3: Review the final branch diff**

```bash
git diff origin/main...HEAD --check
git diff origin/main...HEAD -- crates/coven-cli/src/api.rs crates/coven-cli/src/daemon.rs crates/coven-cli/src/event_writer.rs docs/superpowers/specs/2026-08-07-direct-event-truncation-boundaries-design.md docs/superpowers/plans/2026-08-07-direct-event-truncation-boundaries.md
```

Expected: the branch contains the approved spec, implementation correction,
tests, and plan update with no unrelated changes.
