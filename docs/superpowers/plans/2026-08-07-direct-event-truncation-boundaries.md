# Direct-Event Truncation Boundaries Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure accepted live-session input, kill, and targeted cast events
close pending raw-output truncation episodes at their exact event boundary.

**Architecture:** Extend `SessionRuntime` with an optional writer-backed event
operation. `LiveSessionRuntime` delegates it to the daemon-owned
`EventWriter`, while writerless runtimes preserve the direct store insert.
The API calls one helper after a successful live action; that helper never
falls back after a writer failure.

**Tech Stack:** Rust, SQLite via rusqlite, serde_json, existing `EventWriter`,
Rust unit/integration tests.

---

## File Structure

- `crates/coven-cli/src/event_writer.rs` owns byte-bounded queueing,
  per-session truncation episodes, and critical-event ordering. Expose only
  test-visible capacity controls and prove generic direct records split
  episodes.
- `crates/coven-cli/src/api.rs` owns `SessionRuntime`, HTTP action handlers,
  direct-store fallback, and endpoint-level regressions.
- `crates/coven-cli/src/daemon.rs` owns the live runtime implementation that
  has access to the shared daemon `EventWriter`.

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
